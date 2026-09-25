import { describe, expect, it } from 'vitest';

import type { NotifySettings } from '../api/types';
import {
  COOLDOWN_SEC_MAX,
  describeCooldown,
  describeQuietHours,
  draftFromSettings,
  parseNotifyPolitenessDraft,
  type NotifyPolitenessDraft,
} from './notifyPoliteness';

/**
 * 「离线通知」设置页礼貌两件的判据（决策 284②③⑤）。
 *
 * 钉的是**按下保存之前**就能看见的那一层：整体取值（两件一起送，越界一项也不放行）、
 * 与后端同一把尺的范围（0–86400 秒 / 0–23 整点，边界含）、以及两句实时描述文案。
 * 后端是权威——越界后端还会再拦一次并点名，这里不测那条路径。
 */

function draft(over: Partial<NotifyPolitenessDraft> = {}): NotifyPolitenessDraft {
  return { cooldownSec: '300', quietStart: '22', quietEnd: '8', ...over };
}

describe('notifyPoliteness（决策 284）', () => {
  it('读数 → 草稿：预填**生效值**（整体覆盖不必手抄配置里的数字）', () => {
    const s = {
      enabled: true,
      channel: 'bluebubbles',
      origin: 'settings',
      webhook_url: '',
      bluebubbles_url: '',
      bluebubbles_password: '',
      bluebubbles_recipient: '',
      cooldown_sec: 60,
      quiet_hours: [23, 7],
      politeness_origin: 'settings',
    } satisfies NotifySettings;
    expect(draftFromSettings(s)).toEqual({
      cooldownSec: '60',
      quietStart: '23',
      quietEnd: '7',
    });
  });

  it('过了就整体给值：跨零点 / 起止相同 / 上下边界都合法', () => {
    expect(parseNotifyPolitenessDraft(draft())).toEqual({
      ok: true,
      payload: { cooldown_sec: 300, quiet_hours: [22, 8] },
    });
    expect(parseNotifyPolitenessDraft(draft({ quietStart: '8', quietEnd: '8' }))).toEqual({
      ok: true,
      payload: { cooldown_sec: 300, quiet_hours: [8, 8] },
    });
    expect(
      parseNotifyPolitenessDraft({
        cooldownSec: String(COOLDOWN_SEC_MAX),
        quietStart: '0',
        quietEnd: '23',
      }),
    ).toEqual({
      ok: true,
      payload: { cooldown_sec: COOLDOWN_SEC_MAX, quiet_hours: [0, 23] },
    });
    // 0 = 不节流（不是「没填」）：它是合法值，照送。
    expect(parseNotifyPolitenessDraft(draft({ cooldownSec: '0' }))).toEqual({
      ok: true,
      payload: { cooldown_sec: 0, quiet_hours: [22, 8] },
    });
  });

  it('没过就一项也不放行：越界 / 空 / 半截 / 小数 / 负号都点名是哪一件', () => {
    const cooldownBad = parseNotifyPolitenessDraft(draft({ cooldownSec: '86401' }));
    expect(cooldownBad.ok).toBe(false);
    if (!cooldownBad.ok) expect(cooldownBad.error).toMatch(/节流/);

    for (const bad of ['', '  ', '3.5', '-1', '1e3', 'abc']) {
      const r = parseNotifyPolitenessDraft(draft({ cooldownSec: bad }));
      expect(r.ok, bad).toBe(false);
    }

    const hourBad = parseNotifyPolitenessDraft(draft({ quietEnd: '24' }));
    expect(hourBad.ok).toBe(false);
    if (!hourBad.ok) expect(hourBad.error).toMatch(/免打扰/);
    expect(parseNotifyPolitenessDraft(draft({ quietStart: '' })).ok).toBe(false);
    expect(parseNotifyPolitenessDraft(draft({ quietEnd: '8.5' })).ok).toBe(false);
  });

  it('两句描述文案：起止相同 = 全天，跨零点说「次日」，节流 0 说不节流', () => {
    expect(describeQuietHours([22, 8])).toBe('22 点–次日 8 点之间除待办与失败外不出站');
    expect(describeQuietHours([8, 18])).toBe('8–18 点之间除待办与失败外不出站');
    expect(describeQuietHours([8, 8])).toBe('免打扰关着（全天都送）');
    expect(describeCooldown(300)).toBe('同类 300 秒内只出站一条');
    expect(describeCooldown(0)).toBe('不节流（同类有多少发多少）');
  });
});
