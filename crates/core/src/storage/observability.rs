//! 可观测性：runs / 阶段产出 / 流转 / 会话 / 命令（§12.4）。

use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use sqlx::FromRow;

use super::{decode_lossy, decode_node, decode_stage, parse_ts, ts, Store};
use crate::agent::tools::{CommandFinish, CommandRecorder, CommandStart};
use crate::metrics;
use crate::types::{
    CommandSource, MergeResult, Node, NodeCommand, NodeConversation, NodeRun, NodeStatus, Stage,
    StageOutput, Transition, TransitionTrigger,
};
use crate::{Error, Result};

/// 新建 run 的输入（决策 63 / 99 / 114）。
#[derive(Debug, Clone)]
pub struct NewRun {
    pub task_id: String,
    pub cursor_id: String,
    pub stage: Stage,
    pub node: Node,
    pub attempt: u32,
    /// `main` / 子代理名 / `pseudo:*` / `system`。
    pub agent_type: String,
    pub parent_run_id: Option<i64>,
    pub prompt_template_hash: Option<String>,
    pub process_group_id: Option<i32>,
}

/// 新建**项目级** run 的输入（票 10 / 决策 100）。
///
/// `project_analysis` 没有任务、没有游标，归属改以 `project_id` 表达；
/// `task_id` / `cursor_id` 落 NULL（schema 由迁移 0004 放开为可空）。
#[derive(Debug, Clone)]
pub struct NewProjectRun {
    pub project_id: String,
    pub stage: Stage,
    pub node: Node,
    pub attempt: u32,
    /// 伪阶段名，如 `pseudo:project_analysis`（决策 130 ②：非 `system` 计入 `total_calls`）。
    pub agent_type: String,
}

/// run 收尾信息。
#[derive(Debug, Clone, Default)]
pub struct RunOutcome {
    pub status: Option<NodeStatus>,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub cache_read_tokens: u32,
    pub cache_write_tokens: u32,
    pub duration_ms: u64,
    pub error: Option<String>,
    pub process_group_id: Option<i32>,
}

#[derive(Debug, FromRow)]
struct RunRow {
    id: i64,
    task_id: Option<String>,
    cursor_id: Option<String>,
    project_id: Option<String>,
    stage: String,
    node: String,
    attempt: i64,
    agent_type: String,
    parent_run_id: Option<i64>,
    status: String,
    prompt_tokens: i64,
    completion_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    duration_ms: i64,
    error: Option<String>,
    process_group_id: Option<i64>,
    last_activity_at: Option<String>,
    prompt_template_hash: Option<String>,
    continued_from_run_id: Option<i64>,
    started_at: String,
    finished_at: Option<String>,
}

impl RunRow {
    fn into_run(self) -> Result<NodeRun> {
        Ok(NodeRun {
            id: self.id,
            task_id: self.task_id,
            cursor_id: self.cursor_id,
            project_id: self.project_id,
            stage: decode_stage(&self.stage)?,
            node: decode_node(&self.node)?,
            attempt: self.attempt as u32,
            agent_type: self.agent_type,
            parent_run_id: self.parent_run_id,
            // run status 驱动 active_runs / 超时判定 → 执行语义，损坏即报错
            status: self.status.parse()?,
            prompt_tokens: self.prompt_tokens as u32,
            completion_tokens: self.completion_tokens as u32,
            cache_read_tokens: self.cache_read_tokens as u32,
            cache_write_tokens: self.cache_write_tokens as u32,
            duration_ms: self.duration_ms as u64,
            error: self.error,
            process_group_id: self.process_group_id.map(|v| v as i32),
            last_activity_at: self.last_activity_at.map(|s| parse_ts(&s)).transpose()?,
            prompt_template_hash: self.prompt_template_hash,
            continued_from_run_id: self.continued_from_run_id,
            started_at: parse_ts(&self.started_at)?,
            finished_at: self.finished_at.map(|s| parse_ts(&s)).transpose()?,
        })
    }
}

const RUN_COLUMNS: &str =
    "id, task_id, cursor_id, project_id, stage, node, attempt, agent_type, parent_run_id, \
     status, prompt_tokens, completion_tokens, cache_read_tokens, cache_write_tokens, duration_ms, \
     error, process_group_id, last_activity_at, prompt_template_hash, continued_from_run_id, \
     started_at, finished_at";

impl Store {
    /// 落一行 run（所有节点都落，含 `agent_type = "system"`，决策 99 / 114）。
    pub async fn insert_run(&self, new_run: &NewRun) -> Result<i64> {
        let now = self.now();
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO kanban_node_runs
             (task_id, cursor_id, project_id, stage, node, attempt, agent_type, parent_run_id, status,
              prompt_tokens, completion_tokens, cache_read_tokens, cache_write_tokens,
              duration_ms, error, process_group_id, last_activity_at, prompt_template_hash,
              started_at, finished_at)
             VALUES (?, ?, NULL, ?, ?, ?, ?, ?, 'running', 0, 0, 0, 0, 0, NULL, ?, ?, ?, ?, NULL)
             RETURNING id",
        )
        .bind(&new_run.task_id)
        .bind(&new_run.cursor_id)
        .bind(new_run.stage.as_str())
        .bind(new_run.node.as_str())
        .bind(new_run.attempt as i64)
        .bind(&new_run.agent_type)
        .bind(new_run.parent_run_id)
        .bind(new_run.process_group_id)
        .bind(ts(now))
        .bind(&new_run.prompt_template_hash)
        .bind(ts(now))
        .fetch_one(self.pool())
        .await?;
        Ok(id)
    }

    /// 落一行**项目级**伪阶段 run（票 10 / 决策 100）。
    ///
    /// `task_id` / `cursor_id` 为 NULL、`project_id` 归属项目；
    /// `CHECK` 约束保证归属二选一。用户可在项目分析详情里查看该 run。
    pub async fn insert_project_run(&self, new_run: &NewProjectRun) -> Result<i64> {
        let now = self.now();
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO kanban_node_runs
             (task_id, cursor_id, project_id, stage, node, attempt, agent_type, parent_run_id, status,
              prompt_tokens, completion_tokens, cache_read_tokens, cache_write_tokens,
              duration_ms, error, process_group_id, last_activity_at, prompt_template_hash,
              started_at, finished_at)
             VALUES (NULL, NULL, ?, ?, ?, ?, ?, NULL, 'running', 0, 0, 0, 0, 0, NULL, NULL, ?, NULL, ?, NULL)
             RETURNING id",
        )
        .bind(&new_run.project_id)
        .bind(new_run.stage.as_str())
        .bind(new_run.node.as_str())
        .bind(new_run.attempt as i64)
        .bind(&new_run.agent_type)
        .bind(ts(now))
        .bind(ts(now))
        .fetch_one(self.pool())
        .await?;
        Ok(id)
    }

    pub async fn finish_run(&self, run_id: i64, outcome: &RunOutcome) -> Result<()> {
        let now = self.now();
        sqlx::query(
            "UPDATE kanban_node_runs
             SET status = COALESCE(?, status), prompt_tokens = ?, completion_tokens = ?,
                 cache_read_tokens = ?, cache_write_tokens = ?, duration_ms = ?, error = ?,
                 process_group_id = COALESCE(?, process_group_id), finished_at = ?,
                 last_activity_at = ?
             WHERE id = ?",
        )
        .bind(outcome.status.map(|s| s.as_str()))
        .bind(outcome.prompt_tokens as i64)
        .bind(outcome.completion_tokens as i64)
        .bind(outcome.cache_read_tokens as i64)
        .bind(outcome.cache_write_tokens as i64)
        .bind(outcome.duration_ms as i64)
        .bind(&outcome.error)
        .bind(outcome.process_group_id)
        .bind(ts(now))
        .bind(ts(now))
        .bind(run_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// 心跳刷新（决策 64 / 100：超时时钟取自 started_at，空闲判定取自 last_activity_at）。
    pub async fn touch_run_heartbeat(&self, run_id: i64) -> Result<()> {
        sqlx::query("UPDATE kanban_node_runs SET last_activity_at = ? WHERE id = ?")
            .bind(ts(self.now()))
            .bind(run_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// 回填 run 的真实进程组 id（决策 66 / 票 17：超时时 scheduler 杀整个进程组）。
    pub async fn set_run_process_group(&self, run_id: i64, pgid: i32) -> Result<()> {
        sqlx::query("UPDATE kanban_node_runs SET process_group_id = ? WHERE id = ?")
            .bind(pgid)
            .bind(run_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// 补记 prompt 版本标注（决策 137：最终组装 system prompt 的 SHA-256 前 16 位）。
    pub async fn set_run_template_hash(&self, run_id: i64, hash: &str) -> Result<()> {
        sqlx::query("UPDATE kanban_node_runs SET prompt_template_hash = ? WHERE id = ?")
            .bind(hash)
            .bind(run_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// 正在跑的 run（超时检测的唯一输入）。
    pub async fn active_runs(&self) -> Result<Vec<NodeRun>> {
        let sql = format!(
            "SELECT {RUN_COLUMNS} FROM kanban_node_runs WHERE status = 'running' ORDER BY started_at"
        );
        let rows: Vec<RunRow> = sqlx::query_as(&sql).fetch_all(self.pool()).await?;
        rows.into_iter().map(RunRow::into_run).collect()
    }

    pub async fn list_runs(&self, task_id: &str) -> Result<Vec<NodeRun>> {
        let sql =
            format!("SELECT {RUN_COLUMNS} FROM kanban_node_runs WHERE task_id = ? ORDER BY id");
        let rows: Vec<RunRow> = sqlx::query_as(&sql)
            .bind(task_id)
            .fetch_all(self.pool())
            .await?;
        rows.into_iter().map(RunRow::into_run).collect()
    }

    /// 某项目的项目级伪阶段 run（`project_analysis`，票 10 / 决策 100）。
    pub async fn list_project_runs(&self, project_id: &str) -> Result<Vec<NodeRun>> {
        let sql =
            format!("SELECT {RUN_COLUMNS} FROM kanban_node_runs WHERE project_id = ? ORDER BY id");
        let rows: Vec<RunRow> = sqlx::query_as(&sql)
            .bind(project_id)
            .fetch_all(self.pool())
            .await?;
        rows.into_iter().map(RunRow::into_run).collect()
    }

    pub async fn list_runs_at(
        &self,
        task_id: &str,
        stage: Stage,
        node: Node,
    ) -> Result<Vec<NodeRun>> {
        let sql = format!(
            "SELECT {RUN_COLUMNS} FROM kanban_node_runs
             WHERE task_id = ? AND stage = ? AND node = ? ORDER BY id"
        );
        let rows: Vec<RunRow> = sqlx::query_as(&sql)
            .bind(task_id)
            .bind(stage.as_str())
            .bind(node.as_str())
            .fetch_all(self.pool())
            .await?;
        rows.into_iter().map(RunRow::into_run).collect()
    }

    /// 某 `(task, stage, node)` 上**节点自身**的 run 行数（决策 172，票 14）。
    ///
    /// `attempt` 的唯一取数口：伪阶段与子代理的 run 复用父节点的 stage/node，若一并计入，
    /// 一次没重试的节点会被顶成 `attempt > 1`（虚增重试率、错位会话行）。
    /// 白名单见 [`crate::metrics::is_node_owning_run`]。
    pub async fn count_node_owning_runs(
        &self,
        task_id: &str,
        stage: Stage,
        node: Node,
    ) -> Result<u32> {
        let count: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM kanban_node_runs
             WHERE task_id = ? AND stage = ? AND node = ? AND agent_type IN ({})",
            metrics::NODE_OWNING_AGENT_TYPES_SQL
        ))
        .bind(task_id)
        .bind(stage.as_str())
        .bind(node.as_str())
        .fetch_one(self.pool())
        .await?;
        Ok(count as u32)
    }

    /// 全量 run 行：全局指标用。口径由 [`crate::metrics`] 的纯函数定义，
    /// 这里只取数、不在 SQL 里重算，避免「口径契约」在 SQL 与 Rust 之间漂移（决策 137）。
    pub async fn all_runs(&self) -> Result<Vec<NodeRun>> {
        let sql = format!("SELECT {RUN_COLUMNS} FROM kanban_node_runs");
        let rows: Vec<RunRow> = sqlx::query_as(&sql).fetch_all(self.pool()).await?;
        rows.into_iter().map(RunRow::into_run).collect()
    }

    /// 指标：阶段聚合（纯 SQL，口径与 [`crate::metrics::stage_metrics`] 一致）。
    pub async fn stage_aggregation(&self) -> Result<Vec<(String, f64, f64, i64)>> {
        let rows: Vec<(String, f64, f64, i64)> = sqlx::query_as(&metrics::stage_aggregation_sql())
            .fetch_all(self.pool())
            .await?;
        Ok(rows)
    }

    /// 指标：逃逸率（决策 137）。
    pub async fn escape_events_by_stage(&self) -> Result<Vec<(Option<String>, i64)>> {
        let rows: Vec<(Option<String>, i64)> = sqlx::query_as(metrics::ESCAPE_RATE_SQL)
            .fetch_all(self.pool())
            .await?;
        Ok(rows)
    }

    // ─────────────────────────── 阶段产出（决策 30 / 108）───────────────────────────

    pub async fn upsert_stage_output(
        &self,
        task_id: &str,
        stage: Stage,
        output_type: &str,
        file_path: &str,
        metadata: Option<&serde_json::Value>,
    ) -> Result<()> {
        let now = self.now();
        sqlx::query(
            "INSERT INTO kanban_stage_outputs
             (task_id, stage, output_type, file_path, metadata_json, stale, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, 0, ?, ?)
             ON CONFLICT(task_id, stage, output_type) DO UPDATE SET
                 file_path = excluded.file_path,
                 metadata_json = excluded.metadata_json,
                 stale = 0,
                 updated_at = excluded.updated_at",
        )
        .bind(task_id)
        .bind(stage.as_str())
        .bind(output_type)
        .bind(file_path)
        .bind(metadata.map(|m| m.to_string()))
        .bind(ts(now))
        .bind(ts(now))
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn get_stage_output(
        &self,
        task_id: &str,
        stage: Stage,
        output_type: &str,
    ) -> Result<Option<StageOutput>> {
        #[derive(FromRow)]
        struct Row {
            id: i64,
            task_id: String,
            stage: String,
            output_type: String,
            file_path: String,
            metadata_json: Option<String>,
            stale: i64,
            created_at: String,
            updated_at: String,
        }
        let row: Option<Row> = sqlx::query_as(
            "SELECT id, task_id, stage, output_type, file_path, metadata_json, stale, created_at, updated_at
             FROM kanban_stage_outputs WHERE task_id = ? AND stage = ? AND output_type = ?",
        )
        .bind(task_id)
        .bind(stage.as_str())
        .bind(output_type)
        .fetch_optional(self.pool())
        .await?;
        Ok(match row {
            Some(r) => Some(StageOutput {
                id: r.id,
                task_id: r.task_id,
                stage: decode_stage(&r.stage)?,
                output_type: r.output_type,
                file_path: r.file_path,
                metadata_json: r
                    .metadata_json
                    .map(|s| serde_json::from_str(&s))
                    .transpose()?,
                stale: r.stale != 0,
                created_at: parse_ts(&r.created_at)?,
                updated_at: parse_ts(&r.updated_at)?,
            }),
            None => None,
        })
    }

    pub async fn stage_output_metadata(
        &self,
        task_id: &str,
        stage: Stage,
        output_type: &str,
    ) -> Result<Option<serde_json::Value>> {
        Ok(self
            .get_stage_output(task_id, stage, output_type)
            .await?
            .and_then(|o| o.metadata_json))
    }

    pub async fn list_stage_outputs(&self, task_id: &str) -> Result<Vec<StageOutput>> {
        #[derive(FromRow)]
        struct Row {
            id: i64,
            task_id: String,
            stage: String,
            output_type: String,
            file_path: String,
            metadata_json: Option<String>,
            stale: i64,
            created_at: String,
            updated_at: String,
        }
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT id, task_id, stage, output_type, file_path, metadata_json, stale, created_at, updated_at
             FROM kanban_stage_outputs WHERE task_id = ? ORDER BY id",
        )
        .bind(task_id)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(StageOutput {
                    id: r.id,
                    task_id: r.task_id,
                    stage: decode_stage(&r.stage)?,
                    output_type: r.output_type,
                    file_path: r.file_path,
                    metadata_json: r
                        .metadata_json
                        .map(|s| serde_json::from_str(&s))
                        .transpose()?,
                    stale: r.stale != 0,
                    created_at: parse_ts(&r.created_at)?,
                    updated_at: parse_ts(&r.updated_at)?,
                })
            })
            .collect()
    }

    /// 读取 merge metadata。行缺失返回 `None`（= 闸门未跑）；**行损坏报错**，
    /// 不做静默兜底（执行语义字段 fail-fast，Q6 分类）。
    pub async fn merge_metadata(&self, task_id: &str) -> Result<Option<MergeResult>> {
        Ok(self
            .stage_output_metadata(task_id, Stage::Merge, crate::types::MERGE_OUTPUT_TYPE)
            .await?
            .map(serde_json::from_value::<MergeResult>)
            .transpose()?)
    }

    /// merge metadata 的 upsert：**显式跳过 `gate_failures`**（决策 108）——
    /// 该计数跨阶段跳转不重置，否则 merge ↔ test 循环不终止。
    pub async fn upsert_merge_result(
        &self,
        task_id: &str,
        file_path: &str,
        incoming: &MergeResult,
    ) -> Result<MergeResult> {
        let previous = self.merge_metadata(task_id).await?;
        let mut merged = incoming.clone();
        if let Some(prev) = &previous {
            merged.gate_failures = prev.gate_failures;
        }
        self.upsert_stage_output(
            task_id,
            Stage::Merge,
            crate::types::MERGE_OUTPUT_TYPE,
            file_path,
            Some(&serde_json::to_value(&merged)?),
        )
        .await?;
        Ok(merged)
    }

    /// 闸门失败计数 +1（lint 与测试统一累加），返回累计值。
    ///
    /// 阈值判定在 [`crate::pipeline::routes::route_merge`]（validate_retry_max），
    /// 这里只负责累加。行缺失说明阶段 A 还没跑到闸门——那是调用方的 bug，报错。
    pub async fn increment_gate_failures(&self, task_id: &str) -> Result<u32> {
        let mut stored = self.merge_metadata(task_id).await?.ok_or_else(|| {
            crate::Error::Cursor(format!(
                "任务 {task_id} 尚无 merge_result 产出，无法累计闸门失败"
            ))
        })?;
        stored.gate_failures += 1;
        self.upsert_stage_output(
            task_id,
            Stage::Merge,
            crate::types::MERGE_OUTPUT_TYPE,
            &stored.diff_path,
            Some(&serde_json::to_value(&stored)?),
        )
        .await?;
        Ok(stored.gate_failures)
    }

    // ─────────────────────────── 流转（§12.4.2）───────────────────────────

    #[allow(clippy::too_many_arguments)]
    pub async fn insert_transition(
        &self,
        task_id: &str,
        branch: &str,
        from: Option<(Stage, Node)>,
        to: (Stage, Node),
        trigger: TransitionTrigger,
        reason: Option<&str>,
    ) -> Result<i64> {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO kanban_transitions
             (task_id, branch, from_stage, from_node, to_stage, to_node, trigger, reason, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(task_id)
        .bind(branch)
        .bind(from.map(|(s, _)| s.as_str()))
        .bind(from.map(|(_, n)| n.as_str()))
        .bind(to.0.as_str())
        .bind(to.1.as_str())
        .bind(trigger.as_str())
        .bind(reason)
        .bind(ts(self.now()))
        .fetch_one(self.pool())
        .await?;
        Ok(id)
    }

    pub async fn list_transitions(&self, task_id: &str) -> Result<Vec<Transition>> {
        #[derive(FromRow)]
        struct Row {
            id: i64,
            task_id: String,
            branch: String,
            from_stage: Option<String>,
            from_node: Option<String>,
            to_stage: String,
            to_node: String,
            trigger: String,
            reason: Option<String>,
            created_at: String,
        }
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT id, task_id, branch, from_stage, from_node, to_stage, to_node, trigger, reason,
                    created_at
             FROM kanban_transitions WHERE task_id = ? ORDER BY id",
        )
        .bind(task_id)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(Transition {
                    id: r.id,
                    task_id: r.task_id,
                    branch: r.branch,
                    from_stage: r.from_stage.as_deref().map(decode_stage).transpose()?,
                    from_node: r.from_node.as_deref().map(decode_node).transpose()?,
                    to_stage: decode_stage(&r.to_stage)?,
                    to_node: decode_node(&r.to_node)?,
                    // 观测类字段：非法值 warn + 兜底，不让坏观测行打垮查询（Q6 分类）
                    trigger: decode_lossy(&r.trigger, "流转 trigger", TransitionTrigger::Normal),
                    reason: r.reason,
                    created_at: parse_ts(&r.created_at)?,
                })
            })
            .collect()
    }

    // ─────────────────────────── 会话（§12.4.3）───────────────────────────

    #[allow(clippy::too_many_arguments)]
    pub async fn insert_conversation(
        &self,
        task_id: &str,
        run_id: i64,
        stage: Stage,
        node: Node,
        attempt: u32,
        agent_type: &str,
        parent_run_id: Option<i64>,
        messages: &serde_json::Value,
        metadata: Option<&serde_json::Value>,
        prompt_tokens: u32,
        completion_tokens: u32,
    ) -> Result<i64> {
        // conversation_max_chars：超出截断（§12.4.3）。在 Value 层做——丢最旧轮次、
        // 单条仍超限时截其 content——保证落库的 messages_json 永远是合法 JSON。
        let truncated = truncate_messages_json(messages, self.conversation_max_chars);
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO kanban_node_conversations
             (task_id, project_id, run_id, stage, node, attempt, agent_type, parent_run_id,
              messages_json, metadata_json, prompt_tokens, completion_tokens, created_at)
             VALUES (?, NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(task_id)
        .bind(run_id)
        .bind(stage.as_str())
        .bind(node.as_str())
        .bind(attempt as i64)
        .bind(agent_type)
        .bind(parent_run_id)
        .bind(truncated.to_string())
        .bind(metadata.map(|m| m.to_string()))
        .bind(prompt_tokens as i64)
        .bind(completion_tokens as i64)
        .bind(ts(self.now()))
        .fetch_one(self.pool())
        .await?;
        Ok(id)
    }

    /// 落一行**项目级**伪阶段会话（票 10 / 决策 100）：`project_id` 归属项目，
    /// `task_id` 为 NULL，`run_id` 指向项目级 run。
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_project_conversation(
        &self,
        project_id: &str,
        run_id: i64,
        stage: Stage,
        node: Node,
        attempt: u32,
        agent_type: &str,
        messages: &serde_json::Value,
        metadata: Option<&serde_json::Value>,
        prompt_tokens: u32,
        completion_tokens: u32,
    ) -> Result<i64> {
        let truncated = truncate_messages_json(messages, self.conversation_max_chars);
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO kanban_node_conversations
             (task_id, project_id, run_id, stage, node, attempt, agent_type, parent_run_id,
              messages_json, metadata_json, prompt_tokens, completion_tokens, created_at)
             VALUES (NULL, ?, ?, ?, ?, ?, ?, NULL, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(project_id)
        .bind(run_id)
        .bind(stage.as_str())
        .bind(node.as_str())
        .bind(attempt as i64)
        .bind(agent_type)
        .bind(truncated.to_string())
        .bind(metadata.map(|m| m.to_string()))
        .bind(prompt_tokens as i64)
        .bind(completion_tokens as i64)
        .bind(ts(self.now()))
        .fetch_one(self.pool())
        .await?;
        Ok(id)
    }

    pub async fn get_conversation(
        &self,
        task_id: &str,
        run_id: i64,
    ) -> Result<Option<NodeConversation>> {
        let row: Option<ConversationRow> = sqlx::query_as(
            "SELECT id, task_id, project_id, run_id, stage, node, attempt, agent_type, parent_run_id,
                    messages_json, metadata_json, prompt_tokens, completion_tokens, created_at,
                    archived_at
             FROM kanban_node_conversations WHERE task_id = ? AND run_id = ?",
        )
        .bind(task_id)
        .bind(run_id)
        .fetch_optional(self.pool())
        .await?;
        row.map(ConversationRow::into_conversation).transpose()
    }

    /// 项目级伪阶段会话（票 10 / 决策 100）：按 `project_id + run_id` 取。
    pub async fn get_project_conversation(
        &self,
        project_id: &str,
        run_id: i64,
    ) -> Result<Option<NodeConversation>> {
        let row: Option<ConversationRow> = sqlx::query_as(
            "SELECT id, task_id, project_id, run_id, stage, node, attempt, agent_type, parent_run_id,
                    messages_json, metadata_json, prompt_tokens, completion_tokens, created_at,
                    archived_at
             FROM kanban_node_conversations WHERE project_id = ? AND run_id = ?",
        )
        .bind(project_id)
        .bind(run_id)
        .fetch_optional(self.pool())
        .await?;
        row.map(ConversationRow::into_conversation).transpose()
    }

    /// 归档任务的全部会话（重试时标记旧 attempt，§12.2 / 决策 113 同构）。
    ///
    /// 只更新 `archived_at`，绝不物理删除：历史仍可查，`run_id` 外键不悬空。
    /// 返回被标记的行数。
    pub async fn archive_conversations(&self, task_id: &str) -> Result<u64> {
        let affected = sqlx::query(
            "UPDATE kanban_node_conversations SET archived_at = ?
             WHERE task_id = ? AND archived_at IS NULL",
        )
        .bind(ts(self.now()))
        .bind(task_id)
        .execute(self.pool())
        .await?
        .rows_affected();
        Ok(affected)
    }

    /// 会话列表：默认只返回未归档行；`include_archived = true` 时取回全部历史 attempt
    /// （含被重试归档的旧会话，§12.2）。
    ///
    /// 归档判定在 SQL 层（`archived_at IS NULL`），不再依赖 run 列表，避免与
    /// 「重试后旧 run 仍在」的现实混淆。
    pub async fn list_conversations(
        &self,
        task_id: &str,
        include_archived: bool,
    ) -> Result<Vec<NodeConversation>> {
        let mut sql = String::from(
            "SELECT id, task_id, project_id, run_id, stage, node, attempt, agent_type, parent_run_id,
                    messages_json, metadata_json, prompt_tokens, completion_tokens, created_at,
                    archived_at
             FROM kanban_node_conversations WHERE task_id = ?",
        );
        if !include_archived {
            sql.push_str(" AND archived_at IS NULL");
        }
        sql.push_str(" ORDER BY id");
        let rows: Vec<ConversationRow> = sqlx::query_as(&sql)
            .bind(task_id)
            .fetch_all(self.pool())
            .await?;
        rows.into_iter()
            .map(ConversationRow::into_conversation)
            .collect()
    }

    /// 某 `(task, stage, node)` 上**主 agent 自己**最近一次的会话行（决策 180，票 13）。
    ///
    /// 「自己」由 `agent_type = 'main'` 界定：伪阶段与子代理的 run 复用父节点的 stage/node
    /// （决策 172 / 票 14），不过滤会把伪阶段的会话当成上一轮对话读回来。
    ///
    /// **只看已归档与否不影响取数**：续接要的是「上一 attempt 的 messages」，无论它是否
    /// 被 `archive_conversations` 标记过（归档是重试路径对**旧尝试**的标记，不是删除）。
    pub async fn latest_own_conversation(
        &self,
        task_id: &str,
        stage: Stage,
        node: Node,
    ) -> Result<Option<NodeConversation>> {
        let row: Option<ConversationRow> = sqlx::query_as(
            "SELECT id, task_id, project_id, run_id, stage, node, attempt, agent_type, parent_run_id,
                    messages_json, metadata_json, prompt_tokens, completion_tokens, created_at,
                    archived_at
             FROM kanban_node_conversations
             WHERE task_id = ? AND stage = ? AND node = ? AND agent_type = 'main'
             ORDER BY id DESC LIMIT 1",
        )
        .bind(task_id)
        .bind(stage.as_str())
        .bind(node.as_str())
        .fetch_optional(self.pool())
        .await?;
        row.map(ConversationRow::into_conversation).transpose()
    }

    /// 记录「本 run 续接了哪条历史 run」（决策 180，票 13）。
    ///
    /// 单独一条 UPDATE 而不是往 [`NewRun`] 加字段：续接只发生在主 agent 的
    /// [`crate::pipeline::executor`] 那一条路径上，其余 20 处构造点（子代理 / 伪阶段 /
    /// system / 测试）填 `None` 不表达任何信息，改全部构造点是纯噪声。
    pub async fn link_run_continuation(&self, run_id: i64, from_run_id: i64) -> Result<()> {
        sqlx::query("UPDATE kanban_node_runs SET continued_from_run_id = ? WHERE id = ?")
            .bind(from_run_id)
            .bind(run_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// 某项目的项目级伪阶段会话列表（票 10 / 决策 100）。
    pub async fn list_project_conversations(
        &self,
        project_id: &str,
    ) -> Result<Vec<NodeConversation>> {
        let rows: Vec<ConversationRow> = sqlx::query_as(
            "SELECT id, task_id, project_id, run_id, stage, node, attempt, agent_type, parent_run_id,
                    messages_json, metadata_json, prompt_tokens, completion_tokens, created_at,
                    archived_at
             FROM kanban_node_conversations WHERE project_id = ? ORDER BY id",
        )
        .bind(project_id)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter()
            .map(ConversationRow::into_conversation)
            .collect()
    }

    // ─────────────────────────── 命令日志（§12.4.4）───────────────────────────

    pub async fn list_commands(
        &self,
        task_id: &str,
        stage: Option<Stage>,
        node: Option<Node>,
    ) -> Result<Vec<NodeCommand>> {
        let mut sql = String::from(
            "SELECT id, task_id, session_id, run_id, stage, node, source, command, cwd, exit_code,
                    stdout_path, stdout_preview, stderr_preview, duration_ms, started_at, finished_at
             FROM kanban_node_commands WHERE task_id = ?",
        );
        if stage.is_some() {
            sql.push_str(" AND stage = ?");
        }
        if node.is_some() {
            sql.push_str(" AND node = ?");
        }
        sql.push_str(" ORDER BY id");
        let mut q = sqlx::query_as::<_, CommandRow>(&sql).bind(task_id);
        if let Some(s) = stage {
            q = q.bind(s.as_str());
        }
        if let Some(n) = node {
            q = q.bind(n.as_str());
        }
        let rows = q.fetch_all(self.pool()).await?;
        rows.into_iter().map(CommandRow::into_command).collect()
    }

    /// 某个值班会话的命令（决策 204④：值班长的命令没有 task_id，它挂会话）。
    ///
    /// 与 [`Store::list_commands`] 是**两条平行的读法**，不是同一个查询的两个过滤项
    /// ——归属列恰好一个非空，故两者永不重叠。
    pub async fn list_foreman_commands(&self, session_id: &str) -> Result<Vec<NodeCommand>> {
        let rows: Vec<CommandRow> = sqlx::query_as(
            "SELECT id, task_id, session_id, run_id, stage, node, source, command, cwd, exit_code,
                    stdout_path, stdout_preview, stderr_preview, duration_ms, started_at, finished_at
             FROM kanban_node_commands WHERE session_id = ? ORDER BY id",
        )
        .bind(session_id)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter().map(CommandRow::into_command).collect()
    }

    pub async fn get_command(&self, command_id: i64) -> Result<Option<NodeCommand>> {
        let row: Option<CommandRow> = sqlx::query_as(
            "SELECT id, task_id, session_id, run_id, stage, node, source, command, cwd, exit_code,
                    stdout_path, stdout_preview, stderr_preview, duration_ms, started_at, finished_at
             FROM kanban_node_commands WHERE id = ?",
        )
        .bind(command_id)
        .fetch_optional(self.pool())
        .await?;
        row.map(CommandRow::into_command).transpose()
    }
}

#[derive(Debug, FromRow)]
struct ConversationRow {
    id: i64,
    task_id: Option<String>,
    project_id: Option<String>,
    run_id: i64,
    stage: String,
    node: String,
    attempt: i64,
    agent_type: String,
    parent_run_id: Option<i64>,
    messages_json: String,
    metadata_json: Option<String>,
    prompt_tokens: i64,
    completion_tokens: i64,
    created_at: String,
    archived_at: Option<String>,
}

impl ConversationRow {
    fn into_conversation(self) -> Result<NodeConversation> {
        Ok(NodeConversation {
            id: self.id,
            task_id: self.task_id,
            project_id: self.project_id,
            run_id: self.run_id,
            stage: decode_stage(&self.stage)?,
            node: decode_node(&self.node)?,
            attempt: self.attempt as u32,
            agent_type: self.agent_type,
            parent_run_id: self.parent_run_id,
            messages_json: serde_json::from_str(&self.messages_json)?,
            metadata_json: self
                .metadata_json
                .map(|s| serde_json::from_str(&s))
                .transpose()?,
            prompt_tokens: self.prompt_tokens as u32,
            completion_tokens: self.completion_tokens as u32,
            created_at: parse_ts(&self.created_at)?,
            archived_at: self.archived_at.map(|s| parse_ts(&s)).transpose()?,
        })
    }
}

#[derive(Debug, FromRow)]
struct CommandRow {
    id: i64,
    task_id: Option<String>,
    session_id: Option<String>,
    run_id: Option<i64>,
    stage: String,
    node: String,
    source: String,
    command: String,
    cwd: String,
    exit_code: Option<i64>,
    stdout_path: Option<String>,
    stdout_preview: Option<String>,
    stderr_preview: Option<String>,
    duration_ms: Option<i64>,
    started_at: String,
    finished_at: Option<String>,
}

impl CommandRow {
    fn into_command(self) -> Result<NodeCommand> {
        Ok(NodeCommand {
            id: self.id,
            task_id: self.task_id,
            session_id: self.session_id,
            run_id: self.run_id,
            stage: decode_stage(&self.stage)?,
            node: decode_node(&self.node)?,
            // 观测类字段：非法值 warn + 兜底（Q6 分类）
            source: decode_lossy(&self.source, "命令来源", CommandSource::System),
            command: self.command,
            cwd: self.cwd,
            exit_code: self.exit_code.map(|v| v as i32),
            stdout_path: self.stdout_path,
            stdout_preview: self.stdout_preview,
            stderr_preview: self.stderr_preview,
            duration_ms: self.duration_ms.map(|v| v as u64),
            started_at: parse_ts(&self.started_at)?,
            finished_at: self.finished_at.map(|s| parse_ts(&s)).transpose()?,
        })
    }
}

/// 让 Store 直接充当工具层的命令记录器（§12.4.4 + 决策 100 心跳）。
impl CommandRecorder for Store {
    fn record_start(&self, start: CommandStart) -> BoxFuture<'static, Result<i64>> {
        let store = self.clone();
        Box::pin(async move {
            // 归属归一：空串不是归属，它是「没有」的伪装（迁移 0004:11 的原话
            // 「不采用哨兵值」）。归一之后由本函数给出人话错误，而不是让 SQLite
            // 抛一句 `CHECK constraint failed: commands_new`。
            let task_id = start.task_id.filter(|s| !s.is_empty());
            let session_id = start.session_id.filter(|s| !s.is_empty());
            if task_id.is_none() == session_id.is_none() {
                return Err(crate::Error::Validation(
                    "命令必须恰好属于一个归属（流水线任务或值班会话）".into(),
                ));
            }
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO kanban_node_commands
                 (task_id, session_id, run_id, stage, node, source, command, cwd, exit_code,
                  stdout_path, stdout_preview, stderr_preview, duration_ms, started_at, finished_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL, NULL, NULL, NULL, ?, NULL)
                 RETURNING id",
            )
            .bind(&task_id)
            .bind(&session_id)
            .bind(start.run_id)
            .bind(start.stage.as_str())
            .bind(start.node.as_str())
            .bind(start.source.as_str())
            .bind(&start.command)
            .bind(&start.cwd)
            .bind(ts(store.now()))
            .fetch_one(store.pool())
            .await?;
            Ok(id)
        })
    }

    fn record_finish(
        &self,
        command_id: i64,
        finish: CommandFinish,
    ) -> BoxFuture<'static, Result<()>> {
        let store = self.clone();
        Box::pin(async move {
            sqlx::query(
                "UPDATE kanban_node_commands
                 SET exit_code = ?, stdout_path = ?, stdout_preview = ?, stderr_preview = ?,
                     duration_ms = ?, finished_at = ?
                 WHERE id = ?",
            )
            .bind(finish.exit_code)
            .bind(&finish.stdout_path)
            .bind(&finish.stdout_preview)
            .bind(&finish.stderr_preview)
            .bind(finish.duration_ms as i64)
            .bind(ts(store.now()))
            .bind(command_id)
            .execute(store.pool())
            .await?;
            Ok(())
        })
    }

    fn touch_heartbeat(&self, run_id: Option<i64>) -> BoxFuture<'static, Result<()>> {
        let store = self.clone();
        Box::pin(async move {
            if let Some(id) = run_id {
                store.touch_run_heartbeat(id).await?;
            }
            Ok(())
        })
    }

    fn set_process_group(&self, run_id: i64, pgid: i32) -> BoxFuture<'static, Result<()>> {
        let store = self.clone();
        Box::pin(async move { store.set_run_process_group(run_id, pgid).await })
    }
}

/// 未完成的 run 视为崩溃残留（进程恢复时判定用）。
pub async fn stale_running_runs(store: &Store, now: DateTime<Utc>) -> Result<Vec<NodeRun>> {
    let runs = store.active_runs().await?;
    Ok(runs.into_iter().filter(|r| r.started_at < now).collect())
}

/// 会话落库截断（§12.4.3 `conversation_max_chars`）：
/// 超限时从最旧的消息开始丢弃（保留最近轮次，与 keep_recent_rounds 语义一致）；
/// 单条仍超限则截断其 content 字段，并追加一条截断标记消息。
/// 输出永远是合法 JSON（重新序列化自 Value，绝不按字符拦腰截断）。
pub fn truncate_messages_json(messages: &serde_json::Value, max_chars: usize) -> serde_json::Value {
    if messages.to_string().chars().count() <= max_chars {
        return messages.clone();
    }
    let Some(arr) = messages.as_array() else {
        return messages.clone();
    };
    // 从最旧开始丢
    for keep in 1..=arr.len() {
        let candidate = serde_json::Value::Array(arr[arr.len() - keep..].to_vec());
        if candidate.to_string().chars().count() <= max_chars {
            return candidate;
        }
    }
    // 只留最新一条仍超限：整条替换为截断标记（超长内容可能在 tool_calls.arguments
    // 等任意嵌套位置，逐字段截不可靠；直接以合法 JSON 的标记消息兜底）
    serde_json::Value::Array(vec![serde_json::json!({
        "role": "system",
        "content": format!("…[已截断，原文 {} 字符]", messages.to_string().chars().count()),
    })])
}

/// run 是否处于超时窗口（决策 64 / 66）。
pub fn is_timed_out(
    run: &NodeRun,
    now: DateTime<Utc>,
    idle_timeout_sec: u64,
    max_duration_sec: u64,
) -> Option<NodeStatus> {
    let started = run.started_at;
    let last_activity = run.last_activity_at.unwrap_or(started);
    let idle = (now - last_activity).num_seconds().max(0) as u64;
    let total = (now - started).num_seconds().max(0) as u64;
    if total > max_duration_sec {
        return Some(NodeStatus::Timeout);
    }
    if idle > idle_timeout_sec {
        return Some(NodeStatus::Timeout);
    }
    None
}

/// 错误便捷构造。
pub fn run_missing(task_id: &str, run_id: i64) -> Error {
    Error::Task(format!("任务 {task_id} 没有 run {run_id}"))
}
