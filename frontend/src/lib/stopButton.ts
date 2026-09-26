/**
 * 停钮的阶段态（决策 294 / 票 09）：**只在这一轮在飞时出现**，按下之后进入「正在停」。
 *
 * 为什么单拎出来：这一颗钮的三个态各有一个不同的错法，而它们都不是肉眼一眼能验的——
 * ① 没在飞时还亮着（点了回一句 `cancelled: false`，人以为自己按坏了）；
 * ② 按下之后不置「正在停」，于是人在等收口的那几秒里连点（后端是幂等的，但连点给人的
 *    感觉是「没反应」）；③ 按下之后不撤这个态，下一轮一开始它就带着上一轮的「正在停」。
 * 判据因此写成一条纯函数 + 用例，而不是塞在组件的 `{#if}` 里。
 *
 * **在飞 = 本机这一趟（`sending`）或服务端此刻的读数（`turn_in_flight`，决策 260）**：
 * 前者管「我刚发出去的那一趟」，后者管「刷新/换设备之后它仍在跑」——只认前者的话，
 * 刷新页面就没法停；只认后者的话，本机发出的那一趟在服务端登记之前有一段空隙。
 *
 * **值守轮不在此列**：它压根不登记停钮通道（裁决 10：值守轮归开关），故这本账上没有
 * 这颗钮——组件在 `watchMode` 下整个输入坞都不渲染，这里不再判一次。
 */
export type StopButtonState = 'hidden' | 'ready' | 'stopping';

export interface StopButtonInput {
  /** 这一班此刻有一轮在飞吗（`sending || session.turn_in_flight`）。 */
  inFlight: boolean;
  /** 本机已经按过停、那一轮还没收口。 */
  asked: boolean;
}

export function stopButtonState({ inFlight, asked }: StopButtonInput): StopButtonState {
  if (!inFlight) return 'hidden';
  return asked ? 'stopping' : 'ready';
}

/**
 * 按钮上的字。
 *
 * `hidden` 也在签名里（返回空串）而不是靠调用方窄化类型：组件里那个 `{#if}` 是 Svelte
 * 模板里的判据，TS 在那边窄化不了 `$derived` 的值——让它返回空串，两边就不用对暗号。
 */
export function stopButtonLabel(state: StopButtonState): string {
  if (state === 'hidden') return '';
  return state === 'stopping' ? '正在停…' : '停';
}
