import type { GlobalMetrics, Stage, StageMetric } from '../api/types';
import { RAIL_LABELS, RAIL_STAGES, formatDuration } from './pipeline';

/**
 * 指标 → 轨道分段条形图的纯映射（design §7 / 决策 130 / 137）。
 *
 * 后端字段口径（crates/core/src/metrics.rs 与 routes/tasks.rs）：
 * - `GET /metrics`：`success_rate`（done / (done+failed+cancelled)）· `stage_aggregation`
 *   （stage / avg_duration_ms / retry_rate / total_runs）· `escape_events`（`[from_stage|null, count]`）；
 * - `GET /tasks/{id}/metrics`：token 汇总 + `stages` + `validate_first_pass_rate`。
 *
 * 视觉：条形图挂在轨道站点下（design §7），站点顺序/标签复用 Board 的 9 站（决策 107，去 sync-check）。
 *
 * **说人话与「分母为 0」的处置（UX 审计票 27 / 用户故事 22）**：字段口径是内部事实，
 * 页面第一段用平实说法讲清四个量各自是什么、怎么算的；某个比值算不出来时，
 * 由下面几条 `*_REASON` 说明**为什么暂时没有意义**，而不是只给一条没有解释的横线。
 */

export type MetricTone = 'go' | 'caution' | 'stop' | 'done' | 'dev' | 'test';

export interface MetricBar {
  key: string;
  label: string;
  value: number;
  /** 相对 0..1 的条长（不是百分比文本）。 */
  pct: number;
  display: string;
  tone: MetricTone;
}

const RAIL_ORDER = new Map<Stage, number>(RAIL_STAGES.map((s, i) => [s, i]));

function clamp01(n: number): number {
  if (!Number.isFinite(n) || n <= 0) return 0;
  return n > 1 ? 1 : n;
}

export function railStageLabel(stage: string): string {
  return RAIL_LABELS[stage] ?? stage;
}

/** 只保留 9 个轨道站点（sync-check / 未知阶段不属于任何站，决策 107）。 */
export function orderedStageMetrics(metrics: StageMetric[]): StageMetric[] {
  return metrics
    .filter((m) => RAIL_ORDER.has(m.stage))
    .sort((a, b) => RAIL_ORDER.get(a.stage)! - RAIL_ORDER.get(b.stage)!);
}

/** 被轨道图排除的阶段（用于 UI 明确提示，而不是静默丢弃）。 */
export function excludedStageMetrics(metrics: StageMetric[]): StageMetric[] {
  return metrics.filter((m) => !RAIL_ORDER.has(m.stage));
}

export function formatPercent(rate: number | null | undefined): string {
  if (rate === null || rate === undefined || Number.isNaN(rate)) return '—';
  return `${(rate * 100).toFixed(1)}%`;
}

export function formatDurationMs(ms: number): string {
  if (!Number.isFinite(ms)) return '—';
  return formatDuration(Math.round(ms));
}

export function formatCount(n: number): string {
  return n.toLocaleString('en-US');
}

/**
 * 分母为 0 的比值：页面显示的原因（票 27 / 用户故事 22）。
 *
 * 全部是**说明性段落**——按整备规格的 Testing Decisions，不做文案精确匹配断言；
 * 测试只断言「有值时有数值、没值时有解释」这一行为。
 */
export const NO_SETTLED_TASKS_REASON =
  '还没有跑完的任务（成功、失败、取消都算跑完，正在跑的不算），所以这个比值暂时没有意义。';

export const NO_VALIDATE_RUNS_REASON =
  '还没有质量检查的记录，所以这个比值暂时没有意义（这里不画一条 0 冒充真实值）。';

export const NO_VALIDATE_RUNS_TASK_REASON =
  '这个任务还没做过质量检查，所以首过率暂时没有意义（这里不画 0 冒充真实值）。';

/** 相对最大值归一化（最大值 0 → 全 0）。 */
function relativePct(value: number, max: number): number {
  if (!Number.isFinite(value) || value <= 0 || max <= 0) return 0;
  return clamp01(value / max);
}

/* ────────────────────────── 全局 /metrics ────────────────────────── */

/**
 * 成功率：单条轨道条（pct 直接用 0..1 的比率）。
 *
 * 分母为 0（还没有跑完的任务）时**不画条**，交页面用 `NO_SETTLED_TASKS_REASON` 说明原因
 * ——与首过率同一处置（票 27）：一条没有解释的横线读者分不清是坏了还是本来就不适用。
 */
export function successBar(rate: number | null | undefined): MetricBar[] {
  if (rate === null || rate === undefined || Number.isNaN(rate)) return [];
  return [
    {
      key: 'success_rate',
      label: '成功率',
      value: rate,
      pct: clamp01(rate),
      display: formatPercent(rate),
      tone: 'go',
    },
  ];
}

/** 各阶段平均耗时：条长按最长阶段归一。 */
export function durationBars(metrics: StageMetric[]): MetricBar[] {
  const ordered = orderedStageMetrics(metrics);
  const max = Math.max(0, ...ordered.map((m) => m.avg_duration_ms));
  return ordered.map((m) => ({
    key: `dur-${m.stage}`,
    label: railStageLabel(m.stage),
    value: m.avg_duration_ms,
    pct: relativePct(m.avg_duration_ms, max),
    display: formatDurationMs(m.avg_duration_ms),
    tone: 'dev' as const,
  }));
}

/** 各阶段重试率：比率本身就是 0..1，条长按绝对值。
 *
 *  **灯色随值取**（UX 审计票 12 / 票 01 的逐处判定）：视觉规格 §3.1 明文允许这张图的灯与条
 *  用 `caution`（`--pending`），但同时写明「琥珀仍是全站唯一告警」——所以「有重试」才配得上
 *  这个颜色，而**值全是 0.0% 也照样是琥珀**正说明它当时退化成了刻度装饰。故 0 值走与「各阶段
 *  平均耗时」同一档的中性材质色（`dev` / `--belt-lit`），有值才取 `caution`。
 *  灯与条同色是硬要求，此处一并由 `tone` 承担（消费方 `TrackSegmentBars` 两者同取一档）。 */
export function retryBars(metrics: StageMetric[]): MetricBar[] {
  return orderedStageMetrics(metrics).map((m) => ({
    key: `retry-${m.stage}`,
    label: railStageLabel(m.stage),
    value: m.retry_rate,
    pct: clamp01(m.retry_rate),
    display: formatPercent(m.retry_rate),
    tone: m.retry_rate > 0 ? ('caution' as const) : ('dev' as const),
  }));
}

/** 逃逸事件（kickback 按 from_stage 分组）：条长按最大事件数归一。 */
export function escapeBars(events: Array<[string | null, number]>): MetricBar[] {
  const byStage = new Map<string, number>();
  for (const entry of events) {
    if (!Array.isArray(entry)) continue;
    const [stage, count] = entry;
    if (!stage || typeof count !== 'number') continue;
    byStage.set(stage, (byStage.get(stage) ?? 0) + count);
  }
  const ordered = RAIL_STAGES.filter((s) => byStage.has(s)).map((s) => ({
    stage: s,
    count: byStage.get(s)!,
  }));
  const max = Math.max(0, ...ordered.map((o) => o.count));
  return ordered.map((o) => ({
    key: `escape-${o.stage}`,
    label: railStageLabel(o.stage),
    value: o.count,
    pct: relativePct(o.count, max),
    display: formatCount(o.count),
    tone: 'stop' as const,
  }));
}

/** 逃逸事件总数（0 也如实展示）。 */
export function escapeTotal(events: Array<[string | null, number]>): number {
  return escapeBars(events).reduce((sum, b) => sum + b.value, 0);
}

/** 首过率条（全局字段缺失时返回空数组，由 UI 明确标注「未提供」）。 */
export function firstPassBars(rate: number | null | undefined): MetricBar[] {
  if (rate === null || rate === undefined) return [];
  return [
    {
      key: 'first_pass',
      label: '首过率',
      value: rate,
      pct: clamp01(rate),
      display: formatPercent(rate),
      tone: 'done',
    },
  ];
}

export interface GlobalMetricsView {
  success: MetricBar[];
  duration: MetricBar[];
  retry: MetricBar[];
  escape: MetricBar[];
  firstPass: MetricBar[];
  /** 成功率分母为 0 时要显示的原因；有值时是空串（`TrackSegmentBars` 只在没有条时用它）。 */
  successReason: string;
  /** 首过率缺数据时要显示的原因；**有值时是空串**——「这一项现在有没有意义」读这一个字段就够（空串即有意义），不再单列一个布尔。 */
  firstPassReason: string;
  excludedStages: string[];
  escapeEvents: number;
  /** 全程 token 求和与 LLM 调用数（design §7「token 消耗」）。 */
  tokenDisplay: string;
  callsDisplay: string;
}

/** 把 `GET /metrics` 一次映射成各图的数据（供页面直接渲染，便于单测）。 */
export function mapGlobalMetrics(metrics: GlobalMetrics): GlobalMetricsView {
  const stageAgg = Array.isArray(metrics.stage_aggregation) ? metrics.stage_aggregation : [];
  const escapes = Array.isArray(metrics.escape_events) ? metrics.escape_events : [];
  const firstPassAvailable = metrics.validate_first_pass_rate !== null &&
    metrics.validate_first_pass_rate !== undefined;
  const successAvailable = metrics.success_rate !== null && metrics.success_rate !== undefined;
  return {
    success: successBar(metrics.success_rate),
    duration: durationBars(stageAgg),
    retry: retryBars(stageAgg),
    escape: escapeBars(escapes),
    firstPass: firstPassBars(metrics.validate_first_pass_rate),
    successReason: successAvailable ? '' : NO_SETTLED_TASKS_REASON,
    firstPassReason: firstPassAvailable ? '' : NO_VALIDATE_RUNS_REASON,
    excludedStages: excludedStageMetrics(stageAgg).map((m) => m.stage),
    escapeEvents: escapeTotal(escapes),
    tokenDisplay: formatCount(metrics.total_tokens ?? 0),
    callsDisplay: formatCount(metrics.total_calls ?? 0),
  };
}

/* ────────────────────────── 任务级 /tasks/{id}/metrics ────────────────────────── */

export interface TaskMetricsView {
  duration: MetricBar[];
  retry: MetricBar[];
  firstPass: MetricBar[];
  tokenDisplay: string;
  callsDisplay: string;
  storedTokenDisplay: string;
  storedCallsDisplay: string;
  /** 求和口径与持久化口径是否存在差异（观测提示，不是错误）。 */
  tokenDrift: boolean;
  callsDrift: boolean;
}

export function mapTaskMetrics(metrics: {
  total_tokens: number;
  total_calls: number;
  stored_total_tokens: number;
  stored_total_calls: number;
  stages: StageMetric[];
  validate_first_pass_rate: number | null;
}): TaskMetricsView {
  return {
    duration: durationBars(metrics.stages ?? []),
    retry: retryBars(metrics.stages ?? []),
    firstPass: firstPassBars(metrics.validate_first_pass_rate),
    tokenDisplay: formatCount(metrics.total_tokens),
    callsDisplay: formatCount(metrics.total_calls),
    storedTokenDisplay: formatCount(metrics.stored_total_tokens),
    storedCallsDisplay: formatCount(metrics.stored_total_calls),
    tokenDrift: metrics.total_tokens !== metrics.stored_total_tokens,
    callsDrift: metrics.total_calls !== metrics.stored_total_calls,
  };
}
