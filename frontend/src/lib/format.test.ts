/**
 * 时间格式的唯一出处（票 15 / R2-18）。
 *
 * 起因是市场页用裸 `toLocaleString()`：非 zh-CN 的浏览器上，同一页会同时出现
 * `2026/9/18 15:04:05`（阶段页那一档）与 `9/18/2026, 3:04:05 PM`（市场那一档）——
 * 两处各写一遍的必然结果。
 *
 * 这一组因此钉两件事：① 格式化函数本身的行为；② **静态扫描**——`src/` 里除
 * `lib/format.ts` 之外不许再出现时间/日期的 locale 调用（数字的千分位分组
 * `toLocaleString('en-US')` 是另一件事，不在此列，但也必须显式给 locale）。
 */

import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative, resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { formatClock, formatClockAt, formatDateTime } from './format';

const srcRoot = resolve(process.cwd(), 'src');

function walk(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const p = join(dir, name);
    return statSync(p).isDirectory() ? walk(p) : [p];
  });
}

function sourceFiles(): string[] {
  // 只看**生产源码**：测试文件里出现这些字面量是它自己的事（本文件就在做这件事）
  return walk(srcRoot).filter(
    (f) => (f.endsWith('.svelte') || f.endsWith('.ts')) && !f.endsWith('.test.ts'),
  );
}

/** 去掉注释，避免把说明文字判成代码。 */
function stripComments(text: string): string {
  return text.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '');
}

describe('时间格式化（票 15 / R2-18）', () => {
  it('时刻与日期时间都是 zh-CN + 24 小时制（不随浏览器 locale 变）', () => {
    expect(formatDateTime('2026-09-18T15:04:05Z')).toMatch(/^\d{4}\/\d{1,2}\/\d{1,2} \d{1,2}:\d{2}:\d{2}$/);
    expect(formatClock('2026-09-18T15:04:05Z')).toMatch(/^\d{1,2}:\d{2}:\d{2}$/);
    expect(formatClockAt(new Date('2026-09-18T15:04:05Z'))).toBe(formatClock('2026-09-18T15:04:05Z'));
  });

  it('取不到 / 解析不出来的值给一个占位，不抛也不给 Invalid Date', () => {
    expect(formatDateTime(null)).toBe('—');
    expect(formatDateTime('not a date')).toBe('—');
    expect(formatClock(undefined)).toBe('—');
  });
});

describe('时间格式只有一处出处（静态扫描）', () => {
  it('时间/日期的 locale 调用只许出现在 lib/format.ts', () => {
    const bad: string[] = [];
    for (const file of sourceFiles()) {
      const rel = relative(srcRoot, file).replaceAll('\\', '/');
      if (rel === 'lib/format.ts') continue;
      const text = stripComments(readFileSync(file, 'utf8'));
      // 只认「时刻 / 日期」这两个专用方法；`toLocaleString` 的日期用法由下一条拦
      for (const m of text.matchAll(/toLocale(?:Time|Date)String\s*\(/g)) {
        bad.push(`${rel}: ${m[0]}`);
      }
    }
    expect(bad).toEqual([]);
  });

  it('千分位数字必须显式给 locale，不许裸 toLocaleString()', () => {
    const bad: string[] = [];
    for (const file of sourceFiles()) {
      const rel = relative(srcRoot, file).replaceAll('\\', '/');
      const text = stripComments(readFileSync(file, 'utf8'));
      // 裸调用 = 后面紧跟 `)` 或只有空白——那正是「随浏览器 locale 变」的那种写法
      for (const m of text.matchAll(/toLocaleString\s*\(\s*\)/g)) {
        bad.push(`${rel}: ${m[0]}`);
      }
    }
    expect(bad).toEqual([]);
  });

  /**
   * 上一轮评审当场指出的漏洞：`toLocaleString('en-US', { hour: '2-digit' })` —— **就是
   * R2-18 那处分叉**（市场页拿 `toLocaleString` 当日期用），却因为「显式给了 locale」而
   * 从上面两条里漏过去。日期/时刻类的 option 键才是判据：带这些键的 `toLocaleString`
   * 仍然是格式化时间，一律归 `lib/format.ts`。
   */
  it('拿 toLocaleString 当日期用的（带小时/年月日等 option）也归一处', () => {
    const DATE_OPTS = /\b(hour|minute|second|hour12|year|month|day|weekday|timeZone)\b/;
    const bad: string[] = [];
    for (const file of sourceFiles()) {
      const rel = relative(srcRoot, file).replaceAll('\\', '/');
      if (rel === 'lib/format.ts') continue;
      const text = stripComments(readFileSync(file, 'utf8'));
      for (const m of text.matchAll(/toLocaleString\s*\(([^)]*)\)/g)) {
        if (DATE_OPTS.test(m[1])) bad.push(`${rel}: ${m[0]}`);
      }
    }
    expect(bad).toEqual([]);
  });
});
