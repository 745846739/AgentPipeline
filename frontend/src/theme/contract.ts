/**
 * 主题契约模块（决策 169）——本 effort 唯一新接缝。
 *
 * 像素主题「夜班流水线」的 token、几何常量、sprite 与状态映射的**唯一事实源**。
 * 规格：design/theme-6-pixel.md §2（视觉事实源）；验收参照：design/prototype-pixel.html
 * （参照物冻结点提交 612cc07）。
 *
 * 边界（决策 143）：本模块只承载**视觉数据**，不承载状态或业务语义——状态语义仍在
 * stores 与 realtime/reduce.ts 中。`app.css` 手工镜像本文件的色彩 token，由
 * `css-parity.test.ts` 锁死一致性（不引入代码生成，见决策 169）。
 *
 * 与规格文本的几处**对账**（原型即验收参照，选型 A）：theme-6 §2.3 原文的
 * 「传送带 8px」「dossier 320px」「阴影只有 4px 一档」是原型定稿前的措辞，已按冻结原型
 * 校正为 6px 链节 / 340px dossier / 两级硬投影（4px 容器 + 3px 小控件）——§2.3 已同步改写。
 */

/** 深浅两套。属性名 `agentpipeline.theme` / `html[data-theme]` 沿用既有约定。 */
export type ThemeName = 'dark' | 'light';

/** 一张货箱的语义状态（灯 + 描边 + 小人节奏共用同一套枚举）。 */
export type CrateState = 'running' | 'pending' | 'failed' | 'done' | 'queued' | 'waiting';

/** 色彩 token 名（与 app.css 的 `--*` 逐一对应）。 */
export type ColorToken =
  | '--bg'
  | '--panel'
  | '--wash'
  | '--pane'
  | '--ink'
  | '--hairline'
  | '--text-hi'
  | '--text'
  | '--text-2'
  | '--text-3'
  | '--text-4'
  | '--go'
  | '--go-hi'
  | '--go-ink'
  | '--pending'
  | '--stop'
  | '--done'
  | '--branch-dev'
  | '--branch-tst'
  | '--diff-add'
  | '--diff-add-bg'
  | '--diff-del'
  | '--diff-del-bg'
  | '--belt-lit';

/** 非色彩 token（几何 / 语义口），值按原型实测。 */
export const GEOMETRY = {
  /** 描边：全站只有 2px 一档。 */
  border: 2,
  /** 圆角恒为 0（像素纪律）。 */
  radius: 0,
  /** 容器硬投影（货箱 / 对话框 / 台账盒）。 */
  shadow: '4px 4px 0',
  /** 小控件硬投影（按钮 / 槽位）。 */
  shadowControl: '3px 3px 0',
  /** wordmark 文字投影（浅色款去投影，见 LIGHT_DEVIATIONS）。 */
  shadowText: '3px 3px 0',
  /** 按压位移 = 投影距离（消影）。 */
  pressShift: 4,
  pressShiftControl: 3,
  /** 字号只取 12 的整数倍。 */
  fontSizes: [12, 24, 36],
  /** 基准字号与行高。 */
  baseFontSize: 12,
  lineHeight: 1.6,
  /** 2×2 棋盘 dither 的唯一周期。 */
  ditherPeriod: 4,
  /** token HP 量表段数（每段 5×10px、间隙 2px，满格 ≈ 64k tok）。 */
  gaugeSegments: 16,
  gaugeSegmentWidth: 5,
  gaugeSegmentHeight: 10,
  gaugeGap: 2,
  gaugeFullTokens: 64_000,
  /** boss 战尝试条段数（20 段，最后一次尝试整条转红）。 */
  bossSegments: 20,
  bossSegmentHeight: 10,
  /**
   * boss 条分母的**镜像默认值**（`Settings::validate_retry_max` 缺省 3，
   * crates/core/src/config.rs）。
   *
   * 诚实说明：该值**没有端点下发**（本 effort 是纯前端视觉替换，规格 Out of Scope
   * 明确不改任何端点），故这里镜像后端缺省，与「app.css 手工镜像 token」同一姿态。
   * 代价：用户把 `validate_retry_max` 改成非 3 的值后，运行中进度条的**分母会偏**。
   * 但最要紧的那条信号不受影响——「是不是最后一次」由后端下发的
   * `pending_reason.type === 'retry_exhausted'` 权威给出，届时整条转红。
   * 若日后加了端点，删掉本常量改读接口即可（前端只有货箱量表一处消费）。
   */
  retryLimitMirror: 3,
  /** 道具栏槽位边长。 */
  slot: 34,
  /** 看板列宽 × 列数。 */
  columnWidth: 264,
  columnCount: 8,
  /** 详情内容区最宽 / dossier 右栏宽 / 分栏后的整体最宽。 */
  detailMax: 1000,
  dossierWidth: 340,
  splitMax: 1240,
  /** 货箱顶盖 dither 带高。 */
  crateLid: 6,
  /** 传送带链节：高 6px、亮 6px 暗 6px（周期 12px）。 */
  belt: 6,
  beltPeriod: 12,
  /** 站点信号灯方块边长。 */
  lamp: 12,
  /** 挥锤小人画布。 */
  worker: 16,
  /** 工头头像显示尺寸（viewBox 16×16）。 */
  foreman: 48,
  /** 唯一媒体断点（桌面 ≥480px 行为不变）。 */
  mobileBreakpoint: 480,
} as const;

/** 单枚像素图元：viewBox 为 N×N 像素网格，rect 用网格坐标。 */
export interface Sprite {
  readonly viewBox: number;
  readonly size: number;
  readonly rects: readonly SpriteRect[];
}

export interface SpriteRect {
  readonly x: number;
  readonly y: number;
  readonly w: number;
  readonly h: number;
  /** 缺省用 `currentColor`；否则为 token 名或字面色。 */
  readonly fill?: string;
}

export type SpriteName =
  | 'flag'
  | 'gem'
  | 'hammer'
  | 'flask'
  | 'gear'
  | 'lens'
  | 'shield'
  | 'merge'
  | 'trophy'
  | 'chest'
  | 'alert'
  | 'chart'
  | 'key'
  | 'phone'
  | 'foreman';

const S8 = (rects: SpriteRect[]): Sprite => ({ viewBox: 8, size: 16, rects });

/**
 * 受控 sprite 表（15 枚）——从原型 `SPRITES` 机械搬运。
 * 新增图元必须回 theme-6-pixel.md §2.3 修订后实现（规格 §2.3）。
 */
export const SPRITES: Readonly<Record<SpriteName, Sprite>> = {
  flag: S8([
    { x: 2, y: 0, w: 1, h: 8 },
    { x: 3, y: 0, w: 4, h: 2 },
    { x: 3, y: 2, w: 3, h: 1 },
    { x: 3, y: 3, w: 2, h: 1 },
  ]),
  gem: S8([
    { x: 3, y: 0, w: 2, h: 1 },
    { x: 2, y: 1, w: 4, h: 1 },
    { x: 1, y: 2, w: 6, h: 1 },
    { x: 2, y: 3, w: 4, h: 1 },
    { x: 3, y: 4, w: 2, h: 1 },
    { x: 2, y: 6, w: 4, h: 2 },
  ]),
  hammer: S8([
    { x: 1, y: 1, w: 6, h: 3 },
    { x: 3, y: 4, w: 2, h: 4 },
  ]),
  flask: S8([
    { x: 3, y: 0, w: 2, h: 3 },
    { x: 2, y: 3, w: 4, h: 2 },
    { x: 1, y: 5, w: 6, h: 3 },
  ]),
  gear: S8([
    { x: 3, y: 0, w: 2, h: 8 },
    { x: 0, y: 3, w: 8, h: 2 },
    { x: 1, y: 1, w: 1, h: 1 },
    { x: 6, y: 1, w: 1, h: 1 },
    { x: 1, y: 6, w: 1, h: 1 },
    { x: 6, y: 6, w: 1, h: 1 },
  ]),
  lens: S8([
    { x: 2, y: 1, w: 3, h: 1 },
    { x: 1, y: 2, w: 1, h: 3 },
    { x: 4, y: 2, w: 1, h: 3 },
    { x: 2, y: 4, w: 3, h: 1 },
    { x: 5, y: 5, w: 1, h: 1 },
    { x: 6, y: 6, w: 1, h: 1 },
  ]),
  shield: S8([
    { x: 1, y: 1, w: 6, h: 2 },
    { x: 2, y: 3, w: 4, h: 1 },
    { x: 3, y: 4, w: 2, h: 2 },
  ]),
  merge: S8([
    { x: 1, y: 1, w: 1, h: 1 },
    { x: 2, y: 2, w: 1, h: 1 },
    { x: 6, y: 1, w: 1, h: 1 },
    { x: 5, y: 2, w: 1, h: 1 },
    { x: 3, y: 3, w: 2, h: 1 },
    { x: 3, y: 4, w: 2, h: 3 },
  ]),
  trophy: S8([
    { x: 2, y: 1, w: 4, h: 3 },
    { x: 1, y: 1, w: 1, h: 2 },
    { x: 6, y: 1, w: 1, h: 2 },
    { x: 3, y: 4, w: 2, h: 1 },
    { x: 2, y: 5, w: 4, h: 2 },
  ]),
  chest: S8([
    { x: 1, y: 1, w: 6, h: 2 },
    { x: 1, y: 3, w: 6, h: 4 },
    { x: 3, y: 4, w: 2, h: 2, fill: '--bg' },
  ]),
  alert: S8([
    { x: 3, y: 0, w: 2, h: 5 },
    { x: 3, y: 6, w: 2, h: 2 },
  ]),
  chart: S8([
    { x: 0, y: 6, w: 2, h: 2 },
    { x: 3, y: 3, w: 2, h: 5 },
    { x: 6, y: 0, w: 2, h: 8 },
  ]),
  key: S8([
    { x: 0, y: 2, w: 4, h: 4 },
    { x: 2, y: 3, w: 2, h: 2, fill: '--bg' },
    { x: 4, y: 3, w: 4, h: 1 },
    { x: 6, y: 4, w: 1, h: 1 },
    { x: 4, y: 5, w: 2, h: 1 },
  ]),
  phone: S8([
    { x: 2, y: 0, w: 4, h: 8 },
    { x: 3, y: 1, w: 2, h: 5, fill: '--bg' },
    { x: 3, y: 7, w: 2, h: 1 },
  ]),
  /** 16×16 工头头像（琥珀安全帽 + 纸白脸 + 绿背心），只出现在 dossier 对话框。 */
  foreman: {
    viewBox: 16,
    size: 48,
    rects: [
      { x: 5, y: 0, w: 6, h: 2, fill: '--pending' },
      { x: 3, y: 2, w: 10, h: 2, fill: '--pending' },
      { x: 4, y: 4, w: 8, h: 5, fill: '--text-hi' },
      { x: 5, y: 6, w: 2, h: 2, fill: '--ink' },
      { x: 9, y: 6, w: 2, h: 2, fill: '--ink' },
      { x: 3, y: 9, w: 10, h: 5, fill: '--go' },
      { x: 2, y: 9, w: 1, h: 3, fill: '--go' },
      { x: 13, y: 9, w: 1, h: 3, fill: '--go' },
      { x: 4, y: 14, w: 3, h: 2, fill: '--text-2' },
      { x: 9, y: 14, w: 3, h: 2, fill: '--text-2' },
    ],
  },
};

/** 挥锤小人双帧（8×8，帧切换为离散 opacity 翻转）。 */
export const WORKER_FRAMES: { readonly raised: readonly SpriteRect[]; readonly struck: readonly SpriteRect[] } = {
  raised: [
    { x: 3, y: 1, w: 2, h: 2 },
    { x: 3, y: 3, w: 2, h: 3 },
    { x: 2, y: 6, w: 1, h: 2 },
    { x: 4, y: 6, w: 1, h: 2 },
    { x: 5, y: 2, w: 1, h: 1 },
    { x: 6, y: 0, w: 2, h: 2 },
    { x: 6, y: 2, w: 1, h: 1 },
  ],
  struck: [
    { x: 3, y: 1, w: 2, h: 2 },
    { x: 3, y: 3, w: 2, h: 3 },
    { x: 2, y: 6, w: 1, h: 2 },
    { x: 4, y: 6, w: 1, h: 2 },
    { x: 5, y: 4, w: 1, h: 1 },
    { x: 6, y: 5, w: 2, h: 1 },
    { x: 6, y: 7, w: 1, h: 1 },
  ],
};

/** 状态 → 灯色 / 描边 / 小人节奏（theme-6 §2.1 规则 + 原型实测）。 */
export interface StateStyle {
  /** 信号灯实心色 token；`null` = 无灯（queued / waiting 用灰字，不点亮）。 */
  readonly lamp: ColorToken | null;
  /** 货箱描边 token。 */
  readonly border: ColorToken;
  /** 列头小人节奏。 */
  readonly worker: 'run' | 'wait' | 'idle';
  /** 小人周期（秒）；`null` = 静止。 */
  readonly workerPeriod: number | null;
}

export const STATE_STYLES: Readonly<Record<CrateState, StateStyle>> = {
  running: { lamp: '--go', border: '--pane', worker: 'run', workerPeriod: 0.6 },
  pending: { lamp: '--pending', border: '--pending', worker: 'wait', workerPeriod: 1.8 },
  failed: { lamp: '--stop', border: '--stop', worker: 'idle', workerPeriod: null },
  done: { lamp: '--done', border: '--pane', worker: 'idle', workerPeriod: null },
  queued: { lamp: null, border: '--pane', worker: 'idle', workerPeriod: null },
  waiting: { lamp: null, border: '--pane', worker: 'idle', workerPeriod: null },
};

/** 浅色专属偏差（两处，theme-6 §2.4）。 */
export const LIGHT_DEVIATIONS = {
  /** ① wordmark 去投影：浅底高对比下 24px 紧排像素字会成重影。 */
  wordmarkShadow: 'none',
  /** ② 工头脸块固定肤色（不用 --text-hi，否则浅色下变墨块）。 */
  foremanFace: '#E3C7A6',
} as const;

/** 深色「夜班靛」——`app.css` 的 `:root` 默认。 */
export const DARK_COLORS: Readonly<Record<ColorToken, string>> = {
  '--bg': '#1B1D2C',
  '--panel': '#232639',
  '--wash': '#2B2F47',
  '--pane': '#3E4363',
  '--ink': '#12131E',
  '--hairline': '#232639',
  '--text-hi': '#F1ECDC',
  '--text': '#C7C3B4',
  '--text-2': '#918E9F',
  '--text-3': '#6E6C82',
  '--text-4': '#55536B',
  '--go': '#55D97C',
  '--go-hi': '#6FE693',
  '--go-ink': '#0B2314',
  '--pending': '#FFB545',
  '--stop': '#FF6157',
  '--done': '#6E6C82',
  '--branch-dev': '#59A7FF',
  '--branch-tst': '#C08BFF',
  '--diff-add': '#57D97C',
  '--diff-add-bg': '#16301F',
  '--diff-del': '#FF7B6E',
  '--diff-del-bg': '#361A20',
  '--belt-lit': '#4E5478',
};

/** 浅色「掌机背光」——`html[data-theme='light']` 覆盖。 */
export const LIGHT_COLORS: Readonly<Record<ColorToken, string>> = {
  '--bg': '#E8E6DC',
  '--panel': '#F6F4EA',
  '--wash': '#DEDBCE',
  '--pane': '#9A97A8',
  '--ink': '#2A2B3A',
  '--hairline': '#D8D5C8',
  '--text-hi': '#14151F',
  '--text': '#2F3040',
  '--text-2': '#55566A',
  '--text-3': '#7A7B8E',
  '--text-4': '#9A9BAC',
  '--go': '#1F7A3C',
  '--go-hi': '#175F2F',
  '--go-ink': '#F6F4EA',
  '--pending': '#8F5B00',
  '--stop': '#B3271E',
  '--done': '#7A7B8E',
  '--branch-dev': '#1D5FA8',
  '--branch-tst': '#6F3BB8',
  '--diff-add': '#1D6B3C',
  '--diff-add-bg': '#DDEBDD',
  '--diff-del': '#A02C22',
  '--diff-del-bg': '#F4DEDA',
  '--belt-lit': '#7E8094',
};

export const COLORS: Readonly<Record<ThemeName, Readonly<Record<ColorToken, string>>>> = {
  dark: DARK_COLORS,
  light: LIGHT_COLORS,
};

/** 全站唯一字族（缝合像素 12px 等宽）+ 防御性回退。 */
export const FONT_FAMILY =
  '"Fusion Pixel 12px Monospaced Simplified Chinese", "SF Mono", Menlo, "PingFang SC", monospace';

/**
 * 量表点亮段数：16 段映射 0–64k tok，非零值至少点亮 1 段（0 值不点亮，不假装有量）。
 */
export function gaugeFilled(tokens: number): number {
  if (!Number.isFinite(tokens) || tokens <= 0) return 0;
  return Math.min(GEOMETRY.gaugeSegments, Math.max(1, Math.round((tokens / GEOMETRY.gaugeFullTokens) * GEOMETRY.gaugeSegments)));
}

/**
 * boss 战尝试条点亮段数：`已用 / 上限` 折算 20 段；
 * 最后一次尝试（已用 ≥ 上限）整条转红（由调用方按 `exhausted` 取色）。
 */
export function bossFilled(used: number, limit: number): number {
  if (!Number.isFinite(used) || !Number.isFinite(limit) || limit <= 0) return 0;
  return Math.min(GEOMETRY.bossSegments, Math.max(0, Math.round((used / limit) * GEOMETRY.bossSegments)));
}

/** 是否处于最后一次尝试（整条转红）。 */
export function bossExhausted(used: number, limit: number): boolean {
  return limit > 0 && used >= limit;
}
