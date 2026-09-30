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
import { actionKey } from '../lib/actions';
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
  /** 失败的状态码（非 `ApiError` 记 0）：界面据此分开「这个 id 没有」与「没读到」（票 01）。 */
  errorStatus = $state<number | null>(null);

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
  /**
   * 实时流的状态（票 13 / R2-15）。看板早就有这一档，详情页此前**没有接** `onStatus`，
   * 于是服务器重启或网络断掉之后，命令、会话增量、状态轨道**静静停止更新**，
   * 页面看起来一切健康。
   */
  connectionState = $state<'idle' | 'open' | 'error'>('idle');
  /** 断线时提交动作的提示（不是错误）：说清「发出去了，回执可能要等」（票 13）。 */
  actionNote = $state<string | null>(null);

  private streamManager: StreamManager;
  private refetchTimer: ReturnType<typeof setTimeout> | null = null;
  private busyTimer: ReturnType<typeof setTimeout> | null = null;
  /** 正在飞的对话框动作（重入时把同一个 promise 交回，见 `submitDialogAction`）。 */
  private dialogInFlight: Promise<void> | null = null;

  constructor() {
    this.streamManager = new StreamManager({
      onEvent: (_taskId, event) => this.handleEvent(event),
      // 与看板同一个口径（票 13）：`open` 是健康态、`error` 是断了，而
      // `idle` / `connecting` / `closed` **保持上一个已知状态**——流在重连途中的一瞬间
      // 不是「断了」（一进页面就闪一条红横幅是误报），只有真出错才翻成 `error`。
      onStatus: (_taskId, status) => {
        this.connectionState =
          status === 'open' ? 'open' : status === 'error' ? 'error' : this.connectionState;
      },
      onRecalibrate: () => void this.load(this.id ?? undefined, true),
    });
  }

  async load(id?: string, silent = false): Promise<void> {
    const taskId = id ?? this.id;
    if (!taskId) return;
    this.id = taskId;
    if (!silent) {
      this.loading = true;
      // 用户可见的那一次加载（进页面 / 换 id / 点「重新加载」）从第一帧起就把上一份收走：
      // 否则新 id 的地址下会先闪出旧任务的正文，失败时旧任务的拍板按钮还会留在屏上
      // 并指向新那个坏 id（票 01 / R2-01）。
      this.resetTaskContent();
      // 旧任务的流一并收：事件回调不按 id 过滤，留着就会把 A 的事件记到 B 的头上。
      this.streamManager.sync([]);
    }
    this.error = null;
    this.errorStatus = null;
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
      this.errorStatus = err instanceof ApiError ? err.status : 0;
      // 失败当下再清一次：`getTask` 成功、随后三个并行请求里有一个失败时，
      // 半截的新任务会与上一份的 transitions / conversations / commands 混在一起。
      // 静默 refetch 失败不清——那是后台对齐，把正在看的页面清空比留着更坏；
      // 它只把错误挂出来（横幅），内容照旧。
      if (!silent) this.resetTaskContent();
    } finally {
      if (!silent) this.loading = false;
    }
  }

  /**
   * 把「上一个任务的一切」收走（票 01 / R2-01）。
   *
   * 必须整份收：`{#if task}` 靠 `task` 短路掉加载与空分支，只清 title 之类的局部字段
   * 仍会让旧任务的正文（含 6 颗指向新 id 的拍板按钮）留在屏上。
   */
  private resetTaskContent(): void {
    this.state = emptyTaskDetailState();
    this.conversationsFull = {};
    this.commandOutputFull = {};
    this.commandOutputError = {};
    this.files = {};
    this.diff = null;
    this.diffRaw = null;
    this.diffError = null;
    this.diffStale = false;
    this.actionError = null;
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

  /**
   * 「现场」页签的批量装载（决策 349）：把每一轮的完整会话读齐，时间线才摆得开——
   * 旧「会话页签」是选中哪轮读哪轮，合并版没有选中态可搭。缓存挡住重复：已装载的
   * 轮不发第二跳，进页签几次都只补缺的。并行发（本机服务，轮数有界）。
   */
  async loadAllConversations(): Promise<void> {
    if (!this.id) return;
    const pending = this.state.conversations
      .map((c) => c.run_id)
      .filter((runId) => !this.conversationsFull[runId]);
    if (pending.length === 0) return;
    this.conversationsLoading = true;
    try {
      await Promise.all(pending.map((runId) => this.loadConversation(runId)));
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
    this.actionNote = null;

    try {
      const cursor = this.cursorFor(action, cursorId);
      const pendingType = cursor?.pending_reason?.type ?? this.state.pendingReason?.type;
      await submitAllowedAction(this.id, action, {
        cursorId,
        input: options.input,
        pendingType,
      });
      // 成功不立即复位：等 SSE 回执（safety timeout 兜底）。
      // 流没连着时「等回执」会等于**静默等 30 秒**（票 13 / R2-15）——那就直说。
      if (this.connectionState !== 'open') {
        this.actionNote =
          '实时流未连通：这次动作已经发出，界面要等重连之后才会更新（最长等 30 秒）。';
      }
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
    await this.submitDialogAction('split_task:', () => splitTask(this.id!, tasks));
  }

  /** 更换任务级模型（旁路动作 model_override 的表单提交）。 */
  async submitModelOverride(providerId: string): Promise<void> {
    if (!this.id) return;
    await this.submitDialogAction('model_override:', () => modelOverrideTask(this.id!, providerId));
  }

  /**
   * 两个对话框动作的共同口径（票 03 / R2-03）。
   *
   * 此前这一对**从不设 `busyKey`**，而两个对话框拿的正是 `busyKey !== null` → `submitting`
   * 恒为 false：按钮不禁用、没有转圈，慢网络下再点一次就发出第二个 `POST /tasks/{id}/split`
   * ——原任务被取消两次、子任务建两套。这里补齐三件事：
   *
   * 1. **进「提交中」态**（`busyKey`），对话框的 `submitting` 才真的生效；
   * 2. **重入护栏**：in-flight 期间同一个动作再来一次，交回**同一个 promise**——
   *    不产生第二个请求，调用方也不会把「什么都没发生」当成功（禁用的按钮是第一道，
   *    这是第二道：两次点击可能落在同一次重渲染之前）；
   * 3. 成功**不立即复位**：与 `runAllowedAction` 同一条口径，等 SSE 回执，30s safety timeout 兜底。
   */
  private submitDialogAction(key: string, send: () => Promise<unknown>): Promise<void> {
    if (this.busyKey === key && this.dialogInFlight) return this.dialogInFlight;
    this.busyKey = key;
    this.actionError = null;
    const done = (async () => {
      try {
        await send();
        // 成功之后照旧重读一次（端点回读是权威，SSE 之外的第二条路）
        void this.load(this.id ?? undefined, true);
        this.busyTimer = setTimeout(() => {
          if (this.busyKey === key) this.clearBusy();
        }, 30_000);
      } catch (err) {
        this.actionError = (err as Error).message;
        this.clearBusy();
        throw err;
      } finally {
        this.dialogInFlight = null;
      }
    })();
    this.dialogInFlight = done;
    return done;
  }

  /** 人工评审表单（review_mode = human）：通过 / 打回并附意见。 */
  async submitReview(approved: boolean, comments?: string): Promise<void> {
    if (!this.id) return;
    // 这两枚是**第三种拼法，但只写不读**（票 05 取证）：全仓没有一处拿它们的值做相等
    // 比较——评审按钮只查 `busyKey !== null`——故它们不参与「哪个动作在忙」的身份判定，
    // 不与 `actionKey` 的四段式竞争。评审表单不是流水线动作（没有游标与落点），硬塞进
    // 四段式只会造出一段恒空的伪身份；裁定：保持原样，由这条注释记下取舍。
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

export const taskDetail = new TaskDetailStore();
