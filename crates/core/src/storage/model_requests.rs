//! 模型请求台账（决策 231）：一次请求一行，**请求开始就写、`finished_at IS NULL` 即「在飞」**。
//!
//! 为什么它值一张表，而不是往 `kanban_node_runs` 加几列：一次 run 里有**多次**模型请求
//! （工具循环每轮一次、子代理自己还有一轮），而「现在在飞的是哪一个、收了多少字节、最后一次
//! 收到字节是什么时候」是 run 行回答不了的问题。2026-09-19 那次实测里，值班长采到了**正确的
//! 方法**（`sample` 连采两次、`pgrep` 排除 git、`lsof` 看到两条连接）却把证据记在**错的 run**
//! 名下——死因就是模型请求这一层没有任何身份（决策 230）。这张表补的就是那个身份。
//!
//! 三条不变式钉在这里，不散到调用方：
//!
//! 1. **落行与收场是两次独立的写入**：`begin_model_request` 在派发前落行，`finish_model_request`
//!    在收场后补全。中间**不能握着事务等模型**——一次流式调用可以跑几十分钟，而 `BEGIN IMMEDIATE`
//!    握着写锁等它，整库都写不进去。写法照 `kanban_node_commands` 的 `record_start` /
//!    `record_finish`（票 09 的同一形状）。
//! 2. **`run_id = 0` 是「没有 run 行」的哨兵**（值班长与项目分析都给 `run_id: 0`），入库前
//!    归一成 NULL：0 会撞外键，而 NULL 才是「没有归属」的忠实写法。
//! 3. **NULL ≠ 0**：`bytes_received` 为 NULL 表示这次调用**没拿到收场读数**（流半途断了、
//!    或调用方把 future 丢了），0 表示真的一个字节都没收到。决策 226③ 刚把「用 0 冒充读数」
//!    从 run 记账里挖掉，这张表不能原地再造一个。

use chrono::{DateTime, Utc};
use sqlx::FromRow;

use crate::storage::{parse_ts, ts, Store};
use crate::Result;

/// 一次请求的结束状态（决策 231 的四个终态 + 在飞）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelRequestStatus {
    /// 派发了、还没收场（`finished_at IS NULL` 的那一行）。
    Running,
    Ok,
    Error,
    /// 执行体按中止请求收口（决策 226）。
    Cancelled,
    /// 调用方在半途放弃（墙钟超时 / future 被丢掉）。
    Timeout,
}

impl ModelRequestStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ModelRequestStatus::Running => "running",
            ModelRequestStatus::Ok => "ok",
            ModelRequestStatus::Error => "error",
            ModelRequestStatus::Cancelled => "cancelled",
            ModelRequestStatus::Timeout => "timeout",
        }
    }

    /// 落库值 → 状态。非法值报错（它决定「在飞」这个读数成不成立）。
    pub fn parse(raw: &str) -> Result<Self> {
        Ok(match raw {
            "running" => ModelRequestStatus::Running,
            "ok" => ModelRequestStatus::Ok,
            "error" => ModelRequestStatus::Error,
            "cancelled" => ModelRequestStatus::Cancelled,
            "timeout" => ModelRequestStatus::Timeout,
            other => {
                return Err(crate::Error::Validation(format!(
                    "未知的模型请求状态：{other}"
                )))
            }
        })
    }

    pub fn is_terminal(self) -> bool {
        !matches!(self, ModelRequestStatus::Running)
    }
}

/// 新建一行的输入。归属三态各自可为 `None`（见模块头：流水线 run / 值班长班次 / 项目分析）。
#[derive(Debug, Clone)]
pub struct NewModelRequest {
    pub run_id: Option<i64>,
    pub session_id: Option<String>,
    pub task_id: Option<String>,
    pub agent_type: String,
    pub stage: String,
    pub node: String,
    pub attempt: u32,
}

/// 收场读数（决策 231 的「用量 + 收字节总量 + 最后一次收字节的时刻」）。
///
/// **每个字段都可空，且 `None` 与 `0` 是两件事**：用量是随流的最后一个 usage 事件到的，
/// 流半途断掉时它根本没到过——那时写 0 会把「没有读数」说成「一个 token 都没烧」，
/// 而 2026-09-19 那次实测里「一千万 prompt token 却记 0」正是把值班长带偏四轮的那个假读数。
#[derive(Debug, Clone, Default)]
pub struct ModelRequestUsage {
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub cache_read_tokens: Option<u32>,
    pub cache_write_tokens: Option<u32>,
    /// `None` = 没量到（不是「量到 0」）。
    pub bytes_received: Option<u64>,
    pub last_byte_at: Option<DateTime<Utc>>,
}

/// 一行模型请求（读出来的形状）。
#[derive(Debug, Clone)]
pub struct ModelRequest {
    pub id: i64,
    pub run_id: Option<i64>,
    pub session_id: Option<String>,
    pub task_id: Option<String>,
    pub agent_type: String,
    pub stage: String,
    pub node: String,
    pub attempt: u32,
    /// 同一次 run（或同一个班次）内的第几次请求，1 起。
    pub seq: i64,
    pub status: ModelRequestStatus,
    pub usage: ModelRequestUsage,
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

impl ModelRequest {
    /// 是否**此刻在飞**——这张表的现状读数就是它（决策 231）。
    pub fn in_flight(&self) -> bool {
        self.finished_at.is_none()
    }

    /// 已跑多久：收场了取实际时长，还在飞取「从派发到现在」。
    pub fn elapsed_ms(&self, now: DateTime<Utc>) -> u64 {
        let end = self.finished_at.unwrap_or(now);
        (end - self.started_at).num_milliseconds().max(0) as u64
    }
}

#[derive(Debug, FromRow)]
struct ModelRequestRow {
    id: i64,
    run_id: Option<i64>,
    session_id: Option<String>,
    task_id: Option<String>,
    agent_type: String,
    stage: String,
    node: String,
    attempt: i64,
    seq: i64,
    status: String,
    prompt_tokens: Option<i64>,
    completion_tokens: Option<i64>,
    cache_read_tokens: Option<i64>,
    cache_write_tokens: Option<i64>,
    bytes_received: Option<i64>,
    last_byte_at: Option<String>,
    error: Option<String>,
    started_at: String,
    finished_at: Option<String>,
}

impl ModelRequestRow {
    fn into_request(self) -> Result<ModelRequest> {
        Ok(ModelRequest {
            id: self.id,
            run_id: self.run_id,
            session_id: self.session_id,
            task_id: self.task_id,
            agent_type: self.agent_type,
            stage: self.stage,
            node: self.node,
            attempt: self.attempt as u32,
            seq: self.seq,
            status: ModelRequestStatus::parse(&self.status)?,
            usage: ModelRequestUsage {
                prompt_tokens: self.prompt_tokens.map(|v| v.max(0) as u32),
                completion_tokens: self.completion_tokens.map(|v| v.max(0) as u32),
                cache_read_tokens: self.cache_read_tokens.map(|v| v.max(0) as u32),
                cache_write_tokens: self.cache_write_tokens.map(|v| v.max(0) as u32),
                bytes_received: self.bytes_received.map(|v| v.max(0) as u64),
                last_byte_at: self.last_byte_at.map(|s| parse_ts(&s)).transpose()?,
            },
            error: self.error,
            started_at: parse_ts(&self.started_at)?,
            finished_at: self.finished_at.map(|s| parse_ts(&s)).transpose()?,
        })
    }
}

const MODEL_REQUEST_COLUMNS: &str = "id, run_id, session_id, task_id, agent_type, stage, node, \
     attempt, seq, status, prompt_tokens, completion_tokens, cache_read_tokens, cache_write_tokens, \
     bytes_received, last_byte_at, error, started_at, finished_at";

impl Store {
    /// 派发前落一行（`status = running`、`finished_at NULL`），返回它的 id。
    ///
    /// 用量列一并写 NULL（不是 0）：还没收场就是**还没量到**——在飞的行上写 0 会把
    /// 「这一轮还没结束」读成「一个 token 都没烧」，而决策 226③ 修的正是这一类假读数。
    /// 序号在同一归属内自增，且与插入**同一个写事务**——两个并发请求（不同 run）各算各的，
    /// 同一个 run 内的请求是串行的（工具循环一次只 await 一个），故这里不需要额外去重。
    pub async fn begin_model_request(&self, new: &NewModelRequest) -> Result<i64> {
        let mut tx = self.begin_write().await?;
        let seq: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM kanban_model_requests
             WHERE run_id IS ? AND session_id IS ?",
        )
        .bind(new.run_id)
        .bind(new.session_id.as_deref())
        .fetch_one(&mut *tx)
        .await?;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO kanban_model_requests
             (run_id, session_id, task_id, agent_type, stage, node, attempt, seq, status,
              prompt_tokens, completion_tokens, cache_read_tokens, cache_write_tokens,
              bytes_received, last_byte_at, error, started_at, finished_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'running',
                     NULL, NULL, NULL, NULL, NULL, NULL, NULL, ?, NULL)
             RETURNING id",
        )
        .bind(new.run_id)
        .bind(new.session_id.as_deref())
        .bind(new.task_id.as_deref())
        .bind(&new.agent_type)
        .bind(&new.stage)
        .bind(&new.node)
        .bind(new.attempt as i64)
        .bind(seq)
        .bind(ts(self.now()))
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(id)
    }

    /// 收场：补全状态、用量、收字节总量与最后一次收字节的时刻。
    ///
    /// 按值写（不是 `COALESCE`）：终态由**收场这一方**定，重复调用只会覆盖同一次调用的读数；
    /// 但一个 `id` 只由它的派发方收场一次，故不存在「谁说了算」的竞争。
    pub async fn finish_model_request(
        &self,
        id: i64,
        status: ModelRequestStatus,
        usage: &ModelRequestUsage,
        error: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE kanban_model_requests
             SET status = ?, prompt_tokens = ?, completion_tokens = ?, cache_read_tokens = ?,
                 cache_write_tokens = ?, bytes_received = ?, last_byte_at = ?, error = ?,
                 finished_at = ?
             WHERE id = ?",
        )
        .bind(status.as_str())
        .bind(usage.prompt_tokens.map(|v| v as i64))
        .bind(usage.completion_tokens.map(|v| v as i64))
        .bind(usage.cache_read_tokens.map(|v| v as i64))
        .bind(usage.cache_write_tokens.map(|v| v as i64))
        .bind(usage.bytes_received.map(|v| v as i64))
        .bind(usage.last_byte_at.map(ts))
        .bind(error)
        .bind(ts(self.now()))
        .bind(id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// 此刻在飞的请求（`finished_at IS NULL`，按派发顺序）。
    pub async fn inflight_model_requests(&self, limit: usize) -> Result<Vec<ModelRequest>> {
        let rows: Vec<ModelRequestRow> = sqlx::query_as(&format!(
            "SELECT {MODEL_REQUEST_COLUMNS} FROM kanban_model_requests
             WHERE finished_at IS NULL ORDER BY id LIMIT ?"
        ))
        .bind(limit as i64)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter()
            .map(ModelRequestRow::into_request)
            .collect()
    }

    /// 某一次 run 的请求（**最新在前**）。`run_id` 为 0 的调用（值班长 / 项目分析）没有 run，
    /// 用 [`Self::model_requests_for_session`] 那条路。
    pub async fn model_requests_for_run(
        &self,
        run_id: i64,
        limit: usize,
    ) -> Result<Vec<ModelRequest>> {
        let rows: Vec<ModelRequestRow> = sqlx::query_as(&format!(
            "SELECT {MODEL_REQUEST_COLUMNS} FROM kanban_model_requests
             WHERE run_id = ? ORDER BY seq DESC LIMIT ?"
        ))
        .bind(run_id)
        .bind(limit as i64)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter()
            .map(ModelRequestRow::into_request)
            .collect()
    }

    /// 一个任务下**所有 run** 的请求（最新在前）。诊断包用它一次取完，避免逐 run 查。
    pub async fn model_requests_for_task(
        &self,
        task_id: &str,
        limit: usize,
    ) -> Result<Vec<ModelRequest>> {
        let rows: Vec<ModelRequestRow> = sqlx::query_as(&format!(
            "SELECT {MODEL_REQUEST_COLUMNS} FROM kanban_model_requests
             WHERE task_id = ? ORDER BY id DESC LIMIT ?"
        ))
        .bind(task_id)
        .bind(limit as i64)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter()
            .map(ModelRequestRow::into_request)
            .collect()
    }

    /// 一个班次的请求（值班长没有 run 行，归属靠它）。
    pub async fn model_requests_for_session(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<ModelRequest>> {
        let rows: Vec<ModelRequestRow> = sqlx::query_as(&format!(
            "SELECT {MODEL_REQUEST_COLUMNS} FROM kanban_model_requests
             WHERE session_id = ? ORDER BY id DESC LIMIT ?"
        ))
        .bind(session_id)
        .bind(limit as i64)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter()
            .map(ModelRequestRow::into_request)
            .collect()
    }

    /// 启动时把**上一个实例留下的在飞请求**收成终态，返回收了几行。
    ///
    /// 为什么必须有这一步：`finished_at IS NULL` 是这张表唯一的「在飞」读数，而进程被 kill
    /// 时那些行的收场写入永远不会发生——不收口的话，一个**死掉的**请求会永远以「在飞」的姿态
    /// 出现在诊断包里，正是决策 226③ 要根除的那种失真读数（只不过方向相反：不是假装 0，
    /// 是假装还活着）。标 `timeout` 而不是删行：它在时间线上真发生过。
    pub async fn orphan_inflight_model_requests(&self, note: &str) -> Result<u64> {
        let affected = sqlx::query(
            "UPDATE kanban_model_requests
             SET status = 'timeout', error = COALESCE(error, ?), finished_at = ?
             WHERE finished_at IS NULL",
        )
        .bind(note)
        .bind(ts(self.now()))
        .execute(self.pool())
        .await?
        .rows_affected();
        Ok(affected)
    }
}
