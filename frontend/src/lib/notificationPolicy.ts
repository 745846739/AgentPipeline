import type { SseEvent } from '../api/types';

/**
 * 通知策略（决策 65 / 130③）：cooldown / quiet_hours **只作用于前端 toast**。
 *
 * SSE 是状态同步通道，全量推送、不做合并（吞事件会丢状态）。
 * - toast 只对 pending / done / failed 弹；cancelled 永不弹。
 * - 同类通知 5 分钟 cooldown。
 * - 22–8 免打扰，pending 豁免（"等人优先"，§2 原则 3）。
 *
 * **由票 17（R2-23）补的一条口径：节流只作用于「另有通道」的那几类。**
 * 原策略是「一律 5 分钟 + 一律免打扰」，于是夜间（22–8）或同类 5 分钟内，一条
 * `task_failed` **根本不弹**——而那时它没有任何别的常驻位（待办计数只数 pending、
 * 完成横幅只报 done）。所以：`failed` 既免免打扰也免 cooldown（**唯一**这样的类），
 * `pending` 免免打扰（照旧，§2 原则 3）、cooldown 保留（计数芯片与档案盒是它的常驻通道），
 * `done` 两者都保留（完成横幅不受任何节流，它另有通道，丢掉的不算丢）。
 *
 * **决策 383 的显式分叉**：上面这套只管**浏览器 toast**。出机器那条线（webhook /
 * 浏览器推送，core `notify.rs`）对 `failed` 的夜间行为不同——免打扰段内静音 + 只累计，
 * 段结束补一条摘要（夜里连环失败把免打扰洞穿 40 多次是它的起因）。本文件的 failed
 * 豁免不受影响：浏览器开着时，failed 的 toast 仍是它唯一的常驻位。
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

/** 既免免打扰、也免 cooldown 的类（见文件头的口径说明）。 */
export const ALWAYS_ANNOUNCED: ReadonlySet<NotificationClass> = new Set(['failed']);

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
  // 只有 toast 一条通道的那一类：免打扰与节流都不拦它
  if (ALWAYS_ANNOUNCED.has(cls)) return true;
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
