import {
  archiveForemanSession,
  createForemanSession,
  getForemanAttention,
  getForemanSession,
  getForemanSessions,
  getTask,
} from '../api/client';
import type {
  AllowedAction,
  BranchCursor,
  ForemanAttention,
  ForemanSession,
  ForemanSessionMeta,
  SseEvent,
  TaskListItem,
} from '../api/types';
import { TaskStream, type StreamStatus } from '../realtime/connection';
import {
  appendForemanEvent,
  emptyForemanStream,
  emptyForeignActive,
  failForemanStream,
  FOREMAN_LOST_TURN_SUFFIX,
  forgetForeignActive,
  maxLedgerId,
  noteForeignDelta,
  resolveFollowOutcome,
  type ForemanStreamState,
  type ForeignActive,
} from '../realtime/foreman';
import {
  loadSeen,
  markSeen,
  pruneSeen,
  saveSeen,
  saveSessionId,
  seedBaselineIfFirstRun,
  type SeenAt,
} from '../lib/talkSessions';
import { isPairingRequired } from '../lib/sharePairing';
import { router, writeQuery } from '../router.svelte';

/**
 * 与后端 `routes/foreman.rs::SESSION_PAGE_LIMIT` 同一个数——判「首屏读满」的尺。
 * （决策 354 附注，票 04：将由 `GET /foreman/session` 应答回显的 `page_limit` 取代。）
 */
const SESSION_PAGE_LIMIT = 500;

/**
 * 对讲台的**在飞现场**（决策 275）：这一轮正在产的步骤、正在等的那一趟回话，
 * 以及那条 `/foreman/stream` 连接，全住在这里——**页面来去，它不动**。
 *
 * ## 为什么必须搬出组件
 *
 * 这些状态原先住在 `Talk.svelte` 的组件作用域里，于是**切一下界面（看板 / 设置 / 指标）
 * 再回来，本轮已经收到的输出整段不见**：组件被销毁，攒了十几分钟的步骤与文字跟着一起没了，
 * 而那一轮在服务端还在跑（实测：一次正常的定位能跑 10 分钟以上、三十几次模型调用）。
 * 回来时只能重新接上（决策 260 的「跟一轮」），但 SSE 没有回放——重接得上的是**此后**的
 * 增量，此前那些字永远不会再到达。
 *
 * **台账生命周期也住在这里**（决策 354①）：`session` / `sessionList` / `loading` /
 * `loadError` / `hasMoreEarlier` 与 reload / loadEarlier / switchTo / 归档坠落 / seen 标记
 * 一并搬进 store——它们描述的是**那一班**，而「某一班的事发生在页面之外」（收尾、归档、
 * 切走之后落地的回话）正是这个 store 存在的理由。台账搬进来之后，epoch 喊话协议
 * （`ledgerEpoch` + `bindRecalibrate`）两端都没了：收尾自己 `reload()`，不再朝在屏的
 * 那一页喊话。
 *
 * 与看板 store 同一姿态（`stores/board.svelte.ts`）：连接也归 store，`init()` 由
 * `App.svelte` 起一次、`dispose()` 收一次；页面只 `watch()` 认下「我在看哪一班」。
 *
 * ## 与决策 204③ / 220⑤ 的边界（一个字不动）
 *
 * **换班次仍然清现场**：`watch()` 认到一个不同的 id 就把在飞现场倒空——「切走时那一轮
 * 从视野里撤下、回话照旧落台账」是那条决策的原口径，本次只改「切页面」（同一班次）。
 */
class TalkStore {
  /**
   * 这条连接此刻认哪一班（判据在 `realtime/foreman.ts::appendForemanEvent` 的班次守卫）。
   *
   * **同步更新**：页面换班时立刻改它，而不是等台账读回来——中间到达的增量属于哪一班，
   * 靠的就是这一刻的值（与 `Talk.svelte` 原先那个 `currentId` 是同一条纪律）。
   */
  sessionId = $state<string | null>(null);

  /** 在飞一轮的现场：按发生顺序的步骤 + 是否仍在流 + 断流说明。 */
  stream = $state<ForemanStreamState>(emptyForemanStream());

  /** 发送在途（流还没开时也先亮一轮，免得人以为没按上）。 */
  sending = $state(false);

  /** 正在发的那句话（台账里还没有它的回话，故先以乐观轮显示）。 */
  pendingText = $state<string | null>(null);

  /**
   * **重新接上的一轮**的锚点（决策 260）：非空 = 此刻在跟一轮，值是接手那一刻台账里
   * 最大的行 id——那之后多一行就是这一轮落了地。
   */
  followingSince = $state<number | null>(null);
  /**
   * 「转去对话」带过来的一句话（决策 289 / 票 04）：值守台账上那条播报的摘录。
   * 值守账把它放进这里 → 切到人的时间线时对讲台把它预填进输入坞并聚焦——
   * 「把这件事带进人的时间线」的载体。消费即清（预填是**草稿**，人可以改可扔）。
   */
  watchDraft = $state<string | null>(null);

  /** 当前失败是不是**配对缺失**（判据趁 `ApiError` 还在手判好，决策 259）。 */
  pairingNeeded = $state(false);

  /**
   * 排队发送的账（票 04 of talk-live-identity，2026-09-29 决议）：**按班次**各一列。
   *
   * 一轮在飞时输入坞解锁，发出即入队，当前轮收口后由在屏的那一页自动出队发送——
   * 队列因此必须住 store（跨页面存活，与决策 275 同一判据），且**必须按班次分列**：
   * 排在甲班的话不许在切到乙班收口时发进乙班（决策 204⑥ 的队列版）。
   */
  queue = $state<Record<string, string[]>>({});

  /**
   * 「上一轮没回来，排队的话扣住了」——按班次一记号（票 04）。
   *
   * 死轮 / 中断时队列**不自动照发**：把「它不会再来」的可见性留在人手里，
   * 确认（清记号、恢复出队）或清空队列两个出口都在坞里。
   */
  queueHeld = $state<Record<string, boolean>>({});

  /** 排队：入队（在飞时发出的话先住这里，不直接发）。 */
  enqueue(sessionId: string, text: string): void {
    this.queue = { ...this.queue, [sessionId]: [...(this.queue[sessionId] ?? []), text] };
  }

  /** 就地改一条排队的话（票 04：可见可编辑）。 */
  editQueued(sessionId: string, index: number, text: string): void {
    const list = this.queue[sessionId] ?? [];
    if (index < 0 || index >= list.length) return;
    const next = list.slice();
    next[index] = text;
    this.queue = { ...this.queue, [sessionId]: next };
  }

  /** 撤回一条排队的话（票 04：可见可撤回）。 */
  removeQueued(sessionId: string, index: number): void {
    const list = this.queue[sessionId] ?? [];
    this.queue = {
      ...this.queue,
      [sessionId]: list.filter((_, i) => i !== index),
    };
  }

  /** 出队：拿走最前面那条（收口后由在屏的那一页发出去）。 */
  takeQueued(sessionId: string): string | null {
    const list = this.queue[sessionId] ?? [];
    if (list.length === 0) return null;
    this.queue = { ...this.queue, [sessionId]: list.slice(1) };
    return list[0];
  }

  /** 扣住 / 放行这一班的队列（死轮扣住，人确认后放行）。 */
  setQueueHeld(sessionId: string, held: boolean): void {
    this.queueHeld = { ...this.queueHeld, [sessionId]: held };
  }

  /**
   * 折叠三表（票 03 of talk-live-identity）：工位回执 / 思考 / 工具详情的展开态。
   *
   * 自决策 301 的组件作用域搬进 store——它们描述的是**那一轮**，不是这一屏，页面来去
   * 不该把它们重置（「本机发送中」与「回来接上」的形态差有一半就差在这）。键随轮稳定
   * （台账轮锚 `m<id>`、在飞轮拼到半截行时直接用行 id），收口不再需要搬键；
   * `live` 键只剩本机发送那一趟在用，收口接力照旧（`carryLive*Open`）。
   * 不进 localStorage：跨页面存活、**不跨刷新**（决策 217 的边界一字不动）。
   */
  receiptOpen = $state<Record<string, boolean>>({});
  thinkingOpen = $state<Record<string, boolean>>({});
  toolOpen = $state<Record<string, boolean>>({});

  /**
   * 「别的班次正在回话」（决策 220③）。住在 store 里而不是页面里：它说的是**此刻**，
   * 而此刻不因为你去看了一眼看板就不作数（断流/落地两条收口仍在，见 `onStatus` 与
   * `pruneForeign`）。
   */
  foreign = $state<ForeignActive>(emptyForeignActive());

  /** 传输层状态（输入坞那一行断线告知读它）。 */
  status = $state<StreamStatus>('idle');

  /**
   * 这本账是谁的（决策 286 / 票 04）：`talk`（人的班次）/ `watch`（值守台账）。
   *
   * 两个页面（`/talk` 与 `/talk/watch`）共用这个 store 单例，挂载时各自认下；
   * reload 取列表与台账都按它带 `?kind=`——「指定的 id 不在这一班的列表里」按账本各自判。
   * seen 标记（看过表、基线、兜底文件）只属于**人的班次列表**，见 `rememberLanding`。
   */
  kind = $state<'talk' | 'watch'>('talk');

  /** 会话台账（时间线的权威内容；每次回话后重取，不自攒一份账）。 */
  session = $state<ForemanSession | null>(null);

  /** 未归档的班次（chip 行的数据源），按最近活动倒序。 */
  sessionList = $state<ForemanSessionMeta[]>([]);

  /** 首次装载在途（与「还没读到」一起构成时间线的空态）。 */
  loading = $state(true);

  /** 读台账失败的说明（页面把关渲染；读不到台账时这一页必须还说得出话）。 */
  loadError = $state<string | null>(null);

  /** `loadError` 是不是**配对缺失**（按后端给的 `kind` 判，决策 259）——挂不挂配对入口读它，不读报文字样。 */
  loadErrorPairing = $state(false);

  /**
   * 上面还有更早的消息没加载（票 05：向上游标）。**首屏读满 `SESSION_PAGE_LIMIT`
   * 条才置真**——500 条内的班次这条路径一次都不会走到，加载与从前逐字一致、零额外
   * 请求；到头（游标回空段）置假，不再有向上的动作。
   */
  hasMoreEarlier = $state(false);

  /** 向上加载在途：滚到顶会连着 fire 一串 scroll 事件，靠它去重。 */
  loadingEarlier = $state(false);

  /**
   * 「显示已归档」开关（票 06）：关 = 现状（chip 行只见活跃班次）。只换**列表给谁看**，
   * 不动当前在读的那一班；它是一次浏览动作，不是身份——store 寿命比页面长，
   * 页面挂载时把它拨回关（`Talk.svelte` 的 onMount）。
   */
  showArchived = $state(false);

  /**
   * 未消费待办的**只读**读数（决策 307，票 executor-never-returns 06）。
   *
   * `null` = 还没读到 / 读不到——那时**不渲染**页头那枚读数（未接线、离线都走这一支）。
   * 它与 `session.turn_in_flight` 无关，这正是它存在的一半理由：值守轮排队时（决策 289）
   * 那条「值守台账 · 正在跑」的 crumb 根本不出现，而系统此刻正在报警。
   * 与班次同一次重读刷一遍（`reload` 尾部）。
   */
  attention = $state<ForemanAttention | null>(null);

  /**
   * 「哪一条我看过」的时刻表（决策 220③ 的「有新动静」判据）。
   *
   * store 寿命内只从 localStorage 读一次（构造时）；此后写穿（`saveSeen`）。
   * 页面原先每次挂载重读一遍——数据同一份，读写口径不变。
   */
  seen = $state<SeenAt>(loadSeen());

  /** 这份表建过基线没有：第一次读到列表时把当下当基线（否则第一屏每条都带「有新动静」）。 */
  private seenSeeded = false;

  /** 每个 pending 任务的详情（`allowed_actions` 只在详情里下发，决策 101）。 */
  details = $state<Record<string, { actions: AllowedAction[]; cursors: BranchCursor[] }>>({});

  /** 详情重拉的指纹（上一次的 key）：非响应式——它只在这条去重链里读写。 */
  private lastDetailsKey: string | null = null;

  /** 落地哨（决策 260）的计时器：跟着一轮走，收场即停。 */
  private sentinel: ReturnType<typeof setInterval> | null = null;

  private conn: TaskStream | null = null;

  /** 起连接。幂等（App 装载时叫一次；之后页面来去都不再碰它）。 */
  init(): void {
    if (this.conn) return;
    this.conn = new TaskStream(
      '',
      {
        onEvent: (_taskId, event) => this.note(event),
        onStatus: (_taskId, status) => {
          this.status = status;
          // 断开即熄灭（决策 220③）：「别的班次在回话」是一份**描述此刻**的映射，
          // 连接不在的时候它说的就不再是此刻——留着只会变成一个撤不掉的假标记。
          if (status !== 'open') this.foreign = emptyForeignActive();
        },
        // 重连成功后补一次全量（票 03，stream-self-heal）：SSE 无回放。
        // 台账住 store（决策 354①）之后这里就是 store 自己 reload——页面不在屏上
        // 也照样对齐（回来那一次的装载本来就会重读）。
        onRecalibrate: () => void this.reload(),
      },
      { path: '/foreman/stream' },
    );
    this.conn.start();
    if (typeof document !== 'undefined') {
      document.addEventListener('visibilitychange', this.onVisible);
    }
  }

  dispose(): void {
    if (typeof document !== 'undefined') {
      document.removeEventListener('visibilitychange', this.onVisible);
    }
    this.stopSentinel();
    this.conn?.stop();
    this.conn = null;
    this.status = 'closed';
  }

  private onVisible = (): void => {
    if (typeof document === 'undefined' || document.visibilityState !== 'visible') return;
    this.conn?.reconnectNow();
  };

  /**
   * 认下「我在看哪一班」。
   *
   * 换班就倒空在飞现场（决策 204③ / 220⑤，见类头的边界）——**但「从没有班次到有班次」
   * 不算换班**：那正是「这台机器上一个班次都没有，本机发出的第一句话自己开了一班」那条路
   * （`sendTurn` 里 `sid` 为空那一支），此刻在飞现场（乐观轮 + 刚开的流）说的就是**这一班**，
   * 清掉等于把人刚发出去的那句话从屏上抹掉。
   */
  watch(id: string | null): void {
    if (id === this.sessionId) return;
    const switchingAway = this.sessionId !== null;
    this.sessionId = id;
    if (switchingAway) this.resetLive();
  }

  /**
   * 一轮收场了（本机那一趟 POST 回来了 / 断在那里了）：现场退场 + **自己重读台账**。
   *
   * 三条收尾路径（成功 / 换班后作废 / 失败）都走它——**别各自写字段**：漏掉重读那一下的
   * 症状正是本轮输出整段不见。决策 354① 之前它靠 `ledgerEpoch` 朝在屏的那一页喊话；
   * 台账搬进 store 之后喊话两端都没了，这里直接 reload。
   */
  settleTurn(): void {
    this.stream = emptyForemanStream();
    this.pendingText = null;
    this.pairingNeeded = false;
    void this.reload();
  }

  /** 倒空在飞现场：那一轮不再属于这一屏（换班 / 落地 / 新一轮开跑）。 */
  resetLive(): void {
    this.stream = emptyForemanStream();
    this.pendingText = null;
    this.followingSince = null;
    this.pairingNeeded = false;
    this.stopSentinel();
  }

  /**
   * 落地哨（决策 260 / 275）：每 3s 问一次服务端，这一轮收场没有。
   *
   * **住在 store 而不是页面**（决策 275）：它与「跟一轮」这件事实同寿——页面切走之后
   * 那一轮照旧在跑，收口也得照旧发生（不收口的话，回来时看到的是**一段不再更新的旧现场**
   * 叠在台账那一行上）。收场之后由 {@link syncFollowing} 自己 reload 台账（决策 354①）；
   * 页面不在屏上时重读照样发生——台账本来就是 store 的了，回来那一次的装载读到的就是新的。
   *
   * 间隔取得比另外两条心跳（页面里的 5s / 10s）更宽：跟随的那一轮本来就有 SSE 增量在动，
   * 人看得见它在忙；这一条只负责把「已经答完」这件事及时收口，快慢几秒不影响。
   */
  private startSentinel(): void {
    if (this.sentinel !== null) return;
    this.sentinel = setInterval(() => void this.pollFollow(), 3_000);
  }

  private stopSentinel(): void {
    if (this.sentinel === null) return;
    clearInterval(this.sentinel);
    this.sentinel = null;
  }

  private async pollFollow(): Promise<void> {
    const anchor = this.followingSince;
    if (anchor === null) {
      this.stopSentinel();
      return;
    }
    try {
      const payload = await getForemanSession(this.sessionId);
      // 这一趟之间放了手 / 换了班（或又接了新一轮）：这一份读数已经不对着那一轮了
      if (this.followingSince !== anchor) return;
      const outcome = resolveFollowOutcome(payload.messages ?? [], anchor, payload.turn_in_flight);
      if (outcome.kind === 'keep') return;
      // 收口（settled / lost 两支都在里面），它会顺手 reload 台账
      this.syncFollowing(payload);
    } catch {
      // 读不到不影响这一屏：SSE 增量照旧在动，下一趟再收口
    }
  }

  /**
   * 一个流事件落进这一格：先归位「别的班次在回话」，再攒进现场。
   *
   * **到得就攒，不按「在不在等一轮」挑**（票 02 改）：接不接由**渲染时对着快照**判
   * （`spliceAccepts`，`seq > seq0` 才进时间线），到达时挑反而造出一个丢字的窗口——
   * 快照（`GET /foreman/session`）还没读回来的那几拍里，事件若在这里被丢掉，
   * 快照的 `seq0` 再准也补不回它（SSE 无回放，决策 275）。
   *
   * 班次守卫照旧（`appendForemanEvent` 里那一道）：别的班次、流水线的事件进不来。
   * 没在跟也没在发时攒下的尾巴由**下一次读台账**收口（`syncFollowing` 里那一支）——
   * 那一轮已经收场的话，它的完整行就在台账里，尾巴本就多余。
   */
  note(event: SseEvent): void {
    this.foreign = noteForeignDelta(this.foreign, event, this.sessionId, Date.now());
    this.stream = appendForemanEvent(this.stream, event, this.sessionId);
  }

  /**
   * 按这一趟读到的台账收口「跟不跟这一轮」（决策 260 / 275）。
   *
   * 三条判据，各自对应一种真实情形：
   *
   * 1. **本机在发**（`sending`）：跟着的是本机那一趟，`sendTurn` 的收尾负责放手——
   *    这里一个字都不动。
   * 2. **服务端说没在跑**：收场。落了地就**交棒给台账那一行**（在飞现场倒空）；
   *    没落地而它又不再跑，就是**死轮**（进程被杀 / 重启——决策 223 明确不做那一轮的落账）：
   *    **已经收到的部分原样留着**（`failForemanStream` 的姿态），只多一句说明
   *    （决策 260 裁决③）。
   * 3. **服务端说在跑、本机没在发**：接手。`followingSince` 记**接手那一刻的最大行 id**，
   *    尾部此后多一行就是它落了地（这趟 POST 的回包本机没有，只能这么读）。
   *
   * 已经接手且仍在跑时**不改 `followingSince`**：它是「落地」那条判据的锚点，每趟轮询
   * 重记一次的话锚点会跟着往前爬，落地的行反而永远比它小。
   */
  syncFollowing(payload: ForemanSession): void {
    if (this.sending) return;
    const anchor = this.followingSince;
    if (anchor === null) {
      if (payload.turn_in_flight) {
        this.followingSince = maxLedgerId(payload.messages ?? []);
        this.lightStreaming();
        this.startSentinel();
      } else if (this.stream.events.length > 0 || this.stream.steps.length > 0) {
        // 没在跟、服务端也没在跑：手里攒的直播是**上一轮的残渣**（收场发生在这一屏
        // 之外 / 没有人接手过）——清掉。台账里那一轮的完整行会把它接住（票 02：
        // 到得就攒之后，这一支是尾巴唯一的收口）。
        this.stream = emptyForemanStream();
      }
      return;
    }
    const outcome = resolveFollowOutcome(payload.messages ?? [], anchor, payload.turn_in_flight);
    if (outcome.kind === 'keep') return;
    this.followingSince = null;
    this.stopSentinel();
    if (outcome.kind === 'lost') {
      this.stream = failForemanStream(this.stream, FOREMAN_LOST_TURN_SUFFIX);
      // 队列扣住（票 04 of talk-live-identity）：上一轮没回来，排在后面的话**不自动照发**
      // ——把「它不会再来」留在人手里，确认或清空两个出口都在坞里。
      if (this.sessionId) this.setQueueHeld(this.sessionId, true);
      void this.reload();
      return;
    }
    // **中断行同扣**（票 04「死轮/中断扣住等确认」）：这一轮收在启动恢复标的
    // `interrupted` 终态上——回话不会来了，与死轮同罪：排在后面的话**不自动照发**，
    // 等坞里「确认 / 清空」两个出口。正常收口（`interrupted: false`）照旧自动出队。
    if (outcome.interrupted && this.sessionId) this.setQueueHeld(this.sessionId, true);
    // 落地：台账那一行接管（它带着完整回话与段序进来），在飞那一段退场。
    // **先放手再交棒**：`syncFollowing` 在收到 `turn_in_flight` 为真时会重新立锚点，
    // 那一格是「回话落了库、而同一班紧接着又起了一轮」（值守轮插进来）——此时该跟的是
    // **新那一轮**，锚点必须按它落库后的台账重记。
    this.stream = emptyForemanStream();
    this.pendingText = null;
    this.pairingNeeded = false;
    // 回话落了库、而同一班紧接着又起一轮（值守轮插进来）：接着跟**新那一轮**，
    // 锚点按它落库后的台账重记。
    if (payload.turn_in_flight) {
      this.followingSince = maxLedgerId(payload.messages ?? []);
      this.lightStreaming();
      this.startSentinel();
    }
    // 台账那一行接管了：自己重读一次（决策 354①——这一声原先喊给在屏的那一页，
    // 现在台账就是 store 的，直接 reload）。
    void this.reload();
  }

  /**
   * 本地放弃后的**无条件**接手（决策 288 / 票 05）。
   *
   * 与 {@link syncFollowing} 的差别只有一条：「服务端此刻说在跑」不是接手的前提——
   * 本地超时那一刻读到的 `turn_in_flight` 很可能已经过期（重读失败 / 正好落地），
   * 按它决定接不接会把「还在跑」误判成「没了」。落地哨的下一趟轮询按 fresh 读数收场：
   * 落地 → 台账接管；还在跑 → 继续跟；不再跑也没落地 → 收成死轮失败。
   * `anchor` 是**重读之后**的最大行 id（落地判据的起点；用户那一句已在其内）。
   */
  followAfterGiveUp(anchor: number): void {
    this.followingSince = anchor;
    this.lightStreaming();
    this.startSentinel();
  }

  /**
   * 把 `stream.streaming` 点亮（票 02 of talk-live-identity）。
   *
   * 「接上的一轮」此前永远点不亮它——全仓唯一写 `streaming: true` 的是
   * `beginForemanStream()`，只有发送那条路会走。而一切挂在 `streaming` 上的形态
   * （光标、「正在想」ticker、贴底跟随、`partial` 不误报「流断了」）接上之后全部退化。
   * 接手即点亮：**接上路径的形态要与「本机在发」不可区分**。已点亮时返回同一个对象，
   * 不白触发依赖它的效果。
   */
  private lightStreaming(): void {
    if (this.stream.streaming) return;
    this.stream = { ...this.stream, streaming: true };
  }

  // ─────────────── 台账生命周期（决策 354①，票 talk-store-ledger 01）───────────────

  /**
   * 重读班次列表与某个班次的台账。
   *
   * `want` 三态：不给 = 接着看当前这一班；给 id = 切过去；给 `null` = 回到服务端默认
   * （最近活动的未归档班次）。指定的班次不在了（别的设备归档了它、或这个 id 本来就不存在）
   * 时**回落到默认**而不是报错——切班次的地方没有出错这一说，只有「去最近有人说话的那一班」。
   *
   * **过期回包守卫**（票 01 of talk-live-identity，决策 204⑥ 的 reload 版）：给过显式
   * `want` 的那一趟，进门前先把落点认下来（`watch`——守卫的锚与落点同一格），
   * 此后**每个 await 之后**比对「目标 ≠ `sessionId` 即整包丢弃」：不写 `session`、
   * 不 watch、不改地址、不收口。没有这道比对，切班次之前发出的那一趟回来会把整屏
   * 拖回旧班次——这正是「对讲台出现非本次会话的内容」的根因（并发 reload 是常态：
   * 挂载装载、地址回退、重连校准、可见性恢复都走这里）。不带 `want` 的续读以当下的
   * `sessionId` 为锚，同一纪律。
   *
   * 落点与发起时不同（指定的班次不在了、回落默认）：`watch(landed)` 那一步自会把
   * 身份换过去，在途的 send 结果由它自己的比对作废——决策 204⑥ 的语义不变。
   */
  async reload(want?: string | null): Promise<boolean> {
    const wanted = want === undefined ? this.sessionId : want;
    if (want !== undefined) this.watch(wanted ?? null);
    try {
      // 两本账各读各的（票 04 / 决策 286）：`?kind=` 缺省只回人的班次，值守账要显式要。
      // 「指定的 id 不在这一班的列表里」因此按账本各自判——把 talk 的 id 递到值守账
      // （或反过来，刷新后的 localStorage 兜底就是这条路径）回落到本账的默认落点。
      const list = await getForemanSessions(undefined, this.kind, this.showArchived);
      // 过期回包守卫第一道（票 01）：这一趟之间换班了（`watch` 已被别的路径改口），
      // 后面的整包作废——不写 session、不改地址。
      if (this.sessionId !== wanted) return false;
      const target = wanted;
      // 「找得到」多认一种（票 06）：**已经在读的那一班**——归档开关关着时它不在
      // 列表里，但人正看着它，把人弹去默认班才是错。只宽这一种：地址 / 兜底文件指到
      // 一班**没加载的**归档班，照旧回落默认（决策 204⑥：指定的不在列表里 → 去最近
      // 有人说话的那一班）。
      const known =
        !!target &&
        (list.sessions.some((s) => s.id === target) ||
          target === (this.session?.session?.id ?? null));
      const payload = await getForemanSession(known ? target : null, undefined, this.kind);
      // 守卫第二道（票 01）：`sessionId` 已经不是进门前认下的那一班——这一包是
      // 旧目标的台账，整包丢弃。
      if (this.sessionId !== wanted) return false;
      this.sessionList = list.sessions;
      const landed = payload.session?.id ?? null;
      // 同一班的重读（一轮落地后常见）：**已在屏的更早段留着**——重读只回最近 500 条，
      // 直接盖上去会把滚上去加载的那段历史变没（票 05：已加载的消息不重不漏）。
      // 换班（`landed !== prevId`）不并：那是另一班的账。
      const prevId = this.session?.session?.id ?? null;
      const fresh = payload.messages ?? [];
      if (landed !== null && landed === prevId && fresh.length > 0) {
        const older = (this.session?.messages ?? []).filter((m) => m.id < fresh[0].id);
        if (older.length > 0) payload.messages = [...older, ...fresh];
      }
      this.session = payload;
      // 首屏读满才有「更上一层」可言；同一班重读也按这一拍重算（台账可能长过了 500）。
      this.hasMoreEarlier = fresh.length >= SESSION_PAGE_LIMIT;
      this.watch(landed);
      this.rememberLanding(landed, payload.session);
      // 重新接上一轮（决策 260 / 275）：服务端说这一班此刻有一轮在跑，而本机没在等它
      // （`sending` 的现场只属于本机发出的那一趟）。此时把「跟」这件事立起来，增量
      // 才会照旧接进时间线——否则刷新之后实时回话整段看不见，只剩落地后重读台账；
      // 而**切走再回来**那一趟靠 store 里没走的在飞现场接着（决策 275）。
      this.syncFollowing(payload);
      // 未消费待办的读数与班次同一次重读刷一遍（决策 307，票 06）。
      await this.loadAttention();
      this.loadError = null;
      this.loadErrorPairing = false;
      return true;
    } catch (err) {
      this.loadError = (err as Error).message;
      this.loadErrorPairing = isPairingRequired(err);
      return false;
    } finally {
      this.loading = false;
    }
  }

  /**
   * 滚到顶加载更早的消息（票 05：`before_id` 向上游标）。
   *
   * 返回**有没有接上新段**：阅读位置的保活（先量高度、`tick()` 等 DOM 长高之后把
   * scrollTop 补上长高的那一截——眼睛看着的那一行 stays put）是**这一屏**的事，
   * 留在页面（`Talk.svelte` 的包装）；store 只管账。
   * 到头的信号就是**空段**（后端不另给 `has_more`）；到头后 `hasMoreEarlier` 置假，
   * 这条路不再走到。读失败只收手：位置不动、游标不废，下一次滚到顶自然重试。
   */
  async loadEarlier(): Promise<boolean> {
    if (this.loadingEarlier || !this.hasMoreEarlier) return false;
    const oldest = this.session?.messages?.[0]?.id;
    if (oldest == null) {
      this.hasMoreEarlier = false;
      return false;
    }
    const gen = this.sessionId;
    this.loadingEarlier = true;
    try {
      const page = await getForemanSession(this.sessionId, undefined, this.kind, oldest);
      // 这一趟之间换班 / 重读了：这一段是对着旧台账取的，接上去就是串台（决策 204⑥
      // 同一条纪律——await 之后比对记号，不符即丢）。
      if (gen !== this.sessionId) return false;
      const older = page.messages ?? [];
      if (older.length === 0) {
        this.hasMoreEarlier = false;
        return false;
      }
      if (this.session) {
        this.session = { ...this.session, messages: [...older, ...this.session.messages] };
      }
      // 整段读满才可能还有更上一层（后端每段最多 500 条）。
      this.hasMoreEarlier = older.length >= SESSION_PAGE_LIMIT;
      return true;
    } catch {
      // 读失败不碰现场：游标没变、位置没动，下次滚到顶重试。
      return false;
    } finally {
      this.loadingEarlier = false;
    }
  }

  /**
   * 切到另一班。
   *
   * **回话中也可以切**（决策 220②）：那把 `sending || busy` 的锁撤掉了。它只是「别让你把
   * 正在等的那句回话弄丢」的**第三层**自保——前两层是 `appendForemanEvent` 的班次守卫
   * （增量串台）与发送路径里每个 await 之后的 `sessionId` 比对（回包串台，票 01），
   * 那两层一步没动。切走之后「那一轮回话去哪了」改由班次列表里的两枚标记说清楚（决策 220③）。
   *
   * `write`：用户点的切换把班次写进地址（`pushState`——后退回到上一班是想要的，决策 217③）；
   * 从地址来的切换（后退 / 前进）不写，否则自己触发的装载会再写一次地址。
   */
  async switchTo(id: string, opts: { write?: boolean } = {}): Promise<void> {
    if (id === this.sessionId) return;
    // 先认下这件事再写地址：地址一变，页面上看地址的那个效果会拿新值来比——认下了才不重复装载。
    // 认下这一步同时就是在途回包的作废记号（票 01）：此后每个 await 的比对都以它为准。
    this.watch(id);
    if (opts.write) this.writeSessionAddress(id);
    this.resetLedger();
    await this.reload(id);
  }

  /**
   * 开一个新班次并切过去（空班是合法状态：第一句话说出来时它才得名）。
   *
   * **不带 busy 守卫**：调用方已经持有它。归档最后一个班次那条路就是这样调的
   * ——那时的 busy 必然是 true，若这里再守一次，归档完最后一个班次会静默什么都不做，
   * 页面停在一片空白上（「一个班次都没有」且没有当前班次）。
   */
  async openFreshSession(
    opts: { push?: boolean; onReset?: () => void } = {},
  ): Promise<void> {
    const created = await createForemanSession();
    this.sessionList = [created.session, ...this.sessionList];
    this.resetLedger();
    opts.onReset?.();
    // 同 `switchTo`：先把落点认下来，再写地址（用户按的那一颗 push，归档后的自动开新班不写——
    // 地址的落点由随后的 `reload` 用 replaceState 规范化）
    this.watch(created.session.id);
    if (opts.push) this.writeSessionAddress(created.session.id);
    await this.reload(created.session.id);
  }

  /**
   * 归档当前班并**自动切到最近活动的未归档班次**；一个都不剩时新开一个（决策 204）。
   *
   * 不切的话屏幕上会留着一个已经不在列表里的班次，人接着说话才发现「这一班已经归档了」
   * ——把一件必然要做的善后交给使用者做，是界面偷懒。
   * `onArchived`：归档**已成功**的那一拍（页面用它关对话框——改前逐拍一致）；
   * `onReset` 同 {@link switchTo}。
   */
  async archiveAndFall(
    id: string,
    opts: { onArchived?: () => void; onReset?: () => void } = {},
  ): Promise<void> {
    await archiveForemanSession(id);
    opts.onArchived?.();
    this.resetLedger();
    opts.onReset?.();
    const list = await getForemanSessions(undefined, this.kind, this.showArchived);
    this.sessionList = list.sessions;
    // 切去**最近活动的未归档班**（决策 204）：开关开着时列表含归档，而刚归档的那班
    // `last_active_at` 最新会排第一——照 `sessions[0]` 切就是「归档完原地不动」。
    const next = list.sessions.find((s) => !s.archived_at);
    if (next) {
      await this.reload(next.id);
    } else {
      // 一个不剩：新开一班（走不带守卫的那条，见 `openFreshSession`）
      await this.openFreshSession();
    }
  }

  /**
   * 只重读班次列表（不碰这一屏的台账）。
   *
   * 已经落地的回话（含切走之后落地的那些）更新的是 `last_active_at`——「有新动静」那枚
   * 标记的判据。哪一班在屏由 `reload` 管，本函数只管那一列元信息。
   */
  async refreshSessionList(): Promise<void> {
    try {
      const list = await getForemanSessions(undefined, this.kind, this.showArchived);
      this.sessionList = list.sessions;
      // 看过表说的是**人的账**（决策 220③）：值守账的名单是另一本，拿它 prune 会把对讲台
      // 那一侧刚记下的看过时刻抹掉（旧代码在页面里无条件 prune，是本页搬进 store 时顺手
      // 收掉的一处错——见票 01 的注记）。
      if (this.kind === 'talk') {
        const next = pruneSeen(
          this.seen,
          list.sessions.map((s) => s.id),
        );
        if (next !== this.seen) {
          this.seen = next;
          saveSeen(next);
        }
      }
    } catch {
      // 列表读不到不影响这一屏：标记晚一步出现而已，台账是权威
    }
  }

  /**
   * pending 详情的**指纹去重**拉取（决策 101 的 `allowed_actions` 只在详情里下发）。
   *
   * 自决策 354① 起随台账归 store 管：判据（指纹只含 id 与 pending 类型、集合不变不重拉、
   * 值守账只读不拉）在这里，页面只把 `board.pendingTasks` 与 `watch` 两个输入接进来。
   * 类型也进指纹是因为 `pending_updated` 会换理由而 id 不变。
   */
  syncPendingDetails(pending: TaskListItem[], watch: boolean): void {
    if (watch) return;
    const key = pending
      .map((t) => `${t.id}:${t.pending_reason?.type ?? ''}:${t.current_stage}`)
      .sort()
      .join('|');
    if (key === this.lastDetailsKey) return;
    this.lastDetailsKey = key;
    // 空集合无需拉取：直接清空上一次的读数
    if (!key) {
      this.details = {};
      return;
    }
    void this.loadDetails(pending);
  }

  /** 拉取每个 pending 任务的详情。单个任务失败不影响整页（与看板补详情同一姿态）。 */
  async loadDetails(tasks: TaskListItem[]): Promise<void> {
    const next: Record<string, { actions: AllowedAction[]; cursors: BranchCursor[] }> = {};
    await Promise.all(
      tasks.map(async (t) => {
        try {
          const detail = await getTask(t.id);
          next[t.id] = { actions: detail.allowed_actions, cursors: detail.cursors };
        } catch {
          // 单个任务失败不影响整页
        }
      }),
    );
    this.details = next;
  }

  /**
   * 读未消费待办（决策 307，票 06）。
   *
   * **不进 `session` 载荷**：那一条的契约是「这一班的台账」，而待办是**跨班次的任务侧**
   * 事件（值守轮在那里排队时，人正看着人的班次，两条读数来自两处）。
   * 读失败只是**不显示**：未接线 / 离线时它不该把「读台账」也弄红（本页的主责是对话）。
   */
  private async loadAttention(): Promise<void> {
    try {
      this.attention = await getForemanAttention();
    } catch {
      this.attention = null;
    }
  }

  /**
   * 切班 / 开新班时**重置**的台账状态（决策 204③ 的那份清单，store 侧的那一半）。
   *
   * 「属于某一班」的：这一屏读到的台账、读的加载态与错误。在飞现场由 `watch()`
   * 换班时一并倒空（`resetLive`）。页面的输入框草稿由调用方的 `onReset` 处理——
   * 它是页面私有的，store 只在**确知重置成功**的时点回调。
   */
  private resetLedger(): void {
    this.session = null;
    this.loading = true;
    this.loadError = null;
    this.loadErrorPairing = false;
    this.resetLive();
  }

  /**
   * 落点收口（决策 217①④ / 220③）：**看过表、兜底文件、地址**三处一起跟上这一班。
   *
   * 三件事各有各的理由，且都必须在这里做：
   *   - 「我看过它了」记的是**它此刻的 `last_active_at`**（不是本机的当下）——两边同一座钟，
   *     机器一慢一快才不会读出假标记；
   *   - 兜底文件写**落点**而不是「请求的那一班」：指定的班次不在了（别的设备归档了它）时
   *     落回默认，此时该记住的是默认那一班；
   *   - 地址用 `replaceState`（决策 217③：程序改地址一律 replace），否则装载时的规范化
   *     会在历史里多塞一条，后退就不再是「回到上一页」。
   *
   * **看过表只属于人的班次列表**（票 04）：两枚标记（「正在回话」/「有新动静」）挂在
   * 班次列表的行上，而值守账只有**一本**（固定的一行，没有「哪一班有新动静」可说）；
   * 它的动静在屏上有更直接的读法（在飞那一轮 + 「值守正在跑」）。基线与清理若在值守账上跑，
   * 会把人的班次从表里剪掉——回来时每一班都亮假的「有新动静」。故整段跳过；
   * 兜底文件同理不写（写进去只会把人对讲台的兜底落点冲掉）。
   */
  private rememberLanding(landed: string | null, info: ForemanSessionMeta | null): void {
    if (this.kind === 'talk') {
      let next = this.seen;
      if (!this.seenSeeded) {
        // 立基线只在**本机一条记录都没有**时做（判据在 `seedBaselineIfFirstRun`）：
        // 少了它，第一屏每一条都带「有新动静」——而它们只是刚被列出来；写成「每次装载都
        // 拿当下的列表立基线」则相反：**关机期间别处发生的动静会被记成「看过了」**，
        // 而那正是这枚标记最该说话的场合（实测：手机在别处开了新班次、说了话，回到这台
        // 电脑打开对讲台，菜单里那条不该是安静的）。
        next = seedBaselineIfFirstRun(next, this.sessionList);
        this.seenSeeded = true;
      }
      if (info) next = markSeen(next, info.id, info.last_active_at);
      next = pruneSeen(
        next,
        this.sessionList.map((s) => s.id),
      );
      if (next !== this.seen) {
        this.seen = next;
        saveSeen(next);
      }
      saveSessionId(landed);
    }
    // 落地即熄灭（决策 220③）：这一班的回话已经在台账里了，它不再是「此刻在说话」
    if (landed) this.foreign = forgetForeignActive(this.foreign, landed);
    this.writeSessionAddress(landed, { replace: true });
  }

  /**
   * 班次落点写进地址。**只在 talk 路由上写**：store 的重读可能在页面之外跑（落地哨、
   * 重连校准——决策 354① 之后这些不再经过页面），那时地址栏是别的页的，不能碰。
   */
  private writeSessionAddress(id: string | null, opts: { replace?: boolean } = {}): void {
    const name = router.route.name;
    if (name !== 'talk' && name !== 'talk-watch') return;
    writeQuery({ session: id }, opts);
  }
}

export const talk = new TalkStore();
