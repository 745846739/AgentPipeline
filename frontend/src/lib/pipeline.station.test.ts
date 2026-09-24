import { describe, expect, it } from 'vitest';

import { aggregateStationState } from './pipeline';
import type { TaskStatus } from '../api/types';

/**
 * 工位灯聚合（决策 251②）。
 *
 * 「一个工位该亮哪盏灯」是在**看板 8 列**、**顶栏灯带**与**对讲台值班板**三处都要答的
 * 同一个问题，规格 `design/theme-6-pixel.md:628` 明写值班板是「同一份读数……**这是第三份**」。
 * 故判据只有这一处实现，三处都读它——此前 `BoardColumn.svelte` 与 `Talk.svelte` 各推了一遍，
 * 而 Talk 那份**并进了 `idle`**：一个工位的任务全部失败时画的是空灯框，与看板同一列的红灯打架。
 *
 * 优先序取**看板的可见次序**（决策 251②）：急停 > 在跑 > 失败 > 归档 > 空。
 * 它不是随便定的——「在跑优先于失败」是说「这个工位还在动」，而「失败优先于在跑」
 * 会把一个正在重试的工位说成红的。
 */
describe('工位灯聚合（决策 251②）', () => {
  const t = (...statuses: TaskStatus[]): TaskStatus[] => statuses;

  it('四盏灯各自点亮：pending → warn、running → go、failed → stop、全 done → done', () => {
    expect(aggregateStationState(t('pending'))).toBe('warn');
    expect(aggregateStationState(t('running'))).toBe('go');
    expect(aggregateStationState(t('failed'))).toBe('stop');
    expect(aggregateStationState(t('done'))).toBe('done');
  });

  it('cancelled 也点失败灯——它与 failed 是同一档「这个工位不动了」', () => {
    expect(aggregateStationState(t('cancelled'))).toBe('stop');
  });

  it('优先序：pending 压过 running 与 failed（急停最急）', () => {
    expect(aggregateStationState(t('running', 'pending'))).toBe('warn');
    expect(aggregateStationState(t('failed', 'pending'))).toBe('warn');
  });

  it('优先序：running 压过 failed（还在动就不说它红了）', () => {
    expect(aggregateStationState(t('failed', 'running'))).toBe('go');
  });

  it('全 done 才算 done——有一个还在跑就不是', () => {
    expect(aggregateStationState(t('done', 'done'))).toBe('done');
    expect(aggregateStationState(t('done', 'running'))).toBe('go');
  });

  it('空工位 → idle；queued / waiting 这种「还没开始」也是 idle', () => {
    expect(aggregateStationState(t())).toBe('idle');
    expect(aggregateStationState(t('queued'))).toBe('idle');
    expect(aggregateStationState(t('waiting'))).toBe('idle');
  });

  it('多个成功已完成但仍混着 queued → idle，不是 done', () => {
    expect(aggregateStationState(t('done', 'queued'))).toBe('idle');
  });
});
