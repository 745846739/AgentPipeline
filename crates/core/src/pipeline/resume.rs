//! resume 的**唯一实现**（决策 49 / 91 / 130⑤；票 08 抽出来共用）。
//!
//! 为什么单独成模块：这条路的调用者有**两个**——界面那颗钮（`POST /tasks/{id}/resume`）
//! 与值班长的托管自动动作（决策 210② 的 `resume(continue)` 例外）。抄一份到值班长那边，
//! 就是两套「resume 到底做了什么」的实现，而它们之间必须逐字相同：允许集合的校验、
//! `pending_resume_cooldown_sec` 的防连点、`dependency_failed` 的特别处理——任何一处漂移
//! 都会变成「界面按得动、它按不动」或反过来这种查不出来的不一致。
//!
//! 端点仍然负责 HTTP 那一层（参数解析、错误映射、响应 JSON）；这里负责**状态机**。

use crate::actions::is_action_allowed;
use crate::storage::decisions::ResumeAction;
use crate::config::Settings;
use crate::storage::Store;
use crate::types::{Node, PendingKind, Stage};
use crate::{Error, Result};

/// 一次 resume 请求（与界面那颗按钮发来的 body 同形，决策 91）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResumeRequest {
    /// 拍板的动作名（`continue` / `skip` / `goto` …）。
    pub action: String,
    /// 指定游标。省略仅在该任务**恰有一条**活跃游标时允许（决策 91），否则报冲突。
    pub cursor_id: Option<String>,
    pub target_stage: Option<String>,
    pub target_node: Option<String>,
    /// 给这次拍板的说明 / 打回意见。
    pub input: Option<String>,
}

/// 一次 resume 的结果（字段与端点的响应体一一对应）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedResume {
    pub action: String,
    pub cursor_id: String,
    /// 是否真的拉起了 executor。`false` 有两种来路：冷却窗口内（防连点）、
    /// 或这是一次「交还准入」的 dependency 继续（决策 130⑤）。
    pub spawned: bool,
    /// 任务是否因为这次 resume 需要**回落到准入**（dependency_failed 的 continue）。
    /// 端点与托管动作都据此说清「为什么没有立刻跑起来」。
    pub requeued: bool,
}

/// 拍一次板。**所有调用者共用这一处**（端点 / 值班长的托管自动动作）。
pub async fn apply_resume(
    store: &Store,
    settings: &Settings,
    resume: &crate::scheduler::ResumeFn,
    task_id: &str,
    request: &ResumeRequest,
) -> Result<AppliedResume> {
    store.get_task(task_id).await?;

    // 游标解析（决策 91）：恰好一条可省略；多条缺失 → 冲突
    let cursor = match request.cursor_id.as_deref() {
        Some(cursor_id) => store.get_cursor(cursor_id).await?,
        None => store.resolve_sole_cursor(task_id).await?.ok_or_else(|| {
            Error::Conflict("该任务有多条活跃游标，必须显式提供 cursor_id（决策 91）".into())
        })?,
    };
    if cursor.task_id != task_id {
        return Err(Error::Validation("cursor_id 不属于该任务".into()));
    }

    let action = ResumeAction::parse(&request.action)?;

    // 动作必须在当前 pending 的允许集合内（决策 49）
    if let Some(reason) = &cursor.pending_reason {
        if !is_action_allowed(reason, &request.action) {
            return Err(Error::Validation(format!(
                "动作 {} 不在当前 pending 的允许集合内",
                request.action
            )));
        }
    }

    let target = match (request.target_stage.as_deref(), request.target_node.as_deref()) {
        (Some(stage), Some(node)) => Some((stage.parse::<Stage>()?, node.parse::<Node>()?)),
        _ => None,
    };

    // `pending_resume_cooldown_sec` 防连点（§3）：必须**在写入本次 user_resume 流水之前**
    // 判定，否则刚写入的这条会让间隔恒为 0，第一次 resume 就被误判为连点。
    let cooldown = settings.pending_resume_cooldown_sec as i64;
    let since_last = store.seconds_since_last_user_resume(task_id).await?;
    let within_cooldown = matches!(since_last, Some(secs) if secs < cooldown);

    store
        .apply_resume(&cursor, action, target, request.input.as_deref())
        .await?;

    // 决策 130⑤：dependency_failed 的 continue = 清 pending + 置回 queued 交还**准入**，
    // 不直接 spawn executor（避免绕过 max_concurrent_tasks）。
    let requeued = action == ResumeAction::Continue
        && cursor
            .pending_reason
            .as_ref()
            .is_some_and(|r| r.kind == PendingKind::DependencyFailed);
    let spawned = !within_cooldown && !requeued;
    if spawned {
        resume(task_id);
    }

    Ok(AppliedResume {
        action: action.as_str().to_string(),
        cursor_id: cursor.cursor_id,
        spawned,
        requeued,
    })
}
