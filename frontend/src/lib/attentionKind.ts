/**
 * 待办类别的中文名（决策 307，票 executor-never-returns 06）。
 *
 * 页头那枚读数只报**条数**（一行几何），类别摘要落在它的 `title` 里——所以需要一张
 * 「落库值 → 人话」的表。键是**落库值**（`AttentionKind::as_str`），不是文档里的编号：
 * 后端加一类时 `attentionKind.test.ts` 会红，逼着这里同步（漏一类的症状是提示里出现
 * 一个英文标识，比不提示更让人困惑）。
 */
export const ATTENTION_KIND_LABELS: Record<string, string> = {
  task_pending: '任务待处理',
  retry_exhausted: '重试耗尽',
  environment_blocked: '环境受阻',
  context_overflow: '上下文溢出',
  gate_failure: '闸门失败',
  repeated_pending: '反复转待处理',
  scheduler_no_effect: '调度器处置未生效',
  owner_stuck: '执行体卡住',
  task_stale: '停滞太久',
  task_done: '任务完成',
  slow_run: '慢跑',
  run_failed: 'run 失败',
  task_cancelled: '任务被取消',
  resume_blocked: '续跑被挡下',
  blocked_read: '文件读卡住',
};

/** 类别的中文名；认不出就原样回落库值（不吞掉信息）。 */
export function attentionKindLabel(kind: string): string {
  return ATTENTION_KIND_LABELS[kind] ?? kind;
}

/** 页头提示用的类别摘要：`执行体卡住 2、文件读卡住 1`（按条数降序，同数按名字）。 */
export function attentionKindSummary(byKind: Record<string, number>): string {
  return Object.entries(byKind)
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
    .map(([kind, n]) => `${attentionKindLabel(kind)} ${n}`)
    .join('、');
}

/**
 * 页头那枚读数**该不该在**（决策 307）：有未消费待办才渲染，**0 条时不占窄档空间**。
 *
 * 抽成纯函数的理由就是这条判据要可测：它是「最需要被看见的那一刻」的定义——值守轮
 * **正在排队**（`in_flight = false`）时它照样在，而那一刻「值守台账 · 正在跑」那枚 crumb
 * 根本不出现。模板只做一件事：非空就渲染，「不渲染」这件事在这里判。
 */
export function visibleAttention<T extends { open: number }>(summary: T | null): T | null {
  return summary !== null && summary.open > 0 ? summary : null;
}
