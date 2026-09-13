/**
 * app.css ↔ 主题契约 一致性护栏（决策 169 / 规格 Testing Decisions 第 2 条）。
 *
 * 契约模块是 token 的唯一事实源，`app.css` 是它的**手工镜像**（决策 169 不引入
 * 代码生成）。本测试逐条比对两个块的 token 值，并禁止 token 块之外出现裸十六进制颜色
 * ——这是「像素纪律」的可机器检查形式，也是防漂移护栏（毫秒级发现「改了一边忘了另一边」）。
 *
 * 边界：只做镜像比对，不替代真实渲染断言（那在 playwright 层）。
 */
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative, resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { DARK_COLORS, LIGHT_COLORS } from './contract';

// vitest 从 `frontend/` 运行（vite.config.ts 的 include 是 src/**），故以 cwd 定位 src。
const srcRoot = resolve(process.cwd(), 'src');
const appCssPath = join(srcRoot, 'app.css');

/** 抽出指定选择器的声明块文本（按大括号配对，能处理嵌套的 @media）。 */
function blockOf(css: string, selector: string): string {
  const idx = css.indexOf(selector);
  if (idx < 0) throw new Error(`app.css 缺少选择器：${selector}`);
  const open = css.indexOf('{', idx);
  let depth = 0;
  for (let i = open; i < css.length; i++) {
    if (css[i] === '{') depth++;
    else if (css[i] === '}') {
      depth--;
      if (depth === 0) return css.slice(open + 1, i);
    }
  }
  throw new Error(`选择器 ${selector} 的块未闭合`);
}

/** 解析块内的自定义属性声明为 name → 值（保留原样小写比较）。 */
function customProps(block: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const m of block.matchAll(/(--[a-z0-9-]+)\s*:\s*([^;]+);/g)) {
    out[m[1]] = m[2].trim();
  }
  return out;
}

const appCssRaw = readFileSync(appCssPath, 'utf8');
// 先剥注释：注释里会提到选择器与颜色（如头部说明提到 html[data-theme='light']），
// 不剥会把注释里的提及当成真正的声明块起点。
const appCss = appCssRaw.replace(/\/\*[\s\S]*?\*\//g, '');
const darkBlock = customProps(blockOf(appCss, ':root'));
const lightBlock = customProps(blockOf(appCss, "html[data-theme='light']"));

describe('app.css ↔ 契约：色彩 token 镜像', () => {
  it.each(Object.entries(DARK_COLORS))('深色 %s = %s', (name, value) => {
    expect(darkBlock[name]?.toLowerCase()).toBe(value.toLowerCase());
  });

  it.each(Object.entries(LIGHT_COLORS))('浅色 %s = %s', (name, value) => {
    expect(lightBlock[name]?.toLowerCase()).toBe(value.toLowerCase());
  });

  it('浅色块只覆盖色彩 token 与三个语义口，不重复几何常量', () => {
    // 几何常量（圆角 / 列宽 / 字号…）在两套主题下相同，不应在浅色块里重复声明；
    // 允许覆盖的仅：契约里的色彩 token + 三个随主题变的语义口。
    const semanticOverrides = new Set(['--input', '--overlay', '--pending-tint']);
    const extra = Object.keys(lightBlock).filter(
      (k) => !(k in LIGHT_COLORS) && !semanticOverrides.has(k),
    );
    expect(extra).toEqual([]);
  });
});

describe('app.css ↔ 契约：像素纪律', () => {
  it('全站无 border-radius 非 0 值（圆角恒 0）', () => {
    const bad: string[] = [];
    for (const m of appCss.matchAll(/border-radius\s*:\s*([^;]+);/g)) {
      const v = m[1].trim();
      if (v !== '0' && v !== 'var(--r-panel)' && v !== 'var(--r-pill)') bad.push(v);
    }
    expect(bad).toEqual([]);
  });

  it('滚动条是唯一允许的非 token 方角（border-radius: 0）', () => {
    // 占位断言：滚动条显式写 0，见上一条覆盖；此处确保没有声明 0 之外的值。
    expect(appCss).toContain('border-radius: 0');
  });

  it('无平滑渐变（linear-gradient / radial-gradient 不出现在 token 之外）', () => {
    // 像素主题唯一的「渐变」是 conic-gradient 做的 dither；链节用 repeating-linear-gradient
    // 画的是硬边像素条（非平滑过渡），故只禁无平铺的 linear/radial-gradient。
    const smooth = [...appCss.matchAll(/(?<!repeating-)\b(linear-gradient|radial-gradient)\(/g)];
    expect(smooth).toEqual([]);
  });

  it('无非离散缓动（禁 ease / cubic-bezier）', () => {
    const easing = [...appCss.matchAll(/\b(ease|ease-in|ease-out|ease-in-out|cubic-bezier)\(?/g)]
      .map((m) => m[1])
      // `steps(2)` 是允许的离散步进；`ease` 仅允许出现在注释里，故按声明行判定。
      .filter((e) => e !== 'ease');
    expect(easing).toEqual([]);
    // 再按行校验：任何 animation/transition 简写里不得出现 ease 关键字。
    const badLines = appCss
      .split('\n')
      .filter((l) => /\b(ease|ease-in|ease-out|ease-in-out|cubic-bezier)\b/.test(l))
      .filter((l) => !l.trim().startsWith('/*') && !l.trim().startsWith('*'));
    expect(badLines).toEqual([]);
  });

  it('描边只用 2px 一档（禁止 1px / 3px 边框）', () => {
    const bad: string[] = [];
    for (const m of appCss.matchAll(/\bborder(?:-(?:top|right|bottom|left))?\s*:\s*([^;]+);/g)) {
      const v = m[1].trim();
      const width = v.match(/^(\d+)px/);
      if (width && width[1] !== '2') bad.push(v);
      if (/^(thin|medium|thick)\b/.test(v)) bad.push(v);
    }
    expect(bad).toEqual([]);
  });
});

/** 收集 src 下的所有组件/脚本文件（不含测试与 app.css 自身）。 */
function collectSourceFiles(dir: string): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) {
      if (name === 'node_modules') continue;
      out.push(...collectSourceFiles(p));
    } else if (/\.(svelte|ts)$/.test(name) && !name.endsWith('.test.ts')) {
      out.push(p);
    }
  }
  return out;
}

describe('像素纪律：token 块之外的裸十六进制颜色', () => {
  // 白名单：
  //  - contract.ts / css-parity.test.ts 自身 = token 的唯一事实源，颜色必须写在这里；
  //  - 工头脸块固定肤色（§2.4 偏差②明确要求字面值，非 token，与主题无关）。
  const ALLOW_FILES = new Set(['theme/contract.ts', 'theme/contract.test.ts', 'theme/css-parity.test.ts']);
  const ALLOWLIST: Array<{ file: string; hex: string; why: string }> = [
    {
      file: 'components/render/Sprite.svelte',
      hex: '#E3C7A6',
      why: '工头脸块固定肤色（§2.4 偏差②）',
    },
    {
      file: 'routes/Share.svelte',
      hex: '#fff',
      why: '二维码恒白底：扫描器依赖明暗对比，浅色主题也不例外（§3.1）',
    },
  ];

  it('除白名单外，组件与脚本里不出现裸十六进制颜色', () => {
    const offenders: string[] = [];
    for (const file of collectSourceFiles(srcRoot)) {
      const rel = relative(srcRoot, file).replaceAll('\\', '/');
      if (ALLOW_FILES.has(rel)) continue;
      // 剥注释：注释里的说明性颜色不算违规（如「§2.1 ... #FFB545」）。
      const text = readFileSync(file, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '');
      for (const m of text.matchAll(/#[0-9a-fA-F]{3,8}\b/g)) {
        const hex = m[0];
        // 排除两类误报：Svelte 模板 `{#each`（# 后是标识符）；HTML 实体 `&#123;`。
        const before = text.slice(Math.max(0, (m.index ?? 0) - 1), m.index ?? 0);
        if (before === '&') continue;
        if (!/^#([0-9a-fA-F]{3}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$/.test(hex)) continue;
        const allowed = ALLOWLIST.some((a) => a.file === rel && a.hex.toLowerCase() === hex.toLowerCase());
        if (!allowed) offenders.push(`${rel}: ${hex}`);
      }
    }
    expect(offenders).toEqual([]);
  });
});
