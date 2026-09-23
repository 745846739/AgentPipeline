/**
 * @vitest-environment node
 *
 * 跨语言共享表的 **Node 侧那一半**（票 mirror-contract/02，决策 253 ②）。
 *
 * 问题：前端 `api/types.ts` 的 `Stage` / `PendingKind` 是**枚举成员的手抄副本**，
 * 抄的正是后端 `crates/core/src/types.rs` 那两个枚举本身——没有任何测试读 Rust，
 * 抄的那份什么时候漂了没人知道。症状是「前端认得出一个后端认不出的值」
 * （`FromStr` 失败 / 格子不显示），而它不会以报错的形式出现。
 *
 * 修法：`tests/fixtures/enum_members.json` 把两侧钉在一起——本文件断言两个成员表与它
 * **集合相等**（不是「包含」：多一个值也要红），Rust 侧表测试断言那份表仍是
 * `schema_for!` 从枚举导出的结果。两侧同一张表、同一断言方向，任何一侧的枚举变了
 * 另一侧没跟就变红（照 `hostPolicyFixture.test.ts` / `marketReposFixture.test.ts` 先例）。
 *
 * **跑在 node 环境**（照 `lib/e2e-mock-fixture.test.ts` / `hostPolicyFixture.test.ts` 的
 * 先例）：默认 jsdom 下 `import.meta.url` 是 http 形态、`fileURLToPath` 会抛——而本用例
 * 要按 `import.meta.url` 定位仓库根的 fixture，且不碰 DOM。
 */

import { describe, expect, it } from 'vitest';

import { PENDING_KIND_MEMBERS, STAGE_MEMBERS } from '../api/types';
import { readFixture } from './fixtures';

const fixture = readFixture<{
  stage: string[];
  pending_kind: string[];
}>('enum_members.json');

/** 必需行按名钉住（与 Rust 表测试同一把尺）：只数行数时，随便塞行多余值也能过。 */
const REQUIRED_STAGE = ['done', 'merge', 'review', 'init'];
const REQUIRED_PENDING = ['merge_approval', 'user_decision', 'timeout'];

describe('枚举成员表（票 mirror-contract/02，与 Rust 枚举同源）', () => {
  it('fixture 形状完好（两侧的断言对象还在）', () => {
    expect(fixture.stage.length).toBeGreaterThanOrEqual(10);
    expect(fixture.pending_kind.length).toBeGreaterThanOrEqual(9);
    for (const required of REQUIRED_STAGE) {
      expect(fixture.stage, `缺必需行 ${required}`).toContain(required);
    }
    for (const required of REQUIRED_PENDING) {
      expect(fixture.pending_kind, `缺必需行 ${required}`).toContain(required);
    }
    // `'foreman'` **不是** `Stage` 成员（它是配置键，票 03 的范围）：它若出现在这张表里，
    // 说明导出把「配置键」与「阶段」两件事搅在了一起。
    expect(fixture.stage).not.toContain('foreman');
  });

  it('两个成员表与 fixture 集合相等（多一个值也红）', () => {
    // 集合相等而不是包含：前端多认一个值 = 「前端认得出、后端认不出」，那正是要拦的方向。
    // 顺序也 pin——它是后端的声明序，本次导出顺手带上的。
    expect([...STAGE_MEMBERS]).toEqual(fixture.stage);
    expect([...PENDING_KIND_MEMBERS]).toEqual(fixture.pending_kind);
  });
});
