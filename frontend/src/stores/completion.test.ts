import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { completion } from './completion.svelte';
import { setApiBase } from '../api/config';

/**
 * 完成横幅状态机（票 08）：
 * - done 时弹出（且同一次完成不重复弹）；
 * - 收下后消失；
 * - diff 摘要不可得时不假装有数字（stats 保持 null，不画 0）。
 */
const DIFF = [
  'diff --git a/src/lib.js b/src/lib.js',
  '--- a/src/lib.js',
  '+++ b/src/lib.js',
  '@@ -1,3 +1,4 @@',
  '+const a = 1;',
  '+const b = 2;',
  '-const old = 0;',
  ' // ctx',
  '',
].join('\n');

function stubDiff(body: string | null, status = 200): void {
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => {
      if (body === null) return new Response('not found', { status: 404 });
      return new Response(body, { status, headers: { 'Content-Type': 'text/plain' } });
    }),
  );
}

beforeEach(() => {
  setApiBase(null);
  completion.reset();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('CompletionStore（票 08）', () => {
  it('任务 running → done 时弹出横幅', () => {
    completion.observeAll([{ id: 't1', status: 'running', title: '实现登录' }]);
    expect(completion.notice).toBeNull();

    completion.observeAll([{ id: 't1', status: 'done', title: '实现登录' }]);
    expect(completion.notice).toMatchObject({ taskId: 't1', title: '实现登录', stats: null });
  });

  it('首屏已是 done（刷新后）不弹：没有迁移就没有横幅', () => {
    completion.observeAll([{ id: 't1', status: 'done', title: '实现登录' }]);
    expect(completion.notice).toBeNull();
  });

  it('同一次 done 只弹一次：SSE 与轮询重复观测不重复入幡', () => {
    completion.observeAll([{ id: 't1', status: 'running', title: '实现登录' }]);
    completion.note('t1', 'done', '实现登录');
    completion.observeAll([{ id: 't1', status: 'done', title: '实现登录' }]);
    expect(completion.notice?.taskId).toBe('t1');

    // 收下后再次观测同一个已完成任务：不重新弹出
    completion.dismiss();
    expect(completion.notice).toBeNull();
    completion.observeAll([{ id: 't1', status: 'done', title: '实现登录' }]);
    expect(completion.notice).toBeNull();
  });

  it('failed / cancelled 不弹（既有终态 UI 保留）', () => {
    completion.observeAll([{ id: 't1', status: 'running', title: '实现登录' }]);
    completion.note('t1', 'failed', '实现登录');
    completion.note('t1', 'cancelled', '实现登录');
    expect(completion.notice).toBeNull();
  });

  it('点「收下」关闭；队列中的第二个完成随即呈上', () => {
    completion.observeAll([
      { id: 't1', status: 'running', title: '甲' },
      { id: 't2', status: 'running', title: '乙' },
    ]);
    completion.observeAll([
      { id: 't1', status: 'done', title: '甲' },
      { id: 't2', status: 'done', title: '乙' },
    ]);
    expect(completion.notice?.taskId).toBe('t1');

    completion.dismiss();
    expect(completion.notice?.taskId).toBe('t2');

    completion.dismiss();
    expect(completion.notice).toBeNull();
  });

  it('diff 可得时摘要进入横幅（沿用前端 DiffStats 解析）', async () => {
    stubDiff(DIFF);
    completion.observeAll([{ id: 't1', status: 'running', title: '实现登录' }]);
    completion.observeAll([{ id: 't1', status: 'done', title: '实现登录' }]);

    await vi.waitFor(() => expect(completion.notice?.stats).not.toBeNull());
    expect(completion.notice?.stats?.insertions).toBe(2);
    expect(completion.notice?.stats?.deletions).toBe(1);
  });

  it('摘要不可得（404）时 stats 保持 null —— 绝不画 0 冒充真实值', async () => {
    stubDiff(null, 404);
    completion.observeAll([{ id: 't1', status: 'running', title: '实现登录' }]);
    completion.observeAll([{ id: 't1', status: 'done', title: '实现登录' }]);

    // 等一次异步 fetch 走完：仍是 null（而不是 0 / 空 DiffStats）
    await vi.waitFor(() => expect(vi.mocked(fetch)).toHaveBeenCalled());
    await Promise.resolve();
    expect(completion.notice?.stats).toBeNull();
  });
});
