import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { notifications, PENDING_TOAST_TTL_MS, TOAST_TTL_MS } from './notifications.svelte';

/**
 * toast 的停留与暂停（票 17 / R2-23）。
 *
 * 此前 TTL 是写死的 8s / 12s，**不因悬停或焦点暂停**——正在读的一条会在手指底下消失。
 * 这里钉的是那条算术：**暂停扣掉已过去的那一截、移开后接着算剩下的**（不是重新计满）。
 *
 * **用假时钟**（`vi.useFakeTimers`）：`pause` / `resume` 的第二个参数让算术可注入，
 * 但「到点真的收走」是 `setTimeout` 干的——只用注入的 `now`、让真时钟跑，这条算术错了
 * 也照样绿（评审当场指出过这个假信心）。假时钟让「停多久 → 到点没」可断言。
 */

function notifyAt(cls: 'done' | 'pending' = 'done') {
  return notifications.notify(cls, { title: '任务甲 · done' });
}

/** 当前屏上还有没有这一条。 */
function has(id: number): boolean {
  return notifications.toasts.some((t) => t.id === id);
}

describe('toast 的暂停与恢复（票 17）', () => {
  beforeEach(() => {
    // 每条用例从零开始：cooldown 是跨用例的状态（同类 5 分钟内只弹一次）
    //
    // **假时钟必须钉在一个白天时刻**：通知策略有免打扰窗口（22–8），而 `notifyAt()`
    // 用的是 `done` 类、`notify()` 默认取挂钟——夜里跑闸门时 `shouldNotify` 直接返回
    // false，toast 根本不弹，这组用例在 22 点后集体变红（失败点是 `toasts[0].id` 读
    // 不到，与暂停算术毫无关系）。免打扰本身由 `notificationPolicy.test.ts` 用显式
    // 日期单独钉住，这里注入一个固定时刻不会漏掉那条口径。
    vi.useFakeTimers({ now: new Date(2026, 0, 15, 12, 0, 0) });
    notifications.clear();
    notifications.resetCooldown();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('读了 3 秒再暂停：移开后只剩 5 秒（不是重新计满 8 秒）', () => {
    notifyAt();
    const id = notifications.toasts[0].id;

    vi.advanceTimersByTime(3_000); // 读了 3 秒
    notifications.pause(id); // 悬停 → 计时停住

    vi.advanceTimersByTime(60_000); // 手指停着不动，过了一分钟
    expect(has(id), '暂停期间不该被收走').toBe(true);

    notifications.resume(id); // 移开 → 接着算剩下的 5 秒
    vi.advanceTimersByTime(4_900);
    expect(has(id), '剩下的 5 秒还没走完').toBe(true);

    vi.advanceTimersByTime(200);
    expect(has(id), '剩下的 5 秒走完就该消解（重新计满的话这时还在）').toBe(false);
  });

  it('没暂停过就是从头 8 秒；到点自己消解', () => {
    notifyAt();
    const id = notifications.toasts[0].id;

    vi.advanceTimersByTime(TOAST_TTL_MS - 100);
    expect(has(id)).toBe(true);
    vi.advanceTimersByTime(200);
    expect(has(id)).toBe(false);
  });

  it('重复暂停 / 未暂停就恢复都是 no-op（悬停与聚焦会各触发一次）', () => {
    notifyAt();
    const id = notifications.toasts[0].id;

    notifications.resume(id); // 没暂停过：不动
    vi.advanceTimersByTime(1_000);
    notifications.pause(id); // 第一次暂停：已过 1 秒
    notifications.pause(id); // 第二次：不重复扣
    vi.advanceTimersByTime(60_000);
    notifications.resume(id);

    vi.advanceTimersByTime(TOAST_TTL_MS - 1_000 - 100); // 剩下的 7 秒里
    expect(has(id)).toBe(true);
    vi.advanceTimersByTime(200);
    expect(has(id), '重复暂停不该多扣出时间').toBe(false);
  });

  it('pending 停留更久（12s vs 8s），且 dismiss 后从清单里消失', () => {
    expect(PENDING_TOAST_TTL_MS).toBeGreaterThan(TOAST_TTL_MS);
    notifyAt('pending');
    const id = notifications.toasts[0].id;

    vi.advanceTimersByTime(TOAST_TTL_MS + 100);
    expect(has(id), 'pending 的 12s 还没到').toBe(true);
    vi.advanceTimersByTime(PENDING_TOAST_TTL_MS - TOAST_TTL_MS);

    expect(notifications.toasts).toHaveLength(0);
  });
});
