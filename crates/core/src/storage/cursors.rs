//! 游标生命周期（决策 80 / 90 / 91 / 93 / 113）——执行状态的唯一事实来源。
//!
//! 硬约束：**游标行永不物理删除**。合并 / 回退 / 重试都是"归档旧行 + 插入新行"，
//! 因此 `kanban_node_runs.cursor_id` 的 NOT NULL 外键永不悬空。

use chrono::{DateTime, Utc};
use sqlx::FromRow;

use super::{decode_node, decode_pending, decode_stage, encode_pending, parse_ts, ts, Store};
use crate::types::{CursorStatus, Node, NodeCursor, PendingKind, PendingReason, Stage};
use crate::{Error, Result};

#[derive(Debug, FromRow)]
struct CursorRow {
    cursor_id: String,
    task_id: String,
    branch: String,
    stage: String,
    node: String,
    status: String,
    validate_attempts: i64,
    skipped_to_join: i64,
    pending_reason_json: Option<String>,
    created_at: String,
    updated_at: String,
}

impl CursorRow {
    fn into_cursor(self) -> Result<NodeCursor> {
        Ok(NodeCursor {
            cursor_id: self.cursor_id,
            task_id: self.task_id,
            branch: self.branch,
            stage: decode_stage(&self.stage)?,
            node: decode_node(&self.node)?,
            status: CursorStatus::from_str_lossy(&self.status),
            validate_attempts: self.validate_attempts as u32,
            skipped_to_join: self.skipped_to_join != 0,
            pending_reason: decode_pending(self.pending_reason_json)?,
            created_at: parse_ts(&self.created_at)?,
            updated_at: parse_ts(&self.updated_at)?,
        })
    }
}

impl CursorStatus {
    /// DB 取值容错（未知值视为 archived，避免把历史行当成可执行行）。
    pub fn from_str_lossy(raw: &str) -> Self {
        match raw {
            "active" => CursorStatus::Active,
            "waiting_join" => CursorStatus::WaitingJoin,
            "pending" => CursorStatus::Pending,
            _ => CursorStatus::Archived,
        }
    }
}

const SELECT_COLUMNS: &str = "cursor_id, task_id, branch, stage, node, status, \
     validate_attempts, skipped_to_join, pending_reason_json, created_at, updated_at";

impl Store {
    /// `POST /tasks` 同事务插入的单条 main 游标（决策 90：waiting / queued 也有游标）。
    pub async fn create_initial_cursor(&self, task_id: &str) -> Result<NodeCursor> {
        let now = self.now();
        let cursor_id = ulid::Ulid::new().to_string();
        sqlx::query(
            "INSERT INTO kanban_node_cursors
             (cursor_id, task_id, branch, stage, node, status, validate_attempts,
              skipped_to_join, pending_reason_json, created_at, updated_at)
             VALUES (?, ?, 'main', 'init', 'execute', 'active', 0, 0, NULL, ?, ?)",
        )
        .bind(&cursor_id)
        .bind(task_id)
        .bind(ts(now))
        .bind(ts(now))
        .execute(self.pool())
        .await?;
        self.get_cursor(&cursor_id).await
    }

    /// 全部活跃游标（决策 113：历史行不加载）。
    pub async fn load_live_cursors(&self, task_id: &str) -> Result<Vec<NodeCursor>> {
        let sql = format!(
            "SELECT {SELECT_COLUMNS} FROM kanban_node_cursors
             WHERE task_id = ? AND status != 'archived' ORDER BY created_at, branch"
        );
        let rows: Vec<CursorRow> = sqlx::query_as(&sql)
            .bind(task_id)
            .fetch_all(self.pool())
            .await?;
        rows.into_iter().map(CursorRow::into_cursor).collect()
    }

    /// 含归档行的全量游标（审计 / 测试断言用）。
    pub async fn load_all_cursors(&self, task_id: &str) -> Result<Vec<NodeCursor>> {
        let sql = format!(
            "SELECT {SELECT_COLUMNS} FROM kanban_node_cursors
             WHERE task_id = ? ORDER BY created_at, branch"
        );
        let rows: Vec<CursorRow> = sqlx::query_as(&sql)
            .bind(task_id)
            .fetch_all(self.pool())
            .await?;
        rows.into_iter().map(CursorRow::into_cursor).collect()
    }

    pub async fn get_cursor(&self, cursor_id: &str) -> Result<NodeCursor> {
        let sql = format!("SELECT {SELECT_COLUMNS} FROM kanban_node_cursors WHERE cursor_id = ?");
        let row: CursorRow = sqlx::query_as(&sql)
            .bind(cursor_id)
            .fetch_optional(self.pool())
            .await?
            .ok_or_else(|| Error::Cursor(format!("游标不存在：{cursor_id}")))?;
        row.into_cursor()
    }

    /// 恰好一条活跃游标时可省略 `cursor_id`；0 条或多条 → `None`（API 层映射为 409，决策 91）。
    pub async fn resolve_sole_cursor(&self, task_id: &str) -> Result<Option<NodeCursor>> {
        let live = self.load_live_cursors(task_id).await?;
        Ok(if live.len() == 1 {
            live.into_iter().next()
        } else {
            None
        })
    }

    /// architect-design 通过后的分裂点（决策 90）：main 行**就地改写**为 develop-design，
    /// 并插入 test-design 行。partial unique 保证幂等。
    pub async fn split_cursors(&self, task_id: &str) -> Result<Vec<NodeCursor>> {
        let now = self.now();
        let mut tx = self.pool().begin().await?;

        // 幂等：已分裂则直接返回两条分支游标
        let existing: Vec<CursorRow> = sqlx::query_as(&format!(
            "SELECT {SELECT_COLUMNS} FROM kanban_node_cursors
             WHERE task_id = ? AND status != 'archived' AND branch IN ('develop-design','test-design')
             ORDER BY branch"
        ))
        .bind(task_id)
        .fetch_all(&mut *tx)
        .await?;
        if existing.len() == 2 {
            tx.commit().await?;
            return existing
                .into_iter()
                .map(CursorRow::into_cursor)
                .collect::<Result<Vec<_>>>();
        }

        let main = self.live_main_cursor_tx(&mut tx, task_id).await?;
        let main_id = main.cursor_id.clone();
        sqlx::query(
            "UPDATE kanban_node_cursors
             SET branch = 'develop-design', stage = 'develop-design', node = 'validate_input',
                 status = 'active', validate_attempts = 0, skipped_to_join = 0,
                 pending_reason_json = NULL, updated_at = ?
             WHERE cursor_id = ?",
        )
        .bind(ts(now))
        .bind(&main_id)
        .execute(&mut *tx)
        .await?;

        let test_cursor_id = ulid::Ulid::new().to_string();
        sqlx::query(
            "INSERT INTO kanban_node_cursors
             (cursor_id, task_id, branch, stage, node, status, validate_attempts,
              skipped_to_join, pending_reason_json, created_at, updated_at)
             VALUES (?, ?, 'test-design', 'test-design', 'validate_input', 'active', 0, 0, NULL, ?, ?)",
        )
        .bind(&test_cursor_id)
        .bind(task_id)
        .bind(ts(now))
        .bind(ts(now))
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(vec![
            self.get_cursor(&main_id).await?,
            self.get_cursor(&test_cursor_id).await?,
        ])
    }

    /// join 通过 / 回退 / 重试的统一归档+插入：单事务内归档全部活跃行，插入单条 main（决策 90 / 113）。
    pub async fn replace_cursors_with_main(
        &self,
        task_id: &str,
        stage: Stage,
        node: Node,
        branch: &str,
    ) -> Result<NodeCursor> {
        let now = self.now();
        let mut tx = self.pool().begin().await?;
        sqlx::query(
            "UPDATE kanban_node_cursors SET status = 'archived', updated_at = ?
             WHERE task_id = ? AND status != 'archived'",
        )
        .bind(ts(now))
        .bind(task_id)
        .execute(&mut *tx)
        .await?;

        let cursor_id = ulid::Ulid::new().to_string();
        sqlx::query(
            "INSERT INTO kanban_node_cursors
             (cursor_id, task_id, branch, stage, node, status, validate_attempts,
              skipped_to_join, pending_reason_json, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, 'active', 0, 0, NULL, ?, ?)",
        )
        .bind(&cursor_id)
        .bind(task_id)
        .bind(branch)
        .bind(stage.as_str())
        .bind(node.as_str())
        .bind(ts(now))
        .bind(ts(now))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.get_cursor(&cursor_id).await
    }

    /// 合并两条分支游标 → 单条 main（proceed：develop.execute）。
    pub async fn merge_cursors_to_develop(&self, task_id: &str) -> Result<NodeCursor> {
        self.replace_cursors_with_main(
            task_id,
            Stage::Develop,
            Node::Execute,
            NodeCursor::BRANCH_MAIN,
        )
        .await
    }

    /// 回退（backtrack）：两条分支一起重置到 architect-design.validate_input（决策 83）。
    pub async fn backtrack_cursors(&self, task_id: &str) -> Result<NodeCursor> {
        self.replace_cursors_with_main(
            task_id,
            Stage::ArchitectDesign,
            Node::ValidateInput,
            NodeCursor::BRANCH_MAIN,
        )
        .await
    }

    /// 重试：归档全部旧行、插入单条 main 指向 init.execute（决策 90 / 125）。
    pub async fn reset_cursors_to_init(&self, task_id: &str) -> Result<NodeCursor> {
        self.replace_cursors_with_main(task_id, Stage::Init, Node::Execute, NodeCursor::BRANCH_MAIN)
            .await
    }

    /// 跨阶段跳转：置 stage / node 并把 `validate_attempts` 归零（决策 43）。
    pub async fn set_cursor_stage(&self, cursor_id: &str, stage: Stage, node: Node) -> Result<()> {
        sqlx::query(
            "UPDATE kanban_node_cursors
             SET stage = ?, node = ?, validate_attempts = 0, updated_at = ?
             WHERE cursor_id = ?",
        )
        .bind(stage.as_str())
        .bind(node.as_str())
        .bind(ts(self.now()))
        .bind(cursor_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// 阶段内重试：只加 attempts，不改 stage / node。
    pub async fn increment_cursor_attempts(&self, cursor_id: &str) -> Result<u32> {
        let attempts: i64 = sqlx::query_scalar(
            "UPDATE kanban_node_cursors
             SET validate_attempts = validate_attempts + 1, updated_at = ?
             WHERE cursor_id = ?
             RETURNING validate_attempts",
        )
        .bind(ts(self.now()))
        .bind(cursor_id)
        .fetch_one(self.pool())
        .await?;
        Ok(attempts as u32)
    }

    /// 仅移动 stage / node，**不重置 attempts**（阶段内重试回边 validate_output → execute；
    /// 跨阶段跳转必须用 [`Store::set_cursor_stage`]，它按决策 43 归零）。
    pub async fn move_cursor(&self, cursor_id: &str, stage: Stage, node: Node) -> Result<()> {
        sqlx::query(
            "UPDATE kanban_node_cursors SET stage = ?, node = ?, updated_at = ? WHERE cursor_id = ?",
        )
        .bind(stage.as_str())
        .bind(node.as_str())
        .bind(ts(self.now()))
        .bind(cursor_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn reset_cursor_attempts(&self, cursor_id: &str) -> Result<()> {
        sqlx::query("UPDATE kanban_node_cursors SET validate_attempts = 0, updated_at = ? WHERE cursor_id = ?")
            .bind(ts(self.now()))
            .bind(cursor_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// 置 pending（挂在该游标上，决策 82）。
    pub async fn set_cursor_pending(&self, cursor_id: &str, reason: &PendingReason) -> Result<()> {
        sqlx::query(
            "UPDATE kanban_node_cursors
             SET status = 'pending', pending_reason_json = ?, updated_at = ?
             WHERE cursor_id = ?",
        )
        .bind(encode_pending(&Some(reason.clone())))
        .bind(ts(self.now()))
        .bind(cursor_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// 清 pending 并恢复为 active（不改变 stage / node）。
    pub async fn clear_cursor_pending(&self, cursor_id: &str) -> Result<()> {
        sqlx::query(
            "UPDATE kanban_node_cursors
             SET status = 'active', pending_reason_json = NULL, updated_at = ?
             WHERE cursor_id = ? AND status = 'pending'",
        )
        .bind(ts(self.now()))
        .bind(cursor_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// 并行分支 skip：置 `waiting_join` + `skipped_to_join = 1`（决策 93），
    /// **不改写产出元数据**（不伪造 readiness）。
    pub async fn mark_cursor_skipped_to_join(&self, cursor_id: &str) -> Result<()> {
        sqlx::query(
            "UPDATE kanban_node_cursors
             SET status = 'waiting_join', skipped_to_join = 1,
                 pending_reason_json = NULL, updated_at = ?
             WHERE cursor_id = ?",
        )
        .bind(ts(self.now()))
        .bind(cursor_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// `advance_cursor` 发现 next 是 join 时的唯一写入路径（决策 107）。
    pub async fn set_cursor_waiting_join(&self, cursor_id: &str) -> Result<()> {
        sqlx::query(
            "UPDATE kanban_node_cursors
             SET status = 'waiting_join', updated_at = ?
             WHERE cursor_id = ? AND status != 'archived'",
        )
        .bind(ts(self.now()))
        .bind(cursor_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// conflict_wait 复检后更新冲突对象（决策 102：保持 pending，只换 id 列表）。
    pub async fn update_cursor_pending_context(
        &self,
        cursor_id: &str,
        key: &str,
        values: &[String],
    ) -> Result<()> {
        let cursor = self.get_cursor(cursor_id).await?;
        let mut reason = cursor
            .pending_reason
            .ok_or_else(|| Error::Cursor(format!("游标 {cursor_id} 当前没有 pending")))?;
        let mut ctx = reason.context.unwrap_or_default();
        if key == "conflict_task_ids" {
            ctx.conflict_task_ids = values.to_vec();
        } else {
            ctx.extra.insert(
                key.to_string(),
                serde_json::Value::Array(
                    values
                        .iter()
                        .map(|v| serde_json::Value::String(v.clone()))
                        .collect(),
                ),
            );
        }
        reason.context = Some(ctx);
        sqlx::query("UPDATE kanban_node_cursors SET pending_reason_json = ?, updated_at = ? WHERE cursor_id = ?")
            .bind(encode_pending(&Some(reason)))
            .bind(ts(self.now()))
            .bind(cursor_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// 归档某条游标（用户 resume 后旧游标让位等场景）。
    pub async fn archive_cursor(&self, cursor_id: &str) -> Result<()> {
        sqlx::query("UPDATE kanban_node_cursors SET status = 'archived', updated_at = ? WHERE cursor_id = ?")
            .bind(ts(self.now()))
            .bind(cursor_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// pending 类型查询（scheduler conflict_wait 恢复用）。
    pub async fn cursors_with_pending_kind(&self, kind: PendingKind) -> Result<Vec<NodeCursor>> {
        let sql = format!(
            "SELECT {SELECT_COLUMNS} FROM kanban_node_cursors
             WHERE status = 'pending' ORDER BY updated_at"
        );
        let rows: Vec<CursorRow> = sqlx::query_as(&sql).fetch_all(self.pool()).await?;
        let mut out = Vec::new();
        for row in rows {
            let cursor = row.into_cursor()?;
            if cursor.pending_reason.as_ref().map(|r| r.kind) == Some(kind) {
                out.push(cursor);
            }
        }
        Ok(out)
    }

    /// 全部 pending 游标（调度器提醒 / stalled 用）。
    pub async fn pending_cursors(&self) -> Result<Vec<NodeCursor>> {
        let sql = format!(
            "SELECT {SELECT_COLUMNS} FROM kanban_node_cursors
             WHERE status = 'pending' ORDER BY updated_at"
        );
        let rows: Vec<CursorRow> = sqlx::query_as(&sql).fetch_all(self.pool()).await?;
        rows.into_iter().map(CursorRow::into_cursor).collect()
    }

    /// 活跃游标（超时检测 / 恢复用）。
    pub async fn active_cursors(&self) -> Result<Vec<NodeCursor>> {
        let sql = format!(
            "SELECT {SELECT_COLUMNS} FROM kanban_node_cursors
             WHERE status = 'active' ORDER BY updated_at"
        );
        let rows: Vec<CursorRow> = sqlx::query_as(&sql).fetch_all(self.pool()).await?;
        rows.into_iter().map(CursorRow::into_cursor).collect()
    }

    async fn live_main_cursor_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        task_id: &str,
    ) -> Result<NodeCursor> {
        let sql = format!(
            "SELECT {SELECT_COLUMNS} FROM kanban_node_cursors
             WHERE task_id = ? AND branch = 'main' AND status != 'archived' LIMIT 1"
        );
        let row: Option<CursorRow> = sqlx::query_as(&sql)
            .bind(task_id)
            .fetch_optional(&mut **tx)
            .await?;
        row.map(CursorRow::into_cursor)
            .transpose()?
            .ok_or_else(|| Error::Cursor(format!("任务 {task_id} 没有活跃 main 游标")))
    }

    /// 该任务最近一次游标更新时间（看板排序用）。
    pub async fn cursors_updated_at(&self, task_id: &str) -> Result<Option<DateTime<Utc>>> {
        let raw: Option<String> =
            sqlx::query_scalar("SELECT MAX(updated_at) FROM kanban_node_cursors WHERE task_id = ?")
                .bind(task_id)
                .fetch_one(self.pool())
                .await?;
        raw.map(|s| parse_ts(&s)).transpose()
    }
}
