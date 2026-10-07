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
  /** merge approve 的「合入后 push」开关（决策 393）。 */
  push?: boolean;
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
  /** 现场页签是否在屏（决策 365）。非 `$state`：它只由页面 effect 写，界面从不读它。 */
  private sceneVisible = false;

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
      const [flow, conversations] = await Promise.all([getFlow(taskId), getConversations(taskId)]);
      this.state = {
        ...this.state,
        transitions: flow.transitions,
        conversations,
      };
      this.streamManager.sync([taskId]);
      // commands 走**后台补拉**（决策 361，票 02）：它在首屏的 `Promise.all` 里时，时间线
      // 要的 `transitions`（上面那 7 KB 的 `/flow`）被 1.33 MB 的 commands 一起扣住——
      // 106 实测那一条要在公网链路上搬 8.5–10.9 秒，而它与此页签的默认视图毫无关系。
      //
      // 静默 refetch 也补拉**仅当现场页签在屏**（决策 365）：361 给它留的保鲜机制是
      // 「SSE 的 `command_started` / `command_finished` 承担增量」，而服务端从不发这两类
      // 事件（见 `setSceneVisible` 的说明）——不补这一趟，开屏之后跑的命令再也到不了界面。
      if (!silent || this.sceneVisible) void this.loadCommands(taskId);
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

  /**
   * 现场页签在屏 / 离屏（决策 365，修订决策 361② 的一条错误前提）。
   *
   * 361 把 commands 移出首屏时，给它留的保鲜机制写的是「SSE 的 `command_started` /
   * `command_finished` 与 `liveTools` 已承担在飞与增量」——**前半句不成立**：服务端从不发
   * 这两类事件（`crates/core/src/sse.rs` 里只有枚举与往返测试，`crates/core/src/exec.rs`
   * 只发 `CommandOutput`），客户端 reduce 里那两个分支是只有单测喂得到的死代码。于是首屏
   * 那一次补拉之后，命令**再也不刷新**：页签徽标停在旧数，现场时间线看不到新跑的命令
   * （E2E-⑦ 用例① 实测红，2026-10-01）。
   *
   * 改法不是把 commands 塞回首屏，而是**只在现场页签在屏时保鲜**：页签进场当场补拉一次，
   * 在屏期间静默 refetch 也重拉（`load()` 里的判据）。页签不在屏时不重拉——那是 361 认下
   * 的账，且此刻没人看这条数据。
   *
   * 落点在 store 而不是页面的 `$effect` 里直拉：effect 里拉一次就把 `state` 写成了自己的
   * 依赖，下一次 `state` 变化又触发一次，转成自触发。这里只在**翻转**时动作，重复调用是空操作。
   */
  setSceneVisible(visible: boolean): void {
    if (visible === this.sceneVisible) return;
    this.sceneVisible = visible;
    if (visible && this.id) void this.loadCommands(this.id);
  }

  /**
   * commands 的**后台补拉**（决策 361，票 02；保鲜口径见 `setSceneVisible`）。
   *
   * 载荷最大的一条路，故它自己走：首屏不等它（页签徽标初始短暂显示 0，随补拉更新），
   * 到货后并入 `state.commands`。
   *
   * **非静默装载必发、静默 refetch 只在现场页签在屏时发**（决策 365）：SSE 驱动的静默
   * refetch（`scheduleRefetch` 的 300ms 去抖）每转一次都重拉 1.33 MB 是不划算的，故页签
   * 不在屏时不拉；在屏时拉是因为**没有别的承担者**（361 原以为有，见 `setSceneVisible`）。
   * **也不清已到的那一份**：清掉是纯倒退。
   *
   * 失败挂到页面的错误位（与静默 refetch 同一条口径：报错不静默），但**只在这一份仍然
   * 属于当前任务时**——换过任务/已被收走就别把上一份的失败挂到新 id 上。
   */
  private async loadCommands(taskId: string): Promise<void> {
    try {
      const commands = await getCommands(taskId);
      if (this.id !== taskId) return;
      this.state = { ...this.state, commands };
    } catch (err) {
      if (this.id !== taskId) return;
      this.error = (err as Error).message;
    }
  }

  dispose(): void {
    this.streamManager.stopAll();
    if (this.refetchTimer) clearTimeout(this.refetchTimer);
    if (this.busyTimer) clearTimeout(this.busyTimer);
  }

  /**
   * **落地即清**（决策 362②）：某个 run 的会话正文一到手（`conversationsFull[runId]`），
   * 它的直播增量就没有消费价值了——不清的话，时间线会把已落地的 `msgSteps` 与直播折步
   * **同时摆上屏**（双渲染），而且它已停跑（`streaming` 为假），此后任何别的 delta 触发的
   * 全量重归约都会对**全量文本**重跑 `renderMarkdown`（热路径上的 markdown 缝）。
   *
   * 判据是**严格信号** `conversationsFull[runId] !== undefined`，**不是**「摘要里出现了
   * `run_id`」：票 03 的批量填充会先到摘要、后到正文，只看摘要会在正文到手前把流式文本清掉。
   *
   * 只清**已落地**的 run：在飞的另一轮（决策 260 的双轮）不受影响。落点在 store 而非
   * reducer——`TaskDetailState` 没有消息正文字段，正文住在 `conversationsFull` 里，
   * reducer 看不到它。这条也不走 `emptyTaskDetailState()`（那是换任务 / 首次加载的路径）。
   */
  private clearLandedLive(): void {
    const landed = this.conversationsFull;
    if (Object.keys(landed).length === 0) return;
    const isLanded = (runId: number) => landed[runId] !== undefined;
    // 截断标记也要一起核对：某个 run 的增量可能**全被上限丢光**（数组里一条都不剩），
    // 只剩标记——那种情况下只看两个数组会提前返回，标记留在库里，等它落地后
    // 那一轮会永久摆一行不该有的「更早的增量已省略」。
    const hasLanded =
      this.state.liveDeltas.some((d) => isLanded(d.run_id)) ||
      this.state.liveTools.some((t) => isLanded(t.run_id)) ||
      Object.keys(this.state.liveDroppedRuns).some((key) => isLanded(Number(key)));
    if (!hasLanded) return;
    const keptDeltas = this.state.liveDeltas.filter((d) => !isLanded(d.run_id));
    const keptTools = this.state.liveTools.filter((t) => !isLanded(t.run_id));
    const droppedRuns = { ...this.state.liveDroppedRuns };
    for (const key of Object.keys(landed)) delete droppedRuns[Number(key)];
    this.state = {
      ...this.state,
      liveDeltas: keptDeltas,
      liveTools: keptTools,
      liveDroppedRuns: droppedRuns,
    };
  }

  handleEvent(event: Parameters<typeof reduceTaskDetail>[1]): void {
    const previousPending = this.state.pendingReason?.type;
    this.state = reduceTaskDetail(this.state, event);
    // 落地即清（决策 362②）：正文已到手的 run 又来了一条增量（落地那一刻前后到达序上的
    // 竞态），当场收走——否则它会一直挂在时间线上直到下一次正文装载。
    if (
      (event.type === 'conversation_delta' || event.type === 'tool_event') &&
      this.conversationsFull[event.run_id] !== undefined
    ) {
      this.clearLandedLive();
    }
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
      this.clearLandedLive();
      return conv;
    } catch (err) {
      this.error = (err as Error).message;
      return null;
    } finally {
      this.conversationsLoading = false;
    }
  }

  /**
   * 「现场」页签的批量装载（决策 349；决策 361 票 03 改为**一次请求**）。
   *
   * 把每一轮的完整会话读齐，时间线才摆得开——旧「会话页签」是选中哪轮读哪轮，合并版
   * 没有选中态可搭。此前它是 N+1：每轮一条 `GET /conversations/{run_id}`，而浏览器在
   * HTTP/1.1 单源上约 6 并发，48 轮要排八波；现在一条 `?include_messages=true` 拿回
   * 整个任务的全部轮。
   *
   * **增量拉取**：批量请求带 `run_ids` 只取 `conversationsFull` 里还没有的轮——106 实测
   * 全量载荷 1.76 MB、公网链路上要搬 8.5–10.9 秒（决策 361 票 02 的注释），而重进页签 /
   * 静默 refetch 时大头早已在缓存里，重发的只有新落的轮。
   *
   * 缓存仍按 `run_id` 挡重复（`conversationsFull` 与单条读法共用同一张表，深链走
   * `loadConversation` 时不会打架）：已装载的轮不进这个请求的待办。
   *
   * 失败**重试一次再降级到逐条**：批量读法是加性参数，客户端不把「一次拿全」变成唯一的
   * 活路；而 30s 超时后直接掉进逐条会让本已拥塞的链路雪上加霜，一次整段重试是更便宜的
   * 第二发。
   */
  async loadAllConversations(): Promise<void> {
    if (!this.id) return;
    const pending = this.state.conversations
      .map((c) => c.run_id)
      .filter((runId) => !this.conversationsFull[runId]);
    if (pending.length === 0) return;
    this.conversationsLoading = true;
    try {
      let all: NodeConversation[];
      try {
        all = await getConversations(this.id, { includeMessages: true, runIds: pending });
      } catch (first) {
        // 只对「再试一次可能就好」的那类失败付重试（`mapRequestError` 的口径：
        // 超时 / 网络不通都是 status 0）；4xx/5xx 是确定性答案，重发只是浪费一发。
        if (!(first instanceof ApiError) || first.status !== 0) throw first;
        all = await getConversations(this.id, { includeMessages: true, runIds: pending });
      }
      const fresh: Record<number, NodeConversation> = { ...this.conversationsFull };
      for (const conv of all) {
        if (pending.includes(conv.run_id)) fresh[conv.run_id] = conv;
      }
      this.conversationsFull = fresh;
      this.clearLandedLive();
    } catch {
      // 老后端（无 `include_messages`）或中间层剥了 query → 回到逐条
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
        push: options.push,
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
