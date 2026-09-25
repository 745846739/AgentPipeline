import type { NotifyPolitenessPayload, NotifySettings } from '../api/types';

/**
 * 「离线通知」设置页**礼貌两件**的判据（决策 284②③⑤）。
 *
 * 管的是**出机器那条线**（webhook / 飞书 / iMessage）：节流窗口与免打扰时段——
 * 此前只住 `config.toml` 的 `[notify]` 段（272⑥），自 284 起这一页能改它们，
 * 与通道四件同构：**整体覆盖**配置文件、`origin` 说清谁定的、可以整组交还。
 * 浏览器里的 toast 另有自己一份固定表（`lib/notificationPolicy.ts`），本页不动它。
 *
 * 判据只做「按下按钮之前就能看见的错」，且与后端**同一把尺**（越界那一层后端还会
 * 再拦一次并点名，报错不静默）：节流 0–86400 整数秒、免打扰起止各 0–23 整点。
 */

/** 界面那一级的节流上限，与 core `notify::COOLDOWN_SEC_MAX` 同值（秒，一天）。 */
export const COOLDOWN_SEC_MAX = 86_400;

/** 整点上限（`[start, end)` 的合法值域）。 */
export const QUIET_HOUR_MAX = 23;

/** 编辑态草稿：数字用字符串装——输入框里空着、写一半都是合法中间态。 */
export interface NotifyPolitenessDraft {
  cooldownSec: string;
  quietStart: string;
  quietEnd: string;
}

/** 读数 → 草稿（预填**生效值**：整体覆盖因此不必手抄一遍配置里的数字）。 */
export function draftFromSettings(s: NotifySettings): NotifyPolitenessDraft {
  return {
    cooldownSec: String(s.cooldown_sec),
    quietStart: String(s.quiet_hours[0]),
    quietEnd: String(s.quiet_hours[1]),
  };
}

/** `0`–`max` 的十进制整数（空 / 半截 / 小数 / 负号 / 越界一律不算）。 */
function parseIntegerInRange(raw: string, max: number): number | null {
  const text = raw.trim();
  if (!/^\d+$/.test(text)) return null;
  const value = Number(text);
  return value <= max ? value : null;
}

/** 草稿的判读结果：过了给载荷，没过给面向用户的中文报文。 */
export type PolitenessParse =
  | { ok: true; payload: NotifyPolitenessPayload }
  | { ok: false; error: string };

/**
 * 草稿 → 载荷（唯一的判读点：校验与取值同一处，界面拿 `error` 直接进 `note bad`）。
 */
export function parseNotifyPolitenessDraft(d: NotifyPolitenessDraft): PolitenessParse {
  const cooldown = parseIntegerInRange(d.cooldownSec, COOLDOWN_SEC_MAX);
  if (cooldown === null) {
    return {
      ok: false,
      error: `节流要填 0–${COOLDOWN_SEC_MAX} 之间的整数秒（0 = 不节流）。`,
    };
  }
  const start = parseIntegerInRange(d.quietStart, QUIET_HOUR_MAX);
  const end = parseIntegerInRange(d.quietEnd, QUIET_HOUR_MAX);
  if (start === null || end === null) {
    return {
      ok: false,
      error: '免打扰起止要填 0–23 之间的整点（起止相同 = 全天不静默）。',
    };
  }
  return { ok: true, payload: { cooldown_sec: cooldown, quiet_hours: [start, end] } };
}

/** 免打扰时段的一句话（页面上按草稿实时显示，判据纯函数化以便单测）。 */
export function describeQuietHours(quiet: [number, number]): string {
  const [start, end] = quiet;
  if (start === end) return '免打扰关着（全天都送）';
  if (start < end) return `${start}–${end} 点之间除待办与失败外不出站`;
  return `${start} 点–次日 ${end} 点之间除待办与失败外不出站`;
}

/** 节流窗口的一句话。 */
export function describeCooldown(sec: number): string {
  return sec === 0 ? '不节流（同类有多少发多少）' : `同类 ${sec} 秒内只出站一条`;
}
