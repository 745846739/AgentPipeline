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

/// **不设 TTL** 的提议用的远期有效期（决策 212① / 票 12）。
///
/// 为什么是「远期」而不是「把列改成可空」：`expires_at` 是 NOT NULL，而它的意义
/// ——「这条提议按时间作废」——对修复类**不成立**：修复恰好是唯一一条你有意留给自己
/// 第二天早上看的。100 年是一个明确的「不按时间过期」，而不是一个算得出来的时刻；
/// 真正的生命周期归年龄清理（`conversation_retention_days` 同口径）。
pub const FOREMAN_PROPOSAL_NO_TTL_DAYS: i64 = 36_500;

/// 提议的两种载荷形态（决策 212① / 票 12）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForemanProposalKind {
    /// 一次工具调用（原先唯一的那种）。
    ApiCall,
    /// 一次**修复**：执行的不是工具，而是「合入一个分支」。
    Repair,
}

impl ForemanProposalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ForemanProposalKind::ApiCall => "api_call",
            ForemanProposalKind::Repair => "repair",
        }
    }

    pub fn parse(raw: &str) -> Result<Self> {
        Ok(match raw {
            "api_call" => ForemanProposalKind::ApiCall,
            "repair" => ForemanProposalKind::Repair,
            other => return Err(crate::Error::Validation(format!("未知的提议形态：{other}"))),
        })
    }
}

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
    /// 载荷形态（决策 212① / 票 12）。
    pub kind: ForemanProposalKind,
    /// 修复类提议的现场（worktree / 分支 / 基准 / 闸门读数 / diff 路径）。
    pub payload: Option<Value>,
    /// 执行占用时间戳（见模块头）。
    pub claimed_at: Option<DateTime<Utc>>,
    /// **来路**：它来自一轮被人按停的话吗（决策 294 / 票 09）。
    ///
    /// 人按停那一轮提的悬空提议**不作废**（显式修订 233③），于是这一列是它与正常来路的
    /// 提议之间唯一的差别——提议提在一轮没说完的话里，按之前值得多看一眼。
    pub stopped_round: bool,
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
    /// 形态（票 12）。`Repair` 时 `expires_at` 走远期值——**修复不按时间过期**。
    pub kind: ForemanProposalKind,
    /// 修复类的现场。
    pub payload: Option<Value>,
}

const PROPOSAL_COLUMNS: &str = "id, session_id, tool, args_json, summary, situation_json, kind, \
                                payload_json, claimed_at, stopped_round, created_at, expires_at, \
                                status, resolved_at";

#[derive(FromRow)]
struct ForemanProposalRow {
    id: String,
    session_id: String,
    tool: String,
    args_json: String,
    summary: String,
    situation_json: Option<String>,
    kind: String,
    payload_json: Option<String>,
    claimed_at: Option<String>,
    stopped_round: i64,
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
            // 形态与现场是**执行语义**字段（决定按哪条路走）：损坏即报错。
            kind: ForemanProposalKind::parse(&self.kind)?,
            payload: self
                .payload_json
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?,
            claimed_at: self.claimed_at.as_deref().map(parse_ts).transpose()?,
            stopped_round: self.stopped_round != 0,
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
        // 修复类**不按时间过期**（决策 212① / 票 12）：它是唯一一条你有意留到第二天早上
        // 看的东西，10 分钟的 TTL 会让早上看到的是一排灰按钮。年龄清理照旧管它。
        let expires_at = now
            + match new.kind {
                ForemanProposalKind::ApiCall => Duration::minutes(FOREMAN_PROPOSAL_TTL_MINUTES),
                ForemanProposalKind::Repair => Duration::days(FOREMAN_PROPOSAL_NO_TTL_DAYS),
            };
        let id = ulid::Ulid::new().to_string();
        sqlx::query(
            "INSERT INTO kanban_foreman_proposals
             (id, session_id, tool, args_json, summary, situation_json, kind, payload_json,
              claimed_at, created_at, expires_at, status, resolved_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, ?, ?, NULL)",
        )
        .bind(&id)
        .bind(&new.session_id)
        .bind(&new.tool)
        .bind(new.args.to_string())
        .bind(&new.summary)
        .bind(new.situation.as_ref().map(Value::to_string))
        .bind(new.kind.as_str())
        .bind(new.payload.as_ref().map(Value::to_string))
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
            kind: new.kind,
            payload: new.payload,
            claimed_at: None,
            // 落库时**恒 false**：来路这一列只由「人按停」那条路事后标（决策 294），
            // 而它标注的是「提出它的那一轮后来被停了」——落这一行时那件事还没发生。
            stopped_round: false,
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

    /// 作废**这一轮提的**、此刻仍悬空的提议（决策 233③：一轮死了，它那一轮提的提议随之失效）。
    ///
    /// 判据是 `created_at >= since`（`since` = 这一轮开始的时刻）——不能只按班次收：
    /// 那会把**上一轮**留下的、正等着人按键的提议一起作废，而它们是合规的待办
    /// （实测里那两条悬空提议要的是「**它们那一轮**死了才失效」，不是「之后任何一轮死了都失效」）。
    ///
    /// 与 [`Self::expire_foreman_proposals`] 的差别只有判据：那条按**时间**（`expires_at` 到点），
    /// 这条按**事件**（提出它的那一轮死了）。两条都要求 `claimed_at IS NULL`——正在被按下的
    /// 那一条不在此列（它的终态归那次执行）。
    ///
    /// 状态用 `expired` 而不是新造一个：对界面而言「过期」与「那一轮死了」是同一件事
    /// （钮按不动、行留在时间线里）。**不删行**，与其余路径同一姿态。
    pub async fn invalidate_pending_foreman_proposals(
        &self,
        session_id: &str,
        since: DateTime<Utc>,
    ) -> Result<usize> {
        let affected = sqlx::query(
            "UPDATE kanban_foreman_proposals SET status = ?, resolved_at = ?
             WHERE session_id = ? AND status = ? AND claimed_at IS NULL AND created_at >= ?",
        )
        .bind(FOREMAN_PROPOSAL_EXPIRED)
        .bind(ts(self.now()))
        .bind(session_id)
        .bind(FOREMAN_PROPOSAL_PENDING)
        .bind(ts(since))
        .execute(self.pool())
        .await?
        .rows_affected();
        Ok(affected as usize)
    }

    /// **人按停**那一轮提的、此刻仍悬空的提议：**保留**，只标它来自一轮没说完的话
    /// （决策 294 / 票 09，显式修订决策 233③）。
    ///
    /// 与 [`Self::invalidate_pending_foreman_proposals`] 是同一把判据（班次 + `created_at >=
    /// since` + `claimed_at IS NULL`）、相反的两条出路，差别只在**轮是怎么结束的**：
    /// 轮**自己**死了（失败 / panic）→ 等它的人已经没了，钮必须作废；人主动按停 →
    /// 「这话先这样，按你提的第 2 条办」——提议正是他还想按的东西，作废它等于把他刚点的
    /// 菜端走（实测里那两条悬空提议要的正是前一条，本条不改它的语义一个字）。
    ///
    /// 标注（`stopped_round`）是给人看的：这一条提在一轮没说完的话里，按之前多看一眼。
    /// 状态**不动**（`pending` 是它的归宿），故按钮照旧按得动。
    pub async fn mark_pending_foreman_proposals_stopped(
        &self,
        session_id: &str,
        since: DateTime<Utc>,
    ) -> Result<usize> {
        let affected = sqlx::query(
            "UPDATE kanban_foreman_proposals SET stopped_round = 1
             WHERE session_id = ? AND status = ? AND claimed_at IS NULL AND created_at >= ?",
        )
        .bind(session_id)
        .bind(FOREMAN_PROPOSAL_PENDING)
        .bind(ts(since))
        .execute(self.pool())
        .await?
        .rows_affected();
        Ok(affected as usize)
    }

    /// 这一轮**实际提了几条提议**（决策 311，票 foreman-burns 03）。
    ///
    /// 判据与 [`Self::invalidate_pending_foreman_proposals`] /
    /// [`Self::mark_pending_foreman_proposals_stopped`] **同一把尺**（`session_id` +
    /// `created_at >= since`），不另造一个「这一轮」的定义——收场文案要说的正是那两条路
    /// 处理的那批东西，两把尺不一致就会再出现一次「文案说有一批、库里其实没有」。
    ///
    /// 与它们不同的是**不筛状态**：文案问的是「这一轮提没提过提议」，提过就是提过
    /// （按掉的、过期的不改变「提过」这个事实）。这才对得上 2026-09-27 那次的事实——
    /// 该会话的提议计数是 **0**，而文案说「这一轮提的提议都还在」。
    pub async fn count_round_foreman_proposals(
        &self,
        session_id: &str,
        since: DateTime<Utc>,
    ) -> Result<usize> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM kanban_foreman_proposals
             WHERE session_id = ? AND created_at >= ?",
        )
        .bind(session_id)
        .bind(ts(since))
        .fetch_one(self.pool())
        .await?;
        Ok(count as usize)
    }

    /// 这一轮提的**修复提议**带出的 diff 全文（票 01：改动清单的第二份来源）。
    ///
    /// **为什么需要它**：工具痕迹只看得见 `write_file` / `edit_file`，而 `run_command`
    /// 里也可能改文件（`git apply` / `sed -i`）——那一支只有 repair 那份权威 diff 看得见，
    /// 而它从 `propose_repair` 那一刻就落在 `payload_json.diff` 里了。
    ///
    /// **不读磁盘上那份 `.diff`**：`{home}/worktrees/repair-{id}.diff` 是给人看的抄本，
    /// 库里这一份才是权威；而且读它会在收口路径上引入一次阻塞 IO（决策 143 那条缝只留给
    /// 真需要它的地方）。**不筛状态**——「这一轮产出过这份 diff」与它后来被按 / 过期无关
    /// （与 [`Self::count_round_foreman_proposals`] 同一把尺、同一条理由）。
    ///
    /// 解析交给调用方（`pipeline::foreman::changes::diff_paths`）：存储层不解释 diff 的语法。
    pub async fn round_repair_diffs(
        &self,
        session_id: &str,
        since: DateTime<Utc>,
    ) -> Result<Vec<String>> {
        let rows: Vec<Option<String>> = sqlx::query_scalar(
            "SELECT json_extract(payload_json, '$.diff') FROM kanban_foreman_proposals
             WHERE session_id = ? AND created_at >= ? AND kind = ?",
        )
        .bind(session_id)
        .bind(ts(since))
        .bind(ForemanProposalKind::Repair.as_str())
        .fetch_all(self.pool())
        .await?;
        // `json_extract` 对没有 `payload_json` 的行返回 NULL——闸门没过的那种提议就是
        // （`diff` 与 `commit` 一起是 `None`）。那不是错误，只是「这一条没有 diff」。
        Ok(rows.into_iter().flatten().collect())
    }

    /// 超过保留期、**没人按过**的修复提议（决策 212③ / 票 12 的最后一格）。
    ///
    /// 为什么需要它：修复提议的 worktree 只被两条路回收——人按「合入」、人按「拒绝」。
    /// 而「一直没人按」这一条没有落点：行会被年龄清理删掉，那个 worktree 于是变成没有任何
    /// 东西指向的目录残留。回收必须在**删行之前**做，因为行里的 `args.project_id` 与载荷是
    /// 唯一知道那个目录属于谁的东西——故本方法与 [`Self::purge_foreman_proposals`] 在同一趟
    /// 维护作业里，且必须排在它前面。
    ///
    /// 判据用**行**而不是目录年龄：行是「人还按得着吗」的唯一权威，而目录上的时间戳会被
    /// checkout 与编译改得与这件事无关。`status = pending` 把已决的排除在外（执行与拒绝
    /// 那两条路各自回收过了，见 `reject_proposal`）。
    ///
    /// 不按 `expires_at` 筛：过期清扫碰不到修复类（`FOREMAN_PROPOSAL_NO_TTL_DAYS` = 100 年），
    /// 对修复提议而言「没人按过」只有年龄清理这一个出口。
    pub async fn list_pending_repair_proposals_before(
        &self,
        cutoff: DateTime<Utc>,
    ) -> Result<Vec<ForemanProposal>> {
        let sql = format!(
            "SELECT {PROPOSAL_COLUMNS} FROM kanban_foreman_proposals
             WHERE kind = ? AND status = ? AND created_at < ?
             ORDER BY id ASC"
        );
        let rows: Vec<ForemanProposalRow> = sqlx::query_as(&sql)
            .bind(ForemanProposalKind::Repair.as_str())
            .bind(FOREMAN_PROPOSAL_PENDING)
            .bind(ts(cutoff))
            .fetch_all(self.pool())
            .await?;
        rows.into_iter()
            .map(ForemanProposalRow::into_proposal)
            .collect()
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
