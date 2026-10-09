//! merge 状态机（决策 249 · 片④，票 04）：Phase A rebase / 内存合入与引用写回 /
//! Phase B / proposal 生成与 `pending(merge_approval)` 收成一片。排序放最后（②→③→⑤→④）：
//! 它只有 2 条全栈测试钉着真 git 链，是五片里最险的一片。
//!
//! - **闸门执行不搬**（决策 249 留守核裁定）：命令跑法与日志落盘留留守核，本片经其
//!   **自由函数出口** [`executor::run_code_gate`] 调用（依赖显式传入 store / settings /
//!   clock——与 SSE 出口同一姿态，不借 `&Executor`）。
//! - **不新开 git trait**：`Git` 今天是单元 struct 直调——testkit 真仓 fixture 是既定
//!   测法，开 trait 即单 adapter 假想 seam（DEEPENING 判据）。
//! - 与路由的接缝：[`NodeOutput`] 三通道形状不动，`route_merge_*` 的全 `EdgeKind` 分支表
//!   （testing §5 第一优先级）原样。承重性质逐字保：基准移动 → 审批失效 → 重跑 Phase A；
//!   脏工作区是 Pending 不是 Route；`Approval::Approved → None` 的中间态只在循环内。

use std::path::Path;
use std::sync::Arc;

use crate::clock::Clock;
use crate::config::Settings;
use crate::git::Git;
use crate::pipeline::subagent::RunTokens;
use crate::process::ProcessKiller;
use crate::sse::SseSink;
use crate::storage::Store;
use crate::types::{
    Approval, DiffStats, EdgeKind, Gate, GateFailureKind, MergeResult, MergeStatus, Node,
    NodeCursor, PendingContext, PendingKind, PendingReason, Project, Stage, Task,
};
use crate::{Error, Result};

use super::events::finish_run_with_sse;
use super::executor::{
    begin_run_with_sse, clear_zero_commit_facts, declared_no_changes, pend_reason, project_or_err,
    run_code_gate, write_zero_commit_facts, zero_commit_facts, NodeOutput,
};
use super::run_ledger::RunLedger;

/// merge 状态机的依赖面（决策 249 · 票 04）：五件全是借用——留守核每次派发借一遍。
pub(crate) struct MergeFlow<'a> {
    pub(crate) store: &'a Store,
    pub(crate) settings: &'a Settings,
    pub(crate) sse: &'a dyn SseSink,
    pub(crate) clock: &'a dyn Clock,
    /// 合入前的闸门走命令收口（决策 297 / 票 02），收口要一个终止器。
    pub(crate) killer: &'a Arc<dyn ProcessKiller>,
}

impl MergeFlow<'_> {
    fn ledger(&self) -> RunLedger<'_> {
        RunLedger::new(self.store, self.clock)
    }

    /// 挂起出口（经留守核的 [`super::executor::pend_reason`]，形状单点）。
    async fn pend(
        &self,
        cursor: &NodeCursor,
        kind: PendingKind,
        message: impl Into<String>,
    ) -> Result<()> {
        self.pend_with_kind(cursor, kind, message, None).await
    }

    /// 同上，带 `context.kind`（决策 130 ①）：专用动作集靠它才下发得出来，缺失会落到
    /// `(user_decision, _)` 通用兜底行——申报零变更那条路必须带 `zero_changes`。
    async fn pend_with_kind(
        &self,
        cursor: &NodeCursor,
        kind: PendingKind,
        message: impl Into<String>,
        kind_str: Option<&str>,
    ) -> Result<()> {
        let mut reason = PendingReason::new(kind, cursor.stage, cursor.node, message);
        reason.context = kind_str.map(PendingContext::with_kind);
        pend_reason(self.store, self.sse, cursor, reason).await
    }

    /// 基准失配 → approval 重置回 none（决策 96；gate_failures 由 upsert 保留，决策 108）。
    /// 抽到这一层：**不跑全节点循环也能直测**（票 04 窄测试）。返回 true = 已重置、
    /// 调用方回阶段 A。一致时不动行（`stored` 原样，调用方直接进 Phase B）。
    pub(crate) async fn reset_stale_approval(
        &self,
        task_id: &str,
        stored: &MergeResult,
        current_base: &str,
    ) -> Result<bool> {
        if current_base == stored.base_commit {
            return Ok(false);
        }
        let mut reset = stored.clone();
        reset.approval = Approval::None;
        self.store
            .upsert_merge_result(task_id, &reset.diff_path, &reset)
            .await?;
        tracing::info!(task = %task_id, "基准已前移，approval 重置回阶段 A（决策 96）");
        Ok(true)
    }

    /// merge.execute（§6 merge；决策 72 / 85 / 95 / 96 / 97 / 108 / 119 / 139）。
    pub(crate) async fn execute(&self, task: &Task, cursor: &NodeCursor) -> Result<NodeOutput> {
        let project = project_or_err(self.store, &task.project_id).await?;
        let worktree = task
            .worktree_path
            .clone()
            .ok_or_else(|| Error::Validation("merge 阶段缺少 worktree（init 未完成？）".into()))?;

        // 入口判定（决策 72 / 95）：approval 决定阶段 A / B
        let approval = self
            .store
            .merge_metadata(&task.id)
            .await?
            .map(|m| m.approval)
            .unwrap_or(Approval::None);

        let mut current = approval;
        loop {
            match current {
                Approval::Pending => {
                    // 仍在等审批：不推进、不重入（决策 95）；保证挂上 pending 以退出循环
                    if !cursor.is_pending() {
                        self.pend(cursor, PendingKind::MergeApproval, "等待审批合入")
                            .await?;
                    }
                    return Ok(NodeOutput::Route(crate::pipeline::MetadataView::default()));
                }
                Approval::None | Approval::Returned => {
                    // 阶段 A 内部已解析 base_ref，冲突反馈需要它，这里取一次
                    let repo = Path::new(&project.local_path);
                    let base_ref = Git.base_ref(repo, &project.default_branch).await?;
                    let outcome = self.phase_a(task, &project, cursor, &worktree).await?;
                    return match outcome {
                        // 阶段 A 末尾已挂 pending(merge_approval)；闸门失败已写 metadata。
                        // 两者都交给 route_merge 确认（NoOp / GotoTest / KickbackDevelop / 耗尽）。
                        // PendingDecision（决策 391）：零变更申报的 pending 已挂上，
                        // merge metadata 无闸门结果 → route_merge NoOp，等用户动作。
                        PhaseA::Proposal | PhaseA::GateRan | PhaseA::PendingDecision => {
                            Ok(NodeOutput::Route(crate::pipeline::MetadataView::default()))
                        }
                        PhaseA::Conflict(files) => Ok(NodeOutput::Edge(
                            EdgeKind::KickbackDevelop,
                            Some(format!(
                                "rebase {base_ref} 时发生冲突，冲突文件：{}，请基于最新基准修改代码解决冲突",
                                files.join("、")
                            )),
                        )),
                    };
                }
                Approval::Approved => {
                    // (0) 基准校验（决策 96）：不一致 → approval 重置回 none，回阶段 A
                    let repo = Path::new(&project.local_path);
                    let base_ref = Git.base_ref(repo, &project.default_branch).await?;
                    let current_base = Git.rev_parse(repo, &base_ref).await?;
                    let stored = self.store.merge_metadata(&task.id).await?.ok_or_else(|| {
                        Error::Validation("approval=approved 但没有 merge_result".into())
                    })?;
                    // 基准校验（决策 96）：失配 → approval 重置回 none、回阶段 A（continue）。
                    // 判定与重置抽成独立一层：不跑全节点循环也能直测（票 04 窄测试）。
                    if self
                        .reset_stale_approval(&task.id, &stored, &current_base)
                        .await?
                    {
                        current = Approval::None;
                        continue;
                    }
                    return self.phase_b(task, &project, cursor, stored).await;
                }
            }
        }
    }

    /// 阶段 A：rebase → 闸门 → 生成 proposal → pending(merge_approval)。
    async fn phase_a(
        &self,
        task: &Task,
        project: &Project,
        cursor: &NodeCursor,
        worktree: &str,
    ) -> Result<PhaseA> {
        let (run_id, attempt) =
            begin_run_with_sse(self.sse, &self.ledger(), task, cursor, "system").await?;
        let started = self.clock.now();
        let result = self
            .phase_a_inner(task, project, cursor, worktree, run_id)
            .await;
        // PendingDecision（决策 391）挂的是等用户的 pending，不是节点失败。
        let failed = !matches!(
            result,
            Ok(PhaseA::Proposal) | Ok(PhaseA::GateRan) | Ok(PhaseA::PendingDecision)
        );
        finish_run_with_sse(
            self.sse,
            &self.ledger(),
            run_id,
            task,
            cursor,
            attempt,
            failed,
            started,
            result.as_ref().err().map(|e| e.to_string()),
            &RunTokens::default(),
        )
        .await?;
        result
    }

    async fn phase_a_inner(
        &self,
        task: &Task,
        project: &Project,
        cursor: &NodeCursor,
        worktree: &str,
        run_id: i64,
    ) -> Result<PhaseA> {
        let repo = Path::new(&project.local_path);
        let wt = Path::new(worktree);
        self.ledger()
            .mark_step(run_id, "解析合入基准（base_ref）")
            .await;
        let base_ref = Git.base_ref(repo, &project.default_branch).await?;

        // (2) rebase 到基准（决策 74 / 96 / pipeline-spec §6）
        self.ledger()
            .mark_step(run_id, "把任务分支 rebase 到基准")
            .await;
        // 决策 416 B · 自愈：工作区里还剩没提交的已跟踪改动时，libgit2 的 rebase 会在
        // 起步处就以 `unstaged changes exist in workdir` 拒绝——那是本任务自己的产出，
        // 先落一个带标记的提交再 rebase。未跟踪文件不动（见原语文档）。
        let autosaved = Git.commit_unstaged_tracked_changes(wt).await?;
        if let Some((commit, files)) = &autosaved {
            tracing::info!(
                task = %task.id,
                commit = %commit,
                files = ?files,
                "merge 阶段 A 自愈：工作区残留改动已自动落提交（决策 416 B）"
            );
        }
        let mut auto_resolved: Vec<String> = Vec::new();
        match Git.rebase_onto_with_auto_resolve(wt, &base_ref).await? {
            crate::git::AutoRebaseOutcome::Clean { .. } => {}
            crate::git::AutoRebaseOutcome::AutoResolved { files, .. } => {
                tracing::info!(
                    task = %task.id,
                    files = ?files,
                    "rebase 冲突已自动解决，继续合入流程（pipeline-spec §6）"
                );
                auto_resolved = files;
            }
            crate::git::AutoRebaseOutcome::Conflict { files } => {
                // 无法机械判定：helper 已 rebase --abort，按决策 74 打回 develop
                return Ok(PhaseA::Conflict(files));
            }
        }
        let base_commit = Git.rev_parse(repo, &base_ref).await?;

        // 生成 diff（{base_ref}..{branch}）——先于闸门：merge_result 行是文档必填全字段，
        // 失败分支也必须引用真实存在的 diff 文件（§4.2 / §8 幂等，决策 96）
        let branch = task
            .branch_name
            .clone()
            .ok_or_else(|| Error::Validation("merge 阶段缺少 branch_name".into()))?;
        let range = format!("{base_ref}..{branch}");
        let diff_path = "merge-proposal.diff";
        let diff_stats = parse_diff_stats(&Git.diff_stat(repo, &range).await?);
        if diff_stats.files_changed == 0 {
            // 决策 391：先看 develop 是否显式申报了「本任务零变更」。
            // 申报成立 → 空分支不是缺陷而是诚实结论：交用户确认（cancel 收尾或回
            // develop 继续改），不写闸门失败、不烧 gate_failures。
            if declared_no_changes(self.store, &task.id).await? {
                clear_zero_commit_facts(self.store.home(), &task.id);
                // 必须带 `context.kind`：否则 `allowed_actions` 落到
                // `(user_decision, _)` 通用兜底行（{skip, cancel}），而下发不出本分支
                // 语义的 {goto develop, cancel}（决策 130 ①）。
                self.pend_with_kind(
                    cursor,
                    PendingKind::UserDecision,
                    "develop 申报本任务零变更：确认零变更收尾取消，或回 develop 继续修改",
                    Some(crate::actions::kinds::ZERO_CHANGES),
                )
                .await?;
                return Ok(PhaseA::PendingDecision);
            }
            // 未申报的空差异 = 没有可合入的变更；按闸门失败分流处理，不静默合入。
            // 类型是 EmptyBranch（决策 391）：确定性失败直接打回 develop（routes 决策
            // 139 先例），不进 test 复检——修用例造不出分支提交，复检必然空转。
            // 事实段与 develop 守卫同源落盘：重入 develop.execute 时 prompt 带同样指令。
            let dirty = Git.dirty_files(wt).await.unwrap_or_default();
            let headline = format!(
                "- 任务分支 `{branch}` 相对基准 `{base_ref}` 的净差异：**0 个文件**\n\
                 - 工作区未提交改动：{} 处\n",
                dirty.len(),
            );
            write_zero_commit_facts(
                self.store.home(),
                &task.id,
                &zero_commit_facts(&headline, &dirty),
            )?;
            self.store
                .upsert_merge_result(
                    &task.id,
                    diff_path,
                    &MergeResult {
                        diff_path: diff_path.into(),
                        diff_stats,
                        base_commit,
                        gate: Some(Gate::Fail),
                        gate_failure_kind: Some(GateFailureKind::EmptyBranch),
                        gate_failures: 0,
                        gate_failure_output: Some("diff 为空：任务分支相对基准没有任何变更".into()),
                        conflict_files: Vec::new(),
                        approval: Approval::None,
                        status: MergeStatus::PendingApproval,
                        push_after_merge: false,
                    },
                )
                .await?;
            self.store.increment_gate_failures(&task.id).await?;
            return Ok(PhaseA::GateRan);
        }
        self.store.home().ensure_task_dirs(&task.id)?;
        let diff = Git.diff_range(repo, &range).await?;
        // 决策 416 B：自动落提交的文件会一并进入这份 diff，说明块交代它们从哪来——
        // 用户在审批面看到「没申报过的文件」时，这是唯一的出处。
        let diff = match &autosaved {
            Some((commit, files)) => format!("{}{diff}", autosave_note(commit, files)),
            None => diff,
        };
        std::fs::write(self.store.home().task_file(&task.id, diff_path), &diff)?;

        // (4) 合入前强制闸门：lint（如已配置）+ 测试（决策 139）
        self.ledger()
            .mark_step(run_id, "跑合入前的闸门（lint + 测试）")
            .await;
        let gate = run_code_gate(
            self.store,
            self.settings,
            self.clock,
            self.killer,
            task,
            project,
            run_id,
            Stage::Merge,
            Node::Execute,
            wt,
            true,
        )
        .await?;
        if gate.passed {
            // proposal（决策 96）
            let proposal = MergeResult {
                diff_path: diff_path.into(),
                diff_stats,
                base_commit,
                gate: Some(Gate::Pass),
                gate_failure_kind: None,
                gate_failures: 0,
                gate_failure_output: None,
                conflict_files: auto_resolved.clone(),
                approval: Approval::Pending,
                status: MergeStatus::PendingApproval,
                push_after_merge: false,
            };
            self.store
                .upsert_merge_result(&task.id, diff_path, &proposal)
                .await?;
            self.pend(cursor, PendingKind::MergeApproval, "等待审批合入")
                .await?;
            return Ok(PhaseA::Proposal);
        }

        // 闸门失败：写 gate=fail + 累计计数，交 route_merge 分流（决策 85 / 108 / 139）
        let failure = MergeResult {
            diff_path: diff_path.into(),
            diff_stats,
            base_commit,
            gate: Some(Gate::Fail),
            gate_failure_kind: Some(gate.failure_kind),
            gate_failures: 0, // upsert 路径跳过该字段，真正计数在 increment_gate_failures
            gate_failure_output: Some(gate.output),
            conflict_files: Vec::new(),
            approval: Approval::None,
            status: MergeStatus::PendingApproval,
            push_after_merge: false,
        };
        self.store
            .upsert_merge_result(&task.id, diff_path, &failure)
            .await?;
        // 决策 392 ⑤：**环境类失败不烧 `gate_failures`**。那个计数是「代码改不动」的
        // 信号（决策 108 的单调保留就是为它服务的），环境问题冒充它会把信号污染掉。
        // 与决策 391 对空分支申报「不写闸门失败、不烧 gate_failures」同一款处置。
        if gate.failure_kind != GateFailureKind::Environment {
            self.store.increment_gate_failures(&task.id).await?;
        }
        Ok(PhaseA::GateRan)
    }

    /// 阶段 B：脏工作区检查 → 合入 → 写回（决策 59 / 73 / 97 / 132）。
    /// 基准校验已在 [`MergeFlow::execute`] 的入口完成（决策 96）。
    async fn phase_b(
        &self,
        task: &Task,
        project: &Project,
        cursor: &NodeCursor,
        mut stored: MergeResult,
    ) -> Result<NodeOutput> {
        let (run_id, attempt) =
            begin_run_with_sse(self.sse, &self.ledger(), task, cursor, "system").await?;
        let started = self.clock.now();
        let result = self
            .phase_b_inner(task, project, cursor, &mut stored, run_id)
            .await;
        finish_run_with_sse(
            self.sse,
            &self.ledger(),
            run_id,
            task,
            cursor,
            attempt,
            result.is_err(),
            started,
            result.as_ref().err().map(|e| e.to_string()),
            &RunTokens::default(),
        )
        .await?;
        match result? {
            PhaseB::Merged => Ok(NodeOutput::Route(crate::pipeline::MetadataView::default())),
            // 挂起必须走 NodeOutput::Pending：由 advance_cursor 统一落 pending 并**不推进游标**。
            // 若这里返回 Route，route_merge 会因 approval=approved 放行到 done，
            // 造成「游标已终态但 pending 仍在」的矛盾状态（决策 61 / 132）。
            PhaseB::DirtyWorktree(reason) => Ok(NodeOutput::Pending(reason)),
        }
    }

    async fn phase_b_inner(
        &self,
        task: &Task,
        project: &Project,
        _cursor: &NodeCursor,
        stored: &mut MergeResult,
        run_id: i64,
    ) -> Result<PhaseB> {
        let repo = Path::new(&project.local_path);

        // (1) 目标分支工作区干净检查（决策 61 / 132）：不自动 stash。
        // 只返回挂起意图，由调用方经 NodeOutput::Pending 落库（否则游标会被推进到 done）。
        self.ledger()
            .mark_step(run_id, "检查目标分支工作区是否干净")
            .await;
        if !self.settings.allow_dirty_worktree_merge && Git.is_dirty(repo).await? {
            let reason = PendingReason::new(
                PendingKind::UserDecision,
                Stage::Merge,
                Node::Execute,
                "目标分支工作区不干净，请手动处理后继续合入",
            )
            .with_context(PendingContext::with_kind(
                crate::actions::kinds::DIRTY_WORKTREE,
            ));
            return Ok(PhaseB::DirtyWorktree(reason));
        }

        // (2) 合入（git2 内存合入 + update-ref 语义写回，决策 73 / 97）
        let branch = task
            .branch_name
            .clone()
            .ok_or_else(|| Error::Validation("merge 阶段缺少 branch_name".into()))?;
        self.ledger()
            .mark_step(run_id, "把任务分支合入默认分支")
            .await;
        let outcome = Git
            .merge_into_default_branch(repo, &project.default_branch, &branch)
            .await?;
        tracing::info!(
            task = %task.id,
            fast_forward = outcome.fast_forward,
            commit = %outcome.commit,
            "合入完成"
        );

        // (2.5) push（决策 393）：审批时勾选了才推；无 remote 跳过、不算失败。
        // 放在 status=merged 落库**之前**：push 失败让这次 run 以错误收场（元数据
        // 仍是 approval=approved，重试合并会再走一遍 Phase B——合入幂等（Already up
        // to date）、push 重试），不会出现「状态已 merged 但远端没推上」的静默漂移。
        if stored.push_after_merge {
            self.ledger().mark_step(run_id, "推送默认分支到远端").await;
            match Git
                .push_default_branch(repo, &project.default_branch)
                .await?
            {
                crate::git::PushOutcome::NoRemote => {
                    tracing::info!(task = %task.id, "仓未配置 remote，跳过 push（决策 393）");
                }
                crate::git::PushOutcome::Pushed { remote } => {
                    tracing::info!(
                        task = %task.id,
                        remote = %remote,
                        branch = %project.default_branch,
                        "默认分支已 push 到远端"
                    );
                }
            }
        }

        // (3) status = merged
        stored.approval = Approval::Approved;
        stored.status = MergeStatus::Merged;
        self.store
            .upsert_merge_result(&task.id, &stored.diff_path, stored)
            .await?;
        Ok(PhaseB::Merged)
    }
}

enum PhaseA {
    /// 已生成 proposal 并挂 pending(merge_approval)。
    Proposal,
    /// 闸门已跑且失败，结果在 merge metadata 里，交 route_merge 分流。
    GateRan,
    /// 决策 391：develop 申报零变更成立，已挂 pending(user_decision)——
    /// merge metadata 无闸门结果，route_merge 按 `gate == None` NoOp 退出循环。
    PendingDecision,
    /// rebase 冲突（已 abort），打回 develop。
    Conflict(Vec<String>),
}

/// 阶段 B 的结果：合入完成，或脏工作区挂起等用户处理（决策 61 / 132）。
enum PhaseB {
    Merged,
    DirtyWorktree(PendingReason),
}

/// 测试框架 → 系统闸门命令（§6：按 test_framework 动态构建）。
/// 未配置 → `true`（跳过闸门环节，不阻塞）；带空格的值视作原始命令。
pub fn test_command_for(framework: Option<&str>) -> String {
    match framework {
        None | Some("") => "true".into(),
        Some("cargo") => "cargo test --quiet".into(),
        Some("pytest") => "python3 -m pytest -q".into(),
        Some("npm") | Some("node") => "npm test --silent".into(),
        Some(raw) => raw.into(),
    }
}

/// 从 `git diff --stat` 输出解析汇总行（files_changed / insertions / deletions）。
#[doc(hidden)]
pub fn parse_diff_stats(stat: &str) -> DiffStats {
    let mut stats = DiffStats {
        files_changed: 0,
        insertions: 0,
        deletions: 0,
        file_details: Vec::new(),
    };
    for line in stat.lines().rev() {
        let lower = line.trim_start();
        if lower.contains("changed") || lower.contains("insertion") || lower.contains("deletion") {
            for part in lower.split(',') {
                let part = part.trim();
                let num = part.split(' ').next().and_then(|n| n.parse::<u64>().ok());
                if let Some(n) = num {
                    if part.contains("changed") {
                        stats.files_changed = n;
                    } else if part.contains("insertion") {
                        stats.insertions = n;
                    } else if part.contains("deletion") {
                        stats.deletions = n;
                    }
                }
            }
            break;
        }
    }
    stats
}

/// 决策 416 B：自动落提交的说明块，写在 `merge-proposal.diff` 的**头部**。
///
/// 放头部不是随手选的：`frontend/src/lib/diff.ts::parseUnifiedDiff` 在 `current == null`
/// 时会跳过一切不以 `--- ` 开头的行（头部注记整块被忽略），而追加到**尾部**会被当成上一个
/// 文件的上下文行，且以 `-` 开头的行会被计进 deletions——既污染渲染也污染统计。
fn autosave_note(commit: &str, files: &[String]) -> String {
    let mut out = format!(
        "# {} 合入前自动落提交（决策 416 B）\n\n\
         工作区里有 {} 处未提交的已跟踪改动；rebase 前管线把它们落成提交 `{}`（带 `{}` 标记）。\n\
         它们本来就是本任务的产出，因此下面这份 diff 里也有它们——标记是它们与 agent 手写\n\
         提交之间唯一的区别。\n\n\
         未提交清单：\n",
        crate::git::MERGE_AUTOSAVE_MARK,
        files.len(),
        commit,
        crate::git::MERGE_AUTOSAVE_MARK,
    );
    for f in files {
        out.push_str(&format!("  - `{f}`\n"));
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::SystemClock;
    use crate::home::Home;
    use crate::sse::SseEvent;
    use crate::storage::tasks::NewTask;
    use crate::types::Project;
    use std::sync::Arc;

    struct NoopSse;
    impl SseSink for NoopSse {
        fn emit(&self, _event: SseEvent) {}
    }

    async fn base() -> (tempfile::TempDir, Store, Task) {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = Home::new(tmp.path().join("home"));
        let store = Store::open(home, Arc::new(SystemClock)).await.unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let project = Project {
            id: "p1".into(),
            name: "proj".into(),
            local_path: repo.display().to_string(),
            default_branch: "main".into(),
            language: None,
            test_framework: None,
            lint_command: None,
            agents_md_path: None,
            created_at: chrono::Utc::now(),
        };
        store.create_project(&project).await.unwrap();
        let task = store
            .create_task(&NewTask::new("t1", "任务 t1", "p1"))
            .await
            .unwrap();
        (tmp, store, task)
    }

    fn seed(base_commit: &str, approval: Approval) -> MergeResult {
        MergeResult {
            diff_path: "merge-proposal.diff".into(),
            diff_stats: DiffStats {
                files_changed: 1,
                insertions: 2,
                deletions: 0,
                file_details: Vec::new(),
            },
            base_commit: base_commit.into(),
            gate: Some(Gate::Pass),
            gate_failure_kind: None,
            gate_failures: 0,
            gate_failure_output: None,
            conflict_files: Vec::new(),
            approval,
            status: MergeStatus::PendingApproval,
            push_after_merge: false,
        }
    }

    /// 基准失配 → 审批失效（决策 96）：**不跑全节点循环**、不碰 git、不跑闸门——
    /// 判定与重置在这一层直测（票 04 验收的窄测试）。
    #[tokio::test]
    async fn stale_base_resets_approval_without_the_full_loop() {
        let (_tmp, store, task) = base().await;
        let settings = Settings::default();
        let sse = NoopSse;
        let clock = SystemClock;
        let killer: Arc<dyn ProcessKiller> = Arc::new(crate::process::RealProcessKiller);
        let flow = MergeFlow {
            store: &store,
            settings: &settings,
            sse: &sse,
            clock: &clock,
            killer: &killer,
        };
        store
            .upsert_merge_result(
                &task.id,
                "merge-proposal.diff",
                &seed("oldbase", Approval::Approved),
            )
            .await
            .unwrap();
        let stored = store.merge_metadata(&task.id).await.unwrap().unwrap();

        // 失配 → 重置
        assert!(
            flow.reset_stale_approval(&task.id, &stored, "newbase")
                .await
                .unwrap(),
            "基准变了必须判失配"
        );
        let after = store.merge_metadata(&task.id).await.unwrap().unwrap();
        assert_eq!(after.approval, Approval::None, "approval 重置回 none");
        assert_eq!(
            after.base_commit, "oldbase",
            "只动 approval，存档基准不改写"
        );

        // 一致 → 不动
        assert!(
            !flow
                .reset_stale_approval(&task.id, &after, "oldbase")
                .await
                .unwrap(),
            "基准没变不重置"
        );
    }

    /// `parse_diff_stats` 随迁（票 04）：零依赖单测，语义逐字（B3 的第三条）。
    #[test]
    fn parse_diff_stats_still_parses_the_summary_line() {
        let stats = parse_diff_stats(" src/a.rs | 2 ++\n 1 file changed, 2 insertions(+)\n");
        assert_eq!(stats.files_changed, 1);
        assert_eq!(stats.insertions, 2);
        assert_eq!(stats.deletions, 0);
    }

    /// 决策 416 B：自愈注记必须是 diff **头部**的一块「解析器看不见」的文本。
    ///
    /// 两个牙齿缺一个都会红：
    /// - 注记行若以 `diff --git ` 或 `--- ` 开头，`frontend/src/lib/diff.ts::parseUnifiedDiff`
    ///   会把注记误当文件头——`files_changed` 与渲染全部失真（这正是它必须写在头部、
    ///   且行首不能长那样的原因）；
    /// - 注记丢了 `[autosave]` 标记或提交 SHA，审批面上就没法把机器提交与 agent 手写提交
    ///   分开（`git log --grep` 也认不出）。
    #[test]
    fn autosave_note_is_invisible_to_the_diff_parser_and_carries_the_marker() {
        let note = autosave_note(
            "abc1234",
            &["src/lib.rs".into(), "tests/acceptance.rs".into()],
        );
        assert!(note.starts_with('#'), "注记应是头部 markdown 块");
        assert!(
            note.contains(crate::git::MERGE_AUTOSAVE_MARK),
            "注记必须带 [autosave] 标记"
        );
        assert!(note.contains("abc1234"), "注记必须带自愈提交 SHA");
        assert!(note.contains("`src/lib.rs`"), "注记必须列全自愈文件");
        assert!(
            note.contains("`tests/acceptance.rs`"),
            "注记必须列全自愈文件"
        );
        for line in note.lines() {
            assert!(
                !line.starts_with("diff --git ") && !line.starts_with("--- "),
                "注记行不得冒充 diff 文件头（parseUnifiedDiff 会当真）：{line}"
            );
        }
        // 空文件清单也得是合法块（diff_stats.files_changed>0 才到这里，但函数本身不设前提）
        let empty = autosave_note("deadbeef", &[]);
        assert!(empty.contains(crate::git::MERGE_AUTOSAVE_MARK));
        assert!(empty.starts_with('#'));
    }
}
