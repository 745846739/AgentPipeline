/**
 * 看板 store 的装载语义（票 10 / R2-12）。
 *
 * 钉的是「新建任务之后跳哪一个」这条外部行为：`createTask` 必须返回**服务端建的那一个**，
 * 而不是「当前项目任务列表里的第一个」——两个可以不是同一个（对话框选的项目可以不同于
 * 看板当前在看的项目），而跳到别的项目的任务上，用户会以为建错了或没建成。
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { AllowedAction, Task, TaskListItem } from '../api/types';
import { actionKey } from '../lib/actions';

const mocks = vi.hoisted(() => ({
  createTask: vi.fn(),
  listTasks: vi.fn(),
  listProjects: vi.fn(),
  getTask: vi.fn(),
  submitAllowedAction: vi.fn(),
}));

vi.mock('../api/client', () => ({
  createTask: mocks.createTask,
  listTasks: mocks.listTasks,
  listProjects: mocks.listProjects,
  getTask: mocks.getTask,
}));

// 动作提交走的是 lib/actionSubmit（不是 api/client）——board 的忙态测试要能把它拖住，
// 才好在「提交还没回来」的窗口里断言 actionBusy 的形状。
vi.mock('../lib/actionSubmit', () => ({
  submitAllowedAction: mocks.submitAllowedAction,
}));

vi.mock('../realtime/connection', () => ({
  StreamManager: class {
    sync = vi.fn();
    stopAll = vi.fn();
    dispose = vi.fn();
  },
}));

const { board } = await import('./board.svelte');

function item(id: string, projectId: string, title: string): TaskListItem {
  return {
    id,
    project_id: projectId,
    title,
    description: '',
    status: 'running',
    current_stage: 'init',
    current_node: 'execute',
    validate_attempts: 0,
    pending_reason: null,
    worktree_path: null,
    branch_name: null,
    stewardship: null,
    total_tokens: 0,
    total_calls: 0,
    review_mode: 'agent',
    model_override: null,
    archived_at: null,
    stalled: false,
    executor_owner: null,
    created_at: '2026-09-18T00:00:00Z',
    updated_at: '2026-09-18T00:10:00Z',
    branches: [],
    blocks: [],
  };
}

/** 服务端建出来的那一个（`POST /tasks` 的 `{task}`）。 */
function created(id: string, projectId: string): Task {
  const { branches, blocks, ...task } = item(id, projectId, '新任务');
  void branches;
  void blocks;
  return task;
}

beforeEach(() => {
  mocks.createTask.mockReset();
  mocks.listTasks.mockReset().mockResolvedValue([]);
  mocks.listProjects.mockReset().mockResolvedValue([]);
  mocks.getTask.mockReset();
  board.tasks = [];
  board.projects = [];
  board.projectId = 'A';
});

afterEach(() => {
  vi.clearAllMocks();
});

describe('看板新建任务（票 10 / R2-12）', () => {
  it('对话框选了别的项目：返回的是那一个任务的 id，不是当前项目的第一条', async () => {
    // 看板当前在看 A，A 里已经有一条任务
    mocks.listTasks.mockResolvedValue([item('A1', 'A', 'A 的任务')]);
    mocks.createTask.mockResolvedValue({ task: created('B1', 'B') });

    const task = await board.createTask({ project_id: 'B', title: '新任务' });

    expect(mocks.createTask).toHaveBeenCalledWith({ project_id: 'B', title: '新任务' });
    expect(task?.id).toBe('B1');
    expect(task?.project_id).toBe('B');
  });

  it('看板当前项目一条任务都没有：仍然返回新建的那一个（此前返回 null → 哪儿都不去）', async () => {
    mocks.listTasks.mockResolvedValue([]);
    mocks.createTask.mockResolvedValue({ task: created('B2', 'B') });

    const task = await board.createTask({ project_id: 'B', title: '新任务' });

    expect(task?.id).toBe('B2');
  });

  it('服务端没回 task：返回 null，由调用方明说（不静默留在原地）', async () => {
    mocks.createTask.mockResolvedValue({ task: undefined });

    await expect(board.createTask({ project_id: 'A', title: '新任务' })).resolves.toBeNull();
  });

  it('创建失败：错误上到 store 并原样抛出', async () => {
    mocks.createTask.mockRejectedValue(new Error('项目不存在'));

    await expect(board.createTask({ project_id: 'A', title: '新任务' })).rejects.toThrow('项目不存在');
    expect(board.error).toBe('项目不存在');
  });
});

describe('看板动作的身份与忙态（票 05：与 actionKey 同一把尺子）', () => {
  const cont: AllowedAction = {
    action: 'continue',
    kind: 'resume',
    label: '补充信息并继续',
    cursor_id: 'c-main',
  };

  beforeEach(() => {
    board.actionBusy = null;
    board.actionError = null;
    mocks.submitAllowedAction.mockReset();
  });

  /** 把提交拖在半路，造出「还在忙」的观察窗口；调用方负责 release() 后 await。 */
  function hangSubmit(): { release: () => void; pending: Promise<void> } {
    let release!: () => void;
    mocks.submitAllowedAction.mockImplementation(
      () =>
        new Promise<void>((resolve) => {
          release = resolve;
        }),
    );
    const pending = board.handleTaskAction('t1', cont, { cursorId: 'c-main' });
    return { release, pending };
  }

  it('提交中 actionBusy 存的是四段式 actionKey（不再是两段 taskId:action）', async () => {
    const { release, pending } = hangSubmit();
    // busy 在第一个 await 之前同步置上——此刻提交还悬着
    expect(board.actionBusy).toBe(actionKey(cont, 'c-main'));
    expect(board.actionBusy).toBe('continue:c-main::');
    release();
    await pending;
    expect(board.actionBusy).toBeNull();
  });

  it('同名动作、不同游标 → 不是同一个忙（§12.3 那一行要守的语义）', async () => {
    const { release, pending } = hangSubmit();
    const other: AllowedAction = { ...cont, cursor_id: 'c-other' };
    expect(board.actionBusy === actionKey(other, 'c-other')).toBe(false);
    expect(board.actionBusy === actionKey(cont, 'c-main')).toBe(true);
    release();
    await pending;
  });
});
