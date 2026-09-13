import { describe, expect, it } from 'vitest';
import { newlyDoneTaskIds } from './completion';

/**
 * 完成横幅的纯判定（票 08 第 5 条）：
 * 只在「非 done → done」迁移时命中；首屏 / 刷新（无前值）与 failed / cancelled 不命中。
 */
describe('newlyDoneTaskIds（任务完成横幅的弹出门槛）', () => {
  const prev = (entries: Array<[string, string]>) => new Map(entries);

  it('running → done：命中', () => {
    expect(newlyDoneTaskIds(prev([['t1', 'running']]), [{ id: 't1', status: 'done' }])).toEqual([
      't1',
    ]);
  });

  it('pending → done：命中', () => {
    expect(newlyDoneTaskIds(prev([['t1', 'pending']]), [{ id: 't1', status: 'done' }])).toEqual([
      't1',
    ]);
  });

  it('首次观测（刷新后首屏）：不命中，刷新不重弹', () => {
    expect(newlyDoneTaskIds(prev([]), [{ id: 't1', status: 'done' }])).toEqual([]);
  });

  it('已是 done 再观测：不命中（同一次完成只弹一次）', () => {
    expect(newlyDoneTaskIds(prev([['t1', 'done']]), [{ id: 't1', status: 'done' }])).toEqual([]);
  });

  it.each(['failed', 'cancelled'] as const)('%s 不命中（既有终态提示保留）', (status) => {
    expect(newlyDoneTaskIds(prev([['t1', 'running']]), [{ id: 't1', status }])).toEqual([]);
  });

  it('多任务：只挑出本批新完成者', () => {
    const hits = newlyDoneTaskIds(
      prev([
        ['t1', 'running'],
        ['t2', 'done'],
      ]),
      [
        { id: 't1', status: 'done' },
        { id: 't2', status: 'done' },
        { id: 't3', status: 'done' },
      ],
    );
    expect(hits).toEqual(['t1']);
  });
});
