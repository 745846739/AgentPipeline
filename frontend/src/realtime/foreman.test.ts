import { describe, expect, it } from 'vitest';
import type { ConversationDeltaEvent, ForemanSessionMeta, ToolEventEvent } from '../api/types';
import {
  appendForemanDelta,
  appendForemanTool,
  beginForemanStream,
  emptyForeignActive,
  emptyForemanStream,
  failedLedgerRowIds,
  failForemanStream,
  failureNotice,
  FOREIGN_TTL_MS,
  foreignIsReplying,
  forgetForeignActive,
  FOREMAN_TIMEOUT_SUFFIX,
  isTimeoutMessage,
  ledgerOwnsTheFailure,
  noteForeignDelta,
  pruneForeignActive,
  settleForemanStream,
  FOREMAN_ATTRIBUTION_MARK,
  FOREMAN_FAILED_TURN_MARK,
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
    const failedRow = (id: number) => ({
      id,
      role: 'system',
      content: `${FOREMAN_FAILED_TURN_MARK}这一轮没跑起来（llm_auth）：…`,
    });
    const mine = { id: 3, role: 'user', content: `${FOREMAN_FAILED_TURN_MARK}我引用了一下这个标记` };
    const consoleRow = { id: 4, role: 'system', content: '【操作台】…' };

    // 空台账 / 只有无关行 → 本地那一行要留着（请求根本没到后端时它是唯一信号）
    expect(ledgerOwnsTheFailure([], failedLedgerRowIds([]))).toBe(false);
    expect(ledgerOwnsTheFailure([mine, consoleRow], failedLedgerRowIds([mine, consoleRow]))).toBe(
      false,
    );

    // 失败之后重取到的台账里多出一条带标记的 system 行 → 它就是这次的失败轮
    const before = failedLedgerRowIds([mine, consoleRow]);
    expect(ledgerOwnsTheFailure([mine, consoleRow, failedRow(7)], before)).toBe(true);

    // 早先那次失败**不算**：不然一次历史失败会让此后每次真实断网都静默
    const stale = failedRow(5);
    const beforeWithStale = failedLedgerRowIds([stale]);
    expect(ledgerOwnsTheFailure([stale], beforeWithStale)).toBe(false);
    expect(ledgerOwnsTheFailure([stale, failedRow(9)], beforeWithStale)).toBe(true);

    // 只有「角色是 system 且带标记」的才算：人的话里引用这个标记不作数
    expect(ledgerOwnsTheFailure([mine], new Set())).toBe(false);
  });

  it('本地超时不等于这一轮失败：补上「它仍在服务端继续」的实情（决策 223）', () => {
    // 判据与 `api/client.ts::mapRequestError` 的超时那句同源
    expect(isTimeoutMessage('请求超时（300 秒没有回应）。')).toBe(true);
    expect(isTimeoutMessage('配对令牌无效')).toBe(false);

    const timedOut = failureNotice('请求超时（300 秒没有回应）。');
    expect(timedOut).toContain('请求超时');
    expect(timedOut).toContain(FOREMAN_TIMEOUT_SUFFIX);
    // 其余失败**不**加这句话：真失败了还说「仍在继续」是在骗人
    expect(failureNotice('这一轮没跑起来（llm_auth）：密钥不对')).toBe(
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

/**
 * 归因类别标记（决策 235① / 238）：界面只做一件事——把后端给的类别翻成一个词。
 *
 * 判据的重点在**未定位那一条**：绝不能编一个假的类别（决策 230 把「没有类别」也算一项
 * 判据，画一个「未定位」的标签会让人以为那也是一类）。
 */
describe('归因类别标记（票 03）', () => {
  // 词表**只有一份**（后端的 `AttributionKind::label`）：界面拿 `attribution_label` 直接渲染，
  // 故这里只钉哨兵这个镜像常量（e2e 的 mock 回话要照它写）。
  it('哨兵与后端同源（原样镜像）', () => {
    // 后端那一份在 `crates/core/src/pipeline/foreman.rs::FOREMAN_ATTRIBUTION_MARK`。
    expect(FOREMAN_ATTRIBUTION_MARK).toBe('【归因】');
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
