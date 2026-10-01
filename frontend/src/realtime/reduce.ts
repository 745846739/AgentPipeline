import type {
  AllowedAction,
  BranchCursor,
  CommandSource,
  ConversationSummary,
  NodeCommand,
  PendingContext,
  PendingReason,
  SseEvent,
  Task,
  TaskListItem,
  Transition,
} from '../api/types';

/**
 * SSE 事件归约（design/frontend-design.md §9.1 归约表逐行实现；决策 76/84/123）。
 *
 * 纯函数：不触网、不读时钟、不改入参。需要时间戳的追加行由调用方补齐；
 * 需要 **allowed_actions** 的场景一律置 `refetchRequested`——
 * 动作集由后端权威下发，前端不自行计算（决策 101 / 49）。
 */

/* ─────────────────────────────── board ─────────────────────────────── */

export interface BoardState {
  tasks: TaskListItem[];
}

export function emptyBoardState(tasks: TaskListItem[] = []): BoardState {
  return { tasks };
}

function updateCursor(
  task: TaskListItem,
  branch: string,
  patch: (c: BranchCursor) => BranchCursor,
): BranchCursor[] {
  return task.branches.map((c) => (c.branch === branch ? patch(c) : c));
}

function applyBoardEvent(task: TaskListItem, event: SseEvent): TaskListItem {
  switch (event.type) {
    case 'node_started':
    case 'node_finished':
      // §9.1：节点事件只归约「列归属 / 站点信号色 / 迷你轨游标」，**不覆盖任务状态**——
      // 否则一条陈旧的节点事件会把 pending 任务拉回 running（直到 refetch 才纠正）。
      return {
        ...task,
        status: task.status === 'pending' ? 'pending' : 'running',
        current_stage: event.stage,
        current_node: event.node,
        branches: updateCursor(task, event.branch, (c) =>
          c.status === 'pending'
            ? c
            : { ...c, stage: event.stage, node: event.node, status: 'active' },
        ),
      };

    case 'cursor_changed':
      return {
        ...task,
        status: event.status === 'pending' ? 'pending' : task.status,
        current_stage: event.stage,
        current_node: event.node,
        branches: updateCursor(task, event.branch, (c) => ({
          ...c,
          stage: event.stage,
          node: event.node,
          status: event.status as BranchCursor['status'],
        })),
      };

    case 'stage_changed':
      return {
        ...task,
        status: task.status === 'pending' ? 'running' : task.status,
        pending_reason: null,
        current_stage: event.to_stage,
        current_node: event.to_node,
        branches: updateCursor(task, event.branch, (c) => ({
          ...c,
          stage: event.to_stage,
          node: event.to_node,
          status: 'active',
          pending_reason: null,
        })),
      };

    case 'pending':
      return {
        ...task,
        status: 'pending',
        pending_reason: event.reason,
        branches: updateCursor(task, event.branch, (c) => ({
          ...c,
          status: 'pending',
          pending_reason: event.reason,
        })),
      };

    case 'pending_updated': {
      const nextReason = withContext(task.pending_reason, event.context);
      return {
        ...task,
        pending_reason: nextReason,
        branches: updateCursor(task, event.branch, (c) => ({
          ...c,
          pending_reason: withContext(c.pending_reason, event.context),
        })),
      };
    }

    case 'stalled':
      return { ...task, stalled: true };

    case 'task_done':
      return {
        ...task,
        status: 'done',
        pending_reason: null,
        stalled: false,
        current_stage: 'done',
        branches: task.branches.map((c) => ({ ...c, status: 'archived' as const })),
      };

    case 'task_failed':
      return { ...task, status: 'failed', pending_reason: null, stalled: false };

    case 'task_cancelled':
      return { ...task, status: 'cancelled', pending_reason: null, stalled: false };

    // command_* / conversation_delta / tool_event 与看板无关（§9.1 表 "—"）
    default:
      return task;
  }
}

/** board 归约：列归属 / 站点信号色 / 迷你轨游标 / 待办计数（§9.1 左列）。 */
export function reduceBoard(state: BoardState, event: SseEvent): BoardState {
  let changed = false;
  const tasks = state.tasks.map((t) => {
    if (t.id !== event.task_id) return t;
    const next = applyBoardEvent(t, event);
    if (next !== t) changed = true;
    return next;
  });
  return changed ? { tasks } : state;
}

function withContext(
  reason: PendingReason | null,
  context: PendingContext,
): PendingReason | null {
  if (!reason) return reason;
  return { ...reason, context: { ...reason.context, ...context } };
}

/* ─────────────────────────────── task detail ─────────────────────────────── */

export interface LiveDelta {
  run_id: number;
  agent_type: string;
  role: string;
  /**
   * 这一段是回话还是思考（决策 244）。后端缺省按 `content` 发（`serde(default)`），
   * 归约在这里把缺省补齐——下游（现场归约）不再各自猜缺省值。
   */
  channel: 'content' | 'reasoning';
  text: string;
  /**
   * 到达序（决策 359①）：与 `LiveTool.seq` 共用 `TaskDetailState.liveSeq` 一只计数器。
   * 增量与工具回执是两份数组，「先想了什么、再查了什么」的交织序只有这对戳答得出来。
   */
  seq: number;
}

export interface LiveTool {
  run_id: number;
  tool: string;
  phase: 'start' | 'end' | 'error';
  args_summary: string;
  /** 完整参数原文（决策 301，老后端缺省 → 空串）。 */
  args: string;
  /** 工具结果（只在 end / error 相位带；老后端缺省 → 空串）。 */
  result: string;
  /** 到达序：与 {@link LiveDelta.seq} 同一只计数器；合并进 start 那条时沿用 start 的戳。 */
  seq: number;
}

/**
 * 直播增量的**条数上限**（决策 362①）：`liveDeltas` 与 `liveTools` **各**一只上限，
 * 方向是**环形缓冲丢最早**——直播只关心尾部，「到顶拒收」会让流停在半截。
 *
 * 归约成本由「原始增量条数」驱动（`buildTaskScene` 每个到达的 delta 都从零重跑一遍
 * 全量 `sort`，O(N log N)），与折出多少步无关；事件以约 50/s 到达，长 run 跑到后段
 * 前端追不上。**当前正在长的那一步不豁免**——一豁免，超长单步那次就绕过上限。
 */
export const LIVE_WINDOW_LIMIT = 10_000;

export interface TaskDetailState {
  task: Task | null;
  cursors: BranchCursor[];
  allowedActions: AllowedAction[];
  transitions: Transition[];
  commands: NodeCommand[];
  commandOutput: Record<number, string>;
  conversations: ConversationSummary[];
  /** 进行中 run 的流式增量（按 run_id 分组）。 */
  liveDeltas: LiveDelta[];
  liveTools: LiveTool[];
  /**
   * 被上限丢弃过增量的 run（决策 362①④）：这些轮的折叠步序最前要摆一行非交互的
   * 「更早的增量已省略」。**只有标记，不含条数**——省略行不可交互、不承诺可数。
   *
   * 丢弃后 `seq` 保留原值（前面留洞）不重编号：`seq` 只用于排序，留洞无害；重编号
   * 要给每次截断加一趟 O(N) 减法，还把「`seq` 单调」偷换成「窗口内单调」。
   */
  liveDroppedRuns: Record<number, true>;
  /** 流式 token 增量累加（决策 123 差距⑤）。 */
  streamTokens: { prompt: number; completion: number };
  /**
   * 直播事件的**到达序**计数器（决策 359①）：每条 conversation_delta / tool_event 领一个号。
   * 增量与工具回执两份数组靠这对戳在渲染层归并出真实的发生顺序。
   */
  liveSeq: number;
  pendingReason: PendingReason | null;
  terminal: 'done' | 'failed' | 'cancelled' | null;
  /** 后端权威动作集可能已变，需 refetch `GET /tasks/{id}`。 */
  refetchRequested: boolean;
  lastEventType: SseEvent['type'] | null;
  lastEventBranch: string | null;
  syntheticSeq: number;
}

export function emptyTaskDetailState(partial: Partial<TaskDetailState> = {}): TaskDetailState {
  return {
    task: null,
    cursors: [],
    allowedActions: [],
    transitions: [],
    commands: [],
    commandOutput: {},
    conversations: [],
    liveDeltas: [],
    liveTools: [],
    liveDroppedRuns: {},
    streamTokens: { prompt: 0, completion: 0 },
    liveSeq: 0,
    pendingReason: null,
    terminal: null,
    refetchRequested: false,
    lastEventType: null,
    lastEventBranch: null,
    syntheticSeq: -1,
    ...partial,
  };
}

/** 从既有状态重算 task/cursors（applyBoardEvent 需要 TaskListItem 形态）。 */
function patchDetailTask(state: TaskDetailState, event: SseEvent): Partial<TaskDetailState> {
  if (!state.task) return {};
  const asList: TaskListItem = { ...state.task, branches: state.cursors, blocks: [] };
  const next = applyBoardEvent(asList, event);
  if (next === asList) return {};
  const { branches, blocks, ...rest } = next;
  void blocks;
  return { task: rest, cursors: branches };
}

function appendTransition(state: TaskDetailState, event: Extract<SseEvent, { type: 'stage_changed' }>): Transition {
  const seq = state.syntheticSeq - 1;
  return {
    id: seq,
    task_id: event.task_id,
    branch: event.branch,
    from_stage: event.from_stage,
    from_node: event.from_node,
    to_stage: event.to_stage,
    to_node: event.to_node,
    trigger: event.trigger as Transition['trigger'],
    reason: event.reason,
    created_at: new Date().toISOString(),
  };
}

function commandFromEvent(
  state: TaskDetailState,
  event: Extract<SseEvent, { type: 'command_started' }>,
): NodeCommand {
  const cursor = state.cursors.find((c) => c.branch === event.branch);
  return {
    id: event.command_id,
    task_id: event.task_id,
    run_id: null,
    stage: cursor?.stage ?? state.task?.current_stage ?? 'init',
    node: cursor?.node ?? state.task?.current_node ?? 'execute',
    source: event.source as CommandSource,
    command: event.command,
    // SSE 那个事件不带原串（决策 297 只把原串落在台账里）：这一行是**乐观**的，
    // 原串要等台账重读才到（`command_finished` 与 10s 对齐 tick 都会重取）。
    // 不在这里编一个：编出来的「原串」比晚几秒到的真相更坏。
    original_command: null,
    cwd: '',
    exit_code: null,
    stdout_path: null,
    stdout_preview: null,
    stderr_preview: null,
    duration_ms: null,
    started_at: new Date().toISOString(),
    finished_at: null,
  };
}

/**
 * 环形缓冲丢最早（决策 362①）：超过 {@link LIVE_WINDOW_LIMIT} 的条数从**头部**丢掉，
 * 返回尾部那份与被丢掉的那些增量所属的 run。
 *
 * 入参是调用方刚拼出的新数组（尚未写回 state），故这里可以安全地切片。
 */
function trimOldest<T extends { run_id: number }>(list: T[]): { kept: T[]; dropped: number[] } {
  if (list.length <= LIVE_WINDOW_LIMIT) return { kept: list, dropped: [] };
  const cut = list.length - LIVE_WINDOW_LIMIT;
  return { kept: list.slice(cut), dropped: list.slice(0, cut).map((x) => x.run_id) };
}

/** 把被丢弃增量的 run 并进标记表（决策 362①）；没丢东西时原样返回，免得白白换新对象。 */
function markDropped(
  prev: Record<number, true>,
  dropped: number[],
): Record<number, true> {
  if (dropped.length === 0) return prev;
  const next = { ...prev };
  for (const runId of dropped) next[runId] = true;
  return next;
}

/** 详情归约（§9.1 右列）。 */
export function reduceTaskDetail(state: TaskDetailState, event: SseEvent): TaskDetailState {
  const base: TaskDetailState = {
    ...state,
    lastEventType: event.type,
    lastEventBranch: event.branch,
  };
  const taskPatch = patchDetailTask(state, event);

  switch (event.type) {
    case 'stage_changed':
      return {
        ...base,
        ...taskPatch,
        transitions: [...state.transitions, appendTransition(state, event)],
        syntheticSeq: state.syntheticSeq - 1,
      };

    case 'pending':
      return {
        ...base,
        ...taskPatch,
        pendingReason: event.reason,
        refetchRequested: true,
      };

    case 'pending_updated':
      return {
        ...base,
        ...taskPatch,
        pendingReason: withContext(state.pendingReason, event.context),
        refetchRequested: true,
      };

    case 'node_started':
    case 'node_finished':
    case 'cursor_changed':
    case 'stalled':
      return { ...base, ...taskPatch };

    case 'task_done':
    case 'task_failed':
    case 'task_cancelled':
      return {
        ...base,
        ...taskPatch,
        terminal:
          event.type === 'task_done'
            ? 'done'
            : event.type === 'task_failed'
              ? 'failed'
              : 'cancelled',
        pendingReason: null,
        refetchRequested: true,
      };

    case 'command_started':
      if (state.commands.some((c) => c.id === event.command_id)) return base;
      return {
        ...base,
        ...taskPatch,
        commands: [...state.commands, commandFromEvent(state, event)],
      };

    case 'command_output':
      return {
        ...base,
        ...taskPatch,
        commandOutput: {
          ...state.commandOutput,
          [event.command_id]: (state.commandOutput[event.command_id] ?? '') + event.chunk,
        },
      };

    case 'command_finished':
      return {
        ...base,
        ...taskPatch,
        commands: state.commands.map((c) =>
          c.id === event.command_id
            ? { ...c, exit_code: event.exit_code, duration_ms: event.duration_ms, finished_at: new Date().toISOString() }
            : c,
        ),
      };

    case 'conversation_delta': {
      const delta: LiveDelta = {
        run_id: event.run_id,
        agent_type: event.agent_type,
        role: event.role,
        // 老后端不发 channel（`serde(default)`）→ 缺省 content：那正是它此前
        // 唯一见过的形状（决策 244 的加性口径）。
        channel: event.channel === 'reasoning' ? 'reasoning' : 'content',
        text: event.text,
        seq: state.liveSeq,
      };
      const trimmed = trimOldest([...state.liveDeltas, delta]);
      return {
        ...base,
        ...taskPatch,
        liveDeltas: trimmed.kept,
        liveDroppedRuns: markDropped(base.liveDroppedRuns, trimmed.dropped),
        streamTokens: {
          prompt: state.streamTokens.prompt + event.prompt_tokens,
          completion: state.streamTokens.completion + event.completion_tokens,
        },
        liveSeq: state.liveSeq + 1,
      };
    }

    case 'tool_event': {
      // 一次调用一条记录（决策 244 同款判据）：start 与随后的 end / error 合成一条，
      // 不合并的话一次 read_file 会在现场留两条回执（一条「运行中」一条「完成」）。
      // 调用按序执行，故「该 run 最后一条仍是 start」就是「这一次调用在等结果」；
      // 用工具名配对不够——同一 run 连查两次同名工具是常态。
      const tools = [...state.liveTools];
      let last = tools.length - 1;
      while (last >= 0 && tools[last].run_id !== event.run_id) last--;
      if (last >= 0 && tools[last].phase === 'start' && event.phase !== 'start') {
        // 收尾整条换掉开调那条：相位与结果取新到的，参数原文新到的不带（老后端）就保住
        // start 那份。**到达序沿用 start 的戳**——回执在时间线上的位置是这次调用开始的
        // 地方（先想 → 查 → 再想），不是它收尾的地方。
        tools[last] = {
          ...tools[last],
          phase: event.phase,
          args: event.args || tools[last].args,
          result: event.result ?? tools[last].result,
        };
        return { ...base, ...taskPatch, liveTools: tools, liveSeq: state.liveSeq + 1 };
      }
      const tool: LiveTool = {
        run_id: event.run_id,
        tool: event.tool,
        phase: event.phase,
        args_summary: event.args_summary,
        args: event.args ?? '',
        result: event.result ?? '',
        seq: state.liveSeq,
      };
      const trimmed = trimOldest([...tools, tool]);
      return {
        ...base,
        ...taskPatch,
        liveTools: trimmed.kept,
        liveDroppedRuns: markDropped(base.liveDroppedRuns, trimmed.dropped),
        liveSeq: state.liveSeq + 1,
      };
    }

    default:
      return base;
  }
}
