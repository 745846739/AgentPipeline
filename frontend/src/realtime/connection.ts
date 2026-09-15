import { PAIRING_HEADER, apiUrl, getPairingToken } from '../api/config';
import type { SseEvent } from '../api/types';

/**
 * SSE 连接层（决策 76 / 153②）。
 *
 * **fetch 流式读取，不用 EventSource**：EventSource 无法携带自定义头，
 * 跨源过不了决策 128 防护，也违背 153② 的桌面壳约束。
 *
 * 特性：指数退避重连 + `document.visibilitychange` 恢复时立即校准一次。
 */

export type StreamStatus = 'idle' | 'connecting' | 'open' | 'closed' | 'error';

export interface TaskStreamHandlers {
  onEvent: (taskId: string, event: SseEvent) => void;
  onStatus?: (taskId: string, status: StreamStatus) => void;
  /** 恢复可见时请求一次全量校准（GET /tasks/{id}，决策 76 唯一状态入口）。 */
  onRecalibrate?: (taskId: string) => void;
}

export interface TaskStreamOptions {
  baseDelayMs?: number;
  maxDelayMs?: number;
  /**
   * 流地址覆盖（票 03）。
   *
   * 值班长的流（`/foreman/stream`）与任务流是同一种 SSE：复用本类的分帧、退避与
   * 主动重连，只换路径。**不另造第二个解析器**——两套分帧迟早各自漂移，而其中一套
   * 的分帧错误只在生产的长 delta 上才现形。缺省是任务流地址。
   */
  path?: string;
}

/** 解析 SSE 帧，返回本次 chunk 产生的 data 载荷。纯函数，便于测试。 */
export function parseSseFrames(buffer: string): { events: string[]; rest: string } {
  const normalized = buffer.replace(/\r\n/g, '\n');
  const events: string[] = [];
  let rest = normalized;
  let idx: number;
  while ((idx = rest.indexOf('\n\n')) >= 0) {
    const raw = rest.slice(0, idx);
    rest = rest.slice(idx + 2);
    let data = '';
    for (const line of raw.split('\n')) {
      if (line.startsWith('data:')) data += line.slice(5).replace(/^ /, '');
    }
    if (data) events.push(data);
  }
  return { events, rest };
}

export class TaskStream {
  private controller: AbortController | null = null;
  private stopped = false;
  /** 由 `reconnectNow()` 触发的主动中断：循环必须重连，而不是退出。 */
  private restarting = false;
  private attempt = 0;
  private status: StreamStatus = 'idle';

  constructor(
    readonly taskId: string,
    private readonly handlers: TaskStreamHandlers,
    private readonly options: TaskStreamOptions = {},
  ) {}

  start(): void {
    this.stopped = false;
    void this.loop();
  }

  stop(): void {
    this.stopped = true;
    this.controller?.abort();
    this.controller = null;
    this.setStatus('closed');
  }

  /** visibilitychange 恢复：重置退避并立即重连（无 SSE 回放，靠 refetch 校准）。 */
  reconnectNow(): void {
    if (this.stopped) return;
    this.attempt = 0;
    // 主动中断必须与 stop() 区分：否则 AbortError 会让循环永久退出，
    // visibilitychange 之后再也收不到实时更新（§9.1「恢复时立即校准一次」）。
    this.restarting = true;
    this.controller?.abort();
    this.controller = null;
  }

  private setStatus(status: StreamStatus): void {
    if (this.status === status) return;
    this.status = status;
    this.handlers.onStatus?.(this.taskId, status);
  }

  private backoffDelay(): number {
    const base = this.options.baseDelayMs ?? 1000;
    const max = this.options.maxDelayMs ?? 30_000;
    const exp = Math.min(max, base * 2 ** this.attempt);
    return exp + Math.floor(Math.random() * Math.min(1000, exp / 2));
  }

  private async loop(): Promise<void> {
    // 路径在每次重连时重算：`path` 覆盖是本类唯一按宿主变化的东西
    const path = this.options.path ?? `/tasks/${encodeURIComponent(this.taskId)}/stream`;
    while (!this.stopped) {
      this.setStatus('connecting');
      this.controller = new AbortController();
      try {
        // 每次尝试都重读令牌：SSE 是 fetch 流不是 EventSource（决策 153②），所以头是能带的
        // ——对讲台在非回环形态下是受护接口，缺它连不上（决策 182㉙）。放在循环内是为了
        // 「先被 403、随后配对成功」的那条路能在下一次重连上带出新令牌。
        const token = getPairingToken();
        const headers: Record<string, string> = { Accept: 'text/event-stream' };
        if (token) headers[PAIRING_HEADER] = token;
        const res = await fetch(apiUrl(path), {
          method: 'GET',
          headers,
          signal: this.controller.signal,
        });
        if (!res.ok || !res.body) {
          throw new Error(`SSE ${res.status}`);
        }
        this.attempt = 0;
        this.setStatus('open');
        await this.consume(res.body);
        // 正常结束（服务端关闭）也走重连
        if (this.stopped) break;
        throw new Error('stream ended');
      } catch (err) {
        if (this.stopped) break;
        if ((err as Error).name === 'AbortError') {
          // 主动重连（reconnectNow）：不退出循环、不退避，立刻重开一条流。
          if (this.restarting) {
            this.restarting = false;
            continue;
          }
          break;
        }
        this.attempt += 1;
        this.setStatus('error');
        await sleep(this.backoffDelay());
      }
    }
    this.setStatus('closed');
  }

  private async consume(body: ReadableStream<Uint8Array>): Promise<void> {
    const reader = body.getReader();
    const decoder = new TextDecoder();
    let buffer = '';
    while (!this.stopped) {
      const { value, done } = await reader.read();
      if (done) return;
      buffer += decoder.decode(value, { stream: true });
      const { events, rest } = parseSseFrames(buffer);
      buffer = rest;
      for (const payload of events) {
        try {
          const event = JSON.parse(payload) as SseEvent;
          if (event && typeof event.type === 'string') {
            this.handlers.onEvent(this.taskId, event);
          }
        } catch {
          // 忽略无法解析的帧（SSE 仅渲染通道，丢失由 refetch 兜底）
        }
      }
    }
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/** 多任务流管理：只为 running / pending 任务开流（§9.1）。 */
export class StreamManager {
  private connections = new Map<string, TaskStream>();
  private disposed = false;

  constructor(
    private readonly handlers: TaskStreamHandlers,
    private readonly options: TaskStreamOptions = {},
  ) {
    if (typeof document !== 'undefined') {
      document.addEventListener('visibilitychange', this.onVisibility);
    }
  }

  /** 收敛订阅集合：新增缺失的开流，多余的关流。 */
  sync(taskIds: string[]): void {
    const wanted = new Set(taskIds);
    for (const [id, conn] of this.connections) {
      if (!wanted.has(id)) {
        conn.stop();
        this.connections.delete(id);
      }
    }
    for (const id of wanted) {
      if (!this.connections.has(id)) {
        const conn = new TaskStream(id, this.handlers, this.options);
        this.connections.set(id, conn);
        conn.start();
      }
    }
  }

  stopAll(): void {
    for (const conn of this.connections.values()) conn.stop();
    this.connections.clear();
  }

  dispose(): void {
    this.disposed = true;
    this.stopAll();
    if (typeof document !== 'undefined') {
      document.removeEventListener('visibilitychange', this.onVisibility);
    }
  }

  private onVisibility = (): void => {
    if (this.disposed) return;
    if (typeof document === 'undefined' || document.visibilityState !== 'visible') return;
    for (const [id, conn] of this.connections) {
      this.handlers.onRecalibrate?.(id);
      conn.reconnectNow();
    }
  };
}
