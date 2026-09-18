import type { ForemanSessionMeta, SseEvent } from '../api/types';

/**
 * 值班长流式归约（票 03）：与 `reduce.ts` 同一姿态的纯函数——不触网、不读时钟、不改入参。
 *
 * 一段回话由两类事实拼成：SSE 增量（先到、可能中途断）与 POST 的权威回话（后到、完整）。
 * 两者的合并规则收在这里，界面只渲染 `text` + `streaming`；断流时 `text` 就是
 * 「已经出现的文字」，任何一支都不许把它清掉。
 */

/** 工头事件的 `agent_type`（`crates/core/src/pipeline/foreman.rs::FOREMAN_AGENT_TYPE` 的镜像）。 */
export const FOREMAN_AGENT_TYPE = 'foreman';

export interface ForemanStreamState {
  /** 本轮已到达的流式文本。 */
  text: string;
  /** 是否仍在流：只有 `true` 才渲染既有方块光标（`.streaming`，不新增动画位）。 */
  streaming: boolean;
  /** 断流 / 出错说明；非空时 `text` 照常显示（降级为一次性显示已收到的部分）。 */
  error: string | null;
}

export function emptyForemanStream(): ForemanStreamState {
  return { text: '', streaming: false, error: null };
}

/** 开一轮新回话：丢掉上一轮的残留，点亮方块光标。 */
export function beginForemanStream(): ForemanStreamState {
  return { text: '', streaming: true, error: null };
}

/**
 * 增量累积。
 *
 * 两道判定都不能省：
 *
 * 1. **身份**——只认工头自己的对话增量。`/foreman/stream` 与任务流共用一条总线
 *    （决策 182⑥），漏判就会把别的任务的增量拼进值班长的话里；
 * 2. **班次**（决策 204⑥）——`/foreman/stream` 把所有工头增量广播给所有订阅者，
 *    而**同一台机器上可以多处同时说话**（手机 + 电脑，配对令牌正是为此存在）。
 *    增量与当前班次不符时丢弃：否则手机上那一班的回话会插进电脑这一班的话里。
 *    `sessionId` 为空（还没有当前班次）时同样丢弃——那种状态下屏幕上是空态，
 *    没有「属于哪一班」这一说，接进来只会凭空长出一段不属于任何班次的话。
 */
export function appendForemanDelta(
  state: ForemanStreamState,
  event: SseEvent,
  sessionId: string | null,
): ForemanStreamState {
  if (event.type !== 'conversation_delta' || event.agent_type !== FOREMAN_AGENT_TYPE) return state;
  if (!sessionId || event.session_id !== sessionId) return state;
  return { ...state, text: state.text + event.text };
}

/**
 * 「别的班次正在回话」的映射（决策 220③）。
 *
 * 这一份与 `appendForemanDelta` 是**同一件事的两半**：那个字段（`session_id`）此前只被用来
 * 「丢掉不匹配的增量」——等于把「别的班次正在回话」这条事实白扔了。现在丢掉还是丢掉
 * （串台必须挡），但**先把它记下来**：⋯ 的班次列表靠它点亮「正在回话」那枚标记。
 *
 * 与 `ForemanStreamState` 分开持有一份的原因很实际：切班次会 `emptyForemanStream()`，
 * 而「甲班还在说话」这件事**不该跟着这一屏的重置一起消失**。它是纯前端状态、不进
 * localStorage（描述的是「此刻」，刷新即空是对的）。
 */
export interface ForeignActive {
  /** `sessionId → 最近一次增量到达时的本机毫秒时刻`。 */
  bySession: Record<string, number>;
}

/**
 * 增量**静默**多久算「不在回话了」。
 *
 * 没有它，一次注入后就断掉的增量会让那枚标记永远亮着——而标记的全部价值在于它**说的是真的**。
 * 取值是**两种假之间的取舍**，两边都不致命，故取小的那一头：
 *   - 取太长（初版 90s）：远端那句话**早已落地**，本机列表还没刷新过，标记于是继续说着
 *     「此刻在说话」——它是假的，而且**撤不掉**（要等超时或切进那一班）；
 *   - 取太短：一轮回话里模型卡一下就被判「不在回话了」——但下一批增量一到**它自己又会亮**
 *     （`noteForeignDelta` 每次都刷新时刻），代价只是中间那几秒没亮。
 * 20s 是「实时回话里两次增量之间的正常间隔」的宽裕量级（人眼可读的流是每几百毫秒一批）。
 * 另外两条收尾是免费的：**落地**（本机读到那份新列表时比对 `last_active_at`，见
 * {@link pruneForeignActive}）与**断开**（组件里 `streamStatus !== 'open'` 那一支）。
 */
export const FOREIGN_TTL_MS = 20_000;

export function emptyForeignActive(): ForeignActive {
  return { bySession: {} };
}

/**
 * 收到一条增量：是**别的**班次的工头增量就点亮它。
 *
 * 三条过滤与 {@link appendForemanDelta} 同源（类型 / `agent_type` / 空 `session_id`），
 * 只把「与当前班次不符」从「丢弃」改成「记下来」。当前班次的那一份**不进这张表**：它由
 * 「本机发出且未落地」（`sending`）那一支说——两条路说的是同一件事，别记两遍。
 */
export function noteForeignDelta(
  state: ForeignActive,
  event: SseEvent,
  currentId: string | null,
  at: number,
): ForeignActive {
  if (event.type !== 'conversation_delta' || event.agent_type !== FOREMAN_AGENT_TYPE) return state;
  const sid = event.session_id;
  if (!sid || sid === currentId) return state;
  return { bySession: { ...state.bySession, [sid]: at } };
}

/** 落地 / 处理完（那一班重新读了台账）即熄灭。 */
export function forgetForeignActive(state: ForeignActive, sessionId: string): ForeignActive {
  if (!(sessionId in state.bySession)) return state;
  const next = { ...state.bySession };
  delete next[sessionId];
  return { bySession: next };
}

/** 这一班此刻「正在回话」吗（含静默超时）。 */
export function foreignIsReplying(
  state: ForeignActive,
  sessionId: string,
  now: number,
  ttl: number = FOREIGN_TTL_MS,
): boolean {
  const at = state.bySession[sessionId];
  return at !== undefined && now - at <= ttl;
}

/**
 * 清掉**说过头**的那些，两条判据：
 *
 * 1. **静默超时**：`now - at > ttl`（见 {@link FOREIGN_TTL_MS} 的取舍）；
 * 2. **已经落地**：那一班的 `last_active_at` **不早于**我们记下的时刻——回话落库会同时更新
 *    `last_active_at`（与消息插入同事务，`storage/foreman.rs`），而它比增量时刻新的意思就是
 *    「这一轮的收尾已经写进台账了」，此刻它不再「正在回话」。两边取的是同一座钟：本机记的
 *    `at` 是本机毫秒，`last_active_at` 由同一台机器上的服务端写入（决策 220③ 的同一口径）。
 *    这一条只在**手上有新鲜列表**时才判得动（调用方传 `list`），故它负责的是「本机刚读过列表」
 *    那一刻的收口；列表没刷新时兜底的是第 1 条。
 *
 * **没有该清的项时返回同一个对象**——组件里那个 5s 的清理 `$effect` 因此不会自激
 * （返回值不变 = 状态没变 = 不触发重跑）。
 */
export function pruneForeignActive(
  state: ForeignActive,
  list: readonly ForemanSessionMeta[],
  now: number,
  ttl: number = FOREIGN_TTL_MS,
): ForeignActive {
  const landed = new Map<string, string>();
  for (const s of list) landed.set(s.id, s.last_active_at);
  let dropped = false;
  const next: Record<string, number> = {};
  for (const [sid, at] of Object.entries(state.bySession)) {
    const active = landed.get(sid);
    const landedSince = active !== undefined && Date.parse(active) >= at;
    if (now - at <= ttl && !landedSince) next[sid] = at;
    else dropped = true;
  }
  return dropped ? { bySession: next } : state;
}

/**
 * 收尾：POST 拿回的那句话是权威值，用它收敛流式文本。
 *
 * **空 / 全空白的回话不得覆盖已到达的文字**——那种回话只说明「这一轮没有新内容」，
 * 拿它收敛会把用户已经看到的字擦掉（票 03：不丢已经出现的文字）。
 */
export function settleForemanStream(
  state: ForemanStreamState,
  reply: string | null,
): ForemanStreamState {
  return {
    text: reply && reply.trim() ? reply : state.text,
    streaming: false,
    error: null,
  };
}

/** 断流 / 出错：保留已到达的文字，只落一个说明（不整轮消失）。 */
export function failForemanStream(state: ForemanStreamState, message: string): ForemanStreamState {
  return { text: state.text, streaming: false, error: message };
}

/**
 * 失败回合在台账里的标记（`crates/core/src/pipeline/foreman.rs::FOREMAN_FAILED_TURN_MARK`
 * 的前端镜像，决策 211④ / 票 04）。
 */
export const FOREMAN_FAILED_TURN_MARK = '【没跑起来】';

/**
 * 主动播报的标记（`crates/core/src/pipeline/foreman.rs::FOREMAN_WATCH_MARK` 的前端镜像，
 * 决策 209④ / 票 06）。**值守轮不是回话**——它没人问就自己说话，名牌上要看得出来，
 * 否则值班经理会以为自己在跟它对话（而它其实是在报事件）。
 */
export const FOREMAN_WATCH_MARK = '【值守播报】';

/** 台账里一行带 id 的轮次（只取判据要用的三列）。 */
export interface LedgerRow {
  id: number;
  role: string;
  content: string;
}

/** 这批轮次里**带失败标记**的那些行的 id。 */
export function failedLedgerRowIds(rows: LedgerRow[]): Set<number> {
  return new Set(
    rows
      .filter((m) => m.role === 'system' && m.content.startsWith(FOREMAN_FAILED_TURN_MARK))
      .map((m) => m.id),
  );
}

/**
 * 这一次失败**已经**在台账里记下了吗（决策 211④ / 票 04）。
 *
 * 后端在失败当场就把「为什么没跑起来」落成一条带标记的 `system` 行（含归因），而前端手里
 * 还有一条本地造的「发送失败」轮（承载传输层报文与配对入口）。重取台账成功之后，两行会在
 * 时间线上说同一件事——人得自己分辨哪条是真的。台账那一行更全、刷新之后还在，故本地那行退场。
 *
 * `before` 是**发送之前**已有的失败行 id：不带上它，一次早先的失败会让此后每一次真实断网
 * （请求根本没到后端，台账不会多出任何行）都静默——而那种情况正是本地那行存在的理由。
 */
export function ledgerOwnsTheFailure(rows: LedgerRow[], before: ReadonlySet<number>): boolean {
  return rows.some(
    (m) =>
      m.role === 'system' && m.content.startsWith(FOREMAN_FAILED_TURN_MARK) && !before.has(m.id),
  );
}
