/**
 * @vitest-environment node
 *
 * 跨语言共享表的 **Node 侧那一半**（票 host-policy/03，决策 246）。
 *
 * 问题：「什么算本机」在 Rust（`crates/core/src/host_policy.rs`）与前端
 * （`lib/localPage.ts::isLoopbackHostname`）各有一份实现——前端**调不了 Rust**
 * （页面 hostname 的判定可能发生在任何 API 调用之前），两份之间原本没有任何自动检查，
 * 规范漂移时前端会静默给出另一个答案（手机访问入口显隐、改绑判据随之分叉）。
 *
 * 修法：`tests/fixtures/host_policy_loopback.json` 把两侧钉在一起——Rust 侧
 * `host_policy` 的表测试与本文件**读同一份、同一断言方向**。任一侧改了规范另一侧没跟，
 * 落后的那一侧变红。
 *
 * **跑在 node 环境**（照 `lib/e2e-mock-fixture.test.ts` / `behavior-map.test.ts` 的先例）：
 * 默认 jsdom 下 `import.meta.url` 是 http 形态、`fileURLToPath` 会抛——而本用例要按
 * `import.meta.url` 定位仓库根的 fixture，且不碰 DOM。
 */

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

import { isLoopbackHostname } from './localPage';

/** fixture 定位走 `import.meta.url`（node 环境下是 file: 协议，见文件头注）。 */
const fixturePath = fileURLToPath(
  new URL('../../../tests/fixtures/host_policy_loopback.json', import.meta.url),
);

interface FixtureCase {
  input: string;
  loopback: boolean;
}

const fixture = JSON.parse(readFileSync(fixturePath, 'utf8')) as {
  cases: FixtureCase[];
};

describe('回环判定共享表（决策 246，与 Rust host_policy 同源）', () => {
  it('fixture 形状完好（两侧的断言对象还在）', () => {
    expect(fixture.cases.length).toBeGreaterThanOrEqual(20);
    // 放行 / 拒绝两侧都非空——谓词收窄是安全语义，只测一侧等于没测
    expect(fixture.cases.some((c) => c.loopback)).toBe(true);
    expect(fixture.cases.some((c) => !c.loopback)).toBe(true);
    // 必需行按名钉住（与 Rust 表测试同一把尺）：只数行数时，
    // 把 `::ffff:127.0.0.1` 换成任意多余行也能过 ≥20
    for (const required of [
      'localhost.',
      'LOCALHOST',
      '127.0.0.1.evil.test',
      '127.999.999.999',
      '0.0.0.0',
      '::ffff:127.0.0.1',
      '[::1].',
    ]) {
      expect(
        fixture.cases.some((c) => c.input === required),
        `缺必需行 ${required}`,
      ).toBe(true);
    }
  });

  it('逐行断言 isLoopbackHostname 与 fixture 一致（与 Rust 表测试同一方向）', () => {
    for (const { input, loopback } of fixture.cases) {
      expect(isLoopbackHostname(input), JSON.stringify(input)).toBe(loopback);
    }
  });
});
