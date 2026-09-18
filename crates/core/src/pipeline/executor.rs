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
use std::path::{Path, PathBuf};
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
    SKILL_TOOL,
};
use crate::config::Settings;
use crate::git::Git;
use crate::home::Home;
use crate::pipeline::pseudo::{ConflictCheckResult, CrossCheckResult, PseudoStage};
use crate::process::ProcessKiller;
use crate::sse::{SseEvent, SseSink, ToolPhase};
use crate::storage::observability::{NewRun, PromptSnapshot, RunOutcome};
use crate::types::{
    Approval, CommandSource, DiffStats, DuplicateRisk, EdgeKind, Gate, GateFailureKind,
    MergeResult, MergeStatus, Node, NodeCursor, NodeStatus, PendingContext, PendingKind,
    PendingReason, Project, ReviewMode, Stage, StageConfig, SyncDecision, SyncDecisionKind, Task,
    TestResult,
};
use crate::{Error, Result};

/// 闸门结果（决策 62 / 139）：非零退出是**闸门结果**，不是节点错误。
struct GateOutcome {
    passed: bool,
    failure_kind: GateFailureKind,
    output: String,
}

/// 一次 pending → resume 边界的续接素材（决策 180，票 13）。
///
/// `from_run_id` 是**被续接的那条历史 run**，写进新 run 的 `continued_from_run_id`，
/// 供指标汇总排除被重复计入的输入 token（票 13 必要条件二）。
struct Continuation {
    messages: Vec<Message>,
    from_run_id: i64,
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
/// 把 `task_id` 从**进程内去重**里摘掉（决策 210⑧ / 票 09）。
///
/// 这是 `unstick` 的第一个动作，也是它存在的理由：去重集合是「同一任务只跑一个执行体」的
/// 进程内那一半（DB 那一半是 `executor_owner`），而它**只在执行体返回时**才释放。执行体
/// 卡在一个不返回的系统调用里（2026-09-17 实证：`git2` 的 `open()` 被 macOS 拦住）时，
/// 去重永远摘不掉——清了 DB 也没用，重试会被逐次拒掉。
///
/// **代价如实记**：摘掉之后，那个仍然卡着的旧执行体若哪天活过来，可能与新执行体同时写库。
/// 这不是新增的风险面，而是原本那个僵死状态本来就有的（旧执行体已经不再写库，否则它不会
/// 被判定为「心跳停了」）。
pub fn force_release(task_id: &str) -> bool {
    EXECUTOR_REGISTRY.lock().unwrap().remove(task_id)
}

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
const OUTPUT_REVIEW_DIFF: &str = "review_diff";

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

    /// 入口：抢占单执行者 → 跑循环 → 释放。已有 executor 在跑时立即返回（决策 36）。
    pub async fn run(&self, task_id: &str) -> Result<()> {
        self.try_run(task_id).await.map(|_| ())
    }

    /// 同 [`Executor::run`]，但把「是否真正取得执行权」报告给调用方：
    /// `false` = 已有 executor 在跑同一任务（或 DB 乐观锁被跨进程占用），本次未执行任何节点。
    ///
    /// 生产 resume 钩子需要这个信号：resume / 审批请求若正好落在旧 executor
    /// 「已读完游标、尚未释放注册表」的窗口内，一次性 spawn 会被**静默丢弃**，
    /// 任务永久停在 pending。钩子据 `false` 做有界重试（旧 executor 退出是毫秒级）。
    pub async fn try_run(&self, task_id: &str) -> Result<bool> {
        let _guard = match try_acquire(task_id) {
            Some(g) => g,
            None => return Ok(false), // 已有 executor 在跑，直接返回（决策 36）
        };
        let owner = format!("executor:{}", ulid::Ulid::new());
        if !self.store.try_claim_executor(task_id, &owner).await? {
            return Ok(false); // DB 乐观锁被占（跨进程场景），跳过
        }
        let result = self.run_inner(task_id).await;
        self.store.release_executor(task_id).await?;
        result?;
        Ok(true)
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
                        // 单游标失败不传播（决策 89）。
                        // 可归因的 LLM 配置类失败（主流程票 03）：message 用中文可操作提示，
                        // 原始诊断进 pending.context.diagnostic（不拼进 message）。
                        let context = node_error
                            .llm_classified()
                            .map(|(_kind, raw)| PendingContext::with_diagnostic(raw));
                        self.pend_cursor_with_context(
                            &cursor,
                            PendingKind::RetryExhausted,
                            node_error.to_string(),
                            context,
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
        self.pend_cursor_with_context(cursor, kind, message, None)
            .await
    }

    /// 同上，但带 `context`（决策 130 ①）：`context.kind` 决定 `allowed_actions`
    /// 走哪一行专用动作集，缺失则落到 `(user_decision, _)` 通用兜底行。
    ///
    /// review 打回（`review`）与 test 闸门 code_issue / 复检（`test_code_issue` /
    /// `gate_recheck`）必须经此带上 kind，否则「打回开发修复」「修改测试用例」等
    /// 权威表里写好的动作永远下发不出来（票 05）。
    async fn pend_cursor_with_context(
        &self,
        cursor: &NodeCursor,
        kind: PendingKind,
        message: impl Into<String>,
        context: Option<PendingContext>,
    ) -> Result<()> {
        let mut reason = PendingReason::new(kind, cursor.stage, cursor.node, message);
        reason.context = context;
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

    /// 路由产出 `Pending` 时，按「待办来源」补 `context.kind`（决策 130 ① / 票 05）。
    ///
    /// 只有两类待办需要补，其余返回 `None`（走通用兜底行，语义不变）：
    /// - review 打回：`(Review, ValidateOutput)` 判不通过 → `review`；
    /// - test 闸门：`(Test, ValidateOutput)` 存在 code_issue → `test_code_issue`；
    ///   若本轮是 merge 测试闸门打回后的复检（`gate_recheck = true`）→ `gate_recheck`。
    async fn pending_context_for(
        &self,
        task: &Task,
        cursor: &NodeCursor,
        kind: PendingKind,
    ) -> Result<Option<PendingContext>> {
        if kind != PendingKind::UserDecision {
            return Ok(None);
        }
        match (cursor.stage, cursor.node) {
            (Stage::Review, Node::ValidateOutput) => Ok(Some(PendingContext::with_kind(
                crate::actions::kinds::REVIEW,
            ))),
            (Stage::Test, Node::ValidateOutput) => {
                // 复检标记取自本轮 test 产出（executor 在 test.execute 落库时置位，决策 109）
                let gate_recheck = self
                    .store
                    .stage_output_metadata(&task.id, Stage::Test, OUTPUT_TEST_REPORT)
                    .await?
                    .and_then(|m| m.get("gate_recheck").and_then(|v| v.as_bool()))
                    .unwrap_or(false);
                let kind = if gate_recheck {
                    crate::actions::kinds::GATE_RECHECK
                } else {
                    crate::actions::kinds::TEST_CODE_ISSUE
                };
                Ok(Some(PendingContext::with_kind(kind)))
            }
            _ => Ok(None),
        }
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
        let result = self.do_init(task, &project, run_id).await;
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

    async fn do_init(&self, task: &Task, project: &Project, run_id: i64) -> Result<()> {
        let worktree = self.store.home().worktree_path(&task.id);
        // 目标仓库脏不阻塞，仅记录警告（决策 61）。**这个检查是 best-effort 的**
        // （决策 209）：它换来的只是下面那条 warn，失败或超时都不能影响 init。
        //
        // 这里不写 `.await?` 是有来历的：2026-09-17 那次「任务一创建就永久卡住」，
        // 挂死点正是这一句里的 `git2::Repository::open`（未签名的 app 没有 `~/Documents`
        // 的访问授权，`open()` 被 macOS 拦住、永不返回）。一个只值一条警告的检查，
        // 把整个任务挂死了四小时。
        self.mark_step(run_id, "检查项目工作区是否脏").await;
        match Git.is_dirty(Path::new(&project.local_path)).await {
            Ok(true) => {
                tracing::warn!(task = %task.id, "项目工作区有未提交改动（不阻塞，决策 61）")
            }
            Ok(false) => {}
            Err(e) => tracing::warn!(
                task = %task.id,
                error = %e,
                "脏工作区检查失败或超时，按「不检查」继续（不阻塞，决策 61）"
            ),
        }
        self.mark_step(run_id, "创建隔离工作区（worktree）").await;
        Git.init_worktree(
            Path::new(&project.local_path),
            &task.id,
            &worktree,
            &project.default_branch,
        )
        .await?;
        self.mark_step(run_id, "把工作区与分支写回任务行").await;
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
        let result = self.do_done(task, run_id).await;
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

    async fn do_done(&self, task: &Task, run_id: i64) -> Result<()> {
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
            self.mark_step(run_id, "回收 worktree 与任务分支").await;
            let project = self.project(&task.project_id).await?;
            Git.remove_worktree(Path::new(&project.local_path), Path::new(worktree), true)
                .await?;
            if let Some(branch) = &task.branch_name {
                Git.delete_branch(Path::new(&project.local_path), branch)
                    .await?;
            }
        }
        self.mark_step(run_id, "置任务终态").await;
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
        self.mark_step(run_id, "解析合入基准（base_ref）").await;
        let base_ref = Git.base_ref(repo, &project.default_branch).await?;

        // (2) rebase 到基准（决策 74 / 96 / pipeline-spec §6）
        self.mark_step(run_id, "把任务分支 rebase 到基准").await;
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
        self.mark_step(run_id, "跑合入前的闸门（lint + 测试）")
            .await;
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
                conflict_files: auto_resolved.clone(),
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
            .merge_phase_b_inner(task, project, cursor, &mut stored, run_id)
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
        match result? {
            PhaseB::Merged => Ok(NodeOutput::Route(crate::pipeline::MetadataView::default())),
            // 挂起必须走 NodeOutput::Pending：由 advance_cursor 统一落 pending 并**不推进游标**。
            // 若这里返回 Route，route_merge 会因 approval=approved 放行到 done，
            // 造成「游标已终态但 pending 仍在」的矛盾状态（决策 61 / 132）。
            PhaseB::DirtyWorktree(reason) => Ok(NodeOutput::Pending(reason)),
        }
    }

    async fn merge_phase_b_inner(
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
        self.mark_step(run_id, "检查目标分支工作区是否干净").await;
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
        self.mark_step(run_id, "把任务分支合入默认分支").await;
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
        // 可归因的 LLM 配置类失败（主流程票 03）：跨 attempt 保留最近一次的
        // (类别, 原始诊断)，耗尽后随重试耗尽错误一起带给 pending——否则它会被
        // 下面那层 `Error::Validation` 包装吞掉，用户只剩一段没有任何指引的文本。
        let mut last_classified: Option<(String, String)> = None;
        // 续接素材只在**进循环之前**取一次（决策 180，票 13）：pending → resume 的标记是
        // 一次性的（取走即清零），而循环内第 2、3 次是 `agent_retry_max` 的干净重试——
        // 决策 33 的语义不变，它们拿到的永远是空起点。
        let continuation = self.take_continuation(cursor).await?;
        for round in 0..self.settings.agent_retry_max {
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
            // 本 run 续接了哪条历史（决策 180，票 13）：**只有 round 0 可能续接**
            // （`carried` 从第 2 轮起按构造是 `&[]`，决策 33 的干净重试），故链接也只落在这里。
            // 此前写成「只要 `continuation.is_some()` 就落链」，于是干净重试轮也指向那条历史；
            // 而 `metrics::total_tokens` 把被指到的历史排进排除集——同一段历史被排除两次，
            // 任务是 token **少算**不是双算，且读数随重试次数漂移。
            if round == 0 {
                if let Some(c) = &continuation {
                    self.store
                        .link_run_continuation(run_id, c.from_run_id)
                        .await?;
                }
            }
            self.sse.emit(SseEvent::NodeStarted {
                task_id: task.id.clone(),
                branch: cursor.branch.clone(),
                stage: cursor.stage,
                node: cursor.node,
                attempt,
                run_id,
            });
            let started = Instant::now();
            let carried: &[Message] = if round == 0 {
                continuation
                    .as_ref()
                    .map(|c| c.messages.as_slice())
                    .unwrap_or(&[])
            } else {
                // agent_retry_max 的干净对话重试（决策 33）不受续接影响
                &[]
            };
            match self
                .agent_attempt(task, &project, cursor, kind, run_id, attempt, carried)
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
                    last_classified = e
                        .llm_classified()
                        .map(|(k, r)| (k.to_string(), r.to_string()));
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
        // 分类信息穿透重试耗尽包装（主流程票 03）：message 保持「哪个节点 + 可操作提示」，
        // 原始诊断仍由 run_inner 写进 pending.context.diagnostic。
        match last_classified {
            Some((kind, raw)) => Err(Error::LlmClassified {
                kind,
                message: format!(
                    "agent 节点 {}.{} 重试耗尽：{last_error}",
                    cursor.stage, cursor.node
                ),
                raw,
            }),
            None => Err(Error::Validation(format!(
                "agent 节点 {}.{} 重试耗尽：{last_error}",
                cursor.stage, cursor.node
            ))),
        }
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

    /// 取本节点的续接素材（决策 180 / 205，票 13 / 01）。
    ///
    /// **两道条件**（决策 205 把原来的三道砍掉一道：那个「谁来决定开不开」的配置层整层退场）：
    ///
    /// ① 游标**刚从 pending 被 resume**，且**原因表说该续接**（[`crate::types::resume_continues`]）。
    ///    取数是一次性的（取走即清零），且只有人能按出这个边界——`validate_attempts` 的原地重试、
    ///    `agent_retry_max` 的干净重试、未耗尽的超时都不会置位（决策 33 不变：
    ///    **模型的自动失败重试不给续接，人的介入才给**）。
    /// ② 真有一条上一 attempt 的主 agent 会话行可读。
    ///
    /// 第 ② 条在「该续接却读不到」时**静默干净起跑**而不报错：这是票 13 必要条件一
    /// （`context_overflow` 退出路径补写会话行）修掉的那条路——修复之后它不该再发生，
    /// 但真发生时让节点继续跑仍优于让整条流水线停在一个诊断性错误上。
    async fn take_continuation(&self, cursor: &NodeCursor) -> Result<Option<Continuation>> {
        let Some(cause) = self
            .store
            .take_cursor_resume_cause(&cursor.cursor_id)
            .await?
        else {
            return Ok(None);
        };
        if !crate::types::resume_continues(cause) {
            return Ok(None);
        }
        let Some(conv) = self
            .store
            .latest_own_conversation(&cursor.task_id, cursor.stage, cursor.node)
            .await?
        else {
            return Ok(None);
        };
        let messages: Vec<Message> =
            serde_json::from_value(conv.messages_json.clone()).unwrap_or_default();
        if messages.is_empty() {
            return Ok(None);
        }
        Ok(Some(Continuation {
            messages,
            from_run_id: conv.run_id,
        }))
    }

    /// 单次 agent attempt：prompt 组装 → 工具循环 → 元数据抽取 → 节点后处理。
    /// 返回（结论，token 计量）。
    ///
    /// 参数多于 clippy 的默认阈值：`run_id` / `attempt` / `carried` 三者都是**本次尝试**的
    /// 入参，绑成结构体只是把同一份信息换个地方写，不改变调用点的可读性。
    #[allow(clippy::too_many_arguments)]
    /// 一次 agent 尝试的**外框**（票 01 / 决策 211①）：失败也要落会话，所以现场
    /// （`messages` / `tokens`）必须活到函数出口——`?` 会把它们一起带走，那正是
    /// 2026-09-17 实测里「失败的那一轮什么都不留」的机制。
    ///
    /// `persisted` 保证**一条 run 至多一条会话行**（决策 99）：成功路径已经写过时，
    /// 失败收尾只把错误上下文并进那一行，不再插新行。
    async fn agent_attempt(
        &self,
        task: &Task,
        project: &Project,
        cursor: &NodeCursor,
        kind: AgentNodeKind,
        run_id: i64,
        attempt: u32,
        carried: &[Message],
    ) -> Result<(NodeOutput, RunTokens)> {
        let mut trace = AttemptTrace {
            messages: carried.to_vec(),
            ..Default::default()
        };
        let result = self
            .agent_attempt_inner(task, project, cursor, kind, run_id, attempt, &mut trace)
            .await;
        if let Err(error) = &result {
            self.record_failed_attempt(task, cursor, run_id, attempt, &trace, error)
                .await;
        }
        result
    }

    /// 失败现场的落库。**落库失败不覆盖原错误**：调用方要带回去的是节点为什么失败，
    /// 不是记账为什么失败——后者只值一条 error 日志。
    async fn record_failed_attempt(
        &self,
        task: &Task,
        cursor: &NodeCursor,
        run_id: i64,
        attempt: u32,
        trace: &AttemptTrace,
        error: &Error,
    ) {
        let metadata = failure_metadata(error);
        let prompts = trace
            .prompts
            .as_ref()
            .map(|(system, user)| PromptSnapshot { system, user });
        let result = if trace.persisted {
            self.store
                .annotate_conversation_failure(&task.id, run_id, &metadata)
                .await
        } else {
            let msgs = match serde_json::to_value(&trace.messages) {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!(run_id, "失败会话不可序列化：{e}");
                    return;
                }
            };
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
                    prompts,
                    Some(&metadata),
                    trace.tokens.prompt,
                    trace.tokens.completion,
                )
                .await
                .map(|_| ())
        };
        if let Err(e) = result {
            tracing::error!(run_id, "失败会话落库失败（原错误仍照原样上报）：{e}");
        }
    }

    /// 一次尝试的**内里**：与外框同签名，外加现场。它专管「跑」，
    /// 出口的记账（成功写一行、失败补上下文）归 [`Self::agent_attempt`]。
    #[allow(clippy::too_many_arguments)]
    async fn agent_attempt_inner(
        &self,
        task: &Task,
        project: &Project,
        cursor: &NodeCursor,
        kind: AgentNodeKind,
        run_id: i64,
        attempt: u32,
        trace: &mut AttemptTrace,
    ) -> Result<(NodeOutput, RunTokens)> {
        let home = self.store.home().clone();
        home.ensure_task_dirs(&task.id)?;
        let worktree = task
            .worktree_path
            .clone()
            .unwrap_or_else(|| home.worktree_path(&task.id).display().to_string());
        let task_dir = home.task_dir(&task.id).display().to_string();

        let policy = FileToolPolicy::new(vec![worktree.clone().into(), task_dir.clone().into()]);
        // 阶段配置消费（§10.6.3 / 决策 22 / 46 / 111）：persona、采样参数、工具与技能增量。
        // **在构造执行器之前读**：环境层档位要喂给执行点那道闸（决策 206），而它来自
        // 这一份配置——构造完再读就得回头改执行器的状态。
        let stage_cfg = self.store.get_stage_config(cursor.stage.as_str()).await?;
        let env_mode = crate::types::effective_env_mode(
            self.settings.env_mode,
            cursor.stage.as_str(),
            stage_cfg.as_ref(),
        );
        let tools = ToolExecutor::new(
            home.clone(),
            policy,
            self.settings.clone(),
            self.killer.clone(),
        )
        .with_recorder(Arc::new(self.store.clone()))
        // 命令输出按行推流（票 14 / 决策 100）：长命令期间前端能看到增量输出。
        .with_sse(crate::agent::tools::CommandSse {
            sink: self.sse.clone(),
            task_id: task.id.clone(),
            branch: cursor.branch.clone(),
        })
        // 第三道闸（决策 206）：环境层按档位分三路。**不给它接提议通道**——流水线节点的
        // `ask` 档下写工具会被拒，那是刻意的：提议是**人的**确认钮的载体，而流水线节点
        // 背后没有人盯着，落一条没人会按的提议等于静默丢弃（决策 206 的档位是给
        // 「有值班经理看着」的值班长用的，流水线阶段要么 auto 要么 deny）。
        .with_env_mode(env_mode);
        if env_mode == crate::types::EnvMode::Ask {
            tracing::warn!(
                stage = cursor.stage.as_str(),
                "阶段被配成 ask 档：环境层工具会因没有提议通道而被拒（值班长的确认钮不服务流水线节点）"
            );
        }
        let persona = resolve_stage_persona(&home, stage_cfg.as_ref(), cursor.stage, cursor.node)?;
        let declared_tools =
            json_string_list(stage_cfg.as_ref().and_then(|c| c.tools_json.as_ref()));

        // 子代理（决策 172③，票 08）：**只有阶段显式声明才注入运行器**。不声明时
        // `spawn_sub_agent` 调用会拿到一句「未启用」的说明文本（工具层没有运行器），
        // 这就是「扩展工具、默认关闭」的落点。节点级超时作为该次调用的上限（票 08）。
        let sub_agent: Option<Arc<dyn crate::agent::SubAgentRunner>> = declared_tools
            .iter()
            .any(|t| t == crate::agent::SPAWN_SUB_AGENT_TOOL)
            .then(|| {
                let node_override =
                    crate::config::node_timeouts(stage_cfg.as_ref(), cursor.node.as_str());
                let max_duration = crate::config::effective_max_duration(
                    self.settings.node_max_duration_sec,
                    stage_cfg.as_ref().and_then(|c| c.max_duration_sec),
                    node_override,
                );
                Arc::new(crate::pipeline::subagent::StoreSubAgentRunner::new(
                    crate::pipeline::subagent::SubAgentRunnerConfig {
                        store: self.store.clone(),
                        settings: self.settings.clone(),
                        llm: self.llm.clone(),
                        killer: self.killer.clone(),
                        home: home.clone(),
                        task_id: task.id.clone(),
                        cursor_id: cursor.cursor_id.clone(),
                        stage: cursor.stage,
                        node: cursor.node,
                        attempt,
                        branch: cursor.branch.clone(),
                        parent_run_id: run_id,
                        worktree_path: worktree.clone().into(),
                        task_dir: task_dir.clone().into(),
                        project_root: PathBuf::from(&project.local_path),
                        language: project.language.clone(),
                        test_framework: project.test_framework.clone(),
                        temperature: stage_cfg.as_ref().and_then(|c| c.temperature),
                        max_tokens: stage_cfg.as_ref().and_then(|c| c.max_tokens),
                        env_mode,
                        max_duration: std::time::Duration::from_secs(max_duration),
                    },
                )) as Arc<dyn crate::agent::SubAgentRunner>
            });
        let tools = match sub_agent {
            Some(runner) => tools.with_sub_agent(runner),
            None => tools,
        };
        // 技能：阶段级 ∪ 节点级（决策 170 / 172④），再解析成渲染形态——全文态注入正文、
        // 名字态只列名字（正文交给 `Skill` 工具按需拉取，票 06）、目录态给出「还有哪些
        // 技能可用」（渐进披露）。技能根经 `Home::skills_dir` 取（默认 `{home}/skills`，
        // `[skills] dir` 可覆盖，决策 172）。
        let skills_root = home.skills_dir();
        let mut declared = crate::config::stage_skills(stage_cfg.as_ref())?;
        declared.extend(crate::config::node_skills(
            stage_cfg.as_ref(),
            cursor.node.as_str(),
        )?);
        let declared = effective_skills(&declared);
        let declared_names = crate::agent::baseline::effective_skill_names(&declared);
        // 声明的技能排在目录之前（保持 golden 顺序「先看已启用的」）
        let mut skills = crate::agent::skills::resolve(&skills_root, &declared)?;
        skills.extend(crate::agent::skills::catalogue(
            &skills_root,
            &declared_names,
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
            backtrack_feedback: architect_reentry_segment(
                &home,
                &task.id,
                cursor.stage,
                cursor.node,
                "backtrack-feedback.md",
            ),
            user_input: architect_reentry_segment(
                &home,
                &task.id,
                cursor.stage,
                cursor.node,
                "user-input.md",
            ),
            gate_recheck: self.gate_recheck_segment(task, cursor).await?,
            review_required_changes: self.review_required_changes_segment(task, cursor).await?,
            retry_feedback: architect_reentry_segment(
                &home,
                &task.id,
                cursor.stage,
                cursor.node,
                "retry-feedback.md",
            ),
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
        // 原文落现场（票 02）：hash 与原文同时写——hash 是索引，原文是权威。
        trace.prompts = Some((system_prompt.clone(), user_prompt.clone()));

        // 压缩锚点的边界（决策 180，票 13 必要条件三）：`carried` 是**上一轮**的对话，
        // 它里面的 user 消息不得充当「本轮第一条 user 消息」这个锚点——否则载入历史后，
        // keep 预算会被上一轮的提问占掉。
        let carried_len = trace.messages.len();
        let mut tool_failures = 0u32;
        let mut submitted: Option<serde_json::Value> = None;

        // L0 容量预估（决策 110 / 票 04）：窗口来自解析后的 provider 行
        // （`providers.context_window`，决策 46 / 111）。无可用 provider（FakeAgent /
        // 纯代码场景）时跳过分档——不臆造窗口；provider 存在但窗口未登记则显式失败。
        let capacity = self
            .model_context_window(task, cursor, stage_cfg.as_ref())
            .await?
            .map(|model_window| {
                crate::agent::context::estimate_context_capacity(
                    model_window,
                    &system_prompt,
                    &user_prompt,
                    &self.settings,
                )
            });

        loop {
            // L3 按轮压缩（决策 105）：估算当前 messages 是否超过软限，超了就规则化压缩。
            // 压缩本身不调 LLM（§12.13.3 规则表），压缩发生时有可观测记录。
            if let Some(capacity) = capacity {
                if let Some(reason) = self
                    .enforce_context_budget(
                        task,
                        cursor,
                        capacity,
                        &system_prompt,
                        &user_prompt,
                        carried_len,
                        &mut trace.messages,
                    )
                    .await?
                {
                    // L4：压缩后仍超硬限 → 本节点收口为 pending(context_overflow)
                    //
                    // 票 13 的必要条件一（决策 180）：这条退出路径在会话落库**之前**返回，
                    // 于是「开了续接却读不到上一轮」会是一条静默无效的路。先补写会话行，
                    // 再返回 pending——它正是续接最需要的那个失败现场。
                    let msgs = serde_json::to_value(&trace.messages)?;
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
                            trace
                                .prompts
                                .as_ref()
                                .map(|(system, user)| PromptSnapshot { system, user }),
                            None,
                            trace.tokens.prompt,
                            trace.tokens.completion,
                        )
                        .await?;
                    // 这一条 run 的会话行已经写过（外框的失败收尾只补上下文，不再插行）
                    trace.persisted = true;
                    return Ok((NodeOutput::Pending(reason), trace.tokens));
                }
            }

            let req = LlmRequest {
                stage: cursor.stage,
                node: cursor.node,
                attempt,
                system_prompt: system_prompt.clone(),
                user_prompt: user_prompt.clone(),
                messages: trace.messages.clone(),
                tools: tool_defs(
                    kind,
                    &declared_tools,
                    &skills,
                    env_mode,
                    cursor.stage,
                    cursor.node,
                )?,
                temperature: stage_cfg.as_ref().and_then(|c| c.temperature),
                max_tokens: stage_cfg.as_ref().and_then(|c| c.max_tokens),
                // 任务级 provider 覆盖（决策 105）；阶段配置 / 系统默认由生产适配器解析
                provider_id: task.model_override.clone(),
                run: Some(crate::agent::client::RunContext {
                    task_id: task.id.clone(),
                    session_id: String::new(),
                    branch: cursor.branch.clone(),
                    run_id,
                    agent_type: "main".into(),
                }),
            };
            let response = self.llm.complete(req).await?;
            trace.tokens.add(&response);
            trace.messages.push(Message::assistant(
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
                    session_id: None,
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
                        trace
                            .messages
                            .push(Message::tool_result(call, outcome.content));
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
                        trace
                            .messages
                            .push(Message::tool_result(call, format!("工具执行失败：{e}")));
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
                    content: trace.messages.iter().rev().find_map(|m| m.content.clone()),
                    ..Default::default()
                };
                let extracted = crate::agent::metadata::extract_metadata(&final_resp);
                extracted.value.ok_or_else(|| {
                    Error::Validation(extracted.error.unwrap_or_else(|| "缺少结构化元数据".into()))
                })?
            }
        };
        kind.validate(&value)?;

        // decision 134 / 135：agent 型 validate_output 首判不合格 → 同步调用异族复判。
        // 复判合格（与首判分歧）→ 节点内直接 pending(user_decision, judge_disagreement)，
        // **不进入路由**（路由只看到两侧一致不合格）。
        let mut judge_disagreement: Option<PendingReason> = None;
        if kind == AgentNodeKind::DesignValidateOutput && self.settings.cross_family_judge {
            let first: crate::types::ValidateOutputMetadata =
                serde_json::from_value(value.clone())?;
            if !first.passed {
                let prompt = format!(
                    "请复核上游 validate_output 对 {} 阶段产出「不合格」的判定。\n\
                     阶段：{}\n节点：{}\n首判元数据：{}\n\
                     若你认为产出实际合格，请 submit_metadata passed=true；否则 passed=false。",
                    cursor.stage, cursor.stage, cursor.node, value
                );
                let (cross_value, _cross_tokens) = self
                    .call_pseudo_stage(
                        task,
                        cursor,
                        run_id,
                        attempt,
                        PseudoStage::ValidatorCrossCheck,
                        prompt,
                    )
                    .await?;
                let cross: CrossCheckResult = parse_metadata(&cross_value)?;
                if matches!(
                    crate::pipeline::resolve_validate_output(true, false, Some(cross.passed)),
                    crate::pipeline::ValidateOutcome::JudgeDisagreement
                ) {
                    let detail = if cross.blockers.is_empty() {
                        String::new()
                    } else {
                        format!("（复判备注：{}）", cross.blockers.join("；"))
                    };
                    judge_disagreement = Some(
                        PendingReason::new(
                            PendingKind::UserDecision,
                            cursor.stage,
                            cursor.node,
                            format!("异族复判与首判分歧：首判不合格、复判合格，请用户终审{detail}"),
                        )
                        .with_context(PendingContext::with_kind(
                            crate::actions::kinds::JUDGE_DISAGREEMENT,
                        )),
                    );
                }
            }
        }

        // 会话落库（§12.4.3；1:1 对调 LLM 的 run，决策 99）
        let msgs = serde_json::to_value(&trace.messages)?;
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
                trace
                    .prompts
                    .as_ref()
                    .map(|(system, user)| PromptSnapshot { system, user }),
                Some(&value),
                trace.tokens.prompt,
                trace.tokens.completion,
            )
            .await?;
        // 这一条 run 的会话行已经写过：后面若在 `post_process` 上失败，外框只把错误
        // 上下文并进这一行（票 01），不会再插一行——1:1 的口径不因失败路径而破。
        trace.persisted = true;
        self.store.refresh_task_totals(&task.id).await?;

        // 分歧路径在节点内直接置 pending，不经 post_process / 路由（决策 135）
        if let Some(reason) = judge_disagreement {
            return Ok((NodeOutput::Pending(reason), trace.tokens));
        }

        let output = kind
            .post_process(self, task, cursor, run_id, attempt, value)
            .await?;
        Ok((output, trace.tokens))
    }

    // ─────────────────────── 伪阶段（决策 48 / 60 / 67 / 88 / 100 / 113 / 134）───────────────────────

    /// 同步调用一个伪阶段（不占游标）：落独立 run + 会话行，心跳归父 run。
    ///
    /// `run.agent_type = pseudo:*`（FakeAgent 据此路由脚本）；`cursor_id` 继承父游标
    /// （决策 113）；`agent_type` 非 `system` → 计入 `total_calls`（决策 130 ②）。
    async fn call_pseudo_stage(
        &self,
        task: &Task,
        cursor: &NodeCursor,
        parent_run_id: i64,
        attempt: u32,
        pseudo: PseudoStage,
        user_prompt: String,
    ) -> Result<(serde_json::Value, RunTokens)> {
        let project = self.project(&task.project_id).await?;
        let home = self.store.home().clone();
        let worktree = task
            .worktree_path
            .clone()
            .unwrap_or_else(|| home.worktree_path(&task.id).display().to_string());
        let task_dir = home.task_dir(&task.id).display().to_string();
        let stage_cfg = self.store.get_stage_config(pseudo.stage_key()).await?;

        // persona：persona_path 优先，否则内嵌（决策 7 / 87）；persona_append 追加
        let mut persona = match stage_cfg.as_ref().and_then(|c| c.persona_path.as_deref()) {
            Some(path) => {
                let p = home.root().join(path);
                std::fs::read_to_string(&p).map_err(|e| {
                    Error::Config(format!(
                        "伪阶段 {} 的 persona_path 不可读：{}（{e}）",
                        pseudo.stage_key(),
                        p.display()
                    ))
                })?
            }
            None => pseudo.embedded_persona().to_string(),
        };
        if let Some(append) = stage_cfg.as_ref().and_then(|c| c.persona_append.as_deref()) {
            if !append.trim().is_empty() {
                persona.push_str(&format!("\n\n{append}"));
            }
        }
        let system_prompt = build_system_prompt(
            &load_agents_context(
                Path::new(&project.local_path),
                project.language.as_deref(),
                project.test_framework.as_deref(),
            ),
            &persona,
            &workdirs_line(&worktree, &task_dir),
            &[],
        );

        let run_id = self
            .store
            .insert_run(&NewRun {
                task_id: task.id.clone(),
                cursor_id: cursor.cursor_id.clone(),
                stage: cursor.stage,
                node: cursor.node,
                attempt,
                agent_type: pseudo.agent_type().to_string(),
                parent_run_id: Some(parent_run_id),
                prompt_template_hash: None,
                process_group_id: None,
            })
            .await?;

        let provider_id = crate::storage::catalog::resolve_provider_id(
            None,
            task.model_override.as_deref(),
            stage_cfg.as_ref(),
            None,
        );
        let request = LlmRequest {
            stage: cursor.stage,
            node: cursor.node,
            attempt,
            system_prompt: system_prompt.clone(),
            user_prompt: user_prompt.clone(),
            messages: Vec::new(),
            tools: vec![pseudo.submit_tool()],
            temperature: stage_cfg.as_ref().and_then(|c| c.temperature),
            max_tokens: stage_cfg.as_ref().and_then(|c| c.max_tokens),
            provider_id,
            run: Some(crate::agent::client::RunContext {
                task_id: task.id.clone(),
                branch: cursor.branch.clone(),
                run_id,
                agent_type: pseudo.agent_type().to_string(),
                session_id: String::new(),
            }),
        };

        let started = Instant::now();
        let response = match self.llm.complete(request).await {
            Ok(r) => r,
            Err(e) => {
                self.store
                    .finish_run(
                        run_id,
                        &RunOutcome {
                            status: Some(NodeStatus::Failed),
                            duration_ms: started.elapsed().as_millis() as u64,
                            error: Some(e.to_string()),
                            ..Default::default()
                        },
                    )
                    .await?;
                return Err(e);
            }
        };
        // 心跳写父 run（决策 88：伪阶段不得让父节点被空闲超时误杀）
        let _ = self.store.touch_run_heartbeat(parent_run_id).await;
        let mut tokens = RunTokens::default();
        tokens.add(&response);
        self.store
            .finish_run(
                run_id,
                &RunOutcome {
                    status: Some(NodeStatus::Success),
                    duration_ms: started.elapsed().as_millis() as u64,
                    prompt_tokens: tokens.prompt,
                    completion_tokens: tokens.completion,
                    cache_read_tokens: tokens.cache_read,
                    cache_write_tokens: tokens.cache_write,
                    ..Default::default()
                },
            )
            .await?;

        let extracted = crate::agent::metadata::extract_metadata(&response);
        let value = extracted.value.ok_or_else(|| {
            Error::Validation(
                extracted
                    .error
                    .unwrap_or_else(|| "伪阶段缺少结构化元数据".into()),
            )
        })?;
        // 伪阶段独立会话行（决策 100）
        let msgs = serde_json::to_value(vec![Message::assistant(
            response.content.clone(),
            response.tool_calls.clone(),
        )])?;
        self.store
            .insert_conversation(
                &task.id,
                run_id,
                cursor.stage,
                cursor.node,
                attempt,
                pseudo.agent_type(),
                Some(parent_run_id),
                &msgs,
                Some(PromptSnapshot {
                    system: &system_prompt,
                    user: &user_prompt,
                }),
                Some(&value),
                tokens.prompt,
                tokens.completion,
            )
            .await?;
        self.store.refresh_task_totals(&task.id).await?;
        Ok((value, tokens))
    }

    /// project_analysis 伪阶段（decision 48 / 78 / 130）：确定性探测事实由调用方给出，
    /// 伪阶段只负责写人读摘要并标注可疑项，**合并**进分析结果。
    ///
    /// 项目级调用没有 task / 游标，故不落 run 行（v1 的 app 接线由票 17/20 完成）。
    pub async fn project_analysis(
        &self,
        project: &Project,
        mut facts: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let stage_cfg = self
            .store
            .get_stage_config(PseudoStage::ProjectAnalysis.stage_key())
            .await?;
        let persona = match stage_cfg.as_ref().and_then(|c| c.persona_path.as_deref()) {
            Some(path) => std::fs::read_to_string(self.store.home().root().join(path))
                .map_err(|e| Error::Config(format!("project_analysis persona_path 不可读：{e}")))?,
            None => PseudoStage::ProjectAnalysis.embedded_persona().to_string(),
        };
        let system_prompt = build_system_prompt(
            &load_agents_context(
                Path::new(&project.local_path),
                project.language.as_deref(),
                project.test_framework.as_deref(),
            ),
            &persona,
            &workdirs_line(&project.local_path, &project.local_path),
            &[],
        );
        let user_prompt = format!(
            "以下是确定性探测得到的事实清单（JSON）：\n{facts}\n\n\
             请写一段人读摘要（summary）并列出可疑项（suspicious），用 submit_metadata 返回。"
        );
        let provider_id =
            crate::storage::catalog::resolve_provider_id(None, None, stage_cfg.as_ref(), None);
        let request = LlmRequest {
            stage: Stage::Init,
            node: Node::Execute,
            attempt: 1,
            system_prompt,
            user_prompt,
            messages: Vec::new(),
            tools: vec![PseudoStage::ProjectAnalysis.submit_tool()],
            temperature: stage_cfg.as_ref().and_then(|c| c.temperature),
            max_tokens: stage_cfg.as_ref().and_then(|c| c.max_tokens),
            provider_id,
            run: Some(crate::agent::client::RunContext {
                task_id: String::new(),
                branch: String::new(),
                run_id: 0,
                agent_type: PseudoStage::ProjectAnalysis.agent_type().to_string(),
                session_id: String::new(),
            }),
        };
        let response = self.llm.complete(request).await?;
        let extracted = crate::agent::metadata::extract_metadata(&response);
        let value = extracted.value.ok_or_else(|| {
            Error::Validation(
                extracted
                    .error
                    .unwrap_or_else(|| "project_analysis 缺少结构化元数据".into()),
            )
        })?;
        let result: crate::pipeline::pseudo::ProjectAnalysisResult = parse_metadata(&value)?;
        if let Some(obj) = facts.as_object_mut() {
            obj.insert("summary".into(), serde_json::Value::String(result.summary));
            obj.insert(
                "suspicious".into(),
                serde_json::to_value(result.suspicious)?,
            );
        }
        Ok(facts)
    }

    /// 语义第二层冲突检测（决策 60 / 67）：模块路径重叠但符号名无交集时，
    /// 同步调 `conflict_check`；`duplicate_risk = high` → pending(user_decision, duplicate_risk)。
    async fn semantic_conflict_check(
        &self,
        task: &Task,
        cursor: &NodeCursor,
        run_id: i64,
        attempt: u32,
    ) -> Result<Option<PendingReason>> {
        let mine = self.store.overlap_keys(&task.id).await?;
        let mut candidates: Vec<(String, String, Vec<String>, Vec<String>)> = Vec::new();
        for other in self
            .store
            .list_tasks(&crate::storage::tasks::TaskFilter {
                include_archived: false,
                ..Default::default()
            })
            .await?
        {
            if other.id == task.id || !other.status.is_active() {
                continue;
            }
            let theirs = self.store.overlap_keys(&other.id).await?;
            let module_overlap = mine
                .symbols
                .iter()
                .any(|(m, _)| theirs.symbols.iter().any(|(tm, _)| module_overlaps(m, tm)));
            let symbol_overlap = mine.symbols.iter().any(|s| theirs.symbols.contains(s));
            let file_overlap = mine.files.iter().any(|f| theirs.files.contains(f));
            // 模块路径重叠、符号名无交集、文件也无交集 → 需要语义层判断
            if module_overlap && !symbol_overlap && !file_overlap {
                candidates.push((
                    other.id.clone(),
                    other.title.clone(),
                    theirs.files.clone(),
                    theirs
                        .symbols
                        .iter()
                        .map(|(m, n)| format!("{m}::{n}"))
                        .collect(),
                ));
            }
        }
        if candidates.is_empty() {
            return Ok(None);
        }
        let ids: Vec<String> = candidates.iter().map(|c| c.0.clone()).collect();
        let mut prompt = format!(
            "本任务「{}」的架构设计新增符号所在模块与以下活跃任务重叠，但符号名无交集。\n\
             请判断是否存在语义重复（同一功能被两个任务各自实现，duplicate_risk = high）。\n\n\
             本任务新增符号：\n",
            task.title
        );
        for (m, n) in &mine.symbols {
            prompt.push_str(&format!("- {m}::{n}\n"));
        }
        prompt.push_str("\n候选任务：\n");
        for (id, title, files, symbols) in &candidates {
            prompt.push_str(&format!(
                "- 「{title}」（{id}）：文件 [{}]；符号 [{}]\n",
                files.join("、"),
                symbols.join("、")
            ));
        }
        prompt
            .push_str("\n请 submit_metadata 返回 duplicate_risk（low / medium / high）与 reason。");
        let (value, _tokens) = self
            .call_pseudo_stage(
                task,
                cursor,
                run_id,
                attempt,
                PseudoStage::ConflictCheck,
                prompt,
            )
            .await?;
        let result: ConflictCheckResult = parse_metadata(&value)?;
        if result.duplicate_risk != crate::types::DuplicateRisk::High {
            return Ok(None);
        }
        let detail = result
            .reason
            .as_deref()
            .map(|r| format!("：{r}"))
            .unwrap_or_default();
        Ok(Some(
            PendingReason::new(
                PendingKind::UserDecision,
                Stage::ArchitectDesign,
                Node::Execute,
                format!(
                    "语义重复风险（模块路径重叠、符号名无交集）{detail}；冲突任务：{}",
                    ids.join("、")
                ),
            )
            .with_context(PendingContext {
                kind: Some(crate::actions::kinds::DUPLICATE_RISK.to_string()),
                conflict_task_ids: ids,
                ..Default::default()
            }),
        ))
    }

    /// decision 85 / 109：test.execute 被 merge 测试闸门打回时，prompt 注入闸门完整日志
    /// + 失败用例，让 agent 重新判定 `failure_cause`。首轮（无闸门失败）为空不渲染。
    async fn gate_recheck_segment(
        &self,
        task: &Task,
        cursor: &NodeCursor,
    ) -> Result<Option<String>> {
        if cursor.stage != Stage::Test || cursor.node != Node::Execute {
            return Ok(None);
        }
        let Some(merge) = self.store.merge_metadata(&task.id).await? else {
            return Ok(None);
        };
        if merge.gate != Some(Gate::Fail)
            || !matches!(merge.gate_failure_kind, Some(GateFailureKind::Test) | None)
        {
            return Ok(None);
        }
        let mut out = String::new();
        // 决策 109 / 票 09：读闸门命令的**完整日志**（含被首尾预览裁掉的中间行），
        // 不再是 head/tail 预览。日志文件路径按 merge 闸门所在 stage 命名（覆盖写入可重入）；
        // 读取不到时回退 metadata 里的预览并显式标注（不静默）。
        let gate_log_path = self.store.home().task_file(
            &task.id,
            &format!("gate-output-{}.log", Stage::Merge.as_str()),
        );
        let full_log = std::fs::read_to_string(&gate_log_path)
            .ok()
            .filter(|s| !s.trim().is_empty());
        match full_log {
            Some(log) => {
                out.push_str("### 闸门失败完整日志\n");
                out.push_str(&truncate_gate_log(&log, GATE_INJECTION_LIMIT));
                out.push('\n');
            }
            None => {
                // 完整日志缺失（异常路径）：退回 metadata 预览并显式说明，**不静默**
                if let Some(log) = merge
                    .gate_failure_output
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                {
                    out.push_str("### 闸门失败输出（完整日志不可读，以下为首尾预览）\n");
                    out.push_str(log.trim());
                    out.push('\n');
                }
            }
        }
        if let Some(meta) = self
            .store
            .stage_output_metadata(&task.id, Stage::Test, OUTPUT_TEST_REPORT)
            .await?
        {
            if let Ok(t) = serde_json::from_value::<TestResult>(meta) {
                if !t.failures.is_empty() {
                    out.push_str("### 上一轮失败用例\n");
                    for f in &t.failures {
                        out.push_str(&format!(
                            "- {}：{}（{}）\n",
                            f.test_name,
                            f.error_message,
                            match f.failure_cause {
                                crate::types::FailureCause::TestIssue => "test_issue",
                                crate::types::FailureCause::CodeIssue => "code_issue",
                            }
                        ));
                    }
                }
            }
        }
        if out.trim().is_empty() {
            return Ok(None);
        }
        out.push_str("\n请基于以上闸门输出，为每个失败用例重新标注 failure_cause（test_issue / code_issue）。");
        Ok(Some(out))
    }

    /// 决策 133 / pipeline-spec §6：review 打回循环中，develop.execute 重入的 user prompt
    /// 追加 review 的必须修改项。
    ///
    /// 只在 `(Develop, Execute)` 渲染；修改项取自评审产出（review 报告 metadata），
    /// 不重新推断。首轮进入 develop 时尚无评审产出 → 不渲染；评审不通过但
    /// `required_changes` 为空 → 显式降级为「本次无结构化修改项」，不静默留空段。
    async fn review_required_changes_segment(
        &self,
        task: &Task,
        cursor: &NodeCursor,
    ) -> Result<Option<String>> {
        if cursor.stage != Stage::Develop || cursor.node != Node::Execute {
            return Ok(None);
        }
        let Some(meta) = self
            .store
            .stage_output_metadata(&task.id, Stage::Review, OUTPUT_REVIEW_REPORT)
            .await?
        else {
            return Ok(None); // 首轮：review 尚未执行
        };
        let Ok(review) = serde_json::from_value::<crate::types::ReviewResult>(meta) else {
            return Ok(None);
        };
        // 评审通过 → 不是打回，不渲染该段
        if review.approved {
            return Ok(None);
        }
        let source = review
            .review_report_path
            .as_deref()
            .unwrap_or("review-report.md");
        let mut out = format!("来源：评审报告 `{source}`（review 判定不通过）\n");
        if review.required_changes.is_empty() {
            // 显式降级：不让 agent 误以为「没有要求」
            out.push_str("本次无结构化修改项——请阅读上述评审报告，按其文字结论修改。\n");
        } else {
            out.push_str("本轮必须修改：\n");
            for change in &review.required_changes {
                let action = match change.action {
                    crate::types::FileAction::Create => "新增",
                    crate::types::FileAction::Modify => "修改",
                    crate::types::FileAction::Delete => "删除",
                };
                out.push_str(&format!("- {action} `{}`\n", change.path));
            }
        }
        Ok(Some(out))
    }

    /// 解析本次 LLM 调用的模型上下文窗口（决策 110 / 票 04）。
    ///
    /// 窗口来源是 `providers.context_window`（决策 46 / 111：随 provider 行存在一起，
    /// 前端可改）——「注册表」就是 provider 表本身。解析顺序与生产适配器一致
    /// （决策 129 四级：节点级 > 任务覆盖 > 阶段配置 > 系统默认首个 enabled）。
    ///
    /// 返回 `Ok(None)` 仅表示**根本没有可用 provider**（测试注入 FakeAgent / 纯代码场景）——
    /// 此时没有窗口可估，跳过 L0/L3/L4 分档，**不臆造一个窗口值**。
    /// 一旦解析到 provider 但窗口未登记（为 0），**显式失败**，不静默取默认（决策 110）。
    async fn model_context_window(
        &self,
        task: &Task,
        cursor: &NodeCursor,
        stage_cfg: Option<&StageConfig>,
    ) -> Result<Option<usize>> {
        let node_override =
            crate::storage::catalog::node_provider_override(stage_cfg, cursor.node.as_str());
        let providers = self.store.load_providers().await?;
        let resolved = crate::storage::catalog::resolve_provider_id(
            node_override.as_deref(),
            task.model_override.as_deref(),
            stage_cfg,
            providers.iter().find(|p| p.enabled).map(|p| p.id.as_str()),
        );
        let Some(provider_id) = resolved else {
            return Ok(None);
        };
        let provider = providers
            .into_iter()
            .find(|p| p.id == provider_id)
            .ok_or_else(|| {
                Error::Config(format!(
                    "provider {provider_id} 未注册，无法确定模型上下文窗口（决策 110）"
                ))
            })?;
        if provider.context_window == 0 {
            return Err(Error::Config(format!(
                "provider {}（{}）未登记 context_window，无法进行 L0 容量预估（决策 110）",
                provider.id, provider.model
            )));
        }
        Ok(Some(provider.context_window as usize))
    }

    /// 每轮 loop 前的上下文预算检查（决策 105 / 票 04）：超软限 → L3 按轮压缩；
    /// 压缩后仍超硬限 → L4 兜底（返回待挂的 `pending(context_overflow)` 理由，
    /// v1 的 L4 只有这两级——决策 154）。
    ///
    /// 返回 `Some(reason)` 表示调用方应立即把该节点的输出收口为这个 pending；
    /// `None` 表示预算内或压缩后已回到预算内，可继续本轮 LLM 调用。
    #[allow(clippy::too_many_arguments)]
    async fn enforce_context_budget(
        &self,
        task: &Task,
        cursor: &NodeCursor,
        capacity: crate::agent::context::ContextCapacity,
        system_prompt: &str,
        user_prompt: &str,
        carried_len: usize,
        messages: &mut Vec<Message>,
    ) -> Result<Option<PendingReason>> {
        let estimate = |msgs: &[Message]| {
            crate::agent::context::count_tokens(user_prompt)
                + crate::agent::context::count_tokens(system_prompt)
                + msgs
                    .iter()
                    .map(|m| {
                        // 上下文里既有文本，也有 assistant 的 tool_calls 参数
                        // （模型自己发出的 payload 同样占窗口，漏算会低估）
                        let text =
                            crate::agent::context::count_tokens(m.content.as_deref().unwrap_or(""));
                        let args: usize = m
                            .tool_calls
                            .iter()
                            .map(|c| {
                                crate::agent::context::count_tokens(&c.name)
                                    + crate::agent::context::count_tokens(&c.arguments)
                            })
                            .sum();
                        text + args
                    })
                    .sum::<usize>()
        };

        if !crate::agent::context::should_compact(estimate(messages), capacity) {
            return Ok(None);
        }
        // L3：规则化按轮压缩（不调 LLM，§12.13.3 规则表）
        let before = messages.len();
        let outcome = crate::agent::context::compact_messages_from(
            messages,
            self.settings.keep_recent_rounds,
            carried_len,
        );
        let after = outcome.messages.len();
        *messages = outcome.messages;
        // 压缩发生时有可观测记录（票面要求）
        tracing::info!(
            task = %task.id,
            stage = %cursor.stage,
            node = %cursor.node,
            before,
            after,
            compacted = outcome.compacted_messages,
            "上下文超过软限，已按轮压缩（§12.13 L3）"
        );

        if !crate::agent::context::over_hard_limit(estimate(messages), capacity) {
            return Ok(None);
        }
        // L4 兜底（决策 105 / 148⑦ / 154）：v1 的 L4 只有两级——压缩（上面那次）→ pending。
        // 设计阶梯里的第二级（按节点分批 / 拆子代理）**整体不做**，故这里没有「首选动作」可选，
        // 直接构造 pending。`spawn_sub_agent`（决策 172③）是模型可主动调用的只读能力，
        // 不在自动降级路径上——阶段声明了它也走这条。
        //
        // 关键：绝不能因为「首选动作未实现」就放行继续跑——那会让超硬限的节点无限循环。
        tracing::warn!(
            task = %task.id,
            stage = %cursor.stage,
            node = %cursor.node,
            "压缩后仍超硬限，挂 pending(context_overflow)（§12.13 L4）"
        );
        Ok(Some(PendingReason::new(
            crate::types::PendingKind::ContextOverflow,
            cursor.stage,
            cursor.node,
            "上下文压缩后仍超过硬限，请拆分任务 / 换长上下文模型 / 取消",
        )))
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

    /// 决策 124 / 票 13：人工评审前生成 `git diff {base_ref}..kanban/{task_id}`。
    ///
    /// 记为系统来源命令（落 `kanban_node_commands`，`source = system`），写任务目录
    /// `review-diff.diff`（覆盖写入可重入）并落 stage output（`output_type = review_diff`），
    /// 经既有文件下发端点取回。
    ///
    /// 基准不可达 / 缺分支名时不阻断评审——落空 diff 并记 `generated = false`，
    /// 让面板走「无 diff 时降级」而非整个节点失败（票据要求降级不报错）。
    async fn write_review_diff(&self, task: &Task, cursor: &NodeCursor) -> Result<()> {
        let project = self.project(&task.project_id).await?;
        let repo = Path::new(&project.local_path);
        let path = "review-diff.diff";
        let (run_id, attempt) = self.begin_run(task, cursor, "system").await?;
        let started = Instant::now();

        let base_ref = Git.base_ref(repo, &project.default_branch).await?;
        let branch = task.branch_name.clone();
        let (diff, generated) = match &branch {
            Some(branch) => {
                let range = format!("{base_ref}..{branch}");
                match Git.diff_range(repo, &range).await {
                    Ok(d) => (d, true),
                    Err(e) => {
                        tracing::warn!(task = %task.id, error = %e, "review-diff 生成失败，落空 diff 降级");
                        (String::new(), false)
                    }
                }
            }
            None => {
                tracing::warn!(task = %task.id, "人工评审缺少 branch_name，落空 diff");
                (String::new(), false)
            }
        };

        // 记为系统来源命令（决策 124：source = system），命令文本即等价 git 调用，
        // 让审计面能看出这次 diff 是怎么来的。
        let command = match &branch {
            Some(b) => format!("git diff {base_ref}..{b}"),
            None => "git diff（缺 branch_name）".to_string(),
        };
        self.record_system_command(task, run_id, cursor, &command, &diff, generated)
            .await?;

        self.store.home().ensure_task_dirs(&task.id)?;
        std::fs::write(self.store.home().task_file(&task.id, path), &diff)?;
        self.store
            .upsert_stage_output(
                &task.id,
                Stage::Review,
                OUTPUT_REVIEW_DIFF,
                path,
                Some(&serde_json::json!({
                    "base_ref": base_ref,
                    "branch": branch.clone(),
                    "bytes": diff.len(),
                    "generated": generated,
                })),
            )
            .await?;
        self.finish_run(
            run_id,
            task,
            cursor,
            attempt,
            false,
            started.elapsed().as_millis() as u64,
            None,
            &RunTokens::default(),
        )
        .await?;
        Ok(())
    }

    /// 把一条系统来源命令 + 结果写入 `kanban_node_commands`（决策 124 的「记为系统来源命令」）。
    async fn record_system_command(
        &self,
        task: &Task,
        run_id: i64,
        cursor: &NodeCursor,
        command: &str,
        output: &str,
        ok: bool,
    ) -> Result<()> {
        let command_id = self
            .store
            .record_start(CommandStart {
                task_id: Some(task.id.clone()),
                session_id: None,
                run_id: Some(run_id),
                stage: cursor.stage,
                node: cursor.node,
                source: CommandSource::System,
                command: crate::agent::sanitize::sanitize_command_line(command),
                cwd: self.store.home().root().display().to_string(),
            })
            .await?;
        let preview = crate::agent::tools::head_tail(output, 50, 100);
        self.store
            .record_finish(
                command_id,
                CommandFinish {
                    exit_code: Some(if ok { 0 } else { 1 }),
                    stdout_preview: Some(crate::agent::sanitize::sanitize_text(&preview)),
                    duration_ms: 0,
                    ..Default::default()
                },
            )
            .await?;
        Ok(())
    }

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
            // 决策 124 / 票 13：人工评审前由系统生成 `git diff {base_ref}..kanban/{task_id}`
            // （记为系统来源命令），写任务目录 `review-diff.diff` 并落 stage output
            // （output_type = review_diff），经既有文件下发端点交给任务详情评审面板。
            self.write_review_diff(task, cursor).await?;
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
                // 决策 130 ① / 票 05：为两类待办补上 `context.kind`，让权威表的专用动作行生效。
                let context = self.pending_context_for(task, cursor, kind).await?;
                self.pend_cursor_with_context(cursor, kind, message, context)
                    .await?;
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
                    // 跨阶段落点：与 `advance_after_judge_continue` 共用同一张查表（票 03）
                    match crate::pipeline::stage_landing(cursor.stage) {
                        crate::pipeline::StageLanding::Split => {
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
                        }
                        crate::pipeline::StageLanding::JoinBoundary => {
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
                        }
                        crate::pipeline::StageLanding::StageEntry(next, next_node) => {
                            let from = (cursor.stage, cursor.node);
                            let to = (next, next_node);
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
                        // 无下一阶段（done 之后）→ 无流转，终态判定在主循环
                        crate::pipeline::StageLanding::Terminal => {}
                    }
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

    /// 跑系统命令并记录 `kanban_node_commands`（source=system）。返回
    /// `(exit code, 输出预览)`；启动失败是节点错误，非零退出是**闸门结果**而非节点错误。
    async fn run_system_command(
        &self,
        task: &Task,
        run_id: i64,
        stage: Stage,
        node: Node,
        command: &str,
        cwd: &Path,
    ) -> Result<(i32, String)> {
        let sanitized = crate::agent::sanitize::sanitize_command_line(command);
        let command_id = self
            .store
            .record_start(CommandStart {
                task_id: Some(task.id.clone()),
                session_id: None,
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
                let note = format!("命令超时（{}s）", self.settings.test_command_timeout_sec);
                self.store
                    .record_finish(
                        command_id,
                        CommandFinish {
                            exit_code: Some(-1),
                            stdout_preview: None,
                            stderr_preview: Some(note.clone()),
                            duration_ms,
                            ..Default::default()
                        },
                    )
                    .await?;
                return Ok((-1, note));
            }
        };
        let stdout = crate::agent::sanitize::sanitize_text(&stdout);
        let stderr = crate::agent::sanitize::sanitize_text(&stderr);
        let stdout_preview = crate::agent::tools::head_tail(&stdout, 50, 100);
        let stderr_preview = crate::agent::tools::head_tail(&stderr, 50, 100);
        // 决策 109 / 票 09：闸门命令的**完整** stdout/stderr 落可读路径（路径确定、覆盖
        // 写入可重入），复检段读全文而非首尾预览。按 stage 命名，同一阶段的闸门重跑覆盖同一文件。
        let full_path = self
            .store
            .home()
            .task_file(&task.id, &format!("gate-output-{}.log", stage.as_str()));
        self.store.home().ensure_task_dirs(&task.id)?;
        let full_log = match (stdout.is_empty(), stderr.is_empty()) {
            (false, false) => format!("[stdout]\n{stdout}\n[stderr]\n{stderr}"),
            (false, true) => stdout.clone(),
            (true, false) => stderr.clone(),
            (true, true) => String::new(),
        };
        std::fs::write(&full_path, &full_log)?;
        self.store
            .record_finish(
                command_id,
                CommandFinish {
                    exit_code: Some(exit_code),
                    stdout_path: Some(full_path.display().to_string()),
                    stdout_preview: Some(stdout_preview.clone()),
                    stderr_preview: Some(stderr_preview.clone()),
                    duration_ms,
                },
            )
            .await?;
        self.store.touch_run_heartbeat(run_id).await?;
        // merge metadata 里的 `gate_failure_output` 保持原有的命令摘要 + 首尾预览（体积有界，
        // UI / 观测面消费）；**完整日志**已落上方 `full_path`，复检段按确定路径读全文
        // （决策 109 / 票 09）。
        let combined = match (
            stdout_preview.trim().is_empty(),
            stderr_preview.trim().is_empty(),
        ) {
            (false, false) => format!("{stdout_preview}\n{stderr_preview}"),
            (false, true) => stdout_preview,
            (true, false) => stderr_preview,
            (true, true) => String::new(),
        };
        Ok((exit_code, combined))
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
                self.mark_step(run_id, &format!("跑 lint：{lint}")).await;
                let (code, output) = self
                    .run_system_command(task, run_id, stage, node, lint, cwd)
                    .await?;
                if code != 0 {
                    return Ok(GateOutcome {
                        passed: false,
                        failure_kind: GateFailureKind::Lint,
                        output: gate_output("lint", lint, code, &output),
                    });
                }
            }
        }
        let test = test_command_for(project.test_framework.as_deref());
        self.mark_step(run_id, &format!("跑测试：{test}")).await;
        let (code, output) = self
            .run_system_command(task, run_id, stage, node, &test, cwd)
            .await?;
        if code != 0 {
            return Ok(GateOutcome {
                passed: false,
                failure_kind: GateFailureKind::Test,
                output: gate_output("测试", &test, code, &output),
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

    /// 下一个 `attempt` 序号 = 该 `(task, stage, node)` 上**节点自身**的 run 行数 + 1。
    ///
    /// 只数节点自身的 run（决策 172，票 14）：伪阶段 / 子代理复用父节点的 stage/node，
    /// 计入会把一次没重试的节点顶成 `attempt > 1`。取数口径见
    /// [`crate::storage::Store::count_node_owning_runs`]。
    async fn next_attempt(&self, task_id: &str, stage: Stage, node: Node) -> Result<u32> {
        Ok(self
            .store
            .count_node_owning_runs(task_id, stage, node)
            .await?
            + 1)
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
    /// 系统节点的**步边界留痕**（决策 211④ / 票 04）。
    ///
    /// **best-effort**：写不进去只 warn，不 `?`。上一个同族的教训是 `Git::is_dirty`
    /// 那个只值一条警告的检查把任务挂死了四小时（决策 209）——留痕本身更不能挂住关键路径。
    /// 它补偿的是那次挂死的全部信息量：卡在哪个系统调用，事后必须能从台账里读出来。
    async fn mark_step(&self, run_id: i64, step: &str) {
        if let Err(e) = self.store.set_run_step(run_id, step).await {
            tracing::warn!(run_id, step, "步骤留痕写不进去（不阻塞节点）：{e}");
        }
    }

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

use crate::pipeline::subagent::RunTokens;

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

/// 一次 agent 尝试的现场（票 01 / 决策 211①）。
///
/// 失败也要落会话，所以现场必须活到函数出口——`?` 会把内存里的 `messages` 一起带走，
/// 那正是那次实测「失败的那一轮什么都不留」的机制。`persisted` 保证一条 run 至多一条
/// 会话行（决策 99）：成功路径已经写过时，失败收尾只往那一行补错误上下文。
#[derive(Default)]
struct AttemptTrace {
    messages: Vec<Message>,
    tokens: RunTokens,
    /// 组装后的两段原文（票 02）：**成功与失败都要写**——「这是 prompt 问题」这句判断
    /// 在失败的那一轮才最需要证据。
    prompts: Option<(String, String)>,
    persisted: bool,
}

/// 失败会话写在 `metadata_json` 里的上下文（票 01）：读会话的人先看到它，才知道这条
/// 对话为什么停在这里。可归因的 LLM 失败额外带上类别与原始诊断——它们回答的是
/// 「该去改什么」，与 `error` 那句「发生了什么」不是一回事。
fn failure_metadata(error: &Error) -> serde_json::Value {
    let mut meta = serde_json::json!({
        "failed": true,
        "error": error.to_string(),
    });
    if let Some((kind, raw)) = error.llm_classified() {
        meta["classified"] = serde_json::json!({ "kind": kind, "raw": raw });
    }
    meta
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

/// 阶段 B 的结果：合入完成，或脏工作区挂起等用户处理（决策 61 / 132）。
enum PhaseB {
    Merged,
    DirtyWorktree(PendingReason),
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
        cursor: &NodeCursor,
        run_id: i64,
        attempt: u32,
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
                // 两层冲突检测的第一层（决策 53 / 60 / 71 / 102）
                let conflicts = ex.store.first_layer_conflicts(&task.id).await?;
                // 只有文件/符号真交集（High）才 conflict_wait；纯 name 重合是 Low，
                // 只告警不阻塞（决策 71② / 120）。
                let hard: Vec<_> = conflicts
                    .iter()
                    .filter(|w| w.duplicate_risk == Some(DuplicateRisk::High))
                    .collect();
                for warning in conflicts
                    .iter()
                    .filter(|w| w.duplicate_risk == Some(DuplicateRisk::Low))
                {
                    tracing::warn!(
                        task = %task.id,
                        other = %warning.task_id,
                        symbols = ?warning.overlapping_symbols,
                        "纯符号名重合：仅告警，不触发 conflict_wait（决策 71② / 120）"
                    );
                }
                if !hard.is_empty() {
                    let ids: Vec<String> = hard.iter().map(|w| w.task_id.clone()).collect();
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
                // 第二层：模块路径重叠但符号名无交集 → conflict_check 语义比对（决策 60 / 67）。
                // 命中 high → pending(user_decision, duplicate_risk)（决策 60 / 132）。
                if ex.settings.semantic_conflict_check {
                    if let Some(reason) = ex
                        .semantic_conflict_check(task, cursor, run_id, attempt)
                        .await?
                    {
                        return Ok(NodeOutput::Pending(reason));
                    }
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
                let mut m: crate::types::TestResult = serde_json::from_value(value.clone())?;
                let path = m
                    .test_report_path
                    .clone()
                    .unwrap_or_else(|| "test-report.md".into());
                // decision 109：被 merge 测试闸门打回后的复检，系统置 `gate_recheck = true`
                if let Some(merge) = ex.store.merge_metadata(&task.id).await? {
                    if merge.gate == Some(Gate::Fail)
                        && matches!(merge.gate_failure_kind, Some(GateFailureKind::Test) | None)
                    {
                        m.gate_recheck = true;
                    }
                }
                let persisted = serde_json::to_value(&m)?;
                ex.store
                    .upsert_stage_output(
                        &task.id,
                        Stage::Test,
                        OUTPUT_TEST_REPORT,
                        &path,
                        Some(&persisted),
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
/// schema 工具（决策 38：与校验同源，不可移除）。声明了未知工具名**报错**（不静默丢弃）。
///
/// `Skill`（决策 172③，票 06）**不进 [`MANDATORY_TOOLS`]**：它由阶段声明启用。但名字态与
/// 目录态技能的存在意义就是「正文由 `Skill` 工具按需拉取」——若阶段声明了任一非全文态
/// 技能却没声明 `Skill`，那批技能就是断腿的指针。因此这里给一条**自动放行**：只要有技能
/// 不处于全文态，就补上 `Skill` 工具定义，不要求用户在两处各配一遍。
///
/// `stage` / `node` 只进报错信息（阶段 + 节点）——故意传枚举而不是拼好的字符串：
/// 这条分支在正常路径上不可达（同上），不该为它每轮多分配一个 `String`。
fn tool_defs(
    kind: AgentNodeKind,
    declared: &[String],
    skills: &[crate::agent::skills::ResolvedSkill],
    env_mode: crate::types::EnvMode,
    stage: crate::types::Stage,
    node: crate::types::Node,
) -> crate::Result<Vec<ToolDef>> {
    use crate::agent::skills::SkillRender;

    let needs_skill_tool = skills
        .iter()
        .any(|s| matches!(s.render, SkillRender::Name | SkillRender::Catalogue { .. }));
    let mut defs: Vec<ToolDef> = Vec::new();
    // `deny` 档**连广告都不给**（决策 206）：环境层工具直接从 tool 定义里摘掉，
    // 而不是等模型发出来再拒一次。执行点那一道仍在（[`crate::agent::tools::ToolExecutor`]），
    // 两道都留是因为它们挡的不是同一种东西：这里挡「模型看见了一个不该给的选项」，
    // 那里挡「模型无视定义硬发」。
    //
    // 这是全仓**唯一**一处系统级设置压过强制基线的地方（[`MANDATORY_TOOLS`] 里含
    // `write_file` / `run_command`）——压的方向只有收紧一种，故它是安全的：阶段配置动不了它，
    // 只有全机档位可以。
    // 判据只有一处（`agent::tools::denied_by_tier`）：值班长那一侧的广告集与这里问的是同一个
    // 问题，两处各写一份谓词的后果是「一侧摘掉了、另一侧还广告着」这种只能靠现象定位的漂移。
    let denied = |name: &str| crate::agent::tools::denied_by_tier(name, env_mode);
    for name in effective_tools(declared) {
        if name == "submit_metadata" {
            continue; // 最后以 schema 形式追加
        }
        if denied(&name) {
            continue;
        }
        // 扩展工具（决策 172③，票 08）：不是内置工具，但**已实现**且由阶段声明启用。
        // 不认这一条的话，声明了 `spawn_sub_agent` 会在下面被当作「未实现」丢弃 + warn，
        // 于是声明与生效之间静默断开。
        //
        // `deny` 档的摘除**不在这里重复判**：上面那次 `denied` 已经把它挡下了
        // （它与环境层其余工具同归一层）——同一支里判两遍，第二遍永远走不到。
        if name == crate::agent::SPAWN_SUB_AGENT_TOOL {
            defs.push(spawn_sub_agent_tool_def());
            continue;
        }
        // v1 不认识的名字 = 配置错误，**拒绝**（决策 154 的后续票）。
        //
        // 这条分支在启动路径上不可达：`PUT /stage-configs` 与启动校验（`validate_startup`）
        // 用的是同一个判据 [`crate::agent::client::is_known_tool_name`]，那个名字根本写不进库。
        // 留着它是为了**不给同一个错误第二种处置**——手工改库绕过校验时，这里报错（报文也与
        // 校验同源，见 `client::unknown_tools_message`）而不是「静默丢弃 + 一条 warn」：
        // 后者会让「配置写了却没生效」只能靠翻日志发现。
        if !crate::agent::client::is_known_tool_name(&name) {
            return Err(crate::Error::Config(
                crate::agent::client::unknown_tools_message(
                    &format!("阶段 {stage} 节点 {node}"),
                    std::slice::from_ref(&name),
                ),
            ));
        }
        defs.push(ToolDef {
            name,
            description: String::new(),
            parameters: serde_json::json!({"type": "object"}),
        });
    }
    // 有名字态 / 目录态技能 → 自动带上 `Skill`（渐进披露的按需拉取入口）
    if needs_skill_tool && !denied(SKILL_TOOL) && !defs.iter().any(|d| d.name == SKILL_TOOL) {
        defs.push(skill_tool_def());
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
    Ok(defs)
}

/// `Skill` 工具的 tool 定义（决策 172③，票 06）。
///
/// 描述里点明「用技能目录里列出的名字」——渐进披露的闭环：模型从目录态看到可用技能，
/// 再凭名字来这里取正文。
fn skill_tool_def() -> ToolDef {
    ToolDef {
        name: SKILL_TOOL.to_string(),
        description: "按名字加载一个技能的正文（技能目录里列出的名字）。\
                      上游技能正文里的 `Call the Skill tool` 说的就是这个工具。"
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "技能名（见 system prompt 的技能目录）"
                }
            },
            "required": ["name"]
        }),
    }
}

/// `spawn_sub_agent` 工具的 tool 定义（决策 172③，票 08）。
///
/// 描述里明说**只读**：让模型知道子代理能做什么，才不会派它去写文件或跑命令而白等一轮。
fn spawn_sub_agent_tool_def() -> ToolDef {
    ToolDef {
        name: crate::agent::SPAWN_SUB_AGENT_TOOL.to_string(),
        description: "派生一个只读子代理处理可分解的检索子任务，返回摘要。\
                      子代理只能 read_file / list_dir，不能写文件或执行命令，也不再派子代理。\
                      适合「读很多文件、只要结论」的场景——原文留在子代理上下文，父上下文只收摘要。"
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "子任务描述：要检索什么、要回答什么问题、需要什么形态的结论"
                }
            },
            "required": ["task"]
        }),
    }
}

// ─────────────────────── prompt 组装辅助（票 12：§10.3 / G3 / G6 / G12）───────────────────────

/// 模块路径是否重叠（决策 60 第二层：模块路径重叠但符号名无交集）。
fn module_overlaps(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a == b || a.starts_with(&format!("{b}::")) || b.starts_with(&format!("{a}::"))
}

/// G12 工作目录行（system 的「工作目录」段与 user 的「环境路径」段共用，防漂移）。
fn workdirs_line(worktree: &str, task_dir: &str) -> String {
    format!("worktree：{worktree}\n任务目录：{task_dir}")
}

/// architect-design 重入时从任务目录注入的反馈文件段（首轮为空不渲染）。
///
/// 三处注入同构、只差文件名（决策 126 / 79 / 138），共用此读取器：
/// - `backtrack-feedback.md`：sync-check backtrack 的双方 blockers（决策 126）；
/// - `user-input.md`：`info_insufficient` 的用户补充输入（决策 79 / 票 08）；
/// - `retry-feedback.md`：develop / test 重试耗尽回架构设计的失败摘要（决策 138）。
fn architect_reentry_segment(
    home: &Home,
    task_id: &str,
    stage: Stage,
    node: Node,
    file: &str,
) -> Option<String> {
    if stage != Stage::ArchitectDesign || !matches!(node, Node::ValidateInput | Node::Execute) {
        return None;
    }
    std::fs::read_to_string(home.task_file(task_id, file))
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

/// 闸门失败输出（决策 109）：命令 + 退出码 + stdout/stderr 预览，注入 test.execute 复检 prompt。
fn gate_output(kind: &str, command: &str, code: i32, output: &str) -> String {
    let output = output.trim();
    if output.is_empty() {
        format!("{kind}命令 `{command}` 退出码 {code}")
    } else {
        format!("{kind}命令 `{command}` 退出码 {code}\n{output}")
    }
}

/// 闸门复检注入体积上界（决策 109 / 票 09）：约 120k 字符。
///
/// 决策要求注入 `kanban_node_commands` 的**完整日志**；但注入必须有上界，否则一份
/// 超大闸门日志会挤爆复检 prompt。超限时**显式**保留首尾并写明省略了多少字符
/// （并给出完整日志路径），**不静默回退到预览**——票据明确禁止无声降级。
const GATE_INJECTION_LIMIT: usize = 120_000;

/// 显式截断：保留首尾并标注省略量（不静默丢内容）。
fn truncate_gate_log(log: &str, limit: usize) -> String {
    if log.chars().count() <= limit {
        return log.to_string();
    }
    // 按字符切（日志可能含中文），避免在 UTF-8 边界截断
    let head_n = limit * 2 / 3;
    let tail_n = limit - head_n;
    let chars: Vec<char> = log.chars().collect();
    let head: String = chars[..head_n].iter().collect();
    let tail: String = chars[chars.len() - tail_n..].iter().collect();
    format!(
        "{head}\n\n...[闸门日志超长：已省略中间 {} 字符；完整日志见上方 stdout_path]...\n\n{tail}",
        chars.len() - limit
    )
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
            architect_reentry_segment(
                &home,
                "t1",
                Stage::ArchitectDesign,
                Node::ValidateInput,
                "backtrack-feedback.md"
            ),
            None
        );

        std::fs::write(
            home.task_file("t1", "backtrack-feedback.md"),
            "dev blockers：[\"缺少数据流定义\"]\n",
        )
        .unwrap();
        assert!(architect_reentry_segment(
            &home,
            "t1",
            Stage::ArchitectDesign,
            Node::ValidateInput,
            "backtrack-feedback.md"
        )
        .is_some());
        assert!(architect_reentry_segment(
            &home,
            "t1",
            Stage::ArchitectDesign,
            Node::Execute,
            "backtrack-feedback.md"
        )
        .is_some());

        // 决策 126 的注入范围只有 validate_input / execute
        assert_eq!(
            architect_reentry_segment(
                &home,
                "t1",
                Stage::ArchitectDesign,
                Node::ValidateOutput,
                "backtrack-feedback.md"
            ),
            None
        );
        assert_eq!(
            architect_reentry_segment(
                &home,
                "t1",
                Stage::Develop,
                Node::Execute,
                "backtrack-feedback.md"
            ),
            None
        );

        // 空文件（纯空白）不渲染
        std::fs::write(home.task_file("t1", "backtrack-feedback.md"), "  \n").unwrap();
        assert_eq!(
            architect_reentry_segment(
                &home,
                "t1",
                Stage::ArchitectDesign,
                Node::ValidateInput,
                "backtrack-feedback.md"
            ),
            None
        );
    }

    #[test]
    fn module_overlap_detection() {
        assert!(module_overlaps("auth", "auth"));
        assert!(module_overlaps("crate::auth", "crate::auth::login"));
        assert!(module_overlaps("crate::auth::login", "crate::auth"));
        assert!(!module_overlaps("auth", "billing"));
        // 前缀相同但不是模块边界（auth vs authorize）不算重叠
        assert!(!module_overlaps("auth", "authorize"));
        assert!(!module_overlaps("", "auth"));
    }

    #[test]
    fn gate_output_includes_log_when_present() {
        assert_eq!(
            gate_output("测试", "cargo test", 1, "  "),
            "测试命令 `cargo test` 退出码 1"
        );
        let with_log = gate_output("测试", "cargo test", 1, "FAILED: test_login\n");
        assert!(with_log.contains("退出码 1"));
        assert!(with_log.contains("FAILED: test_login"));
    }

    #[test]
    fn gate_log_under_limit_is_returned_verbatim() {
        // 未超限：完整保留（含中间行，票据要求读全文而非首尾预览）
        let log = "line1\n".repeat(10);
        assert_eq!(truncate_gate_log(&log, 1000), log);
    }

    #[test]
    fn gate_log_over_limit_truncates_with_explicit_notice() {
        // 超限：显式截断并标注省略量，保留首尾，**不静默**丢内容
        let mid = "MIDDLE_OMITTED_MARKER\n";
        let log = format!("HEAD\n{}{}", mid.repeat(50), "TAIL\n");
        let out = truncate_gate_log(&log, 100);
        assert!(out.starts_with("HEAD"), "保留首部");
        assert!(out.ends_with("TAIL\n"), "保留尾部");
        assert!(out.contains("闸门日志超长"), "应有显式省略标注");
        assert!(out.contains("已省略中间"), "标注应写明省略量");
        assert!(out.len() < log.len(), "截断后应变短");
        // 中间行确实被省略（这正是首尾预览会丢的那段）
        assert!(!out.contains(&mid.repeat(50)));
    }

    #[test]
    fn gate_log_truncation_is_char_boundary_safe() {
        // 中文字符不得被按字节切开（否则输出非法 UTF-8 / 乱码）
        let log = "中".repeat(500);
        let out = truncate_gate_log(&log, 100);
        assert!(out.is_char_boundary(out.len()));
        assert!(out.chars().all(|c| c == '中'
            || ".\n[闸门日志超长：已省略中间 400 字符；完整日志见上方 stdout_path]".contains(c)));
    }

    /// 决策 154 的后续票：`tool_defs` 对未知工具名**报错**，不再「静默丢弃 + 一条 warn」。
    ///
    /// 这条分支在启动路径上不可达（配置根本写不进来），留着是为了**不给同一个错误第二种
    /// 处置**——手工改库绕过校验时行为与写入时一致：拒绝。故这条用例同时钉住报文形状。
    #[test]
    fn tool_defs_rejects_unknown_names_and_accepts_the_known_set() {
        let (stage, node) = (crate::types::Stage::Develop, crate::types::Node::Execute);
        let err = tool_defs(
            AgentNodeKind::DevelopExecute,
            &["read_file".to_string(), "web_search".to_string()],
            &[],
            crate::types::EnvMode::Auto,
            stage,
            node,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("web_search"), "{err}");
        assert!(
            err.contains("阶段 develop 节点 execute"),
            "报文须定位到阶段 + 节点：{err}"
        );
        assert!(err.contains("v1 已知工具集"), "{err}");

        // 已知集（含扩展工具）照旧出表：`spawn_sub_agent` 走它自己的定义分支
        let defs = tool_defs(
            AgentNodeKind::DevelopExecute,
            &["read_file".to_string(), "spawn_sub_agent".to_string()],
            &[],
            crate::types::EnvMode::Auto,
            stage,
            node,
        )
        .unwrap();
        assert!(defs.iter().any(|d| d.name == "read_file"));
        assert!(defs.iter().any(|d| d.name == "spawn_sub_agent"));
        assert!(defs.iter().any(|d| d.name == "submit_metadata"));
    }
}
