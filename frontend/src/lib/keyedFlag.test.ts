import { describe, expect, it } from 'vitest';
import { isFlagged, setFlag } from './keyedFlag';

/**
 * 按 key 键控的在飞集合（票 12 / R2-17 的形状收口）。
 *
 * 这条形状此前手抄了五遍，抄漏一处就退回「单槽」的旧毛病：慢网络下先后装两个技能，
 * 先完成的那一次会把后一个的 pending 态一起清掉（那一行按钮复活、还能再点一次）。
 */
describe('在飞集合', () => {
  it('置位 / 清位只动自己那一格', () => {
    let flags: Record<string, true> = {};
    flags = setFlag(flags, 'a', true);
    flags = setFlag(flags, 'b', true);
    expect(isFlagged(flags, 'a')).toBe(true);
    expect(isFlagged(flags, 'b')).toBe(true);

    // a 完成：只清 a，b 的在飞态必须还在（否则 b 的转圈消失、按钮复活）
    flags = setFlag(flags, 'a', false);
    expect(isFlagged(flags, 'a')).toBe(false);
    expect(isFlagged(flags, 'b')).toBe(true);
  });

  it('返回新对象（Svelte 的 $state 靠赋值触发更新），原对象不动', () => {
    const before: Record<string, true> = { a: true };
    const after = setFlag(before, 'b', true);
    expect(after).not.toBe(before);
    expect(before).toEqual({ a: true });
  });

  it('清一个不在飞的 key 是 no-op；`null` key 恒不在飞', () => {
    const flags = setFlag({ a: true }, 'zzz', false);
    expect(flags).toEqual({ a: true });
    expect(isFlagged(flags, null)).toBe(false);
  });
});
