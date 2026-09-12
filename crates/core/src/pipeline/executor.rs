//! 自定义 executor（docs/implementation.md §11.2，docs/pipeline-spec.md §6–§9）。
//!
//! 职责边界：executor 是 DAG 执行引擎——并发驱动一个任务的全部活跃游标，
//! 把节点结果交给路由函数推进游标；定时职责（超时 / 冲突恢复 / 准入）归 scheduler。
//!
//! 关键语义（出处见行内标注）：
//! - **单执行者保证**（决策 36）：进程内 `Mutex<HashSet<task_id>>` 非阻塞去重 +
//!   DB `executor_owner` 乐观锁兜底跨进程；
//! - **单游标失败不传播**（决策 89）：一条游标的节点失败只把该游标置 pending，
//!   另一分支继续跑完本阶段后停在 join 边界；
//! - **`waiting_join` 只由 `advance_cursor` 写入**（决策 107）；join 由
//!   [`Executor::advance_join`] 在所有游标到界后执行一次（决策 83 / G5）；
//! - 纯代码节点同样落 run 行（`agent_type = "system"`，决策 99 / 114）；
//! - agent 节点按 `agent_retry_max` 干净对话重试，耗尽才 pending（决策 33 / G13）。

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

use futures::StreamExt;

use crate::agent::client::{LlmClient, LlmRequest, Message, ToolDef};
use crate::agent::metadata::parse_metadata;
use crate::agent::prompts::{
    build_system_prompt, build_user_prompt, load_agents_context, prompt_template_hash,
    render_template, resolve_persona, PromptSegments, TemplateVars,
};
use crate::agent::templates::{system_template, user_template};
use crate::agent::tools::{
    CommandFinish, CommandRecorder, CommandStart, ToolCallContext, ToolExecutor,
};
use crate::agent::{
    effective_skills, effective_tools, file_policy::FileToolPolicy, submit_metadata_tool,
    BUILTIN_TOOLS,
};
use crate::config::Settings;
use crate::git::{Git, RebaseOutcome};
use crate::home::Home;
use crate::process::ProcessKiller;
use crate::sse::{SseEvent, SseSink, ToolPhase};
use crate::storage::observability::{NewRun, RunOutcome};
use crate::types::{
    Approval, CommandSource, DiffStats, EdgeKind, Gate, GateFailureKind, MergeResult, MergeStatus,
    Node, NodeCursor, NodeStatus, PendingContext, PendingKind, PendingReason, Project, ReviewMode,
    Stage, StageConfig, SyncDecision, SyncDecisionKind, Task, TestResult,
};
use crate::{Error, Result};

/// 闸门结果（决策 62 / 139）：非零退出是**闸门结果**，不是节点错误。
struct GateOutcome {
    passed: bool,
    failure_kind: GateFailureKind,
    output: String,
}

// ─────────────────────────────── 单执行者注册表（决策 36）───────────────────────────────

static EXECUTOR_REGISTRY: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

struct ExecutorGuard(String);

impl Drop for ExecutorGuard {
    fn drop(&mut self) {
        EXECUTOR_REGISTRY.lock().unwrap().remove(&self.0);
    }
}

/// 非阻塞抢占：已有 executor 在跑同一任务时返回 `None`（调用方直接退出）。
fn try_acquire(task_id: &str) -> Option<ExecutorGuard> {
    let mut set = EXECUTOR_REGISTRY.lock().unwrap();
    if set.contains(task_id) {
        return None;
    }
    set.insert(task_id.to_string());
    Some(ExecutorGuard(task_id.to_string()))
}

// ─────────────────────────────── 阶段产出类型定名（§4.2）───────────────────────────────

const OUTPUT_DESIGN_DOC: &str = "design_doc";
const OUTPUT_DEV_DOC: &str = "dev_doc";
const OUTPUT_TEST_SCENARIOS: &str = "test_scenarios";
const OUTPUT_REVIEW_REPORT: &str = "review_report";
const OUTPUT_TEST_REPORT: &str = "test_report";
const OUTPUT_CODE_CHANGES: &str = "code_changes";
const OUTPUT_SYNC_DECISION: &str = "sync_decision";

// ─────────────────────────────── 执行器 ───────────────────────────────

/// executor 的协作依赖。`llm` 是测试接缝（决策 142/143）：生产传真实适配层，
/// 测试传 testkit 的 FakeAgent——工具层始终真实执行（决策 148）。
pub struct Executor {
    store: crate::storage::Store,
    settings: Settings,
    sse: Arc<dyn SseSink>,
    llm: Arc<dyn LlmClient>,
    killer: Arc<dyn ProcessKiller>,
}

impl Executor {
    pub fn new(
        store: crate::storage::Store,
        settings: Settings,
        sse: Arc<dyn SseSink>,
        llm: Arc<dyn LlmClient>,
        killer: Arc<dyn ProcessKiller>,
    ) -> Self {
        let mut store = store;
        store.set_conversation_max_chars(settings.conversation_max_chars);
        Executor {
            store,
            settings,
            sse,
            llm,
            killer,
        }
    }

    /// 入口：抢占单执行者 → 跑循环 → 释放。
    pub async fn run(&self, task_id: &str) -> Result<()> {
        let _guard = match try_acquire(task_id) {
            Some(g) => g,
            None => return Ok(()), // 已有 executor 在跑，直接返回（决策 36）
        };
        let owner = format!("executor:{}", ulid::Ulid::new());
        if !self.store.try_claim_executor(task_id, &owner).await? {
            return Ok(()); // DB 乐观锁被占（跨进程场景），跳过
        }
        let result = self.run_inner(task_id).await;
        self.store.release_executor(task_id).await?;
        result
    }

    /// 核心循环（§11.2 伪码）。
    async fn run_inner(&self, task_id: &str) -> Result<()> {
        loop {
            let task = self.store.get_task(task_id).await?;
            if task.status.is_terminal() {
                return Ok(());
            }
            // queued / waiting 由准入路径启动，executor 不越权（决策 98）
            if !matches!(
                task.status,
                crate::types::TaskStatus::Running | crate::types::TaskStatus::Pending
            ) {
                return Ok(());
            }

            let cursors = self.store.load_live_cursors(task_id).await?;
            let pending: Vec<&NodeCursor> = cursors.iter().filter(|c| c.is_pending()).collect();
            let runnable: Vec<NodeCursor> = cursors
                .iter()
                .filter(|c| c.is_runnable())
                .cloned()
                .collect();

            if runnable.is_empty() {
                if !pending.is_empty() {
                    // 有 pending、无可推进 → 任务整体暂停等 resume（决策 82）
                    self.store.sync_task_projection(task_id).await?;
                    return Ok(());
                }
                if !crate::pipeline::is_join_ready(&cursors) {
                    return Ok(()); // 防御性退出（理论上不可达）
                }
                self.advance_join(&task).await?;
                continue;
            }

            // 并发驱动所有可继续游标（决策 81）
            let task_ref = &task;
            let this = &self;
            let results: Vec<(NodeCursor, Result<NodeOutput>)> = futures::stream::iter(runnable)
                .map(move |c| {
                    let cursor = c.clone();
                    async move {
                        let out = this.execute_node(task_ref, &cursor).await;
                        (cursor, out)
                    }
                })
                .buffer_unordered(4)
                .collect()
                .await;

            let before = self.cursor_snapshot(task_id).await?;
            for (cursor, outcome) in results {
                match outcome {
                    Ok(output) => {
                        if let Err(e) = self.advance_cursor(&task, &cursor, output).await {
                            // 推进失败同样只阻塞本游标（决策 89）
                            self.pend_cursor(&cursor, PendingKind::RetryExhausted, e.to_string())
                                .await?;
                        }
                    }
                    Err(node_error) => {
                        // 单游标失败不传播（决策 89）
                        self.pend_cursor(
                            &cursor,
                            PendingKind::RetryExhausted,
                            node_error.to_string(),
                        )
                        .await?;
                    }
                }
            }

            // 终态判定（done.execute 已 mark_terminal）
            if self.store.get_task(task_id).await?.status.is_terminal() {
                return Ok(());
            }
            // 防御：本轮没有任何游标推进（如 merge NoOp 且未挂 pending）→ 退出，避免自旋
            let after = self.cursor_snapshot(task_id).await?;
            if before == after {
                return Ok(());
            }
        }
    }

    async fn cursor_snapshot(&self, task_id: &str) -> Result<Vec<(String, String, Stage, Node)>> {
        Ok(self
            .store
            .load_live_cursors(task_id)
            .await?
            .into_iter()
            .map(|c| (c.cursor_id, format!("{:?}", c.status), c.stage, c.node))
            .collect())
    }

    /// 把某条游标置为 pending 并同步投影 + SSE（决策 82）。
    async fn pend_cursor(
        &self,
        cursor: &NodeCursor,
        kind: PendingKind,
        message: impl Into<String>,
    ) -> Result<()> {
        let reason = PendingReason::new(kind, cursor.stage, cursor.node, message);
        self.store
            .set_cursor_pending(&cursor.cursor_id, &reason)
            .await?;
        self.store.sync_task_projection(&cursor.task_id).await?;
        self.sse.emit(SseEvent::Pending {
            task_id: cursor.task_id.clone(),
            branch: cursor.branch.clone(),
            cursor_id: cursor.cursor_id.clone(),
            reason,
        });
        Ok(())
    }

    // ─────────────────────── 节点分发 ───────────────────────

    /// 执行一个节点的"工作"部分，返回交给路由的结论。
    async fn execute_node(&self, task: &Task, cursor: &NodeCursor) -> Result<NodeOutput> {
        let (stage, node) = (cursor.stage, cursor.node);
        match (stage, node) {
            // ── 纯代码节点（G7；决策 99/114：同样落 run 行，agent_type = system）──
            (Stage::Init, Node::Execute) => self.init_execute(task, cursor).await,
            (Stage::Done, Node::Execute) => self.done_execute(task, cursor).await,
            (Stage::SyncCheck, Node::Execute) => {
                // join 由 advance_join 统一执行（决策 107），游标永远不该指向这里
                Err(Error::Validation(
                    "sync-check 不占游标行（决策 107）".into(),
                ))
            }
            (Stage::Merge, Node::Execute) => self.merge_execute(task, cursor).await,

            // ── 纯代码 validate_output（决策 62）──
            (Stage::Develop, Node::ValidateOutput) => self.develop_code_gate(task, cursor).await,
            (Stage::Test, Node::ValidateOutput) => self.test_code_gate(task, cursor).await,
            (Stage::Review, Node::ValidateOutput) => self.review_verdict(task, cursor).await,

            // ── agent 节点 ──
            _ => self.agent_node(task, cursor).await,
        }
    }

    // ─────────────────────── 纯代码节点 ───────────────────────

    /// init.execute：创建 worktree 隔离工作区（§6；worktree 已存在则复用，§8）。
    async fn init_execute(&self, task: &Task, cursor: &NodeCursor) -> Result<NodeOutput> {
        let project = self.project(&task.project_id).await?;
        let (run_id, attempt) = self.begin_run(task, cursor, "system").await?;
        let started = Instant::now();
        let result = self.do_init(task, &project).await;
        self.finish_run(
            run_id,
            task,
            cursor,
            attempt,
            result.is_err(),
            started.elapsed().as_millis() as u64,
            result.as_ref().err().map(|e| e.to_string()),
            &RunTokens::default(),
        )
        .await?;
        result?;
        Ok(NodeOutput::Route(crate::pipeline::MetadataView::default()))
    }

    async fn do_init(&self, task: &Task, project: &Project) -> Result<()> {
        let worktree = self.store.home().worktree_path(&task.id);
        // 目标仓库脏不阻塞，仅记录警告（决策 61）
        if Git.is_dirty(Path::new(&project.local_path)).await? {
            tracing::warn!(task = %task.id, "项目工作区有未提交改动（不阻塞，决策 61）");
        }
        Git.init_worktree(
            Path::new(&project.local_path),
            &task.id,
            &worktree,
            &project.default_branch,
        )
        .await?;
        self.store
            .set_task_worktree(
                &task.id,
                &worktree.display().to_string(),
                &Git::branch_for(&task.id),
            )
            .await
    }

    /// done.execute：按 merge_result.status 收尾——清理 worktree 与分支，置终态（§6 / 决策 3）。
    async fn done_execute(&self, task: &Task, cursor: &NodeCursor) -> Result<NodeOutput> {
        let (run_id, attempt) = self.begin_run(task, cursor, "system").await?;
        let started = Instant::now();
        let result = self.do_done(task).await;
        self.finish_run(
            run_id,
            task,
            cursor,
            attempt,
            result.is_err(),
            started.elapsed().as_millis() as u64,
            result.as_ref().err().map(|e| e.to_string()),
            &RunTokens::default(),
        )
        .await?;
        result?;
        self.sse.emit(SseEvent::TaskDone {
            task_id: task.id.clone(),
            branch: cursor.branch.clone(),
        });
        Ok(NodeOutput::Route(crate::pipeline::MetadataView::default()))
    }

    async fn do_done(&self, task: &Task) -> Result<()> {
        let merged = self
            .store
            .merge_metadata(&task.id)
            .await?
            .map(|m| m.status == MergeStatus::Merged)
            .unwrap_or(false);
        if !merged {
            return Err(Error::Validation(
                "done 需要 merge_result.status = merged（未合入不得进入终态）".into(),
            ));
        }
        if let Some(worktree) = &task.worktree_path {
            let project = self.project(&task.project_id).await?;
            Git.remove_worktree(Path::new(&project.local_path), Path::new(worktree), true)
                .await?;
            if let Some(branch) = &task.branch_name {
                Git.delete_branch(Path::new(&project.local_path), branch)
                    .await?;
            }
        }
        self.store
            .mark_terminal(&task.id, crate::types::TaskStatus::Done)
            .await?;
        self.store.refresh_task_totals(&task.id).await
    }

    /// merge.execute（§6 merge；决策 72 / 85 / 95 / 96 / 97 / 108 / 119 / 139）。
    async fn merge_execute(&self, task: &Task, cursor: &NodeCursor) -> Result<NodeOutput> {
        let project = self.project(&task.project_id).await?;
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
                        self.pend_cursor(cursor, PendingKind::MergeApproval, "等待审批合入")
                            .await?;
                    }
                    return Ok(NodeOutput::Route(crate::pipeline::MetadataView::default()));
                }
                Approval::None | Approval::Returned => {
                    // 阶段 A 内部已解析 base_ref，冲突反馈需要它，这里取一次
                    let repo = Path::new(&project.local_path);
                    let base_ref = Git.base_ref(repo, &project.default_branch).await?;
                    let outcome = self
                        .merge_phase_a(task, &project, cursor, &worktree)
                        .await?;
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
                    let mut stored =
                        self.store.merge_metadata(&task.id).await?.ok_or_else(|| {
                            Error::Validation(
                                "approval=approved 但没有 merge_result（决策 119 契约）".into(),
                            )
                        })?;
                    if current_base != stored.base_commit {
                        stored.approval = Approval::None;
                        self.store
                            .upsert_merge_result(&task.id, &stored.diff_path, &stored)
                            .await?; // gate_failures 由 upsert 保留（决策 108）
                        tracing::info!(task = %task.id, "基准已前移，approval 重置回阶段 A（决策 96）");
                        current = Approval::None;
                        continue;
                    }
                    return self.merge_phase_b(task, &project, cursor, stored).await;
                }
            }
        }
    }

    /// 阶段 A：rebase → 闸门 → 生成 proposal → pending(merge_approval)。
    async fn merge_phase_a(
        &self,
        task: &Task,
        project: &Project,
        cursor: &NodeCursor,
        worktree: &str,
    ) -> Result<PhaseA> {
        let (run_id, attempt) = self.begin_run(task, cursor, "system").await?;
        let started = Instant::now();
        let result = self
            .merge_phase_a_inner(task, project, cursor, worktree, run_id)
            .await;
        let failed = !matches!(result, Ok(PhaseA::Proposal) | Ok(PhaseA::GateRan));
        self.finish_run(
            run_id,
            task,
            cursor,
            attempt,
            failed,
            started.elapsed().as_millis() as u64,
            result.as_ref().err().map(|e| e.to_string()),
            &RunTokens::default(),
        )
        .await?;
        result
    }

    async fn merge_phase_a_inner(
        &self,
        task: &Task,
        project: &Project,
        cursor: &NodeCursor,
        worktree: &str,
        run_id: i64,
    ) -> Result<PhaseA> {
        let repo = Path::new(&project.local_path);
        let wt = Path::new(worktree);
        let base_ref = Git.base_ref(repo, &project.default_branch).await?;

        // (2) rebase 到基准（决策 74 / 96）
        match Git.rebase_onto(wt, &base_ref).await? {
            RebaseOutcome::Conflict { files } => {
                // 无法自动解决 → 系统先 abort 恢复干净状态，再打回 develop（决策 74）
                Git.rebase_abort(wt).await?;
                return Ok(PhaseA::Conflict(files));
            }
            RebaseOutcome::Clean { .. } => {}
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
        let gate = self
            .run_code_gate(task, project, run_id, Stage::Merge, Node::Execute, wt, true)
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
                conflict_files: Vec::new(),
                approval: Approval::Pending,
                status: MergeStatus::PendingApproval,
            };
            self.store
                .upsert_merge_result(&task.id, diff_path, &proposal)
                .await?;
            self.pend_cursor(cursor, PendingKind::MergeApproval, "等待审批合入")
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
    /// 基准校验已在 [`Executor::merge_execute`] 的入口完成（决策 96）。
    async fn merge_phase_b(
        &self,
        task: &Task,
        project: &Project,
        cursor: &NodeCursor,
        mut stored: MergeResult,
    ) -> Result<NodeOutput> {
        let (run_id, attempt) = self.begin_run(task, cursor, "system").await?;
        let started = Instant::now();
        let result = self
            .merge_phase_b_inner(task, project, cursor, &mut stored)
            .await;
        self.finish_run(
            run_id,
            task,
            cursor,
            attempt,
            result.is_err(),
            started.elapsed().as_millis() as u64,
            result.as_ref().err().map(|e| e.to_string()),
            &RunTokens::default(),
        )
        .await?;
        result?;
        Ok(NodeOutput::Route(crate::pipeline::MetadataView::default()))
    }

    async fn merge_phase_b_inner(
        &self,
        task: &Task,
        project: &Project,
        cursor: &NodeCursor,
        stored: &mut MergeResult,
    ) -> Result<()> {
        let repo = Path::new(&project.local_path);

        // (1) 目标分支工作区干净检查（决策 61 / 132）：不自动 stash
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
            self.store
                .set_cursor_pending(&cursor.cursor_id, &reason)
                .await?;
            self.store.sync_task_projection(&task.id).await?;
            return Ok(());
        }

        // (2) 合入（git2 内存合入 + update-ref 语义写回，决策 73 / 97）
        let branch = task
            .branch_name
            .clone()
            .ok_or_else(|| Error::Validation("merge 阶段缺少 branch_name".into()))?;
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
        Ok(())
    }

    // ─────────────────────── 纯代码 validate_output（决策 62）───────────────────────

    /// develop.validate_output：lint（如配置）+ 单元测试，全过才放行（§6 / 决策 139）。
    async fn develop_code_gate(&self, task: &Task, cursor: &NodeCursor) -> Result<NodeOutput> {
        let project = self.project(&task.project_id).await?;
        let (run_id, attempt) = self.begin_run(task, cursor, "system").await?;
        let started = Instant::now();
        let worktree = task.worktree_path.clone().unwrap_or_default();
        let gate = self
            .run_code_gate(
                task,
                &project,
                run_id,
                Stage::Develop,
                Node::ValidateOutput,
                Path::new(&worktree),
                true,
            )
            .await;
        self.finish_run(
            run_id,
            task,
            cursor,
            attempt,
            gate.as_ref().map(|g| !g.passed).unwrap_or(true),
            started.elapsed().as_millis() as u64,
            gate.as_ref().err().map(|e| e.to_string()),
            &RunTokens::default(),
        )
        .await?;
        let gate = gate?;
        Ok(NodeOutput::Route(crate::pipeline::MetadataView::passed(
            gate.passed,
        )))
    }

    /// test.validate_output：读 execute 提交的 test_result 路由（决策 62 / 85）。
    async fn test_code_gate(&self, task: &Task, cursor: &NodeCursor) -> Result<NodeOutput> {
        let (run_id, attempt) = self.begin_run(task, cursor, "system").await?;
        let meta = self
            .store
            .stage_output_metadata(&task.id, Stage::Test, OUTPUT_TEST_REPORT)
            .await?
            .ok_or_else(|| {
                Error::Validation("test.validate_output 缺少 test_result 元数据".into())
            })?;
        let result: TestResult = serde_json::from_value(meta)?;
        self.finish_run(
            run_id,
            task,
            cursor,
            attempt,
            false,
            0,
            None,
            &RunTokens::default(),
        )
        .await?;
        Ok(NodeOutput::Route(
            crate::pipeline::MetadataView::from_test_result(&result),
        ))
    }

    /// review.validate_output：读 execute 提交的 approved 判定（§6；纯代码逻辑）。
    async fn review_verdict(&self, task: &Task, cursor: &NodeCursor) -> Result<NodeOutput> {
        let (run_id, attempt) = self.begin_run(task, cursor, "system").await?;
        let meta = self
            .store
            .stage_output_metadata(&task.id, Stage::Review, OUTPUT_REVIEW_REPORT)
            .await?
            .ok_or_else(|| Error::Validation("review.validate_output 缺少评审元数据".into()))?;
        let approved = meta
            .get("approved")
            .and_then(|v| v.as_bool())
            .ok_or_else(|| Error::Validation("评审元数据缺少 approved 字段".into()))?;
        self.finish_run(
            run_id,
            task,
            cursor,
            attempt,
            false,
            0,
            None,
            &RunTokens::default(),
        )
        .await?;
        Ok(NodeOutput::Route(crate::pipeline::MetadataView::passed(
            approved,
        )))
    }

    // ─────────────────────── agent 节点 ───────────────────────

    /// agent 节点：独立对话（决策 33）+ 工具真实执行（决策 148）+
    /// `agent_retry_max` 干净对话重试（决策 33 / G13）。
    async fn agent_node(&self, task: &Task, cursor: &NodeCursor) -> Result<NodeOutput> {
        let kind = AgentNodeKind::of(cursor.stage, cursor.node).ok_or_else(|| {
            Error::Validation(format!("{}.{} 不是 agent 节点", cursor.stage, cursor.node))
        })?;
        let project = self.project(&task.project_id).await?;

        let mut last_error = String::new();
        for _ in 0..self.settings.agent_retry_max {
            let attempt = self
                .next_attempt(&task.id, cursor.stage, cursor.node)
                .await?;
            let run_id = self
                .store
                .insert_run(&NewRun {
                    task_id: task.id.clone(),
                    cursor_id: cursor.cursor_id.clone(),
                    stage: cursor.stage,
                    node: cursor.node,
                    attempt,
                    agent_type: "main".into(),
                    parent_run_id: None,
                    prompt_template_hash: None,
                    process_group_id: None,
                })
                .await?;
            self.sse.emit(SseEvent::NodeStarted {
                task_id: task.id.clone(),
                branch: cursor.branch.clone(),
                stage: cursor.stage,
                node: cursor.node,
                attempt,
                run_id,
            });
            let started = Instant::now();
            match self
                .agent_attempt(task, &project, cursor, kind, run_id, attempt)
                .await
            {
                Ok((output, tokens)) => {
                    // run 行与 NodeFinished 事件的 token 计量（决策 100）
                    self.finish_run(
                        run_id,
                        task,
                        cursor,
                        attempt,
                        false,
                        started.elapsed().as_millis() as u64,
                        None,
                        &tokens,
                    )
                    .await?;
                    self.store.refresh_task_totals(&task.id).await?;
                    return Ok(output);
                }
                Err(e) => {
                    last_error = e.to_string();
                    self.finish_run(
                        run_id,
                        task,
                        cursor,
                        attempt,
                        true,
                        started.elapsed().as_millis() as u64,
                        Some(last_error.clone()),
                        &RunTokens::default(),
                    )
                    .await?;
                    // 干净对话重试：messages 不跨 attempt 保留（决策 33）
                }
            }
        }
        Err(Error::Validation(format!(
            "agent 节点 {}.{} 重试耗尽：{last_error}",
            cursor.stage, cursor.node
        )))
    }

    /// 组装 §10.3 / G12 模板变量。上游产出取已登记的 stage output 路径；
    /// 缺失时回退到任务目录下的规范文件名（agent 自行探测存在性，决策 115）。
    async fn template_vars(
        &self,
        task: &Task,
        project: &Project,
        worktree: &str,
        task_dir: &str,
    ) -> Result<TemplateVars> {
        let framework = project.test_framework.as_deref();
        let stored_path = |output: Option<crate::types::StageOutput>, default: &str| {
            output
                .map(|o| format!("{task_dir}/{}", o.file_path))
                .unwrap_or_else(|| format!("{task_dir}/{default}"))
        };
        let design = self
            .store
            .get_stage_output(&task.id, Stage::ArchitectDesign, OUTPUT_DESIGN_DOC)
            .await?;
        let dev = self
            .store
            .get_stage_output(&task.id, Stage::DevelopDesign, OUTPUT_DEV_DOC)
            .await?;
        let scenarios = self
            .store
            .get_stage_output(&task.id, Stage::TestDesign, OUTPUT_TEST_SCENARIOS)
            .await?;
        let code_changes = self
            .store
            .stage_output_metadata(&task.id, Stage::Develop, OUTPUT_CODE_CHANGES)
            .await?;
        let (changed, unit_tests) = code_changes_lists(code_changes.as_ref());
        Ok(TemplateVars {
            test_command: test_command_for(framework),
            test_file_convention: test_file_convention(framework).to_string(),
            test_framework: framework.unwrap_or("未知").to_string(),
            worktree_path: worktree.to_string(),
            task_dir: task_dir.to_string(),
            task_title: task.title.clone(),
            task_description: task.description.clone(),
            design_doc_path: stored_path(design, "design.md"),
            dev_doc_path: stored_path(dev, "dev-plan.md"),
            test_scenarios_path: stored_path(scenarios, "test-scenarios.md"),
            changed_files: changed,
            unit_test_files: unit_tests,
        })
    }

    /// 单次 agent attempt：prompt 组装 → 工具循环 → 元数据抽取 → 节点后处理。
    /// 返回（结论，token 计量）。
    async fn agent_attempt(
        &self,
        task: &Task,
        project: &Project,
        cursor: &NodeCursor,
        kind: AgentNodeKind,
        run_id: i64,
        attempt: u32,
    ) -> Result<(NodeOutput, RunTokens)> {
        let home = self.store.home().clone();
        home.ensure_task_dirs(&task.id)?;
        let worktree = task
            .worktree_path
            .clone()
            .unwrap_or_else(|| home.worktree_path(&task.id).display().to_string());
        let task_dir = home.task_dir(&task.id).display().to_string();

        let policy = FileToolPolicy::new(vec![worktree.clone().into(), task_dir.clone().into()]);
        let tools = ToolExecutor::new(
            home.clone(),
            policy,
            self.settings.clone(),
            self.killer.clone(),
        )
        .with_recorder(Arc::new(self.store.clone()));

        // 阶段配置消费（§10.6.3 / 决策 22 / 46 / 111）：persona、采样参数、工具与技能增量
        let stage_cfg = self.store.get_stage_config(cursor.stage.as_str()).await?;
        let persona = resolve_stage_persona(&home, stage_cfg.as_ref(), cursor.stage, cursor.node)?;
        let declared_tools =
            json_string_list(stage_cfg.as_ref().and_then(|c| c.tools_json.as_ref()));
        let skills = effective_skills(&json_string_list(
            stage_cfg.as_ref().and_then(|c| c.skills_json.as_ref()),
        ));

        // system prompt：[基线前言][工作目录(G12)][AGENTS.md(G3)][persona][技能][格式规则]
        let system_prompt = build_system_prompt(
            &load_agents_context(
                Path::new(&project.local_path),
                project.language.as_deref(),
                project.test_framework.as_deref(),
            ),
            &persona,
            &workdirs_line(&worktree, &task_dir),
            &skills,
        );
        let vars = self
            .template_vars(task, project, &worktree, &task_dir)
            .await?;
        // user prompt：§10.3 节点模板 + G12 环境路径块 + 可选追加段（首轮为空不渲染）
        let segments = PromptSegments {
            backtrack_feedback: backtrack_feedback_segment(
                &home,
                &task.id,
                cursor.stage,
                cursor.node,
            ),
            ..Default::default()
        };
        let user_prompt = build_user_prompt(
            &format!(
                "{}\n\n## 环境路径\n{}",
                render_template(user_template(cursor.stage, cursor.node), &vars),
                workdirs_line(&worktree, &task_dir)
            ),
            &segments,
        );
        let template_hash = prompt_template_hash(&system_prompt);
        self.store
            .set_run_template_hash(run_id, &template_hash)
            .await?;

        let mut messages: Vec<Message> = Vec::new();
        let mut tool_failures = 0u32;
        let mut submitted: Option<serde_json::Value> = None;
        let mut tokens = RunTokens::default();

        loop {
            let req = LlmRequest {
                stage: cursor.stage,
                node: cursor.node,
                attempt,
                system_prompt: system_prompt.clone(),
                user_prompt: user_prompt.clone(),
                messages: messages.clone(),
                tools: tool_defs(kind, &declared_tools),
                temperature: stage_cfg.as_ref().and_then(|c| c.temperature),
                max_tokens: stage_cfg.as_ref().and_then(|c| c.max_tokens),
                // 任务级 provider 覆盖（决策 105）；阶段配置 / 系统默认由生产适配器解析
                provider_id: task.model_override.clone(),
                run: Some(crate::agent::client::RunContext {
                    task_id: task.id.clone(),
                    branch: cursor.branch.clone(),
                    run_id,
                    agent_type: "main".into(),
                }),
            };
            let response = self.llm.complete(req).await?;
            tokens.add(&response);
            messages.push(Message::assistant(
                response.content.clone(),
                response.tool_calls.clone(),
            ));
            self.store.touch_run_heartbeat(run_id).await?;

            if response.tool_calls.is_empty() {
                break;
            }
            for call in &response.tool_calls {
                let summary = args_summary(&call.arguments);
                self.emit_tool_event(task, cursor, run_id, &call.name, ToolPhase::Start, &summary);
                let ctx = ToolCallContext {
                    task_id: task.id.clone(),
                    stage: cursor.stage,
                    node: cursor.node,
                    worktree_path: worktree.clone().into(),
                    task_dir: task_dir.clone().into(),
                    run_id: Some(run_id),
                    command_source: CommandSource::Agent,
                    default_cwd: Some(worktree.clone().into()),
                };
                match tools.execute(call, &ctx).await {
                    Ok(outcome) => {
                        if let Some(m) = outcome.metadata {
                            submitted = Some(m);
                        }
                        messages.push(Message::tool_result(call, outcome.content));
                        self.emit_tool_event(
                            task,
                            cursor,
                            run_id,
                            &call.name,
                            ToolPhase::End,
                            &summary,
                        );
                    }
                    Err(e) => {
                        // error 阶段的 args_summary 仍是参数摘要（决策 123）；
                        // 错误详情走 messages 的 tool_result（已脱敏）
                        self.emit_tool_event(
                            task,
                            cursor,
                            run_id,
                            &call.name,
                            ToolPhase::Error,
                            &summary,
                        );
                        // G13：工具失败在 agent loop 内重试，只计 tool_retry_max 次
                        tool_failures += 1;
                        messages.push(Message::tool_result(call, format!("工具执行失败：{e}")));
                        if tool_failures > self.settings.tool_retry_max {
                            return Err(Error::Validation(format!(
                                "工具失败超过 tool_retry_max：{e}"
                            )));
                        }
                    }
                }
            }
        }

        // 元数据抽取（决策 33：解析/校验失败计入节点重试）
        let value = match submitted {
            Some(v) => v,
            None => {
                let final_resp = crate::agent::client::AgentResponse {
                    content: messages.iter().rev().find_map(|m| m.content.clone()),
                    ..Default::default()
                };
                let extracted = crate::agent::metadata::extract_metadata(&final_resp);
                extracted.value.ok_or_else(|| {
                    Error::Validation(extracted.error.unwrap_or_else(|| "缺少结构化元数据".into()))
                })?
            }
        };
        kind.validate(&value)?;

        // 会话落库（§12.4.3；1:1 对调 LLM 的 run，决策 99）
        let msgs = serde_json::to_value(&messages)?;
        self.store
            .insert_conversation(
                &task.id,
                run_id,
                cursor.stage,
                cursor.node,
                attempt,
                "main",
                None,
                &msgs,
                Some(&value),
                tokens.prompt,
                tokens.completion,
            )
            .await?;
        self.store.refresh_task_totals(&task.id).await?;

        let output = kind.post_process(self, task, value).await?;
        Ok((output, tokens))
    }

    // ─────────────────────── join（决策 83 / 107 / G5）───────────────────────────────

    /// 汇聚节点 sync-check：所有游标到界后执行**一次**；不占游标行，
    /// run 行的 cursor_id 指向同事务新建的 main 游标（决策 107 / 113）。
    async fn advance_join(&self, task: &Task) -> Result<()> {
        let cursors = self.store.load_live_cursors(&task.id).await?;
        let decision = self.compute_sync_decision(task, &cursors).await?;

        let main = if decision.decision == SyncDecisionKind::Proceed {
            self.store.merge_cursors_to_develop(&task.id).await?
        } else {
            // backtrack_cursors 单事务内归档双游标 + 插回 main + 设计文档标过期
            //（决策 83，pipeline-spec §6）；双方 blockers 写任务目录 backtrack-feedback.md（决策 126）
            let main = self.store.backtrack_cursors(&task.id).await?;
            let feedback = format!(
                "# backtrack 反馈\n\ndev blockers：{:?}\ntest blockers：{:?}\n",
                decision.dev_blockers, decision.test_blockers
            );
            self.store.home().ensure_task_dirs(&task.id)?;
            std::fs::write(
                self.store
                    .home()
                    .task_file(&task.id, "backtrack-feedback.md"),
                feedback,
            )?;
            main
        };

        // sync-check 自身的 system run（决策 107 / 114）
        let attempt = self
            .next_attempt(&task.id, Stage::SyncCheck, Node::Execute)
            .await?;
        let run_id = self
            .store
            .insert_run(&NewRun {
                task_id: task.id.clone(),
                cursor_id: main.cursor_id.clone(),
                stage: Stage::SyncCheck,
                node: Node::Execute,
                attempt,
                agent_type: "system".into(),
                parent_run_id: None,
                prompt_template_hash: None,
                process_group_id: None,
            })
            .await?;
        self.store
            .finish_run(
                run_id,
                &RunOutcome {
                    status: Some(NodeStatus::Success),
                    ..Default::default()
                },
            )
            .await?;
        self.store
            .upsert_stage_output(
                &task.id,
                Stage::SyncCheck,
                OUTPUT_SYNC_DECISION,
                "sync-decision.json",
                Some(&serde_json::to_value(&decision)?),
            )
            .await?;

        let from = cursors.first().map(|c| (c.stage, c.node));
        self.store
            .insert_transition(
                &task.id,
                &main.branch,
                from,
                (main.stage, main.node),
                crate::types::TransitionTrigger::AutoResume,
                Some(match decision.decision {
                    SyncDecisionKind::Proceed => "sync-check 汇聚通过",
                    SyncDecisionKind::Backtrack => "sync-check 判定回溯",
                }),
            )
            .await?;
        self.sse.emit(SseEvent::StageChanged {
            task_id: task.id.clone(),
            branch: main.branch.clone(),
            from_stage: from.map(|(s, _)| s),
            from_node: from.map(|(_, n)| n),
            to_stage: main.stage,
            to_node: main.node,
            trigger: "auto_resume".into(),
            reason: None,
        });
        self.store.sync_task_projection(&task.id).await?;
        Ok(())
    }

    /// SyncDecision 计算（§7 决策矩阵 + 决策 93 skipped_to_join + 决策 136 引用校验）。
    async fn compute_sync_decision(
        &self,
        task: &Task,
        cursors: &[NodeCursor],
    ) -> Result<SyncDecision> {
        let skipped = |branch: &str| {
            cursors
                .iter()
                .find(|c| c.branch == branch)
                .map(|c| c.skipped_to_join)
                .unwrap_or(false)
        };
        let dev_meta = self
            .store
            .stage_output_metadata(&task.id, Stage::DevelopDesign, OUTPUT_DEV_DOC)
            .await?;
        let test_meta = self
            .store
            .stage_output_metadata(&task.id, Stage::TestDesign, OUTPUT_TEST_SCENARIOS)
            .await?;

        let dev_readiness =
            skipped(NodeCursor::BRANCH_DEVELOP_DESIGN) || meta_flag(dev_meta.as_ref(), "readiness");
        let test_readiness =
            skipped(NodeCursor::BRANCH_TEST_DESIGN) || meta_flag(test_meta.as_ref(), "readiness");
        let dev_blockers = meta_str_list(dev_meta.as_ref(), "blockers");
        let mut test_blockers = meta_str_list(test_meta.as_ref(), "blockers");
        let mut warnings = Vec::new();

        // 决策 136：high 优先级场景的 design_refs 缺失/悬空 → blocker
        if !skipped(NodeCursor::BRANCH_TEST_DESIGN) {
            let criteria = self
                .store
                .stage_output_metadata(&task.id, Stage::ArchitectDesign, OUTPUT_DESIGN_DOC)
                .await?
                .map(|m| {
                    m.get("acceptance_criteria")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|c| {
                                    c.get("id").and_then(|i| i.as_str()).map(String::from)
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            if let Some(scenarios) = test_meta
                .as_ref()
                .and_then(|m| m.get("test_scenarios"))
                .and_then(|v| v.as_array())
            {
                for s in scenarios {
                    let priority = s.get("priority").and_then(|p| p.as_str()).unwrap_or("");
                    let refs = s
                        .get("design_refs")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|r| r.as_str().map(String::from))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    if priority == "high" {
                        let dangling: Vec<String> = refs
                            .iter()
                            .filter(|r| !criteria.contains(r))
                            .cloned()
                            .collect();
                        if refs.is_empty() || !dangling.is_empty() {
                            let name = s.get("name").and_then(|n| n.as_str()).unwrap_or("场景");
                            test_blockers.push(format!(
                                "high 场景「{name}」的 design_refs 缺失或悬空（决策 136）"
                            ));
                        }
                    } else {
                        // medium/low：引用缺失仅 warning（决策 136）
                        let dangling: Vec<String> = refs
                            .iter()
                            .filter(|r| !criteria.contains(r))
                            .cloned()
                            .collect();
                        if !dangling.is_empty() {
                            let name = s.get("name").and_then(|n| n.as_str()).unwrap_or("场景");
                            warnings.push(format!(
                                "场景「{name}」引用了不存在的验收标准：{}",
                                dangling.join("、")
                            ));
                        }
                    }
                }
            }
        }

        let proceed =
            dev_readiness && test_readiness && dev_blockers.is_empty() && test_blockers.is_empty();
        Ok(SyncDecision {
            decision: if proceed {
                SyncDecisionKind::Proceed
            } else {
                SyncDecisionKind::Backtrack
            },
            dev_readiness,
            test_readiness,
            dev_blockers,
            test_blockers,
            warnings,
        })
    }

    // ─────────────────────── 游标推进（§11.2 advance_cursor）───────────────────────────────

    async fn advance_cursor(
        &self,
        task: &Task,
        cursor: &NodeCursor,
        output: NodeOutput,
    ) -> Result<()> {
        // review 人工模式：预审后无论结论如何都等用户提交评审（§6 / §12.5）
        if cursor.stage == Stage::Review
            && cursor.node == Node::ValidateOutput
            && task.review_mode == ReviewMode::Human
        {
            self.pend_cursor(cursor, PendingKind::HumanReview, "等待人工评审")
                .await?;
            return Ok(());
        }

        let (edge, reason_override) = match output {
            NodeOutput::Route(view) => {
                let merge = self
                    .store
                    .merge_metadata(&task.id)
                    .await?
                    .unwrap_or_else(placeholder_merge);
                let ctx = crate::pipeline::RouteContext {
                    validate_retry_max: self.settings.validate_retry_max,
                    metadata: view,
                    merge,
                };
                (crate::pipeline::route(cursor, &ctx), None)
            }
            NodeOutput::Edge(edge, reason) => (edge, reason),
            NodeOutput::Pending(reason) => {
                self.store
                    .set_cursor_pending(&cursor.cursor_id, &reason)
                    .await?;
                self.store.sync_task_projection(&task.id).await?;
                self.sse.emit(SseEvent::Pending {
                    task_id: task.id.clone(),
                    branch: cursor.branch.clone(),
                    cursor_id: cursor.cursor_id.clone(),
                    reason,
                });
                return Ok(());
            }
        };
        self.apply_edge(task, cursor, edge, reason_override).await
    }

    async fn apply_edge(
        &self,
        task: &Task,
        cursor: &NodeCursor,
        edge: EdgeKind,
        reason_override: Option<String>,
    ) -> Result<()> {
        let task_id = &task.id;
        let kickback_reason = |default: &'static str| {
            reason_override
                .clone()
                .unwrap_or_else(|| default.to_string())
        };
        match edge {
            EdgeKind::NoOp => {}
            EdgeKind::Retry => {
                // 阶段内重试：attempts +1，回到 execute（重试回边 validate_output → execute）
                self.store
                    .increment_cursor_attempts(&cursor.cursor_id)
                    .await?;
                self.store
                    .move_cursor(&cursor.cursor_id, cursor.stage, Node::Execute)
                    .await?;
                self.store
                    .insert_transition(
                        task_id,
                        &cursor.branch,
                        Some((cursor.stage, cursor.node)),
                        (cursor.stage, Node::Execute),
                        crate::types::TransitionTrigger::NodeRetry,
                        None,
                    )
                    .await?;
            }
            EdgeKind::Pending(kind) => {
                let message = pending_message(kind);
                self.pend_cursor(cursor, kind, message).await?;
            }
            EdgeKind::Next => {
                // 阶段内推进（§1.2 图内边）：validate_input → execute → validate_output
                let intra = match cursor.node {
                    Node::ValidateInput
                        if crate::pipeline::stage_has_node(cursor.stage, Node::Execute) =>
                    {
                        Some(Node::Execute)
                    }
                    Node::Execute
                        if crate::pipeline::stage_has_node(cursor.stage, Node::ValidateOutput) =>
                    {
                        Some(Node::ValidateOutput)
                    }
                    _ => None,
                };
                if let Some(to_node) = intra {
                    let from = (cursor.stage, cursor.node);
                    self.store
                        .move_cursor(&cursor.cursor_id, cursor.stage, to_node)
                        .await?;
                    self.store
                        .insert_transition(
                            task_id,
                            &cursor.branch,
                            Some(from),
                            (cursor.stage, to_node),
                            crate::types::TransitionTrigger::Normal,
                            None,
                        )
                        .await?;
                    self.emit_cursor_changed(task_id, &cursor.branch).await?;
                } else {
                    let nexts = crate::pipeline::next_stages(cursor.stage);
                    if nexts.len() > 1 {
                        // 并行分裂点（architect-design → develop-design ∥ test-design，决策 90）
                        self.store.split_cursors(task_id).await?;
                        self.store
                            .insert_transition(
                                task_id,
                                &cursor.branch,
                                Some((cursor.stage, cursor.node)),
                                (Stage::DevelopDesign, Node::ValidateInput),
                                crate::types::TransitionTrigger::Normal,
                                Some("游标分裂（决策 90）"),
                            )
                            .await?;
                        for branch in [
                            NodeCursor::BRANCH_DEVELOP_DESIGN,
                            NodeCursor::BRANCH_TEST_DESIGN,
                        ] {
                            self.emit_cursor_changed(task_id, branch).await?;
                        }
                    } else if crate::pipeline::next_is_join(cursor.stage) {
                        // 下一阶段是 join：本游标置 waiting_join（决策 107，唯一写入路径）
                        self.store
                            .set_cursor_waiting_join(&cursor.cursor_id)
                            .await?;
                        self.store
                            .insert_transition(
                                task_id,
                                &cursor.branch,
                                Some((cursor.stage, cursor.node)),
                                (crate::pipeline::JOIN_STAGE, Node::Execute),
                                crate::types::TransitionTrigger::Normal,
                                Some("到达 join 边界"),
                            )
                            .await?;
                        self.emit_cursor_changed(task_id, &cursor.branch).await?;
                    } else if let Some(&next) = nexts.first() {
                        let from = (cursor.stage, cursor.node);
                        let to = (next, crate::pipeline::entry_node(next));
                        self.store
                            .set_cursor_stage(&cursor.cursor_id, to.0, to.1)
                            .await?;
                        self.store
                            .insert_transition(
                                task_id,
                                &cursor.branch,
                                Some(from),
                                to,
                                crate::types::TransitionTrigger::Normal,
                                None,
                            )
                            .await?;
                        self.sse.emit(SseEvent::StageChanged {
                            task_id: task_id.clone(),
                            branch: cursor.branch.clone(),
                            from_stage: Some(from.0),
                            from_node: Some(from.1),
                            to_stage: to.0,
                            to_node: to.1,
                            trigger: "normal".into(),
                            reason: None,
                        });
                        self.emit_cursor_changed(task_id, &cursor.branch).await?;
                    }
                    // nexts 为空（done 之后）→ 无流转，终态判定在主循环
                }
            }
            EdgeKind::KickbackDevelop => {
                let from = (cursor.stage, cursor.node);
                self.store
                    .set_cursor_stage(&cursor.cursor_id, Stage::Develop, Node::Execute)
                    .await?;
                self.store
                    .insert_transition(
                        task_id,
                        &cursor.branch,
                        Some(from),
                        (Stage::Develop, Node::Execute),
                        crate::types::TransitionTrigger::Kickback,
                        Some(
                            kickback_reason(
                                "merge 打回 develop（冲突 / lint 闸门失败 / 返回修改）",
                            )
                            .as_str(),
                        ),
                    )
                    .await?;
                self.sse.emit(SseEvent::StageChanged {
                    task_id: task_id.clone(),
                    branch: cursor.branch.clone(),
                    from_stage: Some(from.0),
                    from_node: Some(from.1),
                    to_stage: Stage::Develop,
                    to_node: Node::Execute,
                    trigger: "kickback".into(),
                    reason: None,
                });
            }
            EdgeKind::GotoTest => {
                let from = (cursor.stage, cursor.node);
                self.store
                    .set_cursor_stage(&cursor.cursor_id, Stage::Test, Node::Execute)
                    .await?;
                self.store
                    .insert_transition(
                        task_id,
                        &cursor.branch,
                        Some(from),
                        (Stage::Test, Node::Execute),
                        crate::types::TransitionTrigger::Kickback,
                        Some(
                            kickback_reason(
                                "merge 测试闸门失败，跳回 test.execute 复检（决策 85）",
                            )
                            .as_str(),
                        ),
                    )
                    .await?;
                self.sse.emit(SseEvent::StageChanged {
                    task_id: task_id.clone(),
                    branch: cursor.branch.clone(),
                    from_stage: Some(from.0),
                    from_node: Some(from.1),
                    to_stage: Stage::Test,
                    to_node: Node::Execute,
                    trigger: "kickback".into(),
                    reason: None,
                });
            }
            EdgeKind::Backtrack => {
                // 双方一起回 architect-design.validate_input（决策 83 / 90）
                let main = self.store.backtrack_cursors(task_id).await?;
                self.store
                    .insert_transition(
                        task_id,
                        &main.branch,
                        Some((cursor.stage, cursor.node)),
                        (Stage::ArchitectDesign, Node::ValidateInput),
                        crate::types::TransitionTrigger::Kickback,
                        Some("sync-check backtrack（决策 83）"),
                    )
                    .await?;
                self.sse.emit(SseEvent::StageChanged {
                    task_id: task_id.clone(),
                    branch: main.branch.clone(),
                    from_stage: Some(cursor.stage),
                    from_node: Some(cursor.node),
                    to_stage: Stage::ArchitectDesign,
                    to_node: Node::ValidateInput,
                    trigger: "kickback".into(),
                    reason: None,
                });
            }
        }
        self.store.sync_task_projection(task_id).await?;
        Ok(())
    }

    async fn emit_cursor_changed(&self, task_id: &str, branch: &str) -> Result<()> {
        let cursor = self
            .store
            .load_live_cursors(task_id)
            .await?
            .into_iter()
            .find(|c| c.branch == branch);
        if let Some(c) = cursor {
            self.sse.emit(SseEvent::CursorChanged {
                task_id: task_id.to_string(),
                branch: c.branch.clone(),
                cursor_id: c.cursor_id.clone(),
                status: c.status.as_str().into(),
                stage: c.stage,
                node: c.node,
            });
        }
        Ok(())
    }

    // ─────────────────────── 闸门命令执行（决策 62 / 139）───────────────────────────────

    /// 跑系统命令并记录 `kanban_node_commands`（source=system）。返回 exit code；
    /// 启动失败是节点错误，非零退出是**闸门结果**而非节点错误。
    async fn run_system_command(
        &self,
        task: &Task,
        run_id: i64,
        stage: Stage,
        node: Node,
        command: &str,
        cwd: &Path,
    ) -> Result<i32> {
        let sanitized = crate::agent::sanitize::sanitize_command_line(command);
        let command_id = self
            .store
            .record_start(CommandStart {
                task_id: task.id.clone(),
                run_id: Some(run_id),
                stage,
                node,
                source: CommandSource::System,
                command: sanitized,
                cwd: cwd.display().to_string(),
            })
            .await?;
        self.store.touch_run_heartbeat(run_id).await?;

        let started = Instant::now();
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(self.settings.test_command_timeout_sec),
            tokio::process::Command::new("sh")
                .arg("-c")
                .arg(command)
                .current_dir(cwd)
                .output(),
        )
        .await;
        let duration_ms = started.elapsed().as_millis() as u64;
        let (exit_code, stdout, stderr) = match output {
            Ok(Ok(out)) => (
                out.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&out.stdout).to_string(),
                String::from_utf8_lossy(&out.stderr).to_string(),
            ),
            Ok(Err(e)) => return Err(Error::Git(format!("闸门命令启动失败：{e}"))),
            Err(_) => {
                // 超时按闸门失败处理（exit = -1），错误信息进输出
                self.store
                    .record_finish(
                        command_id,
                        CommandFinish {
                            exit_code: Some(-1),
                            stdout_preview: None,
                            stderr_preview: Some(format!(
                                "命令超时（{}s）",
                                self.settings.test_command_timeout_sec
                            )),
                            duration_ms,
                            ..Default::default()
                        },
                    )
                    .await?;
                return Ok(-1);
            }
        };
        let stdout = crate::agent::sanitize::sanitize_text(&stdout);
        let stderr = crate::agent::sanitize::sanitize_text(&stderr);
        self.store
            .record_finish(
                command_id,
                CommandFinish {
                    exit_code: Some(exit_code),
                    stdout_preview: Some(crate::agent::tools::head_tail(&stdout, 50, 100)),
                    stderr_preview: Some(crate::agent::tools::head_tail(&stderr, 50, 100)),
                    duration_ms,
                    ..Default::default()
                },
            )
            .await?;
        self.store.touch_run_heartbeat(run_id).await?;
        Ok(exit_code)
    }

    /// develop / merge 共用的闸门：lint（如配置）+ 测试（决策 139）。
    #[allow(clippy::too_many_arguments)]
    async fn run_code_gate(
        &self,
        task: &Task,
        project: &Project,
        run_id: i64,
        stage: Stage,
        node: Node,
        cwd: &Path,
        include_lint: bool,
    ) -> Result<GateOutcome> {
        if include_lint {
            if let Some(lint) = &project.lint_command {
                let code = self
                    .run_system_command(task, run_id, stage, node, lint, cwd)
                    .await?;
                if code != 0 {
                    return Ok(GateOutcome {
                        passed: false,
                        failure_kind: GateFailureKind::Lint,
                        output: format!("lint 命令 `{lint}` 退出码 {code}"),
                    });
                }
            }
        }
        let test = test_command_for(project.test_framework.as_deref());
        let code = self
            .run_system_command(task, run_id, stage, node, &test, cwd)
            .await?;
        if code != 0 {
            return Ok(GateOutcome {
                passed: false,
                failure_kind: GateFailureKind::Test,
                output: format!("测试命令 `{test}` 退出码 {code}"),
            });
        }
        Ok(GateOutcome {
            passed: true,
            failure_kind: GateFailureKind::Test,
            output: String::new(),
        })
    }

    // ─────────────────────── 小工具 ───────────────────────

    async fn project(&self, project_id: &str) -> Result<Project> {
        self.store
            .get_project(project_id)
            .await?
            .ok_or_else(|| Error::Task(format!("项目不存在：{project_id}")))
    }

    async fn next_attempt(&self, task_id: &str, stage: Stage, node: Node) -> Result<u32> {
        Ok(self.store.list_runs_at(task_id, stage, node).await?.len() as u32 + 1)
    }

    async fn begin_run(
        &self,
        task: &Task,
        cursor: &NodeCursor,
        agent_type: &str,
    ) -> Result<(i64, u32)> {
        let attempt = self
            .next_attempt(&task.id, cursor.stage, cursor.node)
            .await?;
        let run_id = self
            .store
            .insert_run(&NewRun {
                task_id: task.id.clone(),
                cursor_id: cursor.cursor_id.clone(),
                stage: cursor.stage,
                node: cursor.node,
                attempt,
                agent_type: agent_type.into(),
                parent_run_id: None,
                prompt_template_hash: None,
                process_group_id: None,
            })
            .await?;
        self.sse.emit(SseEvent::NodeStarted {
            task_id: task.id.clone(),
            branch: cursor.branch.clone(),
            stage: cursor.stage,
            node: cursor.node,
            attempt,
            run_id,
        });
        Ok((run_id, attempt))
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    async fn finish_run(
        &self,
        run_id: i64,
        task: &Task,
        cursor: &NodeCursor,
        attempt: u32,
        failed: bool,
        duration_ms: u64,
        error: Option<String>,
        tokens: &RunTokens,
    ) -> Result<()> {
        self.store
            .finish_run(
                run_id,
                &RunOutcome {
                    status: Some(if failed {
                        NodeStatus::Failed
                    } else {
                        NodeStatus::Success
                    }),
                    duration_ms,
                    error,
                    prompt_tokens: tokens.prompt,
                    completion_tokens: tokens.completion,
                    cache_read_tokens: tokens.cache_read,
                    cache_write_tokens: tokens.cache_write,
                    ..Default::default()
                },
            )
            .await?;
        self.sse.emit(SseEvent::NodeFinished {
            task_id: task.id.clone(),
            branch: cursor.branch.clone(),
            stage: cursor.stage,
            node: cursor.node,
            attempt,
            run_id,
            status: if failed {
                "failed".into()
            } else {
                "success".into()
            },
            duration_ms,
            prompt_tokens: tokens.prompt,
            completion_tokens: tokens.completion,
        });
        Ok(())
    }
}

// ─────────────────────────────── 节点输出 ───────────────────────────────

/// 一次 agent attempt 的 token 计量（决策 46：prompt / completion / cache 落 run 行）。
#[derive(Debug, Clone, Copy, Default)]
struct RunTokens {
    prompt: u32,
    completion: u32,
    cache_read: u32,
    cache_write: u32,
}

impl RunTokens {
    fn add(&mut self, response: &crate::agent::client::AgentResponse) {
        self.prompt += response.prompt_tokens;
        self.completion += response.completion_tokens;
        self.cache_read += response.cache_read_tokens;
        self.cache_write += response.cache_write_tokens;
    }
}

impl Executor {
    /// 决策 123 的 `tool_event` 发射（start / end / error 三态共用一个出口）。
    fn emit_tool_event(
        &self,
        task: &Task,
        cursor: &NodeCursor,
        run_id: i64,
        tool: &str,
        phase: ToolPhase,
        args_summary: &str,
    ) {
        self.sse.emit(SseEvent::ToolEvent {
            task_id: task.id.clone(),
            branch: cursor.branch.clone(),
            run_id,
            tool: tool.to_string(),
            phase,
            args_summary: args_summary.to_string(),
        });
    }
}

/// `tool_event` 的参数摘要（决策 123：只给摘要，不外发全量参数）。
fn args_summary(text: &str) -> String {
    const LIMIT: usize = 120;
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.chars().count() <= LIMIT {
        compact
    } else {
        let cut: String = compact.chars().take(LIMIT).collect();
        format!("{cut}…")
    }
}

/// 节点执行结论：大多数走路由；少数（merge 冲突打回 / 决策 135 分歧）直接给边或 pending。
/// `Edge` 的第二个字段覆盖默认流转原因（如冲突文件清单）。
enum NodeOutput {
    Route(crate::pipeline::MetadataView),
    Edge(EdgeKind, Option<String>),
    Pending(PendingReason),
}

enum PhaseA {
    /// 已生成 proposal 并挂 pending(merge_approval)。
    Proposal,
    /// 闸门已跑且失败，结果在 merge metadata 里，交 route_merge 分流。
    GateRan,
    /// rebase 冲突（已 abort），打回 develop。
    Conflict(Vec<String>),
}

/// agent 节点种类：元数据类型与后处理按此分发（决策 38）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentNodeKind {
    ValidateInput,
    ArchitectExecute,
    DevelopDesignExecute,
    TestDesignExecute,
    DesignValidateOutput,
    DevelopExecute,
    ReviewExecute,
    TestExecute,
}

impl AgentNodeKind {
    fn of(stage: Stage, node: Node) -> Option<Self> {
        use AgentNodeKind::*;
        Some(match (stage, node) {
            (
                Stage::ArchitectDesign | Stage::DevelopDesign | Stage::TestDesign,
                Node::ValidateInput,
            ) => ValidateInput,
            (Stage::ArchitectDesign, Node::Execute) => ArchitectExecute,
            (Stage::DevelopDesign, Node::Execute) => DevelopDesignExecute,
            (Stage::TestDesign, Node::Execute) => TestDesignExecute,
            (
                Stage::ArchitectDesign | Stage::DevelopDesign | Stage::TestDesign,
                Node::ValidateOutput,
            ) => DesignValidateOutput,
            (Stage::Develop, Node::Execute) => DevelopExecute,
            (Stage::Review, Node::Execute) => ReviewExecute,
            (Stage::Test, Node::Execute) => TestExecute,
            _ => return None,
        })
    }

    /// 类型化校验（决策 33：元数据解析/校验失败 → 节点重试）。
    fn validate(&self, value: &serde_json::Value) -> Result<()> {
        macro_rules! check {
            ($t:ty) => {
                parse_metadata::<$t>(value).map(|_| ())
            };
        }
        match self {
            AgentNodeKind::ValidateInput => check!(crate::types::ValidateInputMetadata),
            AgentNodeKind::ArchitectExecute => check!(crate::types::ArchitectExecuteMetadata),
            AgentNodeKind::DevelopDesignExecute => check!(crate::types::DevelopDesignMetadata),
            AgentNodeKind::TestDesignExecute => check!(crate::types::TestDesignMetadata),
            AgentNodeKind::DesignValidateOutput => check!(crate::types::ValidateOutputMetadata),
            AgentNodeKind::DevelopExecute => check!(crate::types::CodeChanges),
            AgentNodeKind::ReviewExecute => check!(crate::types::ReviewResult),
            AgentNodeKind::TestExecute => check!(crate::types::TestResult),
        }
    }

    /// 节点后处理：execute 类节点落阶段产出 + 返回路由视图。
    async fn post_process(
        &self,
        ex: &Executor,
        task: &Task,
        value: serde_json::Value,
    ) -> Result<NodeOutput> {
        use crate::pipeline::MetadataView;
        let view = match self {
            AgentNodeKind::ValidateInput => {
                let m: crate::types::ValidateInputMetadata = serde_json::from_value(value)?;
                MetadataView::readiness(m.readiness)
            }
            AgentNodeKind::DesignValidateOutput => {
                let m: crate::types::ValidateOutputMetadata = serde_json::from_value(value)?;
                MetadataView::passed(m.passed)
            }
            AgentNodeKind::ArchitectExecute => {
                let m: crate::types::ArchitectExecuteMetadata =
                    serde_json::from_value(value.clone())?;
                let path = m
                    .design_doc_path
                    .clone()
                    .unwrap_or_else(|| "design.md".into());
                ex.store
                    .upsert_stage_output(
                        &task.id,
                        Stage::ArchitectDesign,
                        OUTPUT_DESIGN_DOC,
                        &path,
                        Some(&value),
                    )
                    .await?;
                // 两层冲突检测的第一层（决策 53 / 60 / 71 / 102；第二层归票 16）
                let conflicts = ex.store.first_layer_conflicts(&task.id).await?;
                if !conflicts.is_empty() {
                    let ids: Vec<String> = conflicts.iter().map(|w| w.task_id.clone()).collect();
                    let reason = PendingReason::new(
                        PendingKind::ConflictWait,
                        Stage::ArchitectDesign,
                        Node::Execute,
                        format!("与活跃任务存在文件/符号冲突：{}", ids.join("、")),
                    )
                    .with_context(PendingContext {
                        conflict_task_ids: ids,
                        ..Default::default()
                    });
                    return Ok(NodeOutput::Pending(reason));
                }
                MetadataView::default()
            }
            AgentNodeKind::DevelopDesignExecute => {
                let m: crate::types::DevelopDesignMetadata = serde_json::from_value(value.clone())?;
                let path = m
                    .dev_doc_path
                    .clone()
                    .unwrap_or_else(|| "dev-plan.md".into());
                ex.store
                    .upsert_stage_output(
                        &task.id,
                        Stage::DevelopDesign,
                        OUTPUT_DEV_DOC,
                        &path,
                        Some(&value),
                    )
                    .await?;
                MetadataView::default()
            }
            AgentNodeKind::TestDesignExecute => {
                let m: crate::types::TestDesignMetadata = serde_json::from_value(value.clone())?;
                let path = m
                    .test_scenarios_path
                    .clone()
                    .unwrap_or_else(|| "test-scenarios.md".into());
                ex.store
                    .upsert_stage_output(
                        &task.id,
                        Stage::TestDesign,
                        OUTPUT_TEST_SCENARIOS,
                        &path,
                        Some(&value),
                    )
                    .await?;
                MetadataView::default()
            }
            AgentNodeKind::DevelopExecute => {
                let m: crate::types::CodeChanges = serde_json::from_value(value.clone())?;
                ex.store
                    .upsert_stage_output(
                        &task.id,
                        Stage::Develop,
                        OUTPUT_CODE_CHANGES,
                        "code-changes.json",
                        Some(&value),
                    )
                    .await?;
                let _ = m;
                MetadataView::default()
            }
            AgentNodeKind::ReviewExecute => {
                let m: crate::types::ReviewResult = serde_json::from_value(value.clone())?;
                let path = m
                    .review_report_path
                    .clone()
                    .unwrap_or_else(|| "review-report.md".into());
                ex.store
                    .upsert_stage_output(
                        &task.id,
                        Stage::Review,
                        OUTPUT_REVIEW_REPORT,
                        &path,
                        Some(&value),
                    )
                    .await?;
                // review 的判定在 validate_output（纯代码）做，execute 只产出
                MetadataView::default()
            }
            AgentNodeKind::TestExecute => {
                let m: crate::types::TestResult = serde_json::from_value(value.clone())?;
                let path = m
                    .test_report_path
                    .clone()
                    .unwrap_or_else(|| "test-report.md".into());
                ex.store
                    .upsert_stage_output(
                        &task.id,
                        Stage::Test,
                        OUTPUT_TEST_REPORT,
                        &path,
                        Some(&value),
                    )
                    .await?;
                // test 的判定在 validate_output（纯代码）做
                MetadataView::default()
            }
        };
        Ok(NodeOutput::Route(view))
    }
}

/// 有效工具定义：基线并集（G6）内且 v1 已实现的内置工具 + `submit_metadata`
/// schema 工具（决策 38：与校验同源，不可移除）。声明了未实现的工具只告警不阻塞。
fn tool_defs(kind: AgentNodeKind, declared: &[String]) -> Vec<ToolDef> {
    let mut defs: Vec<ToolDef> = Vec::new();
    for name in effective_tools(declared) {
        if name == "submit_metadata" {
            continue; // 最后以 schema 形式追加
        }
        if !BUILTIN_TOOLS.contains(&name.as_str()) {
            tracing::warn!(tool = %name, "阶段声明的工具在 v1 未实现，已忽略");
            continue;
        }
        defs.push(ToolDef {
            name,
            description: String::new(),
            parameters: serde_json::json!({"type": "object"}),
        });
    }
    let schema_tool: ToolDef = match kind {
        AgentNodeKind::ValidateInput => {
            submit_metadata_tool::<crate::types::ValidateInputMetadata>("提交输入充分性判定")
        }
        AgentNodeKind::ArchitectExecute => {
            submit_metadata_tool::<crate::types::ArchitectExecuteMetadata>("提交架构设计元数据")
        }
        AgentNodeKind::DevelopDesignExecute => {
            submit_metadata_tool::<crate::types::DevelopDesignMetadata>("提交开发计划元数据")
        }
        AgentNodeKind::TestDesignExecute => {
            submit_metadata_tool::<crate::types::TestDesignMetadata>("提交测试场景元数据")
        }
        AgentNodeKind::DesignValidateOutput => {
            submit_metadata_tool::<crate::types::ValidateOutputMetadata>("提交产出校验结论")
        }
        AgentNodeKind::DevelopExecute => {
            submit_metadata_tool::<crate::types::CodeChanges>("提交代码变更元数据")
        }
        AgentNodeKind::ReviewExecute => {
            submit_metadata_tool::<crate::types::ReviewResult>("提交评审结论")
        }
        AgentNodeKind::TestExecute => {
            submit_metadata_tool::<crate::types::TestResult>("提交测试结果元数据")
        }
    };
    defs.push(schema_tool);
    defs
}

// ─────────────────────── prompt 组装辅助（票 12：§10.3 / G3 / G6 / G12）───────────────────────

/// G12 工作目录行（system 的「工作目录」段与 user 的「环境路径」段共用，防漂移）。
fn workdirs_line(worktree: &str, task_dir: &str) -> String {
    format!("worktree：{worktree}\n任务目录：{task_dir}")
}

/// 决策 126：backtrack blockers 写在任务目录 `backtrack-feedback.md`，
/// architect-design 重入（validate_input / execute）的 user prompt 注入该内容；
/// 首轮（文件尚不存在）为空不渲染。
fn backtrack_feedback_segment(
    home: &Home,
    task_id: &str,
    stage: Stage,
    node: Node,
) -> Option<String> {
    if stage != Stage::ArchitectDesign || !matches!(node, Node::ValidateInput | Node::Execute) {
        return None;
    }
    std::fs::read_to_string(home.task_file(task_id, "backtrack-feedback.md"))
        .ok()
        .filter(|s| !s.trim().is_empty())
}

/// persona 解析（决策 7 / §10.6.3）：`stage_configs.persona_path` 显式指定优先，
/// 其次 `prompts/{stage}/{node}.md` 用户覆盖，最后内嵌 §10.3 模板；
/// `persona_append` 追加为额外指令段。
fn resolve_stage_persona(
    home: &Home,
    stage_cfg: Option<&StageConfig>,
    stage: Stage,
    node: Node,
) -> Result<String> {
    let embedded = system_template(stage, node);
    let mut content = match stage_cfg.and_then(|c| c.persona_path.as_deref()) {
        Some(path) => {
            // 相对路径按 home 根解析；绝对路径原样使用
            let p = home.root().join(path);
            let read = std::fs::read_to_string(&p).map_err(|e| {
                Error::Config(format!(
                    "阶段 {stage} 的 persona_path 不可读：{}（{e}）",
                    p.display()
                ))
            })?;
            if read.trim().is_empty() {
                return Err(Error::Config(format!(
                    "阶段 {stage} 的 persona_path 内容为空：{}",
                    p.display()
                )));
            }
            read
        }
        None => resolve_persona(&home.prompts_dir(), stage, node, embedded).content,
    };
    if let Some(append) = stage_cfg.and_then(|c| c.persona_append.as_deref()) {
        if !append.trim().is_empty() {
            content.push_str(&format!("\n\n{append}"));
        }
    }
    Ok(content)
}

/// 阶段配置里的字符串数组字段（tools_json / skills_json）。
fn json_string_list(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// 从 code_changes stage output 提取变更文件 / 单元测试文件列表（每行一个路径）。
/// 缺失时给降级说明（决策 115 / 133：评审与测试模板需容忍上游阶段被跳过）。
fn code_changes_lists(value: Option<&serde_json::Value>) -> (String, String) {
    const MISSING: &str = "（缺失：本任务跳过了对应阶段，按决策 115 降级处理）";
    let Some(value) = value else {
        return (MISSING.into(), MISSING.into());
    };
    let changes: Option<crate::types::CodeChanges> = serde_json::from_value(value.clone()).ok();
    let join = |specs: &[crate::types::FileChangeSpec]| {
        if specs.is_empty() {
            MISSING.to_string()
        } else {
            specs
                .iter()
                .map(|s| s.path.clone())
                .collect::<Vec<_>>()
                .join("\n")
        }
    };
    match changes {
        Some(c) => (join(&c.changed_files), join(&c.unit_test_files)),
        None => (MISSING.into(), MISSING.into()),
    }
}

fn pending_message(kind: PendingKind) -> &'static str {
    match kind {
        PendingKind::InfoInsufficient => "设计输入信息不足，请补充",
        PendingKind::UserDecision => "需要用户决策",
        PendingKind::RetryExhausted => "重试耗尽，需要用户介入",
        PendingKind::MergeApproval => "等待审批合入",
        PendingKind::HumanReview => "等待人工评审",
        _ => "任务被阻塞",
    }
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

fn test_file_convention(framework: Option<&str>) -> &'static str {
    match framework {
        Some("pytest") => "tests/test_*.py",
        Some("npm") | Some("node") => "**/*.test.ts",
        _ => "tests/*_test.rs",
    }
}

/// 路由上下文的 merge 占位（非 merge 节点不会用到；字段满足文档必填契约）。
fn placeholder_merge() -> MergeResult {
    MergeResult {
        diff_path: String::new(),
        diff_stats: DiffStats {
            files_changed: 0,
            insertions: 0,
            deletions: 0,
            file_details: Vec::new(),
        },
        base_commit: String::new(),
        gate: None,
        gate_failure_kind: None,
        gate_failures: 0,
        gate_failure_output: None,
        conflict_files: Vec::new(),
        approval: Approval::None,
        status: MergeStatus::PendingApproval,
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

// ─────────────────────────────── 辅助（元数据布尔 / 字符串列表）───────────────────────────────

fn meta_flag(meta: Option<&serde_json::Value>, key: &str) -> bool {
    meta.and_then(|m| m.get(key))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

fn meta_str_list(meta: Option<&serde_json::Value>, key: &str) -> Vec<String> {
    meta.and_then(|m| m.get(key))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backtrack_feedback_only_injected_for_architect_reentry() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = Home::new(tmp.path());
        std::fs::create_dir_all(home.task_dir("t1")).unwrap();

        // 首轮：反馈文件不存在 → 不渲染（决策 126「首轮为空不渲染」）
        assert_eq!(
            backtrack_feedback_segment(&home, "t1", Stage::ArchitectDesign, Node::ValidateInput),
            None
        );

        std::fs::write(
            home.task_file("t1", "backtrack-feedback.md"),
            "dev blockers：[\"缺少数据流定义\"]\n",
        )
        .unwrap();
        assert!(backtrack_feedback_segment(
            &home,
            "t1",
            Stage::ArchitectDesign,
            Node::ValidateInput
        )
        .is_some());
        assert!(
            backtrack_feedback_segment(&home, "t1", Stage::ArchitectDesign, Node::Execute)
                .is_some()
        );

        // 决策 126 的注入范围只有 validate_input / execute
        assert_eq!(
            backtrack_feedback_segment(&home, "t1", Stage::ArchitectDesign, Node::ValidateOutput),
            None
        );
        assert_eq!(
            backtrack_feedback_segment(&home, "t1", Stage::Develop, Node::Execute),
            None
        );

        // 空文件（纯空白）不渲染
        std::fs::write(home.task_file("t1", "backtrack-feedback.md"), "  \n").unwrap();
        assert_eq!(
            backtrack_feedback_segment(&home, "t1", Stage::ArchitectDesign, Node::ValidateInput),
            None
        );
    }
}
