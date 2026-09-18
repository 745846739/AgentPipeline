import type { SseEvent } from '../api/types';

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
