//! 指标口径（§12.4.1，决策 130 / 137）。
//!
//! 口径是契约，不是实现细节：
//! - `total_tokens` = 该任务所有 `kanban_node_runs` 行求和（决策 100：父行 token 不含子行）；
//! - `total_calls` = **调 LLM 的 run 行数**（main + 子代理 + 伪阶段），不含 `agent_type = "system"`（决策 130 ②）；
//! - 逃逸率 = 下游质量事件数 ÷ 上游闸门放行数（决策 137）。

use std::collections::BTreeMap;

use crate::types::{NodeRun, NodeStatus, Stage, TaskStatus};

/// 单个 run 的 token 之和（prompt + completion）。
///
/// cache 读写 token 单独统计——它们是缓存命中信息，不是成本口径的一部分。
pub fn run_tokens(run: &NodeRun) -> u64 {
    run.prompt_tokens as u64 + run.completion_tokens as u64
}

/// 任务累计 token = Σ 所有 run 行（含 system / 子代理 / 伪阶段，决策 100）。
pub fn total_tokens(runs: &[NodeRun]) -> u64 {
    runs.iter().map(run_tokens).sum()
}

/// 任务累计 LLM 调用次数 = 调 LLM 的 run 行数（排除 `agent_type = "system"`，决策 130 ②）。
pub fn total_calls(runs: &[NodeRun]) -> u64 {
    runs.iter().filter(|r| is_llm_run(r)).count() as u64
}

/// 是否调用了 LLM：`main` / 子代理 / `pseudo:*` 都算，`system` 不算。
pub fn is_llm_run(run: &NodeRun) -> bool {
    run.agent_type != "system"
}

/// 任务成功率 = done / (done + failed + cancelled)（§12.4.1）。
pub fn success_rate(statuses: &[TaskStatus]) -> Option<f64> {
    let done = statuses.iter().filter(|s| **s == TaskStatus::Done).count() as f64;
    let settled = statuses
        .iter()
        .filter(|s| {
            matches!(
                s,
                TaskStatus::Done | TaskStatus::Failed | TaskStatus::Cancelled
            )
        })
        .count() as f64;
    if settled == 0.0 {
        None
    } else {
        Some(done / settled)
    }
}

/// 各闸门逃逸率（决策 137）：下游质量事件数 ÷ 上游闸门放行数。
///
/// v1 不做自动归因到具体上游闸门（`escaped_from` 推断列留 v2），只按阶段对比悬殊度。
pub fn escape_rate(downstream_events: u64, upstream_passes: u64) -> Option<f64> {
    if upstream_passes == 0 {
        None
    } else {
        Some(downstream_events as f64 / upstream_passes as f64)
    }
}

/// 阶段级聚合（对应 §12.4.1 的 `GROUP BY stage` 查询）。
#[derive(Debug, Clone, PartialEq)]
pub struct StageMetric {
    pub stage: Stage,
    pub total_runs: u64,
    pub avg_duration_ms: f64,
    /// attempt > 1 的比例。
    pub retry_rate: f64,
}

/// 阶段聚合的纯函数实现（L2 用真 SQL 对照同口径）。
pub fn stage_metrics(runs: &[NodeRun]) -> Vec<StageMetric> {
    let mut buckets: BTreeMap<Stage, Vec<&NodeRun>> = BTreeMap::new();
    for run in runs {
        buckets.entry(run.stage).or_default().push(run);
    }
    buckets
        .into_iter()
        .map(|(stage, rs)| {
            let total = rs.len() as f64;
            let avg = rs.iter().map(|r| r.duration_ms as f64).sum::<f64>() / total;
            let retried = rs.iter().filter(|r| r.attempt > 1).count() as f64;
            StageMetric {
                stage,
                total_runs: rs.len() as u64,
                avg_duration_ms: avg,
                retry_rate: retried / total,
            }
        })
        .collect()
}

/// validate 通过率（首次通过比例，§12.4.1）。
pub fn validate_first_pass_rate(runs: &[NodeRun]) -> Option<f64> {
    let validates: Vec<&NodeRun> = runs
        .iter()
        .filter(|r| r.node == crate::types::Node::ValidateOutput)
        .collect();
    if validates.is_empty() {
        return None;
    }
    let first_pass = validates
        .iter()
        .filter(|r| r.attempt == 1 && r.status == NodeStatus::Success)
        .count() as f64;
    Some(first_pass / validates.len() as f64)
}

/// §12.4.1 的阶段聚合 SQL（L2 直接执行，验证与 [`stage_metrics`] 同口径）。
pub const STAGE_AGGREGATION_SQL: &str = "\
SELECT stage,
       AVG(duration_ms) AS avg_duration,
       AVG(CASE WHEN attempt > 1 THEN 1.0 ELSE 0.0 END) AS retry_rate,
       COUNT(*) AS total_runs
FROM kanban_node_runs
GROUP BY stage
ORDER BY avg_duration DESC";

/// §12.4.1 的逃逸率 SQL（决策 137）。
pub const ESCAPE_RATE_SQL: &str = "\
SELECT t.from_stage AS escaped_from_hint, COUNT(*) AS escape_events
FROM kanban_transitions t
WHERE t.trigger = 'kickback'
GROUP BY t.from_stage";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Node, NodeRun};
    use chrono::Utc;

    fn run(
        stage: Stage,
        node: Node,
        agent_type: &str,
        attempt: u32,
        duration_ms: u64,
        tokens: (u32, u32),
    ) -> NodeRun {
        NodeRun {
            id: 1,
            task_id: "t1".into(),
            cursor_id: "c1".into(),
            stage,
            node,
            attempt,
            agent_type: agent_type.into(),
            parent_run_id: None,
            status: NodeStatus::Success,
            prompt_tokens: tokens.0,
            completion_tokens: tokens.1,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            duration_ms,
            error: None,
            process_group_id: None,
            last_activity_at: None,
            prompt_template_hash: None,
            started_at: Utc::now(),
            finished_at: None,
        }
    }

    #[test]
    fn total_tokens_sums_all_runs_including_system_and_pseudo() {
        let runs = vec![
            run(Stage::Develop, Node::Execute, "main", 1, 10, (100, 50)),
            // 伪阶段也计入（决策 100：父行不含子行，所以求和才是全量）
            run(
                Stage::ArchitectDesign,
                Node::Execute,
                "pseudo:conflict_check",
                1,
                5,
                (20, 10),
            ),
            // system run token 为 0
            run(Stage::Init, Node::Execute, "system", 1, 1, (0, 0)),
        ];
        assert_eq!(total_tokens(&runs), 180);
    }

    #[test]
    fn total_calls_excludes_system_runs() {
        let runs = vec![
            run(Stage::Develop, Node::Execute, "main", 1, 10, (1, 1)),
            run(
                Stage::Develop,
                Node::Execute,
                "code_searcher",
                1,
                10,
                (1, 1),
            ),
            run(
                Stage::ArchitectDesign,
                Node::Execute,
                "pseudo:validator_cross_check",
                1,
                1,
                (1, 1),
            ),
            run(Stage::Init, Node::Execute, "system", 1, 1, (0, 0)),
            run(Stage::Test, Node::ValidateOutput, "system", 1, 1, (0, 0)),
        ];
        assert_eq!(total_calls(&runs), 3);
        assert_eq!(runs.iter().filter(|r| !is_llm_run(r)).count(), 2);
    }

    #[test]
    fn success_rate_only_counts_settled_tasks() {
        let statuses = vec![
            TaskStatus::Done,
            TaskStatus::Done,
            TaskStatus::Failed,
            TaskStatus::Cancelled,
            TaskStatus::Running,
            TaskStatus::Pending,
        ];
        assert_eq!(success_rate(&statuses), Some(0.5));
        assert_eq!(success_rate(&[TaskStatus::Running]), None);
    }

    #[test]
    fn escape_rate_is_none_without_upstream_passes() {
        assert_eq!(escape_rate(3, 0), None);
        assert_eq!(escape_rate(3, 12), Some(0.25));
    }

    #[test]
    fn stage_aggregation_matches_sql_semantics() {
        let runs = vec![
            run(Stage::Develop, Node::Execute, "main", 1, 100, (0, 0)),
            run(Stage::Develop, Node::Execute, "main", 2, 300, (0, 0)),
            run(Stage::Test, Node::Execute, "main", 1, 50, (0, 0)),
        ];
        let metrics = stage_metrics(&runs);
        let dev = metrics.iter().find(|m| m.stage == Stage::Develop).unwrap();
        assert_eq!(dev.total_runs, 2);
        assert_eq!(dev.avg_duration_ms, 200.0);
        assert_eq!(dev.retry_rate, 0.5);
        let test = metrics.iter().find(|m| m.stage == Stage::Test).unwrap();
        assert_eq!(test.retry_rate, 0.0);
    }

    #[test]
    fn validate_first_pass_rate_metric() {
        let runs = vec![
            run(
                Stage::ArchitectDesign,
                Node::ValidateOutput,
                "main",
                1,
                1,
                (0, 0),
            ),
            run(
                Stage::ArchitectDesign,
                Node::ValidateOutput,
                "main",
                2,
                1,
                (0, 0),
            ),
            run(
                Stage::DevelopDesign,
                Node::ValidateOutput,
                "main",
                1,
                1,
                (0, 0),
            ),
        ];
        assert_eq!(validate_first_pass_rate(&runs), Some(2.0 / 3.0));
        assert_eq!(validate_first_pass_rate(&[]), None);
    }

    #[test]
    fn sql_constants_target_the_right_tables() {
        assert!(STAGE_AGGREGATION_SQL.contains("kanban_node_runs"));
        assert!(STAGE_AGGREGATION_SQL.contains("GROUP BY stage"));
        assert!(STAGE_AGGREGATION_SQL.contains("attempt > 1"));
        assert!(ESCAPE_RATE_SQL.contains("kanban_transitions"));
        assert!(ESCAPE_RATE_SQL.contains("'kickback'"));
    }
}
