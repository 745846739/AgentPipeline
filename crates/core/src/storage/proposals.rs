//! 值班长提议的存储（决策 188 / 207，票 02）。
//!
//! **提议不是动作**（决策 188 的第一约束）：库里存的是「打算做什么」。执行要经端点的全部
//! 校验、由人按下、再走既有的那条路——`allowed_actions` 那颗接缝（后端下发、LLM 的判断
//! 绝不直接接进状态机）由此在「值班长能动手」之后仍然成立。
//!
//! 四个状态是**结果**的词汇表（决策 207）：`pending` / `executed` / `rejected` / `expired`。
//! 「正在执行」不是其中之一——那是 [`Store::claim_foreman_proposal`] 写的 `claimed_at`，
//! 一个实现细节：把在途塞进 status 会让「执行失败后回到 pending」看起来像状态抖动。
//!
//! **行永不因为过期而消失**：过期只改状态，那一轮留在时间线里（审计价值与
//! `briefing_json` / `traces_json` 同一理由）。行只按年龄清理，与对讲台对话同一个保留期。

use chrono::{DateTime, Duration, Utc};
use serde_json::Value;
use sqlx::FromRow;

use super::{parse_ts, ts, Store};
use crate::Result;

/// 提议的有效期（决策 207：**10 分钟**，比照 resume cooldown 的姿态）。
pub const FOREMAN_PROPOSAL_TTL_MINUTES: i64 = 10;

/// `status` 的四个取值（与迁移 0015 的 CHECK 同源）。
pub const FOREMAN_PROPOSAL_PENDING: &str = "pending";
pub const FOREMAN_PROPOSAL_EXECUTED: &str = "executed";
pub const FOREMAN_PROPOSAL_REJECTED: &str = "rejected";
pub const FOREMAN_PROPOSAL_EXPIRED: &str = "expired";

/// 一次列出的提议上限（照会话列表的精神：这是给人看的清单，不是分页数据源）。
pub const FOREMAN_PROPOSAL_LIST_LIMIT: usize = 200;

/// 提议的状态（决策 207 的四个状态）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForemanProposalStatus {
    Pending,
    Executed,
    Rejected,
    Expired,
}

impl ForemanProposalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ForemanProposalStatus::Pending => FOREMAN_PROPOSAL_PENDING,
            ForemanProposalStatus::Executed => FOREMAN_PROPOSAL_EXECUTED,
            ForemanProposalStatus::Rejected => FOREMAN_PROPOSAL_REJECTED,
            ForemanProposalStatus::Expired => FOREMAN_PROPOSAL_EXPIRED,
        }
    }

    /// 认不出的值当作 `pending` 之外的东西处理不了，故按「已作废」读——**不静默兜底成
    /// pending**：那会让一条本来按不了的提议在界面上一直亮着执行钮。
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            FOREMAN_PROPOSAL_PENDING => Some(ForemanProposalStatus::Pending),
            FOREMAN_PROPOSAL_EXECUTED => Some(ForemanProposalStatus::Executed),
            FOREMAN_PROPOSAL_REJECTED => Some(ForemanProposalStatus::Rejected),
            FOREMAN_PROPOSAL_EXPIRED => Some(ForemanProposalStatus::Expired),
            _ => None,
        }
    }

    /// 还是「要人按键」的那一档吗。
    pub fn is_open(self) -> bool {
        matches!(self, ForemanProposalStatus::Pending)
    }
}

/// 一条提议。
#[derive(Debug, Clone, PartialEq)]
pub struct ForemanProposal {
    pub id: String,
    pub session_id: String,
    /// 工具名（C / D / E 层的那一族工具之一）。
    pub tool: String,
    /// 原样的调用参数。执行时按它走既有端点——**提议层不解释参数**。
    pub args: Value,
    /// 一句话说明（人读的那句：要动什么、为什么）。
    pub summary: String,
    /// 提议成立时的态势指纹（参数里带 `task_id` 时才有）。
    pub situation: Option<Value>,
    /// 执行占用时间戳（见模块头）。
    pub claimed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub status: ForemanProposalStatus,
    pub resolved_at: Option<DateTime<Utc>>,
}

impl ForemanProposal {
    /// 到期了吗。**判定不落库**——状态由清扫与执行两条路各自落。
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.expires_at <= now
    }
}

/// 待写入的一条提议（`id` / 时间戳由存储层给，不接调用方的时钟）。
#[derive(Debug, Clone)]
pub struct NewForemanProposal {
    pub session_id: String,
    pub tool: String,
    pub args: Value,
    pub summary: String,
    pub situation: Option<Value>,
}

const PROPOSAL_COLUMNS: &str = "id, session_id, tool, args_json, summary, situation_json, \
                                claimed_at, created_at, expires_at, status, resolved_at";

#[derive(FromRow)]
struct ForemanProposalRow {
    id: String,
    session_id: String,
    tool: String,
    args_json: String,
    summary: String,
    situation_json: Option<String>,
    claimed_at: Option<String>,
    created_at: String,
    expires_at: String,
    status: String,
    resolved_at: Option<String>,
}

impl ForemanProposalRow {
    fn into_proposal(self) -> Result<ForemanProposal> {
        Ok(ForemanProposal {
            id: self.id,
            session_id: self.session_id,
            tool: self.tool,
            // 参数与态势是**决定执行什么**的字段，不是观测字段：损坏即报错。
            // 静默兜底成空对象会让「提议执行了一个空参数的动作」这种最难查的事发生
            // （与 `pending_reason` 同一分类）。
            args: serde_json::from_str(&self.args_json)?,
            summary: self.summary,
            situation: self
                .situation_json
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?,
            claimed_at: self.claimed_at.as_deref().map(parse_ts).transpose()?,
            created_at: parse_ts(&self.created_at)?,
            expires_at: parse_ts(&self.expires_at)?,
            // status 决定这一轮在界面上是什么模样，认不出时按「已作废」读（见 `parse`），
            // 不报错：这是观测类字段（storage/mod.rs 的 Q6 分类）。
            status: ForemanProposalStatus::parse(&self.status)
                .unwrap_or(ForemanProposalStatus::Expired),
            resolved_at: self.resolved_at.as_deref().map(parse_ts).transpose()?,
        })
    }
}

impl Store {
    /// 落一条提议。有效期由**存储层**按 [`FOREMAN_PROPOSAL_TTL_MINUTES`] 算，
    /// 不由调用方给：TTL 是决策 207 的一条规则，不是每个调用点各写一遍的参数。
    pub async fn create_foreman_proposal(
        &self,
        new: NewForemanProposal,
    ) -> Result<ForemanProposal> {
        let now = self.now();
        let expires_at = now + Duration::minutes(FOREMAN_PROPOSAL_TTL_MINUTES);
        let id = ulid::Ulid::new().to_string();
        sqlx::query(
            "INSERT INTO kanban_foreman_proposals
             (id, session_id, tool, args_json, summary, situation_json, claimed_at, created_at,
              expires_at, status, resolved_at)
             VALUES (?, ?, ?, ?, ?, ?, NULL, ?, ?, ?, NULL)",
        )
        .bind(&id)
        .bind(&new.session_id)
        .bind(&new.tool)
        .bind(new.args.to_string())
        .bind(&new.summary)
        .bind(new.situation.as_ref().map(Value::to_string))
        .bind(ts(now))
        .bind(ts(expires_at))
        .bind(FOREMAN_PROPOSAL_PENDING)
        .execute(self.pool())
        .await?;
        Ok(ForemanProposal {
            id,
            session_id: new.session_id,
            tool: new.tool,
            args: new.args,
            summary: new.summary,
            situation: new.situation,
            claimed_at: None,
            created_at: now,
            expires_at,
            status: ForemanProposalStatus::Pending,
            resolved_at: None,
        })
    }

    pub async fn get_foreman_proposal(&self, id: &str) -> Result<Option<ForemanProposal>> {
        let sql = format!("SELECT {PROPOSAL_COLUMNS} FROM kanban_foreman_proposals WHERE id = ?");
        let row: Option<ForemanProposalRow> = sqlx::query_as(&sql)
            .bind(id)
            .fetch_optional(self.pool())
            .await?;
        row.map(ForemanProposalRow::into_proposal).transpose()
    }

    /// 某个会话的全部提议（**含已决与已过期**），按 id 升序。
    ///
    /// 时间线要的是全量：过期的那一轮必须还在（决策 207——过期只让按钮变灰），
    /// 已决的那一轮也要在（「值班长当时提议过什么、人怎么按的」是审计）。
    pub async fn list_foreman_proposals(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<ForemanProposal>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let sql = format!(
            "SELECT {PROPOSAL_COLUMNS} FROM (
                 SELECT {PROPOSAL_COLUMNS} FROM kanban_foreman_proposals
                 WHERE session_id = ?
                 ORDER BY id DESC LIMIT ?
             ) ORDER BY id ASC"
        );
        let rows: Vec<ForemanProposalRow> = sqlx::query_as(&sql)
            .bind(session_id)
            .bind(limit as i64)
            .fetch_all(self.pool())
            .await?;
        rows.into_iter()
            .map(ForemanProposalRow::into_proposal)
            .collect()
    }

    /// 某个会话的**未决**提议（`GET /foreman/proposals` 的读数），按 id 升序。
    ///
    /// 「未决」只按 status 判，**不看是否到期**：到期的提议状态要等清扫或一次 execute
    /// 尝试才落到 `expired`，而读端点不该顺手写库。前端拿 `expires_at` 自己算倒计时，
    /// 到点即把按钮摆成灰的——两边对同一件事的判定因此不会互相等待。
    pub async fn list_pending_foreman_proposals(
        &self,
        session_id: &str,
    ) -> Result<Vec<ForemanProposal>> {
        let sql = format!(
            "SELECT {PROPOSAL_COLUMNS} FROM kanban_foreman_proposals
             WHERE session_id = ? AND status = ?
             ORDER BY id ASC"
        );
        let rows: Vec<ForemanProposalRow> = sqlx::query_as(&sql)
            .bind(session_id)
            .bind(FOREMAN_PROPOSAL_PENDING)
            .fetch_all(self.pool())
            .await?;
        rows.into_iter()
            .map(ForemanProposalRow::into_proposal)
            .collect()
    }

    /// **原子占用**一条未决提议（`execute` 的第一步）。
    ///
    /// 条件全写在 `WHERE` 上：`status = pending` 且 `claimed_at IS NULL`。
    /// 拿不到行就是「有人先按了 / 已经处理过 / 正在执行」——调用方据此回冲突，
    /// 于是「一次一按、不可重放」不是靠 UI 的禁用态，而是靠这一次 UPDATE。
    ///
    /// **不在这里判过期**：过期要走「先标记成 expired 再拒」那条路（见路由），
    /// 由它自己先判——把两个判据挤进一条 SQL，出错时说不清是哪个条件没过。
    pub async fn claim_foreman_proposal(&self, id: &str) -> Result<Option<ForemanProposal>> {
        let affected = sqlx::query(
            "UPDATE kanban_foreman_proposals SET claimed_at = ?
             WHERE id = ? AND status = ? AND claimed_at IS NULL",
        )
        .bind(ts(self.now()))
        .bind(id)
        .bind(FOREMAN_PROPOSAL_PENDING)
        .execute(self.pool())
        .await?
        .rows_affected();
        if affected == 0 {
            return Ok(None);
        }
        self.get_foreman_proposal(id).await
    }

    /// 占用 → 终态（`executed` / `rejected`）。**只有被占用过的提议能走到这里**
    /// ——没被占用的那条会被 `WHERE claimed_at IS NOT NULL` 挡下。
    ///
    /// 返回 `None` 表示没有落到终态（行不存在 / 没被占用 / 已经是终态）。
    pub async fn resolve_foreman_proposal(
        &self,
        id: &str,
        status: ForemanProposalStatus,
    ) -> Result<Option<ForemanProposal>> {
        if status.is_open() {
            return Err(crate::Error::Validation(
                "提议的终态只能是 executed / rejected / expired".into(),
            ));
        }
        let now = self.now();
        let affected = sqlx::query(
            "UPDATE kanban_foreman_proposals SET status = ?, resolved_at = ?, claimed_at = NULL
             WHERE id = ? AND claimed_at IS NOT NULL",
        )
        .bind(status.as_str())
        .bind(ts(now))
        .bind(id)
        .execute(self.pool())
        .await?
        .rows_affected();
        if affected == 0 {
            return Ok(None);
        }
        self.get_foreman_proposal(id).await
    }

    /// 到期作废：`pending` → `expired`（**不需要占用**，也不抢那把锁）。
    ///
    /// 与 [`Self::resolve_foreman_proposal`] 分开的理由：它不由人按下，也就不该与
    /// 「有人正在执行」抢占用——一条正好在执行中的提议**不该**被清扫改成过期
    /// （它已经被人按了，正跑着）。`claimed_at IS NULL` 这个条件表达的就是这一条，
    /// 拿不到行就说明它正在被处理，交给那次执行自己落终态。
    pub async fn expire_foreman_proposal(&self, id: &str) -> Result<Option<ForemanProposal>> {
        let now = self.now();
        let affected = sqlx::query(
            "UPDATE kanban_foreman_proposals SET status = ?, resolved_at = ?
             WHERE id = ? AND status = ? AND claimed_at IS NULL",
        )
        .bind(FOREMAN_PROPOSAL_EXPIRED)
        .bind(ts(now))
        .bind(id)
        .bind(FOREMAN_PROPOSAL_PENDING)
        .execute(self.pool())
        .await?
        .rows_affected();
        if affected == 0 {
            return Ok(None);
        }
        self.get_foreman_proposal(id).await
    }

    /// 释放占用、退回未决（执行**失败**时走这条）。
    ///
    /// 失败不消耗提议：人看到失败理由之后可以再按一次（参数过不了校验是模型的事，
    /// 不是提议本身作废）。这也正是「status 不变成 executed」那条验收的落点。
    pub async fn release_foreman_proposal(&self, id: &str) -> Result<()> {
        sqlx::query(
            "UPDATE kanban_foreman_proposals SET claimed_at = NULL
             WHERE id = ? AND status = ?",
        )
        .bind(id)
        .bind(FOREMAN_PROPOSAL_PENDING)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// 过期清扫（接进既有每小时维护作业）：全部到点仍未决的 → `expired`。
    ///
    /// **不删行**：那一轮留在时间线里。真正的删除是 [`Self::purge_foreman_proposals`]
    /// 的年龄清理，与对讲台对话同一个保留期。
    ///
    /// `claimed_at IS NULL` 与单条那条同一个理由（两处必须一致，否则清扫会把一条**正在
    /// 执行**的提议改成过期，而那次执行随后落终态时已经把状态写成了 expired）。
    pub async fn expire_foreman_proposals(&self, now: DateTime<Utc>) -> Result<usize> {
        let affected = sqlx::query(
            "UPDATE kanban_foreman_proposals SET status = ?, resolved_at = ?
             WHERE status = ? AND claimed_at IS NULL AND expires_at <= ?",
        )
        .bind(FOREMAN_PROPOSAL_EXPIRED)
        .bind(ts(now))
        .bind(FOREMAN_PROPOSAL_PENDING)
        .bind(ts(now))
        .execute(self.pool())
        .await?
        .rows_affected();
        Ok(affected as usize)
    }

    /// 按年龄清理（与 `conversation_retention_days` 同口径）。
    pub async fn purge_foreman_proposals(&self, cutoff: DateTime<Utc>) -> Result<usize> {
        let purged = sqlx::query("DELETE FROM kanban_foreman_proposals WHERE created_at < ?")
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
    fn status_round_trips_through_its_wire_name() {
        for s in [
            ForemanProposalStatus::Pending,
            ForemanProposalStatus::Executed,
            ForemanProposalStatus::Rejected,
            ForemanProposalStatus::Expired,
        ] {
            assert_eq!(ForemanProposalStatus::parse(s.as_str()), Some(s));
        }
        // 认不出的值不兜底成 pending（见 `parse` 的注释）
        assert_eq!(ForemanProposalStatus::parse("running"), None);
        assert_eq!(ForemanProposalStatus::parse(""), None);
    }

    #[test]
    fn only_pending_is_open() {
        assert!(ForemanProposalStatus::Pending.is_open());
        for s in [
            ForemanProposalStatus::Executed,
            ForemanProposalStatus::Rejected,
            ForemanProposalStatus::Expired,
        ] {
            assert!(!s.is_open(), "{s:?} 不该是未决");
        }
    }
}
