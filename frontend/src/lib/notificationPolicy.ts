import type { SseEvent } from '../api/types';

/**
 * 通知策略（决策 65 / 130③）：cooldown / quiet_hours **只作用于前端 toast**。
 *
 * SSE 是状态同步通道，全量推送、不做合并（吞事件会丢状态）。
 * - toast 只对 pending / done / failed 弹；cancelled 永不弹。
 * - 同类通知 5 分钟 cooldown。
 * - 22–8 免打扰，pending 豁免（"等人优先"，§2 原则 3）。
 */

export type NotificationClass = 'pending' | 'done' | 'failed' | 'cancelled';

export interface NotificationPolicyConfig {
  notifyOn: Record<NotificationClass, boolean>;
  /** 同类通知合并窗口（秒）。 */
  cooldownSec: number;
  /** [开始, 结束)，跨零点表示法：22 → 8。 */
  quietHours: [number, number];
}

export const DEFAULT_NOTIFICATION_POLICY: NotificationPolicyConfig = {
  notifyOn: { pending: true, done: true, failed: true, cancelled: false },
  cooldownSec: 300,
  quietHours: [22, 8],
};

export interface NotifyState {
  lastNotifiedAt: Partial<Record<NotificationClass, number>>;
}

export function isQuietHours(date: Date, quietHours: [number, number]): boolean {
  const [start, end] = quietHours;
  if (start === end) return false;
  const h = date.getHours();
  if (start < end) return h >= start && h < end;
  return h >= start || h < end;
}

/** 是否应弹 toast（`now` 注入以便测试）。 */
export function shouldNotify(
  cls: NotificationClass,
  now: Date,
  state: NotifyState,
  policy: NotificationPolicyConfig = DEFAULT_NOTIFICATION_POLICY,
): boolean {
  if (!policy.notifyOn[cls]) return false;
  // quiet hours：pending 豁免
  if (isQuietHours(now, policy.quietHours) && cls !== 'pending') return false;
  const last = state.lastNotifiedAt[cls];
  if (last !== undefined && now.getTime() - last < policy.cooldownSec * 1000) return false;
  return true;
}

/** SSE 事件 → 通知分类（非通知事件返回 null）。stalled 只做高亮，不弹。 */
export function notificationClassForEvent(event: SseEvent): NotificationClass | null {
  switch (event.type) {
    case 'pending':
      return 'pending';
    case 'pending_updated':
      return 'pending';
    case 'task_done':
      return 'done';
    case 'task_failed':
      return 'failed';
    case 'task_cancelled':
      return 'cancelled';
    default:
      return null;
  }
}
