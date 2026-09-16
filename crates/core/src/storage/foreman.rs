//! 值班长会话存储（决策 176 / 182，票 01 / 05）。
//!
//! **独立于 `kanban_node_runs` / `kanban_node_conversations`**：那两张表的归属是
//! 「任务 / 项目恰好一个非空」（迁移 0004 的 CHECK），而值班长是**本机夜班级**的角色
//! ——没有任务，首启时也可以没有项目。放宽那条 CHECK 会让阶段聚合混进无阶段行，
//! 代价大于收益（决策 182）。理由与取舍写在迁移 0005 的头部。
//!
//! **不进 `total_tokens` / `total_calls`**：那两个字段的口径是「流水线执行 + 伪阶段」。
//! 对话自身的 token 由本表承载，对讲台据此自报「本次会话 N tok」（票 05）。

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::FromRow;

use super::{parse_ts, ts, Store};
use crate::Result;

/// `role` 的两个取值（与迁移 0005 的列注释同源）。
pub const FOREMAN_ROLE_USER: &str = "user";
pub const FOREMAN_ROLE_ASSISTANT: &str = "assistant";

/// 一行值班长会话。
#[derive(Debug, Clone, PartialEq)]
pub struct ForemanMessage {
    pub id: i64,
    pub role: String,
    pub content: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    /// 该轮注入的夜班态势快照（审计：它当时看到的是这份读数）。用户消息为 `None`。
    pub briefing_json: Option<Value>,
    /// 该轮调用过的只读工具痕迹（票 05）。无工具调用时为 `None`。
    pub traces_json: Option<Value>,
    pub created_at: DateTime<Utc>,
}

/// 待写入的一行（`id` / `created_at` 由存储层给，不接调用方的时钟）。
#[derive(Debug, Clone)]
pub struct NewForemanMessage {
    pub role: String,
    pub content: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub briefing_json: Option<Value>,
    pub traces_json: Option<Value>,
}

impl NewForemanMessage {
    /// 值班经理说的话：无读数、无快照（它还没被处理）。
    pub fn user(content: impl Into<String>) -> Self {
        NewForemanMessage {
            role: FOREMAN_ROLE_USER.to_string(),
            content: content.into(),
            prompt_tokens: 0,
            completion_tokens: 0,
            briefing_json: None,
            traces_json: None,
        }
    }
}

#[derive(FromRow)]
struct ForemanMessageRow {
    id: i64,
    role: String,
    content: String,
    prompt_tokens: i64,
    completion_tokens: i64,
    briefing_json: Option<String>,
    traces_json: Option<String>,
    created_at: String,
}

impl ForemanMessageRow {
    fn into_message(self) -> Result<ForemanMessage> {
        Ok(ForemanMessage {
            id: self.id,
            // role 是观测类字段：非法值不该打垮整个查询（storage/mod.rs 的 Q6 分类），
            // 但它同时决定界面把这句话摆在谁那一侧，故不兜底成别的角色——
            // 原样带出去，前端按 `=== "user"` 判定，不认识的值落到「值班长」一侧。
            role: self.role,
            content: self.content,
            prompt_tokens: self.prompt_tokens.max(0) as u32,
            completion_tokens: self.completion_tokens.max(0) as u32,
            briefing_json: self.briefing_json.as_deref().map(parse_json).transpose()?,
            traces_json: self.traces_json.as_deref().map(parse_json).transpose()?,
            created_at: parse_ts(&self.created_at)?,
        })
    }
}

/// 审计 JSON 列解析。损坏即报错——它与 `pending_reason` 同类：决定「依据什么」这一
/// 审计结论，静默兜底会让页面显示一份从未存在过的快照。
fn parse_json(raw: &str) -> Result<Value> {
    Ok(serde_json::from_str(raw)?)
}

const FOREMAN_COLUMNS: &str = "id, role, content, prompt_tokens, completion_tokens, \
                               briefing_json, traces_json, created_at";

impl Store {
    /// 追加一行会话，返回其 id。
    pub async fn append_foreman_message(&self, msg: NewForemanMessage) -> Result<i64> {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO kanban_foreman_messages
             (role, content, prompt_tokens, completion_tokens, briefing_json, traces_json, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(&msg.role)
        .bind(&msg.content)
        .bind(msg.prompt_tokens as i64)
        .bind(msg.completion_tokens as i64)
        .bind(msg.briefing_json.as_ref().map(Value::to_string))
        .bind(msg.traces_json.as_ref().map(Value::to_string))
        .bind(ts(self.now()))
        .fetch_one(self.pool())
        .await?;
        Ok(id)
    }

    /// 最近 `limit` 条，按 id **升序**返回（时间线顺序，与界面一致）。
    ///
    /// 由新往旧取、再由旧往新排：`LIMIT` 必须作用在最新的那一端，否则一段长对话
    /// 会永远只显示最早几句。`limit = 0` 返回空表（不当作「不限」）。
    pub async fn list_foreman_messages(&self, limit: usize) -> Result<Vec<ForemanMessage>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let sql = format!(
            "SELECT {FOREMAN_COLUMNS} FROM (
                 SELECT {FOREMAN_COLUMNS} FROM kanban_foreman_messages
                 ORDER BY id DESC LIMIT ?
             ) ORDER BY id ASC"
        );
        let rows: Vec<ForemanMessageRow> = sqlx::query_as(&sql)
            .bind(limit as i64)
            .fetch_all(self.pool())
            .await?;
        rows.into_iter()
            .map(ForemanMessageRow::into_message)
            .collect()
    }

    /// 本会话合计 `(total_tokens, total_calls)`。
    ///
    /// `total_calls` 数的是**值班长的回话次数**（assistant 行），不是工具往返次数
    /// ——对讲台要报的是「聊了几轮」，与全局指标的 `total_calls`（run 行数）不是同一个量。
    pub async fn foreman_session_totals(&self) -> Result<(u64, u64)> {
        let (tokens, calls): (i64, i64) = sqlx::query_as(
            "SELECT COALESCE(SUM(prompt_tokens + completion_tokens), 0),
                    COALESCE(SUM(CASE WHEN role = ? THEN 1 ELSE 0 END), 0)
             FROM kanban_foreman_messages",
        )
        .bind(FOREMAN_ROLE_ASSISTANT)
        .fetch_one(self.pool())
        .await?;
        Ok((tokens.max(0) as u64, calls.max(0) as u64))
    }

    /// 清掉 `cutoff` 之前的会话行（票 05：与 `conversation_retention_days` 同口径）。
    ///
    /// 与 `kanban_node_conversations` 的清理**不同**，这里不按任务终态过滤——值班长
    /// 对话不挂任务，没有「任务还没结束所以先留着」这一说；年龄是唯一判据。
    pub async fn purge_foreman_messages(&self, cutoff: DateTime<Utc>) -> Result<usize> {
        let purged = sqlx::query("DELETE FROM kanban_foreman_messages WHERE created_at < ?")
            .bind(ts(cutoff))
            .execute(self.pool())
            .await?
            .rows_affected();
        Ok(purged as usize)
    }
}
