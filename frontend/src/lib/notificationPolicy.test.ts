import { describe, expect, it } from 'vitest';
import type { SseEvent } from '../api/types';
import {
  DEFAULT_NOTIFICATION_POLICY,
  isQuietHours,
  notificationClassForEvent,
  shouldNotify,
  type NotifyState,
} from './notificationPolicy';

function at(hour: number, minute = 0, second = 0): Date {
  return new Date(2026, 8, 12, hour, minute, second, 0);
}

const fresh: NotifyState = { lastNotifiedAt: {} };

describe('NotificationPolicy（决策 65 / 130③）', () => {
  it('cancelled 永不弹', () => {
    expect(shouldNotify('cancelled', at(12), fresh)).toBe(false);
    expect(DEFAULT_NOTIFICATION_POLICY.notifyOn.cancelled).toBe(false);
  });

  it('同类 5 分钟 cooldown', () => {
    const state: NotifyState = { lastNotifiedAt: { pending: at(12, 0).getTime() } };
    expect(shouldNotify('pending', at(12, 4, 59), state)).toBe(false);
    expect(shouldNotify('pending', at(12, 5, 0), state)).toBe(true);
  });

  it('不同类互不影响 cooldown', () => {
    const state: NotifyState = { lastNotifiedAt: { pending: at(12, 0).getTime() } };
    expect(shouldNotify('done', at(12, 1), state)).toBe(true);
  });

  it('22–8 免打扰：done / failed 静音，pending 豁免', () => {
    expect(isQuietHours(at(23), [22, 8])).toBe(true);
    expect(isQuietHours(at(3), [22, 8])).toBe(true);
    expect(isQuietHours(at(12), [22, 8])).toBe(false);
    expect(isQuietHours(at(8), [22, 8])).toBe(false);
    expect(shouldNotify('done', at(23), fresh)).toBe(false);
    expect(shouldNotify('failed', at(3), fresh)).toBe(false);
    // 等人优先（§2 原则 3）：pending 在免打扰时段照弹
    expect(shouldNotify('pending', at(23), fresh)).toBe(true);
  });

  it('notifyOn 关闭即静音', () => {
    const policy = {
      ...DEFAULT_NOTIFICATION_POLICY,
      notifyOn: { ...DEFAULT_NOTIFICATION_POLICY.notifyOn, done: false },
    };
    expect(shouldNotify('done', at(12), fresh, policy)).toBe(false);
  });
});

describe('notificationClassForEvent', () => {
  it('映射 pending / done / failed / cancelled，其余为 null', () => {
    const base = { task_id: 't', branch: 'main' };
    const cases: Array<[SseEvent['type'], string | null]> = [
      ['pending', 'pending'],
      ['pending_updated', 'pending'],
      ['task_done', 'done'],
      ['task_failed', 'failed'],
      ['task_cancelled', 'cancelled'],
      ['stalled', null],
      ['stage_changed', null],
      ['conversation_delta', null],
    ];
    for (const [type, expected] of cases) {
      const event = { type, ...base } as SseEvent;
      expect(notificationClassForEvent(event)).toBe(expected);
    }
  });
});
