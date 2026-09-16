/**
 * 状态区急停轮的展开规则（决策 183 / 192）。判据全在这里，组件里那段 `{#if}` 只有渲染——
 * 于是「一张不折叠 / 两张以上一张都不展开 / 窄屏连单张也折」这些规矩可以被 L1 逐条钉住。
 *
 * **为什么需要折叠**：一张急停轮要内联后端下发的恢复动作，最高的形状
 * （`info_insufficient`：带补充输入的 resume + 旁路动作，见 `crates/core/src/actions.rs`）
 * 约 **330px**（窄屏实测 376px）；而状态区上限桌面 `46vh`，13″ 笔记本上可用只有约 **344px**——
 * **展开一张就已经把整个状态区占满**，第二张必然滚出第一屏，正是 `theme-6-pixel.md`
 * §3.3 明写禁止的出错（「一个两小时前挂起的急停滚出视野，是本页最不能出的错」）。
 *
 * 故两张及以上时**一张都不展开**：每张收成一行约 36px 的摘要条，几张都留在第一屏。
 * 默认态是「都看得到」，展开态才是「看这一张」——展开需要人点，因而是主动的，不是静默滚出。
 *
 * **窄屏（`forceFold`）连单张也折**（决策 192）。手机上的版面反过来：整页随手指滚，
 * 状态区**钉在顶栏下沿**——急停永不滚出视野这件事因此由「钉住」完成，而不是由「区内滚动」
 * 完成。钉住的东西必须便宜：一张展开的急停轮 376px 钉在 900px 的屏幕上就是钉死了整块屏幕
 * （桌面那份 46vh 区内滚动的机制在窄屏也已撤掉，见 §5）。折成一行摘要条（窄屏实测 51px）
 * 是唯一两者兼得的形态——**默认一行，动作一个不少，展开靠人点**。
 */

import type { AllowedAction } from '../api/types';

/**
 * 折叠判据：两张以上折（决策 183）；`forceFold`（窄屏，决策 192）时**一张也折**。
 *
 * 没有急停时无所谓折叠（`ids` 空 → `false`）：折叠的前提是有东西可折。
 *
 * 一张、宽屏时状态区就是折叠前的样子：不出现任何折叠 UI，单急停的既有体验（含 e2e ⑩ 那条
 * 「急停轮里后端下发的动作可下发」）逐像素不变。
 */
export function isFoldable(ids: readonly string[], forceFold = false): boolean {
  if (ids.length === 0) return false;
  return forceFold || ids.length > 1;
}

/**
 * 默认展开项：**宽屏下只有一张时展开它**（单急停与折叠前一致）；两张以上一张都不展开；
 * 窄屏（`forceFold`）**恒不展开**——包括只有一张时（决策 192，理由见模块头部）。
 *
 * 不是「展开最新那张」——展开一张就占满了 13″ 上的状态区，那样第二张照样掉出第一屏，
 * 折叠也就白做了（见模块头部的算术）。
 */
export function defaultOpenStop(ids: readonly string[], forceFold = false): string | null {
  if (forceFold) return null;
  return ids.length === 1 ? (ids[0] ?? null) : null;
}

/**
 * 展开项收敛。`chosen` 是三态：
 *
 * - `undefined`：还没选过 → 跟随 {@link defaultOpenStop}（宽屏一张展开它自己 / 其余全收起）；
 * - `null`：人**显式收起** → 保持收起，不自动弹回（自动弹回会让人按不动这个钮）；
 * - 具体 id：仍在集合里就保持；已被处理掉（resume 成功后不再 pending）则回落到默认——
 *   否则展开项悬空，那条摘要条会「点开了却没有内容」。
 */
export function resolveOpenStop(
  ids: readonly string[],
  chosen: string | null | undefined,
  forceFold = false,
): string | null {
  if (chosen === null) return null;
  if (chosen !== undefined && ids.includes(chosen)) return chosen;
  return defaultOpenStop(ids, forceFold);
}

/**
 * 某一张是否展开——折叠与展开项的**合成判据**，组件里那个 `{#if}` 只调它。
 *
 * 不折叠时（宽屏单张）**恒展开**：那种形态下没有「收起」钮，也就没有「是否展开」的问题，
 * 组件也不给那个钮，所以这里短路是对的——把它显式写在模块里、并用 L1 钉住，胜过留在模板里。
 */
export function isStopOpen(foldable: boolean, openStop: string | null, id: string): boolean {
  return !foldable || openStop === id;
}

/**
 * 一处翻转同时服务两种钮：「展开恢复动作」与「收起」。
 *
 * 摘要条点的结果必然是换成它（`current !== id`），展开行点的结果必然是收起它
 * （`current === id` → `null`，即上面那个「显式收起」态）。**同时只会有一张展开**
 * 就是这条函数的全部机制：换一张等于收起前一张。
 */
export function toggleOpenStop(current: string | null, id: string): string | null {
  return current === id ? null : id;
}

/**
 * 摘要条上的动作个数：**详情没到就不给数字**（`null`）。
 *
 * 写「0 个动作」是把「还没读到」说成「没有」——与空看板那条纪律（决策 182①：装载中
 * 不算空）是同一个错。详情到了才报数，因为 `allowed_actions` 只在详情里下发（决策 101），
 * 摘要条上的这个数字正是「动作集仍在后端手里」的可见证据。
 */
export function stopActionCount(detail: { actions: AllowedAction[] } | undefined): number | null {
  return detail ? detail.actions.length : null;
}
