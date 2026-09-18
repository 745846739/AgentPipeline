//! 值班长待办（决策 209③ / 票 05）：调度器 tick 的发现**落表**，值守轮消费。
//!
//! 三条规则钉在这里，不散到调用方：
//!
//! 1. **一事件一行，同一件事只写一行**：去重键是 `(task_id, kind, occurred_at)`。
//!    `occurred_at` 是事件**发生**的时刻（pending 游标的 `updated_at` / 终态 run 的
//!    `finished_at` / 任务行的 `updated_at`），不是「这一 tick 的时刻」——否则每 10 秒
//!    就会给同一次 pending 写一行，唤醒会被自己的重试刷屏。
//! 2. **`consumed_at IS NULL` 即未处理**：值守轮处理完才置位（票 06）。所以「这条我
//!    处理过没有」有一个可查的答案，而不用去猜日志。
//! 3. **保留期与对讲台其余各表同口径**：年龄清理在每小时维护作业里。
//!
//! 事件类别是稳定标识（落库列），前端 / 播报 / 测试都按它判，不按文案。

use chrono::{DateTime, Utc};
use sqlx::FromRow;

use crate::storage::{parse_ts, ts, Store};
use crate::Result;

/// 待办事件的类别（决策 209② 的事件清单，逐条一个）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttentionKind {
    /// 任务转 pending（任何 pending 原因）。
    TaskPending,
    /// 重试耗尽的 pending（`PendingKind::RetryExhausted` 的子集，单列便于播报分级）。
    RetryExhausted,
    /// 上下文溢出的 pending。
    ContextOverflow,
    /// 闸门失败（develop 闸门 / `MergeResult.gate_failure_kind`）。
    GateFailure,
    /// **同一任务在窗口内再次 pending**：自动修复没治好（§4.9 止损的判据之一）。
    RepeatedPending,
    /// **调度器处置未生效**：run 已是终态而游标仍 active、任务仍 running。
    /// 2026-09-17 实测倒出来的那一类，此前零信号。
    SchedulerNoEffect,
    /// owner 持有超时：任务 running 且 `executor_owner` 非空，却长时间没有 run 心跳。
    OwnerStuck,
    /// 停滞提醒：pending 超过 `pending_reminder_hours` 仍无人处理（票 05 顺手修掉的
    /// 「提醒只存内存、从不外发」那个缺陷的落点）。
    TaskStale,
    /// 任务转 done（收尾播报）。
    TaskDone,
    /// 慢跑（3×P90，决策 66 的自适应告警）。
    SlowRun,
}

impl AttentionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AttentionKind::TaskPending => "task_pending",
            AttentionKind::RetryExhausted => "retry_exhausted",
            AttentionKind::ContextOverflow => "context_overflow",
            AttentionKind::GateFailure => "gate_failure",
            AttentionKind::RepeatedPending => "repeated_pending",
            AttentionKind::SchedulerNoEffect => "scheduler_no_effect",
            AttentionKind::OwnerStuck => "owner_stuck",
            AttentionKind::TaskStale => "task_stale",
            AttentionKind::TaskDone => "task_done",
            AttentionKind::SlowRun => "slow_run",
        }
    }

    /// 这一类要不要**唤醒**值守轮（§2.1 那条纪律的载体）。
    ///
    /// 唤醒是花钱的，且是在没人在场的时候花，所以判据是「需不需要有人管」而不是
    /// 「值不值得知道」。慢跑是自适应告警（决策 66）：**只播报，不唤醒**——它没有
    /// 可操作的动作，夜里为此叫一次模型是纯开销。它仍在表里，于是下一次因为别的原因
    /// 醒着时，它照样是态势的一部分。
    pub fn wakes(self) -> bool {
        !matches!(self, AttentionKind::SlowRun)
    }

    /// 落库值 → 类别。非法值报错（它是执行语义：决定唤不唤醒、播报什么）。
    pub fn parse(raw: &str) -> Result<Self> {
        Ok(match raw {
            "task_pending" => AttentionKind::TaskPending,
            "retry_exhausted" => AttentionKind::RetryExhausted,
            "context_overflow" => AttentionKind::ContextOverflow,
            "gate_failure" => AttentionKind::GateFailure,
            "repeated_pending" => AttentionKind::RepeatedPending,
            "scheduler_no_effect" => AttentionKind::SchedulerNoEffect,
            "owner_stuck" => AttentionKind::OwnerStuck,
            "task_stale" => AttentionKind::TaskStale,
            "task_done" => AttentionKind::TaskDone,
            "slow_run" => AttentionKind::SlowRun,
            other => {
                return Err(crate::Error::Validation(format!(
                    "未知的值班长待办类别：{other}"
                )))
            }
        })
    }
}

/// 一次唤醒的结局（票 07 的唤醒账）。三态互斥：`capped` 的那次**没花钱**，
/// 而 `silent` 那次花了钱但没播报——把两者混成一个数，全局上限就形同虚设。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchWakeOutcome {
    /// 播报了一轮。
    Broadcast,
    /// 醒了、判定无需处理（不落会话行，但花了 token）。
    Silent,
    /// 触顶，没醒（`event_count` 是那批未播报的待办条数）。
    Capped,
}

impl WatchWakeOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            WatchWakeOutcome::Broadcast => "broadcast",
            WatchWakeOutcome::Silent => "silent",
            WatchWakeOutcome::Capped => "capped",
        }
    }
}

/// 一行待办。
#[derive(Debug, Clone, PartialEq)]
pub struct AttentionItem {
    pub id: i64,
    pub task_id: String,
    pub kind: AttentionKind,
    /// 事件**发生**的时刻（去重键的一半），不是写入时刻。
    pub occurred_at: DateTime<Utc>,
    pub detail_json: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub consumed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, FromRow)]
struct AttentionRow {
    id: i64,
    task_id: String,
    kind: String,
    occurred_at: String,
    detail_json: Option<String>,
    created_at: String,
    consumed_at: Option<String>,
}

impl AttentionRow {
    fn into_item(self) -> Result<AttentionItem> {
        Ok(AttentionItem {
            id: self.id,
            task_id: self.task_id,
            kind: AttentionKind::parse(&self.kind)?,
            occurred_at: parse_ts(&self.occurred_at)?,
            detail_json: self
                .detail_json
                .map(|s| serde_json::from_str(&s))
                .transpose()?,
            created_at: parse_ts(&self.created_at)?,
            consumed_at: self.consumed_at.map(|s| parse_ts(&s)).transpose()?,
        })
    }
}

const ATTENTION_COLUMNS: &str =
    "id, task_id, kind, occurred_at, detail_json, created_at, consumed_at";

impl Store {
    /// 记一条待办。同一 `(task_id, kind, occurred_at)` 已存在时**什么都不做**。
    ///
    /// 返回 `true` 表示这次真的写进去了一行（调用方据此计数「本 tick 新发现了几件事」）。
    pub async fn note_attention(
        &self,
        task_id: &str,
        kind: AttentionKind,
        occurred_at: DateTime<Utc>,
        detail: Option<&serde_json::Value>,
    ) -> Result<bool> {
        let inserted = sqlx::query(
            "INSERT INTO kanban_foreman_attention
             (task_id, kind, occurred_at, detail_json, created_at, consumed_at)
             VALUES (?, ?, ?, ?, ?, NULL)
             ON CONFLICT(task_id, kind, occurred_at) DO NOTHING",
        )
        .bind(task_id)
        .bind(kind.as_str())
        .bind(ts(occurred_at))
        .bind(detail.map(|d| d.to_string()))
        .bind(ts(self.now()))
        .execute(self.pool())
        .await?
        .rows_affected()
            > 0;
        Ok(inserted)
    }

    /// 未消费的待办（`id` 升序 = 事件发生顺序）。
    pub async fn open_attention(&self, limit: usize) -> Result<Vec<AttentionItem>> {
        let rows: Vec<AttentionRow> = sqlx::query_as(&format!(
            "SELECT {ATTENTION_COLUMNS} FROM kanban_foreman_attention
             WHERE consumed_at IS NULL ORDER BY id LIMIT ?"
        ))
        .bind(limit as i64)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter().map(AttentionRow::into_item).collect()
    }

    /// 消费这些待办（值守轮处理完之后调用）。返回被置位的行数。
    pub async fn consume_attention(&self, ids: &[i64]) -> Result<u64> {
        let mut affected = 0;
        for id in ids {
            affected += sqlx::query(
                "UPDATE kanban_foreman_attention SET consumed_at = ?
                 WHERE id = ? AND consumed_at IS NULL",
            )
            .bind(ts(self.now()))
            .bind(id)
            .execute(self.pool())
            .await?
            .rows_affected();
        }
        Ok(affected)
    }

    /// 窗口内某一类的条数（**事件数**，不是行数：去重键含 `occurred_at`）。
    ///
    /// §4.9 的「同一任务 30 分钟内转 pending 超过一次」用它判。
    pub async fn count_attention_since(
        &self,
        task_id: &str,
        kind: AttentionKind,
        since: DateTime<Utc>,
    ) -> Result<usize> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM kanban_foreman_attention
             WHERE task_id = ? AND kind = ? AND occurred_at >= ?",
        )
        .bind(task_id)
        .bind(kind.as_str())
        .bind(ts(since))
        .fetch_one(self.pool())
        .await?;
        Ok(count as usize)
    }

    /// 窗口内**已被消费**的条数（按任务）。
    ///
    /// 票 07 的同任务冷却用它判：某任务刚被处理过（有已消费的待办），它新来的事件就
    /// **不单独唤醒**——留在表里，等冷却到期后与那时的事件合并播报。
    pub async fn count_consumed_attention_since(
        &self,
        task_id: &str,
        since: DateTime<Utc>,
    ) -> Result<usize> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM kanban_foreman_attention
             WHERE task_id = ? AND consumed_at IS NOT NULL AND consumed_at >= ?",
        )
        .bind(task_id)
        .bind(ts(since))
        .fetch_one(self.pool())
        .await?;
        Ok(count as usize)
    }

    // ─────────────────────── 唤醒账（决策 209⑤ / 票 07）───────────────────────

    /// 记一次唤醒（或一次触顶）。返回值是这一行的 id。
    pub async fn record_watch_wake(
        &self,
        session_id: Option<&str>,
        outcome: WatchWakeOutcome,
        event_count: usize,
        prompt_tokens: u32,
        completion_tokens: u32,
    ) -> Result<i64> {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO kanban_foreman_watch_wakes
             (session_id, outcome, event_count, prompt_tokens, completion_tokens, created_at)
             VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(session_id)
        .bind(outcome.as_str())
        .bind(event_count as i64)
        .bind(prompt_tokens as i64)
        .bind(completion_tokens as i64)
        .bind(ts(self.now()))
        .fetch_one(self.pool())
        .await?;
        Ok(id)
    }

    /// 窗口内**真的醒过**几次（触顶那一种不算——它没花钱）。
    pub async fn count_watch_wakes_since(&self, since: DateTime<Utc>) -> Result<usize> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM kanban_foreman_watch_wakes
             WHERE outcome IN ('broadcast', 'silent') AND created_at >= ?",
        )
        .bind(ts(since))
        .fetch_one(self.pool())
        .await?;
        Ok(count as usize)
    }

    /// 窗口内记过几次某种结果（触顶提示「同一小时只记一次」用它）。
    pub async fn count_watch_wakes_with(
        &self,
        outcome: WatchWakeOutcome,
        since: DateTime<Utc>,
    ) -> Result<usize> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM kanban_foreman_watch_wakes
             WHERE outcome = ? AND created_at >= ?",
        )
        .bind(outcome.as_str())
        .bind(ts(since))
        .fetch_one(self.pool())
        .await?;
        Ok(count as usize)
    }

    /// 「这段时间值守花了多少」：`(醒过的次数, prompt token, completion token)`。
    ///
    /// 与人的回话分开统计——这是票 07 的最后一格：同一张账上答不出「这周谁花的钱」。
    pub async fn watch_cost_since(&self, since: DateTime<Utc>) -> Result<(usize, u64, u64)> {
        let row: (i64, i64, i64) = sqlx::query_as(
            "SELECT COUNT(*), COALESCE(SUM(prompt_tokens), 0), COALESCE(SUM(completion_tokens), 0)
             FROM kanban_foreman_watch_wakes
             WHERE outcome IN ('broadcast', 'silent') AND created_at >= ?",
        )
        .bind(ts(since))
        .fetch_one(self.pool())
        .await?;
        Ok((row.0 as usize, row.1 as u64, row.2 as u64))
    }

    /// 唤醒账的年龄清理（与其余各表同口径）。
    pub async fn purge_watch_wakes(&self, cutoff: DateTime<Utc>) -> Result<u64> {
        let affected = sqlx::query("DELETE FROM kanban_foreman_watch_wakes WHERE created_at < ?")
            .bind(ts(cutoff))
            .execute(self.pool())
            .await?
            .rows_affected();
        Ok(affected)
    }

    /// 年龄清理：删掉早于 `cutoff` 的行（含已消费的）。返回删掉的行数。
    ///
    /// 与 `conversation_retention_days` 同口径——对讲台不另设一套保留期。
    pub async fn purge_attention(&self, cutoff: DateTime<Utc>) -> Result<u64> {
        let affected = sqlx::query("DELETE FROM kanban_foreman_attention WHERE created_at < ?")
            .bind(ts(cutoff))
            .execute(self.pool())
            .await?
            .rows_affected();
        Ok(affected)
    }
}
