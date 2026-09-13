import { getTaskFile } from '../api/client';
import { parseUnifiedDiff } from '../lib/diff';
import { newlyDoneTaskIds, type TaskStatusLike } from '../lib/completion';
import type { DiffStats } from '../api/types';

/**
 * 任务完成横幅的状态机（票 08 / theme-6-pixel.md §3「完成反馈」）。
 *
 * 为什么是独立 store：横幅跨路由（看板 / 详情）常驻在 `App.svelte`，触发源有两条
 * （SSE 事件与轮询对齐 refetch），需要一个会话内唯一的去重与队列。
 *
 * 去重口径（票面第 5 条）：
 * - 只在**从非 done 迁移到 done** 时弹（`newlyDoneTaskIds`）；刷新后 store 是新的，
 *   首屏看到已是 done 的任务没有迁移 → 不重弹；
 * - `settled` 记录会话内已弹过的任务：同一次 done 即使 SSE 与轮询各看到一次也只弹一次，
 *   且同一个任务第二次回到 done（理论上不会——done 不可重试）也保持静默。
 *
 * 数据来源：diff 摘要沿用既有前端解析（`parseUnifiedDiff`），读任务目录的
 * `merge-proposal.diff`（合入前生成，done 后仍在任务目录）。**不可得时 `stats` 保持
 * `null`，只显示标题与「已合入」，绝不画 0 冒充真实值**（票面第 2 条）。
 */
export interface CompletionNotice {
  taskId: string;
  title: string;
  /** `null` = diff 摘要不可得（不画 0）。 */
  stats: DiffStats | null;
}

interface ObservedTask extends TaskStatusLike {
  title?: string;
}

class CompletionStore {
  /** 当前可见横幅；`null` = 无。 */
  notice = $state<CompletionNotice | null>(null);

  /** 上一次观测到的状态（会话内）。刷新页面即重置 → 首屏不重弹。 */
  private lastStatus = new Map<string, string>();
  /** 会话内已弹过横幅的任务（含已收下）。 */
  private settled = new Set<string>();
  /** 同时有多个任务进入 done 时的排队（罕见，但一个都不能吞）。 */
  private queued: CompletionNotice[] = [];

  /** 批量观测（看板 `loadTasks` / 详情 `load` 的每次对齐都调用）。 */
  observeAll(tasks: readonly ObservedTask[]): void {
    const newly = newlyDoneTaskIds(this.lastStatus, tasks);
    for (const t of tasks) this.lastStatus.set(t.id, t.status);
    for (const id of newly) {
      if (this.settled.has(id)) continue;
      this.settled.add(id);
      const task = tasks.find((t) => t.id === id);
      this.enqueue({ taskId: id, title: task?.title ?? id, stats: null });
    }
  }

  /** 单任务观测（SSE 终态事件即时触发，不必等轮询）。 */
  note(taskId: string, status: string, title?: string): void {
    this.observeAll([{ id: taskId, status, title }]);
  }

  /** 点「收下」关闭，并立即呈上队列中的下一个完成。 */
  dismiss(): void {
    this.notice = null;
    this.presentNext();
  }

  private enqueue(notice: CompletionNotice): void {
    if (this.notice === null) this.present(notice);
    else this.queued.push(notice);
  }

  private present(notice: CompletionNotice): void {
    this.notice = notice;
    void this.loadStats(notice.taskId);
  }

  private presentNext(): void {
    const next = this.queued.shift();
    if (next) this.present(next);
  }

  private async loadStats(taskId: string): Promise<void> {
    try {
      const raw = await getTaskFile(taskId, 'merge-proposal.diff');
      const stats = parseUnifiedDiff(raw).stats;
      // 竞态：等待期间已收下或换成别的横幅 → 丢弃这次结果。
      if (this.notice?.taskId === taskId) this.notice = { ...this.notice, stats };
    } catch {
      // 摘要不可得：保留 title + 「已合入」，不画 0（票 08 / 与指标页缺数据同姿态）。
    }
  }

  /** 测试用：回到「刚打开页面」的会话内状态。 */
  reset(): void {
    this.notice = null;
    this.queued = [];
    this.lastStatus.clear();
    this.settled.clear();
  }
}

export const completion = new CompletionStore();
