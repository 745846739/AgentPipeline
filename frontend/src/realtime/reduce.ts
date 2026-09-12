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
  text: string;
}

export interface LiveTool {
  run_id: number;
  tool: string;
  phase: 'start' | 'end' | 'error';
  args_summary: string;
}

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
  /** 流式 token 增量累加（决策 123 差距⑤）。 */
  streamTokens: { prompt: number; completion: number };
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
    streamTokens: { prompt: 0, completion: 0 },
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

    case 'conversation_delta':
      return {
        ...base,
        ...taskPatch,
        liveDeltas: [
          ...state.liveDeltas,
          {
            run_id: event.run_id,
            agent_type: event.agent_type,
            role: event.role,
            text: event.text,
          },
        ],
        streamTokens: {
          prompt: state.streamTokens.prompt + event.prompt_tokens,
          completion: state.streamTokens.completion + event.completion_tokens,
        },
      };

    case 'tool_event':
      return {
        ...base,
        ...taskPatch,
        liveTools: [
          ...state.liveTools,
          {
            run_id: event.run_id,
            tool: event.tool,
            phase: event.phase,
            args_summary: event.args_summary,
          },
        ],
      };

    default:
      return base;
  }
}
