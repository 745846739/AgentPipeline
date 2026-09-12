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
});
