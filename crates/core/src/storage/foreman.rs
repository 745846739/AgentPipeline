//! 值班长会话存储（决策 176 / 182 / 204，票 01 / 05）。
//!
//! **独立于 `kanban_node_runs` / `kanban_node_conversations`**：那两张表的归属是
//! 「任务 / 项目恰好一个非空」（迁移 0004 的 CHECK），而值班长是**本机夜班级**的角色
//! ——没有任务，首启时也可以没有项目。放宽那条 CHECK 会让阶段聚合混进无阶段行，
//! 代价大于收益（决策 182）。理由与取舍写在迁移 0005 的头部。
//!
//! **会话（班次）是对话的容器**（决策 204）：一条长台账拆成一排可以新建 / 切换 / 重命名 /
//! 归档的班次。隔离的是**上下文**——喂给模型的 transcript 与页头的 token 合计都按会话过滤；
//! **不隔离权限**，也不隔离态势快照（`read_task` / `read_conversation` 与 `build_briefing`
//! 照旧全局。「换会话 ≠ 换看板」）。
//!
//! **不进 `total_tokens` / `total_calls`**：那两个字段的口径是「流水线执行 + 伪阶段」。
//! 对话自身的 token 由本表承载，对讲台据此自报「本次会话 N tok」——按会话过滤之后，
//! 那个数字第一次名副其实（决策 204⑤：此前它是对整张表求和，即「自建库以来的累计值」）。

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::FromRow;

use super::{parse_ts, ts, Store};
use crate::Result;

/// `role` 的三个取值（与迁移 0005 的列注释同源）。
pub const FOREMAN_ROLE_USER: &str = "user";
pub const FOREMAN_ROLE_ASSISTANT: &str = "assistant";
/// 操作台自己记的一轮（决策 207）：提议的执行结果。
///
/// **不能塞进 assistant**：那一侧是「值班长说的话」，而提议的结果恰恰是它**做不了**
/// 的那件事的下场。写成 assistant，模型下一轮读到自己的历史时会把这段当成自己说过的话
/// ——「你已经写入了 notes.md」这种声称正是人格里第一条纪律要挡的东西。
pub const FOREMAN_ROLE_SYSTEM: &str = "system";

/// 会话标题的字符上限（决策 204②：「前若干字」落到这个数）。
///
/// 24 是窄屏 chip 上还能认出这一班是干什么的长度——再长就被 CSS 截断了，
/// 而截断发生在渲染层意味着库里的标题与看到的标题不是一个东西。
pub const SESSION_TITLE_MAX_CHARS: usize = 24;

/// 还没说出第一句话的会话叫这个（决策 204②「没有首条用户消息时给一个中性标题」）。
pub const FOREMAN_SESSION_DEFAULT_TITLE: &str = "新班次";

/// 一次列出的会话上限（不分页，照 `SESSION_PAGE_LIMIT` 的精神）。
///
/// 班次是**按天/按班**的粒度，不是按调用——一个人一晚上不会新建 50 个班次。
/// 真撞上这个上限时，说明有人把它当成了日志列表用，而那件事该由台账去回答。
pub const FOREMAN_SESSION_LIST_LIMIT: usize = 50;

/// 会话标题：首条用户消息 → 一行标题（决策 204②）。
///
/// **纯函数**：迁移 0012 的回填标题在 SQL 里写了同一条规则（`TRIM` → `SUBSTR(...,1,24)`
/// → 超长补省略号），两处必须给出同一个答案——SQLite 的 `SUBSTR` / `LENGTH` 对 TEXT
/// 计数的是字符而不是字节，与这里 `.chars()` 的语义相同。改这里就要改那条 SQL。
pub fn session_title_from(first_message: Option<&str>) -> String {
    let trimmed = first_message.unwrap_or("").trim();
    if trimmed.is_empty() {
        return FOREMAN_SESSION_DEFAULT_TITLE.to_string();
    }
    let mut title: String = trimmed.chars().take(SESSION_TITLE_MAX_CHARS).collect();
    if trimmed.chars().count() > SESSION_TITLE_MAX_CHARS {
        title.push('…');
    }
    title
}

/// 一行会话（班次）。
#[derive(Debug, Clone, PartialEq)]
pub struct ForemanSession {
    pub id: String,
    pub title: String,
    pub created_at: DateTime<Utc>,
    /// 最近一次说话的时间。列表按它倒序（决策 204⑦）。
    pub last_active_at: DateTime<Utc>,
    /// 归档时间。归档 = 从列表里收起来，**不物理删除**，也不保护消息（决策 204⑦）。
    pub archived_at: Option<DateTime<Utc>>,
}

/// 一行值班长会话。
#[derive(Debug, Clone, PartialEq)]
pub struct ForemanMessage {
    pub id: i64,
    /// 所属会话。会话隔离上下文，故它是所有读取的**必填**入口参数，不是可选项。
    pub session_id: String,
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
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub briefing_json: Option<Value>,
    pub traces_json: Option<Value>,
}

impl NewForemanMessage {
    /// 操作台记的一轮（决策 207）：提议执行 / 拒绝的结果，或一次失败的理由。
    pub fn system(session_id: impl Into<String>, content: impl Into<String>) -> Self {
        NewForemanMessage {
            session_id: session_id.into(),
            role: FOREMAN_ROLE_SYSTEM.to_string(),
            content: content.into(),
            prompt_tokens: 0,
            completion_tokens: 0,
            briefing_json: None,
            traces_json: None,
        }
    }

    /// 值班长回的话：带读数与（可能的）工具痕迹。
    pub fn assistant(session_id: impl Into<String>, content: impl Into<String>) -> Self {
        NewForemanMessage {
            session_id: session_id.into(),
            role: FOREMAN_ROLE_ASSISTANT.to_string(),
            content: content.into(),
            prompt_tokens: 0,
            completion_tokens: 0,
            briefing_json: None,
            traces_json: None,
        }
    }
}

#[derive(FromRow)]
struct ForemanSessionRow {
    id: String,
    title: String,
    created_at: String,
    last_active_at: String,
    archived_at: Option<String>,
}

impl ForemanSessionRow {
    fn into_session(self) -> Result<ForemanSession> {
        Ok(ForemanSession {
            id: self.id,
            title: self.title,
            created_at: parse_ts(&self.created_at)?,
            last_active_at: parse_ts(&self.last_active_at)?,
            archived_at: self.archived_at.as_deref().map(parse_ts).transpose()?,
        })
    }
}

#[derive(FromRow)]
struct ForemanMessageRow {
    id: i64,
    session_id: String,
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
            session_id: self.session_id,
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

const FOREMAN_MESSAGE_COLUMNS: &str = "id, session_id, role, content, prompt_tokens, \
                                       completion_tokens, briefing_json, traces_json, created_at";

const FOREMAN_SESSION_COLUMNS: &str = "id, title, created_at, last_active_at, archived_at";

impl Store {
    // ─────────────────────────── 会话（班次）───────────────────────────

    /// 未归档的会话，按最近活动倒序、上限一条常量、不分页（决策 204⑦）。
    ///
    /// 归档的**不在这个列表里**——「从列表里收起来」就是归档的全部含义。
    /// 它们仍在库里，也仍能按 id 单独取到（`get_foreman_session`）。
    pub async fn list_foreman_sessions(&self) -> Result<Vec<ForemanSession>> {
        let sql = format!(
            "SELECT {FOREMAN_SESSION_COLUMNS} FROM kanban_foreman_sessions
             WHERE archived_at IS NULL
             ORDER BY last_active_at DESC, id DESC LIMIT ?"
        );
        let rows: Vec<ForemanSessionRow> = sqlx::query_as(&sql)
            .bind(FOREMAN_SESSION_LIST_LIMIT as i64)
            .fetch_all(self.pool())
            .await?;
        rows.into_iter()
            .map(ForemanSessionRow::into_session)
            .collect()
    }

    /// 按 id 取一个会话（含已归档的）。
    pub async fn get_foreman_session(&self, session_id: &str) -> Result<Option<ForemanSession>> {
        let sql =
            format!("SELECT {FOREMAN_SESSION_COLUMNS} FROM kanban_foreman_sessions WHERE id = ?");
        let row: Option<ForemanSessionRow> = sqlx::query_as(&sql)
            .bind(session_id)
            .fetch_optional(self.pool())
            .await?;
        row.map(ForemanSessionRow::into_session).transpose()
    }

    /// 最近活动的未归档会话（对讲台的默认落点）。
    pub async fn latest_foreman_session(&self) -> Result<Option<ForemanSession>> {
        let sql = format!(
            "SELECT {FOREMAN_SESSION_COLUMNS} FROM kanban_foreman_sessions
             WHERE archived_at IS NULL
             ORDER BY last_active_at DESC, id DESC LIMIT 1"
        );
        let row: Option<ForemanSessionRow> =
            sqlx::query_as(&sql).fetch_optional(self.pool()).await?;
        row.map(ForemanSessionRow::into_session).transpose()
    }

    /// 新建一个会话。`title` 为空（或全空白）时用中性标题。
    pub async fn create_foreman_session(&self, title: &str) -> Result<ForemanSession> {
        let now = self.now();
        let id = ulid::Ulid::new().to_string();
        let title = if title.trim().is_empty() {
            FOREMAN_SESSION_DEFAULT_TITLE.to_string()
        } else {
            title.trim().to_string()
        };
        sqlx::query(
            "INSERT INTO kanban_foreman_sessions (id, title, created_at, last_active_at, archived_at)
             VALUES (?, ?, ?, ?, NULL)",
        )
        .bind(&id)
        .bind(&title)
        .bind(ts(now))
        .bind(ts(now))
        .execute(self.pool())
        .await?;
        Ok(ForemanSession {
            id,
            title,
            created_at: now,
            last_active_at: now,
            archived_at: None,
        })
    }

    /// 改名。返回改后的会话；会话不存在时 `None`（路由据此回 404）。
    ///
    /// **已归档的会话也能改名**：归档不是封存，恢复要靠改名找回来这条不成立，
    /// 但拒绝改名也没有任何好处——两边都不解决「怎么把归档的弄回来」。
    pub async fn rename_foreman_session(
        &self,
        session_id: &str,
        title: &str,
    ) -> Result<Option<ForemanSession>> {
        let title = title.trim();
        if title.is_empty() {
            return Ok(None);
        }
        let affected = sqlx::query("UPDATE kanban_foreman_sessions SET title = ? WHERE id = ?")
            .bind(title)
            .bind(session_id)
            .execute(self.pool())
            .await?
            .rows_affected();
        if affected == 0 {
            return Ok(None);
        }
        self.get_foreman_session(session_id).await
    }

    /// 归档（置 `archived_at`）。幂等：重复归档不改第一次的时间戳。
    pub async fn archive_foreman_session(
        &self,
        session_id: &str,
    ) -> Result<Option<ForemanSession>> {
        sqlx::query(
            "UPDATE kanban_foreman_sessions SET archived_at = ?
             WHERE id = ? AND archived_at IS NULL",
        )
        .bind(ts(self.now()))
        .bind(session_id)
        .execute(self.pool())
        .await?;
        self.get_foreman_session(session_id).await
    }

    // ─────────────────────────── 会话行 ───────────────────────────

    /// 追加一行会话，返回其 id。同事务刷新所属会话的 `last_active_at`。
    pub async fn append_foreman_message(&self, msg: NewForemanMessage) -> Result<i64> {
        let mut tx = self.begin_write().await?;
        let now = self.now();
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO kanban_foreman_messages
             (session_id, role, content, prompt_tokens, completion_tokens, briefing_json,
              traces_json, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(&msg.session_id)
        .bind(&msg.role)
        .bind(&msg.content)
        .bind(msg.prompt_tokens as i64)
        .bind(msg.completion_tokens as i64)
        .bind(msg.briefing_json.as_ref().map(Value::to_string))
        .bind(msg.traces_json.as_ref().map(Value::to_string))
        .bind(ts(now))
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query("UPDATE kanban_foreman_sessions SET last_active_at = ? WHERE id = ?")
            .bind(ts(now))
            .bind(&msg.session_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(id)
    }

    /// 追加**值班经理**说的话，并在这是该会话第一句话时按它命名会话（决策 204②）。
    ///
    /// 命名与落库同事务：分两步做的话，中间崩掉会留下一个「有话但还叫新班次」的会话，
    /// 而那个状态没有任何东西会去修它。
    ///
    /// 判据是**该会话有没有过消息**，不是「标题是否等于中性标题」——后者把「人自己
    /// 把会话改名叫『新班次』」当成了「还没命名」，于是下一句话会把人的命名冲掉。
    pub async fn append_foreman_user_message(
        &self,
        session_id: &str,
        content: &str,
    ) -> Result<i64> {
        let mut tx = self.begin_write().await?;
        let before: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM kanban_foreman_messages WHERE session_id = ?")
                .bind(session_id)
                .fetch_one(&mut *tx)
                .await?;
        let now = self.now();
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO kanban_foreman_messages
             (session_id, role, content, prompt_tokens, completion_tokens, briefing_json,
              traces_json, created_at)
             VALUES (?, ?, ?, 0, 0, NULL, NULL, ?) RETURNING id",
        )
        .bind(session_id)
        .bind(FOREMAN_ROLE_USER)
        .bind(content)
        .bind(ts(now))
        .fetch_one(&mut *tx)
        .await?;
        if before == 0 {
            let title = session_title_from(Some(content));
            sqlx::query(
                "UPDATE kanban_foreman_sessions SET title = ?, last_active_at = ? WHERE id = ?",
            )
            .bind(&title)
            .bind(ts(now))
            .bind(session_id)
            .execute(&mut *tx)
            .await?;
        } else {
            sqlx::query("UPDATE kanban_foreman_sessions SET last_active_at = ? WHERE id = ?")
                .bind(ts(now))
                .bind(session_id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(id)
    }

    /// 某个会话最近 `limit` 条，按 id **升序**返回（时间线顺序，与界面一致）。
    ///
    /// 由新往旧取、再由旧往新排：`LIMIT` 必须作用在最新的那一端，否则一段长对话
    /// 会永远只显示最早几句。`limit = 0` 返回空表（不当作「不限」）。
    pub async fn list_foreman_messages(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<ForemanMessage>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let sql = format!(
            "SELECT {FOREMAN_MESSAGE_COLUMNS} FROM (
                 SELECT {FOREMAN_MESSAGE_COLUMNS} FROM kanban_foreman_messages
                 WHERE session_id = ?
                 ORDER BY id DESC LIMIT ?
             ) ORDER BY id ASC"
        );
        let rows: Vec<ForemanMessageRow> = sqlx::query_as(&sql)
            .bind(session_id)
            .bind(limit as i64)
            .fetch_all(self.pool())
            .await?;
        rows.into_iter()
            .map(ForemanMessageRow::into_message)
            .collect()
    }

    /// 本会话合计 `(total_tokens, total_calls)`（决策 204⑤：按会话过滤，不再对整张表求和）。
    ///
    /// `total_calls` 数的是**值班长的回话次数**（assistant 行），不是工具往返次数
    /// ——对讲台要报的是「聊了几轮」，与全局指标的 `total_calls`（run 行数）不是同一个量。
    pub async fn foreman_session_totals(&self, session_id: &str) -> Result<(u64, u64)> {
        let (tokens, calls): (i64, i64) = sqlx::query_as(
            "SELECT COALESCE(SUM(prompt_tokens + completion_tokens), 0),
                    COALESCE(SUM(CASE WHEN role = ? THEN 1 ELSE 0 END), 0)
             FROM kanban_foreman_messages WHERE session_id = ?",
        )
        .bind(FOREMAN_ROLE_ASSISTANT)
        .bind(session_id)
        .fetch_one(self.pool())
        .await?;
        Ok((tokens.max(0) as u64, calls.max(0) as u64))
    }

    /// 清掉 `cutoff` 之前的会话行（票 05：与 `conversation_retention_days` 同口径）。
    ///
    /// 与 `kanban_node_conversations` 的清理**不同**，这里不按任务终态过滤——值班长
    /// 对话不挂任务，没有「任务还没结束所以先留着」这一说；年龄是唯一判据。
    /// **归档不保护消息**（决策 204⑦）：归档是把会话从列表里收起来，不是永久保存。
    pub async fn purge_foreman_messages(&self, cutoff: DateTime<Utc>) -> Result<usize> {
        let purged = sqlx::query("DELETE FROM kanban_foreman_messages WHERE created_at < ?")
            .bind(ts(cutoff))
            .execute(self.pool())
            .await?
            .rows_affected();
        Ok(purged as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_title_is_the_first_message_cut_to_budget() {
        assert_eq!(session_title_from(Some("帮我看下 t1")), "帮我看下 t1");
        assert_eq!(session_title_from(Some("  前后有空白  ")), "前后有空白");
        // 恰好 24 字不补省略号，多一字才补。
        let exactly: String = "字".repeat(SESSION_TITLE_MAX_CHARS);
        assert_eq!(session_title_from(Some(&exactly)), exactly);
        let over: String = "字".repeat(SESSION_TITLE_MAX_CHARS + 1);
        let title = session_title_from(Some(&over));
        assert_eq!(title.chars().count(), SESSION_TITLE_MAX_CHARS + 1);
        assert!(title.ends_with('…'));
        assert_eq!(
            title.chars().filter(|c| *c == '字').count(),
            SESSION_TITLE_MAX_CHARS
        );
    }

    #[test]
    fn session_title_cuts_on_char_boundaries_not_bytes() {
        // 24 个多字节字符不能被切成半个字。
        let text = "流水线卡住了请帮我看一下这个任务到底停在哪一步了谢谢".repeat(2);
        let title = session_title_from(Some(&text));
        let kept: String = text.chars().take(SESSION_TITLE_MAX_CHARS).collect();
        assert_eq!(title, format!("{kept}…"));
    }

    #[test]
    fn empty_first_message_falls_back_to_a_neutral_title() {
        assert_eq!(session_title_from(None), FOREMAN_SESSION_DEFAULT_TITLE);
        assert_eq!(session_title_from(Some("")), FOREMAN_SESSION_DEFAULT_TITLE);
        assert_eq!(
            session_title_from(Some("  \n\t ")),
            FOREMAN_SESSION_DEFAULT_TITLE
        );
    }
}
