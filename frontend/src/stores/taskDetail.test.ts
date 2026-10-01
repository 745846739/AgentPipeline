/**
 * 任务详情 store 的装载语义（票 01 / R2-01）。
 *
 * 钉的是「失败之后屏上还剩什么」这条外部行为：一次用户可见的加载失败，
 * 不能在 `state.task` 里留下上一个任务——渲染层的 `{#if task}` 靠它短路，
 * 留着就等于把六颗拍板按钮挂到一个坏 id 上。
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ConversationSummary, NodeCommand, NodeConversation, Task } from '../api/types';
import { emptyTaskDetailState } from '../realtime/reduce';
import { taskDetail } from './taskDetail.svelte';

const mocks = vi.hoisted(() => {
  class ApiError extends Error {
    constructor(
      readonly status: number,
      message: string,
    ) {
      super(message);
    }
  }
  return {
    ApiError,
    getTask: vi.fn(),
    getFlow: vi.fn(),
    getConversations: vi.fn(),
    getConversation: vi.fn(),
    getCommands: vi.fn(),
    splitTask: vi.fn(),
    modelOverrideTask: vi.fn(),
    sync: vi.fn(),
  };
});

vi.mock('../api/client', () => ({
  ApiError: mocks.ApiError,
  getTask: mocks.getTask,
  getFlow: mocks.getFlow,
  getConversations: mocks.getConversations,
  getCommands: mocks.getCommands,
  splitTask: mocks.splitTask,
  modelOverrideTask: mocks.modelOverrideTask,
  // 本用例走不到，但被测模块要能 import
  getCommandOutput: vi.fn(),
  getConversation: mocks.getConversation,
  getTaskFile: vi.fn(),
  reviewTask: vi.fn(),
}));

vi.mock('../realtime/connection', () => ({
  StreamManager: class {
    sync = mocks.sync;
    stopAll = vi.fn();
    dispose = vi.fn();
  },
}));

function task(id: string, title: string): Task {  return {
    id,
    project_id: 'p1',
    title,
    description: '',
    status: 'running',
    current_stage: 'develop',
    current_node: 'execute',
    validate_attempts: 0,
    pending_reason: null,
    worktree_path: null,
    branch_name: null,
    stewardship: null,
    total_tokens: 10,
    total_calls: 1,
    review_mode: 'agent',
    model_override: null,
    archived_at: null,
    stalled: false,
    executor_owner: null,
    created_at: '2026-09-16T00:00:00Z',
    updated_at: '2026-09-16T00:10:00Z',
  };
}

/** 把 store 摆到「A 已经完整装载过」那一态。 */
function armLoadedA(): void {
  taskDetail.id = 'A';
  taskDetail.error = null;
  taskDetail.errorStatus = null;
  taskDetail.loading = false;
  taskDetail.state = emptyTaskDetailState({
    task: task('A', '实现用户登录接口'),
    cursors: [
      {
        cursor_id: 'c1',
        branch: 'main',
        stage: 'develop',
        node: 'execute',
        status: 'active',
        validate_attempts: 0,
        skipped_to_join: false,
        pending_reason: null,
      },
    ],
    allowedActions: [
      { action: 'approve', kind: 'side_effect', label: '合入', cursor_id: 'c1' },
      { action: 'return', kind: 'side_effect', label: '返回修改', cursor_id: 'c1' },
    ],
    commands: [],
    conversations: [],
  });
  taskDetail.files = { 'a.md': { path: 'a.md', content: 'A', error: null, status: 200 } };
  taskDetail.actionError = null;
}

beforeEach(() => {
  mocks.getTask.mockReset();
  mocks.getFlow.mockReset().mockResolvedValue({ transitions: [] });
  mocks.getConversations.mockReset().mockResolvedValue([]);
  mocks.getConversation.mockReset().mockResolvedValue(null);
  mocks.getCommands.mockReset().mockResolvedValue([]);
  mocks.splitTask.mockReset();
  mocks.modelOverrideTask.mockReset();
  mocks.sync.mockReset();
  armLoadedA();
  // 单例 store 的普通字段不随用例重置：上一条用例若把现场页签报成在屏（决策 365），
  // 后续用例的静默 refetch 会平白多拉一次 commands。显式收回离屏态。
  taskDetail.setSceneVisible(false);
});

afterEach(() => {
  vi.clearAllMocks();
});

describe('详情页装载失败不留上一个任务（票 01 / R2-01）', () => {
  it('切到一个不存在的 id：上一个任务的正文、游标与动作集一起收走', async () => {
    mocks.getTask.mockRejectedValue(new mocks.ApiError(404, '任务不存在：B'));

    await taskDetail.load('B');

    expect(taskDetail.state.task).toBeNull();
    expect(taskDetail.state.cursors).toEqual([]);
    expect(taskDetail.state.allowedActions).toEqual([]);
    expect(taskDetail.state.transitions).toEqual([]);
    expect(taskDetail.state.conversations).toEqual([]);
    expect(taskDetail.state.commands).toEqual([]);
    expect(taskDetail.errorStatus).toBe(404);
    expect(taskDetail.error).toBe('任务不存在：B');
  });

  it('换 id 时先收走上一份再发请求：新地址下不会先闪旧正文', async () => {
    let seenDuringFlight: string | null = 'sentinel';
    mocks.getTask.mockImplementation(async () => {
      seenDuringFlight = taskDetail.state.task?.title ?? null;
      throw new mocks.ApiError(404, '任务不存在：B');
    });

    await taskDetail.load('B');

    expect(seenDuringFlight).toBeNull();
  });

  it('旧任务的流一并收掉（事件回调不按 id 过滤）', async () => {
    mocks.getTask.mockRejectedValue(new mocks.ApiError(500, 'boom'));

    await taskDetail.load('B');

    expect(mocks.sync).toHaveBeenCalledWith([]);
  });

  it('首个请求成功、随后的 flow 失败：半截的新任务也要收（不混上一份的 transitions）', async () => {
    mocks.getTask.mockResolvedValue({
      task: task('B', '另一个任务'),
      cursors: [],
      allowed_actions: [],
    });
    mocks.getFlow.mockRejectedValue(new mocks.ApiError(500, 'boom'));

    await taskDetail.load('B');

    expect(taskDetail.state.task).toBeNull();
    expect(taskDetail.error).toBe('boom');
  });

  it('成功路径照旧：任务与动作集就位，流接到这个 id 上', async () => {
    mocks.getTask.mockResolvedValue({
      task: task('B', '另一个任务'),
      cursors: [],
      allowed_actions: [],
    });

    await taskDetail.load('B');

    expect(taskDetail.state.task?.id).toBe('B');
    expect(taskDetail.error).toBeNull();
    expect(mocks.sync).toHaveBeenCalledWith(['B']);
  });
});

describe('现场页签的批量装载（决策 361，票 03）', () => {
  /** 摘要态的那一份（`/conversations` 缺省分支）。 */
  const summary = (runId: number): ConversationSummary => ({
    run_id: runId,
    stage: 'develop',
    node: 'execute',
    attempt: 1,
    agent_type: 'main',
    parent_run_id: null,
    prompt_tokens: 1,
    completion_tokens: 1,
    status: 'success',
    archived_at: null,
  });
  /** 完整会话（`?include_messages=true` 的元素，与单条读法同形）。 */
  const conversation = (runId: number): NodeConversation => ({
    id: runId,
    task_id: 'A',
    run_id: runId,
    stage: 'develop',
    node: 'execute',
    attempt: 1,
    agent_type: 'main',
    parent_run_id: null,
    messages_json: [{ role: 'user', content: `第 ${runId} 轮` }],
    metadata_json: null,
    prompt_tokens: 1,
    completion_tokens: 1,
    system_prompt: null,
    user_prompt: null,
    reasoning: null,
    created_at: '2026-09-16T00:00:00Z',
  });
  const armConversations = (summaries: ConversationSummary[]): void => {
    taskDetail.id = 'A';
    taskDetail.state = emptyTaskDetailState({ conversations: summaries });
    taskDetail.conversationsFull = {};
  };

  it('一次请求取回全部轮（不再是 N+1），并按 run_id 归位', async () => {
    armConversations([summary(1), summary(2), summary(3)]);
    mocks.getConversations.mockResolvedValue([conversation(1), conversation(2), conversation(3)]);

    await taskDetail.loadAllConversations();

    expect(mocks.getConversations).toHaveBeenCalledTimes(1);
    expect(mocks.getConversations).toHaveBeenCalledWith('A', { includeMessages: true });
    expect(mocks.getConversation).not.toHaveBeenCalled();
    expect(Object.keys(taskDetail.conversationsFull).sort()).toEqual(['1', '2', '3']);
    expect(taskDetail.conversationsFull[2].messages_json[0].content).toBe('第 2 轮');
  });

  it('已经装载过的轮不再进这次请求的待办', async () => {
    armConversations([summary(1)]);
    taskDetail.conversationsFull = { 1: conversation(1) };

    await taskDetail.loadAllConversations();

    expect(mocks.getConversations).not.toHaveBeenCalled();
  });

  it('批量读法不可用（老后端 / 中间层剥了 query）→ 降级逐条，功能不被关死', async () => {
    armConversations([summary(1), summary(2)]);
    mocks.getConversations.mockRejectedValue(new mocks.ApiError(400, '未知参数'));
    mocks.getConversation.mockImplementation(async (_id: string, runId: number) =>
      conversation(runId),
    );

    await taskDetail.loadAllConversations();

    expect(mocks.getConversation).toHaveBeenCalledTimes(2);
    expect(Object.keys(taskDetail.conversationsFull).sort()).toEqual(['1', '2']);
  });
});

describe('commands 移出首屏关键路径（决策 361，票 02）', () => {
  const command = (id: number): NodeCommand => ({
    id,
    task_id: 'A',
    run_id: null,
    stage: 'develop',
    node: 'execute',
    source: 'agent',
    command: 'npm test',
    original_command: null,
    cwd: '/tmp',
    exit_code: 0,
    stdout_path: null,
    stdout_preview: 'PASS',
    stderr_preview: null,
    duration_ms: 12,
    started_at: '2026-09-16T00:00:00Z',
    finished_at: '2026-09-16T00:00:01Z',
  });

  /** 让后台补拉那一条链跑完（它是 fire-and-forget，不在 `load()` 的 await 链上）。 */
  const settle = async (): Promise<void> => {
    for (let i = 0; i < 4; i++) await Promise.resolve();
  };

  /** 悬挂的 commands：resolve 由用例自己放行。 */
  function hangingCommands(): { release: () => void } {
    let release = (): void => undefined;
    mocks.getCommands.mockImplementation(
      () => new Promise<NodeCommand[]>((resolve) => (release = () => resolve([command(1)]))),
    );
    return { release: () => release() };
  }

  it('首屏不等 commands：它挂住不返回，时间线要的数据照样就位', async () => {
    const { release } = hangingCommands();
    mocks.getTask.mockResolvedValue({
      task: task('A', 'A'),
      cursors: [],
      allowed_actions: [],
    });
    mocks.getFlow.mockResolvedValue({
      transitions: [{ id: 1, to_stage: 'develop', to_node: 'execute' }],
    });

    await taskDetail.load('A');

    // `/flow` 那份已经落位（时间线据此渲染），而 commands 那条路还挂着
    expect(taskDetail.state.transitions).toHaveLength(1);
    expect(taskDetail.state.commands).toEqual([]);
    expect(taskDetail.loading).toBe(false);

    // 放行之后照样并进来（补拉不是「发出去就不管」）
    release();
    await settle();
    expect(taskDetail.state.commands).toHaveLength(1);
  });

  it('后台补拉失败挂到错误位（报错不静默），但不翻掉已经就位的首屏内容', async () => {
    mocks.getTask.mockResolvedValue({
      task: task('A', 'A'),
      cursors: [],
      allowed_actions: [],
    });
    mocks.getFlow.mockResolvedValue({ transitions: [] });
    mocks.getCommands.mockRejectedValue(new mocks.ApiError(500, 'commands 读不到'));

    await taskDetail.load('A');
    await settle();

    expect(taskDetail.error).toBe('commands 读不到');
    expect(taskDetail.state.task?.id).toBe('A');
    expect(taskDetail.state.cursors).toEqual([]);
  });

  it('静默 refetch 不再拉 commands，也不清已经到的那一份（现场页签不在屏时）', async () => {
    mocks.getTask.mockResolvedValue({
      task: task('A', 'A'),
      cursors: [],
      allowed_actions: [],
    });
    mocks.getFlow.mockResolvedValue({ transitions: [] });
    mocks.getCommands.mockResolvedValue([command(7)]);

    await taskDetail.load('A');
    expect(mocks.getCommands).toHaveBeenCalledTimes(1);
    await settle();
    expect(taskDetail.state.commands).toHaveLength(1);

    await taskDetail.load('A', true);

    expect(mocks.getCommands).toHaveBeenCalledTimes(1);
    expect(taskDetail.state.commands).toHaveLength(1);
  });

  /**
   * 决策 365：361 把 commands 的保鲜托付给「SSE 的 `command_started` / `command_finished`」，
   * 而服务端从不发这两类事件——于是开屏后跑的命令再也到不了界面（E2E-⑦ 用例① 实测红）。
   * 改法是**现场页签在屏时**重拉：进场当场一次，在屏期间静默 refetch 也跟着一次。
   */
  it('现场页签在屏：进场当场补拉一次，此后静默 refetch 也重拉（决策 365）', async () => {
    mocks.getTask.mockResolvedValue({
      task: task('A', 'A'),
      cursors: [],
      allowed_actions: [],
    });
    mocks.getFlow.mockResolvedValue({ transitions: [] });
    mocks.getCommands.mockResolvedValue([command(7)]);

    await taskDetail.load('A');
    await settle();
    expect(mocks.getCommands).toHaveBeenCalledTimes(1);

    // 进场：当场补拉（不能等下一次 refetch——任务可能已经没有后续事件了）
    taskDetail.setSceneVisible(true);
    await settle();
    expect(mocks.getCommands).toHaveBeenCalledTimes(2);

    // 在屏期间：静默 refetch 重拉（保鲜）
    await taskDetail.load('A', true);
    await settle();
    expect(mocks.getCommands).toHaveBeenCalledTimes(3);

    // 离屏之后回到旧口径：静默 refetch 不再拉
    taskDetail.setSceneVisible(false);
    await taskDetail.load('A', true);
    await settle();
    expect(mocks.getCommands).toHaveBeenCalledTimes(3);
  });

  it('重复报同一个可见性不重复拉（页面 effect 会重复跑，必须是空操作）（决策 365）', async () => {
    mocks.getTask.mockResolvedValue({
      task: task('A', 'A'),
      cursors: [],
      allowed_actions: [],
    });
    mocks.getFlow.mockResolvedValue({ transitions: [] });
    mocks.getCommands.mockResolvedValue([command(7)]);

    await taskDetail.load('A');
    await settle();
    expect(mocks.getCommands).toHaveBeenCalledTimes(1);

    taskDetail.setSceneVisible(true);
    await settle();
    taskDetail.setSceneVisible(true);
    await settle();
    expect(mocks.getCommands).toHaveBeenCalledTimes(2);
  });
});

describe('对话框动作的提交中态与重入护栏（票 03 / R2-03）', () => {
  /** 一个悬挂的 split 请求：resolve 由用例自己放行。 */
  function hangingSplit(): { release: () => void } {
    let release = (): void => undefined;
    mocks.splitTask.mockImplementation(
      () => new Promise<void>((resolve) => (release = resolve)),
    );
    return { release: () => release() };
  }

  it('in-flight 期间 busyKey 非空（对话框的 submitting 才真的生效）', async () => {
    const { release } = hangingSplit();
    const pending = taskDetail.submitSplit([{ title: '子任务' }]);

    expect(taskDetail.busyKey).toBe('split_task:');
    // 对话框拿的正是这一个判据
    expect(taskDetail.busyKey !== null).toBe(true);

    release();
    await pending;
  });

  it('in-flight 期间第二次调用不产生第二个请求（双击不会建两套子任务）', async () => {
    const { release } = hangingSplit();
    const first = taskDetail.submitSplit([{ title: '子任务' }]);
    const second = taskDetail.submitSplit([{ title: '子任务' }]);

    expect(mocks.splitTask).toHaveBeenCalledTimes(1);

    release();
    await Promise.all([first, second]);
  });

  it('失败：错误可读、busy 复位（按钮不卡在转圈上）', async () => {
    mocks.splitTask.mockRejectedValue(new mocks.ApiError(500, '拆不开'));

    await expect(taskDetail.submitSplit([{ title: '子任务' }])).rejects.toThrow('拆不开');

    expect(taskDetail.actionError).toBe('拆不开');
    expect(taskDetail.busyKey).toBeNull();
  });

  it('换模型与拆分互不干扰：各自进自己的提交中态', async () => {
    mocks.modelOverrideTask.mockImplementation(() => new Promise<void>(() => undefined));
    void taskDetail.submitModelOverride('p-1');

    expect(taskDetail.busyKey).toBe('model_override:');
    expect(mocks.modelOverrideTask).toHaveBeenCalledTimes(1);
  });
});

describe('落地即清（决策 362②）：正文到手后收走该 run 的直播增量', () => {
  const conversation = (runId: number): NodeConversation => ({
    id: runId,
    task_id: 'A',
    run_id: runId,
    stage: 'develop',
    node: 'execute',
    attempt: 1,
    agent_type: 'main',
    parent_run_id: null,
    messages_json: [{ role: 'assistant', content: `第 ${runId} 轮` }],
    metadata_json: null,
    prompt_tokens: 1,
    completion_tokens: 1,
    system_prompt: null,
    user_prompt: null,
    reasoning: null,
    created_at: '2026-09-16T00:00:00Z',
  });
  const summary = (runId: number): ConversationSummary => ({
    run_id: runId,
    stage: 'develop',
    node: 'execute',
    attempt: 1,
    agent_type: 'main',
    parent_run_id: null,
    prompt_tokens: 1,
    completion_tokens: 1,
    status: 'running',
    archived_at: null,
  });
  const liveDelta = (runId: number, text: string, seq: number) => ({
    run_id: runId,
    agent_type: 'main',
    role: 'assistant',
    channel: 'content' as const,
    text,
    seq,
  });
  const liveTool = (runId: number, seq: number) => ({
    run_id: runId,
    tool: 'read_file',
    phase: 'end' as const,
    args_summary: 'a',
    args: '',
    result: 'r',
    seq,
  });
  /** 两条 run 都在冒，其中 run 1 被上限丢弃过（标记与增量一起受落地清理管）。 */
  const armLive = (): void => {
    taskDetail.id = 'A';
    taskDetail.state = emptyTaskDetailState({
      conversations: [summary(1), summary(2)],
      liveDeltas: [liveDelta(1, '甲在冒', 0), liveDelta(2, '乙在冒', 1)],
      liveTools: [liveTool(1, 2)],
      liveDroppedRuns: { 1: true },
    });
    taskDetail.conversationsFull = {};
  };

  it('某 run 落地：它的增量消失，在飞的另一 run 不受影响', async () => {
    armLive();
    mocks.getConversation.mockResolvedValue(conversation(1));

    await taskDetail.loadConversation(1);

    expect(taskDetail.state.liveDeltas.map((d) => d.run_id)).toEqual([2]);
    expect(taskDetail.state.liveTools).toEqual([]);
  });

  it('批量装载落地同样清（一次拿回的那条路）', async () => {
    armLive();
    mocks.getConversations.mockResolvedValue([conversation(1), conversation(2)]);

    await taskDetail.loadAllConversations();

    expect(taskDetail.state.liveDeltas).toEqual([]);
    expect(taskDetail.state.liveTools).toEqual([]);
  });

  it('只清已落地的 run：正文还没到的那一轮照旧在冒', async () => {
    armLive();
    // 只回 run 2 的正文（run 1 的正文没到）
    mocks.getConversations.mockResolvedValue([conversation(2)]);

    await taskDetail.loadAllConversations();

    expect(taskDetail.state.liveDeltas.map((d) => d.run_id)).toEqual([1]);
    expect(taskDetail.state.liveTools.map((t) => t.run_id)).toEqual([1]);
    expect(taskDetail.state.liveDroppedRuns).toEqual({ 1: true });
  });

  it('增量全被上限丢光、只剩标记的 run 落地后：标记也随之收走', async () => {
    // 两个数组里一条它的增量都不剩（全被环形缓冲丢光），只剩截断标记——只看数组会提前
    // 返回，标记留在库里，等它落地后那一轮会永久摆一行不该有的「更早的增量已省略」。
    taskDetail.id = 'A';
    taskDetail.state = emptyTaskDetailState({
      conversations: [summary(1)],
      liveDroppedRuns: { 1: true },
    });
    taskDetail.conversationsFull = {};
    mocks.getConversation.mockResolvedValue(conversation(1));

    await taskDetail.loadConversation(1);

    expect(taskDetail.state.liveDroppedRuns).toEqual({});
  });

  it('丢弃标记随落地一起收走（已落地的轮不再摆省略行）', async () => {
    armLive();
    mocks.getConversation.mockResolvedValue(conversation(1));

    await taskDetail.loadConversation(1);

    expect(taskDetail.state.liveDroppedRuns).toEqual({});
  });

  it('正文已到手却又晚到一条增量（竞态）→ 当场收走', () => {
    armLive();
    taskDetail.conversationsFull = { 2: conversation(2) };

    taskDetail.handleEvent({
      type: 'conversation_delta',
      task_id: 'A',
      branch: 'main',
      run_id: 2,
      agent_type: 'main',
      role: 'assistant',
      text: '晚到的',
      prompt_tokens: 0,
      completion_tokens: 1,
    });

    expect(taskDetail.state.liveDeltas.map((d) => d.run_id)).toEqual([1]);
    expect(taskDetail.state.liveDroppedRuns).toEqual({ 1: true });
  });

  it('没有任何正文落地时不扫也不动（在飞的轮照旧）', () => {
    armLive();

    taskDetail.handleEvent({
      type: 'conversation_delta',
      task_id: 'A',
      branch: 'main',
      run_id: 2,
      agent_type: 'main',
      role: 'assistant',
      text: '继续在冒',
      prompt_tokens: 0,
      completion_tokens: 1,
    });

    expect(taskDetail.state.liveDeltas.map((d) => d.run_id)).toEqual([1, 2, 2]);
    expect(taskDetail.state.liveDroppedRuns).toEqual({ 1: true });
  });
});
