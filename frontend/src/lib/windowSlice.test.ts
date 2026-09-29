import { describe, expect, it } from 'vitest';
import { DEFAULT_PAGE, nextPage, windowSlice } from './windowSlice';

/**
 * 长列表切片原语（票 01 / spec list-windowing）：全站确认零分页零虚拟化之后的统一做法
 * 是**前端切片**——缺省 50 条/页、「加载更多」按钮（不做滚动自动加载）、头/尾两种锚定。
 * 三处接线（命令 / 会话 / 看板列）共用这一份判据。
 */
function seq(n: number): number[] {
  return Array.from({ length: n }, (_, i) => i + 1);
}

describe('windowSlice：尾部锚定（命令 / 会话缺省——最新最有用）', () => {
  it('超上限：只留尾部 50 条，前面省掉的数量算得对', () => {
    const s = windowSlice(seq(137), DEFAULT_PAGE, 'tail');
    expect(s.total).toBe(137);
    expect(s.visible).toHaveLength(50);
    expect(s.visible[0]).toBe(88);
    expect(s.visible[49]).toBe(137);
    expect(s.omittedBefore).toBe(87);
    expect(s.omittedAfter).toBe(0);
  });

  it('没超上限：全量可见，两头都不省', () => {
    const s = windowSlice(seq(50), DEFAULT_PAGE, 'tail');
    expect(s.visible).toEqual(seq(50));
    expect(s.omittedBefore).toBe(0);
    expect(s.omittedAfter).toBe(0);
  });

  it('空列表：可见为空、不省任何条', () => {
    const s = windowSlice([], DEFAULT_PAGE, 'tail');
    expect(s.visible).toEqual([]);
    expect(s.omittedBefore).toBe(0);
    expect(s.omittedAfter).toBe(0);
    expect(s.total).toBe(0);
  });

  it('shown 覆盖到超过总量：等价于全量', () => {
    const s = windowSlice(seq(60), 80, 'tail');
    expect(s.visible).toEqual(seq(60));
    expect(s.omittedBefore).toBe(0);
  });

  it('shown 为 0：一条不显示，全部算省略', () => {
    const s = windowSlice(seq(10), 0, 'tail');
    expect(s.visible).toEqual([]);
    expect(s.omittedBefore).toBe(10);
  });
});

describe('windowSlice：头部锚定（看板列缺省——先看见先来的）', () => {
  it('超上限：留头部 50 条，后面省掉的数量算得对', () => {
    const s = windowSlice(seq(137), DEFAULT_PAGE, 'head');
    expect(s.visible).toEqual(seq(50));
    expect(s.omittedBefore).toBe(0);
    expect(s.omittedAfter).toBe(87);
  });

  it('省略计数互斥：头部锚定永不说「前面省了」', () => {
    const s = windowSlice(seq(51), 50, 'head');
    expect(s.omittedBefore).toBe(0);
    expect(s.omittedAfter).toBe(1);
  });
});

describe('nextPage：加载更多的游标推进', () => {
  it('每次推进一页', () => {
    expect(nextPage(50, 137)).toBe(100);
    expect(nextPage(100, 137)).toBe(137);
  });

  it('钳在总量上——多点几下不会把游标推过数据', () => {
    expect(nextPage(120, 137)).toBe(137);
    expect(nextPage(137, 137)).toBe(137);
  });

  it('页大小可覆盖', () => {
    expect(nextPage(0, 1000, 200)).toBe(200);
  });
});
