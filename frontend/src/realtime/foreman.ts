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
 * 只认工头自己的对话增量：这条流上理论上只有它，但判定不能省——`/foreman/stream`
 * 与任务流共用一条总线，漏判就会把别的任务的增量拼进值班长的话里。
 */
export function appendForemanDelta(
  state: ForemanStreamState,
  event: SseEvent,
): ForemanStreamState {
  if (event.type !== 'conversation_delta' || event.agent_type !== FOREMAN_AGENT_TYPE) return state;
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
