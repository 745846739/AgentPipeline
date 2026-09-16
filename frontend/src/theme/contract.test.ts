/**
 * 主题契约模块单测（决策 169 / 规格 Testing Decisions 第 1 条）。
 *
 * 纯数据断言，最快的失败面：几何常量、sprite 表、状态映射、深浅两套 token 名。
 * 断言的是**契约与规格一致**，不是组件实现——这是防漂移的第一道护栏。
 */
import { describe, expect, it } from 'vitest';
import {
  SPRITES,
  STATE_STYLES,
  WORKER_FRAMES,
  GEOMETRY,
  DARK_COLORS,
  LIGHT_COLORS,
  LIGHT_DEVIATIONS,
  gaugeFilled,
  bossFilled,
  bossExhausted,
  type CrateState,
  type SpriteName,
} from './contract';

const SPRITE_NAMES: SpriteName[] = [
  'flag',
  'gem',
  'hammer',
  'flask',
  'gear',
  'lens',
  'shield',
  'merge',
  'trophy',
  'chest',
  'alert',
  'chart',
  'key',
  'phone',
  'foreman',
];

describe('主题契约 · 几何常量（theme-6-pixel.md §2.3）', () => {
  it('像素纪律：0 圆角、2px 一档描边、两级硬投影', () => {
    expect(GEOMETRY.radius).toBe(0);
    expect(GEOMETRY.border).toBe(2);
    expect(GEOMETRY.shadow).toBe('4px 4px 0');
    expect(GEOMETRY.shadowControl).toBe('3px 3px 0');
  });

  it('字号只取 12 的整数倍', () => {
    expect(GEOMETRY.fontSizes).toEqual([12, 24, 36]);
    expect(GEOMETRY.fontSizes.every((s) => s % 12 === 0)).toBe(true);
  });

  it('量表 16 段 / boss 条 20 段 / 槽位 34px / 列宽 264px × 8', () => {
    expect(GEOMETRY.gaugeSegments).toBe(16);
    expect(GEOMETRY.bossSegments).toBe(20);
    expect(GEOMETRY.slot).toBe(34);
    expect(GEOMETRY.columnWidth).toBe(264);
    expect(GEOMETRY.columnCount).toBe(8);
  });

  it('宽度：详情 1000px / dossier 340px / 分栏 1240px（对账冻结原型）', () => {
    expect(GEOMETRY.detailMax).toBe(1000);
    expect(GEOMETRY.dossierWidth).toBe(340);
    expect(GEOMETRY.splitMax).toBe(1240);
  });

  it('传送带 6px 链节（周期 12px）、12px 信号灯、6px 顶盖带', () => {
    expect(GEOMETRY.belt).toBe(6);
    expect(GEOMETRY.beltPeriod).toBe(12);
    expect(GEOMETRY.lamp).toBe(12);
    expect(GEOMETRY.crateLid).toBe(6);
  });

  // 决策 196 起断点有两条：480px 是移动款那一条，1400px 是**桌面款内部**的看板钉右档。
  // 旧口径「唯一媒体断点」由决策 196 显式开例外（`.scratch/agentpipeline-pixel-theme/spec.md`
  // 的 Out of Scope 那条已就地标注），故这里不再写「唯一」。
  it('媒体断点两条：移动款 480px + 看板钉右档 1400px', () => {
    expect(GEOMETRY.mobileBreakpoint).toBe(480);
    expect(GEOMETRY.boardPinBreakpoint).toBe(1400);
  });

  it('dither 周期 4px 与量表满格 ≈64k tok', () => {
    expect(GEOMETRY.ditherPeriod).toBe(4);
    expect(GEOMETRY.gaugeFullTokens).toBe(64_000);
  });
});

describe('主题契约 · sprite 表（受控 15 枚）', () => {
  it('恰好 15 枚且名字齐备', () => {
    expect(Object.keys(SPRITES).sort()).toEqual([...SPRITE_NAMES].sort());
    expect(Object.keys(SPRITES)).toHaveLength(15);
  });

  it.each(SPRITE_NAMES)('%s：有 viewBox 且 rect 非空、全在网格内', (name) => {
    const sp = SPRITES[name];
    expect(sp.viewBox).toBeGreaterThan(0);
    expect(sp.rects.length).toBeGreaterThan(0);
    for (const r of sp.rects) {
      expect(r.w).toBeGreaterThan(0);
      expect(r.h).toBeGreaterThan(0);
      expect(r.x).toBeGreaterThanOrEqual(0);
      expect(r.y).toBeGreaterThanOrEqual(0);
      expect(r.x + r.w).toBeLessThanOrEqual(sp.viewBox);
      expect(r.y + r.h).toBeLessThanOrEqual(sp.viewBox);
    }
  });

  it('8×8 图元用 8 网格；工头头像用 16 网格', () => {
    for (const name of SPRITE_NAMES.filter((n) => n !== 'foreman')) {
      expect(SPRITES[name].viewBox).toBe(8);
    }
    expect(SPRITES.foreman.viewBox).toBe(16);
  });

  it('未指定 fill 的 rect 用 currentColor（缺省），工头用 token 名', () => {
    expect(SPRITES.flag.rects.every((r) => r.fill === undefined)).toBe(true);
    expect(SPRITES.foreman.rects.some((r) => r.fill === '--pending')).toBe(true);
    expect(SPRITES.foreman.rects.some((r) => r.fill === '--go')).toBe(true);
  });

  it('挥锤小人双帧各有 rect', () => {
    expect(WORKER_FRAMES.raised.length).toBeGreaterThan(0);
    expect(WORKER_FRAMES.struck.length).toBeGreaterThan(0);
  });
});

describe('主题契约 · 状态映射（覆盖六种状态）', () => {
  const states: CrateState[] = ['running', 'pending', 'failed', 'done', 'queued', 'waiting'];

  it('六种状态齐备', () => {
    expect(Object.keys(STATE_STYLES).sort()).toEqual([...states].sort());
  });

  it.each(states)('%s：灯 / 描边 / 小人节奏齐备', (state) => {
    const s = STATE_STYLES[state];
    expect(s.border).toMatch(/^--/);
    expect(['run', 'wait', 'idle']).toContain(s.worker);
  });

  it('灯即状态：绿=执行、琥珀=等人、红=失败、灰=归档', () => {
    expect(STATE_STYLES.running.lamp).toBe('--go');
    expect(STATE_STYLES.pending.lamp).toBe('--pending');
    expect(STATE_STYLES.failed.lamp).toBe('--stop');
    expect(STATE_STYLES.done.lamp).toBe('--done');
  });

  it('queued / waiting 无灯（灰字，不点亮）', () => {
    expect(STATE_STYLES.queued.lamp).toBeNull();
    expect(STATE_STYLES.waiting.lamp).toBeNull();
  });

  it('pending 是唯一琥珀描边；failed 是唯一红描边', () => {
    const amber = states.filter((s) => STATE_STYLES[s].border === '--pending');
    const red = states.filter((s) => STATE_STYLES[s].border === '--stop');
    expect(amber).toEqual(['pending']);
    expect(red).toEqual(['failed']);
  });

  it('小人节奏：run 0.6s / wait 1.8s / 其余静止', () => {
    expect(STATE_STYLES.running.workerPeriod).toBe(0.6);
    expect(STATE_STYLES.pending.workerPeriod).toBe(1.8);
    expect(STATE_STYLES.done.workerPeriod).toBeNull();
    expect(STATE_STYLES.queued.workerPeriod).toBeNull();
  });
});

describe('主题契约 · 色彩 token（深浅两套覆盖同一组名）', () => {
  it('两套 token 名集合一致', () => {
    expect(Object.keys(DARK_COLORS).sort()).toEqual(Object.keys(LIGHT_COLORS).sort());
  });

  it('值都是合法 CSS 颜色（十六进制）', () => {
    for (const v of Object.values(DARK_COLORS)) expect(v).toMatch(/^#[0-9a-f]{6}$/i);
    for (const v of Object.values(LIGHT_COLORS)) expect(v).toMatch(/^#[0-9a-f]{6}$/i);
  });

  it('关键像素 token 逐条对齐规格 §2.1 / §2.4', () => {
    expect(DARK_COLORS['--bg']).toBe('#1B1D2C');
    expect(DARK_COLORS['--pending']).toBe('#FFB545');
    expect(DARK_COLORS['--text-hi']).toBe('#F1ECDC');
    expect(DARK_COLORS['--branch-dev']).toBe('#59A7FF');
    expect(DARK_COLORS['--branch-tst']).toBe('#C08BFF');
    expect(DARK_COLORS['--belt-lit']).toBe('#4E5478');
    expect(LIGHT_COLORS['--bg']).toBe('#E8E6DC');
    expect(LIGHT_COLORS['--pending']).toBe('#8F5B00');
    expect(LIGHT_COLORS['--hairline']).toBe('#D8D5C8');
  });

  it('浅色两处偏差已登记', () => {
    expect(LIGHT_DEVIATIONS.wordmarkShadow).toBe('none');
    expect(LIGHT_DEVIATIONS.foremanFace).toBe('#E3C7A6');
  });

  // 决策 195 ⑤ / §2.6 裁决四：--text-3 提到次级必读档（对比度门在 theme/contrast.test.ts），
  // --done **原地不动**——它是信号色「归档灰，刻意退后」，提亮会抹掉归档语义并连带改灯 / 量表 /
  // 条形图。两者曾共用同一字面值，从 195 起不再相同是**预期的**（parity 逐 token 比对不会红）。
  it('--text-3 提档、--done 不动，且不与 --text-2 合并（决策 195 / §2.6）', () => {
    expect(DARK_COLORS['--done']).toBe('#6E6C82');
    expect(LIGHT_COLORS['--done']).toBe('#7A7B8E');
    expect(DARK_COLORS['--text-3']).not.toBe(DARK_COLORS['--done']);
    expect(LIGHT_COLORS['--text-3']).not.toBe(LIGHT_COLORS['--done']);
    expect(DARK_COLORS['--text-3']).not.toBe(DARK_COLORS['--text-2']);
    expect(LIGHT_COLORS['--text-3']).not.toBe(LIGHT_COLORS['--text-2']);
  });
});

describe('主题契约 · 量表折算', () => {
  it('token 量表：0 不点亮、非零至少 1 段、满格 16 段', () => {
    expect(gaugeFilled(0)).toBe(0);
    expect(gaugeFilled(-5)).toBe(0);
    expect(gaugeFilled(1)).toBe(1);
    expect(gaugeFilled(64_000)).toBe(16);
    expect(gaugeFilled(999_999)).toBe(16);
    expect(gaugeFilled(32_000)).toBe(8);
  });

  it('boss 条：按已用/上限折算，最后一次尝试整条转红', () => {
    expect(bossFilled(0, 3)).toBe(0);
    expect(bossFilled(2, 3)).toBe(13);
    expect(bossFilled(3, 3)).toBe(20);
    expect(bossExhausted(2, 3)).toBe(false);
    expect(bossExhausted(3, 3)).toBe(true);
    expect(bossExhausted(3, 0)).toBe(false);
  });
});
