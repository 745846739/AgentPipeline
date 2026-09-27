import { getForemanSession } from '../api/client';
import type { ForemanSession, SseEvent } from '../api/types';
import { TaskStream, type StreamStatus } from '../realtime/connection';
import {
  appendForemanEvent,
  emptyForemanStream,
  emptyForeignActive,
  failForemanStream,
  FOREMAN_LOST_TURN_SUFFIX,
  maxLedgerId,
  noteForeignDelta,
  resolveFollowOutcome,
  type ForemanStreamState,
  type ForeignActive,
} from '../realtime/foreman';

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
 * 判据是「这一格描述的是谁」：台账（`session`）与班次列表是**读出来的**，
 * 每次装载重读一遍就对了；而「此刻屏幕外那一轮说到哪了」读不回来——它只能**攒**，
 * 于是它属于生命周期比页面长的那个东西。
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
   * 「别的班次正在回话」（决策 220③）。住在 store 里而不是页面里：它说的是**此刻**，
   * 而此刻不因为你去看了一眼看板就不作数（断流/落地两条收口仍在，见 `onStatus` 与
   * `pruneForeign`）。
   */
  foreign = $state<ForeignActive>(emptyForeignActive());

  /** 传输层状态（输入坞那一行断线告知读它）。 */
  status = $state<StreamStatus>('idle');

  /**
   * **台账代次**（决策 275）：+1 的意思是「此刻这一屏读的那份台账可能已经变了，
   * 该重读一次」。
   *
   * 为什么需要它：一轮的收尾**可能发生在页面之外**——页面切走之后那一趟 POST 才回来、
   * 或别处看到那一轮落了地；而那时**在屏的那一页**读的仍是旧台账（它自己的那次重读写在
   * 一个已经销毁的组件里）。代次是 store 喊给「此刻在屏的那一页」的那一声：它只加一，
   * 谁在读谁就去重读（页面不在屏上就没人听，回来那一次的装载本来就会读到新的）。
   */
  ledgerEpoch = $state(0);

  private conn: TaskStream | null = null;

  /**
   * 「接回来之后补一次全量」的入口（决策 76 / 票 03）：由页面挂载时登记、卸载时撤。
   *
   * **注册制而不是 store 自己 reload**：全量是**台账**（`session` / 班次列表），那是页面的
   * 东西；页面不在屏上时不发这一跳——回来那一次装载本来就会重读。
   *
   * 它还兼着**落地交棒**（决策 275）：一轮落了地要重读一次台账，那一行才会进这一屏。
   * 之所以必须由 store 来喊：一轮的收尾**可能发生在页面之外**（页面切走之后那一趟 POST
   * 才回来），而那时**只有这一屏在读的那份台账**需要更新——在别处喊，都写进了一个已经
   * 销毁的组件里，页面上于是整轮不见（2026-09-25 实测：切去看板再回来，落地那一刻
   * 时间线空了一格，而那一轮其实答完了）。
   */
  private recalibrate: (() => void) | null = null;

  /** 落地哨（决策 260）的计时器：跟着一轮走，收场即停。 */
  private sentinel: ReturnType<typeof setInterval> | null = null;

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
        onRecalibrate: () => this.recalibrate?.(),
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
    this.recalibrate = null;
    this.conn?.stop();
    this.conn = null;
    this.status = 'closed';
  }

  private onVisible = (): void => {
    if (typeof document === 'undefined' || document.visibilityState !== 'visible') return;
    this.conn?.reconnectNow();
  };

  bindRecalibrate(fn: (() => void) | null): void {
    this.recalibrate = fn;
  }

  /**
   * 认下「我在看哪一班」。
   *
   * 换班就倒空在飞现场（决策 204③ / 220⑤，见类头的边界）——**但「从没有班次到有班次」
   * 不算换班**：那正是「这台机器上一个班次都没有，本机发出的第一句话自己开了一班」那条路
   * （`send()` 里 `sid` 为空那一支），此刻在飞现场（乐观轮 + 刚开的流）说的就是**这一班**，
   * 清掉等于把人刚发出去的那句话从屏上抹掉。
   */
  watch(id: string | null): void {
    if (id === this.sessionId) return;
    const switchingAway = this.sessionId !== null;
    this.sessionId = id;
    if (switchingAway) this.resetLive();
  }

  /**
   * 一轮收场了（本机那一趟 POST 回来了 / 断在那里了）：现场退场 + 提醒台账重读。
   *
   * 三条收尾路径（成功 / 换班后作废 / 失败）都走它——**别各自写字段**：漏掉代次那一下的
   * 症状正是本轮输出整段不见（见 {@link ledgerEpoch}）。
   */
  settleTurn(): void {
    this.stream = emptyForemanStream();
    this.pendingText = null;
    this.pairingNeeded = false;
    this.ledgerEpoch += 1;
  }

  /**
   * 「这一份台账可能已经变了」——与 {@link settleTurn} 分开的那一格：**失败**那一支要留着
   * 本地那条失败轮（它是唯一信号），只提醒在屏的那一页重读一次。
   */
  markLedgerStale(): void {
    this.ledgerEpoch += 1;
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
   * 叠在台账那一行上）。收场之后由 {@link recalibrate} 请**当前在屏的那一页**重读台账；
   * 页面不在屏上时那一跳免了——回来那一次的装载本来就会读到落地后的那一行。
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
      // 收口（settled / lost 两支都在里面），它会顺手请在屏的那一页重读台账
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
   * 1. **本机在发**（`sending`）：跟着的是本机那一趟，`send()` 的收尾负责放手——
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
      this.ledgerEpoch += 1;
      this.recalibrate?.();
      return;
    }
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
      this.startSentinel();
    }
    // 请**在屏的那一页**重读一次台账：落地那一刻那一行才会进这一屏。页面不在就不喊
    // （回来那一次的装载本来就会读到它）。
    this.ledgerEpoch += 1;
    this.recalibrate?.();
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
    this.startSentinel();
  }
}

export const talk = new TalkStore();
