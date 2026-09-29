/**
 * 长列表切片原语（spec list-windowing 票 01）。
 *
 * 全站确认零分页零虚拟化，后端分页等数据量证明必要再做；统一做法是**前端切片**：
 * 数据一次全量到前端（本机单用户数据量有上界），界面只画一个窗口。
 *
 * - 缺省 50 条/页，调用点可覆盖；
 * - 「加载更多」用**按钮**（`components/ui/MoreRow.svelte`），不做滚动自动加载——
 *   滚动加载与「贴底跟随」的语义会打架；
 * - 两种锚定：`'tail'`（命令 / 会话，最新最有用，省略的是**前面**）与
 *   `'head'`（看板列，先来的先看，省略的是**后面**）。
 *
 * 切片保持原序：尾部锚定不是「倒序取再翻回来」，`visible` 永远按输入顺序排列。
 */

/** 缺省页大小（spec 决议：个别页面可覆盖）。 */
export const DEFAULT_PAGE = 50;

export type WindowAnchor = 'head' | 'tail';

export interface WindowSlice<T> {
  /** 窗口内的条目，**保持输入顺序**。 */
  visible: T[];
  /** 尾部锚定时被省略的**头部**条数；头部锚定恒为 0。 */
  omittedBefore: number;
  /** 头部锚定时被省略的**尾部**条数；尾部锚定恒为 0。 */
  omittedAfter: number;
  /** 总条数（省略提示文案的分子分母都从这来）。 */
  total: number;
}

export function windowSlice<T>(items: readonly T[], shown: number, anchor: WindowAnchor): WindowSlice<T> {
  const total = items.length;
  const take = Math.max(0, Math.min(shown, total));
  if (anchor === 'tail') {
    const start = total - take;
    return { visible: items.slice(start), omittedBefore: start, omittedAfter: 0, total };
  }
  return { visible: items.slice(0, take), omittedBefore: 0, omittedAfter: total - take, total };
}

/** 「加载更多」的游标推进：加一页、钳在总量上（多点几下不越界）。 */
export function nextPage(shown: number, total: number, step: number = DEFAULT_PAGE): number {
  return Math.min(shown + step, total);
}
