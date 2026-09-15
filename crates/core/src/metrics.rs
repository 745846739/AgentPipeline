//! 指标口径（§12.4.1，决策 130 / 137）。
//!
//! 口径是契约，不是实现细节：
//! - **范围 = 流水线执行 + 伪阶段，不含面向人的对话**（决策 182）。项目级伪阶段的运行行
//!   （`pseudo:project_analysis`）**计入**——它同样调了 LLM，是流水线自己发起的一次调用；
//!   值班长（对讲台）的对话**不计入**，它既不挂任务也不挂阶段，token 由
//!   `kanban_foreman_messages` 承载并在对讲台自报「本次会话 N tok」。这条边界必须写死在
//!   这里：两处都是「不挂任务的 LLM 调用」，只看实现很难分辨哪一个该进指标。
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

/// 任务累计 token = Σ 所有 run 行（含 system / 子代理 / 伪阶段，决策 100），
/// **但排除被续接的历史 run**（决策 180，票 13）。
///
/// 续接（`resume_continuation`）会把上一 attempt 的对话重新发一遍，于是历史 run 报过的输入
/// token 在新 run 里再报一次。上面那条盲求和的规则在续接出现之前是对的；有了续接就要把
/// 「被后继 run 指为续接来源」的那些排除掉，否则同一个 token 算两遍。
///
/// 排除的是**历史那一侧**而不是新 run：新 run 的 `prompt_tokens` 才是当前真实的上下文成本。
pub fn total_tokens(runs: &[NodeRun]) -> u64 {
    let superseded: std::collections::HashSet<i64> = runs
        .iter()
        .filter_map(|r| r.continued_from_run_id)
        .collect();
    runs.iter()
        .filter(|r| !superseded.contains(&r.id))
        .map(run_tokens)
        .sum()
}

/// 任务累计 LLM 调用次数 = 调 LLM 的 run 行数（排除 `agent_type = "system"`，决策 130 ②）。
///
/// 项目级伪阶段 run（`pseudo:project_analysis`，无任务以 `project_id` 归属，票 10）
/// 一并计入——它同样调了 LLM；`system` run 仍不计。
pub fn total_calls(runs: &[NodeRun]) -> u64 {
    runs.iter().filter(|r| is_llm_run(r)).count() as u64
}

/// 是否调用了 LLM：`main` / 子代理 / `pseudo:*` 都算，`system` 不算。
pub fn is_llm_run(run: &NodeRun) -> bool {
    run.agent_type != "system"
}

/// 该 run 是否代表**节点自身**的一次尝试（决策 172，票 14）。
///
/// `attempt` 与重试率只统计这类 run。伪阶段（`pseudo:*`）与子代理（`subagent`）是挂在
/// 父节点坐标（同 `stage` / `node`）下的**辅助调用**，计入就会虚增：伪阶段复用父节点的
/// stage/node，一次没重试的节点会被它顶成 `attempt > 1`，于是重试率把没重试的节点算成
/// 重试、`validate_first_pass_rate` 把复判 run 算成一次首过。
///
/// 采**白名单**而非黑名单：将来新增的辅助 agent 类型默认不计入节点尝试（宁少勿虚增）。
/// `system`（纯代码节点）是节点自身的执行，必须计入——`init` / `merge` 等节点会重复执行，
/// 其 `attempt` 语义与 `main` 无异。
pub fn is_node_owning_run(agent_type: &str) -> bool {
    agent_type == "main" || agent_type == "system"
}

/// [`is_node_owning_run`] 的 SQL 形式（口径只此一处，避免 Rust 与 SQL 漂移，决策 137）。
pub const NODE_OWNING_AGENT_TYPES_SQL: &str = "'main', 'system'";

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
    /// `attempt > 1` 的比例。
    ///
    /// 分母是**节点自身的 run**（[`is_node_owning_run`]）——伪阶段 / 子代理不是重试，
    /// 计入会把没重试的节点算成重试（决策 172，票 14）。`total_runs` / 耗时仍含全部 run：
    /// 伪阶段是真实发生的 LLM 调用，其耗时属于该阶段的实际开销。
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
            let owning: Vec<&&NodeRun> = rs
                .iter()
                .filter(|r| is_node_owning_run(&r.agent_type))
                .collect();
            let retried = owning.iter().filter(|r| r.attempt > 1).count() as f64;
            StageMetric {
                stage,
                total_runs: rs.len() as u64,
                avg_duration_ms: avg,
                retry_rate: if owning.is_empty() {
                    0.0
                } else {
                    retried / owning.len() as f64
                },
            }
        })
        .collect()
}

/// validate 通过率（首次通过比例，§12.4.1）。
///
/// 同样只统计节点自身的 run（决策 172，票 14）：异族复判会以 `pseudo:*` 在
/// `validate_output` 坐标下落一行，计入分母会把它当成一次「未首过」。
pub fn validate_first_pass_rate(runs: &[NodeRun]) -> Option<f64> {
    let validates: Vec<&NodeRun> = runs
        .iter()
        .filter(|r| r.node == crate::types::Node::ValidateOutput)
        .filter(|r| is_node_owning_run(&r.agent_type))
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
///
/// `retry_rate` 只在节点自身的 run 上求平均——`AVG` 忽略 NULL，故非节点 run 走 `ELSE NULL`
/// 即被排除；`COALESCE` 兜住「该阶段只有伪阶段 run」这一退化情形，与
/// [`stage_metrics`] 返回 0.0 对齐。`COUNT(*)` / `AVG(duration_ms)` 仍覆盖全部 run。
///
/// 由 [`NODE_OWNING_AGENT_TYPES_SQL`] 拼出白名单（函数而非常量：口径只此一处，
/// 避免 Rust 判定与 SQL 白名单各写一遍而漂移，决策 137）。
pub fn stage_aggregation_sql() -> String {
    format!(
        "SELECT stage, \
         AVG(duration_ms) AS avg_duration, \
         COALESCE(AVG(CASE WHEN agent_type IN ({types}) THEN \
             (CASE WHEN attempt > 1 THEN 1.0 ELSE 0.0 END) END), 0.0) AS retry_rate, \
         COUNT(*) AS total_runs \
         FROM kanban_node_runs \
         GROUP BY stage \
         ORDER BY avg_duration DESC",
        types = NODE_OWNING_AGENT_TYPES_SQL
    )
}

/// §12.4.1 的逃逸率 SQL（决策 137）。
pub const ESCAPE_RATE_SQL: &str = "\
SELECT t.from_stage AS escaped_from_hint, COUNT(*) AS escape_events
FROM kanban_transitions t
WHERE t.trigger = 'kickback'
GROUP BY t.from_stage";

/// 自适应超时的分位数采样口径（决策 66 / 票 17）。
///
/// **只取成功运行**：failed / timeout 样本会污染分位数（挂死节点自我抬高阈值的
/// 反馈回路，附录 B.4 已记录）。窗口内样本不足时返回 `None`——冷启动无数据
/// 就不展示、不告警，而不是拿一两个样本硬算。
pub const ADAPTIVE_MIN_SAMPLES: usize = 5;
/// 采样窗口：每个 `(stage, node)` 只看最近 N 次成功运行。
pub const ADAPTIVE_WINDOW: usize = 20;

/// 某 `(stage, node)` 的历史耗时分位数（毫秒）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DurationPercentiles {
    pub p50_ms: u64,
    pub p90_ms: u64,
    /// 参与计算的样本数（窗口内成功运行数）。
    pub samples: usize,
}

/// 计算某 `(stage, node)` 的 P50 / P90 耗时（决策 66 / 票 17）。
///
/// 采样口径：
/// - **仅成功运行**（`NodeStatus::Success`）；failed / timeout 排除；
/// - 窗口取该节点最近 [`ADAPTIVE_WINDOW`] 次成功运行（`runs` 按时间序传入）；
/// - 样本数 < [`ADAPTIVE_MIN_SAMPLES`] → `None`（冷启动不展示 / 不告警）。
///
/// **用途仅限进度展示与告警**：返回值绝不参与任何强制超时判定（决策 66 边界）。
pub fn duration_percentiles(
    runs: &[NodeRun],
    stage: Stage,
    node: crate::types::Node,
) -> Option<DurationPercentiles> {
    let mut durs: Vec<u64> = runs
        .iter()
        .filter(|r| r.stage == stage && r.node == node && r.status == NodeStatus::Success)
        .map(|r| r.duration_ms)
        .collect();
    if durs.len() > ADAPTIVE_WINDOW {
        durs = durs.split_off(durs.len() - ADAPTIVE_WINDOW);
    }
    if durs.len() < ADAPTIVE_MIN_SAMPLES {
        return None;
    }
    durs.sort_unstable();
    Some(DurationPercentiles {
        p50_ms: percentile(&durs, 0.50),
        p90_ms: percentile(&durs, 0.90),
        samples: durs.len(),
    })
}

/// 最近邻分位数（`sorted` 必须已升序；`q ∈ [0,1]`）。
fn percentile(sorted: &[u64], q: f64) -> u64 {
    debug_assert!(!sorted.is_empty());
    let idx = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[idx]
}

/// 是否应发「运行时长超过 3×P90」的告警（决策 66 的告警用途，票 17）。
///
/// **只读分位数、不写任何超时配置**：调用方在告警路径用，绝不在超时判定里用。
pub fn should_alert_slow(elapsed_ms: u64, percentiles: DurationPercentiles) -> bool {
    elapsed_ms > percentiles.p90_ms.saturating_mul(3)
}

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
            task_id: Some("t1".into()),
            cursor_id: Some("c1".into()),
            project_id: None,
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
            continued_from_run_id: None,
            started_at: Utc::now(),
            finished_at: None,
        }
    }

    /// 项目级伪阶段 run：无任务 / 游标，以 `project_id` 归属（票 10 / 决策 100）。
    fn project_run(agent_type: &str, tokens: (u32, u32)) -> NodeRun {
        NodeRun {
            id: 2,
            task_id: None,
            cursor_id: None,
            project_id: Some("p1".into()),
            stage: Stage::Init,
            node: Node::Execute,
            attempt: 1,
            agent_type: agent_type.into(),
            parent_run_id: None,
            status: NodeStatus::Success,
            prompt_tokens: tokens.0,
            completion_tokens: tokens.1,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            duration_ms: 1,
            error: None,
            process_group_id: None,
            last_activity_at: None,
            prompt_template_hash: None,
            continued_from_run_id: None,
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
    fn total_calls_counts_project_level_pseudo_run() {
        // 票 10 / 决策 130 ②：项目级 project_analysis 伪阶段（agent_type = pseudo:*，
        // 非 system）也调了 LLM，必须计入 total_calls。
        let runs = vec![
            run(Stage::Develop, Node::Execute, "main", 1, 10, (1, 1)),
            project_run("pseudo:project_analysis", (10, 5)),
        ];
        assert_eq!(total_calls(&runs), 2, "项目级伪阶段应计入 total_calls");
        assert_eq!(total_tokens(&runs), 17, "项目级伪阶段 token 也计入总和");
        assert!(is_llm_run(&project_run("pseudo:project_analysis", (0, 0))));
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

    /// 决策 172 / 票 14：伪阶段不算重试。
    ///
    /// 修正前 `retry_rate` 的分母含伪阶段 run，且伪阶段复用父节点的 stage/node——
    /// 一次没重试的节点会被顶成「该阶段 1/2 的 run attempt > 1」。
    #[test]
    fn pseudo_stage_runs_do_not_inflate_retry_rate() {
        let runs = vec![
            // 一次成功的 architect 节点执行（attempt = 1）
            run(Stage::ArchitectDesign, Node::Execute, "main", 1, 10, (1, 1)),
            // 挂同一坐标的伪阶段 run，attempt 被旧口径数成 2
            run(
                Stage::ArchitectDesign,
                Node::Execute,
                "pseudo:conflict_check",
                2,
                1,
                (1, 1),
            ),
        ];
        let agg = stage_metrics(&runs);
        let arch = agg
            .iter()
            .find(|m| m.stage == Stage::ArchitectDesign)
            .unwrap();
        assert_eq!(
            arch.retry_rate, 0.0,
            "伪阶段不是重试，retry_rate 必须按节点自身 run 计算"
        );
        assert_eq!(arch.total_runs, 2, "total_runs 仍含全部 run（含伪阶段）");
    }

    /// 子代理同样不计入重试率（决策 172，票 14 / 08）。
    #[test]
    fn subagent_runs_do_not_inflate_retry_rate() {
        let runs = vec![
            run(Stage::Develop, Node::Execute, "main", 1, 10, (1, 1)),
            run(Stage::Develop, Node::Execute, "subagent", 2, 5, (1, 1)),
        ];
        let agg = stage_metrics(&runs);
        let dev = agg.iter().find(|m| m.stage == Stage::Develop).unwrap();
        assert_eq!(dev.retry_rate, 0.0);
    }

    #[test]
    fn is_node_owning_run_whitelists_main_and_system_only() {
        assert!(is_node_owning_run("main"));
        assert!(is_node_owning_run("system"), "纯代码节点是节点自身的执行");
        assert!(!is_node_owning_run("subagent"));
        assert!(!is_node_owning_run("pseudo:conflict_check"));
        assert!(
            !is_node_owning_run("pseudo:validator_cross_check"),
            "复判不是节点尝试"
        );
        // 未知类型默认不计入（宁少勿虚增）
        assert!(!is_node_owning_run("future_helper"));
    }

    /// 只有伪阶段 run 的阶段：`retry_rate` 不因分母为空而 NaN/panic（SQL 侧 COALESCE 对齐）。
    #[test]
    fn stage_with_only_pseudo_runs_has_zero_retry_rate() {
        let runs = vec![run(
            Stage::ArchitectDesign,
            Node::Execute,
            "pseudo:conflict_check",
            1,
            5,
            (1, 1),
        )];
        let agg = stage_metrics(&runs);
        assert_eq!(agg[0].retry_rate, 0.0);
        assert_eq!(agg[0].total_runs, 1);
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

    /// 决策 134 / 172：异族复判的 run 挂在 `validate_output` 坐标下，不得计入首过率分母。
    #[test]
    fn cross_check_run_is_not_a_validate_attempt() {
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
                "pseudo:validator_cross_check",
                1,
                1,
                (0, 0),
            ),
        ];
        assert_eq!(
            validate_first_pass_rate(&runs),
            Some(1.0),
            "复判 run 不是一次 validate 尝试"
        );
    }

    #[test]
    fn sql_constants_target_the_right_tables() {
        let sql = stage_aggregation_sql();
        assert!(sql.contains("kanban_node_runs"));
        assert!(sql.contains("GROUP BY stage"));
        assert!(sql.contains("attempt > 1"));
        // 决策 172 / 票 14：白名单由 NODE_OWNING_AGENT_TYPES_SQL 拼出（口径只此一处）
        assert!(sql.contains(NODE_OWNING_AGENT_TYPES_SQL), "{sql}");
        assert!(ESCAPE_RATE_SQL.contains("kanban_transitions"));
        assert!(ESCAPE_RATE_SQL.contains("'kickback'"));
    }

    // ── 票 17：自适应超时分位数（决策 66，告警用途）──

    #[test]
    fn percentiles_use_successful_runs_only() {
        // 仅成功运行参与：failed / timeout 样本被排除，避免污染分位数
        let mut runs: Vec<NodeRun> = (0..10)
            .map(|i| {
                run(
                    Stage::Develop,
                    Node::Execute,
                    "main",
                    1,
                    1000 + i * 100,
                    (0, 0),
                )
            })
            .collect();
        // 一个「挂死」的超长失败样本：若被计入会把 P90 抬到 999999
        let mut hung = run(Stage::Develop, Node::Execute, "main", 1, 999_999, (0, 0));
        hung.status = NodeStatus::Failed;
        runs.push(hung);

        let p = duration_percentiles(&runs, Stage::Develop, Node::Execute).expect("样本足够");
        assert_eq!(p.samples, 10, "只数成功样本");
        assert!(p.p90_ms < 10_000, "失败样本不得抬高 P90：{}", p.p90_ms);
    }

    #[test]
    fn percentiles_require_minimum_samples() {
        // 冷启动样本不足 → None（不展示、不告警），而不是拿一两个样本硬算
        let few: Vec<NodeRun> = (0..ADAPTIVE_MIN_SAMPLES - 1)
            .map(|i| {
                run(
                    Stage::Develop,
                    Node::Execute,
                    "main",
                    1,
                    1000 + i as u64,
                    (0, 0),
                )
            })
            .collect();
        assert_eq!(
            duration_percentiles(&few, Stage::Develop, Node::Execute),
            None
        );
        // 恰好达到下限 → Some
        let enough: Vec<NodeRun> = (0..ADAPTIVE_MIN_SAMPLES)
            .map(|i| {
                run(
                    Stage::Develop,
                    Node::Execute,
                    "main",
                    1,
                    1000 + i as u64,
                    (0, 0),
                )
            })
            .collect();
        let p = duration_percentiles(&enough, Stage::Develop, Node::Execute).unwrap();
        assert_eq!(p.samples, ADAPTIVE_MIN_SAMPLES);
        assert!(p.p50_ms <= p.p90_ms);
    }

    #[test]
    fn percentiles_are_per_stage_and_node() {
        // 分位数按 (stage, node) 分组，不跨节点混算
        let mut runs: Vec<NodeRun> = (0..6)
            .map(|i| run(Stage::Develop, Node::Execute, "main", 1, 100 + i, (0, 0)))
            .collect();
        runs.extend(
            (0..6).map(|i| run(Stage::Review, Node::Execute, "main", 1, 90_000 + i, (0, 0))),
        );
        let dev = duration_percentiles(&runs, Stage::Develop, Node::Execute).unwrap();
        let rev = duration_percentiles(&runs, Stage::Review, Node::Execute).unwrap();
        assert!(dev.p90_ms < 1_000, "develop 分位数不被 review 拉高");
        assert!(rev.p90_ms > 80_000);
    }

    #[test]
    fn slow_alert_thresholds_at_three_times_p90() {
        // 告警判定：超过 3×P90（决策 66 的告警口径）
        let p = DurationPercentiles {
            p50_ms: 1_000,
            p90_ms: 3_000,
            samples: 10,
        };
        assert!(!should_alert_slow(9_000, p), "恰好 3×P90 不告警");
        assert!(should_alert_slow(9_001, p), "超过 3×P90 告警");
    }

    #[test]
    fn adaptive_values_are_never_used_as_hard_timeout() {
        // 决策 66 边界：自适应分位数只用于展示与告警，**不参与强制超时判定**。
        // 这里以类型/接口层面钉住：分位数 API 不返回任何可当阈值的东西，
        // 超时判定函数 `is_timed_out` 也不接受分位数参数。
        let runs: Vec<NodeRun> = (0..ADAPTIVE_WINDOW + 5)
            .map(|i| {
                run(
                    Stage::Develop,
                    Node::Execute,
                    "main",
                    1,
                    1_000 + i as u64,
                    (0, 0),
                )
            })
            .collect();
        let p = duration_percentiles(&runs, Stage::Develop, Node::Execute).unwrap();
        // 窗口上限生效
        assert_eq!(p.samples, ADAPTIVE_WINDOW);
        // should_alert_slow 只读分位数；签名里没有任何可变阈值/配置输出
        assert!(should_alert_slow(u64::MAX, p));
    }
}
