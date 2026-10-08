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
 *
 * 第三条规则（本票扩面，决策 199 的落地）：**半中半英**——A-1「含汉字 + snake_case 内部符号」、
 * A-2「含汉字 + 阶段 id（`architect-design` / `develop-design` / `test-design` / `sync-check` /
 * `validate_output`）或独立词 `develop|review|test`」。既有两条规则管不到这类（它们不含「决策 N」）；
 * 扫描面是标签间文本 + **任意属性值**（组件文本型 props——`state=` / `next=` / `linkLabel=` 这类
 * 渲染成屏上正文的 prop 与标准属性是同一种文案，按名字白名单堵会留下「注进门不变红」的面，
 * 评审场景 6 实证）+ 字符串字面量。**逐段逐 token 报告、豁免按 token 逐命中匹配**（段级匹配会让
 * 同段的豁免符号掩蔽真违例，评审实证的缺口）。产物文件名（`review-diff.diff` / `test-report.md`
 * / `test_result.json`）在 A-1 / A-2 两条判据上**一致负向放过**（B5 边界由排除本身承担，
 * 登记表不留行使不到的死条目）。
 *
 * 第四条规则（同一张扩面的后端半边）：**API 报文与落库渲染字段也是页面文案**（toast /
 * `.reg-err` / dossier / 现场页签 / TimelineView 读它们），而既有门只扫 `frontend/src`。规则 4 只扫
 * **三类构造形态**的字符串字面量——① `ApiError::…("…")` / `Error::…("…")` / `error: Some("…")`
 * ② `test_blockers` 类载荷 push ③ **流转原因载荷**（`reason = Some(…)` 赋值与
 * `insert_transition(…, Some("…"))` 实参——reason 落 `kanban_transitions` 后由 TimelineView 原文渲染，
 * 评审场景 5 实证：resume.rs 的 `（决策 116）` 正是从这个缺口溜过的）。
 * `tracing::` 日志、`#[test]` 断言消息、system prompt、CLI `--help` 不在形态内（口径用例钉住，
 * 防规则烂成误报）。**整库搜改是红线**，门与改法同源同口径。
 *
 * **B 类边界登记表**（规则 3 的豁免，只收 B 类、不收「暂时不想改」）：
 * B1 键名标签与键名校验 / B2 动作句键名引用 / B3 域词表词（词表收录的工具名等）/
 * B4 mono 读数徽章 / B5 产物文件名作对照（**不由登记表承担**：`FILENAME_EXT` /
 * `HYPHEN_TOKEN` 两形态在 A-1 / A-2 两判据上一致剔除产物文件名——但连词剔除在 `STAGE_ID`
 * 扫描**之后**，否则会把 `develop-design` 这类阶段 id 一起吃掉，登记表不收行使不到的死条目）。
 * 条目数入断言，
 * 每条另有活性断言，让「悄悄烂掉」可见。
 *
 * **判别问句**（登记表每条理由都是它的答案）：删掉这个符号，这句话还说清会发生什么、
 * 你该做什么吗？说得清 → 摘；说不清 → 留（B 类，进登记表）但同句要有人话。
 */
import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
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

/* ══════════════════════ 规则 3：前端半中半英（A-1 / A-2）══════════════════════ */

/** 汉字判据：段内有汉字才可能是「人话句里夹符号」（纯英文键名标签天然放过）。 */
const HAN = /\p{Script=Han}/u;
/** A-1：snake_case 内部符号（小写字母开头、至少一段 `_`）。 */
const SNAKE = /\b[a-z][a-z0-9]*(?:_[a-z0-9]+)+\b/;
/** A-2：阶段 / 节点 id 全称。 */
const STAGE_ID = /\b(?:architect-design|develop-design|test-design|sync-check|validate_output)\b/;
/** A-2：独立词形态的阶段名。 */
const STAGE_WORD = /\b(?:develop|review|test)\b/;
/** A-2 负向排除（B5，形态一）：带扩展名的产物文件名——在**所有判据之前**剔：`sync-check.md` 这类
 *  「阶段 id 形状的文件名」整体是文件名（B5 对照物），不是阶段 id。 */
const FILENAME_EXT =
  /\b[\w-]+\.(?:diff|md|log|json|txt|ts|js|yaml|yml|toml|html|css|png|svg)\b/g;
/** A-2 负向排除（B5，形态二）：连词 token（`review-diff` / `stage-name` 连写）——必须在 `STAGE_ID`
 *  扫描**之后**才剔：其备选 `(?:review|test|develop|architect)-[\w-]+` 会把 `architect-design` /
 *  `develop-design` / `test-design` 整体吃掉（评审实证的遮蔽缺口），先剔则这三枚阶段 id
 *  在含汉字句里永远报不出。 */
const HYPHEN_TOKEN = /\b(?:review|test|develop|architect)-[\w-]+\b/g;

interface CopySegment {
  /** 段文本（注释已剥）。 */
  readonly seg: string;
  /** 段在原文中的字节偏移（maskComments 等长，行号可回对原文）。 */
  readonly offset: number;
}

/**
 * 渲染近似：用户看到的是**值**不是表达式——字面段照留，`{…}` / `${…}` 表达式只取其中的
 * 字符串字面量（`base_url {x ?? '默认'}` 渲染出 `默认`，`评审：{task.review_mode}`
 * 渲染出的是值、不带字段名）。这样 `评审：{task.review_mode}` 不误报、
 * `base_url 默认` 这类徽章如实报。
 */
function renderedApprox(seg: string): string {
  let out = '';
  let depth = 0;
  let cur = '';
  for (let i = 0; i < seg.length; i++) {
    const c = seg[i];
    if (c === '{' || (c === '$' && seg[i + 1] === '{')) {
      if (depth === 0) {
        out += cur;
        cur = '';
      }
      if (c === '$') i++;
      depth++;
      continue;
    }
    if (depth > 0 && c === '}') {
      depth--;
      if (depth === 0) {
        for (const sm of cur.matchAll(/'([^']*)'|"([^"]*)"/g)) out += `${sm[1] ?? ''}${sm[2] ?? ''}`;
        cur = '';
      }
      continue;
    }
    if (depth > 0) cur += c;
    else out += c;
  }
  // 不平衡兜底：同样只取表达式里的字符串字面量（不把裸代码当文案）
  if (depth > 0) for (const sm of cur.matchAll(/'([^']*)'|"([^"]*)"/g)) out += `${sm[1] ?? ''}${sm[2] ?? ''}`;
  return out;
}

/**
 * 从（已剥注释的）源码里提取字符串字面量段。
 * 单/双引号遇到换行即判未闭合（防孤立 `'` 吞掉大段代码）；模板字面量允许跨行，
 * 且按 `${…}` 深度配平——嵌套反引号（`… ${f ? 'a' : `b${x}`}`）整段算一个模板。
 */
function extractStringLiterals(masked: string, out: CopySegment[], base = 0): void {
  let i = 0;
  while (i < masked.length) {
    const c = masked[i];
    if (c === "'" || c === '"') {
      const start = i;
      let j = i + 1;
      while (j < masked.length && masked[j] !== c && masked[j] !== '\n') {
        if (masked[j] === '\\') j++;
        j++;
      }
      if (j < masked.length && masked[j] === c) {
        out.push({ seg: masked.slice(start + 1, j), offset: base + start + 1 });
        i = j + 1;
      } else {
        i = start + 1; // 未闭合：跳过这枚引号，不当段
      }
      continue;
    }
    if (c === '`') {
      const start = i;
      let j = i + 1;
      let depth = 0;
      while (j < masked.length) {
        if (masked[j] === '\\') {
          j += 2;
          continue;
        }
        if (masked[j] === '$' && masked[j + 1] === '{') {
          depth++;
          j += 2;
          continue;
        }
        if (depth > 0 && masked[j] === '}') {
          depth--;
          j++;
          continue;
        }
        if (depth === 0 && masked[j] === '`') break;
        j++;
      }
      if (j < masked.length) {
        out.push({ seg: masked.slice(start + 1, j), offset: base + start + 1 });
        i = j + 1;
      } else {
        i = start + 1;
      }
      continue;
    }
    i++;
  }
}

/**
 * 提取一份源码里**面向用户的 copy 段**（注释已剥）：
 * `.svelte` = 标签间文本 + **任意属性值**（标准属性与组件文本型 props 同类，见 `attrRe`）
 * + script 内字符串（`<style>` 不进扫描面）；`.ts` = 字符串与模板字面量。
 */
function extractCopySegments(text: string, html: boolean): { masked: string; segments: CopySegment[] } {
  const masked = maskComments(text, html);
  const segments: CopySegment[] = [];
  if (!html) {
    extractStringLiterals(masked, segments);
    return { masked, segments };
  }
  const blockRe = /(<script[^>]*>[\s\S]*?<\/script>)|(<style[^>]*>[\s\S]*?<\/style>)/g;
  const blocks = [...masked.matchAll(blockRe)].map((m) => ({
    start: m.index,
    end: m.index + m[0].length,
    script: !!m[1],
  }));
  const blockAt = (i: number) => blocks.find((b) => i >= b.start && i < b.end);
  // 标签间文本（跳过 script / style 区间）
  let inTag = false;
  let buf = '';
  let bufStart = 0;
  let wasSkip = false;
  for (let i = 0; i < masked.length; i++) {
    const b = blockAt(i);
    if (b) {
      if (buf.trim()) segments.push({ seg: buf, offset: bufStart });
      buf = '';
      wasSkip = true;
      continue;
    }
    if (wasSkip) {
      wasSkip = false;
      bufStart = i;
    }
    const c = masked[i];
    if (c === '<') {
      if (buf.trim()) segments.push({ seg: buf, offset: bufStart });
      buf = '';
      inTag = true;
      continue;
    }
    if (c === '>' && inTag) {
      inTag = false;
      bufStart = i + 1;
      continue;
    }
    if (!inTag) {
      if (!buf) bufStart = i;
      buf += c;
    }
  }
  if (buf.trim()) segments.push({ seg: buf, offset: bufStart });
  // 任意属性值（引号值与 {...} 表达式值）——**组件文本型 props**（`state=` / `next=` /
  // `linkLabel=` 这类渲染成屏上正文的 prop）与标准属性是同一种文案；按名字白名单只堵
  // 已知名字，新组件的文本 prop 注入门不会变红（评审场景 6 实证）。名字侧排除空白与
  // `"'=<>/{}，避免把标签间文本里的 `x = "…"` 误当属性。
  const attrRe = /[^\s"'=<>/{}]+\s*=\s*(?:"([^"]*)"|\{([^{}]*)\}|'([^']*)')/g;
  for (const m of masked.matchAll(attrRe)) {
    if (blockAt(m.index)) continue;
    // `{…}` 表达式值**连花括号一起进段**：renderedApprox 靠花括号把表达式剥掉、只留其中的
    // 字符串字面量——直接推进裸表达式会把 `source={task.pending_reason?.message ?? '需要…'}`
    // 里的字段名当成文案（评审后补的实证假阳性）；值本身是字面量时照样如实报。
    const raw = m[1] ?? m[3];
    segments.push({ seg: raw !== undefined ? raw : `{${m[2]}}`, offset: m.index });
  }
  // script 内的字符串
  for (const b of blocks) {
    if (b.script) extractStringLiterals(masked.slice(b.start, b.end), segments, b.start);
  }
  return { masked, segments };
}

export interface HalfMixedHit {
  /** 1 起的行号。 */
  readonly line: number;
  /** 命中哪条判据。 */
  readonly rule: 'A-1' | 'A-2';
  /** 命中的符号。 */
  readonly token: string;
  /** 渲染近似文本（截断，供登记表匹配与定位）。 */
  readonly text: string;
}

/**
 * 在一份源码里找「人话句里夹内部符号」（注释已剥）——**逐段、逐 token** 报告：
 * 一段里有几枚符号就报几条（段级「只报首个命中」会让第二枚符号无人看，评审实证的缺口），
 * 豁免才能按 token 逐命中比对。
 *
 * **扫描顺序（评审实证的遮蔽缺口）**：① 先剔**带扩展名**的产物文件名（`FILENAME_EXT`，
 * 连 `sync-check.md` 这类阶段 id 形状的文件名一起放过，B5 不破）；② 对 rendered 扫 `STAGE_ID`
 * 挖空——`HYPHEN_TOKEN` 的连词备选 `(?:review|test|develop|architect)-[\w-]+` 会把
 * `architect-design` / `develop-design` / `test-design` 整体吃掉，若在阶段 id 之前剔它，
 * 这三枚 id 在含汉字句里永远报不出（`sync-check` 不带该前缀所以独存、`validate_output`
 * 由 snake 判据兼报）；③ 再剔连词 token（`HYPHEN_TOKEN`）；④ 最后扫 SNAKE 与独立词——
 * A-1 与独立词判据仍在文件名负向排除之后，`review-diff.diff` / `test-report.md` 照旧放过。
 */
export function findHalfMixed(text: string, html: boolean): HalfMixedHit[] {
  const { masked, segments } = extractCopySegments(text, html);
  const out: HalfMixedHit[] = [];
  for (const { seg, offset } of segments) {
    const rendered = renderedApprox(seg);
    if (!HAN.test(rendered)) continue;
    const loc = lineOf(masked, offset);
    const snippet = rendered.trim().slice(0, 110);
    // 命中即把该区间挖空（等长替换，偏移不漂）：同一符号不跨判据重复报
    //（`validate_output` 兼是 snake 与阶段 id，归 A-2 阶段 id），也不被独立词判据二次命中。
    let rest = rendered.replace(new RegExp(FILENAME_EXT.source, 'g'), ' ');
    const sweep = (re: RegExp, rule: 'A-1' | 'A-2'): void => {
      for (const m of rest.matchAll(new RegExp(re.source, 'g'))) {
        out.push({ ...loc, rule, token: m[0], text: snippet });
        rest = rest.slice(0, m.index) + ' '.repeat(m[0].length) + rest.slice(m.index + m[0].length);
      }
    };
    sweep(STAGE_ID, 'A-2');
    rest = rest.replace(new RegExp(HYPHEN_TOKEN.source, 'g'), ' ');
    sweep(SNAKE, 'A-1');
    sweep(STAGE_WORD, 'A-2');
  }
  return out;
}

function lineOf(masked: string, offset: number): { line: number } {
  return { line: masked.slice(0, offset).split('\n').length };
}

/**
 * B 类豁免登记表——**只收 B1–B4，不收「暂时不想改」**。
 * 每条 = 位置（相对 `src/` 的文件）+ 命中的 token + B 类编号 + 一句理由
 * （理由即判别问句的答案）。条目数入断言（见规则 3 的用例），改了文案导致条目失效同样会红；
 * 另有「每条都被行使」的活性断言——登记表不收行使不到的死条目
 * （B5 产物文件名由 `FILENAME_EXT` / `HYPHEN_TOKEN` 在 A-1 / A-2 两判据的一致负向排除承担，
 * 不在此表）。
 */
interface CopyExemption {
  readonly file: string;
  readonly match: string;
  readonly boundary: 'B1' | 'B2' | 'B3' | 'B4';
  readonly reason: string;
}

const EXEMPTIONS: readonly CopyExemption[] = [
  // B1 键名标签与键名校验：键名即「在填哪一项 / 哪个键不合格」，摘掉说不清
  { file: 'components/settings/ProjectForm.svelte', match: 'local_path', boundary: 'B1', reason: '表单键名标签：标签主体就是配置键，键名即「在填哪一项」。' },
  { file: 'components/settings/ProviderForm.svelte', match: 'context_window', boundary: 'B1', reason: '表单键名标签：键名与输入框一一对应，摘掉悬空。' },
  { file: 'components/settings/ProviderForm.svelte', match: 'base_url', boundary: 'B1', reason: '表单键名标签：可选项说明挂在键名后，键名是主语。' },
  { file: 'components/settings/StageConfigForm.svelte', match: 'provider_id', boundary: 'B1', reason: '表单键名标签：留空 / 填写规则直接挂在键名后。' },
  { file: 'components/settings/StageConfigForm.svelte', match: 'persona_path', boundary: 'B1', reason: '表单键名标签：路径格式要求挂在键名后，摘键名不知指哪项。' },
  { file: 'components/settings/StageConfigForm.svelte', match: 'persona_append', boundary: 'B1', reason: '表单键名标签：追加指令的可选项说明挂在键名后。' },
  { file: 'lib/stageConfigs.ts', match: 'max_tokens', boundary: 'B1', reason: '键名校验：报错必须点名是哪个键不合格。' },
  { file: 'lib/stageConfigs.ts', match: 'idle_timeout_sec', boundary: 'B1', reason: '键名校验：负值报错点名键，用户才知道改哪一格。' },
  { file: 'lib/stageConfigs.ts', match: 'max_duration_sec', boundary: 'B1', reason: '键名校验：负值报错点名键。' },
  { file: 'lib/stageConfigs.ts', match: 'max_rounds', boundary: 'B1', reason: '键名校验：正整数要求点名键。' },
  { file: 'lib/stageConfigs.ts', match: 'watch_token_budget', boundary: 'B1', reason: '键名校验：正整数要求点名键。' },
  { file: 'lib/stageConfigs.ts', match: 'skills_json', boundary: 'B1', reason: '键名校验：结构错误点名键并给改法。' },
  { file: 'lib/stageConfigs.ts', match: 'node_overrides_json', boundary: 'B1', reason: '键名校验：JSON 结构错误点名键。' },
  { file: 'lib/providers.ts', match: 'context_window', boundary: 'B1', reason: '键名校验：值域错误点名键，用户才知道改哪一格。' },
  // B2 动作句键名引用：指令的宾语就是这个键（句式「base_url 要留空 / 写成…」）
  { file: 'lib/providers.ts', match: 'base_url', boundary: 'B2', reason: '动作句键名引用：「要留空 / 写成…」操作的对象就是这个键，摘掉不知道改什么。' },
  // B3 域词表词：词表收录的工具名，句义依赖其名
  { file: 'routes/SettingsTools.svelte', match: 'run_command', boundary: 'B3', reason: '域词表词：run_command 是词表收录的工具名（白名单模式条目），主语即它。' },
  { file: 'routes/SettingsTools.svelte', match: 'offload_run', boundary: 'B3', reason: '域词表词：offload_run 是外发动作面的工具名，说清「只认显式调」靠它。' },
  // B4 mono 读数徽章：键名 + 值的读数形制
  { file: 'routes/SettingsProviders.svelte', match: 'base_url', boundary: 'B4', reason: 'mono 读数徽章：键名是徽章的固定前缀，值跟在其后。' },
  { file: 'routes/SettingsStages.svelte', match: 'max_tokens', boundary: 'B4', reason: 'mono 读数徽章：temp / max_tokens 读数并排，键名即读数标签。' },
];

/** 豁免判定的唯一实现：**按（文件, token）逐命中匹配**——段级 `text.includes` 会让同段的豁免符号掩蔽真违例。 */
function isExempt(rel: string, token: string): boolean {
  return EXEMPTIONS.some((e) => e.file === rel && e.match === token);
}

describe('文案纪律 · 规则 3：人话句里不夹内部符号（半中半英 A-1 / A-2）', () => {
  it('口径正例：A-1 snake_case 夹在汉字句里命中，A-2 阶段词命中', () => {
    const a1 = findHalfMixed('<p>以下为 project_analysis 探测到的事实</p>', true);
    expect(a1).toHaveLength(1);
    expect(a1[0].rule).toBe('A-1');
    expect(a1[0].token).toBe('project_analysis');
    expect(a1[0].line).toBe(1);
    // 逐 token 报告：段里两枚阶段词各报一条（旧的「段级只报首个」会让第二枚无人看）
    const a2 = findHalfMixed('<div>单元测试结果尚未生成（review 在 test 之前）。</div>', true);
    expect(a2.map((h) => h.token)).toEqual(['review', 'test']);
    expect(a2.every((h) => h.rule === 'A-2')).toBe(true);
    // 阶段 id 全称同样算（含汉字前提下）
    expect(findHalfMixed('<p>同步检查 sync-check 不占游标行</p>', true)).toHaveLength(1);
    // 连词形的三枚阶段 id 在汉字句里必须报得出——HYPHEN_TOKEN 的连词备选
    //（`(?:review|test|develop|architect)-[\w-]+`）曾把它们整体吃掉（评审实证的遮蔽缺口）
    for (const id of ['architect-design', 'develop-design', 'test-design'] as const) {
      const ids = findHalfMixed(`<p>说明：${id} 阶段的产物会落到任务目录。</p>`, true);
      expect(ids.map((h) => h.token), id).toEqual([id]);
      expect(ids[0].rule, id).toBe('A-2');
    }
    // .ts 字符串字面量同样在扫描面
    expect(findHalfMixed("throw new Error('split_task 需要提供拆分方案');", false)).toHaveLength(1);
    // 组件文本型 props 在扫描面：EmptyState 的 next= 与标准属性同类（评审场景 6）
    const prop = findHalfMixed('<EmptyState next="新增一行并填好 secret_token 才能用。" />', true);
    expect(prop).toHaveLength(1);
    expect(prop[0].token).toBe('secret_token');
    // 表达式值里的**字面量**照报（表达式剥掉后用户看到的就是它）
    expect(findHalfMixed("<p title={ok ? '已保存（stage_configs）' : ''}>正文</p>", true)).toHaveLength(1);
  });

  it('口径负例：纯英文键名、文件名连写、徽章掩码不命中；注释不算', () => {
    // B1 键名标签不含汉字 → 不是「人话句夹符号」
    expect(findHalfMixed('<span>idle_timeout_sec</span>', true)).toEqual([]);
    // A-2 负向排除：产物文件名连写
    expect(findHalfMixed('<div>评审差异（review-diff.diff）尚未生成或不可读。</div>', true)).toEqual([]);
    expect(findHalfMixed('<div>test-report.md 尚未生成。</div>', true)).toEqual([]);
    // B5 负向排除同样盖住 A-1：带下划线的产物文件名不算 snake_case 内部符号
    expect(findHalfMixed('<div>任务的 test_result.json 还没生成。</div>', true)).toEqual([]);
    // 阶段 id 形状的产物文件名仍是文件名（FILENAME_EXT 先剔），B5 不因扫描顺序调整而破
    expect(findHalfMixed('<div>sync-check.md 还没生成。</div>', true)).toEqual([]);
    expect(findHalfMixed('<div>这是 develop-design.md 的产物。</div>', true)).toEqual([]);
    // 但裸阶段 id 仍要报——文件名先剔不等于连词备选可以继续吃掉它（遮蔽缺口的正向反面）
    expect(findHalfMixed('<div>这是 develop-design 的产物。</div>', true)).toHaveLength(1);
    // 掩码徽章（无汉字）
    expect(findHalfMixed('<span class="mono">api_key ***</span>', true)).toEqual([]);
    expect(findHalfMixed('const badge = `ctx ${n} · max_tokens ${t}`;', false)).toEqual([]);
    // 注释与 HTML 注释剥掉后不算
    expect(findHalfMixed('<!-- 下面会用 project_analysis -->\n<p>正文</p>', true)).toEqual([]);
    expect(findHalfMixed('// stage_configs 的校验\nexport const x = 1;', false)).toEqual([]);
    // 表达式里的字段名渲染出的是值，不算（渲染近似）
    expect(findHalfMixed('<span>评审：{task.review_mode}</span>', true)).toEqual([]);
    // 表达式型 prop 同理：字段名（pending_reason）不是文案，花括号连同进段才剥得掉
    expect(findHalfMixed("<MarkdownView source={task.pending_reason?.message ?? '需要你决定。'} />", true)).toEqual([]);
  });

  it('全站归零：命中全部落在 B 类登记表内（豁免仅经登记表、按 token 逐命中）', () => {
    const violations: string[] = [];
    for (const file of files) {
      const rel = relative(srcRoot, file).replaceAll('\\', '/');
      for (const hit of findHalfMixed(readFileSync(file, 'utf8'), file.endsWith('.svelte'))) {
        if (!isExempt(rel, hit.token)) violations.push(`${rel}:${hit.line}: [${hit.rule}:${hit.token}] ${hit.text}`);
      }
    }
    expect(violations).toEqual([]);
  });

  it('豁免按 token 逐命中匹配：同段的豁免符号掩蔽不了真违例', () => {
    const hits = findHalfMixed('<span>local_path 与 sneaky_flag 同段出现</span>', true);
    expect(hits.map((h) => h.token)).toEqual(['local_path', 'sneaky_flag']);
    expect(isExempt('components/settings/ProjectForm.svelte', 'local_path')).toBe(true);
    expect(hits.filter((h) => !isExempt('components/settings/ProjectForm.svelte', h.token)).map((h) => h.token)).toEqual([
      'sneaky_flag',
    ]);
  });

  it('登记表只收 B 类：条目数固定、每条理由非空、四类边界各至少一条', () => {
    expect(EXEMPTIONS.length).toBe(19);
    expect(EXEMPTIONS.every((e) => e.reason.trim().length > 0)).toBe(true);
    for (const b of ['B1', 'B2', 'B3', 'B4'] as const) {
      expect(EXEMPTIONS.some((e) => e.boundary === b)).toBe(true);
    }
    // B5 产物文件名由 FILENAME_EXT / HYPHEN_TOKEN 的一致负向排除承担（见「口径负例」三条文件名用例），
    // 登记表不留行使不到的死条目。登记的位置必须在扫描面里（防登记到不存在的文件）。
    const relFiles = files.map((f) => relative(srcRoot, f).replaceAll('\\', '/'));
    expect(EXEMPTIONS.every((e) => relFiles.includes(e.file))).toBe(true);
  });

  it('登记表每条都被行使：条目的 token 在其文件里真实命中（死条目 = 计数保护失效）', () => {
    const live = new Set<string>();
    for (const file of files) {
      const rel = relative(srcRoot, file).replaceAll('\\', '/');
      for (const hit of findHalfMixed(readFileSync(file, 'utf8'), file.endsWith('.svelte'))) {
        live.add(`${rel} ${hit.token}`);
      }
    }
    for (const e of EXEMPTIONS) {
      expect(live.has(`${e.file} ${e.match}`), `死条目：${e.file} / ${e.match}`).toBe(true);
    }
  });

  it('扫描面与既有两门共用同一份清单（测试 / bench 文件不在内）', () => {
    expect(files.some((f) => /\.(test|spec|bench)\.ts$/.test(f))).toBe(false);
  });
});

/* ══════════════════════ 规则 4：后端直呈报文窄扫 ═══════════════════════ */

/** 内部编号（含全角与「决策 130 / 137」连写）与票据号：规则 4 的两类编号形态。 */
const BACKEND_REF = new RegExp(
  String.raw`决策\s*[0-9０-９]{1,3}(?:\s*[/、]\s*[0-9０-９]{1,3})*|票\s*[0-9０-９]{1,3}[①②③④⑤⑥⑦⑧⑨⑩⑪⑫⑬⑭⑮]?(?:\s*[/、]\s*[0-9０-９]{1,3})*`,
  'g',
);

/** 构造形态的提取方式（三类形态的字面量位置不同，见下）。 */
type LiteralMode =
  /** 字面量须在构造 `(` 后第一个位置（允许空白 / 换行 / `format!(` 包一层）——①② 的形状。 */
  | 'leading'
  /** 取调用窗内的首个字符串字面量——③ 流转原因在实参**尾部**，前面还有 task_id / from / to 等实参。 */
  | 'first-quote';

interface BackendPattern {
  readonly re: RegExp;
  readonly mode: LiteralMode;
}

/**
 * 三类构造形态（**整库搜改是红线**，门只认这些）：
 * ① 报文构造器 / error 字段 ② `test_blockers` 类载荷 push
 * ③ **流转原因载荷**（落库渲染字段）——`reason = Some(…)` 赋值与
 * `insert_transition(…, Some("…"))` 实参（reason 落 `kanban_transitions` 后由
 * TimelineView 原文渲染；评审场景 5 实证 resume.rs 的 `（决策 116）` 从这个缺口溜过）。
 */
const BACKEND_CONSTRUCTORS: readonly BackendPattern[] = [
  { re: /ApiError::[a-z_]+\s*\(/g, mode: 'leading' },
  { re: /\bError::[A-Za-z]+\s*\(/g, mode: 'leading' },
  { re: /\berror:\s*Some\s*\(/g, mode: 'leading' },
  { re: /\b(?:test_blockers|dev_blockers|metadata_gaps|gaps|warnings)\s*\.push\s*\(/g, mode: 'leading' },
  { re: /\breason\s*=\s*Some\s*\(/g, mode: 'leading' },
  { re: /\binsert_transition(?:_in_tx)?\s*\(/g, mode: 'first-quote' },
];

export interface BackendRef {
  readonly line: number;
  readonly match: string;
  readonly snippet: string;
}

/** 在一段 Rust 源码里找**构造形态内**的编号（注释已剥；日志 / 断言 / prompt / CLI 不在形态内）。 */
export function findBackendCopyRefs(text: string): BackendRef[] {
  const masked = maskComments(text, false);
  const out: BackendRef[] = [];
  for (const { re, mode } of BACKEND_CONSTRUCTORS) {
    re.lastIndex = 0;
    for (const m of masked.matchAll(re)) {
      const window = masked.slice(m.index + m[0].length, m.index + m[0].length + 400);
      const lit = mode === 'leading' ? window.match(/^\s*(?:format!\s*\(\s*)?"([^"]*)"/s) : window.match(/"([^"]*)"/s);
      if (!lit) continue;
      const bad = lit[1].match(BACKEND_REF);
      if (!bad) continue;
      // 行号对回**字面量**（报文本体）所在行，而非构造器行
      const quoteAt = m.index + m[0].length + (lit.index ?? 0) + lit[0].indexOf('"');
      const { line } = lineOf(masked, quoteAt);
      out.push({ line, match: bad[0], snippet: lit[1].trim().slice(0, 110) });
    }
  }
  return out;
}

/** 后端扫描面：`crates` 下的全部 `.rs`，`tests/` / `benches/` 目录除外（与前端同一边界）。 */
function collectCratesFiles(dir: string): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) {
      if (name === 'tests' || name === 'benches' || name === 'node_modules') continue;
      out.push(...collectCratesFiles(p));
    } else if (name.endsWith('.rs')) out.push(p);
  }
  return out;
}

/** vitest 从 `frontend/` 运行，仓根的 crates 在 `../crates`（兼容从仓根直跑）。 */
const cratesRoot = [resolve(process.cwd(), '../crates'), resolve(process.cwd(), 'crates')].find(existsSync);

describe('文案纪律 · 规则 4：后端直呈报文里不出现内部编号（三类构造形态窄扫）', () => {
  it('口径正例：报文构造器、error 字段、载荷 push、流转原因的字面量命中（含多行 format!）', () => {
    expect(findBackendCopyRefs('ApiError::bad_request("只有终态任务可以归档（决策 34）")')).toHaveLength(1);
    expect(findBackendCopyRefs('error: Some("标终态（票 02②）".into()),')).toHaveLength(1);
    expect(findBackendCopyRefs('test_blockers.push(format!("high 场景 design_refs 缺失（决策 136）"));')).toHaveLength(1);
    const multiline = [
      'return Err(Error::Validation(format!(',
      '    "阶段 {} 无 skip（决策 86）",',
      ')));',
    ].join('\n');
    expect(findBackendCopyRefs(multiline)).toHaveLength(1);
    expect(findBackendCopyRefs(multiline)[0].line).toBe(2);
    // ③ 流转原因（落库渲染字段）：reason 赋值与 insert_transition 实参——
    // 评审场景 5 的漏网（resume.rs `（决策 116）`）与场景 7 的门缺口由这两条钉住
    expect(
      findBackendCopyRefs('reason = Some(format!("dependency_overridden：忽略失败依赖 {detail}（决策 116）"));'),
    ).toHaveLength(1);
    expect(
      findBackendCopyRefs('insert_transition(task_id, &cursor.branch, None, to, Trigger::Normal, Some("游标分裂（决策 90）"))'),
    ).toHaveLength(1);
    // 实参尾部的字面量被 kickback_reason(…) 包一层也要抓到（first-quote 提取）
    expect(
      findBackendCopyRefs(
        'insert_transition(task_id, b, from, to, Trigger::Kickback, Some(kickback_reason("merge 测试闸门失败（决策 85）").as_str()))',
      ),
    ).toHaveLength(1);
  });

  it('口径负例：日志、断言消息、注释、prompt 与 CLI 报文不在形态内（防误报烂门）', () => {
    expect(findBackendCopyRefs('tracing::warn!(task = %id, "脏工作区（不阻塞，决策 61）")')).toEqual([]);
    expect(findBackendCopyRefs('assert_eq!(offloaded, 1, "卸载文件应真实落盘（决策 148）");')).toEqual([]);
    expect(findBackendCopyRefs('.expect("放弃分支必须真的落账（决策 304）");')).toEqual([]);
    expect(findBackendCopyRefs('// 决策 3 的日志口径（照旧）')).toEqual([]);
    expect(findBackendCopyRefs('println!("  --allowed-origin <ORIGIN>（决策 157）：");')).toEqual([]);
    // 形态对但没有编号 → 不命中
    expect(findBackendCopyRefs('Error::Task(format!("技能不存在：{name}"))')).toEqual([]);
    // 裸 Some(…) 不在流转构造上下文——落库形态必须落在 reason 赋值 / insert_transition 实参里（正例③）
    expect(findBackendCopyRefs('let hint = Some("游标分裂（决策 90）");')).toEqual([]);
    // 非渲染的命令台账载荷字段不在形态内（`stderr_preview` 无任何组件渲染，triage 依据见 §12.1）
    expect(findBackendCopyRefs('stderr_preview: Some("run_command 在本阶段被禁用（决策 396）".into()),')).toEqual([]);
  });

  it('全站归零：crates 窄形态命中 0（tests / benches 目录不在扫描面）', () => {
    expect(cratesRoot, 'crates 目录应存在').toBeDefined();
    const hits: string[] = [];
    for (const file of collectCratesFiles(cratesRoot!)) {
      const rel = relative(resolve(cratesRoot!, '..'), file).replaceAll('\\', '/');
      for (const ref of findBackendCopyRefs(readFileSync(file, 'utf8'))) {
        hits.push(`${rel}:${ref.line}: 报文里出现「${ref.match}」｜ ${ref.snippet}`);
      }
    }
    expect(hits).toEqual([]);
  });
});
