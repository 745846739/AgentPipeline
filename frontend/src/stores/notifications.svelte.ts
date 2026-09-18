import {
  DEFAULT_NOTIFICATION_POLICY,
  type NotificationClass,
  type NotificationPolicyConfig,
  type NotifyState,
  shouldNotify,
} from '../lib/notificationPolicy';

export interface Toast {
  id: number;
  cls: NotificationClass;
  title: string;
  message?: string;
  taskId?: string;
  createdAt: number;
}

/** toast 的停留时长（决策 65 的 8s / pending 12s，票 17 起可由悬停暂停）。 */
export const TOAST_TTL_MS = 8_000;
export const PENDING_TOAST_TTL_MS = 12_000;

interface LiveToast {
  startedAt: number;
  remaining: number;
  handle: ReturnType<typeof setTimeout> | null;
}

/**
 * 应用内 toast（决策 65：v1 只做应用内通知）。
 * 冷却 / 免打扰判定委托纯函数 `shouldNotify`（可单测）。
 *
 * **计时可暂停**（票 17 / R2-23）：鼠标停在上面、或键盘焦点进到里面时，一条 toast
 * 不该在手指底下消失——那正是「还没读完就没了」的形态。`pause` / `resume` 把剩余时长
 * 接着算（不是重新计），`now` 可注入以便单测。
 */
class NotificationStore {
  toasts = $state<Toast[]>([]);
  policy: NotificationPolicyConfig = DEFAULT_NOTIFICATION_POLICY;
  private notifyState: NotifyState = { lastNotifiedAt: {} };
  private seq = 1;
  private live = new Map<number, LiveToast>();

  /** 返回是否真的弹出（cooldown / quiet_hours / cancelled 被抑制时为 false）。 */
  notify(
    cls: NotificationClass,
    payload: { title: string; message?: string; taskId?: string },
    now: Date = new Date(),
  ): boolean {
    if (!shouldNotify(cls, now, this.notifyState, this.policy)) return false;
    this.notifyState.lastNotifiedAt[cls] = now.getTime();
    const toast: Toast = {
      id: this.seq++,
      cls,
      createdAt: now.getTime(),
      ...payload,
    };
    this.toasts = [...this.toasts, toast];
    // 自动消解（pending 停留更久）
    if (typeof setTimeout !== 'undefined') {
      const ttl = cls === 'pending' ? PENDING_TOAST_TTL_MS : TOAST_TTL_MS;
      this.live.set(toast.id, {
        startedAt: now.getTime(),
        remaining: ttl,
        handle: setTimeout(() => this.dismiss(toast.id), ttl),
      });
    }
    return true;
  }

  /** 悬停 / 聚焦时暂停这一条（票 17）。重复调用是 no-op。 */
  pause(id: number, now: number = Date.now()): void {
    const t = this.live.get(id);
    if (!t || t.handle === null) return;
    clearTimeout(t.handle);
    t.remaining = Math.max(0, t.remaining - (now - t.startedAt));
    t.handle = null;
  }

  /** 接着算剩余时长（不是重新计满）。没暂停过就是 no-op。 */
  resume(id: number, now: number = Date.now()): void {
    const t = this.live.get(id);
    if (!t || t.handle !== null) return;
    t.startedAt = now;
    t.handle = setTimeout(() => this.dismiss(id), t.remaining);
  }

  dismiss(id: number): void {
    const t = this.live.get(id);
    if (t?.handle) clearTimeout(t.handle);
    this.live.delete(id);
    this.toasts = this.toasts.filter((x) => x.id !== id);
  }

  clear(): void {
    for (const t of this.live.values()) if (t.handle) clearTimeout(t.handle);
    this.live.clear();
    this.toasts = [];
  }

  /** 测试用：重置冷却窗口。 */
  resetCooldown(): void {
    this.notifyState = { lastNotifiedAt: {} };
  }
}

export const notifications = new NotificationStore();
