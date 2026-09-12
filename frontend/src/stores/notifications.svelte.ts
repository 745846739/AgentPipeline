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

/**
 * 应用内 toast（决策 65：v1 只做应用内通知）。
 * 冷却 / 免打扰判定委托纯函数 `shouldNotify`（可单测）。
 */
class NotificationStore {
  toasts = $state<Toast[]>([]);
  policy: NotificationPolicyConfig = DEFAULT_NOTIFICATION_POLICY;
  private notifyState: NotifyState = { lastNotifiedAt: {} };
  private seq = 1;

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
      const ttl = cls === 'pending' ? 12_000 : 8_000;
      setTimeout(() => this.dismiss(toast.id), ttl);
    }
    return true;
  }

  dismiss(id: number): void {
    this.toasts = this.toasts.filter((t) => t.id !== id);
  }

  clear(): void {
    this.toasts = [];
  }

  /** 测试用：重置冷却窗口。 */
  resetCooldown(): void {
    this.notifyState = { lastNotifiedAt: {} };
  }
}

export const notifications = new NotificationStore();
