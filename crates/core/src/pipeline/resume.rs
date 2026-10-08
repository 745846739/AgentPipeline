//! resume 的**唯一实现**（决策 49 / 91 / 130⑤；票 08 抽出来共用）。
//!
//! 为什么单独成模块：这条路的调用者有**两个**——界面那颗钮（`POST /tasks/{id}/resume`）
//! 与值班长的托管自动动作（决策 210② 的 `resume(continue)` 例外）。抄一份到值班长那边，
//! 就是两套「resume 到底做了什么」的实现，而它们之间必须逐字相同：允许集合的校验、
//! `pending_resume_cooldown_sec` 的防连点、`dependency_failed` 的特别处理——任何一处漂移
//! 都会变成「界面按得动、它按不动」或反过来这种查不出来的不一致。
//!
//! 端点仍然负责 HTTP 那一层（参数解析、错误映射、响应 JSON）；这里负责**状态机**。
//!
//! **决策 245**：`ResumeAction → 落点` 的翻译也在这里（原在 `Store::apply_resume`）。
//! 那是 storage 里**唯一一整套「原因 → 落点」的翻译**，搬走之后 storage 不再自己推导
//! 该把游标搬到哪（它仍是行级原语 + merge / review 两支**旁路动作**的事务——那两支
//! 决策 119 / 124 的「先写字段再推进」本就不属于推进）。落库统一走
//! [`crate::pipeline::advance`] 的一笔事务。同时补上这条路径一直缺的 SSE——
//! 交互式 resume 此前**完全静默**，界面只能靠重新拉取或轮询。

use std::sync::Arc;

use crate::actions::{is_action_allowed, kinds};
use crate::config::Settings;
use crate::sse::{SseEvent, SseSink};
use crate::storage::decisions::ResumeAction;
use crate::storage::Store;
use crate::types::{Node, NodeCursor, PendingKind, Stage, TransitionTrigger};
use crate::{Error, Result};

use super::advance::{advance, Advanced, Landing};
use super::landing::{
    entry_node, skip_landing, stage_has_node, stage_landing, SkipLanding, StageLanding,
};

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

/// 一次动作执行前后的游标（事件面据此判断有没有真的挪地方）。
pub struct ActionOutcome {
    /// 动作之前的那条游标快照。
    before: NodeCursor,
    /// 动作之后的全部活跃游标（architect 的 `skip` / `continue` 会从一条变两条）。
    cursors: Vec<NodeCursor>,
}

/// 拍一次板。**所有调用者共用这一处**（端点 / 值班长的托管自动动作）。
///
/// `sse` 是这条路径唯一的观测出口（决策 245）：此前 `decisions.rs` 里零 `emit`、
/// `Store` 结构体连 `sse` 字段都没有，于是「人按了 continue」在实时流里是**无声**的。
pub async fn apply_resume(
    store: &Store,
    settings: &Settings,
    resume: &crate::scheduler::ResumeFn,
    sse: &Arc<dyn SseSink>,
    task_id: &str,
    request: &ResumeRequest,
) -> Result<AppliedResume> {
    store.get_task(task_id).await?;

    // 游标解析（决策 91）：恰好一条可省略；多条缺失 → 冲突
    let cursor = match request.cursor_id.as_deref() {
        Some(cursor_id) => store.get_cursor(cursor_id).await?,
        None => store.resolve_sole_cursor(task_id).await?.ok_or_else(|| {
            Error::Conflict("该任务有多条活跃游标，必须显式提供 cursor_id".into())
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

    let target = match (
        request.target_stage.as_deref(),
        request.target_node.as_deref(),
    ) {
        (Some(stage), Some(node)) => Some((stage.parse::<Stage>()?, node.parse::<Node>()?)),
        _ => None,
    };

    // `pending_resume_cooldown_sec` 防连点（§3）：必须**在写入本次 user_resume 流水之前**
    // 判定，否则刚写入的这条会让间隔恒为 0，第一次 resume 就被误判为连点。
    let cooldown = settings.pending_resume_cooldown_sec as i64;
    let since_last = store.seconds_since_last_user_resume(task_id).await?;
    let within_cooldown = matches!(since_last, Some(secs) if secs < cooldown);

    let outcome = apply_action(store, &cursor, action, target, request.input.as_deref()).await?;
    emit_outcome(sse, &outcome);

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

/// **落库**一次动作：`ResumeAction → Landing` 的翻译 + 一笔事务 + 同步投影。
///
/// 它是 [`apply_resume`] 的下半截，两者合起来才是「resume 做了什么」。单独可调是给
/// 已经自己解析好游标的调用方（L2 用例）用的——它**不做**允许集合校验与防连点，
/// 那两道闸在 [`apply_resume`] 里（决策 49 / §3）。
///
/// **四种原因各自校验、各自翻译，校验全在门外**——门只回答「给定落点，一笔事务写完」
/// （决策 245）。文件落盘（`user-input.md` / `retry-feedback.md`）保持在动游标**之前**：
/// 写失败即中止，不出现「游标已动、文件缺失」的半截状态（决策 138 那条纪律）。
pub async fn apply_action(
    store: &Store,
    cursor: &NodeCursor,
    action: ResumeAction,
    target: Option<(Stage, Node)>,
    input: Option<&str>,
) -> Result<ActionOutcome> {
    let task_id = &cursor.task_id;
    // 本次 resume 的流转原因（决策 79 的用户补充输入 / 决策 116 的 dependency_overridden
    // 警告都写进**同一条** user_resume 流转行，不重复插行）。
    let mut reason: Option<String> = input.map(str::to_string);

    let ctx_kind = cursor
        .pending_reason
        .as_ref()
        .and_then(|r| r.context.as_ref())
        .and_then(|c| c.kind.as_deref())
        .map(str::to_string);
    // decision 135：judge_disagreement 的 continue = 用户裁决「合格」，
    // 特判**直接放行到下一阶段入口**，不重跑 validate_output（用户裁决即终审）。
    let is_judge_disagreement = ctx_kind.as_deref() == Some(kinds::JUDGE_DISAGREEMENT);

    let advanced = match action {
        ResumeAction::Continue => {
            // 决策 116：依赖失败的 continue = 用户**主动忽略失败依赖**
            let is_dependency_failed = cursor
                .pending_reason
                .as_ref()
                .is_some_and(|r| r.kind == PendingKind::DependencyFailed);
            let is_info_insufficient = cursor
                .pending_reason
                .as_ref()
                .is_some_and(|r| r.kind == PendingKind::InfoInsufficient);

            if is_judge_disagreement {
                match stage_landing(cursor.stage) {
                    StageLanding::Split => split_via(store, cursor, &reason).await?,
                    StageLanding::JoinBoundary => {
                        advance_one(
                            store,
                            cursor,
                            Landing::JoinBoundary { skipped: false },
                            &reason,
                        )
                        .await?
                    }
                    StageLanding::StageEntry(stage, node) => {
                        advance_one(store, cursor, Landing::Entry(stage, node), &reason).await?
                    }
                    StageLanding::Terminal => {
                        return Err(Error::Cursor(format!(
                            "阶段 {} 没有下一阶段，judge_disagreement continue 无处放行",
                            cursor.stage
                        )));
                    }
                }
            } else if is_dependency_failed {
                // 决策 130 ⑤：清 pending + 置回 queued 交还准入（不直接 spawn）。
                // 决策 116 / 票 06：必须在观测面留下 `dependency_overridden` 警告
                // （含被忽略的依赖任务 id），否则事后无法看出这任务是踩着失败依赖上路的。
                let ignored = match store.dependencies_satisfied(task_id).await? {
                    crate::storage::tasks::DependencyState::Failed(ids) => ids,
                    _ => Vec::new(),
                };
                let detail = if ignored.is_empty() {
                    "（未记录 id）".to_string()
                } else {
                    ignored.join("、")
                };
                reason = Some(format!(
                    "dependency_overridden：忽略失败依赖 {detail}（决策 116）"
                ));
                let advanced = advance_one(store, cursor, Landing::Stay, &reason).await?;
                store
                    .set_task_status(task_id, crate::types::TaskStatus::Queued)
                    .await?;
                advanced
            } else if is_info_insufficient {
                // 决策 79 / 票 08：补充输入不只是流转原因——落任务目录 `user-input.md`，
                // architect-design 重入时注入 prompt。空输入不落文件（重入 prompt 该段不渲染）。
                //
                // 票 05②：落的**不只是用户那两句话**。`info_insufficient` 的 pending 消息里带着
                // 问题清单 + 推荐答案（决策 277④），一起写进去，下游才看得到「同意」同意的是什么。
                // 2026-10-01 事故里 `user-input.md` 只有孤零零一个「同意」，architect-design 只能
                // 靠翻仓库猜范围（60 分钟 / 89 次只读调用 / 一个字没写）。
                if let Some(input) = input.map(str::trim).filter(|s| !s.is_empty()) {
                    let questions = cursor
                        .pending_reason
                        .as_ref()
                        .map(|r| r.message.trim())
                        .filter(|m| !m.is_empty());
                    let mut body = String::from("# 用户补充输入\n\n");
                    if let Some(questions) = questions {
                        body.push_str("## 当时提交的问题（含推荐答案，来自 validate_input）\n\n");
                        body.push_str(questions);
                        body.push_str("\n\n");
                    }
                    body.push_str("## 用户答复\n\n");
                    body.push_str(input);
                    body.push('\n');
                    store.home().ensure_task_dirs(task_id)?;
                    std::fs::write(store.home().task_file(task_id, "user-input.md"), body)?;
                }
                advance_one(store, cursor, Landing::Stay, &reason).await?
            } else {
                advance_one(store, cursor, Landing::Stay, &reason).await?
            }
        }
        ResumeAction::Skip => match skip_landing(cursor.stage) {
            SkipLanding::SplitCursors => split_via(store, cursor, &reason).await?,
            SkipLanding::ToJoinBoundary => {
                advance_one(
                    store,
                    cursor,
                    Landing::JoinBoundary { skipped: true },
                    &reason,
                )
                .await?
            }
            SkipLanding::StageEntry(stage, node) => {
                advance_one(store, cursor, Landing::Entry(stage, node), &reason).await?
            }
            SkipLanding::Forbidden => {
                return Err(Error::Validation(format!("阶段 {} 无 skip", cursor.stage)));
            }
        },
        ResumeAction::Goto => {
            let (stage, node) = target.ok_or_else(|| {
                Error::Validation("goto 必须提供 target_stage / target_node".into())
            })?;
            if is_judge_disagreement && stage == cursor.stage && node == Node::Execute {
                // decision 135：裁决不合格 → 打回本阶段 execute，`validate_attempts`
                // 归零后再 +1（不走 goto 入口校验，落点就是同阶段的 Execute）。
                advance_one(
                    store,
                    cursor,
                    Landing::EntryWithAttempt(stage, node),
                    &reason,
                )
                .await?
            } else {
                if stage == Stage::SyncCheck {
                    return Err(Error::Validation(
                        "sync-check 不占游标行，不能作为 goto 目标".into(),
                    ));
                }
                // 决策 69：goto 落点 = entry_node(stage)。任意节点（如 merge.validate_input、
                // 不存在的节点组合）都是对状态机完整性的破坏，必须拒绝。
                let expected = entry_node(stage);
                if node != expected || !stage_has_node(stage, node) {
                    return Err(Error::Validation(format!(
                        "goto 落点必须是 {stage} 的入口节点 {expected}"
                    )));
                }
                // 决策 138：develop / test 的 retry_exhausted 走「带失败摘要回架构设计修订」时，
                // 先把重试历史摘要落任务目录 `retry-feedback.md`。**先写文件再动游标**：
                // 写失败即中止，不出现「游标已回架构但摘要缺失」的半截状态。
                if stage == Stage::ArchitectDesign
                    && matches!(cursor.stage, Stage::Develop | Stage::Test)
                    && cursor
                        .pending_reason
                        .as_ref()
                        .is_some_and(|r| r.kind == PendingKind::RetryExhausted)
                {
                    store.write_retry_feedback(cursor).await?;
                }
                // 手动暂停那一行里的 `goto` 就是「重跑本阶段」（决策 276）：落点与别的 goto
                // 相同，**要写的原因不同**——`user_rerun`（那一轮不算，重开一段对话），而不是
                // 按被清掉的 pending 原因分类出来的 `user_paused`（那是「续跑」的去向）。
                // 同一行两颗出口键，只有按键的这一方知道按的是哪一颗（与 merge / review 那两条
                // 同一个理由）。
                let rerun = cursor
                    .pending_reason
                    .as_ref()
                    .is_some_and(|r| r.kind == PendingKind::UserPaused);
                let landed = if rerun {
                    advance_one(store, cursor, Landing::Rerun(stage, node), &reason).await?
                } else {
                    advance_one(store, cursor, Landing::Entry(stage, node), &reason).await?
                };
                landed
            }
        }
    };

    store.sync_task_projection(task_id).await?;
    Ok(ActionOutcome {
        before: cursor.clone(),
        // 门交回来的就是这次推进真正动过的游标（决策 245：`Advanced` 的用途）。
        cursors: advanced.cursors,
    })
}

/// 一次落点落库 + 记一条 `user_resume` 流转（决策 79 / 116 的原因都进这一条）。
///
/// 返回门交回来的 [`Advanced`]——调用方据此发事件，不必再读一遍全表。
async fn advance_one(
    store: &Store,
    cursor: &NodeCursor,
    landing: Landing,
    reason: &Option<String>,
) -> Result<Advanced> {
    advance(
        store,
        &cursor.task_id,
        cursor,
        landing,
        TransitionTrigger::UserResume,
        reason.as_deref(),
    )
    .await
}

/// 「离开 pending + 记流转」之后**重建游标集**（architect 的分裂点，决策 90）。
///
/// 分裂是既有的原子 Store 方法（不在 [`Landing`] 里，决策 245），所以这一步分两笔；
/// 门交回来的那条游标在分裂后已经改写，故这里重读全表——分裂正是会改变**游标条数**的
/// 那种落点，别的落点用门交回来的就够了。
async fn split_via(
    store: &Store,
    cursor: &NodeCursor,
    reason: &Option<String>,
) -> Result<Advanced> {
    advance_one(store, cursor, Landing::Stay, reason).await?;
    store.split_cursors(&cursor.task_id).await?;
    Ok(Advanced {
        cursors: store.load_live_cursors(&cursor.task_id).await?,
    })
}

/// 动作之后的事件面（决策 245）：**位置真挪了才发 `StageChanged`，`CursorChanged` 每条都发**。
///
/// 不发 `Pending`：resume 的落点没有一种会把游标挂起——`Landing::Pause` 的 `Pending`
/// 由 executor / 调度器 / `unstick` 经同一扇门发出来。
fn emit_outcome(sse: &Arc<dyn SseSink>, outcome: &ActionOutcome) {
    let task_id = &outcome.before.task_id;
    let primary = outcome
        .cursors
        .iter()
        .find(|c| c.cursor_id == outcome.before.cursor_id);
    if let Some(p) = primary {
        if (p.stage, p.node) != (outcome.before.stage, outcome.before.node) {
            sse.emit(SseEvent::StageChanged {
                task_id: task_id.clone(),
                branch: p.branch.clone(),
                from_stage: Some(outcome.before.stage),
                from_node: Some(outcome.before.node),
                to_stage: p.stage,
                to_node: p.node,
                trigger: TransitionTrigger::UserResume.as_str().into(),
                reason: None,
            });
        }
    }
    for c in &outcome.cursors {
        sse.emit(SseEvent::CursorChanged {
            task_id: task_id.clone(),
            branch: c.branch.clone(),
            cursor_id: c.cursor_id.clone(),
            status: c.status.as_str().into(),
            stage: c.stage,
            node: c.node,
        });
    }
}
