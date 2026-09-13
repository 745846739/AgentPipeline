import {
  ApiError,
  getCommandOutput,
  getCommands,
  getConversation,
  getConversations,
  getFlow,
  getTask,
  getTaskFile,
  modelOverrideTask,
  reviewTask,
  splitTask,
  type SplitTaskSpec,
} from '../api/client';
import { submitAllowedAction } from '../lib/actionSubmit';
import type {
  AllowedAction,
  BranchCursor,
  NodeCommand,
  NodeConversation,
} from '../api/types';
import { parseUnifiedDiff, type ParsedDiff } from '../lib/diff';
import { completion } from './completion.svelte';
import { notifications } from './notifications.svelte';
import { notificationClassForEvent } from '../lib/notificationPolicy';
import { StreamManager } from '../realtime/connection';
import { emptyTaskDetailState, reduceTaskDetail, type TaskDetailState } from '../realtime/reduce';

export interface RunActionOptions {
  /** 显式指定所属游标（决策 91：从所属分支药丸取 cursor_id）。 */
  cursorId?: string;
  /** `requires_input` 的输入；review reject 时作为 comments。 */
  input?: string;
}

export interface LoadedFile {
  path: string;
  content: string | null;
  error: string | null;
  status: number | null;
}

class TaskDetailStore {
  id = $state<string | null>(null);
  state = $state<TaskDetailState>(emptyTaskDetailState());
  loading = $state(false);
  error = $state<string | null>(null);

  conversationsFull = $state<Record<number, NodeConversation>>({});
  conversationsLoading = $state(false);

  commandOutputFull = $state<Record<number, string>>({});
  commandOutputError = $state<Record<number, string>>({});

  files = $state<Record<string, LoadedFile>>({});

  diff = $state<ParsedDiff | null>(null);
  diffRaw = $state<string | null>(null);
  diffError = $state<string | null>(null);
  /** merge 审批但 diff 缺失 → 基准已前移 / 尚未生成（ticket 21 提示）。 */
  diffStale = $state(false);

  busyKey = $state<string | null>(null);
  actionError = $state<string | null>(null);

  private streamManager: StreamManager;
  private refetchTimer: ReturnType<typeof setTimeout> | null = null;
  private busyTimer: ReturnType<typeof setTimeout> | null = null;

  constructor() {
    this.streamManager = new StreamManager({
      onEvent: (_taskId, event) => this.handleEvent(event),
      onRecalibrate: () => void this.load(this.id ?? undefined, true),
    });
  }

  async load(id?: string, silent = false): Promise<void> {
    const taskId = id ?? this.id;
    if (!taskId) return;
    this.id = taskId;
    if (!silent) this.loading = true;
    this.error = null;
    try {
      const detail = await getTask(taskId);
      // 保留流式增量（重载会话前不丢当前 run 的实时文本）
      this.state = {
        ...this.state,
        task: detail.task,
        cursors: detail.cursors,
        allowedActions: detail.allowed_actions,
        pendingReason: detail.task.pending_reason,
        refetchRequested: false,
      };
      // 完成横幅触发源：详情页 SSE 之外的对齐 refetch（票 08）。
      // 首次装载即已是 done → 无迁移，不弹（刷新不重弹）。
      completion.observeAll([{ id: taskId, status: detail.task.status, title: detail.task.title }]);
      const [flow, conversations, commands] = await Promise.all([
        getFlow(taskId),
        getConversations(taskId),
        getCommands(taskId),
      ]);
      this.state = {
        ...this.state,
        transitions: flow.transitions,
        conversations,
        commands,
      };
      this.streamManager.sync([taskId]);
    } catch (err) {
      this.error = (err as Error).message;
    } finally {
      if (!silent) this.loading = false;
    }
  }

  dispose(): void {
    this.streamManager.stopAll();
    if (this.refetchTimer) clearTimeout(this.refetchTimer);
    if (this.busyTimer) clearTimeout(this.busyTimer);
  }

  handleEvent(event: Parameters<typeof reduceTaskDetail>[1]): void {
    const previousPending = this.state.pendingReason?.type;
    this.state = reduceTaskDetail(this.state, event);
    // 完成横幅触发源：详情页 SSE 终态事件（不等 refetch）。failed / cancelled 不弹。
    if (event.type === 'task_done') {
      completion.note(event.task_id, 'done', this.state.task?.title);
    }
    // 长耗时按钮：SSE 回执到达即复位（§12.11）
    this.clearBusy();

    const cls = notificationClassForEvent(event);
    if (cls) {
      const task = this.state.task;
      notifications.notify(cls, {
        title: `${task?.title ?? event.task_id} · ${cls}`,
        message: event.type === 'pending' ? event.reason.message : undefined,
        taskId: event.task_id,
      });
    }

    // 动作集 / 列归属 / pending 状态变化 → refetch 权威数据（前端不自行计算动作集）
    if (this.state.refetchRequested) this.scheduleRefetch();
    // pending 分支变化时刷新 dossier 的 diff
    if (event.type === 'pending' && event.reason.type === 'merge_approval') {
      void this.loadDiff('merge-proposal.diff');
    }
    if (event.type === 'pending' && event.reason.type === 'human_review') {
      void this.loadDiff('review-diff.diff');
    }
    if (previousPending === 'human_review' && event.type === 'stage_changed') {
      void this.loadDiff('review-diff.diff');
    }
  }

  private scheduleRefetch(): void {
    if (this.refetchTimer) return;
    this.refetchTimer = setTimeout(() => {
      this.refetchTimer = null;
      void this.load(this.id ?? undefined, true);
    }, 300);
  }

  /* ─────────────── 会话 / 命令 / 文件 ─────────────── */

  async loadConversation(runId: number, force = false): Promise<NodeConversation | null> {
    if (!this.id) return null;
    if (!force && this.conversationsFull[runId]) return this.conversationsFull[runId];
    this.conversationsLoading = true;
    try {
      const conv = await getConversation(this.id, runId);
      this.conversationsFull = { ...this.conversationsFull, [runId]: conv };
      return conv;
    } catch (err) {
      this.error = (err as Error).message;
      return null;
    } finally {
      this.conversationsLoading = false;
    }
  }

  async loadCommandOutput(commandId: number): Promise<void> {
    if (!this.id) return;
    try {
      const text = await getCommandOutput(this.id, commandId);
      this.commandOutputFull = { ...this.commandOutputFull, [commandId]: text };
    } catch (err) {
      this.commandOutputError = { ...this.commandOutputError, [commandId]: (err as Error).message };
    }
  }

  outputFor(command: NodeCommand): string | null {
    return this.commandOutputFull[command.id] ?? this.state.commandOutput[command.id] ?? null;
  }

  async loadFile(path: string): Promise<void> {
    if (!this.id) return;
    if (this.files[path]?.content !== undefined) return;
    try {
      const content = await getTaskFile(this.id, path);
      this.files = { ...this.files, [path]: { path, content, error: null, status: 200 } };
    } catch (err) {
      const status = err instanceof ApiError ? err.status : 0;
      const message =
        status === 403
          ? '路径越出任务目录，已拒绝访问。'
          : status === 404
            ? '文件不存在或尚未生成。'
            : (err as Error).message;
      this.files = { ...this.files, [path]: { path, content: null, error: message, status } };
    }
  }

  getFile(path: string): LoadedFile | undefined {
    return this.files[path];
  }

  async loadDiff(path: string): Promise<void> {
    if (!this.id) return;
    this.diffError = null;
    this.diffStale = false;
    try {
      const raw = await getTaskFile(this.id, path);
      this.diffRaw = raw;
      this.diff = parseUnifiedDiff(raw);
    } catch (err) {
      const status = err instanceof ApiError ? err.status : 0;
      this.diff = null;
      this.diffRaw = null;
      this.diffError = status === 403 ? '无权限读取该 diff。' : 'diff 尚未生成或不可读。';
      // merge_approval / human_review 期间缺失 → 基准前移或重新生成中
      const pendingType = this.state.pendingReason?.type;
      if (status === 404 && (pendingType === 'merge_approval' || pendingType === 'human_review')) {
        this.diffStale = true;
      }
    }
  }

  /* ─────────────── 动作提交 ─────────────── */

  isBusy(action: AllowedAction, cursorId?: string): boolean {
    return this.busyKey === actionKey(action, cursorId);
  }

  private clearBusy(): void {
    this.busyKey = null;
    if (this.busyTimer) {
      clearTimeout(this.busyTimer);
      this.busyTimer = null;
    }
  }

  /**
   * 纯渲染执行：resume 类走 `POST /resume`，side_effect 走配对端点（决策 101/119）。
   * `cursor_id` 优先取动作自带，其次取显式传入的所属分支游标（决策 91）。
   */
  async runAllowedAction(action: AllowedAction, options: RunActionOptions = {}): Promise<void> {
    if (!this.id) return;
    if (action.kind === 'wait') return;
    const cursorId = action.cursor_id ?? options.cursorId;
    const key = actionKey(action, cursorId);
    this.busyKey = key;
    this.actionError = null;

    try {
      const cursor = this.cursorFor(action, cursorId);
      const pendingType = cursor?.pending_reason?.type ?? this.state.pendingReason?.type;
      await submitAllowedAction(this.id, action, {
        cursorId,
        input: options.input,
        pendingType,
      });
      // 成功不立即复位：等 SSE 回执（safety timeout 兜底）
      this.busyTimer = setTimeout(() => {
        if (this.busyKey === key) this.clearBusy();
      }, 30_000);
    } catch (err) {
      this.actionError = (err as Error).message;
      this.clearBusy();
      throw err;
    }
  }

  private cursorFor(action: AllowedAction, cursorId?: string): BranchCursor | undefined {
    const id = action.cursor_id ?? cursorId;
    return id ? this.state.cursors.find((c) => c.cursor_id === id) : this.state.cursors[0];
  }

  /** 拆分任务（旁路动作 split_task 的表单提交）。 */
  async submitSplit(tasks: SplitTaskSpec[]): Promise<void> {
    if (!this.id) return;
    this.actionError = null;
    try {
      await splitTask(this.id, tasks);
      void this.load(this.id, true);
    } catch (err) {
      this.actionError = (err as Error).message;
      throw err;
    }
  }

  /** 更换任务级模型（旁路动作 model_override 的表单提交）。 */
  async submitModelOverride(providerId: string): Promise<void> {
    if (!this.id) return;
    this.actionError = null;
    try {
      await modelOverrideTask(this.id, providerId);
      void this.load(this.id, true);
    } catch (err) {
      this.actionError = (err as Error).message;
      throw err;
    }
  }

  /** 人工评审表单（review_mode = human）：通过 / 打回并附意见。 */
  async submitReview(approved: boolean, comments?: string): Promise<void> {
    if (!this.id) return;
    this.busyKey = approved ? 'review:approve' : 'review:reject';
    this.actionError = null;
    try {
      await reviewTask(this.id, approved, comments);
      this.busyTimer = setTimeout(() => this.clearBusy(), 30_000);
    } catch (err) {
      this.actionError = (err as Error).message;
      this.clearBusy();
      throw err;
    }
  }
}

function actionKey(action: AllowedAction, cursorId?: string): string {
  return `${action.action}:${action.cursor_id ?? cursorId ?? ''}`;
}

export const taskDetail = new TaskDetailStore();
