import { createTask, getTask, listProjects, listTasks } from '../api/client';
import type { AllowedAction, BranchCursor, CreateTaskPayload, Project, TaskListItem } from '../api/types';
import { submitAllowedAction } from '../lib/actionSubmit';
import { notificationClassForEvent } from '../lib/notificationPolicy';
import { StreamManager } from '../realtime/connection';
import { emptyBoardState, reduceBoard } from '../realtime/reduce';
import { notifications } from './notifications.svelte';

/** 顶栏状态过滤（design §4）。 */
export type StatusFilter = 'all' | 'running' | 'pending' | 'waiting' | 'queued' | 'done' | 'ended';

export const FILTER_LABELS: Record<StatusFilter, string> = {
  all: '全部',
  running: '执行中',
  pending: '待处理',
  waiting: '等依赖',
  queued: '排队',
  done: '已完成',
  ended: '已结束（失败·取消）',
};

const STORAGE_KEY = 'agentpipeline.project_id';

class BoardStore {
  tasks = $state<TaskListItem[]>([]);
  projects = $state<Project[]>([]);
  projectId = $state<string | null>(null);
  filter = $state<StatusFilter>('all');
  includeArchived = $state(false);
  loading = $state(false);
  error = $state<string | null>(null);
  pendingOpen = $state(false);
  connectionState = $state<'idle' | 'open' | 'error'>('idle');
  /** 看板卡片上的 pending 动作集（`GET /tasks` 不含 allowed_actions → 为 pending 任务补详情）。 */
  pendingActions = $state<Record<string, AllowedAction[]>>({});
  pendingCursors = $state<Record<string, BranchCursor[]>>({});
  actionBusy = $state<string | null>(null);
  actionError = $state<string | null>(null);

  private streamManager: StreamManager;
  private reloadTimer: ReturnType<typeof setTimeout> | null = null;
  private tickTimer: ReturnType<typeof setInterval> | null = null;

  constructor() {
    this.streamManager = new StreamManager({
      onEvent: (_taskId, event) => this.handleEvent(event),
      onStatus: (_taskId, status) => {
        this.connectionState = status === 'open' ? 'open' : status === 'error' ? 'error' : this.connectionState;
      },
      onRecalibrate: () => void this.loadTasks(),
    });
  }

  /** 看板初始装载：全量（含各状态，供顶栏计数），前端按桶过滤。 */
  async loadTasks(): Promise<void> {
    this.loading = true;
    this.error = null;
    try {
      const all = await listTasks({
        project_id: this.projectId,
        include_archived: this.includeArchived,
      });
      this.tasks = all;
      this.streamManager.sync(this.activeTaskIds);
      void this.loadPendingActions();
    } catch (err) {
      this.error = (err as Error).message;
    } finally {
      this.loading = false;
    }
  }

  /** `GET /tasks` 不下发 allowed_actions：为 pending 任务补一次详情（数量受待办约束）。 */
  private async loadPendingActions(): Promise<void> {
    const pending = this.tasks.filter((t) => t.status === 'pending');
    const actions: Record<string, AllowedAction[]> = {};
    const cursors: Record<string, BranchCursor[]> = {};
    await Promise.all(
      pending.map(async (t) => {
        try {
          const detail = await getTask(t.id);
          actions[t.id] = detail.allowed_actions;
          cursors[t.id] = detail.cursors;
        } catch {
          // 单个详情失败不影响看板
        }
      }),
    );
    this.pendingActions = actions;
    this.pendingCursors = cursors;
  }

  /** 看板卡片上的动作提交（resume / 配对端点，决策 101）。 */
  async handleTaskAction(
    taskId: string,
    action: AllowedAction,
    opts: { cursorId?: string; input?: string } = {},
  ): Promise<void> {
    const cursors = this.pendingCursors[taskId] ?? [];
    const cursor = action.cursor_id
      ? cursors.find((c) => c.cursor_id === action.cursor_id)
      : cursors[0];
    const key = `${taskId}:${action.action}`;
    this.actionBusy = key;
    this.actionError = null;
    try {
      await submitAllowedAction(taskId, action, {
        cursorId: opts.cursorId,
        input: opts.input,
        pendingType: cursor?.pending_reason?.type,
      });
      await this.loadTasks();
    } catch (err) {
      this.actionError = (err as Error).message;
      throw err;
    } finally {
      this.actionBusy = null;
    }
  }

  /** 用 status 过滤刷新 pending（顶栏下拉用，决策 92）。 */
  async loadPending(): Promise<TaskListItem[]> {
    return listTasks({ project_id: this.projectId, status: 'pending', include_archived: false });
  }

  async loadProjects(): Promise<void> {
    try {
      this.projects = await listProjects();
      const saved = typeof localStorage !== 'undefined' ? localStorage.getItem(STORAGE_KEY) : null;
      const valid = saved && this.projects.some((p) => p.id === saved) ? saved : null;
      this.projectId = valid ?? this.projects[0]?.id ?? null;
    } catch (err) {
      this.error = (err as Error).message;
    }
  }

  async init(): Promise<void> {
    await this.loadProjects();
    await this.loadTasks();
    this.startAlignmentTick();
  }

  selectProject(id: string | null): void {
    this.projectId = id;
    if (typeof localStorage !== 'undefined' && id) {
      localStorage.setItem(STORAGE_KEY, id);
    }
    void this.loadTasks();
  }

  setFilter(filter: StatusFilter): void {
    this.filter = filter;
  }

  async createTask(payload: CreateTaskPayload): Promise<TaskListItem | null> {
    this.error = null;
    try {
      await createTask(payload);
      await this.loadTasks();
      return this.tasks[0] ?? null;
    } catch (err) {
      this.error = (err as Error).message;
      throw err;
    }
  }

  /** SSE 事件 → board 归约（§9.1 左列）。 */
  handleEvent(event: Parameters<typeof reduceBoard>[1]): void {
    this.tasks = reduceBoard(emptyBoardState(this.tasks), event).tasks;
    const cls = notificationClassForEvent(event);
    if (cls) {
      const task = this.tasks.find((t) => t.id === event.task_id);
      notifications.notify(cls, {
        title: `${task?.title ?? event.task_id} · ${notifyTitle(cls)}`,
        message: event.type === 'pending' ? event.reason.message : undefined,
        taskId: event.task_id,
      });
    }
    // 列归属 / 动作集可能变化 → 短延迟 refetch（兜底，权威在后端）
    if (event.type === 'pending' || event.type === 'pending_updated' || event.type.startsWith('task_')) {
      this.scheduleReload();
    }
  }

  private scheduleReload(): void {
    if (this.reloadTimer) return;
    this.reloadTimer = setTimeout(() => {
      this.reloadTimer = null;
      void this.loadTasks();
    }, 400);
  }

  /** 10s 对齐 tick 的轻量 refetch（§9.1），兜底 waiting / queued 迁移。 */
  private startAlignmentTick(): void {
    if (this.tickTimer) return;
    this.tickTimer = setInterval(() => void this.loadTasks(), 10_000);
  }

  dispose(): void {
    this.streamManager.dispose();
    if (this.tickTimer) clearInterval(this.tickTimer);
    this.tickTimer = null;
  }

  get activeTaskIds(): string[] {
    return this.tasks
      .filter((t) => t.status === 'running' || t.status === 'pending')
      .map((t) => t.id);
  }

  get visibleTasks(): TaskListItem[] {
    const f = this.filter;
    if (f === 'all') return this.tasks;
    if (f === 'ended') return this.tasks.filter((t) => t.status === 'failed' || t.status === 'cancelled');
    return this.tasks.filter((t) => t.status === f);
  }

  get pendingTasks(): TaskListItem[] {
    return this.tasks.filter((t) => t.status === 'pending');
  }

  get pendingCount(): number {
    return this.pendingTasks.length;
  }

  countFor(filter: StatusFilter): number {
    if (filter === 'all') return this.tasks.length;
    if (filter === 'ended') {
      return this.tasks.filter((t) => t.status === 'failed' || t.status === 'cancelled').length;
    }
    return this.tasks.filter((t) => t.status === filter).length;
  }

  taskById(id: string): TaskListItem | undefined {
    return this.tasks.find((t) => t.id === id);
  }

  togglePendingDropdown(): void {
    this.pendingOpen = !this.pendingOpen;
    if (this.pendingOpen) void this.refreshPendingInto();
  }

  private async refreshPendingInto(): Promise<void> {
    try {
      const pending = await this.loadPending();
      const byId = new Map(this.tasks.map((t) => [t.id, t]));
      for (const p of pending) byId.set(p.id, p);
      this.tasks = [...byId.values()];
    } catch {
      // 下拉刷新失败不阻塞（本地缓存仍可用）
    }
  }
}

function notifyTitle(cls: 'pending' | 'done' | 'failed' | 'cancelled'): string {
  switch (cls) {
    case 'pending':
      return '等待你处理';
    case 'done':
      return '已完成';
    case 'failed':
      return '已失败';
    case 'cancelled':
      return '已取消';
  }
}

export const board = new BoardStore();
