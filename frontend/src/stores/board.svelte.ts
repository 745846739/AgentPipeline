import { createTask, getTask, listProjects, listTasks } from '../api/client';
import type {
  AllowedAction,
  BranchCursor,
  CreateTaskPayload,
  Project,
  Task,
  TaskListItem,
} from '../api/types';
import { submitAllowedAction } from '../lib/actionSubmit';
import { actionKey } from '../lib/actions';
import { notificationClassForEvent } from '../lib/notificationPolicy';
import { readQuery, router, writeQuery } from '../router.svelte';
import { StreamManager } from '../realtime/connection';
import { emptyBoardState, reduceBoard } from '../realtime/reduce';
import { completion } from './completion.svelte';
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

/**
 * 看板过滤的本地兜底键（决策 217①⑤）。地址是权威：`#/?filter=pending` 在就照地址，
 * 不在才用这里记的值——「我一直在看 pending」不该被「点进任务再回来」那一步重置。
 */
const FILTER_KEY = 'agentpipeline.board_filter';
const FILTER_VALUES = new Set<string>(Object.keys(FILTER_LABELS));

/** 枚举外的值不是过滤器（决策 217④：非法回落缺省，并顺手删键）。 */
export function isStatusFilter(v: string | undefined): v is StatusFilter {
  return v !== undefined && FILTER_VALUES.has(v);
}

/** 读本地兜底；脏值就地删掉。存储不可用时回 `null`（没有兜底就是没有）。 */
function storedFilter(): StatusFilter | null {
  let saved: string | null;
  try {
    saved = localStorage.getItem(FILTER_KEY);
  } catch {
    return null;
  }
  if (saved === null) return null;
  if (isStatusFilter(saved)) return saved;
  try {
    localStorage.removeItem(FILTER_KEY);
  } catch {
    /* 同上 */
  }
  return null;
}

/** 写本地兜底（`all` 即缺省，不留键）。 */
function saveFilter(filter: StatusFilter): void {
  try {
    if (filter === 'all') localStorage.removeItem(FILTER_KEY);
    else localStorage.setItem(FILTER_KEY, filter);
  } catch {
    /* 存储不可用：丢一条偏好好过抛错 */
  }
}

/** 地址 → 本地 → 缺省（决策 217④）。脏地址在这里就地抹掉，不留到渲染时再猜一次。 */
function initialFilter(): StatusFilter {
  const raw = readQuery().filter;
  if (isStatusFilter(raw)) return raw;
  if (raw !== undefined) writeQuery({ filter: null }, { replace: true });
  return storedFilter() ?? 'all';
}

class BoardStore {
  tasks = $state<TaskListItem[]>([]);
  projects = $state<Project[]>([]);
  projectId = $state<string | null>(null);
  /** 过滤值：地址 → 本地兜底 → 缺省（决策 217④，见 `initialFilter()`）。 */
  filter = $state<StatusFilter>(initialFilter());
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
      // 完成横幅的触发源 A：轮询对齐看到「非 done → done」迁移（票 08；
      // 首屏装载没有迁移，刷新后不重弹）。
      completion.observeAll(all);
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
    opts: { cursorId?: string; input?: string; push?: boolean } = {},
  ): Promise<void> {
    const cursors = this.pendingCursors[taskId] ?? [];
    const cursor = action.cursor_id
      ? cursors.find((c) => c.cursor_id === action.cursor_id)
      : cursors[0];
    // 「哪个动作在忙」与规范形**同一把尺子**（票 05）：此前这里是两段
    // `taskId:action` 拼法，与 taskDetail / PendingActions 的四段 `actionKey`
    // 并存——同一个概念两种拼法，且旧拼法长在 §12.3 映射表没盖到的地方。
    // 游标是 ULID、全局唯一，四段式不带 taskId 也跨任务分得开；`opts.cursorId` 与消费
    // 者比较时用的是同一个 `fallbackCursorId`（PendingActions 的 emit 与 busy() 同源）。
    const key = actionKey(action, opts.cursorId);
    this.actionBusy = key;
    this.actionError = null;
    try {
      await submitAllowedAction(taskId, action, {
        cursorId: opts.cursorId,
        input: opts.input,
        push: opts.push,
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
    // 用户切过滤 = `pushState`（决策 217③）；缺省 `all` 不写进地址（217②）。
    // 看板以外的路由不写——`?filter=` 只属于 `#/`，本地兜底照写不误（217④「跨页面的工作
    // 语境」：地址丢了参数时靠它接住）。
    if (router.route.name === 'board') {
      writeQuery({ filter: filter === 'all' ? null : filter });
    }
    saveFilter(filter);
  }

  /**
   * 后退 / 前进把 `?filter=` 换了：照地址恢复（决策 217④），**不再回写地址**——回写会再生成
   * 一条历史，后退就退不动了。地址里没这一项时调用方应当直接 return（那时是本地兜底在管，
   * store 初始化时已经读过），不拿缺省去覆盖它。
   */
  syncFilterFromQuery(raw: string): void {
    if (isStatusFilter(raw)) {
      if (raw !== this.filter) {
        this.filter = raw;
        saveFilter(raw);
      }
      return;
    }
    // 脏地址：回落缺省并删键（决策 217④）。
    writeQuery({ filter: null }, { replace: true });
    try {
      localStorage.removeItem(FILTER_KEY);
    } catch {
      /* 存储不可用 */
    }
    this.filter = 'all';
  }

  /**
   * 新建任务，返回**服务端建的那一个**（票 10 / R2-12）。
   *
   * 此前这里丢掉 `POST /tasks` 的返回值、改从 `loadTasks()` 的结果里取 `tasks[0]`——
   * 而 `loadTasks` 是按 `board.projectId` 过滤的，对话框的项目却是它自己的局部状态：
   * 两个 id 可以不同，于是「在 B 里建任务」跳到「A 的某个任务」，A 一个任务都没有时
   * 干脆哪儿也不去。现在以响应里的那个 id 为准，与看板当前在看哪个项目无关。
   */
  async createTask(payload: CreateTaskPayload): Promise<Task | null> {
    this.error = null;
    try {
      const { task } = await createTask(payload);
      await this.loadTasks();
      return task ?? null;
    } catch (err) {
      this.error = (err as Error).message;
      throw err;
    }
  }

  /** SSE 事件 → board 归约（§9.1 左列）。 */
  handleEvent(event: Parameters<typeof reduceBoard>[1]): void {
    this.tasks = reduceBoard(emptyBoardState(this.tasks), event).tasks;
    // 完成横幅的触发源 B：SSE 终态事件即时弹出（不等 400ms 轮询兜底）。
    if (event.type === 'task_done') {
      const task = this.tasks.find((t) => t.id === event.task_id);
      completion.note(event.task_id, 'done', task?.title);
    }
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
