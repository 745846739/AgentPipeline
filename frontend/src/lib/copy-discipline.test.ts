/**
 * 文案纪律兜底门（决策 199 裁决 ④ / 票 23）：**面向用户的文案里不出现内部决策编号**。
 *
 * 为什么要有兜底门：编号退场是逐页做的（各页所有者删自己页面上的编号），但没有一条
 * 全站检查的话，新写的文案还会把「（决策 N）」带回来——索引与规则最大的失败形态不是没建，
 * 是建完悄悄烂掉。这条门只认一条硬规则：`决策 N` 不得出现在**面向用户的位置**。
 *
 * 边界（199 裁决 ① / ②，照抄）：
 * - **代码注释、开发文档、测试文件里的编号照旧**——所以扫描先把注释剥掉（长度与行号
 *   不变），并且不扫测试文件（`*.test.ts` / `*.spec.ts` 与 `e2e/`）与 **bench 文件
 *   （`*.bench.ts`）**——后者与测试同类：它是跑给自己看的工具，`describe` 的标签从不渲染
 *   给用户（决策 361 票 04 引进第一份 bench 时暴露的漏网）。
 * - 编号的界面外归宿是 `design/frontend-design.md` §12.3「行为 / 规则 → 实现位置」表的
 *   **备注列**与代码注释（另有 DEC-IA 的悬空引用检查守那张表）。定稿文案里 `title` 悬停
 *   提示放的是**理由**（如 `title="有活跃任务的项目不能删除"`），**不夹编号**——故本门对
 *   `title` 属性与模板文本一视同仁。
 *
 * 形状：既有「静态扫描」家族（对照 `theme/css-parity.test.ts` 的全站扫描），
 * **不新增接缝、不为它改生产代码形状**。
 *
 * 第二条规则（ux-audit-3 票 08）：同一张扫描面、同一条边界，再钉「面向用户的文案里
 * 不出现字面 Markdown 强调星号 `**…**`」——设置两页走查当场的唯一新问题，剥注释后
 * 全站实测恰 4 处。判据天然放过掩码 `***`（`NOTIFY_SECRET_MASK`，有意设计）。
 */
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative, resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

/** vitest 从 `frontend/` 运行（vite.config.ts 的 include 是 src/**），故以 cwd 定位 src。 */
const srcRoot = resolve(process.cwd(), 'src');

/** 内部编号的形态：「决策 N」，含全角数字，含「决策 130 / 137」这类连写（整段算一处）。 */
const DECISION_REF = /决策\s*[0-9０-９]{1,3}(?:\s*[/、]\s*[0-9０-９]{1,3})*/g;

/**
 * 把注释字符换成空格——**长度与行号不变**，故命中位置还能对回原文。
 *
 * 三层：块注释（含 svelte 的 `{/* … *\/}`）→ 行注释（`//` 前是 `:` 的不算，
 * 否则 `https://…` 会把同一行后半句吃掉、漏报真命中）→ HTML 注释（仅 .svelte）。
 */
function maskComments(text: string, html: boolean): string {
  const blank = (source: string, re: RegExp): string =>
    source.replace(re, (m) => m.replace(/[^\n]/g, ' '));
  let out = blank(text, /\/\*[\s\S]*?\*\//g);
  out = blank(out, /(?<!:)\/\/[^\n]*/g);
  if (html) out = blank(out, /<!--[\s\S]*?-->/g);
  return out;
}

export interface DecisionRef {
  /** 1 起的行号（对回原文的行）。 */
  readonly line: number;
  /** 命中的编号文本。 */
  readonly match: string;
  /** 命中所在行的原文摘录（截断，便于定位）。 */
  readonly snippet: string;
}

/** 在一段源码里找面向用户的内部编号（注释已剥）。 */
function findDecisionRefs(text: string, html: boolean): DecisionRef[] {
  const masked = maskComments(text, html);
  const rawLines = text.split('\n');
  const out: DecisionRef[] = [];
  for (const m of masked.matchAll(DECISION_REF)) {
    const line = masked.slice(0, m.index).split('\n').length;
    out.push({ line, match: m[0], snippet: (rawLines[line - 1] ?? '').trim().slice(0, 110) });
  }
  return out;
}

/**
 * 字面 Markdown 强调星号（行内）：`**文本**`（ux-audit-3 票 08 的机器门）。
 *
 * 判据与审计 ⑤.2 的订正同源：`[^*\n]+` 至少要一个**非星、非换行**字符——
 * 掩码 `***`（`NOTIFY_SECRET_MASK`，是有意设计、不算缺陷）天然不命中，
 * 跨行断开的 `**` 也不命中；`***text***` 只取中段的 `**text**`（那仍是字面强调）。
 */
const LITERAL_BOLD = /\*\*[^*\n]+\*\*/g;

export interface LiteralStar {
  /** 1 起的行号（对回原文的行）。 */
  readonly line: number;
  /** 命中所在行的原文摘录（截断，便于定位）。 */
  readonly snippet: string;
}

/** 在一段源码里找面向用户的字面强调星号（注释已剥）。 */
export function findLiteralStars(text: string, html: boolean): LiteralStar[] {
  const masked = maskComments(text, html);
  const rawLines = text.split('\n');
  const out: LiteralStar[] = [];
  for (const m of masked.matchAll(LITERAL_BOLD)) {
    const line = masked.slice(0, m.index).split('\n').length;
    out.push({ line, snippet: (rawLines[line - 1] ?? '').trim().slice(0, 110) });
  }
  return out;
}

/** 收集扫描面：src 下的 `.svelte` 与 `.ts`，**测试与 bench 文件除外**（199：那里的编号照旧）。 */
function collectCopyFiles(dir: string): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) {
      if (name === 'node_modules') continue;
      out.push(...collectCopyFiles(p));
    } else if (/\.(svelte|ts)$/.test(name) && !/\.(test|spec|bench)\.ts$/.test(name)) {
      out.push(p);
    }
  }
  return out;
}

const files = collectCopyFiles(srcRoot);

describe('文案纪律 · 面向用户的文案里不出现内部编号（决策 199 / 票 23）', () => {
  it('扫描面是 src 下的 .svelte 与 .ts（测试文件与各自的注释不在内）', () => {
    expect(files.length).toBeGreaterThan(50);
    expect(files.some((f) => /\.(test|spec|bench)\.ts$/.test(f))).toBe(false);
    expect(files.every((f) => /\.(svelte|ts)$/.test(f))).toBe(true);
  });

  it('全站没有一处「决策 N」落在面向用户的位置', () => {
    const hits: string[] = [];
    for (const file of files) {
      const rel = relative(srcRoot, file).replaceAll('\\', '/');
      for (const ref of findDecisionRefs(readFileSync(file, 'utf8'), file.endsWith('.svelte'))) {
        hits.push(`${rel}:${ref.line}: 面向用户的文案里出现「${ref.match}」｜ ${ref.snippet}`);
      }
    }
    // 编号的界面外归宿：design/frontend-design.md §12.3 行为映射表的「备注」列 + 代码注释。
    expect(hits).toEqual([]);
  });
});

describe('文案纪律 · 扫描口径（注释 / 测试文件照旧，文案里才算）', () => {
  it('代码注释里的编号不算（注释照旧）', () => {
    expect(findDecisionRefs('// 决策 101：并发动作的提交口径', false)).toEqual([]);
    expect(findDecisionRefs('/* 决策 101\n   续行 */', false)).toEqual([]);
    expect(findDecisionRefs('<!-- 决策 169 / theme-6-pixel.md §3 -->', true)).toEqual([]);
    expect(findDecisionRefs('{/* 决策 105 的说明 */}\n<p>正文</p>', true)).toEqual([]);
  });

  it('模板文本与字符串字面量里的编号算（含 title 悬停提示）', () => {
    expect(findDecisionRefs('<p>不能删除（决策 101）。</p>', true)).toHaveLength(1);
    expect(findDecisionRefs('throw new Error(`动作无配对端点（决策 101）`)', false)).toHaveLength(1);
    expect(findDecisionRefs('<span title="不受支持（决策 103）">!</span>', true)).toHaveLength(1);
  });

  it('报得准：文件行号与命中文本都有', () => {
    const hits = findDecisionRefs('<p>第一行</p>\n<p>理由（决策 7 / 决策 8）</p>', true);
    expect(hits.map((h) => h.line)).toEqual([2, 2]);
    expect(hits.map((h) => h.match)).toEqual(['决策 7', '决策 8']);
    expect(hits[0].snippet).toContain('理由（决策 7 / 决策 8）');
  });

  it('括注里连写两个编号、全角数字也认得（连写整段算一处）', () => {
    expect(findDecisionRefs('<p>口径见 core metrics（决策 130 / 137）</p>', true).map((h) => h.match)).toEqual([
      '决策 130 / 137',
    ]);
    expect(findDecisionRefs('<p>（决策 １０１）</p>', true)).toHaveLength(1);
  });

  it('URL 里的 `//` 不被当成注释起点（否则同一行后半句会漏报）', () => {
    expect(findDecisionRefs('<a href="https://example.test/x">说明（决策 9）</a>', true)).toHaveLength(1);
  });

  it('开发文档不受影响：这条门只扫前端源码（设计规格里的编号由别的检查管）', () => {
    expect(files.every((f) => f.startsWith(srcRoot))).toBe(true);
  });
});

describe('文案纪律 · 面向用户的文案里不出现字面 Markdown 强调星号（ux-audit-3 票 08）', () => {
  it('全站没有一处 `**…**` 落在面向用户的位置（hits === []）', () => {
    const hits: string[] = [];
    for (const file of files) {
      const rel = relative(srcRoot, file).replaceAll('\\', '/');
      for (const star of findLiteralStars(readFileSync(file, 'utf8'), file.endsWith('.svelte'))) {
        hits.push(`${rel}:${star.line}: 面向用户的文案里出现字面强调星号｜ ${star.snippet}`);
      }
    }
    // 走查当场全站实测 4 处（SettingsNotify 577/589/676 + Tools 160），修完归零。
    expect(hits).toEqual([]);
  });

  it('注释照旧不算 / 测试文件不在扫描面（199 同一边界）', () => {
    expect(findLiteralStars('// **照旧**', false)).toEqual([]);
    expect(findLiteralStars('/* **照旧**\n   续行 */', false)).toEqual([]);
    expect(findLiteralStars('<!-- **照旧** -->', true)).toEqual([]);
    expect(findLiteralStars('{/* **照旧** */}\n<p>正文</p>', true)).toEqual([]);
    // 测试与 bench 文件不在扫描面（与编号门共用 collectCopyFiles 的同一份清单）
    expect(files.some((f) => /\.(test|spec|bench)\.ts$/.test(f))).toBe(false);
    // 实证：SettingsNotify 文件头注释里仍有 `**`（注释照旧），但面向用户位置 0 命中
    const notify = files.find((f) => f.endsWith('SettingsNotify.svelte'));
    expect(notify).toBeDefined();
    const raw = readFileSync(notify!, 'utf8');
    expect(raw).toContain('**');
    expect(findLiteralStars(raw, true)).toEqual([]);
  });

  it('掩码 `***` 是有意设计，不计（回归审计 ⑤.2 的订正）', () => {
    expect(findLiteralStars('<p>读回 *** </p>', true)).toEqual([]);
    expect(findLiteralStars("const NOTIFY_SECRET_MASK = '***';", false)).toEqual([]);
    expect(findLiteralStars('<li>***（掩码）</li>', true)).toEqual([]);
  });

  it('模板正文里的算（正例：`<p>保存的是**整体覆盖**</p>` → 1 处，带行号与 snippet）', () => {
    const hits = findLiteralStars('<p>第一行</p>\n<p>保存的是**整体覆盖**</p>', true);
    expect(hits).toHaveLength(1);
    expect(hits[0].line).toBe(2);
    expect(hits[0].snippet).toContain('保存的是**整体覆盖**');
    // .ts 字符串字面量同样算（不是只有模板才报）
    expect(findLiteralStars("note('闸门**不改写**')", false)).toHaveLength(1);
  });
});
