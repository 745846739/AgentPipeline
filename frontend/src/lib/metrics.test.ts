import { describe, expect, it } from 'vitest';
import type { StageMetric } from '../api/types';
import {
  durationBars,
  escapeBars,
  escapeTotal,
  excludedStageMetrics,
  firstPassBars,
  formatPercent,
  mapGlobalMetrics,
  mapTaskMetrics,
  orderedStageMetrics,
  retryBars,
  successBar,
} from './metrics';

function stageMetric(overrides: Partial<StageMetric> = {}): StageMetric {
  return {
    stage: 'develop',
    total_runs: 4,
    avg_duration_ms: 2000,
    retry_rate: 0.25,
    ...overrides,
  };
}

describe('指标字段映射（crates/core/src/metrics.rs 口径）', () => {
  it('阶段按轨道 9 站顺序排列，sync-check 被排除并报告', () => {
    const metrics: StageMetric[] = [
      stageMetric({ stage: 'test' }),
      stageMetric({ stage: 'sync-check' }),
      stageMetric({ stage: 'init' }),
      stageMetric({ stage: 'develop-design' }),
    ];
    expect(orderedStageMetrics(metrics).map((m) => m.stage)).toEqual([
      'init',
      'develop-design',
      'test',
    ]);
    expect(excludedStageMetrics(metrics).map((m) => m.stage)).toEqual(['sync-check']);
  });

  it('平均耗时条长按最大值归一，展示用时长格式', () => {
    const bars = durationBars([
      stageMetric({ stage: 'develop', avg_duration_ms: 1000 }),
      stageMetric({ stage: 'test', avg_duration_ms: 4000 }),
    ]);
    expect(bars.map((b) => b.label)).toEqual(['develop', 'test']);
    expect(bars[0].pct).toBeCloseTo(0.25);
    expect(bars[1].pct).toBe(1);
    expect(bars[1].display).toBe('4s');
  });

  it('重试率按绝对比率成条，展示百分比', () => {
    const bars = retryBars([
      stageMetric({ stage: 'develop', retry_rate: 0.5 }),
      stageMetric({ stage: 'test', retry_rate: 0 }),
    ]);
    expect(bars[0].pct).toBe(0.5);
    expect(bars[0].display).toBe('50.0%');
    expect(bars[1].pct).toBe(0);
  });

  it('逃逸事件按 from_stage 归一，null 阶段丢弃', () => {
    const events: Array<[string | null, number]> = [
      ['develop', 3],
      ['test', 1],
      [null, 5],
      ['review', 0],
    ];
    const bars = escapeBars(events);
    expect(bars.map((b) => b.key)).toEqual(['escape-develop', 'escape-review', 'escape-test']);
    expect(bars[0].pct).toBe(1);
    expect(bars[0].display).toBe('3');
    expect(escapeTotal(events)).toBe(4);
  });

  it('成功率 null → — 且条长为 0；正常值按比率', () => {
    expect(successBar(null)[0]).toMatchObject({ display: '—', pct: 0 });
    expect(successBar(0.75)[0]).toMatchObject({ display: '75.0%', pct: 0.75 });
    expect(formatPercent(undefined)).toBe('—');
  });

  it('首过率仅在字段存在时产生条（缺失为空，交 UI 标注）', () => {
    expect(firstPassBars(undefined)).toEqual([]);
    expect(firstPassBars(null)).toEqual([]);
    expect(firstPassBars(2 / 3)[0].display).toBe('66.7%');
  });
});

describe('mapGlobalMetrics', () => {
  it('一次映射出各图并标注排除阶段 / 首过率缺失', () => {
    const view = mapGlobalMetrics({
      tasks: 7,
      success_rate: 0.5,
      stage_aggregation: [
        stageMetric({ stage: 'develop', avg_duration_ms: 100, retry_rate: 0 }),
        stageMetric({ stage: 'sync-check', avg_duration_ms: 1, retry_rate: 0 }),
      ],
      escape_events: [['develop', 2]],
    });
    expect(view.success).toHaveLength(1);
    expect(view.duration.map((b) => b.label)).toEqual(['develop']);
    expect(view.retry).toHaveLength(1);
    expect(view.escape).toHaveLength(1);
    expect(view.firstPassAvailable).toBe(false);
    expect(view.firstPass).toEqual([]);
    expect(view.excludedStages).toEqual(['sync-check']);
    expect(view.escapeEvents).toBe(2);
    expect(view.tokenDisplay).toBe('0');
    expect(view.callsDisplay).toBe('0');
  });

  it('后端下发首过率与全局 token / 调用数时直接展示（ticket 22 全局面板）', () => {
    const view = mapGlobalMetrics({
      tasks: 3,
      success_rate: 2 / 3,
      stage_aggregation: [stageMetric({ stage: 'develop' })],
      escape_events: [],
      validate_first_pass_rate: 0.5,
      total_tokens: 12345,
      total_calls: 42,
    });
    expect(view.firstPassAvailable).toBe(true);
    expect(view.firstPass[0].display).toBe('50.0%');
    expect(view.tokenDisplay).toBe('12,345');
    expect(view.callsDisplay).toBe('42');
  });
});

describe('mapTaskMetrics', () => {
  it('映射 token / 调用 / 阶段，并标记求和与持久化口径差异', () => {
    const view = mapTaskMetrics({
      total_tokens: 180,
      total_calls: 3,
      stored_total_tokens: 175,
      stored_total_calls: 3,
      stages: [stageMetric({ stage: 'develop' })],
      validate_first_pass_rate: 1,
    });
    expect(view.duration).toHaveLength(1);
    expect(view.retry).toHaveLength(1);
    expect(view.firstPass[0].display).toBe('100.0%');
    expect(view.tokenDisplay).toBe('180');
    expect(view.storedTokenDisplay).toBe('175');
    expect(view.tokenDrift).toBe(true);
    expect(view.callsDrift).toBe(false);
  });
});
