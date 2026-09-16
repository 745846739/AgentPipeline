/**
 * 对比度门（决策 195 / 票 15；视觉规格 design/theme-6-pixel.md §2.6）。
 *
 * 这是「改动即被拦」的那条门：改色板时把某一档改差、只改一套配色、只核一个承载表面，
 * 三种都会在这里变红。断言的是**契约里的值与它们之间的对比度关系**（纯数据 +
 * 逐值比对），属既有的「静态扫描 + 逐值比对」家族——不是新接缝，也没为它改生产代码。
 *
 * 值的唯一事实源是 `contract.ts`；本文件**不重抄任何 token 字面值**（重抄＝一副
 * 「改契约不改测试也绿」的假门）。失败信息按 §2.6 裁决三的定稿格式逐条列出。
 */
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { DARK_COLORS, LIGHT_COLORS, type ColorToken } from './contract';
import {
  ALPHA_MATERIAL_TOKENS,
  DECORATIVE_TOKENS,
  EXEMPT,
  PAIR_RULES,
  SIGNAL_TOKENS,
  SURFACE_TOKENS,
  TEXT_TIERS,
  contrastFailures,
  contrastRatio,
  contrastVerdicts,
  formatVerdict,
  relativeLuminance,
} from './contrast';

/** vitest 从 `frontend/` 运行（vite.config.ts 的 include 是 src/**），故以 cwd 定位 src。 */
const appCss = readFileSync(resolve(process.cwd(), 'src/app.css'), 'utf8');

describe('对比度 · WCAG 纯函数', () => {
  it('相对亮度：白 = 1、黑 = 0', () => {
    expect(relativeLuminance('#FFFFFF')).toBeCloseTo(1, 10);
    expect(relativeLuminance('#000000')).toBe(0);
  });

  it('对比度比：黑白 = 21:1、同色 = 1:1、与参数顺序无关', () => {
    expect(contrastRatio('#FFFFFF', '#000000')).toBeCloseTo(21, 6);
    expect(contrastRatio('#000000', '#FFFFFF')).toBeCloseTo(21, 6);
    expect(contrastRatio('#1B1D2C', '#1B1D2C')).toBeCloseTo(1, 10);
  });

  it('接受 #RGB 简写与 rgb() 形态（契约是 #RRGGBB，但形状要宽容）', () => {
    expect(contrastRatio('#fff', '#000')).toBeCloseTo(21, 6);
    expect(contrastRatio('rgb(255, 255, 255)', '#000000')).toBeCloseTo(21, 6);
  });

  it('解析不了的颜色报错，不静默跳过（门的输入坏了要响）', () => {
    expect(() => relativeLuminance('var(--bg)')).toThrow(/无法解析颜色/);
  });
});

describe('对比度门 · 门槛表（§2.6 裁决二）', () => {
  it('逐档门槛：--text-hi 12 / --text 7 / --text-2 4.5 / --text-3 4.5', () => {
    expect(Object.fromEntries(TEXT_TIERS.map((t) => [t.token, t.min]))).toEqual({
      '--text-hi': 12,
      '--text': 7,
      '--text-2': 4.5,
      '--text-3': 4.5,
    });
  });

  it('7:1 只属于 --text 一档（不是全站口径）', () => {
    expect(TEXT_TIERS.filter((t) => t.min === 7).map((t) => t.token)).toEqual(['--text']);
  });

  it('承载表面就取 --bg 与 --panel 两个；--wash 是瞬态底，不进这门', () => {
    expect(SURFACE_TOKENS).toEqual(['--bg', '--panel']);
    expect(SURFACE_TOKENS).not.toContain('--wash');
  });

  it('成对色单独一栏：钮字 / diff 增删各自 ≥ 4.5:1', () => {
    expect(PAIR_RULES.map((p) => [p.fg, p.bg, p.min])).toEqual([
      ['--go-ink', '--go', 4.5],
      ['--diff-add', '--diff-add-bg', 4.5],
      ['--diff-del', '--diff-del-bg', 4.5],
    ]);
  });

  it('--text-4 不在任何门槛表里（纯装饰，门里豁免）', () => {
    expect(TEXT_TIERS.map((t) => t.token)).not.toContain('--text-4');
    expect(PAIR_RULES.flatMap((p) => [p.fg, p.bg])).not.toContain('--text-4');
    expect(DECORATIVE_TOKENS).toEqual(['--text-4']);
  });
});

describe('对比度门 · 全表（深浅两套 × 两个承载表面）', () => {
  it('全部达标（不达标时按定稿格式逐条列出）', () => {
    expect(contrastFailures().map(formatVerdict)).toEqual([]);
  });

  it('跑满四个档 × 两个面 × 两套 + 三对成对色 × 两套 = 22 个判定点', () => {
    expect(contrastVerdicts()).toHaveLength(TEXT_TIERS.length * SURFACE_TOKENS.length * 2 + PAIR_RULES.length * 2);
    expect(contrastVerdicts()).toHaveLength(22);
  });

  it('每个判定点都带齐 token / 配色 / 表面 / 实测 / 门槛', () => {
    for (const v of contrastVerdicts()) {
      expect(v.token).toMatch(/^--/);
      expect(['dark', 'light']).toContain(v.theme);
      expect(v.surface).toMatch(/^--/);
      expect(Number.isFinite(v.actual)).toBe(true);
      expect(v.actual).toBeGreaterThanOrEqual(1);
      expect(v.min).toBeGreaterThan(1);
    }
  });

  it('失败信息给全六项（token / 配色 / 表面 / 实测 / 门槛 / 差值），按定稿格式', () => {
    expect(
      formatVerdict({ kind: 'tier', token: '--text-3', theme: 'dark', surface: '--panel', actual: 4.13, min: 4.5 }),
    ).toBe('对比度门：--text-3（深色）对 --panel 实测 4.13:1 < 门槛 4.5:1（差 0.37）');
  });

  it('改差会被拦住：把 --text-3 改回旧值，门立刻红（深色两个面都红）', () => {
    // 旧值 #6E6C82 是票 15 之前的字面值（§2.6 现状列 3.28 / 2.93）。
    const rolledBack = { dark: { ...DARK_COLORS, '--text-3': '#6E6C82' }, light: LIGHT_COLORS };
    expect(contrastFailures(rolledBack).map(formatVerdict)).toEqual([
      '对比度门：--text-3（深色）对 --bg 实测 3.28:1 < 门槛 4.5:1（差 1.22）',
      '对比度门：--text-3（深色）对 --panel 实测 2.93:1 < 门槛 4.5:1（差 1.57）',
    ]);
  });

  it('只改一套配色也拦得住：浅色款漏改同样红', () => {
    const halfChanged = { dark: DARK_COLORS, light: { ...LIGHT_COLORS, '--text-3': '#7A7B8E' } };
    expect(contrastFailures(halfChanged).map(formatVerdict)).toEqual([
      '对比度门：--text-3（浅色）对 --bg 实测 3.32:1 < 门槛 4.5:1（差 1.18）',
      '对比度门：--text-3（浅色）对 --panel 实测 3.77:1 < 门槛 4.5:1（差 0.73）',
    ]);
  });
});

describe('对比度门 · 豁免名单（§2.6 裁决三）', () => {
  it('EXEMPT 里的每个 token 都能在契约里找到（防死名字）', () => {
    const inContract = new Set(Object.keys(DARK_COLORS));
    expect(EXEMPT.filter((t) => !inContract.has(t))).toEqual([]);
  });

  it('豁免名单显式含装饰档与信号色 / 材质色那一族', () => {
    const wanted: ColorToken[] = [
      '--text-4', // 纯装饰
      '--go', '--go-hi', '--pending', '--stop', '--done', // 信号色
      '--branch-dev', '--branch-tst', '--belt-lit', // 徽章与亮度阶
      '--pane', '--wash', '--ink', '--hairline', // 描边 / 底 / 墨 / 弱分隔
      '--bg', '--panel', // 承载表面自身
    ];
    for (const t of wanted) expect(EXEMPT).toContain(t);
    expect(SIGNAL_TOKENS).toHaveLength(14);
  });

  it('豁免名单完整：EXEMPT + 门槛表 + 成对色 = 契约全部 token，一个不漏', () => {
    const covered = new Set<string>([
      ...EXEMPT,
      ...TEXT_TIERS.map((t) => t.token),
      ...PAIR_RULES.flatMap((p) => [p.fg, p.bg]),
    ]);
    expect(Object.keys(DARK_COLORS).filter((t) => !covered.has(t))).toEqual([]);
  });

  it('两个透明材质口不在契约 token 表内，但确实在 app.css 里声明（防死名字）', () => {
    const inContract = new Set(Object.keys(DARK_COLORS));
    for (const t of ALPHA_MATERIAL_TOKENS) {
      expect(inContract.has(t)).toBe(false);
      expect(appCss).toMatch(new RegExp(`${t}\\s*:`));
    }
  });
});
