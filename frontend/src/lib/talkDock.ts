/**
 * 折行档输入坞的自长判据（决策 282 ③，修订 218④ 隐含的「rows=2 固定」）：
 * **1 行起步、随内容自长、封顶 {@link TALK_DOCK_CAP_ROWS} 行，到顶后框内滚**。
 *
 * 判据与量测分两层：
 * - [`dockRows`](#dockrows) 是**纯判据**（文本 → 目标行数）：硬换行决定档位，clamp 到
 *   `[1, 封顶]`。与 `enterToSend` 同款小而可测，单测五例钉住：空 / 单行 / 多行 / 封顶 /
 *   换行归零。
 * - [`growTextarea`](#growtextarea) 是**接线层的量测校正**：`rows` 属性只认硬换行，
 *   **软换行**（无换行符的长句在框内折行）要靠 `scrollHeight` 实测把档位补上去（仍受
 *   封顶）；**空值不量测**——占位语在 480 宽上会折行，不能让它撑高框。软换行那一半
 *   jsdom 量不出来（`scrollHeight` 恒 0），由 e2e 覆盖。
 *
 * **自长只落折行档（≤899）**；桌面（≥900）`rows=2` 逐像素不变——两侧的差异是刻意的
 * （决策 282 ③）。组件里只有一个 `$effect` 接线：折行档调 [`growTextarea`](#growtextarea)，
 * 桌面档把 `rows` 写回 2。
 */

/** 自长的封顶行数（决策 282 ③：候选 5–6 取 6——390 宽 16px 像素字约 17 字/行，六行约容一条百字长句）。 */
export const TALK_DOCK_CAP_ROWS = 6;

/** 文本 → 目标行数：1 行起步、随硬换行自长、封顶（决策 282 ③）。 */
export function dockRows(text: string): number {
  return Math.max(1, Math.min(TALK_DOCK_CAP_ROWS, text.split('\n').length));
}

/**
 * 把 textarea 的档位拨到当前内容需要的那一行（接线层；软换行校正见模块头）。
 *
 * 容差 2px 才补档：行高 25.6px 是小数，`scrollHeight` / `clientHeight` 各自取整的
 * 假差最多 1~2px——为它多拨一行（25.6px）不划算；真缺一行时差的是 25.6px，远过得去。
 */
export function growTextarea(el: HTMLTextAreaElement): void {
  el.rows = dockRows(el.value);
  if (!el.value) return;
  while (el.rows < TALK_DOCK_CAP_ROWS && el.scrollHeight - el.clientHeight > 2) el.rows += 1;
}
