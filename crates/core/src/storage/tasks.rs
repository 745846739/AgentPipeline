//! 任务 / 依赖 / 并发准入（决策 21 / 36 / 57 / 98 / 116 / 117 / 127）。

use chrono::{DateTime, Utc};
use sqlx::FromRow;

use super::{decode_node, decode_pending, decode_stage, encode_pending, parse_ts, ts, Store};
use crate::pipeline::cursor::{focus_cursor, project_pending_reason, project_task_status};
use crate::types::{NodeCursor, PendingKind, ReviewMode, Stage, Task, TaskStatus};
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
    stewardship_json: Option<String>,
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
            stewardship: self
                .stewardship_json
                .map(|s| serde_json::from_str(&s))
                .transpose()?,
            created_at: parse_ts(&self.created_at)?,
            updated_at: parse_ts(&self.updated_at)?,
        })
    }
}

const TASK_COLUMNS: &str = "id, project_id, title, description, status, current_stage, current_node, \
     validate_attempts, pending_reason_json, worktree_path, branch_name, total_tokens, total_calls, \
     review_mode, model_override, archived_at, stalled, executor_owner, stewardship_json,
     created_at, updated_at";

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
              executor_owner, stewardship_json, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, 'init', 'execute', 0, NULL, NULL, NULL, 0, 0, ?, ?, NULL, 0,
                     NULL, NULL, ?, ?)",
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

    /// 换模型 + **解除 `context_overflow` 的 pending**（决策 105 / 205，票 04）。
    ///
    /// 为什么换模型要顺手解除 pending：`context_overflow` 的动作集里本来就有「更换长上下文模型」
    /// 这一颗钮（决策 105），而人按下它表达的正是「换一次再试」。此前这个端点只写字段、
    /// 不清 pending——界面上按下去什么都不变，**给了出口但门没开**。
    ///
    /// 两处刻意的取舍：
    ///
    /// - **只对 `context_overflow` 解除**。这个端点在任何时候都可被调用，而别的 pending
    ///   （合入审批 / 人工评审）等的是另一种决定——顺手替人清掉，会让他永远等不到那个决定。
    /// - **三件事一个事务**：写字段 / 清 pending / 任务回 `queued`（`try_admit` 只认 queued，
    ///   这是任务重新被调度器接走的前提）。分开做会留下「游标已 active、任务还是 pending」的
    ///   中间态，而那种状态下没有任何东西会再推它一把。
    ///
    /// 返回值是**解除掉的 pending 游标数**（0 = 只是换了模型）。
    pub async fn apply_model_override(&self, task_id: &str, provider_id: &str) -> Result<usize> {
        let mut tx = self.begin_write().await?;
        let now = ts(self.now());
        sqlx::query("UPDATE kanban_tasks SET model_override = ?, updated_at = ? WHERE id = ?")
            .bind(provider_id)
            .bind(&now)
            .bind(task_id)
            .execute(&mut *tx)
            .await?;

        let rows: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT cursor_id, pending_reason_json FROM kanban_node_cursors
             WHERE task_id = ? AND status = 'pending'",
        )
        .bind(task_id)
        .fetch_all(&mut *tx)
        .await?;
        let mut cleared = 0usize;
        for (cursor_id, reason_json) in rows {
            let is_overflow = decode_pending(reason_json)?
                .map(|r| r.kind == PendingKind::ContextOverflow)
                .unwrap_or(false);
            if !is_overflow {
                continue;
            }
            self.clear_pending_in_tx(&mut tx, &cursor_id, None).await?;
            cleared += 1;
        }
        if cleared > 0 {
            // 任务的 pending 是焦点游标的投影，清完要一并刷掉那一列：
            // 留着它，界面会继续显示一条已经不存在的待办。
            sqlx::query(
                "UPDATE kanban_tasks SET status = 'queued', pending_reason_json = NULL,
                     updated_at = ? WHERE id = ?",
            )
            .bind(&now)
            .bind(task_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(cleared)
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

    /// 在任务的 pending 说明里加上一句「等修复合入」（决策 210⑨ / 票 11 的最后一格）。
    ///
    /// 为什么**不新增一个 `PendingKind`**（票面给了这个选项）：任务的状态是焦点游标 pending
    /// 原因的**投影**（[`Self::sync_task_projection`]），而 `PendingKind` 还参与
    /// `ResumeCause::classify`——换一个 kind 会把它**为什么停**（`retry_exhausted` /
    /// `timeout`…）连同 resume 的语义一起改掉，而修复并没有改变它停下来的原因。故只往那句
    /// 话里加一句：「为什么停」照旧在，人读到的是「为什么停 + 现在在等什么」。
    ///
    /// 没有 pending 游标时**什么都不做**并返回 `false`：一条没停下来的任务不该因为值班长顺手
    /// 提了个修复就显示成「在等」。返回值就是让调用方说得清「标记落没落上」。
    ///
    /// 幂等：note 已经在那句话里就不再追加（它会随 pending 一起留在库里，重复追加只会变噪声）。
    pub async fn note_task_awaiting_repair_merge(&self, task_id: &str, note: &str) -> Result<bool> {
        let live = self.load_live_cursors(task_id).await?;
        let Some(cursor) = live
            .iter()
            .filter(|c| c.pending_reason.is_some())
            .max_by_key(|c| c.updated_at)
        else {
            return Ok(false);
        };
        let Some(mut reason) = cursor.pending_reason.clone() else {
            return Ok(false);
        };
        if reason.message.contains(note) {
            return Ok(true);
        }
        reason.message = format!("{}；{note}", reason.message);
        sqlx::query(
            "UPDATE kanban_node_cursors SET pending_reason_json = ?, updated_at = ?
             WHERE cursor_id = ?",
        )
        .bind(encode_pending(&Some(reason)))
        .bind(ts(self.now()))
        .bind(&cursor.cursor_id)
        .execute(self.pool())
        .await?;
        // 投影要跟着刷：任务的 `pending_reason_json` 是这一列的副本，不刷的话界面上读到的
        // 还是上一句——那条任务仍然看不出它在等修复合入。
        self.sync_task_projection(task_id).await?;
        Ok(true)
    }

    /// 从 runs 重算 `total_tokens` / `total_calls`（口径见 metrics，决策 130 ②）。
    ///
    /// **读数没变就不写——但正在跑的任务除外。**
    ///
    /// `updated_at` 在 done 任务上是「刚完成」的新鲜度判据（`scheduler::note_discoveries`
    /// ④ 的「新鲜事」窗口，决策 209③），无条件 `updated_at = now` 会让小时级维护
    /// （`aggregate_node_metrics`）每小时把每个 done 任务的完成时刻洗成现在——
    /// `TaskDone` 待办的去重键含 `occurred_at`，新时刻必插新行，PWA「任务完成」推送
    /// 跟着每小时一条。指标聚合是**读数**，不是**事件**；读数没变（run 没多、用量没补记）
    /// 就不该刷新 `updated_at`。
    ///
    /// **为什么在跑的任务不走这条判据**：run 行的 token 要到收口才落库（轮内只走
    /// `touch_run_heartbeat`，只写 `last_activity_at`），故一个长节点在**库面上**的读数
    /// 不会变——这里若也跳过，详情页的 `updated_at` / `total_tokens` 会再次冻结成
    /// 45 分钟不动，正是决策 375 修掉的那个「活跃任务被误判卡死」。
    ///
    /// 判据收在**一条条件 `UPDATE`** 里（不是「先 SELECT 再判断」）：读与写之间不留竞态
    /// 窗口，`WHERE` 不成立时一行都不动、`updated_at` 也就不被改写。
    pub async fn refresh_task_totals(&self, task_id: &str) -> Result<()> {
        let runs = self.list_runs(task_id).await?;
        let tokens = crate::metrics::total_tokens(&runs) as i64;
        let calls = crate::metrics::total_calls(&runs) as i64;
        sqlx::query(
            "UPDATE kanban_tasks SET total_tokens = ?, total_calls = ?, updated_at = ?
             WHERE id = ? AND (total_tokens <> ? OR total_calls <> ? OR status = ?)",
        )
        .bind(tokens)
        .bind(calls)
        .bind(ts(self.now()))
        .bind(task_id)
        .bind(tokens)
        .bind(calls)
        .bind(TaskStatus::Running.as_str())
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

    /// 写任务级托管（决策 210① / 票 08）。`None` = 关掉（清空那一列）。
    ///
    /// 只在**任务级**动这一列：托管不是「模式的开关」，是「对这个任务的一次授权」。
    pub async fn set_stewardship(
        &self,
        task_id: &str,
        stewardship: Option<&crate::types::Stewardship>,
    ) -> Result<()> {
        sqlx::query("UPDATE kanban_tasks SET stewardship_json = ?, updated_at = ? WHERE id = ?")
            .bind(stewardship.map(serde_json::to_string).transpose()?)
            .bind(ts(self.now()))
            .bind(task_id)
            .execute(self.pool())
            .await?;
        Ok(())
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
