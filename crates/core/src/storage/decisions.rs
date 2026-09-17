//! 旁路动作的事务实现（决策 2 / 119 / 124）。
//!
//! 这些动作"先写字段再推进"，不是纯 resume——必须在**单事务**内完成，否则会出现
//! "pending 已清但 approval 未写"的中间态。

use sqlx::Row;

use super::{decode_pending, parse_ts, ts, Store};
use crate::pipeline::landing::{
    entry_node, next_stages, skip_landing, stage_has_node, SkipLanding,
};
use crate::types::{
    Approval, CursorStatus, MergeResult, Node, NodeCursor, PendingKind, PendingReason, ResumeCause,
    Stage, TransitionTrigger,
};
use crate::{Error, Result};

/// merge_approval 的旁路动作（决策 119）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeDecision {
    Approve,
    Return,
}

impl MergeDecision {
    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "approve" => Ok(MergeDecision::Approve),
            "return" => Ok(MergeDecision::Return),
            other => Err(Error::Validation(format!("未知 merge decision：{other}"))),
        }
    }

    fn approval(self) -> Approval {
        match self {
            MergeDecision::Approve => Approval::Approved,
            MergeDecision::Return => Approval::Returned,
        }
    }
}

/// resume 动作（决策 35）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeAction {
    Continue,
    Skip,
    Goto,
}

impl ResumeAction {
    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "continue" => Ok(ResumeAction::Continue),
            "skip" => Ok(ResumeAction::Skip),
            "goto" => Ok(ResumeAction::Goto),
            other => Err(Error::Validation(format!("未知 resume 动作：{other}"))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ResumeAction::Continue => "continue",
            ResumeAction::Skip => "skip",
            ResumeAction::Goto => "goto",
        }
    }
}

impl Store {
    /// 单事务应用 merge 决策（决策 119）。
    pub async fn apply_merge_decision(
        &self,
        task_id: &str,
        decision: MergeDecision,
    ) -> Result<NodeCursor> {
        let now = self.now();
        let live = self.load_live_cursors(task_id).await?;
        let cursor = live
            .iter()
            .find(|c| c.stage == Stage::Merge)
            .cloned()
            .ok_or_else(|| Error::Cursor(format!("任务 {task_id} 没有 merge 阶段的活跃游标")))?;

        // 读取现有 merge metadata 并写入新的 approval（同事务内 upsert）。
        // 行缺失 = 阶段 A 没跑完，无 proposal 可审批；行损坏报错，不静默兜底。
        let mut merge: MergeResult = match self.merge_metadata(task_id).await? {
            Some(v) => v,
            None => {
                return Err(Error::Cursor(format!(
                    "任务 {task_id} 尚无 merge_result 产出，无法写审批（阶段 A 未完成）"
                )))
            }
        };
        merge.approval = decision.approval();

        let (target_stage, target_node) = match decision {
            // approve → 重入 merge.execute 走阶段 B
            MergeDecision::Approve => (Stage::Merge, Node::Execute),
            // return → develop.execute，validate_attempts 重置
            MergeDecision::Return => (Stage::Develop, Node::Execute),
        };

        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO kanban_stage_outputs
             (task_id, stage, output_type, file_path, metadata_json, created_at, updated_at)
             VALUES (?, 'merge', ?, ?, ?, ?, ?)
             ON CONFLICT(task_id, stage, output_type) DO UPDATE SET
                 metadata_json = excluded.metadata_json, updated_at = excluded.updated_at",
        )
        .bind(task_id)
        .bind(crate::types::MERGE_OUTPUT_TYPE)
        .bind(&merge.diff_path)
        .bind(serde_json::to_string(&merge)?)
        .bind(ts(now))
        .bind(ts(now))
        .execute(&mut *tx)
        .await?;

        // 落点**无条件**改写（不只是 pending 时）：人工决策总是要换地方，
        // 而「清 pending」只在它确实卡着时才有意义——两件事合并成一条 SQL 会让
        // 「对一条 active 的游标提交决策」静默失效。
        sqlx::query(
            "UPDATE kanban_node_cursors
             SET stage = ?, node = ?, validate_attempts = 0, updated_at = ?
             WHERE cursor_id = ?",
        )
        .bind(target_stage.as_str())
        .bind(target_node.as_str())
        .bind(ts(now))
        .bind(&cursor.cursor_id)
        .execute(&mut *tx)
        .await?;
        // 离开 pending 走**共同实现**（决策 205）：「通过与驳回」在 pending 原因上同名
        // （都是 merge_approval），只有这里知道人按的是哪一颗，故原因显式给。
        // 同事务——分两个事务会留下「已 active、落点还是旧的」的窗口。
        let cause = match decision {
            MergeDecision::Approve => ResumeCause::MergeApproved,
            MergeDecision::Return => ResumeCause::MergeReturned,
        };
        self.clear_pending_in_tx(&mut tx, &cursor.cursor_id, Some(cause))
            .await?;

        sqlx::query(
            "INSERT INTO kanban_transitions
             (task_id, branch, from_stage, from_node, to_stage, to_node, trigger, reason, created_at)
             VALUES (?, ?, 'merge', 'execute', ?, ?, 'user_resume', ?, ?)",
        )
        .bind(task_id)
        .bind(&cursor.branch)
        .bind(target_stage.as_str())
        .bind(target_node.as_str())
        .bind(match decision {
            MergeDecision::Approve => "用户批准合入",
            MergeDecision::Return => "用户选择返回修改",
        })
        .bind(ts(now))
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        // 任务投影必须同步：否则 /tasks/{id} 在执行器下次写库前一直报
        // 旧的 merge_approval——UI/轮询方会把「返回修改后的过渡态」当成仍待审批，
        // 对着陈旧状态再次提交决策（主流程票 06 的 e2e 打红：二次合入 404）。
        self.sync_task_projection(task_id).await?;
        self.get_cursor(&cursor.cursor_id).await
    }

    /// 人工评审结果（决策 2 / 124）：通过 → test.execute；不通过 → develop.execute。
    ///
    /// 用户评论（§12.5 `{approved, comments}`）写进流转原因，打回时带给 develop。
    pub async fn apply_human_review(
        &self,
        task_id: &str,
        approved: bool,
        comments: Option<&str>,
    ) -> Result<NodeCursor> {
        let now = self.now();
        let live = self.load_live_cursors(task_id).await?;
        let cursor = live
            .iter()
            .find(|c| c.stage == Stage::Review)
            .cloned()
            .or_else(|| live.first().cloned())
            .ok_or_else(|| Error::Cursor(format!("任务 {task_id} 没有活跃游标")))?;

        let (target_stage, target_node) = if approved {
            (Stage::Test, Node::Execute)
        } else {
            (Stage::Develop, Node::Execute)
        };

        let mut reason = if approved {
            "人工评审通过".to_string()
        } else {
            "人工评审打回".to_string()
        };
        if let Some(c) = comments.map(str::trim).filter(|c| !c.is_empty()) {
            reason.push('：');
            reason.push_str(c);
        }

        let mut tx = self.begin_write().await?;
        // 落点无条件改写（同 `apply_merge_decision`）
        sqlx::query(
            "UPDATE kanban_node_cursors
             SET stage = ?, node = ?, validate_attempts = 0, updated_at = ?
             WHERE cursor_id = ?",
        )
        .bind(target_stage.as_str())
        .bind(target_node.as_str())
        .bind(ts(now))
        .bind(&cursor.cursor_id)
        .execute(&mut *tx)
        .await?;
        // 同上：通过与驳回同名，原因由这里给。**驳回这一条正是决策 205 点名要 true 的路**
        // ——此前它绕过 flag，于是「评审驳回 → 打回开发」根本不续接（一处静默的行为缺口）。
        let cause = if approved {
            ResumeCause::HumanReviewApproved
        } else {
            ResumeCause::HumanReviewRejected
        };
        self.clear_pending_in_tx(&mut tx, &cursor.cursor_id, Some(cause))
            .await?;

        sqlx::query(
            "INSERT INTO kanban_transitions
             (task_id, branch, from_stage, from_node, to_stage, to_node, trigger, reason, created_at)
             VALUES (?, ?, 'review', 'validate_output', ?, ?, 'user_resume', ?, ?)",
        )
        .bind(task_id)
        .bind(&cursor.branch)
        .bind(target_stage.as_str())
        .bind(target_node.as_str())
        .bind(&reason)
        .bind(ts(now))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        // 同 apply_merge_decision：决策落库后立刻同步任务投影，不让 UI/轮询方
        // 读到已失效的 human_review（主流程票 06）。
        self.sync_task_projection(task_id).await?;
        self.get_cursor(&cursor.cursor_id).await
    }

    /// 应用 resume 动作（决策 35 / 43 / 69 / 93 / 116）。
    ///
    /// 落点查 [`crate::pipeline::landing`]，不在此处硬编码阶段特例。
    pub async fn apply_resume(
        &self,
        cursor: &NodeCursor,
        action: ResumeAction,
        target: Option<(Stage, Node)>,
        input: Option<&str>,
    ) -> Result<()> {
        // 本次 resume 的流转原因（决策 79 的用户补充输入 / 决策 116 的 dependency_overridden
        // 警告都写进**同一条** user_resume 流转行，不重复插行）。
        let mut reason: Option<String> = input.map(str::to_string);
        match action {
            ResumeAction::Continue => {
                let ctx_kind = cursor
                    .pending_reason
                    .as_ref()
                    .and_then(|r| r.context.as_ref())
                    .and_then(|c| c.kind.as_deref())
                    .map(str::to_string);
                // decision 135：judge_disagreement 的 continue = 用户裁决「合格」，
                // 特判**直接放行到下一阶段入口**，不重跑 validate_output（用户裁决即终审）。
                let is_judge_disagreement =
                    ctx_kind.as_deref() == Some(crate::actions::kinds::JUDGE_DISAGREEMENT);
                // dependency_failed 的 continue 特殊：忽略失败依赖，置回 queued 重新准入（决策 116）
                let is_dependency_failed = cursor
                    .pending_reason
                    .as_ref()
                    .map(|r| r.kind == PendingKind::DependencyFailed)
                    .unwrap_or(false);
                self.clear_cursor_pending(&cursor.cursor_id).await?;
                if is_judge_disagreement {
                    self.advance_after_judge_continue(cursor).await?;
                } else if is_dependency_failed {
                    // 决策 130 ⑤：清 pending + 置回 queued 交还准入（不直接 spawn）。
                    // 决策 116 / 票 06：这一步等于用户**主动忽略失败依赖**，必须在观测面
                    // 留下 `dependency_overridden` 警告（含被忽略的依赖任务 id），
                    // 否则事后无法从审计面看出这个任务是踩着失败依赖上路的。
                    let ignored = match self.dependencies_satisfied(&cursor.task_id).await? {
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
                    self.set_task_status(&cursor.task_id, crate::types::TaskStatus::Queued)
                        .await?;
                } else if cursor
                    .pending_reason
                    .as_ref()
                    .map(|r| r.kind == PendingKind::InfoInsufficient)
                    .unwrap_or(false)
                {
                    // 决策 79 / 票 08：info_insufficient 的补充输入不只是流转原因——
                    // 落任务目录 `user-input.md`，architect-design 重入时注入 prompt。
                    // 空输入不落文件（重入 prompt 该段不渲染）。
                    if let Some(input) = input.map(str::trim).filter(|s| !s.is_empty()) {
                        self.home().ensure_task_dirs(&cursor.task_id)?;
                        std::fs::write(
                            self.home().task_file(&cursor.task_id, "user-input.md"),
                            format!("# 用户补充输入\n\n{input}\n"),
                        )?;
                    }
                }
            }
            ResumeAction::Skip => {
                self.clear_cursor_pending(&cursor.cursor_id).await?;
                match skip_landing(cursor.stage) {
                    SkipLanding::SplitCursors => {
                        self.split_cursors(&cursor.task_id).await?;
                    }
                    SkipLanding::ToJoinBoundary => {
                        self.mark_cursor_skipped_to_join(&cursor.cursor_id).await?;
                    }
                    SkipLanding::StageEntry(stage, node) => {
                        self.set_cursor_stage(&cursor.cursor_id, stage, node)
                            .await?;
                    }
                    SkipLanding::Forbidden => {
                        return Err(Error::Validation(format!(
                            "阶段 {} 无 skip（决策 86）",
                            cursor.stage
                        )));
                    }
                }
                self.reset_cursor_attempts(&cursor.cursor_id).await?;
            }
            ResumeAction::Goto => {
                let is_judge_disagreement = cursor
                    .pending_reason
                    .as_ref()
                    .and_then(|r| r.context.as_ref())
                    .and_then(|c| c.kind.as_deref())
                    == Some(crate::actions::kinds::JUDGE_DISAGREEMENT);
                let (stage, node) = target.ok_or_else(|| {
                    Error::Validation("goto 必须提供 target_stage / target_node".into())
                })?;
                if is_judge_disagreement && stage == cursor.stage && node == Node::Execute {
                    // decision 135：裁决不合格 → 打回本阶段 execute，`validate_attempts` +1
                    // （不走 goto 入口校验，落点就是同阶段的 Execute）。
                    self.clear_cursor_pending(&cursor.cursor_id).await?;
                    self.set_cursor_stage(&cursor.cursor_id, stage, node)
                        .await?;
                    self.increment_cursor_attempts(&cursor.cursor_id).await?;
                } else {
                    if stage == Stage::SyncCheck {
                        return Err(Error::Validation(
                            "sync-check 不占游标行，不能作为 goto 目标（决策 107）".into(),
                        ));
                    }
                    // 决策 69：goto 落点 = entry_node(stage)。任意节点（如 merge.validate_input、
                    // 不存在的节点组合）都是对状态机完整性的破坏，必须拒绝。
                    let expected = entry_node(stage);
                    if node != expected || !stage_has_node(stage, node) {
                        return Err(Error::Validation(format!(
                            "goto 落点必须是 {stage} 的入口节点 {expected}（决策 69）"
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
                            .map(|r| r.kind == PendingKind::RetryExhausted)
                            .unwrap_or(false)
                    {
                        self.write_retry_feedback(cursor).await?;
                    }
                    self.clear_cursor_pending(&cursor.cursor_id).await?;
                    self.set_cursor_stage(&cursor.cursor_id, stage, node)
                        .await?;
                    self.reset_cursor_attempts(&cursor.cursor_id).await?;
                }
            }
        }

        // 流转原因（决策 79 的用户补充输入 / 决策 116 的 dependency_overridden 警告）
        self.insert_transition(
            &cursor.task_id,
            &cursor.branch,
            Some((cursor.stage, cursor.node)),
            (cursor.stage, cursor.node),
            TransitionTrigger::UserResume,
            reason.as_deref(),
        )
        .await?;
        self.sync_task_projection(&cursor.task_id).await?;
        Ok(())
    }

    /// decision 138 / 票 08：把重试历史摘要写入任务目录 `retry-feedback.md`。
    ///
    /// 内容 = 该阶段各次 attempt 的 run 结果（失败原因）+ 最近一次 validate_output
    /// 的 blockers。与 `backtrack-feedback.md` 同构，由 architect-design 重入时注入 prompt。
    async fn write_retry_feedback(&self, cursor: &NodeCursor) -> Result<()> {
        let stage = cursor.stage;
        let mut out = format!("# 重试历史摘要（{stage} 重试耗尽）\n\n");
        out.push_str("## 各次 attempt 的失败原因\n");
        // 只列节点自身的 run（决策 172，票 14）：伪阶段 / 子代理复用父节点的 stage/node，
        // 计入会让摘要里出现与父 run 同 attempt 号的重影行。
        let runs: Vec<crate::types::NodeRun> = self
            .list_runs_at(&cursor.task_id, stage, Node::Execute)
            .await?
            .into_iter()
            .filter(|r| crate::metrics::is_node_owning_run(&r.agent_type))
            .collect();
        if runs.is_empty() {
            out.push_str("（无 run 记录）\n");
        } else {
            for run in &runs {
                let outcome = match run.status {
                    crate::types::NodeStatus::Failed => run
                        .error
                        .clone()
                        .map(|e| format!("失败：{e}"))
                        .unwrap_or_else(|| "失败".into()),
                    crate::types::NodeStatus::Success => "成功".into(),
                    crate::types::NodeStatus::Timeout => "超时".into(),
                    crate::types::NodeStatus::Running => "未完成".into(),
                };
                out.push_str(&format!(
                    "- attempt {}：{outcome}（{}ms）\n",
                    run.attempt, run.duration_ms
                ));
            }
        }
        out.push_str("\n## 最近一次产出校验的 blockers\n");
        let blockers = self
            .last_validate_output_blockers(&cursor.task_id, stage)
            .await?;
        if blockers.is_empty() {
            out.push_str("（无结构化 blockers）\n");
        } else {
            for b in blockers {
                out.push_str(&format!("- {b}\n"));
            }
        }
        self.home().ensure_task_dirs(&cursor.task_id)?;
        std::fs::write(
            self.home().task_file(&cursor.task_id, "retry-feedback.md"),
            out,
        )?;
        Ok(())
    }

    /// 最近一条 `validate_output` 产出的 blockers（无则空）。
    async fn last_validate_output_blockers(
        &self,
        task_id: &str,
        stage: Stage,
    ) -> Result<Vec<String>> {
        let raw: Option<String> = sqlx::query_scalar(
            "SELECT metadata_json FROM kanban_stage_outputs
             WHERE task_id = ? AND stage = ? AND output_type LIKE '%validate%'
             ORDER BY id DESC LIMIT 1",
        )
        .bind(task_id)
        .bind(stage.as_str())
        .fetch_optional(self.pool())
        .await?;
        Ok(raw
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| {
                v.get("blockers").and_then(|b| b.as_array()).map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
            })
            .unwrap_or_default())
    }

    /// 距上次用户 resume 的秒数（`pending_resume_cooldown_sec` 防连点，§3）。
    pub async fn seconds_since_last_user_resume(&self, task_id: &str) -> Result<Option<i64>> {
        let raw: Option<String> = sqlx::query_scalar(
            "SELECT MAX(created_at) FROM kanban_transitions
             WHERE task_id = ? AND trigger = 'user_resume'",
        )
        .bind(task_id)
        .fetch_one(self.pool())
        .await?;
        let last = raw.map(|s| parse_ts(&s)).transpose()?;
        Ok(last.map(|t| (self.now() - t).num_seconds()))
    }

    /// 该游标的 pending 动作集（`GET /tasks/{id}` 下发用）。
    pub async fn cursor_allowed_actions(
        &self,
        cursor: &NodeCursor,
    ) -> Vec<crate::actions::AllowedAction> {
        match &cursor.pending_reason {
            Some(reason) => crate::actions::allowed_actions(reason, Some(&cursor.cursor_id)),
            None => Vec::new(),
        }
    }

    /// 任务当前所有 pending 游标的动作集（前端纯渲染，决策 49）。
    pub async fn allowed_actions_for_task(
        &self,
        task_id: &str,
    ) -> Result<Vec<crate::actions::AllowedAction>> {
        let mut out = Vec::new();
        for cursor in self.load_live_cursors(task_id).await? {
            if cursor.status == CursorStatus::Pending {
                out.extend(self.cursor_allowed_actions(&cursor).await);
            }
        }
        Ok(out)
    }

    /// 取消任务：终态 + 依赖任务置 pending(dependency_failed)（§12.3）。
    ///
    /// 该原因只挂在**尚未启动**（queued / waiting）的依赖任务上——已在执行的依赖
    /// 不因本任务取消而被翻成 pending（data-model §5 的适用范围）。
    pub async fn cancel_task(&self, task_id: &str) -> Result<Vec<String>> {
        self.mark_terminal(task_id, crate::types::TaskStatus::Cancelled)
            .await?;
        let mut notified = Vec::new();
        for dependent in self.dependents_of(task_id).await? {
            let status = self.get_task(&dependent).await?.status;
            if !matches!(
                status,
                crate::types::TaskStatus::Queued | crate::types::TaskStatus::Waiting
            ) {
                continue;
            }
            let dependents_cursors = self.load_live_cursors(&dependent).await?;
            if let Some(cursor) = dependents_cursors.first() {
                let reason = PendingReason::new(
                    PendingKind::DependencyFailed,
                    cursor.stage,
                    cursor.node,
                    format!("依赖任务 {task_id} 已取消"),
                )
                .with_context(crate::types::PendingContext::with_kind(
                    crate::actions::kinds::DEPENDENCY_CANCELLED,
                ));
                self.set_cursor_pending(&cursor.cursor_id, &reason).await?;
                self.sync_task_projection(&dependent).await?;
                notified.push(dependent);
            }
        }
        Ok(notified)
    }

    /// 依赖任务被重试后恢复：清 pending、退回 waiting（决策 57）。
    pub async fn recover_dependency_failed(&self) -> Result<Vec<String>> {
        let mut recovered = Vec::new();
        for cursor in self
            .cursors_with_pending_kind(PendingKind::DependencyFailed)
            .await?
        {
            if self.dependency_running(&cursor.task_id).await? {
                self.clear_cursor_pending(&cursor.cursor_id).await?;
                self.set_task_status(&cursor.task_id, crate::types::TaskStatus::Waiting)
                    .await?;
                // 任务级 pending 是焦点游标投影，清 pending 后必须一并刷新
                self.sync_task_projection(&cursor.task_id).await?;
                recovered.push(cursor.task_id);
            }
        }
        Ok(recovered)
    }

    /// 依赖全 done → 置 queued（决策 57 的 `check_waiting_tasks`）。
    pub async fn promote_ready_dependencies(&self) -> Result<Vec<String>> {
        let mut promoted = Vec::new();
        for task in self.waiting_tasks().await? {
            if self.dependencies_satisfied(&task.id).await? == super::tasks::DependencyState::Ready
            {
                self.set_task_status(&task.id, crate::types::TaskStatus::Queued)
                    .await?;
                promoted.push(task.id);
            }
        }
        Ok(promoted)
    }

    /// 游标的下一阶段入口（`advance_cursor` 的落点，join 边界在此表达）。
    pub fn next_landing(stage: Stage) -> Vec<(Stage, Node)> {
        next_stages(stage)
            .iter()
            .map(|s| (*s, entry_node(*s)))
            .collect()
    }

    /// decision 135：judge_disagreement 的 continue = 用户裁决「合格」，放行到下一阶段入口。
    ///
    /// 落点判定**复用** [`crate::pipeline::stage_landing`]（票 03：不再复刻游标落点逻辑），
    /// 与 executor 的 `EdgeKind::Next` 跨阶段分支同源。不重跑 validate_output。
    async fn advance_after_judge_continue(&self, cursor: &NodeCursor) -> Result<()> {
        match crate::pipeline::stage_landing(cursor.stage) {
            crate::pipeline::StageLanding::Split => {
                self.split_cursors(&cursor.task_id).await?;
                Ok(())
            }
            crate::pipeline::StageLanding::JoinBoundary => {
                self.set_cursor_waiting_join(&cursor.cursor_id).await
            }
            crate::pipeline::StageLanding::StageEntry(stage, node) => {
                self.set_cursor_stage(&cursor.cursor_id, stage, node)
                    .await?;
                self.reset_cursor_attempts(&cursor.cursor_id).await
            }
            crate::pipeline::StageLanding::Terminal => Err(Error::Cursor(format!(
                "阶段 {} 没有下一阶段，judge_disagreement continue 无处放行（决策 135）",
                cursor.stage
            ))),
        }
    }
}

/// 读取一行事件的便捷查询（调试 / 测试断言用）。
pub async fn latest_pending_kind(store: &Store, cursor_id: &str) -> Result<Option<PendingKind>> {
    let raw: Option<String> =
        sqlx::query("SELECT pending_reason_json FROM kanban_node_cursors WHERE cursor_id = ?")
            .bind(cursor_id)
            .fetch_optional(store.pool())
            .await?
            .and_then(|row| row.try_get("pending_reason_json").ok());
    Ok(decode_pending(raw)?.map(|r| r.kind))
}
