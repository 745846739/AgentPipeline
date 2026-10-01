import { describe, expect, it } from 'vitest';
import type {
  BranchCursor,
  PendingReason,
  SseEvent,
  TaskListItem,
} from '../api/types';
import {
  emptyBoardState,
  emptyTaskDetailState,
  reduceBoard,
  reduceTaskDetail,
} from './reduce';

function cursor(overrides: Partial<BranchCursor> = {}): BranchCursor {
  return {
    cursor_id: 'c-main',
    branch: 'main',
    stage: 'architect-design',
    node: 'execute',
    status: 'active',
    validate_attempts: 0,
    skipped_to_join: false,
    pending_reason: null,
    ...overrides,
  };
}

function task(overrides: Partial<TaskListItem> = {}): TaskListItem {
  return {
    id: 't1',
    project_id: 'p1',
    title: '实现用户登录',
    description: '',
    status: 'running',
    current_stage: 'architect-design',
    current_node: 'execute',
    validate_attempts: 0,
    pending_reason: null,
    stewardship: null,
    worktree_path: null,
    branch_name: 'kanban/t1',
    total_tokens: 100,
    total_calls: 2,
    review_mode: 'agent',
    model_override: null,
    archived_at: null,
    stalled: false,
    executor_owner: null,
    created_at: '2026-09-12T00:00:00Z',
    updated_at: '2026-09-12T00:05:00Z',
    branches: [cursor()],
    blocks: [],
    ...overrides,
  };
}

const pendingReason: PendingReason = {
  type: 'info_insufficient',
  stage: 'architect-design',
  node: 'validate_input',
  message: '设计缺少数据流定义',
};

const taskId = 't1';

describe('reduceBoard — §9.1 归约表左列逐事件', () => {
  it('node_started：更新当前阶段 / 游标状态 / 列归属', () => {
    const event: SseEvent = {
      type: 'node_started',
      task_id: taskId,
      branch: 'main',
      stage: 'develop',
      node: 'execute',
      attempt: 1,
      run_id: 7,
    };
    const next = reduceBoard(emptyBoardState([task()]), event);
    expect(next.tasks[0].status).toBe('running');
    expect(next.tasks[0].current_stage).toBe('develop');
    expect(next.tasks[0].current_node).toBe('execute');
    expect(next.tasks[0].branches[0].stage).toBe('develop');
    expect(next.tasks[0].branches[0].status).toBe('active');
  });

  it('node_finished：只推进游标，不改任务状态', () => {
    const event: SseEvent = {
      type: 'node_finished',
      task_id: taskId,
      branch: 'main',
      stage: 'develop',
      node: 'validate_output',
      attempt: 1,
      run_id: 7,
      status: 'success',
      duration_ms: 1200,
      prompt_tokens: 10,
      completion_tokens: 20,
    };
    const next = reduceBoard(emptyBoardState([task()]), event);
    expect(next.tasks[0].status).toBe('running');
    expect(next.tasks[0].current_node).toBe('validate_output');
  });

  it('cursor_changed：waiting_join 不把任务翻成 pending', () => {
    const event: SseEvent = {
      type: 'cursor_changed',
      task_id: taskId,
      branch: 'test-design',
      cursor_id: 'c-test',
      status: 'waiting_join',
      stage: 'test-design',
      node: 'validate_output',
    };
    const t = task({
      branches: [
        cursor(),
        cursor({ cursor_id: 'c-test', branch: 'test-design', stage: 'test-design' }),
      ],
    });
    const next = reduceBoard(emptyBoardState([t]), event);
    expect(next.tasks[0].status).toBe('running');
    expect(next.tasks[0].branches[1].status).toBe('waiting_join');
  });

  it('cursor_changed：pending 翻成 pending 状态', () => {
    const event: SseEvent = {
      type: 'cursor_changed',
      task_id: taskId,
      branch: 'main',
      cursor_id: 'c-main',
      status: 'pending',
      stage: 'architect-design',
      node: 'validate_input',
    };
    const next = reduceBoard(emptyBoardState([task()]), event);
    expect(next.tasks[0].status).toBe('pending');
  });

  it('stage_changed：清 pending 并更新列归属', () => {
    const t = task({ status: 'pending', pending_reason: pendingReason });
    const event: SseEvent = {
      type: 'stage_changed',
      task_id: taskId,
      branch: 'main',
      from_stage: 'architect-design',
      from_node: 'validate_input',
      to_stage: 'architect-design',
      to_node: 'execute',
      trigger: 'user_resume',
      reason: null,
    };
    const next = reduceBoard(emptyBoardState([t]), event);
    expect(next.tasks[0].status).toBe('running');
    expect(next.tasks[0].pending_reason).toBeNull();
    expect(next.tasks[0].branches[0].pending_reason).toBeNull();
  });

  it('pending / pending_updated：顶栏计数来源 + 分支药丸', () => {
    const event: SseEvent = {
      type: 'pending',
      task_id: taskId,
      branch: 'develop-design',
      cursor_id: 'c-dev',
      reason: pendingReason,
    };
    const t = task({
      branches: [cursor({ cursor_id: 'c-dev', branch: 'develop-design' })],
    });
    const next = reduceBoard(emptyBoardState([t]), event);
    expect(next.tasks[0].status).toBe('pending');
    expect(next.tasks[0].branches[0].status).toBe('pending');
    expect(next.tasks[0].branches[0].pending_reason?.type).toBe('info_insufficient');

    const updated: SseEvent = {
      type: 'pending_updated',
      task_id: taskId,
      branch: 'develop-design',
      cursor_id: 'c-dev',
      context: { kind: 'duplicate_risk', conflict_task_ids: ['t2'] },
    };
    const next2 = reduceBoard(next, updated);
    expect(next2.tasks[0].pending_reason?.context?.kind).toBe('duplicate_risk');
    expect(next2.tasks[0].pending_reason?.context?.conflict_task_ids).toEqual(['t2']);
  });

  it('stalled：只置高亮标志', () => {
    const event: SseEvent = {
      type: 'stalled',
      task_id: taskId,
      branch: 'main',
      pending_hours: 80,
    };
    const next = reduceBoard(emptyBoardState([task({ status: 'pending' })]), event);
    expect(next.tasks[0].stalled).toBe(true);
    expect(next.tasks[0].status).toBe('pending');
  });

  it.each([
    ['task_done', 'done'],
    ['task_failed', 'failed'],
    ['task_cancelled', 'cancelled'],
  ] as const)('%s：移列到终态', (type, expected) => {
    const event = { type, task_id: taskId, branch: 'main' } as SseEvent;
    const next = reduceBoard(emptyBoardState([task({ status: 'pending' })]), event);
    expect(next.tasks[0].status).toBe(expected);
    expect(next.tasks[0].pending_reason).toBeNull();
    expect(next.tasks[0].stalled).toBe(false);
    if (type === 'task_done') expect(next.tasks[0].current_stage).toBe('done');
  });

  it('command_* / conversation_delta / tool_event 与看板无关（返回同一 state）', () => {
    const state = emptyBoardState([task()]);
    const events: SseEvent[] = [
      { type: 'command_started', task_id: taskId, branch: 'main', command_id: 1, command: 'ls', source: 'agent' },
      { type: 'command_output', task_id: taskId, branch: 'main', command_id: 1, chunk: 'x' },
      { type: 'command_finished', task_id: taskId, branch: 'main', command_id: 1, exit_code: 0, duration_ms: 5 },
      {
        type: 'conversation_delta',
        task_id: taskId,
        branch: 'main',
        run_id: 1,
        agent_type: 'main',
        role: 'assistant',
        text: 'hi',
        prompt_tokens: 1,
        completion_tokens: 1,
      },
      { type: 'tool_event', task_id: taskId, branch: 'main', run_id: 1, tool: 'read_file', phase: 'end', args_summary: 'a' },
    ];
    for (const event of events) {
      expect(reduceBoard(state, event)).toBe(state);
    }
  });

  it('非本任务事件不改变 state', () => {
    const state = emptyBoardState([task()]);
    const event: SseEvent = { type: 'task_done', task_id: 'other', branch: 'main' };
    expect(reduceBoard(state, event)).toBe(state);
  });
});

describe('reduceTaskDetail — §9.1 归约表右列逐事件', () => {
  const base = () =>
    emptyTaskDetailState({
      task: task(),
      cursors: [cursor()],
      allowedActions: [],
    });

  it('stage_changed：hero 状态 + 时间线追加', () => {
    const event: SseEvent = {
      type: 'stage_changed',
      task_id: taskId,
      branch: 'main',
      from_stage: 'architect-design',
      from_node: 'validate_input',
      to_stage: 'architect-design',
      to_node: 'execute',
      trigger: 'normal',
      reason: null,
    };
    const next = reduceTaskDetail(base(), event);
    expect(next.task?.current_node).toBe('execute');
    expect(next.transitions).toHaveLength(1);
    expect(next.transitions[0].to_stage).toBe('architect-design');
    expect(next.transitions[0].trigger).toBe('normal');
  });

  it('pending：打开 dossier 并请求后端权威动作集', () => {
    const event: SseEvent = {
      type: 'pending',
      task_id: taskId,
      branch: 'main',
      cursor_id: 'c-main',
      reason: pendingReason,
    };
    const next = reduceTaskDetail(base(), event);
    expect(next.pendingReason?.type).toBe('info_insufficient');
    expect(next.refetchRequested).toBe(true);
    expect(next.task?.status).toBe('pending');
  });

  it('command_started / command_output / command_finished：命令表增行 / 追加 / 收尾', () => {
    let state = base();
    state = reduceTaskDetail(state, {
      type: 'command_started',
      task_id: taskId,
      branch: 'main',
      command_id: 11,
      command: 'cargo test',
      source: 'agent',
    });
    expect(state.commands).toHaveLength(1);
    expect(state.commands[0].command).toBe('cargo test');

    state = reduceTaskDetail(state, {
      type: 'command_output',
      task_id: taskId,
      branch: 'main',
      command_id: 11,
      chunk: 'running 1 test\n',
    });
    state = reduceTaskDetail(state, {
      type: 'command_output',
      task_id: taskId,
      branch: 'main',
      command_id: 11,
      chunk: 'ok\n',
    });
    expect(state.commandOutput[11]).toBe('running 1 test\nok\n');

    state = reduceTaskDetail(state, {
      type: 'command_finished',
      task_id: taskId,
      branch: 'main',
      command_id: 11,
      exit_code: 0,
      duration_ms: 42,
    });
    expect(state.commands[0].exit_code).toBe(0);
    expect(state.commands[0].duration_ms).toBe(42);
  });

  it('command_started 重复 command_id 幂等', () => {
    const event: SseEvent = {
      type: 'command_started',
      task_id: taskId,
      branch: 'main',
      command_id: 11,
      command: 'cargo test',
      source: 'system',
    };
    const once = reduceTaskDetail(base(), event);
    const twice = reduceTaskDetail(once, event);
    expect(twice.commands).toHaveLength(1);
  });

  it('conversation_delta：会话追加 + token 流式累加', () => {
    let state = base();
    state = reduceTaskDetail(state, {
      type: 'conversation_delta',
      task_id: taskId,
      branch: 'main',
      run_id: 7,
      agent_type: 'main',
      role: 'assistant',
      text: '我先读取',
      prompt_tokens: 100,
      completion_tokens: 20,
    });
    state = reduceTaskDetail(state, {
      type: 'conversation_delta',
      task_id: taskId,
      branch: 'main',
      run_id: 7,
      agent_type: 'main',
      role: 'assistant',
      text: '设计文档',
      prompt_tokens: 0,
      completion_tokens: 30,
    });
    expect(state.liveDeltas).toHaveLength(2);
    expect(state.streamTokens).toEqual({ prompt: 100, completion: 50 });
  });

  it('tool_event：工具卡增 / 收（决策 359① 的字段与到达序）', () => {
    const event: SseEvent = {
      type: 'tool_event',
      task_id: taskId,
      branch: 'main',
      run_id: 7,
      tool: 'write_file',
      phase: 'end',
      args_summary: 'src/auth/mod.rs',
    };
    const next = reduceTaskDetail(base(), event);
    expect(next.liveTools).toEqual([
      {
        run_id: 7,
        tool: 'write_file',
        phase: 'end',
        args_summary: 'src/auth/mod.rs',
        args: '',
        result: '',
        seq: 0,
      },
    ]);
    expect(next.liveSeq).toBe(1);
  });

  it('conversation_delta：channel 缺省落 content、reasoning 原样带上；seq 按到达序发（决策 244 / 359①）', () => {
    const delta = (overrides: Partial<Extract<SseEvent, { type: 'conversation_delta' }>>) =>
      reduceTaskDetail(base(), {
        type: 'conversation_delta',
        task_id: taskId,
        branch: 'main',
        run_id: 7,
        agent_type: 'main',
        role: 'assistant',
        text: 'x',
        prompt_tokens: 0,
        completion_tokens: 0,
        ...overrides,
      });
    const plain = delta({});
    expect(plain.liveDeltas[0].channel).toBe('content');
    const reasoning = delta({ channel: 'reasoning' });
    expect(reasoning.liveDeltas[0].channel).toBe('reasoning');
    // 到达序跨事件单调（增量与工具回执共用一只计数器）
    let state = base();
    state = reduceTaskDetail(state, {
      type: 'conversation_delta',
      task_id: taskId,
      branch: 'main',
      run_id: 7,
      agent_type: 'main',
      role: 'assistant',
      text: 'a',
      prompt_tokens: 0,
      completion_tokens: 0,
    });
    state = reduceTaskDetail(state, {
      type: 'tool_event',
      task_id: taskId,
      branch: 'main',
      run_id: 7,
      tool: 'read_file',
      phase: 'start',
      args_summary: 'a',
    });
    state = reduceTaskDetail(state, {
      type: 'conversation_delta',
      task_id: taskId,
      branch: 'main',
      run_id: 7,
      agent_type: 'main',
      role: 'assistant',
      text: 'b',
      prompt_tokens: 0,
      completion_tokens: 0,
    });
    expect(state.liveDeltas.map((d) => d.seq)).toEqual([0, 2]);
    expect(state.liveTools.map((t) => t.seq)).toEqual([1]);
    expect(state.liveSeq).toBe(3);
  });

  it('tool_event：start 与随后的 end / error 合成一条，参数与结果归并，到达序沿用 start（决策 359①）', () => {
    let state = base();
    state = reduceTaskDetail(state, {
      type: 'tool_event',
      task_id: taskId,
      branch: 'main',
      run_id: 7,
      tool: 'read_file',
      phase: 'start',
      args_summary: 'src/auth/mod.rs',
      args: '{"path":"src/auth/mod.rs"}',
    });
    state = reduceTaskDetail(state, {
      type: 'tool_event',
      task_id: taskId,
      branch: 'main',
      run_id: 7,
      tool: 'read_file',
      phase: 'end',
      args_summary: 'src/auth/mod.rs',
      args: '{"path":"src/auth/mod.rs"}',
      result: '文件内容',
    });
    expect(state.liveTools).toEqual([
      {
        run_id: 7,
        tool: 'read_file',
        phase: 'end',
        args_summary: 'src/auth/mod.rs',
        args: '{"path":"src/auth/mod.rs"}',
        result: '文件内容',
        seq: 0,
      },
    ]);
    // error 相位同样合成；结果取错误文本
    state = reduceTaskDetail(state, {
      type: 'tool_event',
      task_id: taskId,
      branch: 'main',
      run_id: 7,
      tool: 'grep',
      phase: 'start',
      args_summary: 'todo',
      args: '{"pattern":"todo"}',
    });
    state = reduceTaskDetail(state, {
      type: 'tool_event',
      task_id: taskId,
      branch: 'main',
      run_id: 7,
      tool: 'grep',
      phase: 'error',
      args_summary: 'todo',
      result: '工具执行失败：炸了',
    });
    expect(state.liveTools).toHaveLength(2);
    expect(state.liveTools[1]).toMatchObject({ tool: 'grep', phase: 'error', result: '工具执行失败：炸了', seq: 2 });
  });

  it('tool_event：合并按 run 分辨——并行 run 的 start 不串台', () => {
    let state = base();
    state = reduceTaskDetail(state, {
      type: 'tool_event',
      task_id: taskId,
      branch: 'main',
      run_id: 7,
      tool: 'read_file',
      phase: 'start',
      args_summary: 'a',
    });
    state = reduceTaskDetail(state, {
      type: 'tool_event',
      task_id: taskId,
      branch: 'b',
      run_id: 8,
      tool: 'grep',
      phase: 'start',
      args_summary: 'b',
    });
    state = reduceTaskDetail(state, {
      type: 'tool_event',
      task_id: taskId,
      branch: 'main',
      run_id: 7,
      tool: 'read_file',
      phase: 'end',
      args_summary: 'a',
    });
    expect(state.liveTools.map((t) => [t.run_id, t.tool, t.phase])).toEqual([
      [7, 'read_file', 'end'],
      [8, 'grep', 'start'],
    ]);
  });

  it.each([
    ['task_done', 'done'],
    ['task_failed', 'failed'],
    ['task_cancelled', 'cancelled'],
  ] as const)('%s：终态横幅 + refetch', (type, expected) => {
    const next = reduceTaskDetail(base(), { type, task_id: taskId, branch: 'main' } as SseEvent);
    expect(next.terminal).toBe(expected);
    expect(next.refetchRequested).toBe(true);
  });
});

describe('陈旧节点事件不得唤醒 pending（§9.1：节点事件只归约列归属 / 信号色）', () => {
  const pendingReason: PendingReason = {
    type: 'user_decision',
    stage: 'merge',
    node: 'execute',
    message: '等待审批合入',
    context: { kind: 'dirty_worktree' },
  };

  it('pending 任务收到 node_started 后仍为 pending，且 pending 游标不被置 active', () => {
    const next = reduceBoard(
      emptyBoardState([
        task({
          status: 'pending',
          pending_reason: pendingReason,
          branches: [cursor({ status: 'pending', pending_reason: pendingReason })],
        }),
      ]),
      {
        type: 'node_started',
        task_id: 't1',
        branch: 'main',
        stage: 'merge',
        node: 'execute',
        attempt: 1,
        run_id: 42,
      } as SseEvent,
    );
    expect(next.tasks[0].status).toBe('pending');
    expect(next.tasks[0].branches[0].status).toBe('pending');
  });
});
