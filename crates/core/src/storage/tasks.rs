//! 任务 / 依赖 / 并发准入（决策 21 / 36 / 57 / 98 / 116 / 117 / 127）。

use chrono::{DateTime, Utc};
use sqlx::FromRow;

use super::{decode_node, decode_pending, decode_stage, encode_pending, parse_ts, ts, Store};
use crate::pipeline::cursor::{focus_cursor, project_pending_reason, project_task_status};
use crate::types::{NodeCursor, ReviewMode, Stage, Task, TaskStatus};
use crate::{Error, Result};

/// 创建任务的输入。
#[derive(Debug, Clone)]
pub struct NewTask {
    pub id: String,
    pub title: String,
    pub description: String,
    pub project_id: String,
    pub review_mode: ReviewMode,
    pub model_override: Option<String>,
    /// 前置任务 id。
    pub depends_on: Vec<String>,
}

impl NewTask {
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        project_id: impl Into<String>,
    ) -> Self {
        NewTask {
            id: id.into(),
            title: title.into(),
            description: String::new(),
            project_id: project_id.into(),
            review_mode: ReviewMode::Agent,
            model_override: None,
            depends_on: Vec::new(),
        }
    }
}

/// 任务列表过滤（决策 101：`GET /tasks` 是看板唯一数据源）。
#[derive(Debug, Clone, Default)]
pub struct TaskFilter {
    pub project_id: Option<String>,
    pub status: Option<TaskStatus>,
    pub include_archived: bool,
}

#[derive(Debug, FromRow)]
struct TaskRow {
    id: String,
    project_id: String,
    title: String,
    description: String,
    status: String,
    current_stage: String,
    current_node: String,
    validate_attempts: i64,
    pending_reason_json: Option<String>,
    worktree_path: Option<String>,
    branch_name: Option<String>,
    total_tokens: i64,
    total_calls: i64,
    review_mode: String,
    model_override: Option<String>,
    archived_at: Option<String>,
    stalled: i64,
    executor_owner: Option<String>,
    created_at: String,
    updated_at: String,
}

impl TaskRow {
    fn into_task(self) -> Result<Task> {
        Ok(Task {
            id: self.id,
            project_id: self.project_id,
            title: self.title,
            description: self.description,
            // 执行语义字段：损坏即报错（status→queued 会让坏行重新可被准入）
            status: self.status.parse()?,
            current_stage: decode_stage(&self.current_stage)?,
            current_node: decode_node(&self.current_node)?,
            validate_attempts: self.validate_attempts as u32,
            pending_reason: decode_pending(self.pending_reason_json)?,
            worktree_path: self.worktree_path,
            branch_name: self.branch_name,
            total_tokens: self.total_tokens as u64,
            total_calls: self.total_calls as u64,
            review_mode: match self.review_mode.as_str() {
                "agent" => ReviewMode::Agent,
                "human" => ReviewMode::Human,
                other => return Err(Error::Validation(format!("未知评审模式：{other}"))),
            },
            model_override: self.model_override,
            archived_at: self.archived_at.map(|s| parse_ts(&s)).transpose()?,
            stalled: self.stalled != 0,
            executor_owner: self.executor_owner,
            created_at: parse_ts(&self.created_at)?,
            updated_at: parse_ts(&self.updated_at)?,
        })
    }
}

const TASK_COLUMNS: &str = "id, project_id, title, description, status, current_stage, current_node, \
     validate_attempts, pending_reason_json, worktree_path, branch_name, total_tokens, total_calls, \
     review_mode, model_override, archived_at, stalled, executor_owner, created_at, updated_at";

impl Store {
    /// 创建任务 + 同事务插入单条 main 游标（决策 90）。
    ///
    /// **一律以 `queued`（有依赖则 `waiting`）落库**，由 scheduler 准入（决策 98）。
    pub async fn create_task(&self, new_task: &NewTask) -> Result<Task> {
        let now = self.now();
        let status = if new_task.depends_on.is_empty() {
            TaskStatus::Queued
        } else {
            TaskStatus::Waiting
        };
        let mut tx = self.begin_write().await?;

        sqlx::query(
            "INSERT INTO kanban_tasks
             (id, title, description, project_id, status, current_stage, current_node,
              validate_attempts, pending_reason_json, worktree_path, branch_name,
              total_tokens, total_calls, review_mode, model_override, archived_at, stalled,
              executor_owner, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, 'init', 'execute', 0, NULL, NULL, NULL, 0, 0, ?, ?, NULL, 0, NULL, ?, ?)",
        )
        .bind(&new_task.id)
        .bind(&new_task.title)
        .bind(&new_task.description)
        .bind(&new_task.project_id)
        .bind(status.as_str())
        .bind(match new_task.review_mode {
            ReviewMode::Agent => "agent",
            ReviewMode::Human => "human",
        })
        .bind(&new_task.model_override)
        .bind(ts(now))
        .bind(ts(now))
        .execute(&mut *tx)
        .await?;

        // 同事务插入初始 main 游标：waiting / queued 任务也有游标，dependency_failed 因此有处可挂
        let cursor_id = ulid::Ulid::new().to_string();
        sqlx::query(
            "INSERT INTO kanban_node_cursors
             (cursor_id, task_id, branch, stage, node, status, validate_attempts,
              skipped_to_join, pending_reason_json, created_at, updated_at)
             VALUES (?, ?, 'main', 'init', 'execute', 'active', 0, 0, NULL, ?, ?)",
        )
        .bind(&cursor_id)
        .bind(&new_task.id)
        .bind(ts(now))
        .bind(ts(now))
        .execute(&mut *tx)
        .await?;

        for dep in &new_task.depends_on {
            sqlx::query("INSERT INTO kanban_task_deps (task_id, depends_on_id) VALUES (?, ?)")
                .bind(&new_task.id)
                .bind(dep)
                .execute(&mut *tx)
                .await?;
        }

        tx.commit().await?;
        self.get_task(&new_task.id).await
    }

    pub async fn get_task(&self, task_id: &str) -> Result<Task> {
        let sql = format!("SELECT {TASK_COLUMNS} FROM kanban_tasks WHERE id = ?");
        let row: TaskRow = sqlx::query_as(&sql)
            .bind(task_id)
            .fetch_optional(self.pool())
            .await?
            .ok_or_else(|| Error::Task(format!("任务不存在：{task_id}")))?;
        row.into_task()
    }

    pub async fn list_tasks(&self, filter: &TaskFilter) -> Result<Vec<Task>> {
        let mut sql = format!("SELECT {TASK_COLUMNS} FROM kanban_tasks WHERE 1 = 1");
        if !filter.include_archived {
            sql.push_str(" AND archived_at IS NULL");
        }
        if filter.project_id.is_some() {
            sql.push_str(" AND project_id = ?");
        }
        if filter.status.is_some() {
            sql.push_str(" AND status = ?");
        }
        sql.push_str(" ORDER BY created_at DESC");

        let mut q = sqlx::query_as::<_, TaskRow>(&sql);
        if let Some(p) = &filter.project_id {
            q = q.bind(p);
        }
        if let Some(s) = filter.status {
            q = q.bind(s.as_str());
        }
        let rows = q.fetch_all(self.pool()).await?;
        rows.into_iter().map(TaskRow::into_task).collect()
    }

    /// 任务级 provider 覆盖（决策 105 / 129）。
    pub async fn set_task_model_override(
        &self,
        task_id: &str,
        provider_id: Option<&str>,
    ) -> Result<()> {
        sqlx::query("UPDATE kanban_tasks SET model_override = ?, updated_at = ? WHERE id = ?")
            .bind(provider_id)
            .bind(ts(self.now()))
            .bind(task_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn set_task_status(&self, task_id: &str, status: TaskStatus) -> Result<()> {
        sqlx::query("UPDATE kanban_tasks SET status = ?, updated_at = ? WHERE id = ?")
            .bind(status.as_str())
            .bind(ts(self.now()))
            .bind(task_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn set_task_worktree(
        &self,
        task_id: &str,
        worktree_path: &str,
        branch_name: &str,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE kanban_tasks SET worktree_path = ?, branch_name = ?, updated_at = ? WHERE id = ?",
        )
        .bind(worktree_path)
        .bind(branch_name)
        .bind(ts(self.now()))
        .bind(task_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// 刷新焦点游标投影（决策 80 / 92）：`current_stage` / `current_node` /
    /// `validate_attempts` / `pending_reason_json` 都来自焦点游标。
    ///
    /// 只在任务已 `running` / `pending` 时投影——`queued` / `waiting` 的语义由
    /// 准入与依赖路径决定，不能被游标投影覆盖。
    pub async fn sync_task_projection(&self, task_id: &str) -> Result<Task> {
        let task = self.get_task(task_id).await?;
        let live = self.load_live_cursors(task_id).await?;
        if live.is_empty() {
            return Ok(task);
        }
        let focus = focus_cursor(&live).cloned();
        let projected_status = match task.status {
            // 执行中的任务在 running / pending 之间投影
            TaskStatus::Running | TaskStatus::Pending => Some(project_task_status(&live)),
            // queued / waiting 不被投影成 running，但"有游标被阻塞"会投影成 pending
            // （dependency_failed 就挂在这类任务的 main 游标上，决策 90 / 116）
            TaskStatus::Queued | TaskStatus::Waiting
                if crate::pipeline::cursor::has_pending_cursor(&live) =>
            {
                Some(TaskStatus::Pending)
            }
            _ => None,
        };
        let pending = project_pending_reason(&live);
        let Some(focus) = focus else {
            return Ok(task);
        };

        sqlx::query(
            "UPDATE kanban_tasks
             SET current_stage = ?, current_node = ?, validate_attempts = ?,
                 pending_reason_json = ?, status = COALESCE(?, status), updated_at = ?
             WHERE id = ?",
        )
        .bind(focus.stage.as_str())
        .bind(focus.node.as_str())
        .bind(focus.validate_attempts as i64)
        .bind(encode_pending(&pending))
        .bind(projected_status.map(|s| s.as_str()))
        .bind(ts(self.now()))
        .bind(task_id)
        .execute(self.pool())
        .await?;
        self.get_task(task_id).await
    }

    /// 从 runs 重算 `total_tokens` / `total_calls`（口径见 metrics，决策 130 ②）。
    pub async fn refresh_task_totals(&self, task_id: &str) -> Result<()> {
        let runs = self.list_runs(task_id).await?;
        let tokens = crate::metrics::total_tokens(&runs);
        let calls = crate::metrics::total_calls(&runs);
        sqlx::query("UPDATE kanban_tasks SET total_tokens = ?, total_calls = ?, updated_at = ? WHERE id = ?")
            .bind(tokens as i64)
            .bind(calls as i64)
            .bind(ts(self.now()))
            .bind(task_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn archive_task(&self, task_id: &str) -> Result<()> {
        let now = self.now();
        sqlx::query("UPDATE kanban_tasks SET archived_at = ?, updated_at = ? WHERE id = ?")
            .bind(ts(now))
            .bind(ts(now))
            .bind(task_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    // ─────────────────────────── 依赖（决策 57 / 116）───────────────────────────

    pub async fn add_dependency(&self, task_id: &str, depends_on_id: &str) -> Result<()> {
        sqlx::query(
            "INSERT OR IGNORE INTO kanban_task_deps (task_id, depends_on_id) VALUES (?, ?)",
        )
        .bind(task_id)
        .bind(depends_on_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn dependencies_of(&self, task_id: &str) -> Result<Vec<String>> {
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT depends_on_id FROM kanban_task_deps WHERE task_id = ? ORDER BY depends_on_id",
        )
        .bind(task_id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }

    /// 依赖本任务的任务（派生，不落库；§4.1 `blocks`）。
    pub async fn dependents_of(&self, task_id: &str) -> Result<Vec<String>> {
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT task_id FROM kanban_task_deps WHERE depends_on_id = ? ORDER BY task_id",
        )
        .bind(task_id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }

    /// 循环依赖检测（决策 27）：沿 `depends_on` 方向 DFS，若回到待创建任务 id 则有环。
    ///
    /// `deps` 是拟创建任务的依赖列表——该任务本身尚未落库，所以环一定经过它。
    pub async fn would_create_cycle(&self, task_id: &str, deps: &[String]) -> Result<bool> {
        let mut stack: Vec<String> = deps.to_vec();
        let mut seen: Vec<String> = Vec::new();
        while let Some(current) = stack.pop() {
            if current == task_id {
                return Ok(true);
            }
            if seen.contains(&current) {
                continue;
            }
            seen.push(current.clone());
            stack.extend(self.dependencies_of(&current).await?);
        }
        Ok(false)
    }

    // ─────────────────────────── 并发准入（决策 36 / 98 / 117）───────────────────────────

    /// 名额占用 = `status ∈ {running, pending}`（决策 117）。
    pub async fn occupying_slots(&self) -> Result<usize> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM kanban_tasks
             WHERE archived_at IS NULL AND status IN ('running', 'pending')",
        )
        .fetch_one(self.pool())
        .await?;
        Ok(count as usize)
    }

    /// 排队等名额的任务，按创建时间先来先得。
    pub async fn queued_tasks(&self) -> Result<Vec<Task>> {
        let sql = format!(
            "SELECT {TASK_COLUMNS} FROM kanban_tasks
             WHERE status = 'queued' AND archived_at IS NULL ORDER BY created_at"
        );
        let rows: Vec<TaskRow> = sqlx::query_as(&sql).fetch_all(self.pool()).await?;
        rows.into_iter().map(TaskRow::into_task).collect()
    }

    /// 依赖未完成的任务（`check_waiting_tasks` 用）。
    pub async fn waiting_tasks(&self) -> Result<Vec<Task>> {
        let sql = format!(
            "SELECT {TASK_COLUMNS} FROM kanban_tasks
             WHERE status = 'waiting' AND archived_at IS NULL ORDER BY created_at"
        );
        let rows: Vec<TaskRow> = sqlx::query_as(&sql).fetch_all(self.pool()).await?;
        rows.into_iter().map(TaskRow::into_task).collect()
    }

    /// 准入闸门：名额未满则置 `running`。返回是否放行（决策 98）。
    pub async fn try_admit(&self, task_id: &str, max_concurrent: usize) -> Result<bool> {
        let occupying = self.occupying_slots().await?;
        if occupying >= max_concurrent {
            return Ok(false);
        }
        let affected = sqlx::query(
            "UPDATE kanban_tasks SET status = 'running', updated_at = ?
             WHERE id = ? AND status = 'queued'",
        )
        .bind(ts(self.now()))
        .bind(task_id)
        .execute(self.pool())
        .await?
        .rows_affected();
        Ok(affected == 1)
    }

    // ─────────────────────────── executor 单执行者（决策 36 / 127）───────────────────────────

    /// DB 乐观锁：仅在 `executor_owner IS NULL` 时抢占。
    pub async fn try_claim_executor(&self, task_id: &str, owner: &str) -> Result<bool> {
        let affected = sqlx::query(
            "UPDATE kanban_tasks SET executor_owner = ?, updated_at = ?
             WHERE id = ? AND executor_owner IS NULL",
        )
        .bind(owner)
        .bind(ts(self.now()))
        .bind(task_id)
        .execute(self.pool())
        .await?
        .rows_affected();
        Ok(affected == 1)
    }

    pub async fn release_executor(&self, task_id: &str) -> Result<()> {
        sqlx::query("UPDATE kanban_tasks SET executor_owner = NULL, updated_at = ? WHERE id = ?")
            .bind(ts(self.now()))
            .bind(task_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// 启动时清理残留持有者（决策 127）：kill -9 后乐观锁不会自己释放。
    pub async fn clear_executor_owners(&self) -> Result<usize> {
        let affected = sqlx::query(
            "UPDATE kanban_tasks SET executor_owner = NULL WHERE executor_owner IS NOT NULL",
        )
        .execute(self.pool())
        .await?
        .rows_affected();
        Ok(affected as usize)
    }

    /// 启动恢复（决策 127 补全，主流程票 08）：把中断留下的孤儿 `running` 任务归队
    /// `queued`。调度器准入只认 `queued`（`try_admit`），光清 `executor_owner`
    /// 不归队的话任务会在重启后永久挂起。单机单进程（决策 127 同一前提），
    /// 启动瞬间不存在合法持有者，running 必为 kill -9 / 停机残留。
    /// 返回归队任务 id，供日志与测试断言。
    pub async fn requeue_running_tasks(&self) -> Result<Vec<String>> {
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM kanban_tasks WHERE status = 'running' AND archived_at IS NULL",
        )
        .fetch_all(self.pool())
        .await?;
        if ids.is_empty() {
            return Ok(ids);
        }
        sqlx::query("UPDATE kanban_tasks SET status = 'queued', updated_at = ? WHERE status = 'running' AND archived_at IS NULL")
            .bind(ts(self.now()))
            .execute(self.pool())
            .await?;
        Ok(ids)
    }

    /// 标记 / 取消 stalled（决策 34）。
    pub async fn set_stalled(&self, task_id: &str, stalled: bool) -> Result<()> {
        sqlx::query("UPDATE kanban_tasks SET stalled = ?, updated_at = ? WHERE id = ?")
            .bind(if stalled { 1 } else { 0 })
            .bind(ts(self.now()))
            .bind(task_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// 任务终态（done / failed / cancelled）。
    pub async fn mark_terminal(&self, task_id: &str, status: TaskStatus) -> Result<()> {
        debug_assert!(status.is_terminal());
        let now = self.now();
        sqlx::query(
            "UPDATE kanban_tasks SET status = ?, pending_reason_json = NULL, executor_owner = NULL, updated_at = ?
             WHERE id = ?",
        )
        .bind(status.as_str())
        .bind(ts(now))
        .bind(task_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// 看板列归属：按有无 pending 游标区分（决策 92）。
    pub async fn tasks_with_live_cursors(&self) -> Result<Vec<(Task, Vec<NodeCursor>)>> {
        let tasks = self
            .list_tasks(&TaskFilter {
                include_archived: false,
                ..Default::default()
            })
            .await?;
        let mut out = Vec::with_capacity(tasks.len());
        for task in tasks {
            let cursors = self.load_live_cursors(&task.id).await?;
            out.push((task, cursors));
        }
        Ok(out)
    }

    pub async fn task_updated_at(&self, task_id: &str) -> Result<Option<DateTime<Utc>>> {
        let raw: Option<String> =
            sqlx::query_scalar("SELECT updated_at FROM kanban_tasks WHERE id = ?")
                .bind(task_id)
                .fetch_optional(self.pool())
                .await?
                .flatten();
        raw.map(|s| parse_ts(&s)).transpose()
    }

    /// 该任务是否已完成全部依赖（决策 57：all done → queued）。
    pub async fn dependencies_satisfied(&self, task_id: &str) -> Result<DependencyState> {
        let deps = self.dependencies_of(task_id).await?;
        if deps.is_empty() {
            return Ok(DependencyState::Ready);
        }
        let mut all_done = true;
        let mut terminal_failures = Vec::new();
        let mut waiting = Vec::new();
        for dep_id in deps {
            match self.get_task(&dep_id).await {
                Ok(dep) => {
                    if !dep.status.is_terminal() {
                        all_done = false;
                        waiting.push(dep_id);
                    } else if dep.status != TaskStatus::Done {
                        terminal_failures.push(dep_id);
                    }
                }
                Err(_) => {
                    // 依赖已消失（归档删除等）——按未就绪处理
                    all_done = false;
                    waiting.push(dep_id);
                }
            }
        }
        if !terminal_failures.is_empty() {
            return Ok(DependencyState::Failed(terminal_failures));
        }
        if all_done {
            Ok(DependencyState::Ready)
        } else {
            Ok(DependencyState::Waiting(waiting))
        }
    }

    /// 依赖任务当前是否处于"正在跑"（dependency_failed 恢复用，决策 57）。
    pub async fn dependency_running(&self, task_id: &str) -> Result<bool> {
        for dep_id in self.dependencies_of(task_id).await? {
            if let Ok(dep) = self.get_task(&dep_id).await {
                if dep.status == TaskStatus::Running {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}

/// 依赖状态三态（决策 116）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencyState {
    Ready,
    Waiting(Vec<String>),
    Failed(Vec<String>),
}

/// 供 executor 恢复时判断游标是否指向 done。
pub fn cursor_reached_done(cursors: &[NodeCursor]) -> bool {
    !cursors.is_empty()
        && cursors
            .iter()
            .all(|c| c.stage == Stage::Done && c.status != crate::types::CursorStatus::Archived)
}
