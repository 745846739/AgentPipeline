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

use super::executor::{
    begin_run_with_sse, finish_run_with_sse, pend_reason, project_or_err, run_code_gate, NodeOutput,
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
        let reason = PendingReason::new(kind, cursor.stage, cursor.node, message);
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
                        PhaseA::Proposal | PhaseA::GateRan => {
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
                        Error::Validation(
                            "approval=approved 但没有 merge_result（决策 119 契约）".into(),
                        )
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
        let failed = !matches!(result, Ok(PhaseA::Proposal) | Ok(PhaseA::GateRan));
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
            // 空差异 = 没有可合入的变更；按闸门失败分流处理，不静默合入
            self.store
                .upsert_merge_result(
                    &task.id,
                    diff_path,
                    &MergeResult {
                        diff_path: diff_path.into(),
                        diff_stats,
                        base_commit,
                        gate: Some(Gate::Fail),
                        gate_failure_kind: Some(GateFailureKind::Test),
                        gate_failures: 0,
                        gate_failure_output: Some("diff 为空：任务分支相对基准没有任何变更".into()),
                        conflict_files: Vec::new(),
                        approval: Approval::None,
                        status: MergeStatus::PendingApproval,
                    },
                )
                .await?;
            self.store.increment_gate_failures(&task.id).await?;
            return Ok(PhaseA::GateRan);
        }
        self.store.home().ensure_task_dirs(&task.id)?;
        let diff = Git.diff_range(repo, &range).await?;
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
        };
        self.store
            .upsert_merge_result(&task.id, diff_path, &failure)
            .await?;
        self.store.increment_gate_failures(&task.id).await?;
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
}
