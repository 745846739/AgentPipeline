/**
 * 现场页签批量装载的**增量拉取与重试**（106 读路径诊断：全量载荷 1.76 MB、公网链路
 * 8.5–10.9 秒，是「查看运行中任务卡在查询」的主因）。
 *
 * 钉三条外部行为：
 * - 批量请求只带缓存里还没有的轮（`runIds: pending`），已有的不重发；
 * - 批量请求失败时，超时/网络类（`ApiError` status 0）**重试一次**再谈降级；
 * - 4xx/5xx 是确定性答案，不付重试，直接落到逐条降级。
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ConversationSummary, NodeConversation } from '../api/types';

const mocks = vi.hoisted(() => {
  class ApiError extends Error {
    readonly status: number;
    readonly kind: string | undefined;
    constructor(status: number, message: string, kind?: string) {
      super(message);
      this.name = 'ApiError';
      this.status = status;
      this.kind = kind;
    }
  }
  return {
    ApiError,
    getConversations: vi.fn(),
    getConversation: vi.fn(),
  };
});

vi.mock('../api/client', () => ({
  ApiError: mocks.ApiError,
  getConversations: mocks.getConversations,
  getConversation: mocks.getConversation,
  getTask: vi.fn(),
  getFlow: vi.fn(),
  getCommands: vi.fn(async () => []),
  getCommandOutput: vi.fn(async () => ''),
  getTaskFile: vi.fn(),
  modelOverrideTask: vi.fn(),
  reviewTask: vi.fn(),
  splitTask: vi.fn(),
}));

vi.mock('../realtime/connection', () => ({
  StreamManager: class {
    sync = vi.fn();
    stopAll = vi.fn();
    dispose = vi.fn();
  },
}));

const { taskDetail } = await import('./taskDetail.svelte');

function summary(runId: number): ConversationSummary {
  return {
    run_id: runId,
    stage: 'test',
    node: 'execute',
    attempt: 1,
    agent_type: 'main',
    parent_run_id: null,
    prompt_tokens: 10,
    completion_tokens: 5,
    status: 'success',
    archived_at: null,
  };
}

function conversation(runId: number): NodeConversation {
  return {
    ...summary(runId),
    id: runId,
    task_id: 'task-1',
    messages_json: [{ role: 'user', content: `第 ${runId} 轮正文` }],
    metadata_json: null,
    system_prompt: null,
    user_prompt: null,
    reasoning: null,
    created_at: '2026-10-06T00:00:00Z',
  };
}

describe('loadAllConversations（现场页签批量装载）', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    taskDetail.id = 'task-1';
    taskDetail.state = {
      ...taskDetail.state,
      conversations: [summary(1), summary(2), summary(3)],
    };
    taskDetail.conversationsFull = {};
  });

  afterEach(() => {
    taskDetail.conversationsFull = {};
  });

  it('只拉缓存里没有的轮：已有 run 1 时，runIds 只带 [2, 3]', async () => {
    taskDetail.conversationsFull = { 1: conversation(1) };
    mocks.getConversations.mockResolvedValue([conversation(2), conversation(3)]);

    await taskDetail.loadAllConversations();

    expect(mocks.getConversations).toHaveBeenCalledOnce();
    expect(mocks.getConversations).toHaveBeenCalledWith('task-1', {
      includeMessages: true,
      runIds: [2, 3],
    });
    expect(mocks.getConversation).not.toHaveBeenCalled();
    expect(Object.keys(taskDetail.conversationsFull).sort()).toEqual(['1', '2', '3']);
  });

  it('超时（status 0）重试一次后成功：不落逐条降级', async () => {
    const timeout = new mocks.ApiError(0, '请求超时（30 秒没有回应）。', 'request_timeout');
    mocks.getConversations
      .mockRejectedValueOnce(timeout)
      .mockResolvedValue([conversation(1), conversation(2), conversation(3)]);

    await taskDetail.loadAllConversations();

    expect(mocks.getConversations).toHaveBeenCalledTimes(2);
    expect(mocks.getConversation).not.toHaveBeenCalled();
    expect(Object.keys(taskDetail.conversationsFull).sort()).toEqual(['1', '2', '3']);
  });

  it('5xx 不付重试：批量只打一发，直接降级到逐条', async () => {
    mocks.getConversations.mockRejectedValue(new mocks.ApiError(500, '炸了'));
    mocks.getConversation.mockImplementation(async (_id: string, runId: number) =>
      conversation(runId),
    );

    await taskDetail.loadAllConversations();

    expect(mocks.getConversations).toHaveBeenCalledOnce();
    expect(mocks.getConversation).toHaveBeenCalledTimes(3);
    expect(Object.keys(taskDetail.conversationsFull).sort()).toEqual(['1', '2', '3']);
  });
});
