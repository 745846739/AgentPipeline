import { afterEach, describe, expect, it, vi } from 'vitest';
import type {
  ConversationDeltaEvent,
  ForemanMessage,
  ForemanSession,
  ForemanSessionMeta,
  TaskListItem,
} from '../api/types';
import { beginForemanStream, FOREMAN_LOST_TURN_SUFFIX } from '../realtime/foreman';
import { ApiError, KIND_REQUEST_TIMEOUT } from '../api/client';
import { saveSeen, saveSessionId } from '../lib/talkSessions';

const mocks = vi.hoisted(() => ({
  getForemanSession: vi.fn(),
  getForemanSessions: vi.fn(),
  getForemanAttention: vi.fn(),
  createForemanSession: vi.fn(),
  archiveForemanSession: vi.fn(),
  sendForemanMessage: vi.fn(),
  getTask: vi.fn(),
}));

// 只换掉取数口；`ApiError` 等原样保留——`isPairingRequired`（sharePairing）趁 `ApiError`
// 还在手判配对缺失，整模块换掉它就没了。
vi.mock('../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../api/client')>()),
  ...mocks,
}));

const { talk } = await import('./talk.svelte');

/**
 * 对讲台的在飞现场（决策 275）——用户报的「切换界面后再回来，本轮之前的输出不见了」。
 *
 * 现场原先住在 `Talk.svelte` 的组件作用域里，页面一切走就被销毁。这里钉的是它搬进 store
 * 之后的两条承重判据：
 *
 * ① **切页面（同一班次）什么都不丢**：`watch()` 认到同一个 id 就不动现场；
 * ② **换班次仍然清现场**：那是决策 204③ / 220⑤ 的既有口径，本次一个字没改。
 *
 * 另加三支收口（决策 260）与**到得就攒、读台账收口**的尾巴纪律（票 02 把旧的到达闸门
 * 换掉了：筛在渲染时按快照做，到达时挑会丢掉补不回的字）——它们此前只有 e2e 覆盖。
 */

const SESSION = 'sess-1';

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
    prompt_tokens: 0,
    completion_tokens: 0,
  };
}

/** 台账里的一行（本组只用得到它的 id 与 status——判据是「尾部有没有更新的行」与
 * 「接手那条半截行收口没有」）。 */
function row(id: number, over: Partial<ForemanMessage> = {}): ForemanMessage {
  return {
    id,
    session_id: SESSION,
    role: 'assistant',
    content: '回话',
    prompt_tokens: 0,
    completion_tokens: 0,
    briefing: null,
    traces: null,
    created_at: '2026-09-25T00:05:00Z',
    kind: 'fm',
    proactive: false,
    status: null,
    ...over,
  };
}

/** 接手那一刻的**半截行**（票 01）：锚的就是它，收口就地写它（status 落 null）。 */
function inflight(id: number): ForemanMessage {
  return row(id, { status: 'in_flight', content: '' });
}

function payload(over: Partial<ForemanSession> = {}): ForemanSession {
  return {
    session: {
      id: SESSION,
      title: '班次',
      created_at: '2026-09-25T00:00:00Z',
      last_active_at: '2026-09-25T00:10:00Z',
      kind: 'talk',
      archived_at: null,
    },
    messages: [],
    proposals: [],
    total_tokens: 0,
    total_calls: 0,
    turn_in_flight: false,
    foreman: { agent_type: 'foreman', stage_key: 'foreman', wired: true },
    ...over,
  };
}

/** 把 store 拨回「刚进这一页、还没在读一轮」的干净态（单例，用例之间必须互不残留）。 */
function reset(id: string | null = null): void {
  // 排水环（票 02）先收掉：它是唯一会自己往外发请求的东西，留着会让**别的用例**的
  // 队列状态在事后偷偷发一跳（`sendForemanMessage` 是 mock，但调用计数会脏）。
  talk.stopQueueDrain();
  talk.sessionId = null;
  talk.sending = false;
  talk.sendingSid = null;
  talk.unsentText = null;
  talk.failureBaseline = new Set();
  talk.resetLive();
  talk.foreign = { bySession: {} };
  talk.queue = {};
  talk.queueHeld = {};
  // 台账生命周期（决策 354①）搬进 store 之后同住一个单例，一并清
  talk.session = null;
  talk.sessionList = [];
  talk.loading = true;
  talk.loadError = null;
  talk.loadErrorPairing = false;
  talk.hasMoreEarlier = false;
  talk.loadingEarlier = false;
  talk.showArchived = false;
  talk.kind = 'talk';
  talk.seen = {};
  talk.details = {};
  talk.attention = null;
  talk.sessionId = id;
  // localStorage 侧同样归零：store 只在构造时读一次，残留会从「记忆」里漏进来
  saveSeen({});
  saveSessionId(null);
}

afterEach(() => {
  reset();
  vi.restoreAllMocks();
});

describe('在飞现场随页面来去（决策 275）', () => {
  it('切页面（同一班次）什么都不丢：步骤与乐观轮还在', () => {
    reset(SESSION);
    // 一轮正在跑：本机发出、增量在路上
    talk.sending = true;
    talk.pendingText = '看板里有哪些重试？';
    talk.stream = beginForemanStream();
    talk.note(delta('先看看板。'));
    talk.note({ ...delta('我要查一下。'), channel: 'reasoning' });

    // 页面被卸载又挂回来：`reload()` 走完会拿**同一个** id 再认一次
    talk.watch(SESSION);

    expect(talk.pendingText).toBe('看板里有哪些重试？');
    expect(talk.sending).toBe(true);
    expect(talk.stream.steps.map((step) => step.kind)).toEqual(['text', 'thinking']);
    // 而且增量照旧接得上（切走那段时间它一路攒着）
    talk.note(delta('t1 还在排队。'));
    expect(talk.stream.steps.map((step) => step.kind)).toEqual(['text', 'thinking', 'text']);
  });

  it('换班次照旧清现场（决策 204③ / 220⑤ 的口径一个字没改）', () => {
    reset(SESSION);
    talk.sending = true;
    talk.pendingText = '甲班的话';
    talk.stream = beginForemanStream();
    talk.note(delta('甲班的回话'));
    talk.followingSince = 7;
    talk.pairingNeeded = true;

    talk.watch('sess-2');

    expect(talk.sessionId).toBe('sess-2');
    expect(talk.pendingText, '乐观轮属于**那一班**，不许跟到这一班来').toBeNull();
    expect(talk.stream.steps).toEqual([]);
    expect(talk.followingSince).toBeNull();
    expect(talk.pairingNeeded).toBe(false);
  });

  it('「从没有班次到有班次」不算换班：第一句话自己开的那一班，现场说的就是它', () => {
    // 首启空 home：`send()` 先乐观地亮一轮，再去开一个新班次——这时若把现场清掉，
    // 人刚发出去的那句话会从屏上消失（`talk.watch(sid)` 就在那一步）
    reset(null);
    talk.sending = true;
    talk.pendingText = '第一句话';
    talk.stream = beginForemanStream();

    talk.watch(SESSION);

    expect(talk.pendingText).toBe('第一句话');
    expect(talk.stream.streaming).toBe(true);
  });
});

describe('增量闸门与收口（决策 260 的三支）', () => {
  it('没在等一轮时增量**也攒**（票 02：筛在渲染时按快照做）——但下一次读台账就把它收口', () => {
    // 旧口径是「到达时就丢」：那会在快照读回来之前的那几拍里丢掉补不回的字
    // （SSE 无回放，决策 275）。现在到得就攒，尾巴的收口交给下一次读台账。
    reset(SESSION);
    talk.note(delta('接手前到达的半句'));
    expect(talk.stream.steps).toEqual([{ kind: 'text', text: '接手前到达的半句' }]);

    // 没在跟、服务端也没在跑：这条尾巴是残渣（收场发生在这一屏之外）——读一次台账就清掉，
    // 不许它凭空造一轮久驻。
    talk.syncFollowing(payload({ messages: [row(1)], turn_in_flight: false }));
    expect(talk.stream.steps).toEqual([]);
  });

  it('在跟一轮时（本机没在发）增量照旧接进分段', () => {
    reset(SESSION);
    talk.followingSince = 0;
    talk.note(delta('接着说的'));
    expect(talk.stream.steps).toEqual([{ kind: 'text', text: '接着说的' }]);
  });

  it('还在跑：一个字都不动（锚点也不许跟着往前爬）', () => {
    reset(SESSION);
    talk.followingSince = 2;
    talk.stream = beginForemanStream();
    talk.note(delta('正在答'));

    // 接手的锚点就是那条半截行（票 01）：它 status 还在途 → keep，现场一个字不动
    talk.syncFollowing(
      payload({ messages: [row(1), inflight(2)], turn_in_flight: true }),
    );

    expect(talk.followingSince, '锚点必须留在接手那一刻').toBe(2);
    expect(talk.stream.steps).toHaveLength(1);
  });

  it('落地：在飞那一段退场（台账那一行接管），仍有一轮在跑就重新立锚点', () => {
    reset(SESSION);
    talk.followingSince = 2;
    talk.stream = beginForemanStream();
    talk.note(delta('半截'));

    talk.syncFollowing(payload({ messages: [row(1), row(2), row(3)], turn_in_flight: false }));

    expect(talk.stream.steps).toEqual([]);
    expect(talk.followingSince).toBeNull();

    // 「回话落了库、而同一班紧接着又起了一轮」（值守轮插进来）：跟的是**新那一轮**
    talk.stream = beginForemanStream();
    talk.followingSince = 2;
    talk.syncFollowing(payload({ messages: [row(1), row(2), row(3)], turn_in_flight: true }));
    expect(talk.followingSince, '新那一轮的锚点按落库后的台账重记').toBe(3);
  });

  it('死轮：半截字留着、只多一句说明（决策 260 裁决③）', () => {
    reset(SESSION);
    talk.followingSince = 2;
    talk.stream = beginForemanStream();
    talk.note(delta('说了一半就断'));

    // 进程被杀：半截行还挂着（status 仍是 in_flight）、服务端也不再报在跑 → 死轮
    talk.syncFollowing(
      payload({ messages: [row(1), inflight(2)], turn_in_flight: false }),
    );

    expect(talk.stream.steps).toEqual([{ kind: 'text', text: '说了一半就断' }]);
    expect(talk.stream.error).toBe(FOREMAN_LOST_TURN_SUFFIX);
    expect(talk.followingSince).toBeNull();
  });

  it('**就地收口**（票 01）：接手那条半截行 status 落成 null → 落地，不误判成死轮', () => {
    // 收口写的是同一行（尾部不多一行）：老判据在这里会走 lost，半截字顶着
    // 「不会再来」的说明——而它其实答完了。
    reset(SESSION);
    talk.followingSince = 2;
    talk.stream = beginForemanStream();
    talk.note(delta('说了一半'));

    talk.syncFollowing(payload({ messages: [row(1), row(2)], turn_in_flight: false }));

    expect(talk.stream.steps, '落地：台账那一行接管，本地那一段退场').toEqual([]);
    expect(talk.stream.error).toBeNull();
    expect(talk.followingSince).toBeNull();
  });

  it('**台账中断行接手**（票 03）：重启后半截行标成 interrupted → 落地，由台账接管', () => {
    // 进程被杀、重启后启动恢复把悬挂行标成 interrupted（显式修订决策 223）——那一行
    // 就是这一轮的终态。若这里走 lost，界面会**本地合成**一条失败轮，与台账那条中断行
    // 并排成两套真相（决策 260 裁决③从此以台账为准：清本地、信台账、重读）。
    reset(SESSION);
    talk.followingSince = 2;
    talk.stream = beginForemanStream();
    talk.note(delta('说了一半就断'));

    talk.syncFollowing(
      payload({
        messages: [
          row(1),
          row(2, {
            status: 'interrupted',
            interrupted_at: '2026-09-25T00:09:00Z',
            content: '说了一半就断',
          }),
        ],
        turn_in_flight: false,
      }),
    );

    expect(talk.stream.steps, '台账那一行接管，本地那一段退场').toEqual([]);
    expect(talk.stream.error, '中断不是失败轮：标记由台账那条行自己渲染').toBeNull();
    expect(talk.followingSince).toBeNull();
  });

  it('收尾：`settleTurn` 清现场并自己重读台账（决策 354①：epoch 喊话的两端都没了）', async () => {
    reset(SESSION);
    talk.stream = beginForemanStream();
    talk.pendingText = '问句';
    mocks.getForemanSessions.mockResolvedValue({ sessions: [meta(SESSION)] });
    mocks.getForemanSession.mockResolvedValue(payload({ messages: [row(1)] }));
    mocks.getForemanAttention.mockResolvedValue({ open: 0, by_kind: {}, blocked_reads: { stuck_now: 0, stuck_total: 0, longest_wait_ms: 0 } });

    talk.settleTurn();
    expect(talk.pendingText, '现场退场').toBeNull();
    expect(talk.stream.steps).toEqual([]);

    // 收尾自己把台账重读一遍——不再靠 ledgerEpoch 朝在屏的那一页喊话
    await vi.waitFor(() => expect(mocks.getForemanSession).toHaveBeenCalled());
  });

  it('本机在发时不接手（两条来源各收各的口）', () => {
    reset(SESSION);
    talk.sending = true;
    talk.syncFollowing(payload({ messages: [row(1)], turn_in_flight: true }));
    expect(talk.followingSince, '本机这一趟的收尾归 send() 管').toBeNull();
  });
});

/**
 * 接上路径点亮流式 + 排队发送（票 02 / 04 of talk-live-identity，2026-09-29 决议）。
 *
 * 前者钉「接上的一轮形态与发送中不可区分」的引擎那一半（`streaming` 只有 `send()`
 * 一个点火点，是「假流断」的根因）；后者钉队列的三条纪律：按班次分列、死轮扣住、
 * 值守轮不触发出队（出队效果在组件里，store 只管账）。
 */
describe('接上点亮流式（票 02 of talk-live-identity）', () => {
  it('刷新后接上：syncFollowing 立锚那一刻 streaming 点亮', () => {
    reset(SESSION);
    expect(talk.stream.streaming).toBe(false);
    talk.syncFollowing(payload({ messages: [row(1), inflight(2)], turn_in_flight: true }));
    expect(talk.followingSince).toBe(2);
    expect(talk.stream.streaming, '光标 / ticker / 贴底跟随全挂在这枚旗上').toBe(true);
  });

  it('落地收口：现场退场（streaming 归假）；紧跟着又起一轮则重新点亮', () => {
    reset(SESSION);
    talk.followingSince = 2;
    talk.stream = { ...beginForemanStream(), streaming: true };
    talk.syncFollowing(payload({ messages: [row(1), row(2)], turn_in_flight: false }));
    expect(talk.stream.streaming).toBe(false);

    talk.syncFollowing(payload({ messages: [row(1), row(2), inflight(3)], turn_in_flight: true }));
    expect(talk.followingSince, '新那一轮重新立锚').toBe(3);
    expect(talk.stream.streaming, '新那一轮照旧点亮').toBe(true);
  });

  it('本地放弃后的无条件接手同样点亮（followAfterGiveUp）', () => {
    reset(SESSION);
    talk.followAfterGiveUp(7);
    expect(talk.followingSince).toBe(7);
    expect(talk.stream.streaming).toBe(true);
  });
});

describe('排队发送（票 04 of talk-live-identity）', () => {
  it('入队 / 出队：先进先出，取完回 null', () => {
    reset(SESSION);
    talk.enqueue(SESSION, '第一句');
    talk.enqueue(SESSION, '第二句');
    expect(talk.takeQueued(SESSION)).toBe('第一句');
    expect(talk.takeQueued(SESSION)).toBe('第二句');
    expect(talk.takeQueued(SESSION)).toBeNull();
  });

  it('队列**按班次分列**：排进甲班的话不许被乙班取走（决策 204⑥ 的队列版）', () => {
    reset(SESSION);
    talk.enqueue(SESSION, '甲班的话');
    talk.enqueue('sess-2', '乙班的话');
    expect(talk.takeQueued('sess-2')).toBe('乙班的话');
    expect(talk.takeQueued(SESSION)).toBe('甲班的话');
  });

  it('就地编辑与撤回', () => {
    reset(SESSION);
    talk.enqueue(SESSION, '原话');
    talk.enqueue(SESSION, '另一句');
    talk.editQueued(SESSION, 0, '改过的话');
    expect(talk.queue[SESSION]).toEqual(['改过的话', '另一句']);
    talk.removeQueued(SESSION, 0);
    expect(talk.queue[SESSION]).toEqual(['另一句']);
    // 越界 / 空文本编辑不动账
    talk.editQueued(SESSION, 5, 'x');
    expect(talk.queue[SESSION]).toEqual(['另一句']);
  });

  it('死轮：队列扣住（不自动照发），确认 / 清空两个出口', () => {
    reset(SESSION);
    talk.enqueue(SESSION, '排在后面的话');
    talk.followingSince = 2;
    talk.syncFollowing(payload({ messages: [row(1), inflight(2)], turn_in_flight: false }));
    expect(talk.stream.error, '死轮照旧收成失败轮').toBe(FOREMAN_LOST_TURN_SUFFIX);
    expect(talk.queueHeld[SESSION], '队列不许照发——「它不会再来」要人看见').toBe(true);

    talk.setQueueHeld(SESSION, false);
    expect(talk.queueHeld[SESSION]).toBe(false);
  });

  it('中断行：队列**同样**扣住（回话不会来了），确认后放行', () => {
    // 票 04 原文「死轮 / 中断时队列扣住等确认」：中断与死轮同罪——评审实错的回归，
    // 初版只在 `lost` 支扣，中断走 `settled` 支被静默放行照发。
    reset(SESSION);
    talk.enqueue(SESSION, '排在后面的话');
    talk.followingSince = 2;
    talk.syncFollowing(
      payload({
        messages: [row(1), row(2, { status: 'interrupted' })],
        turn_in_flight: false,
      }),
    );
    expect(talk.queueHeld[SESSION], '中断行也不许自动照发').toBe(true);

    talk.setQueueHeld(SESSION, false);
    expect(talk.queueHeld[SESSION]).toBe(false);
  });

  it('正常收口：队列**不**扣住（出队效果据此自动发下一条）', () => {
    reset(SESSION);
    talk.enqueue(SESSION, '排在后面的话');
    talk.followingSince = 2;
    talk.syncFollowing(payload({ messages: [row(1), row(2)], turn_in_flight: false }));
    expect(talk.queueHeld[SESSION], '回话来了就该自动发——扣住会把队列卡死').toBeFalsy();
  });
});

// ═══════════ 台账生命周期（决策 354①，票 talk-store-ledger 01）═══════════
//
// reload / loadEarlier / switchTo / 归档坠落 / seen 标记自 Talk.svelte 搬进 store。
// 钉的是搬完之后仍然承重的四条判据：重读合并已分页的旧消息（票 05 的不重不漏）、
// 过期回包守卫（票 01 of talk-live-identity 的纪律跟着数据走）、向上游标翻页、
// 归档坠落（决策 204：归档完自动切到最近有说话的班次）。

function meta(id: string, over: Partial<ForemanSessionMeta> = {}): ForemanSessionMeta {
  return {
    id,
    title: `班次 ${id}`,
    kind: 'talk',
    created_at: '2026-09-25T00:00:00Z',
    last_active_at: '2026-09-25T00:10:00Z',
    archived_at: null,
    ...over,
  };
}

const ATTENTION = {
  open: 0,
  by_kind: {},
  blocked_reads: { stuck_now: 0, stuck_total: 0, longest_wait_ms: 0 },
};

/** 拿 payload() 的默认 session 形状换一个 id（归档坠落要用多班的载荷）。 */
function payloadFor(id: string, over: Partial<ForemanSession> = {}): ForemanSession {
  return payload({ session: meta(id), ...over });
}

function taskItem(id: string): TaskListItem {
  return {
    id,
    project_id: 'p1',
    title: `任务 ${id}`,
    description: '',
    status: 'pending',
    current_stage: 'develop',
    current_node: 'execute',
    validate_attempts: 0,
    pending_reason: null,
    worktree_path: null,
    branch_name: null,
    stewardship: null,
    total_tokens: 0,
    total_calls: 0,
    review_mode: 'agent',
  } as TaskListItem;
}

function flush(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

describe('台账生命周期（决策 354①）', () => {
  it('reload：同一班重读时，已分页加载的更早消息并回头部（票 05 的不重不漏）', async () => {
    reset(SESSION);
    // 已通过「滚到顶加载」拿到更早的 1–3 行；重读只回最近的 4、5 两行——
    // 直接盖上会把滚上去加载的那段历史变没
    talk.session = payloadFor(SESSION, { messages: [row(1), row(2), row(3)] });
    mocks.getForemanSessions.mockResolvedValue({ sessions: [meta(SESSION)] });
    mocks.getForemanSession.mockResolvedValue(payloadFor(SESSION, { messages: [row(4), row(5)] }));
    mocks.getForemanAttention.mockResolvedValue(ATTENTION);

    const ok = await talk.reload();

    expect(ok).toBe(true);
    expect(talk.session?.messages.map((m) => m.id)).toEqual([1, 2, 3, 4, 5]);
  });

  it('reload：换班不并——另一班的账整包落屏，不掺上一班的残页', async () => {
    reset(SESSION);
    talk.session = payloadFor(SESSION, { messages: [row(1), row(2), row(3)] });
    mocks.getForemanSessions.mockResolvedValue({ sessions: [meta('sess-2')] });
    mocks.getForemanSession.mockResolvedValue(payloadFor('sess-2', { messages: [row(9)] }));
    mocks.getForemanAttention.mockResolvedValue(ATTENTION);

    await talk.reload('sess-2');

    expect(talk.sessionId).toBe('sess-2');
    expect(talk.session?.messages.map((m) => m.id)).toEqual([9]);
  });

  it('reload：切班后过期的回包整包丢弃（守卫跟着数据走，纪律不搬家）', async () => {
    reset(SESSION);
    talk.session = payloadFor(SESSION);
    mocks.getForemanSessions.mockResolvedValue({ sessions: [meta('sess-2')] });
    // 第一趟挂着不落地；在它回来之前人已经切去了 sess-2
    let release!: (v: ForemanSession) => void;
    mocks.getForemanSession.mockReturnValue(
      new Promise<ForemanSession>((resolve) => (release = resolve)),
    );
    const pending = talk.reload('sess-2');
    talk.watch('sess-3');
    release(payloadFor('sess-2', { messages: [row(9)] }));
    const ok = await pending;

    expect(ok, '旧目标的台账整包作废').toBe(false);
    expect(talk.sessionId).toBe('sess-3');
    expect(talk.session?.session?.id, '在屏的台账不被旧回包拖回去').toBe(SESSION);
  });

  it('loadEarlier：更早一段接进头部，游标用当时的最老行；空段即到头', async () => {
    reset(SESSION);
    talk.session = payloadFor(SESSION, { messages: [row(5), row(6)] });
    talk.hasMoreEarlier = true;
    mocks.getForemanSession.mockResolvedValue({ messages: [row(3), row(4)] });

    await talk.loadEarlier();

    // 游标取的是请求那一刻的最老行（before_id=5）；段接在头部，顺序不乱
    expect(mocks.getForemanSession).toHaveBeenCalledWith(SESSION, undefined, 'talk', 5);
    expect(talk.session?.messages.map((m) => m.id)).toEqual([3, 4, 5, 6]);
    // 没读满一段（500）就到头了：hasMoreEarlier 置假，这条路不再走到
    expect(talk.hasMoreEarlier).toBe(false);
    // 「不滚动」在 store 这一侧的落点：翻页只往头部接行，**不重排本屏**——上面钉住的是
    // 只发了一跳取数，这里再钉住那一跳不是 `reload`（reload 会先重读列表、并动 loading /
    // attention 那一排读数）。滚动几何是页面的活儿（`delegation-scan.test.ts` 钉着它留那边）。
    expect(mocks.getForemanSessions, '向上翻页不重读列表').not.toHaveBeenCalled();

    await talk.loadEarlier();
    expect(mocks.getForemanSession, '到头后不再发第二跳').toHaveBeenCalledTimes(1);
  });

  it('归档坠落：切到最近活动的未归档班（刚归档的那班排在第一也不许「原地不动」）', async () => {
    reset(SESSION);
    mocks.archiveForemanSession.mockResolvedValue({ session: meta(SESSION) });
    mocks.getForemanSessions.mockResolvedValue({
      sessions: [meta(SESSION, { archived_at: '2026-09-25T01:00:00Z' }), meta('sess-2')],
    });
    mocks.getForemanSession.mockResolvedValue(payloadFor('sess-2', { messages: [row(1)] }));
    mocks.getForemanAttention.mockResolvedValue(ATTENTION);

    await talk.archiveAndFall(SESSION, { onArchived: () => {} });

    expect(talk.sessionId, '落点是列表里第一个未归档班').toBe('sess-2');
    expect(talk.session?.session?.id).toBe('sess-2');
  });

  it('归档坠落：一个不剩就新开一班（空班是合法状态）', async () => {
    reset(SESSION);
    mocks.archiveForemanSession.mockResolvedValue({ session: meta(SESSION) });
    mocks.getForemanSessions.mockResolvedValue({ sessions: [] });
    mocks.createForemanSession.mockResolvedValue({ session: meta('sess-new') });
    mocks.getForemanSession.mockResolvedValue(payloadFor('sess-new'));
    // openFreshSession 落点后的重读会带回**服务端已建好**的那一班——列表里自然有它
    mocks.getForemanSessions.mockResolvedValue({ sessions: [meta('sess-new')] });
    mocks.getForemanAttention.mockResolvedValue(ATTENTION);

    await talk.archiveAndFall(SESSION, { onArchived: () => {} });

    expect(talk.sessionId).toBe('sess-new');
    expect(talk.sessionList.map((s) => s.id), '新开的班进列表').toEqual(['sess-new']);
  });

  it('pending 详情指纹去重（归 store 管）：集合不变不重拉，值守账不拉', async () => {
    reset(null);
    mocks.getTask.mockResolvedValue({ allowed_actions: [], cursors: [] });

    talk.syncPendingDetails([taskItem('t1')], false);
    await flush();
    expect(mocks.getTask).toHaveBeenCalledTimes(1);

    talk.syncPendingDetails([taskItem('t1')], false);
    await flush();
    expect(mocks.getTask, '指纹没变：不重拉').toHaveBeenCalledTimes(1);

    // 类型变（pending_updated 换理由而 id 不变）要重拉
    const changed = taskItem('t1');
    changed.pending_reason = { type: 'info_insufficient' } as TaskListItem['pending_reason'];
    talk.syncPendingDetails([changed], false);
    await flush();
    expect(mocks.getTask, '指纹变了：重拉').toHaveBeenCalledTimes(2);

    talk.syncPendingDetails([taskItem('t1')], true);
    await flush();
    expect(mocks.getTask, '值守账只读：不拉那批详情').toHaveBeenCalledTimes(2);
  });

  it('seen 标记随 reload 收口：落点记进看过表，先有基线才不假亮（决策 220③）', async () => {
    reset(SESSION);
    talk.sessionList = [meta(SESSION), meta('sess-2')];
    mocks.getForemanSessions.mockResolvedValue({
      sessions: [meta(SESSION), meta('sess-2')],
    });
    mocks.getForemanSession.mockResolvedValue(
      payloadFor(SESSION, {
        session: meta(SESSION, { last_active_at: '2026-09-25T02:00:00Z' }),
      }),
    );
    mocks.getForemanAttention.mockResolvedValue(ATTENTION);

    await talk.reload();

    expect(talk.seen[SESSION], '落点记它此刻的 last_active_at').toBe('2026-09-25T02:00:00Z');
    expect(talk.seen['sess-2'], '没落到的班不记看过——它的新动静要亮').toBeUndefined();
  });
});

// ═══════════ 发送编排（决策 354②，票 talk-store-ledger 02）═══════════
//
// sendTurn / claim / 排水环自 Talk.svelte 搬进 store。钉的是搬完之后**新的**承重判据：
// 排水不再以「页面在屏」为前提（决策 354② 接受的唯一行为修正）、串台守卫收成一处的
// `claim()`、失败那句话交回页面回填的载体（`unsent`）。

/** 一发一收的常规桩：列表、台账、待办读数三层都备好。 */
function stubSend(): void {
  mocks.sendForemanMessage.mockResolvedValue({ reply: null });
  mocks.getForemanSessions.mockResolvedValue({ sessions: [meta(SESSION)] });
  mocks.getForemanSession.mockResolvedValue(payloadFor(SESSION, { messages: [row(1)] }));
  mocks.getForemanAttention.mockResolvedValue(ATTENTION);
}

describe('发送编排（决策 354②）', () => {
  it('claim：认下当下这一班；换了班这一趟的回包即作废', () => {
    reset(SESSION);
    const mine = talk.claim();
    expect(mine()).toBe(true);

    talk.watch('sess-2');

    expect(mine(), '换班之后这一趟不再属于这一屏（决策 204⑥）').toBe(false);
    expect(talk.claim()(), '重新认下就是新那一班').toBe(true);
  });

  it('sendTurn：发出去就亮乐观轮，POST 回来以台账收口', async () => {
    reset(SESSION);
    stubSend();

    const pending = talk.sendTurn('看板里有哪些重试？');

    // 服务端登记之前也先亮一轮（免得人以为没按上）
    expect(talk.sending).toBe(true);
    expect(talk.pendingText).toBe('看板里有哪些重试？');
    expect(talk.stream.streaming).toBe(true);
    expect(talk.sendingSid, '「正在回话」那枚标记要落在**它**那一行上').toBe(SESSION);
    expect(talk.followingSince, '本机这一趟接手之后就不再跟别人').toBeNull();

    await pending;

    expect(mocks.sendForemanMessage).toHaveBeenCalledWith('看板里有哪些重试？', SESSION);
    expect(talk.sending).toBe(false);
    expect(talk.sendingSid).toBeNull();
    expect(talk.pendingText, '收口：乐观轮交给台账那一行').toBeNull();
    expect(talk.unsentText, '送到了就没有要回填的话').toBeNull();
  });

  it('没有班次的第一句话：本机先开一班再发（不肯让它落到服务端缺省上）', async () => {
    reset(null);
    stubSend();
    mocks.createForemanSession.mockResolvedValue({ session: meta('sess-new') });
    // 回话落地后重读：这一班已经在列表里，服务端也认它
    mocks.getForemanSessions.mockResolvedValue({ sessions: [meta('sess-new')] });
    mocks.getForemanSession.mockResolvedValue(payloadFor('sess-new'));

    await talk.sendTurn('第一句话');

    expect(mocks.sendForemanMessage).toHaveBeenCalledWith('第一句话', 'sess-new');
    expect(talk.sessionId).toBe('sess-new');
  });

  it('传输失败：那句话交回页面回填，现场收成失败轮（措辞说它是**这次**的）', async () => {
    reset(SESSION);
    stubSend();
    mocks.sendForemanMessage.mockRejectedValue(new Error('断网了'));

    await talk.sendTurn('这句话没送出去');

    expect(talk.unsentText, '页面据此把它送回输入框（框空着时）——「失败不清空输入框」').toBe(
      '这句话没送出去',
    );
    expect(talk.stream.error).toContain('断网了');
    expect(talk.sending).toBe(false);
  });

  it('本地放弃（超时）那一类：同样把话交回框里，但**不落失败轮**、无条件接手', async () => {
    // 决策 223 / 288：本地等不到回包**不等于**那一轮失败——服务端那一轮不随这次请求死。
    // 故这一支与真失败（上面那条）恰好相反：现场收成安静态、交棒给落地哨，而那句话照旧
    // 回到框里（人得看见自己那句还在手上）。判据趁 `ApiError` 还在手按 `kind` 判。
    reset(SESSION);
    stubSend();
    mocks.sendForemanMessage.mockRejectedValue(
      new ApiError(0, '请求超时（120 秒没有回应）。', KIND_REQUEST_TIMEOUT),
    );

    await talk.sendTurn('这句话也许还在跑');

    expect(talk.unsentText).toBe('这句话也许还在跑');
    expect(talk.stream.error, '安静态：不落失败轮').toBeNull();
    expect(talk.pairingNeeded).toBe(false);
    expect(talk.followingSince, '无条件接手（不等 stale 读数）').toBe(1);
  });

  it('串台：POST 期间换了班，回包整包丢弃、也不去重读**那一班**的台账', async () => {
    reset(SESSION);
    stubSend();
    let release!: (v: { reply: string | null }) => void;
    mocks.sendForemanMessage.mockReturnValue(
      new Promise<{ reply: string | null }>((resolve) => (release = resolve)),
    );

    const pending = talk.sendTurn('甲班的话');
    // 人切去了乙班（回话照旧落甲班的台账，但不许落在这一屏上）
    talk.watch('sess-2');
    release({ reply: '甲班的回话' });
    await pending;

    expect(talk.pendingText, '乐观轮属于已经不显示的那一班：连它一起撤').toBeNull();
    expect(talk.stream.steps, '回包不接进乙班的现场').toEqual([]);
    expect(
      mocks.getForemanSession,
      '更不去重读甲班的台账（回话落库这件事不由这一趟说话）',
    ).not.toHaveBeenCalledWith(SESSION, undefined, 'talk');
  });

  it('失败也没送出去的那一类不清台账基线：`failureBaseline` 记的是**发之前**的失败行', async () => {
    // 决策 337 的判据要有差集才成立（`ledgerOwnsTheFailure`）——基线跟着这一趟走，
    // 而它现在由 store 在发出去的那一刻自己记。
    reset(SESSION);
    stubSend();
    talk.session = payloadFor(SESSION, { messages: [row(1), row(2, { kind: 'failed' })] });

    await talk.sendTurn('再问一句');

    expect(talk.failureBaseline.has(2), '发之前台账里那条失败行算「早先的」').toBe(true);
  });

  it('排水环：**页面不在场**也照排照发（决策 354② 接受的唯一行为修正）', async () => {
    reset(SESSION);
    stubSend();
    talk.startQueueDrain();

    talk.enqueue(SESSION, '排队的话');

    await vi.waitFor(() =>
      expect(mocks.sendForemanMessage).toHaveBeenCalledWith('排队的话', SESSION),
    );
    expect(talk.queue[SESSION], '出队即取走').toEqual([]);
    await flush();
  });

  it('排水环：还在跑 / 队列扣住时都不发（节奏照旧，一个条件都不放松）', async () => {
    reset(SESSION);
    stubSend();
    talk.startQueueDrain();

    talk.sending = true;
    talk.enqueue(SESSION, '排在后面的话');
    await flush();
    expect(mocks.sendForemanMessage, '本机在发：等着').not.toHaveBeenCalled();

    talk.sending = false;
    talk.setQueueHeld(SESSION, true);
    await flush();
    expect(mocks.sendForemanMessage, '扣住了：等人确认或清空').not.toHaveBeenCalled();

    talk.setQueueHeld(SESSION, false);
    await vi.waitFor(() => expect(mocks.sendForemanMessage).toHaveBeenCalledTimes(1));
    await flush();
  });

  it('submit：在飞就入队、空着就直接发（「在飞」的判据只有 store 这一处）', async () => {
    reset(SESSION);
    stubSend();

    talk.submit('第一句');
    await vi.waitFor(() => expect(talk.sending, '第一趟收口').toBe(false));
    expect(mocks.sendForemanMessage).toHaveBeenCalledWith('第一句', SESSION);

    // 一轮在飞：这一句进队列，不发
    talk.sending = true;
    talk.submit('第二句');
    expect(talk.queue[SESSION]).toEqual(['第二句']);
    expect(mocks.sendForemanMessage).toHaveBeenCalledTimes(1);

    // 本机那一趟收口：队列由排水环送出去（页面这一侧一个字都不用做）
    talk.sending = false;
    talk.startQueueDrain();
    await vi.waitFor(() =>
      expect(mocks.sendForemanMessage).toHaveBeenCalledWith('第二句', SESSION),
    );
    await flush();
  });
});
