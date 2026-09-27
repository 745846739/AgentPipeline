import { describe, expect, it } from 'vitest';
import type { ForemanSessionMeta } from '../api/types';
import { chipRow } from './sessionChips';

/**
 * 班次 chip 行的取舍（票 06）。
 *
 * 两条判据各有各的牙齿：
 * ① 缺省不列归档——退成「全都列」，「从列表里收起来」就名存实亡（决策 204⑦ 的口径
 *    在列表这一层的落点）；
 * ② 关掉开关**回到现状**：行里一个归档都不留，哪怕正读着的那一班也是——开关关着
 *    还挂着一枚灰 chip，「显示已归档」就在说谎（票面「关开关复原」那条 E2E）。
 */
function meta(id: string, over: Partial<ForemanSessionMeta> = {}): ForemanSessionMeta {
  return {
    id,
    title: `班次 ${id}`,
    kind: 'talk',
    created_at: '2026-09-25T09:00:00Z',
    last_active_at: '2026-09-25T10:00:00Z',
    archived_at: null,
    ...over,
  };
}

const active = [meta('a'), meta('b')];
const mixed = [meta('a'), meta('gone', { archived_at: '2026-09-25T11:00:00Z' })];
const reading = meta('gone', { archived_at: '2026-09-25T11:00:00Z' });

describe('chipRow：「显示已归档」开关（票 06）', () => {
  it('缺省：归档的不列，活跃的照列', () => {
    expect(chipRow(mixed, false).map((s) => s.id)).toEqual(['a']);
    expect(chipRow(active, false).map((s) => s.id)).toEqual(['a', 'b']);
  });

  it('打开：归档的照列（灰不灰由 archived_at 判，这里只管在不在行里）', () => {
    expect(chipRow(mixed, true).map((s) => s.id)).toEqual(['a', 'gone']);
  });

  it('关开关回到现状：**当前正在读的归档班也离开行**（开关不说谎；读它的落点别处兜）', () => {
    expect(chipRow([reading, ...active], false).map((s) => s.id)).toEqual(['a', 'b']);
    // 开着时它照列——「灰而选中」那条判据的可见表现是列出来之后的事
    expect(chipRow([reading, ...active], true).map((s) => s.id)).toEqual(['gone', 'a', 'b']);
  });

  it('空列表原样；活跃班原序不动', () => {
    expect(chipRow([], false)).toEqual([]);
    expect(chipRow(active, true).map((s) => s.id)).toEqual(['a', 'b']);
  });
});
