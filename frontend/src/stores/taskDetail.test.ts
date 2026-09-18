/**
 * 任务详情 store 的装载语义（票 01 / R2-01）。
 *
 * 钉的是「失败之后屏上还剩什么」这条外部行为：一次用户可见的加载失败，
 * 不能在 `state.task` 里留下上一个任务——渲染层的 `{#if task}` 靠它短路，
 * 留着就等于把六颗拍板按钮挂到一个坏 id 上。
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Task } from '../api/types';
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
  getConversation: vi.fn(),
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
  mocks.getCommands.mockReset().mockResolvedValue([]);
  mocks.splitTask.mockReset();
  mocks.modelOverrideTask.mockReset();
  mocks.sync.mockReset();
  armLoadedA();
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
