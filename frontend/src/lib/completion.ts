import type { TaskStatus } from '../api/types';

/**
 * 任务完成横幅的纯判定（票 08）。
 *
 * 「何时弹横幅」与「何时不该弹」是可单测的纯逻辑，不放在 store 里：
 * - 只在任务**从非 done 迁移到 done** 时返回（同一次完成只弹一次）；
 * - 首屏装载（prev 为空）不返回——页面刷新后不重弹（没有迁移就没有横幅）；
 * - failed / cancelled 永远不返回（失败/取消的既有终态提示保留）。
 *
 * store 还持有更严的一次性去重集合（SSE 与轮询两条路都可能看到同一次 done），
 * 这里只回答「这次状态对齐里出现了哪些新完成」。
 */
export interface TaskStatusLike {
  id: string;
  status: TaskStatus | string;
}

export function newlyDoneTaskIds(
  previous: ReadonlyMap<string, string>,
  taskList: readonly TaskStatusLike[],
): string[] {
  const out: string[] = [];
  for (const task of taskList) {
    const prev = previous.get(task.id);
    if (prev !== undefined && prev !== 'done' && task.status === 'done') out.push(task.id);
  }
  return out;
}
