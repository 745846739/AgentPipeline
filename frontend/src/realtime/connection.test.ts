import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

import { afterEach, describe, expect, it, vi } from 'vitest';
import { parseSseFrames, StreamManager, TaskStream } from './connection';

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

/** 统一的流式 fetch 桩：每次调用开一条真 ReadableStream 并记进 `opened`。 */
function stubStreamFetch(opened: ReturnType<typeof openStream>[]) {
  const fetchMock = vi.fn(async (_url: string, init?: RequestInit) => {
    const stream = openStream(init!.signal!);
    opened.push(stream);
    return new Response(stream.stream, {
      status: 200,
      headers: { 'content-type': 'text/event-stream' },
    });
  });
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
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

  // 心跳注释帧（票 01，stream-self-heal）：服务端 keep-alive 只发注释——
  // 解析器零改动的钉子：不产出事件、不留残渣、也不吞掉同 chunk 里后面的真事件。
  it('心跳注释帧不产出事件、不留残渣', () => {
    expect(parseSseFrames(': keepalive\n\n')).toEqual({ events: [], rest: '' });
    const mixed = parseSseFrames(': keepalive\n\ndata: {"a":1}\n\n');
    expect(mixed.events).toEqual(['{"a":1}']);
    expect(mixed.rest).toBe('');
  });
});

describe('TaskStream 主动重连', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('reconnectNow 后立刻重开一条流，且重连后的流仍能收到事件', async () => {
    const opened: ReturnType<typeof openStream>[] = [];
    const fetchMock = stubStreamFetch(opened);

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

  it('主动重连（reconnectNow）成功后也要对齐一次（修解锁竞态，票 03）', async () => {
    // 场景：锁屏解锁 → visibilitychange → 首次校准请求因网络未醒先失败，
    // 紧随的重连却成功——若主动重连不置「断过线」标记，系统会认为没错过，
    // 断流期间的变化停在旧画面。这里钉住：主动重连成功后校准回调必被调用。
    const opened: ReturnType<typeof openStream>[] = [];
    const fetchMock = stubStreamFetch(opened);

    const recalibrated: string[] = [];
    const conn = new TaskStream('t1', {
      onEvent: () => {},
      onRecalibrate: (id) => recalibrated.push(id),
    });
    conn.start();
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));

    conn.reconnectNow();
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2), { timeout: 2000 });
    // 重连成功（第二次流 open）之后：断过的线必须补一次全量校准
    await vi.waitFor(() => expect(recalibrated).toEqual(['t1']), { timeout: 2000 });

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

describe('TaskStream 停滞看门狗（票 02，stream-self-heal）', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('open 后静默到阈值：判死重连，且重连成功后对齐一次', async () => {
    // 半开连接的形态：连接「通」、状态 open、再也没字节。看门狗必须主动中断重连；
    // 断流期事件已丢，重连成功后要补一次全量校准（reconnectNow 置「断过线」）。
    const opened: ReturnType<typeof openStream>[] = [];
    const fetchMock = stubStreamFetch(opened);

    const recalibrated: string[] = [];
    const conn = new TaskStream(
      't1',
      {
        onEvent: () => {},
        onRecalibrate: (id) => recalibrated.push(id),
      },
      { stallTimeoutMs: 40, baseDelayMs: 1, maxDelayMs: 1 },
    );
    conn.start();
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));
    // 一个字节都不推：等看门狗自己判死（周期性重连会继续发生，故断言「至少」，
    // 不钉「恰好」——那是把并发时序写进断言）
    await vi.waitFor(() => expect(fetchMock.mock.calls.length).toBeGreaterThanOrEqual(2), {
      timeout: 2000,
    });
    await vi.waitFor(() => expect(recalibrated[0]).toBe('t1'), { timeout: 2000 });

    conn.stop();
  });

  it('阈值内收到字节（心跳也是字节）：计时重置，不误杀健康安静的连接', async () => {
    // 判据是字节而非事件：心跳注释帧解析出零事件，但它证明线还活着。
    const opened: ReturnType<typeof openStream>[] = [];
    const fetchMock = stubStreamFetch(opened);

    const conn = new TaskStream('t1', { onEvent: () => {} }, { stallTimeoutMs: 60 });
    conn.start();
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));

    // 每 20ms 一帧心跳，跨过两倍阈值——期间最后一次字节离现在总 < 60ms
    for (let i = 0; i < 5; i += 1) {
      opened[0].push(': keepalive\n\n');
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
    expect(fetchMock, '心跳字节一直在来，不该被判死').toHaveBeenCalledTimes(1);

    conn.stop();
  });

  it('stop 之后：不再发起任何连接（没有悬挂的看门狗）', async () => {
    const fetchMock = stubStreamFetch([]);

    // open 一到立刻 stop（微任务，远小于阈值）：计时器刚挂上就被拆掉。
    // 用 waitFor 轮询会等 30ms+，看门狗会先合法地开出第二次连接——那是把
    // 轮询延迟写进了断言。
    let conn!: TaskStream;
    await new Promise<void>((resolve) => {
      conn = new TaskStream(
        't1',
        {
          onEvent: () => {},
          onStatus: (_id, s) => {
            if (s === 'open') resolve();
          },
        },
        { stallTimeoutMs: 30 },
      );
      conn.start();
    });
    conn.stop();
    expect(fetchMock).toHaveBeenCalledTimes(1);

    await new Promise((resolve) => setTimeout(resolve, 90));
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  // 跨语言镜像（票 01/02 验收：两常量成 3 倍关系，「改一个必须看另一个」）——
  // 散文耦合由机器守，姿态照 css-parity / delegation-scan 的先例。
  it('看门狗缺省阈值 = 3 × 服务端心跳间隔（跨语言镜像，机器钉住）', () => {
    // jsdom 下 `import.meta.url` 是 http:// 形态（behavior-map.test.ts 记过同一坑）：
    // 走 cwd，姿态照 delegation-scan。
    const rust = readFileSync(resolve(process.cwd(), '../crates/app/src/stream.rs'), 'utf8');
    const ts = readFileSync(resolve(process.cwd(), 'src/realtime/connection.ts'), 'utf8');
    const keepalive = rust.match(
      /SSE_KEEPALIVE_INTERVAL[^=]*=\s*std::time::Duration::from_secs\((\d+)\)/,
    );
    const stall = ts.match(/stallTimeoutMs \?\? ([\d_]+)/);
    expect(keepalive, 'stream.rs 里认不出 SSE_KEEPALIVE_INTERVAL 常量').not.toBeNull();
    expect(stall, 'connection.ts 里认不出看门狗缺省阈值').not.toBeNull();
    expect(Number(stall![1].replace(/_/g, ''))).toBe(Number(keepalive![1]) * 1000 * 3);
  });
});

describe('StreamManager 可见性恢复（spec stream-self-heal 缝 A ④）', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('visibilitychange 恢复：每条流都校准 + 立即重开一条', async () => {
    const opened: ReturnType<typeof openStream>[] = [];
    const fetchMock = stubStreamFetch(opened);

    const recalibrated: string[] = [];
    const manager = new StreamManager({
      onEvent: () => {},
      onRecalibrate: (id) => recalibrated.push(id),
    });
    try {
      manager.sync(['t1', 't2']);
      await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2));

      // jsdom 的 visibilityState 缺省就是 visible：事件本身是唤醒信号
      document.dispatchEvent(new Event('visibilitychange'));

      // 校准：可见性回调对每条流直接来一次（用集合断言，容忍重连成功后的又一次补校准）
      await vi.waitFor(() => expect(new Set(recalibrated).size).toBe(2));
      expect([...new Set(recalibrated)].sort()).toEqual(['t1', 't2']);
      // 重连：每条流再开一条（reconnectNow 中断旧的、立刻新开）
      await vi.waitFor(() => expect(fetchMock.mock.calls.length).toBeGreaterThanOrEqual(4), {
        timeout: 2000,
      });
    } finally {
      manager.dispose();
    }
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
