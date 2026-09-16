/**
 * 对比度门（决策 195 / 票 15）——视觉规格 `design/theme-6-pixel.md` §2.6 的可执行形式。
 *
 * 补回的不是一段说明，是一条门：主题六改版把旧主题各自都有的「无障碍与实现注记」整节丢掉了
 * （改版后查「对比度」「无障碍」零命中），于是承载必读内容的两档灰字（实测 3.28:1 / 2.25:1）
 * 在纸面上没有任何要求，规则丢了半年没人发现。这里把「按 token 职责分档」写成一张表：
 *
 * - 逐档门槛：`--text-hi` ≥ 12、`--text` ≥ 7（**这条 7:1 只属于这一档**）、
 *   `--text-2` ≥ 4.5、`--text-3`（次级必读）≥ 4.5；`--text-4` 定性为纯装饰、门里豁免。
 * - **承载表面两个：`--bg` 与 `--panel`，深浅两套都要过。** 深色款的约束面是 `--panel`
 *   （比 `--bg` 亮一档），浅色款的约束面是 `--bg`（比 `--panel` 暗一档）——只核一个面
 *   会在另一套配色上漏。`--wash` 是 hover / active 的瞬态底，不进这门。
 * - 信号色与材质色（灯 / 条 / 图元 / 描边 / 底）不是文字档，不进这门；成对色
 *   （实心钮字、diff 增删行）单独一栏，逐对 ≥ 4.5:1。
 *
 * **值一律从 `contract.ts` 的 `DARK_COLORS` / `LIGHT_COLORS` 读，本模块不重抄任何字面值**
 * ——重抄就等于造一副「改契约不改测试也绿」的假门（§2.6 裁决三）。
 *
 * 边界：本模块只有纯函数与门槛表，机器门本体（vitest 用例）在 `contrast.test.ts`；
 * 属既有「静态扫描 + 逐值比对」家族，**不新增可测试性接缝、不为它改生产代码形状**。
 */
import { COLORS, type ColorToken, type ThemeName } from './contract';

/** 深浅两套的配色顺序（失败信息按这个顺序列出）。 */
export const THEMES: readonly ThemeName[] = ['dark', 'light'];

/** 承载表面：每档文字门槛对这两个面都要成立（§2.6 裁决三）。 */
export const SURFACE_TOKENS: readonly ColorToken[] = ['--bg', '--panel'];

/** 一档文字的规则。 */
export interface TierRule {
  readonly token: ColorToken;
  /** 门槛（WCAG 对比度比）。 */
  readonly min: number;
  /** 用法职责（§2.6 裁决二）。 */
  readonly role: string;
}

/**
 * 逐档门槛表（§2.6 裁决二）。`--text-4` **不在表里**——它定性为纯装饰、门里豁免。
 * 顺序即信息层级顺序。
 */
export const TEXT_TIERS: readonly TierRule[] = [
  { token: '--text-hi', min: 12, role: '标题、可执行物、当前游标' },
  { token: '--text', min: 7, role: '正文（7:1 只属于这一档，不是全站口径）' },
  { token: '--text-2', min: 4.5, role: '次文本' },
  {
    token: '--text-3',
    min: 4.5,
    role: '次级必读：列头、页面导入语、空态引导句、待处理下拉里的原因、元信息行、返回入口、输入框占位符、任务卡时长',
  },
];

/** 成对色：前景坐在另一枚 token 的实心面上，不坐「页面的底」上（§2.6 用法职责第 4 条）。 */
export interface PairRule {
  readonly fg: ColorToken;
  readonly bg: ColorToken;
  readonly min: number;
  readonly role: string;
}

export const PAIR_RULES: readonly PairRule[] = [
  { fg: '--go-ink', bg: '--go', min: 4.5, role: '实心主动作钮：钮字 / 钮底' },
  { fg: '--diff-add', bg: '--diff-add-bg', min: 4.5, role: 'diff 新增行：字 / 底' },
  { fg: '--diff-del', bg: '--diff-del-bg', min: 4.5, role: 'diff 删除行：字 / 底' },
];

/**
 * 纯装饰档（§2.6 裁决二）：门里豁免，且附用法规约「**凡用到它的地方不得承载必读信息**」。
 * 判据一句话：读不到就会挡住下一步的用 `--text-3`，纯刻度装饰才留 `--text-4`
 * （合法用法只剩结构噪声：未来站名 / 回流带 `↩` / 空刻度槽）。
 */
export const DECORATIVE_TOKENS: readonly ColorToken[] = ['--text-4'];

/**
 * 信号色与材质色（§2.6 用法职责第 3 条）：承担的是灯、条、图元、描边与底，不是文字档，
 * 故不设门槛。用它们**作文字**时另受一条约束——只许出现在已有第二编码（灯 / 条 / 图元 +
 * 状态词同时出现）的状态标记上；只有文字编码、要人读的改用 `--text-3`。
 *
 * `--go` 也在这里（它本身不设门槛），同时被 `PAIR_RULES` 当作 `--go-ink` 的底面消费。
 */
export const SIGNAL_TOKENS: readonly ColorToken[] = [
  '--go',
  '--go-hi',
  '--pending',
  '--stop',
  '--done',
  '--branch-dev',
  '--branch-tst',
  '--belt-lit',
  '--pane',
  '--wash',
  '--bg',
  '--panel',
  '--ink',
  '--hairline',
];

/**
 * **豁免名单（显式列出，并被测试校验「每个 token 都能在契约里找到」）**：
 * 纯装饰档 + 信号色与材质色。索引与豁免最大的失败形态不是没建，是建完悄悄烂掉。
 */
export const EXEMPT: readonly ColorToken[] = [...DECORATIVE_TOKENS, ...SIGNAL_TOKENS];

/**
 * 带透明度的材质口（§2.6 第 3 条也点了它们）：`--overlay` / `--pending-tint` 在 `app.css`
 * 里是 `rgba()`，**不在契约的 token 表内**（契约只装不透明的色彩 token）。
 * 它们天然不进这副门；列在这里是为了让「不进门的名单」是完整的，并由测试核对它们
 * 确实在 `app.css` 里有声明（防死名字）。
 */
export const ALPHA_MATERIAL_TOKENS = ['--overlay', '--pending-tint'] as const;

/** `#RGB` / `#RRGGBB` / `rgb()` / `rgba()` → 三通道 0–255。 */
function channels(color: string): [number, number, number] {
  const text = color.trim().toLowerCase();
  const hex = text.match(/^#([0-9a-f]{3}|[0-9a-f]{6})$/);
  if (hex) {
    const h = hex[1];
    const full = h.length === 3 ? h.split('').map((c) => c + c).join('') : h;
    const n = Number.parseInt(full, 16);
    return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
  }
  const rgb = text.match(/^rgba?\(\s*([\d.]+)[\s,]+([\d.]+)[\s,]+([\d.]+)/);
  if (rgb) return [Number(rgb[1]), Number(rgb[2]), Number(rgb[3])];
  throw new Error(`对比度门：无法解析颜色 ${JSON.stringify(color)}（只支持 #RGB / #RRGGBB / rgb()）`);
}

/** WCAG 2.x 相对亮度（0 = 黑，1 = 白）。 */
export function relativeLuminance(color: string): number {
  const linear = (v: number): number => {
    const c = v / 255;
    return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
  };
  const [r, g, b] = channels(color);
  return 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);
}

/** WCAG 2.x 对比度比（1 = 同色，21 = 黑白）。与参数顺序无关。 */
export function contrastRatio(a: string, b: string): number {
  const la = relativeLuminance(a);
  const lb = relativeLuminance(b);
  const [hi, lo] = la >= lb ? [la, lb] : [lb, la];
  return (hi + 0.05) / (lo + 0.05);
}

/** 一次判定：哪一档（或哪对色）、哪套配色、哪个表面、实测多少、门槛多少。 */
export interface ContrastVerdict {
  readonly kind: 'tier' | 'pair';
  /** 被判的 token（成对色取前景）。 */
  readonly token: ColorToken;
  readonly theme: ThemeName;
  /** 承载表面（成对色取底色 token）。 */
  readonly surface: ColorToken;
  /** 实测对比度比。 */
  readonly actual: number;
  /** 门槛。 */
  readonly min: number;
}

const THEME_LABEL: Readonly<Record<ThemeName, string>> = { dark: '深色', light: '浅色' };

/**
 * 失败信息的定稿格式（§2.6 裁决三）——六项都要有：
 * token / 配色 / 表面 / 实测 / 门槛 / 差值。
 *
 * `对比度门：--text-3（深色）对 --panel 实测 4.13:1 < 门槛 4.5:1（差 0.37）`
 */
export function formatVerdict(verdict: ContrastVerdict): string {
  const diff = (verdict.min - verdict.actual).toFixed(2);
  return `对比度门：${verdict.token}（${THEME_LABEL[verdict.theme]}）对 ${verdict.surface} 实测 ${verdict.actual.toFixed(2)}:1 < 门槛 ${verdict.min}:1（差 ${diff}）`;
}

/** 浮点噪声容差：刚好压在门槛上的值不该因为末位误差判成不达标。 */
const EPSILON = 1e-9;

/** 跑全表：深浅两套 × 两个表面 × 四个文字档 + 深浅两套 × 三对成对色。 */
export function contrastVerdicts(
  colors: Readonly<Record<ThemeName, Readonly<Record<ColorToken, string>>>> = COLORS,
): ContrastVerdict[] {
  const out: ContrastVerdict[] = [];
  for (const theme of THEMES) {
    const set = colors[theme];
    for (const tier of TEXT_TIERS) {
      for (const surface of SURFACE_TOKENS) {
        out.push({
          kind: 'tier',
          token: tier.token,
          theme,
          surface,
          actual: contrastRatio(set[tier.token], set[surface]),
          min: tier.min,
        });
      }
    }
    for (const pair of PAIR_RULES) {
      out.push({
        kind: 'pair',
        token: pair.fg,
        theme,
        surface: pair.bg,
        actual: contrastRatio(set[pair.fg], set[pair.bg]),
        min: pair.min,
      });
    }
  }
  return out;
}

/** 不达标的判定（空数组 = 门绿）。失败信息用 `formatVerdict` 渲染。 */
export function contrastFailures(
  colors: Readonly<Record<ThemeName, Readonly<Record<ColorToken, string>>>> = COLORS,
): ContrastVerdict[] {
  return contrastVerdicts(colors).filter((v) => v.actual + EPSILON < v.min);
}
