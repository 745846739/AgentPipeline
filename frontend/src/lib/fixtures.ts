/**
 * 共享 fixture 的读取 helper（票 mirror-contract 02 / 03）。
 *
 * 两侧的跨语言共享表都住在仓库根的 `tests/fixtures/`，而 vitest 用例要按
 * `import.meta.url` 定位它们。三处（`enumMembersFixture` / `specTablesFixture` /
 * 既有的 `hostPolicyFixture` 与 `marketReposFixture`）各写一遍这段样板是同一个形状的重复；
 * 本 module 把它收成一处，并**在此处集中记下那条容易踩的约束**。
 *
 * **必须在 node 环境下用**（用例头写 `@vitest-environment node`）：默认的 jsdom 里
 * `import.meta.url` 是 http 形态，`fileURLToPath` 会抛——而这里要的正是 file: 协议下的
 * 仓库根相对定位（照 `lib/e2e-mock-fixture.test.ts` / `behavior-map.test.ts` 先例）。
 *
 * 只看不写：这里没有「重新生成 fixture」的路径。表是**消费物**，改它的正确做法是改
 * Rust 侧那个权威（枚举 / 源码文本），再按测试失败报文里印出的 JSON 更新。
 */

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

/** 仓库根 `tests/fixtures/` 下某个 fixture 的绝对路径。 */
export function fixturePath(name: string): string {
  return fileURLToPath(new URL(`../../../tests/fixtures/${name}`, import.meta.url));
}

/** 读一个共享 fixture 并解析。`T` 是要断言的形状——调用方自己声明，不做运行时校验。 */
export function readFixture<T>(name: string): T {
  return JSON.parse(readFileSync(fixturePath(name), 'utf8')) as T;
}
