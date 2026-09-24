import { ApiError, KIND_REQUEST_TIMEOUT } from '../api/client';
import type { ForemanMessage, ForemanSessionMeta, SseEvent } from '../api/types';

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
  /**
   * 本轮已到达的**推理 / 思考**文本（决策 244）。
   *
   * 与 `text` 分开攒：合成一段之后界面分不出哪一段该当回话念、哪一段该收进折叠块。
   * 收尾（`settleForemanStream`）时它**不清空**——回话的权威值由台账给，而这一轮的
   * 思考在台账那一行里同样有（`thinking` 列），两边说的是同一件事。
   */
  thinking: string;
  /** 本轮已在发生的工具调用（决策 244），按到达顺序。 */
  tools: ForemanLiveTool[];
  /** 是否仍在流：只有 `true` 才渲染既有方块光标（`.streaming`，不新增动画位）。 */
  streaming: boolean;
  /** 断流 / 出错说明；非空时 `text` 照常显示（降级为一次性显示已收到的部分）。 */
  error: string | null;
}

/**
 * 一次工具调用在流里的现场（决策 244）。
 *
 * `phase` 是**那一刻**的状态：同一把工具先 `start`（正在查）后 `end`（查到了）/
 * `error`（没查到），两次事件合成**一条**，而不是两条——它是一件正在发生的事，
 * 不是一个又一个独立事件。判据见 {@link appendForemanTool}。
 */
export interface ForemanLiveTool {
  tool: string;
  args_summary: string;
  phase: 'start' | 'end' | 'error';
}

export function emptyForemanStream(): ForemanStreamState {
  return { text: '', thinking: '', tools: [], streaming: false, error: null };
}

/** 开一轮新回话：丢掉上一轮的残留，点亮方块光标。 */
export function beginForemanStream(): ForemanStreamState {
  return { text: '', thinking: '', tools: [], streaming: true, error: null };
}

/**
 * 增量累积。
 *
 * 三道判定都不能省：
 *
 * 1. **身份**——只认工头自己的对话增量。`/foreman/stream` 与任务流共用一条总线
 *    （决策 182⑥），漏判就会把别的任务的增量拼进值班长的话里；
 * 2. **声道**（决策 244）——`reasoning` 进 `thinking`，其余（含缺省）进 `text`。
 *    缺省按 `content` 处理：老后端不发这个字段，那正是它此前唯一见过的形状；
 * 3. **班次**（决策 204⑥）——`/foreman/stream` 把所有工头增量广播给所有订阅者，
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
  return event.channel === 'reasoning'
    ? { ...state, thinking: state.thinking + event.text }
    : { ...state, text: state.text + event.text };
}

/**
 * 工具调用事件累积（决策 244）——诉求里「工具调用也实时展示」那一半。
 *
 * 三处判据与 {@link appendForemanDelta} 同源（类型 / 身份 / 班次），只多一条**相位合并**：
 * 同一把工具的 `start` 与随后的 `end`（或 `error`）合成**一条**现场记录。不合并的话，
 * 一次 `read_task` 会在时间线上留两个「正在查」——而它其实是一次调用。
 *
 * **合并的判据是「最后一条还没收尾」**：值班长的工具调用是一条一条顺序执行的
 * （`run_tool` 在 for 循环里 await），故「最后一条仍处于 `start`」就是「这一次调用在等结果」。
 * 用工具名配对是不够的：同一轮里连着查两次 `read_task` 是常态。
 */
export function appendForemanTool(
  state: ForemanStreamState,
  event: SseEvent,
  sessionId: string | null,
): ForemanStreamState {
  if (event.type !== 'tool_event' || event.agent_type !== FOREMAN_AGENT_TYPE) return state;
  if (!sessionId || event.session_id !== sessionId) return state;
  const live: ForemanLiveTool = {
    tool: event.tool,
    args_summary: event.args_summary,
    phase: event.phase,
  };
  const last = state.tools[state.tools.length - 1];
  const tools =
    last && last.phase === 'start'
      ? [...state.tools.slice(0, -1), { ...last, phase: live.phase }]
      : [...state.tools, live];
  return { ...state, tools };
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
  // 工具事件也算「在回话」（决策 244）：一轮里它可能连着查十几次台账而**一个字都不说**
  // （决策 224 的实测：一次正常定位 16–17 次调用）。只认 `conversation_delta` 的话，
  // 那几十秒里「别的班次正在回话」是暗的——而它恰恰正在忙。
  const isForemanChatter =
    (event.type === 'conversation_delta' || event.type === 'tool_event') &&
    event.agent_type === FOREMAN_AGENT_TYPE;
  if (!isForemanChatter) return state;
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
 *
 * **`thinking` 与 `tools` 原样留着**（决策 244）：回话的权威值由台账给，而这两样在
 * 台账那一行里同样有（`thinking` 列与 `traces`）。收尾是「这一轮流完了」，不是
 * 「把刚才发生的事撤掉」——清掉的话，人在重取台账前的那一段会看到思考与工具调用
 * 突然消失。它们随后由台账那一行接管（同一份内容，多一个来源不算脏）。
 */
export function settleForemanStream(
  state: ForemanStreamState,
  reply: string | null,
): ForemanStreamState {
  return {
    text: reply && reply.trim() ? reply : state.text,
    thinking: state.thinking,
    tools: state.tools,
    streaming: false,
    error: null,
  };
}

/** 断流 / 出错：保留已到达的文字与现场，只落一个说明（不整轮消失）。 */
export function failForemanStream(state: ForemanStreamState, message: string): ForemanStreamState {
  return {
    text: state.text,
    thinking: state.thinking,
    tools: state.tools,
    streaming: false,
    error: message,
  };
}

/**
 * 台账里最大的一行 id（0 = 一行都没有）。
 *
 * 与 {@link turnLanded} 配对使用：接手一轮时记下它，落地时拿它比对。**只看 id**，
 * 与 `failedLedgerRowIds` 同一姿态（判据只该看字段，不看正文）。
 */
export function maxLedgerId(rows: LedgerRow[]): number {
  return rows.reduce((max, m) => (m.id > max ? m.id : max), 0);
}

/**
 * 这一轮落地了吗——判据是**台账尾部多了一行**（决策 260）。
 *
 * 刷新之后重新接上一轮时，本机手里**没有**那一趟 POST 的回包（它随旧页面一起走了），
 * 故「它答完了」只能从台账读：尾部冒出比 `before` 新的行（值班长的回话、操作台记的一笔、
 * 或一条失败账）就是它落了地。
 *
 * `before` 是**接手那一刻**看到的最大行 id：接手时这一轮的用户那一句已经在台账里了
 * （`say` 先落它再叫模型），故回话落地时 id 必然更大。
 *
 * **本函数只管「换行了没有」**；「没换行而服务端也不再报在跑」那一类（进程被杀——决策 223
 * 明确不做那一轮的落账）该拿已经收到的半截字怎么办，是 {@link resolveFollowOutcome} 的事。
 */
export function turnLanded(rows: LedgerRow[], before: number): boolean {
  return rows.some((m) => m.id > before);
}

/** 跟的那一轮收场了，接下来怎么办（见 {@link resolveFollowOutcome}）。 */
export type FollowOutcome =
  /** 继续跟：它还在跑。 */
  | { kind: 'keep' }
  /** 落了地：台账那一行接管（它会带着完整回话进来），本地那一段该收掉了。 */
  | { kind: 'settled' }
  /**
   * **没落地而服务端也不再报在跑**（进程被杀 / 重启）：这一轮永远不会再落一行。
   *
   * 收成一条**失败轮**——不是把半截字清掉。这是本仓那条一以贯之的纪律的落点：
   * 「已经出现的文字，任何一支都不许把它清掉」（本模块文件头，票 03 立的）。
   * 判据与呈现分开：**留什么**由这一支说了算，**怎么说**在 {@link FOREMAN_LOST_TURN_SUFFIX}。
   */
  | { kind: 'lost' };

/**
 * 跟的那一轮这一趟问下来收场没有、以及收成什么样（决策 260 裁决③的落实）。
 *
 * 两个读数各答一半，且**不能互相替代**：
 * - {@link turnLanded}（台账尾部有没有新行）——答「它答完了吗」；
 * - `turn_in_flight`——答「服务端还认不认这一轮」。
 *
 * 两件都是假，就是**死轮**：回话永远不会来（决策 223 明确不做进程退出那一轮的落账）。
 * 此时**不许清字**——那一段是这一轮留下的全部，清掉正是用户报的那条毛病
 * （「刷新就看不到实时对话流」）在死轮场景下的残留子集。故这一支收成一条失败轮，
 * 把已经收到的部分原样留着（{@link failForemanStream} 的既有姿态），并说清发生了什么。
 *
 * `|| !turn_in_flight` **不能摘**（只按 `turnLanded` 判的话，死轮会永远跟下去——每 3s
 * 一趟，永远不落地）。
 */
export function resolveFollowOutcome(
  rows: LedgerRow[],
  anchor: number,
  turnInFlight: boolean,
): FollowOutcome {
  if (turnLanded(rows, anchor)) return { kind: 'settled' };
  return turnInFlight ? { kind: 'keep' } : { kind: 'lost' };
}

/**
 * 死轮那一条失败轮的说明（决策 260 裁决③）。
 *
 * 姿态与 {@link FOREMAN_TIMEOUT_SUFFIX} 同一份：**说清事实 + 说清下一步**，不认领没发生的
 * 事（「它没答完」而不是「它答错了」），也不假装还能等（那一轮永远不会再落一行）。
 */
export const FOREMAN_LOST_TURN_SUFFIX =
  '这一轮没有答完就断了（服务端已不再跑它，回话不会再来）。上面是已经收到的部分：' +
  '可以再问一次，或者切走再切回本班次看看台账里有没有留下别的痕迹。';

/**
 * 「这一轮还在服务端继续」这句话的正文（决策 223）。
 *
 * 服务端那一轮**不随这次请求一起死**：本地放弃（本地超时 / 关页 / 换网）只丢掉这一次的
 * 同步回包，回话照旧落库、增量照旧走 `/foreman/stream`。所以「本地等不到回包」不等于
 * 「这一轮失败」——说成失败会让人重发一句，而服务端那一轮很可能正在把它答完
 * （2026-09-18 实测的两次「没回话」正是这个形状：话说了，回话没等到，界面与库都只剩
 * 一条孤立的用户行）。
 */
export const FOREMAN_TIMEOUT_SUFFIX =
  '本地已不再等这一轮，但它在服务端仍在继续：回话会随流式增量出现，切走再切回本班次也能看到。';

/**
 * 这次失败是「本地等不到回包」吗——**按 `kind` 判，不按报文字样**（票 06，决策 259 的延伸）。
 *
 * 超时那句话由 `api/client.ts::mapRequestError` 构造，此前这里 `startsWith('请求超时')`
 * 与它隔着模块逐字同步——报文即接口，改一句话就断（与配对判据是同一个反模式，票 04 先清了
 * 那一半）。超时是前端本地 `AbortSignal.timeout` 的产物、没有 HTTP 应答体，故 `kind`
 * 由 `mapRequestError` 在构造点带上（`KIND_REQUEST_TIMEOUT`），这里只认那枚字段。
 */
export function isRequestTimeout(err: unknown): boolean {
  return err instanceof ApiError && err.kind === KIND_REQUEST_TIMEOUT;
}

/**
 * 失败轮的说明：超时那一类补上「还在跑」的实情，其余原样返回。
 *
 * `timedOut` 由调用方**趁 `ApiError` 还在手**判好传进来（{@link isRequestTimeout}）——
 * 判据上移、字符串拼接留在这儿：错误降级成流里的字符串之后 `kind` 就丢了，
 * 本函数不再（也不能）摸正文。
 */
export function failureNotice(message: string, timedOut: boolean): string {
  return timedOut ? `${message} ${FOREMAN_TIMEOUT_SUFFIX}` : message;
}

/**
 * 台账里一行带 id 的轮次——判据只需要这两列。
 *
 * `kind` 是**后端给的**（决策 252）：界面不再拿正文前缀去猜这一行是人的话、操作台记的账、
 * 还是没跑起来的那一轮。刻意*不*取 `content`——下面两条判据只该看字段，不看正文。
 *
 * 类型从 `ForemanMessage` **摘**（`Pick`）而不是重抄那四个字面量：抄一遍就是又一份枚举副本
 * （决策 253② 要挡的形状），且后端将来加一种 `kind` 时这里不会跟着宽、只会静默落在
 * 「不是 failed」那一支。`Pick` 让它们是**同一个**类型。
 */
export type LedgerRow = Pick<ForemanMessage, 'id' | 'kind'>;

/** 这批轮次里**没跑起来**的那些行的 id。 */
export function failedLedgerRowIds(rows: LedgerRow[]): Set<number> {
  return new Set(rows.filter((m) => m.kind === 'failed').map((m) => m.id));
}

/**
 * 这一次失败**已经**在台账里记下了吗（决策 211④ / 票 04）。
 *
 * 后端在失败当场就把「为什么没跑起来」落成一条 `kind = "failed"` 的行（含归因），而前端
 * 手里还有一条本地造的「发送失败」轮（承载传输层报文与配对入口）。重取台账成功之后，两行
 * 会在时间线上说同一件事——人得自己分辨哪条是真的。台账那一行更全、刷新之后还在，故本地
 * 那行退场。
 *
 * `before` 是**发送之前**已有的失败行 id：不带上它，一次早先的失败会让此后每一次真实断网
 * （请求根本没到后端，台账不会多出任何行）都静默——而那种情况正是本地那行存在的理由。
 */
export function ledgerOwnsTheFailure(rows: LedgerRow[], before: ReadonlySet<number>): boolean {
  return rows.some((m) => m.kind === 'failed' && !before.has(m.id));
}
