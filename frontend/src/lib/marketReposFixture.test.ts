/**
 * @vitest-environment node
 *
 * 跨语言共享表的 **Node 侧那一半**（票 23，决策 250 Q2）。
 *
 * 问题：「什么样的 `owner/repo` 合法」在 Rust（`repo.rs::RepoId::parse`）与前端
 * （`lib/marketRepos.ts`）各有一份实现——前端**调不了 Rust**（页面判定可能发生在任何
 * API 调用之前，与决策 246 同一个理由），两份之间原本只有文件头注释里的一句承诺
 * （「前端输出 ⊆ 后端接受集」），漂移时没有会变红的落点。
 *
 * 修法：`tests/fixtures/repo_id.json` 把两侧钉在一起——本文件断言
 * `normalizeRepo`/`validateRepo` 逐行产出表里的 `normalized`/`valid`；Rust 侧表测试
 * 断言**表里的 `normalized` 就是 `RepoId::parse` 接受的输入**。两侧同一张表、同一断言
 * 方向，串起来就是子集不变量的机器钉法（照 `hostPolicyFixture.test.ts` / 决策 246 先例）。
 *
 * **跑在 node 环境**（照 `lib/e2e-mock-fixture.test.ts` / `hostPolicyFixture.test.ts` 的
 * 先例）：默认 jsdom 下 `import.meta.url` 是 http 形态、`fileURLToPath` 会抛——而本用例
 * 要按 `import.meta.url` 定位仓库根的 fixture，且不碰 DOM。
 */

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

import { normalizeRepo, validateRepo } from './marketRepos';

/** fixture 定位走 `import.meta.url`（node 环境下是 file: 协议，见文件头注）。 */
const fixturePath = fileURLToPath(
  new URL('../../../tests/fixtures/repo_id.json', import.meta.url),
);

interface FixtureCase {
  input: string;
  normalized: string;
  valid: boolean;
}

const fixture = JSON.parse(readFileSync(fixturePath, 'utf8')) as {
  cases: FixtureCase[];
};

/** 必需行按名钉住（与 Rust 表测试同一把尺）：只数行数时，随便塞行多余输入也能过 ≥30。 */
const REQUIRED = [
  'www.github.com/Obra/Superpowers',
  'HTTPS://GITHUB.COM/obra/superpowers',
  'obra/superpowers.git/',
  'Obra/Superpowers',
  'obra',
  'obra//superpowers',
  'https://gitlab.com/obra/superpowers',
  'git@github.com:obra/superpowers',
  '中文/技能',
  '',
];

describe('RepoId 共享表（票 23，与 Rust RepoId::parse 同源）', () => {
  it('fixture 形状完好（两侧的断言对象还在）', () => {
    expect(fixture.cases.length).toBeGreaterThanOrEqual(30);
    // 合法 / 非法两侧都非空——只测一侧等于没测
    expect(fixture.cases.some((c) => c.valid)).toBe(true);
    expect(fixture.cases.some((c) => !c.valid)).toBe(true);
    // 归一真的干了活——全是 input === normalized 时，粘贴残留那一半等于没测
    expect(fixture.cases.some((c) => c.input !== c.normalized)).toBe(true);
    for (const required of REQUIRED) {
      expect(
        fixture.cases.some((c) => c.input === required),
        `缺必需行 ${JSON.stringify(required)}`,
      ).toBe(true);
    }
  });

  it('逐行断言 normalizeRepo/validateRepo 与 fixture 一致（与 Rust 表测试同一方向）', () => {
    for (const { input, normalized, valid } of fixture.cases) {
      expect(normalizeRepo(input), JSON.stringify(input)).toBe(normalized);
      expect(validateRepo(input) === null, JSON.stringify(input)).toBe(valid);
    }
  });
});
