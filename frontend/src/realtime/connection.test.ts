import { afterEach, describe, expect, it, vi } from 'vitest';
import { parseSseFrames, TaskStream } from './connection';

/**
 * SSE 连接层（决策 153②：fetch 流式读取，不用 EventSource；
 * design/frontend-design.md §9.1：visibilitychange 恢复时立即重连）。
 */

/** 模拟 fetch 的响应体：真实 fetch 会在 abort 时中断读取，所以这里按同样语义绑定 signal。 */
function openStream(signal: AbortSignal) {
  const encoder = new TextEncoder();
  let ctrl: ReadableStreamDefaultController<Uint8Array> | null = null;
  const abort = () => ctrl?.error(new DOMException('Aborted', 'AbortError'));
  if (signal.aborted) abort();
  else signal.addEventListener('abort', abort);
  const stream = new ReadableStream<Uint8Array>({
    start(c) {
      ctrl = c;
    },
  });
  return {
    stream,
    push: (text: string) => ctrl?.enqueue(encoder.encode(text)),
  };
}

describe('parseSseFrames', () => {
  it('只取 data 行并按空行分帧，残留缓冲留待下个 chunk', () => {
    const { events, rest } = parseSseFrames(
      'event: message\ndata: {"a":1}\n\ndata: {"b":2}\n\npartial',
    );
    expect(events).toEqual(['{"a":1}', '{"b":2}']);
    expect(rest).toBe('partial');
  });

  it('兼容 CRLF', () => {
    const { events } = parseSseFrames('data: {"a":1}\r\n\r\n');
    expect(events).toEqual(['{"a":1}']);
  });
});

describe('TaskStream 主动重连', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('reconnectNow 后立刻重开一条流，且重连后的流仍能收到事件', async () => {
    const opened: ReturnType<typeof openStream>[] = [];
    const fetchMock = vi.fn(async (_url: string, init?: RequestInit) => {
      const stream = openStream(init!.signal!);
      opened.push(stream);
      return new Response(stream.stream, {
        status: 200,
        headers: { 'content-type': 'text/event-stream' },
      });
    });
    vi.stubGlobal('fetch', fetchMock);

    const seen: string[] = [];
    const conn = new TaskStream('t1', {
      onEvent: (_id, event) => seen.push(event.type),
    });
    conn.start();
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));

    // visibilitychange 恢复：重置退避并立即重连
    conn.reconnectNow();
    // 修复前：AbortError 让 consume 循环 break，永远不会发起第二次请求
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2), { timeout: 2000 });

    opened[1].push('data: {"type":"stalled","task_id":"t1","branch":"main"}\n\n');
    await vi.waitFor(() => expect(seen).toContain('stalled'));

    conn.stop();
  });

  it('掉线之后接回来的那一次要对齐一次（断流期间的事件已经永久丢了）', async () => {
    // 第一次请求直接 500（模拟断流），第二次起成功
    let attempt = 0;
    const opened: ReturnType<typeof openStream>[] = [];
    const fetchMock = vi.fn(async (_url: string, init?: RequestInit) => {
      attempt += 1;
      if (attempt === 1) return new Response('nope', { status: 500 });
      const stream = openStream(init!.signal!);
      opened.push(stream);
      return new Response(stream.stream, {
        status: 200,
        headers: { 'content-type': 'text/event-stream' },
      });
    });
    vi.stubGlobal('fetch', fetchMock);

    const recalibrated: string[] = [];
    const statuses: string[] = [];
    const conn = new TaskStream(
      't1',
      {
        onEvent: () => {},
        onStatus: (_id, status) => statuses.push(status),
        onRecalibrate: (id) => recalibrated.push(id),
      },
      // 退避压到最小：这条用例只验「接回来之后对齐一次」，不验退避的时长
      { baseDelayMs: 1, maxDelayMs: 1 },
    );
    conn.start();

    await vi.waitFor(() => expect(recalibrated).toEqual(['t1']), { timeout: 3000 });
    expect(statuses, '顺序：先报断线，再报接回来，然后才对齐').toEqual([
      'connecting',
      'error',
      'connecting',
      'open',
    ]);
    // 只对齐一次：正常收事件的那段时间不该反复 refetch
    opened[0].push('data: {"type":"stalled","task_id":"t1","branch":"main"}\n\n');
    await vi.waitFor(() => expect(opened.length).toBe(1));
    expect(recalibrated).toEqual(['t1']);

    conn.stop();
  });
});

describe('TaskStream 路径覆盖（票 03：值班长流复用同一解析层）', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('缺省打任务流；给了 path 就打那条（工头流）', async () => {
    const urls: string[] = [];
    const fetchMock = vi.fn(async (url: string, init?: RequestInit) => {
      urls.push(String(url));
      const stream = openStream(init!.signal!);
      return new Response(stream.stream, {
        status: 200,
        headers: { 'content-type': 'text/event-stream' },
      });
    });
    vi.stubGlobal('fetch', fetchMock);

    const task = new TaskStream('t1', { onEvent: () => {} });
    task.start();
    await vi.waitFor(() => expect(urls).toHaveLength(1));
    task.stop();

    // 工头没有真实 task id（决策 182⑥）：路径必须完全由 path 决定，不能拼出 /tasks//stream
    const foreman = new TaskStream('', { onEvent: () => {} }, { path: '/foreman/stream' });
    foreman.start();
    await vi.waitFor(() => expect(urls).toHaveLength(2));
    foreman.stop();

    expect(urls[0]).toContain('/tasks/t1/stream');
    expect(urls[1]).toContain('/foreman/stream');
  });
});
