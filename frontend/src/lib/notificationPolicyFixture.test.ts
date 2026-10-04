/**
 * @vitest-environment node
 *
 * 跨语言共享表的 **Node 侧那一半**（票 foreman-within-boundary 02，决策 268③）。
 *
 * 问题：礼貌语义（免打扰跨零点、每类 cooldown 严格小于、`failed` 白天恒发免节流）在
 * Rust（`crates/core/src/notify.rs`，离线 webhook）与本文件
 * （`notificationPolicy.ts`，浏览器 toast）各有一份实现——两侧服务的是同一个
 * 「什么时候该吵人」的规范，规范漂移时其中一侧会静默给出另一个答案。
 *
 * 修法照决策 246 的回环表先例：`tests/fixtures/notification_policy.json`
 * 把两侧钉在一起——Rust 侧的表测试与本文件**读同一份、同一断言方向**。
 *
 * 显式差异（记在 fixture `$comment` 与决策 268）：`notifyOn` 每类开关与 `cancelled`
 * 类是前端用户偏好面，后端没有——共享表只收两边共有的语义子集。
 *
 * **跑在 node 环境**（照 `hostPolicyFixture.test.ts` 先例）：按 `import.meta.url`
 * 定位仓库根的 fixture，不碰 DOM。
 */

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

import {
  DEFAULT_NOTIFICATION_POLICY,
  shouldNotify,
  type NotificationClass,
} from './notificationPolicy';

const fixturePath = fileURLToPath(
  new URL('../../../tests/fixtures/notification_policy.json', import.meta.url),
);

interface FixtureCase {
  id: string;
  cls: NotificationClass;
  hour: number;
  age_sec: number | null;
  expected: boolean;
}

const fixture = JSON.parse(readFileSync(fixturePath, 'utf8')) as {
  policy: { cooldown_sec: number; quiet_hours: [number, number] };
  cases: FixtureCase[];
};

describe('通知礼貌共享表（决策 268，与 Rust notify 同源）', () => {
  it('fixture 形状完好（两侧的断言对象还在）', () => {
    expect(fixture.cases.length).toBeGreaterThanOrEqual(20);
    expect(fixture.cases.some((c) => c.expected)).toBe(true);
    expect(fixture.cases.some((c) => !c.expected)).toBe(true);
    // 必需行按名钉住（与 Rust 表测试同一把尺）：只数行数时，
    // 随便补几行同侧断言也能过 ≥20
    for (const required of [
      'failed-noon-fresh',
      'pending-quiet-exempt',
      'cooldown-strict-300',
      'cooldown-299-blocks',
      'done-quiet-silenced',
      'done-hour8-not-quiet',
    ]) {
      expect(
        fixture.cases.some((c) => c.id === required),
        `缺必需行 ${required}`,
      ).toBe(true);
    }
    // cancelled / notifyOn 不进表（显式差异）——有人把它加进来了要在这里露出
    expect(
      fixture.cases.every((c) => c.cls !== 'cancelled'),
      'cancelled 是前端偏好面差异，不该进共享表',
    ).toBe(true);
    // foreman_reply 也不进表（决策 272③④ 的反方向差异：后端独有的类，前端没有
    // 回话完成的 SSE 事件）——照 cancelled 先例，两侧守卫谁悄悄加了谁变红。
    // `cls` 的静态类型是前端那个 NotificationClass，故按原始串比（ TS 会说无交集）。
    expect(
      fixture.cases.every((c) => (c.cls as string) !== 'foreman_reply'),
      'foreman_reply 是决策 272 的后端独有类，不该进共享表',
    ).toBe(true);
    // failed 的**夜间**行也不进表（决策 383 的显式分叉）：免打扰段内前端 toast 照旧
    // 恒发、出机器那条线静音累计补摘要——两侧答案不同，放进表必有一侧永远红。
    // 照 cancelled / foreman_reply 先例，两侧守卫谁悄悄加了谁变红。
    const quiet = fixture.policy.quiet_hours;
    const inQuiet = (hour: number) =>
      quiet[0] < quiet[1]
        ? hour >= quiet[0] && hour < quiet[1]
        : hour >= quiet[0] || hour < quiet[1];
    expect(
      fixture.cases.every((c) => c.cls !== 'failed' || !inQuiet(c.hour)),
      'failed 的免打扰段内行是决策 383 的两侧分叉，不该进共享表',
    ).toBe(true);
  });

  it('逐行断言 shouldNotify 与 fixture 一致（与 Rust 表测试同一方向）', () => {
    for (const { id, cls, hour, age_sec, expected } of fixture.cases) {
      const now = new Date(2026, 8, 12, hour, 0, 0);
      // `NotifyState` 的形状是 `{ lastNotifiedAt: {...} }`——「没发过」= 空对象而不是缺字段
      const state = {
        lastNotifiedAt:
          age_sec === null
            ? {}
            : { [cls]: now.getTime() - age_sec * 1000 },
      };
      const policy = {
        // 表内三类缺省都开；cancelled 根本不进表（见上一个用例的差异守卫）
        notifyOn: { pending: true, done: true, failed: true, cancelled: false },
        cooldownSec: fixture.policy.cooldown_sec,
        quietHours: fixture.policy.quiet_hours,
      };
      expect(shouldNotify(cls, now, state, policy), id).toBe(expected);
      // 顺手钉住 fixture 的 policy 与代码缺省没有漂（漂了表就形同虚设）
      expect(fixture.policy.cooldown_sec).toBe(
        DEFAULT_NOTIFICATION_POLICY.cooldownSec,
      );
      expect(fixture.policy.quiet_hours).toEqual([
        ...DEFAULT_NOTIFICATION_POLICY.quietHours,
      ]);
    }
  });
});
