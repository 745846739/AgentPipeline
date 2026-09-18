/**
 * 「899 一处定义」的机器形式（决策 218 修订 ⑥ / 票 07）。
 *
 * CSS 读不到 TS 常量，故 `Talk.svelte` 的 `@media` 里必然有一个字面量。本用例把那个字面量
 * 与 {@link TALK_FOLD_MAX} **逐字比对**，并禁止组件里再出现第三个断点值——「折行档 / 窄档 /
 * `forceFold` 三者同源」因此是可以变红的一条断言，而不是注释里的一句保证。
 *
 * 扫描面：`routes/Talk.svelte` 的源码文本（纯静态，不跑 Svelte 编译器）。属既有「静态扫描」
 * 家族（`css-parity.test.ts` / `copy-discipline.test.ts`）。
 *
 * @vitest-environment node
 */
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import {
  TALK_FOLD_MAX,
  TALK_FOLD_QUERY,
  TALK_MOBILE_MAX,
  TALK_SPLIT_NARROW_MAX,
} from './talkLayout';

const TALK_PATH = resolve(process.cwd(), 'src/routes/Talk.svelte');
const source = readFileSync(TALK_PATH, 'utf8');

/** 源码里出现过的所有 `@media (max-width: Npx)` / `(min-width: Npx)` 的 N。 */
function breakpointsIn(text: string): number[] {
  return [
    ...[...text.matchAll(/max-width:\s*(\d+)px/g)].map((m) => Number(m[1])),
    ...[...text.matchAll(/min-width:\s*(\d+)px/g)].map((m) => Number(m[1])),
  ];
}

describe('对讲台的断点只有这三个、且都由本模块定义', () => {
  it('折行档上界是 899——它不是移动款那个 479', () => {
    expect(TALK_FOLD_MAX).toBe(899);
    expect(TALK_FOLD_QUERY).toBe('(max-width: 899px)');
    expect(TALK_MOBILE_MAX).toBe(479);
    expect(TALK_SPLIT_NARROW_MAX).toBe(1099);
  });

  it('Talk.svelte 的 `@media` 用的就是这三个数（899 / 479 / 1099），没有第四个', () => {
    const found = [...new Set(breakpointsIn(source))].sort((a, b) => a - b);
    expect(found, `Talk.svelte 里的断点集合：${found.join(' / ')}`).toEqual([
      TALK_MOBILE_MAX,
      TALK_FOLD_MAX,
      TALK_SPLIT_NARROW_MAX,
    ]);
  });

  it('折行档那一档（899）确实在组件里有规则，不是只写在常量里', () => {
    expect(source).toContain(`@media (max-width: ${TALK_FOLD_MAX}px)`);
  });
});
