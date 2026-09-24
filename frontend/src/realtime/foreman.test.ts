import { describe, expect, it } from 'vitest';
import { ApiError, mapRequestError } from '../api/client';
import type { ConversationDeltaEvent, ForemanSessionMeta, ToolEventEvent } from '../api/types';
import { buildTurns, type TalkTurnsInput } from '../lib/talkTurns';
import type { LedgerRow } from './foreman';
import {
  appendForemanDelta,
  appendForemanTool,
  beginForemanStream,
  emptyForeignActive,
  emptyForemanStream,
  failedLedgerRowIds,
  failForemanStream,
  FOREMAN_LOST_TURN_SUFFIX,
  failureNotice,
  FOREIGN_TTL_MS,
  foreignIsReplying,
  forgetForeignActive,
  FOREMAN_TIMEOUT_SUFFIX,
  isRequestTimeout,
  ledgerOwnsTheFailure,
  maxLedgerId,
  noteForeignDelta,
  pruneForeignActive,
  resolveFollowOutcome,
  settleForemanStream,
  turnLanded,
} from './foreman';

/**
 * 值班长流式归约（票 03 的用例口径）：文本累积的顺序、收尾、断流三件事。
 * 决策 244 又加了两条声道：思考（`reasoning`）与工具调用（`tool_event`）。
 *
 * 与 `reduce.test.ts` 同一姿态的纯函数测试——不触网、不读时钟，故不需要 DOM /
 * fetch 替身就能钉住「不丢字」这条硬约束。
 */

/** 工头增量事件（task_id 空、agent_type = "foreman"，决策 182⑥）。 */
function delta(text: string, sessionId = SESSION): ConversationDeltaEvent {
  return {
    type: 'conversation_delta',
    task_id: '',
    branch: '',
    run_id: 0,
    agent_type: 'foreman',
    session_id: sessionId,
    role: 'assistant',
    text,
    prompt_tokens: 1,
    completion_tokens: 1,
  };
}

/** 工头工具事件（决策 244：带身份串与会话，走同一条过滤）。 */
function toolEvent(
  tool: string,
  phase: 'start' | 'end' | 'error',
  sessionId = SESSION,
  argsSummary = 't-1',
): ToolEventEvent {
  return {
    type: 'tool_event',
    task_id: '',
    branch: '',
    run_id: 0,
    agent_type: 'foreman',
    session_id: sessionId,
    tool,
    phase,
    args_summary: argsSummary,
  };
}

/** 当前班次。增量必须带这个 id 才被接纳（决策 204⑥）。 */
const SESSION = 'sess-1';

/** 班次列表里的一条（只填判据用到的那两列：id 与 `last_active_at`）。 */
function meta(id: string, lastActiveAtMs: number): ForemanSessionMeta {
  return {
    id,
    title: id,
    created_at: new Date(0).toISOString(),
    last_active_at: new Date(lastActiveAtMs).toISOString(),
    archived_at: null,
  };
}

describe('foreman 流式归约', () => {
  it('增量按到达顺序累积，且累积期间保持流式态', () => {
    let state = beginForemanStream();
    expect(state.streaming).toBe(true);

    state = appendForemanDelta(state, delta('夜班'), SESSION);
    state = appendForemanDelta(state, delta('安静，'), SESSION);
    state = appendForemanDelta(state, delta('没有待办。'), SESSION);

    expect(state.text).toBe('夜班安静，没有待办。');
    expect(state.streaming).toBe(true);
    expect(state.error).toBeNull();
  });

  it('非工头增量 / 非增量事件旁落（返回同一 state）', () => {
    const state = appendForemanDelta(beginForemanStream(), delta('已到'), SESSION);
    // 别的任务的增量（agent_type = main）不得拼进值班长的话里
    expect(appendForemanDelta(state, { ...delta('别人的'), agent_type: 'main' }, SESSION)).toBe(state);
    // 同一条流上的其它事件类型与文本无关
    expect(
      appendForemanDelta(
        state,
        {
          type: 'tool_event',
          task_id: '',
          branch: '',
          run_id: 0,
          tool: 'read_task',
          phase: 'end',
          args_summary: 'x',
        },
        SESSION,
      ),
    ).toBe(state);
  });

  it('收尾：非空回话收敛为回话，并熄灭方块光标', () => {
    const streamed = appendForemanDelta(beginForemanStream(), delta('半句'), SESSION);
    const settled = settleForemanStream(streamed, '完整回话');
    expect(settled.text).toBe('完整回话');
    expect(settled.streaming).toBe(false);
    expect(settled.error).toBeNull();
  });

  it('收尾：空 / 全空白回话不清掉已到达的文字', () => {
    const streamed = appendForemanDelta(beginForemanStream(), delta('已到达的部分'), SESSION);
    expect(settleForemanStream(streamed, '').text).toBe('已到达的部分');
    expect(settleForemanStream(streamed, '   \n ').text).toBe('已到达的部分');
    expect(settleForemanStream(streamed, null).text).toBe('已到达的部分');
    // 两者皆空仍是空，不凭空造一句
    expect(settleForemanStream(emptyForemanStream(), '').text).toBe('');
  });

  it('断流：已收到的部分原文保留，只多一个说明', () => {
    let state = beginForemanStream();
    state = appendForemanDelta(state, delta('我查到 develop 工位'), SESSION);
    const failed = failForemanStream(state, '连接中断');
    expect(failed.text).toBe('我查到 develop 工位');
    expect(failed.streaming).toBe(false);
    expect(failed.error).toBe('连接中断');
  });

  it('班次守卫：不是当前班次的增量一律丢弃（决策 204⑥）', () => {
    const state = appendForemanDelta(beginForemanStream(), delta('本班的话'), SESSION);
    // 另一台设备在另一个班次里收到的回话——不得插进这一班
    expect(appendForemanDelta(state, delta('别班的话', 'sess-2'), SESSION)).toBe(state);
    // 流水线的增量为空串，同样不是这一班
    expect(appendForemanDelta(state, delta('流水线的话', ''), SESSION)).toBe(state);
    // 没有当前班次时也一律丢弃：那种状态下屏幕上是空态，接进来会凭空长出一段话
    expect(appendForemanDelta(beginForemanStream(), delta('先到的'), null)).toEqual(
      beginForemanStream(),
    );
    expect(appendForemanDelta(beginForemanStream(), delta('先到的'), '')).toEqual(
      beginForemanStream(),
    );
    // 老客户端（事件里没有 session_id）与当前班次对不上，故也丢弃——宁可少拼一段字，
    // 也不让两台设备的回话混成一段
    const legacy = { ...delta('老事件'), session_id: undefined };
    expect(appendForemanDelta(state, legacy, SESSION)).toBe(state);
  });
});

/**
 * 归约层的其余口径：多班次互不干扰、开新一轮丢残留、失败轮的归属、本地超时不算失败。
 *
 * 这些是决策 182③ / 204 / 211④ / 223 的既有断言，组名只是把它们与上面那两组分开。
 */
describe('foreman 流式归约（续）', () => {
  it('两个班次各说各的：两次独立累积互不影响', () => {
    const a = appendForemanDelta(beginForemanStream(), delta('甲班', 'sess-a'), 'sess-a');
    const b = appendForemanDelta(beginForemanStream(), delta('乙班', 'sess-b'), 'sess-b');
    expect(a.text).toBe('甲班');
    expect(b.text).toBe('乙班');
    // 切了班次之后，上一个班次迟到的尾巴进不来
    const switched = appendForemanDelta(a, delta('甲班的尾巴', 'sess-a'), 'sess-b');
    expect(switched.text).toBe('甲班');
  });

  it('开新一轮：丢掉上一轮的残留（上一轮的文字不得串进这一轮）', () => {
    const previous = appendForemanDelta(beginForemanStream(), delta('上一轮的话'), SESSION);
    expect(previous.text).toBe('上一轮的话');
    expect(beginForemanStream().text).toBe('');
  });

  it('失败轮的归属：台账里新出现的那一条才算这一次（决策 211④）', () => {
    // 判据只看 `kind`（决策 252）：后端说这一行是没跑起来的那一轮，界面不再解析正文。
    const failedRow = (id: number) => ({ id, kind: 'failed' as const });
    const mine = { id: 3, kind: 'mine' as const };
    const consoleRow = { id: 4, kind: 'console' as const };

    // 空台账 / 只有无关行 → 本地那一行要留着（请求根本没到后端时它是唯一信号）
    expect(ledgerOwnsTheFailure([], failedLedgerRowIds([]))).toBe(false);
    expect(ledgerOwnsTheFailure([mine, consoleRow], failedLedgerRowIds([mine, consoleRow]))).toBe(
      false,
    );

    // 失败之后重取到的台账里多出一条 `kind = "failed"` 的行 → 它就是这次的失败轮
    const before = failedLedgerRowIds([mine, consoleRow]);
    expect(ledgerOwnsTheFailure([mine, consoleRow, failedRow(7)], before)).toBe(true);

    // 早先那次失败**不算**：不然一次历史失败会让此后每次真实断网都静默
    const stale = failedRow(5);
    const beforeWithStale = failedLedgerRowIds([stale]);
    expect(ledgerOwnsTheFailure([stale], beforeWithStale)).toBe(false);
    expect(ledgerOwnsTheFailure([stale, failedRow(9)], beforeWithStale)).toBe(true);

    // 只有 `kind = "failed"` 的算：人的话（`mine`）与操作台记的账（`console`）都不作数
    // ——**这正是这一批要的东西**：判据不再看正文里有没有那个字样，看的是后端给的字段。
    expect(ledgerOwnsTheFailure([mine], new Set())).toBe(false);
    expect(ledgerOwnsTheFailure([consoleRow], new Set())).toBe(false);
  });

  it('本地超时不等于这一轮失败：补上「它仍在服务端继续」的实情（决策 223 / 票 06）', () => {
    // 判据按 `kind`（票 06）：生产者是 mapRequestError，它在构造点带上 KIND_REQUEST_TIMEOUT——
    // 走真构造链而不是手写字面量，钉的是「两端共用同一枚 kind」这件事本身
    const timeoutErr = mapRequestError(
      Object.assign(new Error('signal timed out'), { name: 'TimeoutError' }),
      300_000,
      false,
    );
    expect(isRequestTimeout(timeoutErr)).toBe(true);

    const timedOut = failureNotice(timeoutErr.message, isRequestTimeout(timeoutErr));
    expect(timedOut).toContain('请求超时');
    expect(timedOut).toContain(FOREMAN_TIMEOUT_SUFFIX);

    // 网络本身不通（kind 不是超时）→ **不**加这句话：真失败了还说「仍在继续」是在骗人
    const offline = mapRequestError(new TypeError('Failed to fetch'), 30_000, false);
    expect(isRequestTimeout(offline)).toBe(false);
    expect(failureNotice(offline.message, isRequestTimeout(offline))).toBe(offline.message);

    // 字样 spoof：正文以「请求超时」开头、但 kind 不是超时 → 不附——这正是「不按 message
    // 里的字样分支」（api/client.ts 的那条纪律）要挡的形状，也是本票替换掉的旧判据
    const spoof = new ApiError(0, '请求超时（30 秒没有回应）。');
    expect(isRequestTimeout(spoof)).toBe(false);
    expect(failureNotice(spoof.message, isRequestTimeout(spoof))).toBe(spoof.message);

    // 其余失败照原样
    expect(failureNotice('这一轮没跑起来（llm_auth）：密钥不对', false)).toBe(
      '这一轮没跑起来（llm_auth）：密钥不对',
    );
  });
});

describe('别的班次「正在回话」（决策 220③）', () => {
  const T0 = 1_000_000;

  it('不匹配的增量不再白扔：记进映射（串台照旧挡住）', () => {
    const active = noteForeignDelta(emptyForeignActive(), delta('别班的话', 'sess-2'), SESSION, T0);
    expect(active.bySession).toEqual({ 'sess-2': T0 });
    // 同一事件在文本那一侧仍然被丢弃（两道判据各管各的）
    expect(appendForemanDelta(beginForemanStream(), delta('别班的话', 'sess-2'), SESSION)).toEqual(
      beginForemanStream(),
    );
  });

  it('当前班次的那一份**不进这张表**（它由「本机发出未落地」那一支说）', () => {
    expect(noteForeignDelta(emptyForeignActive(), delta('本班', SESSION), SESSION, T0)).toEqual(
      emptyForeignActive(),
    );
    // 没有当前班次、或老客户端那条没有 session_id 的事件，也都不点亮
    expect(noteForeignDelta(emptyForeignActive(), delta('先到的', ''), null, T0)).toEqual(
      emptyForeignActive(),
    );
    expect(
      noteForeignDelta(
        emptyForeignActive(),
        { ...delta('老事件'), session_id: undefined },
        SESSION,
        T0,
      ),
    ).toEqual(emptyForeignActive());
  });

  it('非工头增量与其它事件类型一概不点亮', () => {
    expect(
      noteForeignDelta(
        emptyForeignActive(),
        { ...delta('别人的', 'sess-2'), agent_type: 'main' },
        SESSION,
        T0,
      ),
    ).toEqual(emptyForeignActive());
    // 没有 agent_type 的工具事件（老后端）同样不点亮：认不出来就当作别人的
    expect(
      noteForeignDelta(
        emptyForeignActive(),
        {
          type: 'tool_event',
          task_id: '',
          branch: '',
          run_id: 0,
          tool: 'read_task',
          phase: 'end',
          args_summary: 'x',
        },
        SESSION,
        T0,
      ),
    ).toEqual(emptyForeignActive());
    // 流水线节点的工具事件（带身份但不是我方）也不点亮
    expect(
      noteForeignDelta(
        emptyForeignActive(),
        { ...toolEvent('write_file', 'end', 'sess-2'), agent_type: 'main' },
        SESSION,
        T0,
      ),
    ).toEqual(emptyForeignActive());
  });

  it('工头工具事件也算「在回话」（决策 244）：一轮里它可能查十几次而一个字不说', () => {
    // 决策 224 的实测：一次正常定位 16–17 次工具调用，其间没有任何文本增量。
    // 只认 conversation_delta 的话，那几十秒里「别的班次正在回话」是暗的——而它正忙着。
    const active = noteForeignDelta(
      emptyForeignActive(),
      toolEvent('read_task', 'start', 'sess-2'),
      SESSION,
      T0,
    );
    expect(active.bySession).toEqual({ 'sess-2': T0 });
    // 本班次的那一份照旧不进表
    expect(noteForeignDelta(emptyForeignActive(), toolEvent('read_task', 'start'), SESSION, T0)).toEqual(
      emptyForeignActive(),
    );
  });

  it('静默超时即熄灭（标记说的是「此刻」）', () => {
    let active = noteForeignDelta(emptyForeignActive(), delta('别班的话', 'sess-2'), SESSION, T0);
    expect(foreignIsReplying(active, 'sess-2', T0)).toBe(true);
    expect(foreignIsReplying(active, 'sess-9', T0)).toBe(false);

    // 超时：边界取闭区间（正好 TTL 那一刻还亮着）
    expect(foreignIsReplying(active, 'sess-2', T0 + FOREIGN_TTL_MS)).toBe(true);
    expect(foreignIsReplying(active, 'sess-2', T0 + FOREIGN_TTL_MS + 1)).toBe(false);

    // 清理：没有该清的项时返回同一个对象（组件里那个 5s 的 $effect 靠它不自激）
    expect(pruneForeignActive(active, [], T0 + 1)).toBe(active);
    expect(pruneForeignActive(active, [], T0 + FOREIGN_TTL_MS + 1)).toEqual(emptyForeignActive());

    active = forgetForeignActive(active, 'sess-2');
    expect(active).toEqual(emptyForeignActive());
    // 不在表里的 id 不换对象
    expect(forgetForeignActive(active, 'sess-2')).toBe(active);
  });

  it('落地即熄灭：那一班的 `last_active_at` 走到记下的时刻之后（不必等超时）', () => {
    const active = noteForeignDelta(emptyForeignActive(), delta('别班的话', 'sess-2'), SESSION, T0);
    // 列表还是旧的（那一班的上次活动早于我们记的时刻）→ 仍算在回话
    const stale = [meta('sess-2', T0 - 5_000)];
    expect(pruneForeignActive(active, stale, T0 + 1)).toBe(active);
    // 回话落库会同时更新 `last_active_at`（与消息插入同事务）→ 已经落地，熄灭
    const landed = [meta('sess-2', T0 + 500)];
    expect(pruneForeignActive(active, landed, T0 + 1)).toEqual(emptyForeignActive());
    // 边界：正好等于我们记下的时刻也算落地（同一毫秒收尾）
    expect(pruneForeignActive(active, [meta('sess-2', T0)], T0 + 1)).toEqual(
      emptyForeignActive(),
    );
  });

  it('落地判据只认**那一班**：别的班次的新列表不会把它抹掉', () => {
    const active = noteForeignDelta(emptyForeignActive(), delta('别班的话', 'sess-2'), SESSION, T0);
    const others = [meta('sess-me', T0 + 9_000), meta('sess-9', T0 + 9_000)];
    expect(pruneForeignActive(active, others, T0 + 1)).toBe(active);
  });

  it('两个班次各说各的：一次增量只点亮它自己那一行', () => {
    let active = noteForeignDelta(emptyForeignActive(), delta('甲', 'sess-a'), 'sess-me', T0);
    active = noteForeignDelta(active, delta('乙', 'sess-b'), 'sess-me', T0 + 5);
    expect(active.bySession).toEqual({ 'sess-a': T0, 'sess-b': T0 + 5 });
    expect(foreignIsReplying(active, 'sess-a', T0 + 10)).toBe(true);
    expect(foreignIsReplying(active, 'sess-b', T0 + 10)).toBe(true);
  });
});

describe('思考与工具调用的实时声道（决策 244）', () => {
  it('reasoning 声道进 thinking，不进回话正文', () => {
    const state = appendForemanDelta(
      beginForemanStream(),
      { ...delta('我要查一下'), channel: 'reasoning' },
      SESSION,
    );
    expect(state.thinking).toBe('我要查一下');
    expect(state.text, '思考不得混进回话正文——那是分开两条声道的全部理由').toBe('');
  });

  it('缺省声道按回话处理（老后端不发 channel 字段）', () => {
    const state = appendForemanDelta(beginForemanStream(), delta('缺省就是回话'), SESSION);
    expect(state.text).toBe('缺省就是回话');
    expect(state.thinking).toBe('');
  });

  it('两条声道各攒各的，互不覆盖', () => {
    let state = beginForemanStream();
    state = appendForemanDelta(state, { ...delta('先想'), channel: 'reasoning' }, SESSION);
    state = appendForemanDelta(state, delta('再说'), SESSION);
    state = appendForemanDelta(state, { ...delta('再想一点'), channel: 'reasoning' }, SESSION);
    state = appendForemanDelta(state, delta('再多说一点'), SESSION);
    expect(state.thinking).toBe('先想再想一点');
    expect(state.text).toBe('再说再多说一点');
  });

  it('工具调用：start 与随后的 end 合成一条（它不是两件事）', () => {
    let state = beginForemanStream();
    state = appendForemanTool(state, toolEvent('read_task', 'start'), SESSION);
    expect(state.tools).toEqual([{ tool: 'read_task', args_summary: 't-1', phase: 'start' }]);
    state = appendForemanTool(state, toolEvent('read_task', 'end'), SESSION);
    expect(state.tools).toEqual([{ tool: 'read_task', args_summary: 't-1', phase: 'end' }]);
  });

  it('工具调用：error 也是收尾（这一条调用到此为止）', () => {
    let state = appendForemanTool(beginForemanStream(), toolEvent('read_file', 'start'), SESSION);
    state = appendForemanTool(state, toolEvent('read_file', 'error'), SESSION);
    expect(state.tools).toHaveLength(1);
    expect(state.tools[0].phase).toBe('error');
  });

  it('工具调用：连着查两次同一把工具是两条，不是合并成一条', () => {
    // 合并的判据是「最后一条还没收尾」，不是工具名——同一轮里连查两次是常态（决策 224）
    let state = beginForemanStream();
    state = appendForemanTool(state, toolEvent('read_task', 'start'), SESSION);
    state = appendForemanTool(state, toolEvent('read_task', 'end'), SESSION);
    state = appendForemanTool(state, toolEvent('read_task', 'start', SESSION, 't-2'), SESSION);
    expect(state.tools).toHaveLength(2);
    expect(state.tools.map((t) => t.phase)).toEqual(['end', 'start']);
    expect(state.tools[1].args_summary).toBe('t-2');
  });

  it('非工头工具事件旁落：流水线节点的工具调用不得进对讲台', () => {
    const state = beginForemanStream();
    // agent_type = main（流水线）
    expect(
      appendForemanTool(state, { ...toolEvent('write_file', 'start'), agent_type: 'main' }, SESSION),
    ).toBe(state);
    // 老后端不发 agent_type：缺省空串不等于 "foreman"，故也丢弃（认不出来就当作别人的）
    expect(
      appendForemanTool(state, { ...toolEvent('write_file', 'start'), agent_type: undefined }, SESSION),
    ).toBe(state);
  });

  it('班次守卫在工具事件上同样成立（决策 204⑥）', () => {
    const state = beginForemanStream();
    expect(appendForemanTool(state, toolEvent('read_task', 'start', 'sess-2'), SESSION)).toBe(state);
    expect(appendForemanTool(state, toolEvent('read_task', 'start', ''), SESSION)).toBe(state);
    expect(appendForemanTool(state, toolEvent('read_task', 'start'), null)).toBe(state);
  });

  it('收尾不清掉思考与现场（它们在台账那一行里同样有）', () => {
    let state = beginForemanStream();
    state = appendForemanDelta(state, { ...delta('想过了'), channel: 'reasoning' }, SESSION);
    state = appendForemanTool(state, toolEvent('read_task', 'end'), SESSION);
    const settled = settleForemanStream(state, '完整回话');
    expect(settled.text).toBe('完整回话');
    expect(settled.thinking, '收尾是「流完了」，不是「把刚才发生的事撤掉」').toBe('想过了');
    expect(settled.tools).toHaveLength(1);
  });

  it('断流同样保留思考与现场', () => {
    let state = beginForemanStream();
    state = appendForemanDelta(state, { ...delta('想了半截'), channel: 'reasoning' }, SESSION);
    const failed = failForemanStream(state, '连接中断');
    expect(failed.thinking).toBe('想了半截');
  });
});

/**
 * 刷新之后重新接上一轮（决策 260）。
 *
 * 起因是一条实测：对讲台上值班长正在答话时刷新页面，那一轮**整段看不见**——在途轮的现场
 * （乐观轮 / 流式文本）全住在 `sending` 那一侧，刷新即丢，增量到达时无从判断「这一段字属于
 * 谁」，闸门一律不接；于是只剩等它落地后重读台账才出现。
 *
 * 补的是两条纯判据：接手时记锚点（{@link maxLedgerId}），落地时看台账尾部有没有新行
 * （{@link turnLanded}）。它们都是「只读字段、不看正文」的（与 `failedLedgerRowIds` 同一姿态）。
 */
describe('重新接上一轮：锚点与落地判据（决策 260）', () => {
  const row = (id: number, kind: LedgerRow['kind'] = 'fm') => ({ id, kind });

  it('锚点是台账里最大的行 id；一行都没有时是 0', () => {
    expect(maxLedgerId([])).toBe(0);
    expect(maxLedgerId([row(3), row(9), row(5)])).toBe(9);
    // 顺序无关：接手那一刻读到的台账是升序的，但不靠这个顺序（少一处能漂的假设）
    expect(maxLedgerId([row(9), row(3)])).toBe(9);
  });

  it('接手那一刻已有的行不算落地——回话落地时 id 必然更大', () => {
    const before = maxLedgerId([row(1), row(2)]);
    expect(before).toBe(2);
    // 还在跑：台账一动不动
    expect(turnLanded([row(1), row(2)], before)).toBe(false);
    // 落地：值班长的回话进来（id 3）
    expect(turnLanded([row(1), row(2), row(3)], before)).toBe(true);
  });

  it('落地的那一行是哪种 kind 都算：回话 / 操作台记的账 / 失败账都是「这一轮结束了」', () => {
    const before = 2;
    expect(turnLanded([row(1), row(2), row(3, 'fm')], before)).toBe(true);
    expect(turnLanded([row(1), row(2), row(3, 'console')], before)).toBe(true);
    expect(turnLanded([row(1), row(2), row(3, 'failed')], before)).toBe(true);
  });

  it('空台账 / 锚点之后的更小 id：都不算落地（别把历史当成刚发生的事）', () => {
    expect(turnLanded([], 5)).toBe(false);
    expect(turnLanded([row(3), row(4)], 5)).toBe(false);
  });
});

/**
 * 收场三支（决策 260 裁决③的落实）。
 *
 * 决策 260 原文说死轮（没换行而服务端也不再跑它）**「留白」**——保留半截字。可落地哨当时
 * 只写了「落地就 `emptyForemanStream()`」，于是死轮恰好也走那一支：**半截字被清掉、那一轮
 * 从时间线上整段消失**。那正是用户报的那条毛病在死轮场景下的残留子集。这一组把三支的判据
 * 钉死，并钉住「任何一支都不许清掉已经出现的文字」这条模块级纪律。
 */
describe('跟的那一轮怎么收场：keep / settled / lost（决策 260 裁决③）', () => {
  const row = (id: number): LedgerRow => ({ id, kind: 'fm' });

  it('仍在跑：继续跟（哪怕台账一动不动）', () => {
    expect(resolveFollowOutcome([row(1), row(2)], 2, true)).toEqual({ kind: 'keep' });
  });

  it('台账尾部多了一行：落地（台账那一行接管回话）', () => {
    expect(resolveFollowOutcome([row(1), row(2), row(3)], 2, true)).toEqual({ kind: 'settled' });
    // 落了地而同一班紧接着又起一轮（下一轮已在跑）：**落地优先**——这一轮的回话确实落库了
    expect(resolveFollowOutcome([row(1), row(2), row(3)], 2, true).kind).toBe('settled');
  });

  it('没换行而服务端也不再跑它：lost，不是 settled', () => {
    // 进程被杀 / 重启（决策 223 明确不做那一轮的落账）：台账永远不会有它那一行
    expect(resolveFollowOutcome([row(1), row(2)], 2, false)).toEqual({ kind: 'lost' });
    expect(resolveFollowOutcome([], 0, false)).toEqual({ kind: 'lost' });
  });

  it('**半截字在 lost 那一支必须留着**——`failForemanStream` 不许清字', () => {
    // 这是本组的承重断言：死轮那一支若走 `emptyForemanStream()`，半截字就没了，
    // 而「已经出现的文字任何一支都不许清掉」是本模块文件头立的纪律（票 03）。
    const half = appendForemanDelta(beginForemanStream(), delta('说了一半就断'), SESSION);
    const outcome = resolveFollowOutcome([row(1)], 1, false);
    expect(outcome).toEqual({ kind: 'lost' });

    const shown = failForemanStream(half, FOREMAN_LOST_TURN_SUFFIX);
    expect(shown.text, '半截字必须原样留着').toBe('说了一半就断');
    expect(shown.error).toBe(FOREMAN_LOST_TURN_SUFFIX);
    // 而且它仍然渲染成一轮（不是从时间线上消失）
    const turns = buildTurns(inputOf({ stream: shown }));
    expect(turns.map((t) => t.key)).toEqual(['live', 'send-error']);
    expect(turns[0].content).toBe('说了一半就断');
  });

  it('lost 的说明说清「不会再来」——不假装还能等（与超时那句分得开）', () => {
    expect(FOREMAN_LOST_TURN_SUFFIX).toContain('不会再');
    // 超时那句说的是「仍在继续」：两句说的是相反的实情，不能混用
    expect(FOREMAN_LOST_TURN_SUFFIX).not.toContain('仍在继续');
  });
});

/**
 * 本地放弃之后的接力（决策 260）——决策 223 那条路在界面侧的收口。
 *
 * `say` 的超时**不等于**这一轮失败：它跑在自己的任务里（决策 223），回话照旧落库。
 * 此前那条实情只写在文案里（`FOREMAN_TIMEOUT_SUFFIX` 那句「它仍在服务端继续」），
 * 而屏幕上的那一轮会停在半截（`partial`），**增量也不再接**——说的与实际对不上。
 *
 * 现在接力：`turn_in_flight` 为真就继续跟，那一轮的 `partial` 也随之翻假（它仍在流）。
 */
describe('本地超时之后的接力：说的与做的对上（决策 260）', () => {
  it('接力时那一轮不再是「断流」——它仍在流', () => {
    const stalled = failForemanStream(appendForemanDelta(beginForemanStream(), delta('说了一半'), SESSION), '请求超时（300 秒没有回应）。');
    // 本地放弃那一刻：屏幕上是「断流」
    const before = buildTurns(inputOf({ stream: stalled }));
    expect(before[0]).toMatchObject({ key: 'live', partial: true });

    // 服务端说这一轮在跑 → 接着跟：同一段文字现在标注为「仍在流之中」
    const after = buildTurns(inputOf({ following: true, stream: { ...stalled, streaming: true, error: null } }));
    expect(after[0]).toMatchObject({ key: 'live', partial: false, streaming: true });
    expect(after[0].content).toBe('说了一半');
  });

  it('接力之后到达的增量照旧接得上（那一轮没断）', () => {
    let state = failForemanStream(beginForemanStream(), '请求超时（300 秒没有回应）。');
    // 「跟」这一支的闸门在组件里；这里钉的是归约本身不因 error 在场而拒绝累积
    state = appendForemanDelta(state, delta('后台接着说的'), SESSION);
    expect(state.text).toBe('后台接着说的');
  });
});

/** `buildTurns` 的最小输入（判据本身在 `lib/talkTurns.test.ts`；这里只用它读 partial）。 */
function inputOf(over: Partial<TalkTurnsInput>): TalkTurnsInput {
  return {
    session: null,
    pendingText: null,
    sending: false,
    following: false,
    stream: beginForemanStream(),
    pairingNeeded: false,
    ...over,
  };
}
