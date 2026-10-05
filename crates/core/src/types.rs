//! 领域类型（docs/data-model.md §4、docs/implementation.md §11.2）。
//!
//! 这里的枚举字符串值与 DB / JSON / API 契约一一对应，改动即契约变更。

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ─────────────────────────────── 阶段与节点 ───────────────────────────────

/// 流水线阶段（§1.1）。串行阶段恒单游标；develop-design / test-design 并行。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    Init,
    ArchitectDesign,
    DevelopDesign,
    TestDesign,
    SyncCheck,
    Develop,
    Review,
    Test,
    Merge,
    Done,
}

/// 阶段全序（流程图顺序）。
pub const ALL_STAGES: [Stage; 10] = [
    Stage::Init,
    Stage::ArchitectDesign,
    Stage::DevelopDesign,
    Stage::TestDesign,
    Stage::SyncCheck,
    Stage::Develop,
    Stage::Review,
    Stage::Test,
    Stage::Merge,
    Stage::Done,
];

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Init => "init",
            Stage::ArchitectDesign => "architect-design",
            Stage::DevelopDesign => "develop-design",
            Stage::TestDesign => "test-design",
            Stage::SyncCheck => "sync-check",
            Stage::Develop => "develop",
            Stage::Review => "review",
            Stage::Test => "test",
            Stage::Merge => "merge",
            Stage::Done => "done",
        }
    }

    /// 该阶段是否为并行分支（各占一条游标，汇入 sync-check join）。
    ///
    /// 阶段间流转（分支、join、kickback、backtrack）统一定义在
    /// `crate::pipeline::landing`，这里不再维护第二份会漂移的线性 next 表。
    pub fn is_parallel_branch(self) -> bool {
        matches!(self, Stage::DevelopDesign | Stage::TestDesign)
    }

    /// prompt 模板目录名（`prompts/{dir}/{node}.md`，§10.5）。
    pub fn prompt_dir(self) -> &'static str {
        match self {
            Stage::Init => "init",
            Stage::ArchitectDesign => "architect_design",
            Stage::DevelopDesign => "develop_design",
            Stage::TestDesign => "test_design",
            Stage::SyncCheck => "sync_check",
            Stage::Develop => "develop",
            Stage::Review => "review",
            Stage::Test => "test",
            Stage::Merge => "merge",
            Stage::Done => "done",
        }
    }
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Stage {
    type Err = crate::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ALL_STAGES
            .iter()
            .copied()
            .find(|st| st.as_str() == s)
            .ok_or_else(|| crate::Error::Validation(format!("未知阶段：{s}")))
    }
}

/// 阶段内节点（§1.2）。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Node {
    ValidateInput,
    Execute,
    ValidateOutput,
}

pub const ALL_NODES: [Node; 3] = [Node::ValidateInput, Node::Execute, Node::ValidateOutput];

impl Node {
    pub fn as_str(self) -> &'static str {
        match self {
            Node::ValidateInput => "validate_input",
            Node::Execute => "execute",
            Node::ValidateOutput => "validate_output",
        }
    }
}

impl fmt::Display for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Node {
    type Err = crate::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ALL_NODES
            .iter()
            .copied()
            .find(|n| n.as_str() == s)
            .ok_or_else(|| crate::Error::Validation(format!("未知节点：{s}")))
    }
}

// ─────────────────────────────── 任务状态 ───────────────────────────────

/// TaskStatus（决策 34 / 98）：`queued` 与 `waiting` 正交。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// 排队等并发名额（决策 98）。
    Queued,
    /// 等依赖任务完成。
    Waiting,
    /// 正在执行（至少一条可推进游标）。
    Running,
    /// 任一游标被阻塞（投影，决策 82）。
    Pending,
    Done,
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskStatus::Queued => "queued",
            TaskStatus::Waiting => "waiting",
            TaskStatus::Running => "running",
            TaskStatus::Pending => "pending",
            TaskStatus::Done => "done",
            TaskStatus::Failed => "failed",
            TaskStatus::Cancelled => "cancelled",
        }
    }

    /// 终态：done / failed / cancelled。
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            TaskStatus::Done | TaskStatus::Failed | TaskStatus::Cancelled
        )
    }

    /// 参与冲突比对 / 名额占用判定的活跃状态（决策 71 / 117）。
    pub fn is_active(self) -> bool {
        matches!(
            self,
            TaskStatus::Queued | TaskStatus::Waiting | TaskStatus::Running | TaskStatus::Pending
        )
    }

    /// 是否占用 `max_concurrent_tasks` 名额（决策 117：准入后、终态前恒占）。
    pub fn occupies_slot(self) -> bool {
        matches!(self, TaskStatus::Running | TaskStatus::Pending)
    }
}

impl FromStr for TaskStatus {
    type Err = crate::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "queued" => TaskStatus::Queued,
            "waiting" => TaskStatus::Waiting,
            "running" => TaskStatus::Running,
            "pending" => TaskStatus::Pending,
            "done" => TaskStatus::Done,
            "failed" => TaskStatus::Failed,
            "cancelled" => TaskStatus::Cancelled,
            other => return Err(crate::Error::Validation(format!("未知任务状态：{other}"))),
        })
    }
}

/// 游标状态（决策 80 / 82 / 113）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CursorStatus {
    Active,
    /// 已到 join 边界，等其余分支（决策 82 / 107）。
    WaitingJoin,
    Pending,
    /// 被合并 / 回退 / 重试取代的历史行，只读保留（决策 113）。
    Archived,
}

impl CursorStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            CursorStatus::Active => "active",
            CursorStatus::WaitingJoin => "waiting_join",
            CursorStatus::Pending => "pending",
            CursorStatus::Archived => "archived",
        }
    }
}

impl FromStr for CursorStatus {
    type Err = crate::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "active" => CursorStatus::Active,
            "waiting_join" => CursorStatus::WaitingJoin,
            "pending" => CursorStatus::Pending,
            "archived" => CursorStatus::Archived,
            other => return Err(crate::Error::Validation(format!("未知游标状态：{other}"))),
        })
    }
}

// ─────────────────────────────── 路由 ───────────────────────────────

/// 条件边类型（§11.2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// 进入下一阶段。
    Next,
    /// 重试 execute（validate_output 不通过）。
    Retry,
    /// 进入 pending；原因类型由路由决定（决策 94）。
    Pending(PendingKind),
    /// 不推进：merge 未审批 / approval=none|returned（决策 95 / 121）。
    NoOp,
    /// 打回 develop.execute（merge 冲突 / lint 闸门失败，决策 139）。
    KickbackDevelop,
    /// 跳回 test.execute（merge 测试闸门失败，决策 85）。
    GotoTest,
}

/// pending 原因类型（§4.2 PendingReason.type）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PendingKind {
    InfoInsufficient,
    ConflictWait,
    RetryExhausted,
    UserDecision,
    MergeApproval,
    HumanReview,
    DependencyFailed,
    ContextOverflow,
    Timeout,
    /// **人按下的暂停**（手动暂停，`POST /tasks/{id}/pause`）。
    ///
    /// 它不是「流水线遇到了要人拍板的事」，而是**人自己把任务按住**：位置保留、在飞的那一轮
    /// 收口，等人放行。用 pending 承载而不是新造一个 `TaskStatus`，因为这套系统里 pending
    /// 本来就是「停着等人」的那一格（`docs/implementation.md` 的伪码写的是「非空 → 暂停
    /// （等 resume）」）——续跑、看板投影、调度器的准入与超时处置全都不必为它新开一条路。
    ///
    /// 与别的 pending 的**唯一区别**：它不需要任何人管（那正是人自己按下的），故调度器的
    /// 提醒 / 待办发现会跳过它（[`crate::pipeline::cursor::is_human_hold`]）。
    UserPaused,
}

impl PendingKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PendingKind::InfoInsufficient => "info_insufficient",
            PendingKind::ConflictWait => "conflict_wait",
            PendingKind::RetryExhausted => "retry_exhausted",
            PendingKind::UserDecision => "user_decision",
            PendingKind::MergeApproval => "merge_approval",
            PendingKind::HumanReview => "human_review",
            PendingKind::DependencyFailed => "dependency_failed",
            PendingKind::ContextOverflow => "context_overflow",
            PendingKind::Timeout => "timeout",
            PendingKind::UserPaused => "user_paused",
        }
    }
}

impl FromStr for PendingKind {
    type Err = crate::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "info_insufficient" => PendingKind::InfoInsufficient,
            "conflict_wait" => PendingKind::ConflictWait,
            "retry_exhausted" => PendingKind::RetryExhausted,
            "user_decision" => PendingKind::UserDecision,
            "merge_approval" => PendingKind::MergeApproval,
            "human_review" => PendingKind::HumanReview,
            "dependency_failed" => PendingKind::DependencyFailed,
            "context_overflow" => PendingKind::ContextOverflow,
            "timeout" => PendingKind::Timeout,
            "user_paused" => PendingKind::UserPaused,
            other => {
                return Err(crate::Error::Validation(format!(
                    "未知 pending 类型：{other}"
                )))
            }
        })
    }
}

/// resume 的原因（决策 205）：游标离开 pending 的那一刻，被清掉的是哪个原因。
///
/// **为什么需要它**：续不续接上一段对话，由**原因**决定，不由阶段 / 节点参数决定。
/// 换掉一个 bool（`resumed_from_pending`）是因为那个 bool 把原因抹掉了——而
/// 「信息不足被打回」与「合入提案通过」都要续接的话，两者对模型的意义完全不同。
///
/// **扁平枚举**，一个变体对应一个 `(PendingKind, context.kind)` 的合法组合（外加两个
/// 由「人按了哪颗键」决定的分支：merge 与 review 的通过与驳回在 pending 原因上同名）。
/// 扁平而不是嵌套，是为了让判定表能一眼读完——嵌套会让「这一条到底 true 还是 false」
/// 需要两次跳转才能回答。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResumeCause {
    // ── 由 pending 原因分类（`classify`）──
    InfoInsufficient,
    ConflictWait,
    RetryExhausted,
    ContextOverflow,
    Timeout,
    /// 超时梯子第 3 档：**空白重跑**（决策 320 / 376 裁决②）。与 [`ResumeCause::Timeout`]
    /// 分开是因为去向的**形态**不同——`Timeout` 带全卷转录续接，本档改带一份简报
    /// （任务描述 + 产物清单 + 未提交改动 + 最近收口摘要，票 04）。它不由 pending 原因
    /// 分类出来（`classify` 不返回它）：超时路径的游标从未 pending 过，由
    /// `scheduler::handle_timeout` 的降级档用 `mark_cursor_continuation` 直接置位。
    TimeoutBlankRestart,
    DependencyFailed,
    DependencyCancelled,
    /// `user_decision` 且没有 `context.kind`（通用那一行：跳过 / 取消）。
    UserDecision,
    DuplicateRisk,
    DevelopDesignInputInsufficient,
    TestDesignInputInsufficient,
    JudgeDisagreement,
    /// 评审驳回（`review`）：人按下「打回开发修复」。
    Review,
    TestCodeIssue,
    GateRecheck,
    DirtyWorktree,
    /// 人松开一次手动暂停（`user_paused` 的 `continue`）：从按住的那一处接着跑。
    UserPaused,
    /// 人按下「重跑本阶段」：**不带**上一段对话，重开一段。
    ///
    /// 它不是由 pending 原因分类出来的（`classify` 不返回它）：它由**按的是哪颗键**决定
    /// ——`user_paused` 那两条出口（续跑 / 重跑）在 pending 原因上同名，只有端点知道按的是
    /// 哪一颗，故重跑那条显式把它写进列（[`crate::pipeline::advance`] 的 `resume_cause`）。
    UserRerun,
    // ── 由人按的那颗键决定（merge / review 两条专用端点，同一原因有两个去向）──
    MergeApproved,
    MergeReturned,
    HumanReviewApproved,
    HumanReviewRejected,
    /// 迁移前的历史行、或认不出的取值。**兜底 false**：退回「续接出现之前的行为」。
    Unknown,
}

/// 全部原因（`as_str` / `from_str` 的往返用例按它逐条走）。
///
/// 新增一个变体时**先改这里**，再回答 `resume_continues` 那个穷尽 `match`——
/// 编译器会在后者报「未覆盖的模式」，这是本表的牙齿（决策 205：兜底 false 是安全网，
/// 不是让人忘记回答的借口）。
pub const ALL_RESUME_CAUSES: [ResumeCause; 24] = [
    ResumeCause::InfoInsufficient,
    ResumeCause::ConflictWait,
    ResumeCause::RetryExhausted,
    ResumeCause::ContextOverflow,
    ResumeCause::Timeout,
    ResumeCause::TimeoutBlankRestart,
    ResumeCause::DependencyFailed,
    ResumeCause::DependencyCancelled,
    ResumeCause::UserDecision,
    ResumeCause::DuplicateRisk,
    ResumeCause::DevelopDesignInputInsufficient,
    ResumeCause::TestDesignInputInsufficient,
    ResumeCause::JudgeDisagreement,
    ResumeCause::Review,
    ResumeCause::TestCodeIssue,
    ResumeCause::GateRecheck,
    ResumeCause::DirtyWorktree,
    ResumeCause::UserPaused,
    ResumeCause::UserRerun,
    ResumeCause::MergeApproved,
    ResumeCause::MergeReturned,
    ResumeCause::HumanReviewApproved,
    ResumeCause::HumanReviewRejected,
    ResumeCause::Unknown,
];

impl ResumeCause {
    /// DB 列（`kanban_node_cursors.resumed_from_pending_kind`）的取值。
    pub fn as_str(self) -> &'static str {
        match self {
            ResumeCause::InfoInsufficient => "info_insufficient",
            ResumeCause::ConflictWait => "conflict_wait",
            ResumeCause::RetryExhausted => "retry_exhausted",
            ResumeCause::ContextOverflow => "context_overflow",
            ResumeCause::Timeout => "timeout",
            ResumeCause::TimeoutBlankRestart => "timeout_blank_restart",
            ResumeCause::DependencyFailed => "dependency_failed",
            ResumeCause::DependencyCancelled => "dependency_cancelled",
            ResumeCause::UserDecision => "user_decision",
            ResumeCause::DuplicateRisk => "duplicate_risk",
            ResumeCause::DevelopDesignInputInsufficient => "develop_design_input_insufficient",
            ResumeCause::TestDesignInputInsufficient => "test_design_input_insufficient",
            ResumeCause::JudgeDisagreement => "judge_disagreement",
            ResumeCause::Review => "review",
            ResumeCause::TestCodeIssue => "test_code_issue",
            ResumeCause::GateRecheck => "gate_recheck",
            ResumeCause::DirtyWorktree => "dirty_worktree",
            ResumeCause::UserPaused => "user_paused",
            ResumeCause::UserRerun => "user_rerun",
            ResumeCause::MergeApproved => "merge_approved",
            ResumeCause::MergeReturned => "merge_returned",
            ResumeCause::HumanReviewApproved => "human_review_approved",
            ResumeCause::HumanReviewRejected => "human_review_rejected",
            ResumeCause::Unknown => "unknown",
        }
    }

    /// 认不出的取值 → `None`（调用方按 [`ResumeCause::Unknown`] 处置：兜底 false）。
    ///
    /// 刻意不实现 `std::str::FromStr`：那个 trait 要求有一个 `Err` 类型，而这里的
    /// 「认不出」是**正常情况**（库里的历史值、更新版本写下的值），不是错误。
    #[allow(clippy::should_implement_trait)]
    pub fn parse(raw: &str) -> Option<Self> {
        ALL_RESUME_CAUSES
            .iter()
            .copied()
            .find(|c| c.as_str() == raw)
    }

    /// 由被清掉的那个 pending 原因分类（决策 205 的「原因表」入口）。
    ///
    /// **穷尽 `match`**：`PendingKind` 新增一个变体时这里编译不过——那是刻意的，因为
    /// 新加一个 pending 原因时最该被问的就是「它续不续接」。`context_kind` 是自由串
    /// （可能来自库里的历史行），认不出的值退回该 `PendingKind` 的通用那一行。
    pub fn classify(kind: PendingKind, context_kind: Option<&str>) -> ResumeCause {
        let ctx = context_kind.unwrap_or_default();
        match kind {
            PendingKind::InfoInsufficient => ResumeCause::InfoInsufficient,
            PendingKind::ConflictWait => ResumeCause::ConflictWait,
            PendingKind::RetryExhausted => ResumeCause::RetryExhausted,
            PendingKind::ContextOverflow => ResumeCause::ContextOverflow,
            PendingKind::Timeout => ResumeCause::Timeout,
            PendingKind::DependencyFailed if ctx == "dependency_cancelled" => {
                ResumeCause::DependencyCancelled
            }
            PendingKind::DependencyFailed => ResumeCause::DependencyFailed,
            // merge 与 review 的通过与驳回在 pending 原因上同名，去向由端点决定
            // （`clear_pending_in_tx` 的显式分支）——走到分类说明那条路没走端点，
            // 于是退回「还没决定」这一档。
            PendingKind::MergeApproval => ResumeCause::MergeApproved,
            PendingKind::HumanReview => ResumeCause::HumanReviewApproved,
            PendingKind::UserDecision => match ctx {
                "duplicate_risk" => ResumeCause::DuplicateRisk,
                "develop_design_input_insufficient" => ResumeCause::DevelopDesignInputInsufficient,
                "test_design_input_insufficient" => ResumeCause::TestDesignInputInsufficient,
                "judge_disagreement" => ResumeCause::JudgeDisagreement,
                "review" => ResumeCause::Review,
                "test_code_issue" => ResumeCause::TestCodeIssue,
                "gate_recheck" => ResumeCause::GateRecheck,
                "dirty_worktree" => ResumeCause::DirtyWorktree,
                // 认不出的 context.kind：按**通用那一行**（skip / cancel）处置。
                _ => ResumeCause::UserDecision,
            },
            // 手动暂停的**默认出口**（续跑）。重跑那条出口按的是另一颗键，由端点显式写
            // `user_rerun`——同一原因两个去向，只有按键的那一方知道按的是哪一颗。
            PendingKind::UserPaused => ResumeCause::UserPaused,
        }
    }
}

/// 续接判定表（决策 205）：这个原因要不要把上一段对话带进下一轮。
///
/// **硬编码、改它要发版**（与 [`crate::config::SUPPORTED_ADAPTERS`] 同姿态，决策 103 的先例）：
/// 这张表是产品判断，不是配置项——「不展示在设置里、在代码中定义好」正是本决策的原话。
///
/// 这张表管 **resume / 续接边界**：分界原来是「模型的自动失败重试不给续接，
/// 人的介入才给」（决策 33 / 205 裁决②）；**决策 278 显式修订后半句**——`agent_retry_max`
/// 的自动重试如今也续接转录＋错误 turn，但那条路在编排侧（`model_invoke`）直接保留，
/// 不走本表。故 `validate_attempts` 的原地重试、未耗尽的超时仍不出现在这张表里
/// ——它们根本走不到 resume 边界（`clear_cursor_pending` 才是落点）。
///
/// **一个例外（票 04）**：`timeout_blank_restart` 由超时梯子的降级档**自动**置位
/// （`scheduler::handle_timeout` 经 `mark_cursor_continuation`——那条路与
/// `clear_cursor_pending` 是同一个列的第二把合法钥匙，见 `storage::cursors` 的文档），
/// 它不是「人按了键」。它进表是因为下游要拿这个原因判断**形态**（简报而非转录）——
/// 本表回答的是「这是不是一条续接边界」，答案仍是 true。
///
/// **穷尽 `match`**：新增一个原因时不写进这个 match 就编译不过。这比「兜底 false 然后忘掉」
/// 强——兜底仍保留（`Unknown` 那一档），但它只服务于「库里的历史值」，不服务于新代码。
pub fn resume_continues(cause: ResumeCause) -> bool {
    match cause {
        // ── true：人按了键之后，让模型带着上一段对话接着干 ──
        //
        // 信息不足被打回（补充输入后重入同一节点）、校验耗尽（格式不是 json）、
        // 代码有问题被打回（评审驳回 / 闸门 / test code_issue）、超时耗尽后人工「重试执行」、
        // 判分歧、脏工作区、重复风险、冲突等待**自动**放行、依赖失败**自动**恢复。
        //
        // `timeout_blank_restart`（票 04）：本档**也**算一次续接边界（下游要据此把简报
        // 段渲染进首条消息），只是 `take_continuation` 认出它之后**不给转录**——换一份
        // 简报。判定表回答的是「这是不是一个续接边界」，形态由 `ContinuationMode` 定。
        ResumeCause::InfoInsufficient
        | ResumeCause::RetryExhausted
        | ResumeCause::Timeout
        | ResumeCause::TimeoutBlankRestart
        | ResumeCause::ConflictWait
        | ResumeCause::DependencyFailed
        | ResumeCause::DuplicateRisk
        | ResumeCause::DevelopDesignInputInsufficient
        | ResumeCause::TestDesignInputInsufficient
        | ResumeCause::JudgeDisagreement
        | ResumeCause::Review
        | ResumeCause::TestCodeIssue
        | ResumeCause::GateRecheck
        | ResumeCause::DirtyWorktree
        | ResumeCause::MergeReturned
        | ResumeCause::UserPaused
        | ResumeCause::HumanReviewRejected => true,

        // ── false：去向是新的一段（或不是人按的键）──
        //
        // `merge_approved`：merge 的 phase A 是**提案**、phase B 是**执行**，
        //   提案那段对话不该续进执行（决策 205 点名）。
        // `human_review_approved`：去向 test.execute 是新节点，本来就没有自己的旧会话。
        // `context_overflow`：上下文溢出的人为处置之后，重开一段更干净
        //   （决策 205 未列 → 兜底 false；票 04 复用这一档）。
        // `dependency_cancelled`：依赖被取消，人按「忽略失败依赖继续」——那是换一条路走。
        // `user_decision`（通用那一行）：既非打回也非补充，没有可续的上下文。
        // `user_rerun`：**重跑 = 这一轮不算，重来**——带着上一轮的对话重来正是「重来」的
        //   反面（模型会接着自己刚写的那半句往下写）。故它落在 false：重开一段。
        ResumeCause::MergeApproved
        | ResumeCause::HumanReviewApproved
        | ResumeCause::ContextOverflow
        | ResumeCause::DependencyCancelled
        | ResumeCause::UserRerun
        | ResumeCause::UserDecision
        | ResumeCause::Unknown => false,
    }
}

/// pending 的结构化上下文。`kind` 是 `(type, context.kind)` 动作表的第二个 key（决策 130）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PendingContext {
    /// duplicate_risk | dirty_worktree | test_code_issue | judge_disagreement | ...
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// conflict_wait 专用：**全部**冲突任务 id（决策 102）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflict_task_ids: Vec<String>,
    /// 闸门失败详情，传给 test.execute 复检（决策 85）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate_failure_output: Option<String>,
    /// 原始诊断（主流程票 03）：provider 配置类失败时保留原始错误串。
    /// 与 `message` 分离——message 是中文可操作提示，raw 供排查，不拼进 message。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
    /// 其余自由字段，序列化时平铺进 context。
    #[serde(flatten, default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl PendingContext {
    pub fn with_kind(kind: &str) -> Self {
        PendingContext {
            kind: Some(kind.to_string()),
            ..Default::default()
        }
    }

    /// 带原始诊断（主流程票 03）：不设置 `kind`，避免影响 `allowed_actions` 的路由键。
    pub fn with_diagnostic(raw: impl Into<String>) -> Self {
        PendingContext {
            diagnostic: Some(raw.into()),
            ..Default::default()
        }
    }

    pub fn get_str_list(&self, key: &str) -> Vec<String> {
        match key {
            "conflict_task_ids" => self.conflict_task_ids.clone(),
            other => self
                .extra
                .get(other)
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

/// 游标 / 任务级 pending 原因（§4.2）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PendingReason {
    #[serde(rename = "type")]
    pub kind: PendingKind,
    pub stage: Stage,
    pub node: Node,
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggested_actions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<PendingContext>,
}

impl PendingReason {
    pub fn new(kind: PendingKind, stage: Stage, node: Node, message: impl Into<String>) -> Self {
        PendingReason {
            kind,
            stage,
            node,
            message: message.into(),
            suggested_actions: Vec::new(),
            context: None,
        }
    }

    pub fn with_context(mut self, ctx: PendingContext) -> Self {
        self.context = Some(ctx);
        self
    }

    /// `(type, context.kind)` —— allowed_actions 动作表的 key（决策 130）。
    pub fn action_key(&self) -> (PendingKind, Option<&str>) {
        (
            self.kind,
            self.context.as_ref().and_then(|c| c.kind.as_deref()),
        )
    }
}

// ─────────────────────────────── 阶段元数据（§4.2）───────────────────────────────

/// 新增公开符号（冲突检测第一层依据，决策 53 / 120）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NewSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub module_path: String,
    pub file_path: String,
}

/// 符号种类；后三者覆盖非 Rust 目标项目（决策 120）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Struct,
    Enum,
    Trait,
    Method,
    Const,
    Module,
    Class,
    Interface,
    Type,
}

/// 验收标准（决策 136）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AcceptanceCriterion {
    pub id: String,
    pub description: String,
}

/// 冲突警告（第一层路径/符号交集 + 第二层语义比对）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ConflictWarning {
    pub task_id: String,
    pub task_title: String,
    #[serde(default)]
    pub overlapping_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overlapping_symbols: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplicate_risk: Option<DuplicateRisk>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DuplicateRisk {
    Low,
    Medium,
    High,
}

/// 文件变更规格。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FileChangeSpec {
    pub path: String,
    pub action: FileAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileAction {
    Create,
    Modify,
    Delete,
}

impl FileAction {
    /// 中文动词。打回反馈有两处渲染（决策 133 的段 + 决策 387 的 turn），共用这一份。
    pub fn label(self) -> &'static str {
        match self {
            FileAction::Create => "新增",
            FileAction::Modify => "修改",
            FileAction::Delete => "删除",
        }
    }
}

/// 决策 387 裁决③：系统注入的 user turn 必须带的结构化前缀——两类 turn
/// （用户原话逐字 / 系统注入带前缀）在转录里可机器区分、模型可辨识。
pub const REVIEW_REWORK_TURN_PREFIX: &str = "【评审打回反馈·系统注入】";

/// 业务测试场景（test-design 产出，决策 136）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TestScenario {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub preconditions: Vec<String>,
    #[serde(default)]
    pub steps: Vec<String>,
    pub expected_result: String,
    pub priority: ScenarioPriority,
    /// 引用的验收标准 id（决策 136 引用完整性校验）。
    #[serde(default)]
    pub design_refs: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioPriority {
    High,
    Medium,
    Low,
}

/// 失败用例与根因分类（决策 62）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TestFailure {
    pub test_name: String,
    pub error_message: String,
    pub failure_cause: FailureCause,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FailureCause {
    /// 用例问题。
    TestIssue,
    /// 业务代码问题。
    CodeIssue,
}

// ── 各阶段 submit_metadata 结构体（决策 38：tool parameters 由此派生）──

/// architect-design.execute 的 submit_metadata。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ArchitectExecuteMetadata {
    pub readiness: bool,
    #[serde(default)]
    pub blockers: Vec<String>,
    #[serde(default)]
    pub affected_files: Vec<String>,
    #[serde(default)]
    pub new_symbols: Vec<NewSymbol>,
    #[serde(default)]
    pub conflict_warnings: Vec<ConflictWarning>,
    #[serde(default)]
    pub acceptance_criteria: Vec<AcceptanceCriterion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub design_doc_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplicate_risk: Option<DuplicateRisk>,
}

/// agent 型 validate_input 的 submit_metadata。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ValidateInputMetadata {
    pub readiness: bool,
    /// 决策 277②：字段描述进 submit_metadata 的 tool schema（与模板同源同步）；
    /// 类型保持 string[] 自由文本直通——下游 `as_str` 过滤会静默丢非字符串项，
    /// 改结构化对象等于把现有产出静默变哑。
    #[serde(default)]
    #[schemars(
        description = "不充分时列出要问用户的问题；每条 = 问题 + 推荐答案。能从仓库文档/代码确定默认值的约束不问（直接采用并在判定正文注明默认值）"
    )]
    pub blockers: Vec<String>,
}

/// agent 型 validate_output 的 submit_metadata。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ValidateOutputMetadata {
    pub passed: bool,
    #[serde(default)]
    pub blockers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<String>,
}

/// develop-design.execute 的 submit_metadata。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DevelopDesignMetadata {
    pub readiness: bool,
    #[serde(default)]
    pub blockers: Vec<String>,
    #[serde(default)]
    pub file_changes: Vec<FileChangeSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dev_doc_path: Option<String>,
}

/// test-design.execute 的 submit_metadata。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TestDesignMetadata {
    pub readiness: bool,
    #[serde(default)]
    pub blockers: Vec<String>,
    #[serde(default)]
    pub test_scenarios: Vec<TestScenario>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_scenarios_path: Option<String>,
}

/// sync-check.execute 的产出（纯代码，无 agent 调用）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SyncDecision {
    pub decision: SyncDecisionKind,
    pub dev_readiness: bool,
    pub test_readiness: bool,
    #[serde(default)]
    pub dev_blockers: Vec<String>,
    #[serde(default)]
    pub test_blockers: Vec<String>,
    /// design_refs 完整性校验的 warning（决策 136：仅 warning 不进 backtrack）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// 上游元数据的**缺项清单**（票 04，fail-closed）：同步闸门依赖的字段一旦缺失，
    /// 下游校验会静默跳过——2026-10-01 事故里三行元数据都只剩 `{"readiness":true}`，
    /// 闸门因此真空放行。任一缺项都让本闸门判 Backtrack。
    ///
    /// 单列一条通道而不是塞进 `dev_blockers` / `test_blockers`：缺项是**某一行产出**的
    /// 残缺，不是某一支的 blocker，混进 blockers 会让回溯的读号者去找错分支。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub metadata_gaps: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SyncDecisionKind {
    Proceed,
    Backtrack,
}

/// develop.execute 的 submit_metadata。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CodeChanges {
    pub branch_name: String,
    #[serde(default)]
    pub changed_files: Vec<FileChangeSpec>,
    #[serde(default)]
    pub unit_test_files: Vec<FileChangeSpec>,
}

/// review.execute 的 required_changes 单项（决策 387）。
///
/// 不复用 [`FileChangeSpec`]：那是 develop 侧产出契约，塞进 `finding` 等于把评审字段
/// 派生进开发提交的 schema。旧格式输出（无 finding）向后兼容——`serde(default)` 兜底，
/// 打回反馈降级为「只列路径 + 报告绝对路径」。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewRequiredChange {
    pub path: String,
    pub action: FileAction,
    /// 发现摘要：错在哪、该改成什么。评审 execute 产出时逐项填写（决策 387——
    /// 数据流最短路：发现本就在评审 agent 手里，不解析报告 markdown）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "发现摘要：错在哪、该改成什么（一两句话，内联进打回反馈）")]
    pub finding: Option<String>,
}

/// review.execute 的 submit_metadata。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewResult {
    pub approved: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_report_path: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_changes: Vec<ReviewRequiredChange>,
}

/// test.execute 的 submit_metadata。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TestResult {
    pub passed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_report_path: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<TestFailure>,
    /// 被 merge 测试闸门打回后的复检（决策 85 / 109）。
    #[serde(default)]
    pub gate_recheck: bool,
}

/// diff 统计。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DiffStats {
    pub files_changed: u64,
    pub insertions: u64,
    pub deletions: u64,
    #[serde(default)]
    pub file_details: Vec<FileDiffDetail>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FileDiffDetail {
    pub path: String,
    pub additions: u64,
    pub deletions: u64,
    pub status: FileDiffStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileDiffStatus {
    Added,
    Modified,
    Deleted,
}

/// 审批状态（决策 72）。与闸门结果**正交**（决策 95）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Approval {
    #[default]
    None,
    Pending,
    Approved,
    Returned,
}

/// 合入前闸门结果（决策 95）。独立于 approval。
///
/// **没有 `Default`**：闸门未跑 ≠ 闸门通过。承载它的字段是 `Option<Gate>`，
/// `None` 表示阶段 A 尚未跑到闸门，路由层据此落 `NoOp`，绝不推进。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Gate {
    Pass,
    Fail,
}

/// 闸门失败类型（决策 139）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GateFailureKind {
    Lint,
    Test,
}

/// merge 阶段在 `kanban_stage_outputs` 里的 `output_type` 定名（§4.2）。
/// diff 文件本身仍是任务目录的 `merge-proposal.diff`，这个字符串只是产出行的类型键。
pub const MERGE_OUTPUT_TYPE: &str = "merge_result";

/// merge.execute 的产出（§4.2）。
///
/// `diff_path` / `diff_stats` / `base_commit` 按文档**必填**：该行只在闸门跑完、
/// diff 生成后才 upsert，不存在"半行 merge_result"。反序列化缺字段即报错，
/// 不做静默兜底（缺行由调用方按"闸门未跑"处理，绝不当作通过）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MergeResult {
    pub diff_path: String,
    pub diff_stats: DiffStats,
    /// 生成 proposal 时的 `{base_ref}` SHA（决策 96）。
    pub base_commit: String,
    /// 闸门结果：`None` 表示闸门尚未执行（阶段 A 中途），**不可当作通过**。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<Gate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_failure_kind: Option<GateFailureKind>,
    /// 闸门失败累计次数（决策 108：lint 与测试统一累加，跨阶段跳转不重置）。
    #[serde(default)]
    pub gate_failures: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_failure_output: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflict_files: Vec<String>,
    #[serde(default)]
    pub approval: Approval,
    #[serde(default)]
    pub status: MergeStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MergeStatus {
    #[default]
    PendingApproval,
    Merged,
}

/// done.execute 的产出。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DoneResult {
    pub success: bool,
    pub summary: String,
}

// ─────────────────────────────── 持久化实体 ───────────────────────────────

/// 任务（§4.1）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub description: String,
    pub status: TaskStatus,
    /// 焦点游标投影（决策 80/92），只供看板展示与筛选。
    pub current_stage: Stage,
    pub current_node: Node,
    pub validate_attempts: u32,
    pub pending_reason: Option<PendingReason>,
    pub worktree_path: Option<String>,
    pub branch_name: Option<String>,
    pub total_tokens: u64,
    pub total_calls: u64,
    pub review_mode: ReviewMode,
    /// 任务级 provider 覆盖（决策 105），只影响本任务后续节点。
    pub model_override: Option<String>,
    pub archived_at: Option<DateTime<Utc>>,
    pub stalled: bool,
    pub executor_owner: Option<String>,
    /// 任务级托管（决策 210① / 票 08）。`None` = 从来没开过。
    pub stewardship: Option<Stewardship>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewMode {
    #[default]
    Agent,
    Human,
}

/// 活跃游标（决策 80）：执行状态的唯一事实来源。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeCursor {
    pub cursor_id: String,
    pub task_id: String,
    pub branch: String,
    pub stage: Stage,
    pub node: Node,
    pub status: CursorStatus,
    pub validate_attempts: u32,
    /// 用户对本分支 skip 后置位（决策 93），sync-check 视其 readiness=true。
    pub skipped_to_join: bool,
    pub pending_reason: Option<PendingReason>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl NodeCursor {
    /// 分支常量。
    pub const BRANCH_MAIN: &'static str = "main";
    pub const BRANCH_DEVELOP_DESIGN: &'static str = "develop-design";
    pub const BRANCH_TEST_DESIGN: &'static str = "test-design";

    /// 是否可继续执行。
    pub fn is_runnable(&self) -> bool {
        self.status == CursorStatus::Active
    }

    pub fn is_pending(&self) -> bool {
        self.status == CursorStatus::Pending
    }
}

/// 任务级托管（决策 210① / 票 08）。
///
/// **默认关**。打开之后，值班长可以在**这一个任务**上免按键 `resume(continue)`——
/// 恰好一个动作，别的（`retry` / `merge` / `review` / `cancel` / `create`）永远仍要人按。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Stewardship {
    /// 托管开着没有。
    pub enabled: bool,
    /// 已被值班长**自动** resume 过的次数（决策 210⑨ 的止损：满 [`STEWARDSHIP_MAX_AUTO_RESUMES`] 次）。
    ///
    /// 落库而不是放内存：票 05 已经吃过「内存状态重启即失」的亏（`TickReport.reminded`），
    /// 而这里失掉的是**止损线**——重启后重新数一遍等于上限不存在。
    pub auto_resumes: u32,
    /// 上一次自动动手时的态势指纹（决策 210⑨：同一指纹不重复动手）。
    ///
    /// 与次数是**两条独立的闸**：单靠次数挡不住「同一件事被反复触发」，单靠指纹挡不住
    /// 「每次指纹都不同但都没用」。两个一起才封住。
    pub last_fingerprint: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
}

/// 同一个任务的自动 `resume` 次数上限（决策 210⑨：N = 2）。
pub const STEWARDSHIP_MAX_AUTO_RESUMES: u32 = 2;

impl Stewardship {
    /// 打开托管。**计数与指纹一起清零**：新开的一次托管是新的授权，不是上一次的续期。
    pub fn enabled_now(now: DateTime<Utc>) -> Self {
        Stewardship {
            enabled: true,
            auto_resumes: 0,
            last_fingerprint: None,
            updated_at: Some(now),
        }
    }

    /// 这一次动手许可吗（决策 210⑨ 的两条闸）。
    pub fn permits(&self, fingerprint: &str) -> bool {
        self.enabled
            && self.auto_resumes < STEWARDSHIP_MAX_AUTO_RESUMES
            && self.last_fingerprint.as_deref() != Some(fingerprint)
    }

    /// 记一次自动动手（`resume(continue)` 与 `unstick` **共用**这一条止损线，决策 210⑨）。
    ///
    /// 名字沿用列名（`auto_resumes` 就是落库的那一列），语义是「自动动作动过几次」——
    /// 两种动作各记一条线，只会让「它今晚自己动了几次手」这个数变成两个数。
    pub fn note_auto_resume(&mut self, fingerprint: &str, now: DateTime<Utc>) {
        self.auto_resumes += 1;
        self.last_fingerprint = Some(fingerprint.to_string());
        self.updated_at = Some(now);
    }
}

/// 节点执行记录（决策 63 / 99 / 114）。
///
/// 归属二选一（票 10）：任务级 run 有 `task_id` + `cursor_id`，项目级伪阶段
/// （`project_analysis`）无任务无游标，改以 `project_id` 归属——两者恰有其一非空。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeRun {
    pub id: i64,
    /// 任务级 run 的所属任务；项目级伪阶段为 `None`。
    pub task_id: Option<String>,
    /// 任务级 run 的继承游标（决策 113）；项目级伪阶段为 `None`。
    pub cursor_id: Option<String>,
    /// 项目级 run 的所属项目（`project_analysis`）；任务级为 `None`。
    pub project_id: Option<String>,
    pub stage: Stage,
    pub node: Node,
    pub attempt: u32,
    pub agent_type: String,
    pub parent_run_id: Option<i64>,
    pub status: NodeStatus,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub cache_read_tokens: u32,
    pub cache_write_tokens: u32,
    pub duration_ms: u64,
    pub error: Option<String>,
    /// 系统节点「进行到哪一步」（决策 211④ / 票 04）。
    ///
    /// 只由纯代码节点在**步边界**写（`init.execute` 的「检查项目工作区是否脏」这类），
    /// 于是卡住时台账里不再只有一句「超时」——那一句正是 2026-09-17 那次四小时挂死的
    /// 全部信息量。LLM 节点的现场在会话行里，这里恒为 `None`。
    pub step: Option<String>,
    pub process_group_id: Option<i32>,
    pub last_activity_at: Option<DateTime<Utc>>,
    pub prompt_template_hash: Option<String>,
    /// 本 run 续接了哪一条历史 run（决策 180，票 13）。`None` = 干净起跑。
    ///
    /// 只做续接谱系（排障爬链用）；指标汇总不再据它排除——排除规则随决策 375
    /// 删除（真实账语义：重喂的输入是真实成本）。
    pub continued_from_run_id: Option<i64>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    Running,
    Success,
    Failed,
    Timeout,
    /// **人在这一轮跑完之前把它按停了**（手动暂停 / 重跑本阶段）。
    ///
    /// 与 `Timeout` 分开记：一句「超时」会让人去查超时配置，而这一轮根本没有超时——
    /// 是人按了暂停。两个来路的区别在复盘时是承重的（决策 276）。
    Cancelled,
}

impl NodeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            NodeStatus::Running => "running",
            NodeStatus::Success => "success",
            NodeStatus::Failed => "failed",
            NodeStatus::Timeout => "timeout",
            NodeStatus::Cancelled => "cancelled",
        }
    }
}

impl FromStr for NodeStatus {
    type Err = crate::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "running" => NodeStatus::Running,
            "success" => NodeStatus::Success,
            "failed" => NodeStatus::Failed,
            "timeout" => NodeStatus::Timeout,
            "cancelled" => NodeStatus::Cancelled,
            other => return Err(crate::Error::Validation(format!("未知节点状态：{other}"))),
        })
    }
}

/// 阶段产出（决策 30 的 `kanban_stage_outputs` 行）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StageOutput {
    pub id: i64,
    pub task_id: String,
    pub stage: Stage,
    pub output_type: String,
    pub file_path: String,
    pub metadata_json: Option<serde_json::Value>,
    /// 决策 83：backtrack 把设计文档标记为过期（文件保留供回溯，覆盖写入时清除）。
    pub stale: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 流转记录（§12.4.2）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transition {
    pub id: i64,
    pub task_id: String,
    pub branch: String,
    pub from_stage: Option<Stage>,
    pub from_node: Option<Node>,
    pub to_stage: Stage,
    pub to_node: Node,
    pub trigger: TransitionTrigger,
    pub reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionTrigger {
    Normal,
    Retry,
    NodeRetry,
    Kickback,
    UserResume,
    AutoResume,
    Timeout,
    Start,
}

impl TransitionTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            TransitionTrigger::Normal => "normal",
            TransitionTrigger::Retry => "retry",
            TransitionTrigger::NodeRetry => "node_retry",
            TransitionTrigger::Kickback => "kickback",
            TransitionTrigger::UserResume => "user_resume",
            TransitionTrigger::AutoResume => "auto_resume",
            TransitionTrigger::Timeout => "timeout",
            TransitionTrigger::Start => "start",
        }
    }
}

impl FromStr for TransitionTrigger {
    type Err = crate::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "normal" => TransitionTrigger::Normal,
            "retry" => TransitionTrigger::Retry,
            "node_retry" => TransitionTrigger::NodeRetry,
            "kickback" => TransitionTrigger::Kickback,
            "user_resume" => TransitionTrigger::UserResume,
            "auto_resume" => TransitionTrigger::AutoResume,
            "timeout" => TransitionTrigger::Timeout,
            "start" => TransitionTrigger::Start,
            other => return Err(crate::Error::Validation(format!("未知流转触发：{other}"))),
        })
    }
}

/// 节点会话（§12.4.3）。1:1 只对调 LLM 的 run 成立（决策 99）。
///
/// 归属二选一（票 10）：任务级会话有 `task_id`，项目级伪阶段（`project_analysis`）
/// 改以 `project_id` 归属（决策 100：伪阶段独立观测）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeConversation {
    pub id: i64,
    /// 任务级会话的所属任务；项目级伪阶段为 `None`。
    pub task_id: Option<String>,
    /// 项目级会话的所属项目；任务级为 `None`。
    pub project_id: Option<String>,
    pub run_id: i64,
    pub stage: Stage,
    pub node: Node,
    pub attempt: u32,
    pub agent_type: String,
    pub parent_run_id: Option<i64>,
    pub messages_json: serde_json::Value,
    /// 组装后**系统段**的原文（决策 211② / 票 02）。原文是权威，
    /// `kanban_node_runs.prompt_template_hash` 降级为「两次跑的是不是同一份」的快速索引。
    /// 空 = 这一列落地之前落的历史行，或本就不调 LLM 的 run。
    pub system_prompt: Option<String>,
    /// 组装后**用户段**的原文（同上）。落库的 `messages` 里没有这两段——它们只在适配器
    /// 组装 HTTP body 时才被前置，从不回写，所以「这是 prompt 问题」此前无从核对。
    pub user_prompt: Option<String>,
    pub metadata_json: Option<serde_json::Value>,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    /// 这次 run 全部调用的**推理 / 思考**留痕（决策 360，迁移 0038）。工具环多轮调用
    /// 的思考按到达序以空行相连。它与 `messages_json` 最要紧的区别同决策 244：**绝不
    /// 回灌**——只被读出来展示（现场时间线默认收起的思考步），不进模型上下文。
    /// `None` = 模型不产推理，或这一行落在该列之前。
    pub reasoning: Option<String>,
    pub created_at: DateTime<Utc>,
    /// 重试时旧会话被标记的时间（§12.2 / 决策 113 的同构：归档不物理删除）。
    /// `None` = 未归档，参与新执行；`Some` = 历史 attempt，仍可查。
    pub archived_at: Option<DateTime<Utc>>,
}

/// 命令日志（§12.4.4）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeCommand {
    pub id: i64,
    /// 归属任务。**可空**（迁移 0012 / 决策 204④）：值班长的命令没有任务，它挂会话。
    /// 与 `session_id` 恰好一个非空（迁移里的 CHECK），空串不是合法归属。
    pub task_id: Option<String>,
    /// 归属会话（值班长的命令挂它）。流水线命令为 `None`。
    pub session_id: Option<String>,
    pub run_id: Option<i64>,
    pub stage: Stage,
    pub node: Node,
    pub source: CommandSource,
    /// **实际执行的**命令串（脱敏后，§12.4.4）。
    pub command: String,
    /// 改写之前模型（或项目配置）原本写的那一条（决策 297）。
    ///
    /// `None` = 按原样跑——「没启用 / 这一次调用点不改写 / rtk 不在场」这三件事对台账是
    /// 同一件事。台账的折叠行显示**原串**（那才是模型想要的东西），改写过的行带一枚小标，
    /// 展开时两条都摆出来（`components/task/CommandLog.svelte`）。
    pub original_command: Option<String>,
    pub cwd: String,
    pub exit_code: Option<i32>,
    pub stdout_path: Option<String>,
    pub stdout_preview: Option<String>,
    pub stderr_preview: Option<String>,
    pub duration_ms: Option<u64>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandSource {
    /// run_command 工具。
    Agent,
    /// 框架执行。
    System,
}

impl CommandSource {
    pub fn as_str(self) -> &'static str {
        match self {
            CommandSource::Agent => "agent",
            CommandSource::System => "system",
        }
    }
}

impl FromStr for CommandSource {
    type Err = crate::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "agent" => CommandSource::Agent,
            "system" => CommandSource::System,
            other => return Err(crate::Error::Validation(format!("未知命令来源：{other}"))),
        })
    }
}

/// 项目（§11.5 kanban_projects）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub local_path: String,
    pub default_branch: String,
    pub language: Option<String>,
    pub test_framework: Option<String>,
    /// 可选静态检查命令（决策 139）。未配置则闸门跳过 lint 环节。
    pub lint_command: Option<String>,
    pub agents_md_path: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// provider（决策 111 / 112）：一行 = 一个 (vendor, model, context_window)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub vendor: String,
    pub model: String,
    pub context_window: u32,
    pub base_url: Option<String>,
    /// 明文密钥（决策 112）。读接口只回显 `***`。
    pub api_key: Option<String>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Provider {
    /// 读接口回显用（决策 112）：不返回原值。
    pub fn masked_api_key(&self) -> Option<String> {
        self.api_key.as_ref().map(|_| "***".to_string())
    }
}

/// 阶段级 agent 配置（决策 22 / 46 / 66 / 111）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct StageConfig {
    pub stage: String,
    pub provider_id: Option<String>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub persona_path: Option<String>,
    pub persona_append: Option<String>,
    pub tools_json: Option<serde_json::Value>,
    pub skills_json: Option<serde_json::Value>,
    pub idle_timeout_sec: Option<u64>,
    pub max_duration_sec: Option<u64>,
    /// 节点级覆盖（决策 66）：`{"execute": {"idle_timeout_sec": 600}}`。
    pub node_overrides_json: Option<serde_json::Value>,
    /// 环境层权限档位（决策 206）。`None` = 没配过 → 用全局默认 / 该阶段的缺省。
    ///
    /// **不做节点级覆盖**（决策 206）：它是「这台机器上的环境层放手到什么程度」，
    /// 不是「这个节点放手到什么程度」——后者会让同一条命令在不同节点上有不同的自由，
    /// 而人看不出为什么。
    pub env_mode: Option<EnvMode>,
    /// 值班长一轮的**轮数上限**（决策 233① / 239）：`None` = 没配过 → 用缺省
    /// [`crate::pipeline::foreman::FOREMAN_MAX_ROUNDS`]。
    ///
    /// 只对 `foreman` 那一行有意义；写入路径只收**正整数**（`0` / 空都不许，也不提供无上限）。
    /// 它管的是「一轮里能跑几次模型调用」。整轮墙钟已撤（决策 288），token 预算接管了
    /// 「能烧多少」（决策 292）——它退为**模型行为失控时的兜底**。
    pub max_rounds: Option<u32>,
    /// 值班长一轮的**生成 token 预算**（决策 292 / 票 07）：`None` = 没配过 → 用缺省
    /// [`crate::pipeline::foreman::FOREMAN_WATCH_TOKEN_BUDGET`]。
    ///
    /// **只对值守轮是硬界**（触顶 → 部分结论落库 + 【未收口】）；人的那一轮无硬界——终点由人
    /// 决定，同一条线在那里只落一条软告警（只落账不拦）。只对 `foreman` 那一行有意义；
    /// 写入路径只收正整数（`0` / 负数都不许，也不提供「无预算」这一档）。
    pub watch_token_budget: Option<u32>,
    pub updated_at: DateTime<Utc>,
}

/// 这三个取值的人话说法（配置写入路径的报错共用一份）。
///
/// 与 [`EnvMode::parse_or_message`] 同源：两处各写一份字符串的后果是同一件事报出两种说法，
/// 而使用者据此判断「我填错了什么」。
pub const ENV_MODE_EXPECTED: &str = "环境层档位只能是 auto / ask / deny";

/// 环境层权限档位（决策 206）。
///
/// **只管环境层**（文件 / 命令 / 技能拉取 / 子代理）：`auto` 直接执行、`ask` 转成提议
/// 等值班经理按键、`deny` 连广告都不给。**本服务写接口（D 层）不读它**——那半边恒为
/// 提议 + 确认钮，因为它的错误会改变流水线的事实（决策 206 的判据）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvMode {
    /// 直接执行，与档位出现之前逐字相同（**缺省值**）。
    Auto,
    /// 不执行，生成一条提议（决策 188 / 207 的表与确认钮）。
    Ask,
    /// 拒绝执行，且**不广告**（连工具定义都不给）。
    Deny,
}

impl EnvMode {
    pub fn as_str(self) -> &'static str {
        match self {
            EnvMode::Auto => "auto",
            EnvMode::Ask => "ask",
            EnvMode::Deny => "deny",
        }
    }

    /// 认得出就给出，认不出给 `None`。
    ///
    /// **不 panic、也不兜底成 `auto`**：调用方必须自己决定「认不出的值该怎么办」——
    /// 读配置时那是「配置写错了」该 fail fast，读库时那是「这一列被人手工改坏了」
    /// 该退回缺省。两处的正确答案不同，故这里不替它们选。
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "auto" => Some(EnvMode::Auto),
            "ask" => Some(EnvMode::Ask),
            "deny" => Some(EnvMode::Deny),
            _ => None,
        }
    }

    /// 认得出就给出，认不出给一句人话（配置写入路径共用，两处各写一份必然漂移）。
    pub fn parse_or_message(raw: &str) -> std::result::Result<Self, String> {
        EnvMode::parse(raw).ok_or_else(|| format!("{ENV_MODE_EXPECTED}，实际：{raw:?}"))
    }

    /// 某个阶段的**缺省**档位（决策 206：真实阶段 `auto`、值班长 `ask`）。
    ///
    /// 真实阶段的缺省是全局默认（[`crate::config::Settings::env_mode`]）而不是一个写死的
    /// 常量：那正是 config.toml 那一层的意义。值班长的缺省是全局默认之外的**另一条**，
    /// 因为它的输入与流水线节点不是一类东西（人可以随便打的任意文本）。
    pub fn default_for(stage: &str) -> Option<Self> {
        if stage == crate::pipeline::foreman::FOREMAN_STAGE_KEY {
            Some(EnvMode::Ask)
        } else {
            None
        }
    }
}

/// 某阶段此刻生效的环境层档位（决策 206 的两层解析，唯一实现）。
///
/// 顺序：阶段配置行 → 该阶段的缺省（值班长 `ask`；其余没有）→ 全局默认（`config.toml`
/// 的 `[pipeline] env_mode`，缺省 `auto`）。
///
/// **收在一处**：广告集（`model_request::tool_defs` / `ForemanRunner::tool_defs`）与执行点
/// （`ToolExecutor::execute` 的第三道闸）都调它，两处各判一次必然漂移——而漂移的后果是
/// 「模型看得见一个调用就被拒的工具」或反过来（一个能调但没人告诉它的工具）。
pub fn effective_env_mode(
    global_default: EnvMode,
    stage: &str,
    stage_config: Option<&StageConfig>,
) -> EnvMode {
    stage_config
        .and_then(|c| c.env_mode)
        .or_else(|| EnvMode::default_for(stage))
        .unwrap_or(global_default)
}

/// `ask` 档只留给值班长（`run-command-permissions` 规格 §4：流水线只有 `auto` / `deny` 两档）。
/// 写入路径（`config.toml` 的全局默认与 `PUT /stage-configs`）用它把关。
///
/// 理由不是洁癖：流水线节点**无人值守**，而 `ask` 的载体是「等人按键」——节点没有提议通道，
/// 配成 `ask` 的结果是**静默收掉这个阶段全部的环境写动作**（一条 develop 会卡在「写不了文件」
/// 上，而配置看上去只是一行 `ask`）。要收紧就明说 `deny`：那时行为与意图一致（拒绝，且连工具
/// 都不给）。
pub fn stage_may_use_ask(stage: &str) -> bool {
    stage == crate::pipeline::foreman::FOREMAN_STAGE_KEY
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 从 `schema_for!` 取一个枚举的**变体表**——宏路径，不经过任何手写清单。
    ///
    /// 这条是本模块两张共享表测试的共同底层：手写数组（`ALL_STAGES` 那种）自己也会漏一个
    /// 变体，故「判据必须遍历枚举」在这里落成「让宏去遍历枚举定义」。
    ///
    /// `RootSchema` 先经 `serde_json::to_value` 归一：`schema_for!` 给的是
    /// `schemars::schema::RootSchema` 而不是 `Value`（`client.rs::tool_defs` 同一做法）。
    ///
    /// 两种输出形状都要收：无文档注释的枚举给 `enum`；带文档注释的变体在 schemars 0.8 里走
    /// `oneOf`（每个分支一个 `enum`，`description` 在旁边）。一个分支可以带**多个**值——
    /// 相邻变体都没有文档注释时 schemars 会把它们并进一个 `enum`（`TaskStatus` 的
    /// `done` / `failed` / `cancelled` 正是这样并成一组的），故要展开整个数组。
    fn variants_of(name: &str, schema: &schemars::schema::RootSchema) -> Vec<String> {
        let schema = serde_json::to_value(schema).expect("schema 可序列化");
        let mut out: Vec<String> = Vec::new();
        if let Some(list) = schema.get("enum").and_then(|e| e.as_array()) {
            out.extend(list.iter().map(|v| {
                v.as_str()
                    .unwrap_or_else(|| panic!("{name} 的 enum 里有非字符串：{v}"))
                    .to_string()
            }));
        }
        if let Some(branches) = schema.get("oneOf").and_then(|v| v.as_array()) {
            for branch in branches {
                let list = branch["enum"]
                    .as_array()
                    .unwrap_or_else(|| panic!("{name} 的 oneOf 分支没有 enum：{branch}"));
                for v in list {
                    out.push(
                        v.as_str()
                            .unwrap_or_else(|| panic!("{name} 的 oneOf 里有非字符串：{v}"))
                            .to_string(),
                    );
                }
            }
        }
        assert!(
            !out.is_empty(),
            "{name} 的 schema 里一个变体都没取到：{schema}"
        );
        out
    }

    #[test]
    fn stage_string_roundtrip() {
        for st in ALL_STAGES {
            assert_eq!(Stage::from_str(st.as_str()).unwrap(), st);
        }
        assert_eq!(Stage::ArchitectDesign.as_str(), "architect-design");
        assert_eq!(Stage::SyncCheck.as_str(), "sync-check");
    }

    #[test]
    fn node_string_roundtrip() {
        for n in ALL_NODES {
            assert_eq!(Node::from_str(n.as_str()).unwrap(), n);
        }
    }

    #[test]
    fn stage_order_matches_flowchart() {
        let order: Vec<&str> = ALL_STAGES.iter().map(|s| s.as_str()).collect();
        assert_eq!(
            order,
            vec![
                "init",
                "architect-design",
                "develop-design",
                "test-design",
                "sync-check",
                "develop",
                "review",
                "test",
                "merge",
                "done"
            ]
        );
    }

    #[test]
    fn task_status_predicates() {
        assert!(TaskStatus::Done.is_terminal());
        assert!(!TaskStatus::Pending.is_terminal());
        assert!(TaskStatus::Pending.occupies_slot());
        assert!(TaskStatus::Running.occupies_slot());
        assert!(!TaskStatus::Queued.occupies_slot());
        assert!(!TaskStatus::Waiting.occupies_slot());
        assert!(TaskStatus::Queued.is_active());
    }

    /// 判定表逐行（决策 205）。表就是规格，故这里逐条抄一遍**期望值**：
    /// 改动表却忘了改测试，或者改了测试没改表，两种都会在这里现形。
    #[test]
    fn resume_cause_table_is_the_spec() {
        use ResumeCause::*;
        let cases: [(ResumeCause, bool); 24] = [
            // ── true ──
            (InfoInsufficient, true),
            (RetryExhausted, true),
            (Timeout, true),
            // 空白重跑是一条续接边界（下游据它渲染简报段），只是不带转录（票 04）
            (TimeoutBlankRestart, true),
            (ConflictWait, true),
            (DependencyFailed, true),
            (DuplicateRisk, true),
            (DevelopDesignInputInsufficient, true),
            (TestDesignInputInsufficient, true),
            (JudgeDisagreement, true),
            (Review, true),
            (TestCodeIssue, true),
            (GateRecheck, true),
            (DirtyWorktree, true),
            (MergeReturned, true),
            (HumanReviewRejected, true),
            // 人松开自己按下的暂停：接着上一段干（决策 276）
            (UserPaused, true),
            // ── false ──
            (MergeApproved, false),
            (HumanReviewApproved, false),
            (ContextOverflow, false),
            (DependencyCancelled, false),
            (UserDecision, false),
            // 重跑 = 这一轮不算：重开一段（决策 276）
            (UserRerun, false),
            (Unknown, false),
        ];
        for (cause, expected) in cases {
            assert_eq!(
                resume_continues(cause),
                expected,
                "判定表里 {} 的值与决策 205 / 276 不一致",
                cause.as_str()
            );
        }
    }

    /// 原因串与 `PendingKind` + `context.kind` 的组合互相对得上（分类表逐行）。
    #[test]
    fn resume_cause_classification_covers_every_pending_kind() {
        use ResumeCause::*;
        // 每一个 PendingKind 都要有归宿——新增变体时 `classify` 的穷尽 match 会先报错，
        // 这条测试钉的是「归宿不是一个意外的地方」。
        assert_eq!(
            ResumeCause::classify(PendingKind::InfoInsufficient, None),
            InfoInsufficient
        );
        assert_eq!(
            ResumeCause::classify(PendingKind::ConflictWait, None),
            ConflictWait
        );
        assert_eq!(
            ResumeCause::classify(PendingKind::RetryExhausted, None),
            RetryExhausted
        );
        assert_eq!(
            ResumeCause::classify(PendingKind::ContextOverflow, None),
            ContextOverflow
        );
        assert_eq!(ResumeCause::classify(PendingKind::Timeout, None), Timeout);
        // 手动暂停的默认出口是「续跑」（重跑那条出口由端点显式写 user_rerun，决策 276）
        assert_eq!(
            ResumeCause::classify(PendingKind::UserPaused, None),
            UserPaused
        );
        assert_eq!(
            ResumeCause::classify(PendingKind::DependencyFailed, None),
            DependencyFailed
        );
        assert_eq!(
            ResumeCause::classify(PendingKind::DependencyFailed, Some("dependency_cancelled")),
            DependencyCancelled
        );
        assert_eq!(
            ResumeCause::classify(PendingKind::UserDecision, Some("review")),
            Review
        );
        assert_eq!(
            ResumeCause::classify(PendingKind::UserDecision, Some("test_code_issue")),
            TestCodeIssue
        );
        assert_eq!(
            ResumeCause::classify(PendingKind::UserDecision, Some("gate_recheck")),
            GateRecheck
        );
        assert_eq!(
            ResumeCause::classify(PendingKind::UserDecision, Some("dirty_worktree")),
            DirtyWorktree
        );
        assert_eq!(
            ResumeCause::classify(PendingKind::UserDecision, Some("judge_disagreement")),
            JudgeDisagreement
        );
        assert_eq!(
            ResumeCause::classify(PendingKind::UserDecision, Some("duplicate_risk")),
            DuplicateRisk
        );
        assert_eq!(
            ResumeCause::classify(
                PendingKind::UserDecision,
                Some("develop_design_input_insufficient")
            ),
            DevelopDesignInputInsufficient
        );
        assert_eq!(
            ResumeCause::classify(
                PendingKind::UserDecision,
                Some("test_design_input_insufficient")
            ),
            TestDesignInputInsufficient
        );
        // 没有 context.kind、以及认不出的 context.kind 都落到通用那一行
        assert_eq!(
            ResumeCause::classify(PendingKind::UserDecision, None),
            UserDecision
        );
        assert_eq!(
            ResumeCause::classify(PendingKind::UserDecision, Some("未来才有的类别")),
            UserDecision
        );
    }

    /// DB 列的取值往返：每条都写得进去、读得回来。
    #[test]
    fn resume_cause_strings_round_trip() {
        for cause in ALL_RESUME_CAUSES {
            assert_eq!(
                ResumeCause::parse(cause.as_str()),
                Some(cause),
                "{} 往返失败",
                cause.as_str()
            );
        }
        // 认不出的取值 → None（调用方按 Unknown 兜底，不 panic）
        assert_eq!(ResumeCause::parse("未来才有的原因"), None);
        assert_eq!(ResumeCause::parse(""), None);
    }

    #[test]
    fn pending_action_key_includes_context_kind() {
        let reason = PendingReason::new(
            PendingKind::UserDecision,
            Stage::Review,
            Node::ValidateOutput,
            "评审不通过",
        )
        .with_context(PendingContext::with_kind("review"));
        assert_eq!(
            reason.action_key(),
            (PendingKind::UserDecision, Some("review"))
        );
    }

    #[test]
    fn provider_read_api_masks_api_key() {
        let p = Provider {
            id: "p1".into(),
            vendor: "deepseek".into(),
            model: "deepseek-chat".into(),
            context_window: 64_000,
            base_url: None,
            api_key: Some("sk-secret".into()),
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        assert_eq!(p.masked_api_key().as_deref(), Some("***"));
        assert_ne!(p.masked_api_key(), p.api_key);
    }

    /// 枚举成员表：`Stage` / `PendingKind` 的成员导给前端（票 mirror-contract/02，决策 253②）。
    ///
    /// 前端 `api/types.ts` 的两个联合是**手抄的副本**，而它们抄的正是这两个枚举本身
    /// ——抄第二遍的东西自己还会漂。这条测试把它变成会红的机器检查。
    ///
    /// **判据遍历枚举、不是手写数组**（票面要求）：`schema_for!` 由宏从枚举定义取变体表，
    /// 故「导出漏了一个变体」在这条路上不可能发生。这里只需核对**文件里那份**与枚举一致。
    ///
    /// 失败报文带可直接贴回的 JSON：表是**导出物**，改了枚举就该重新导出——
    /// 不手改生成物，重跑生成（`scripts/make-icon.mjs --check` 同一姿态）。
    #[test]
    fn shared_enum_members_fixture_matches_the_enums() {
        #[derive(serde::Deserialize)]
        struct Fixture {
            stage: Vec<String>,
            pending_kind: Vec<String>,
        }

        let raw = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/enum_members.json"
        ));
        let fixture: Fixture = serde_json::from_str(raw).expect("fixture 必须是合法 JSON");

        let stage = variants_of("Stage", &schemars::schema_for!(Stage));
        let pending = variants_of("PendingKind", &schemars::schema_for!(PendingKind));

        // 顺序也 pin：两个枚举都是 `rename_all` 的纯变体表，声明序即展示序
        // （`ALL_STAGES` 的注释写着「阶段全序（流程图顺序）」）。
        assert_eq!(
            fixture.stage, stage,
            "Stage 的成员表与 fixture 不一致——改了枚举就重新导出 tests/fixtures/enum_members.json：\n{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "stage": stage,
                "pending_kind": pending,
            }))
            .unwrap()
        );
        assert_eq!(
            fixture.pending_kind, pending,
            "PendingKind 的成员表与 fixture 不一致——改了枚举就重新导出 tests/fixtures/enum_members.json：\n{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "stage": stage,
                "pending_kind": pending,
            }))
            .unwrap()
        );

        // 形状守卫：两侧都非空（退化成空表时逐项比较照样绿，那等于没测）。
        assert!(!fixture.stage.is_empty() && !fixture.pending_kind.is_empty());
    }

    /// 规格表：**枚举推不出来的那几件事**由手写表钉住（票 mirror-contract/03，决策 253②）。
    ///
    /// 与 [`shared_enum_members_fixture_matches_the_enums`] 分工明确：那份管「有哪些值」
    /// （Rust 导出、前端断言），这份管「按什么次序 / 归哪一组 / 哪几个是终态」
    /// （手写、两侧各自断言）。红的症状也不同：成员表红了是「认不出一个值」，
    /// 本表红了是「格子顺序不对 / 伪键被 400 拒掉」。
    ///
    /// **判据遍历枚举、不手写数组**（票面要求）：`terminal_statuses` 那条跑完全部
    /// `TaskStatus` 变体问 `is_terminal()`，故「新增一个状态但忘了回答它算不算终态」
    /// 会在这条路上现形；前 10 项与 `ALL_STAGES` 的比对同理。
    ///
    /// 本测试**只管 Rust 那一半**：前端 `STAGE_KEYS` / `PSEUDO_KEYS` / `TERMINAL_STATUSES` /
    /// `pendingLabel` 由 `lib/specTablesFixture.test.ts` 断言同一张表。
    #[test]
    fn shared_spec_tables_match_the_backend_spec() {
        #[derive(serde::Deserialize)]
        struct Fixture {
            stage_keys: Vec<String>,
            pseudo_keys: Vec<String>,
            terminal_statuses: Vec<String>,
            user_decision_context_kinds: Vec<String>,
        }

        let raw = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/frontend_spec_tables.json"
        ));
        let fixture: Fixture = serde_json::from_str(raw).expect("fixture 必须是合法 JSON");

        // ── stage_keys：前 N 项就是 `Stage` 的全部成员（含顺序），后 4 项是伪键 ──
        //
        // 真实阶段那一半**从枚举导出**（`variants_of`），不用 `ALL_STAGES`——那个数组是手写的，
        // 新增一个阶段而忘了加进它就测不到（这正是本模块反复提防的形状：判据不许是手写清单）。
        // `variants_of` 给出的次序是 `schema_for!` 的次序而不是声明序，故这里比**集合**；
        // 次序那一半由 `stage_order_matches_flowchart`（对 `ALL_STAGES`）单独钉住。
        let stages = variants_of("Stage", &schemars::schema_for!(Stage));
        let mut want_stages =
            fixture.stage_keys[..fixture.stage_keys.len() - fixture.pseudo_keys.len()].to_vec();
        want_stages.sort();
        let mut got_stages = stages.clone();
        got_stages.sort();
        assert_eq!(
            want_stages, got_stages,
            "stage_keys 的前半段应当恰好是 Stage 的全部成员"
        );
        assert_eq!(
            fixture.stage_keys.len(),
            stages.len() + fixture.pseudo_keys.len(),
            "stage_keys 应当是「全部真实阶段 + 4 个伪键」：{:?}",
            fixture.stage_keys
        );
        assert_eq!(
            &fixture.stage_keys[stages.len()..],
            fixture.pseudo_keys.as_slice(),
            "stage_keys 的后 4 项应当就是 pseudo_keys"
        );
        // 顺序也 pin：前 10 项 == `ALL_STAGES`（阶段全序，`stage_order_matches_flowchart`
        // 的同一份次序）。这一条是**规格**（流程图次序），枚举推不出来，故照 `ALL_STAGES` 比。
        let ordered: Vec<&str> = ALL_STAGES.iter().map(|s| s.as_str()).collect();
        assert_eq!(
            &fixture.stage_keys[..ALL_STAGES.len()],
            ordered.as_slice(),
            "stage_keys 的前 {} 项应当按阶段全序排列",
            ALL_STAGES.len()
        );

        // ── pseudo_keys：**不是** `Stage` 成员，且一个真阶段也不许混进来 ──
        //
        // 这一条正是 `stage_configs.rs` 那段注释许下的承诺：「这份决定后端收不收这一行，
        // 那份决定设置页列不列得出这一行」。同源在 `app/src/routes/stage_configs.rs`
        // 的 `PSEUDO_STAGE_KEYS`（那边由 api_contract 的 stage-configs 用例钉）。
        for key in &fixture.pseudo_keys {
            assert!(
                !stages.contains(key),
                "伪键 {key} 同时是真实阶段——两处分组打架了"
            );
        }
        assert!(fixture.pseudo_keys.contains(&"foreman".to_string()));

        // ── terminal_statuses：遍历全部 `TaskStatus` 变体，问 `is_terminal()` ──
        //
        // **变体表由 `schema_for!` 导出**（与成员表同一条路），不手写数组——手写的那份
        // 自己会漏（新增一个状态而忘了加进来，这条断言就悄悄少测一格）。
        let mut terminal: Vec<String> = Vec::new();
        for name in variants_of("TaskStatus", &schemars::schema_for!(TaskStatus)) {
            let status = TaskStatus::from_str(&name)
                .unwrap_or_else(|e| panic!("schema 里的 {name} 不是合法 TaskStatus：{e}"));
            if status.is_terminal() {
                terminal.push(name);
            }
        }
        // 顺序不 pin（`schema_for!` 把无文档注释的变体并进一个分支，次序与声明序不同）——
        // 这条钉的是**集合**：终态有哪几个。
        let mut want = fixture.terminal_statuses.clone();
        want.sort();
        terminal.sort();
        assert_eq!(
            want, terminal,
            "terminal_statuses 与 TaskStatus::is_terminal 不一致"
        );

        // ── user_decision 的 context.kind 子类 ──
        //
        // 权威在 `actions.rs::kinds`（那些常量）+ 动作表里 `user_decision` 名下的行。
        // 这里断言的是**表里那几个都能被 `ResumeCause::classify` 认出来并且不落回通用行**
        // ——落回通用行说明前端表里写的是一个后端不认识的子类（界面会给它一个专有的词，
        // 而后端的续接判定按「通用那一行」走，两边的说法就分叉了）。
        for kind in &fixture.user_decision_context_kinds {
            let cause = ResumeCause::classify(PendingKind::UserDecision, Some(kind));
            assert_ne!(
                cause,
                ResumeCause::UserDecision,
                "user_decision 的子类 {kind} 在后端落回了通用那一行——表里有后端不认的值"
            );
        }
        // 反方向：`classify` 认识的子类都要在表里，否则界面会退回「等待决定」。
        //
        // 判据是**问 `classify` 自己**（它是这条映射的唯一权威），而不是遍历一张手写的
        // `ResumeCause` 清单——那样新增一个子类时清单不会自己长出来，这条就静默少测一格。
        // 做法：拿 `actions::kinds` 里**全部**上下文常量逐个问 `classify`，凡是它给出
        // 「非通用行」的那些就是后端认的子类。
        for ctx in BACKEND_CONTEXT_KINDS {
            let cause = ResumeCause::classify(PendingKind::UserDecision, Some(ctx));
            if cause == ResumeCause::UserDecision {
                // 这个常量在 `user_decision` 下就是通用行（比如依赖类的两个，它们挂在
                // `DependencyFailed` 名下）——不是「后端认的 user_decision 子类」，跳过。
                continue;
            }
            assert!(
                fixture
                    .user_decision_context_kinds
                    .contains(&ctx.to_string()),
                "后端认识的 user_decision 子类 {ctx} 不在表里——界面那一格会退回「等待决定」"
            );
        }
    }

    /// 后端认识的全部 `context.kind` 常量（`actions::kinds` 那一族）。
    ///
    /// 这不是「又抄一份清单」：`actions::kinds` 只定义常量、不提供枚举它们的路径，而这里
    /// 需要的正是「逐个问一遍」。清单**由下面那条测试自己钉住**——每个常量都被它用一遍，
    /// 而 `actions::kinds` 里新增一个常量时，配对的守卫在 `types::tests::` 的
    /// `every_actions_kind_is_covered_here`（同一模块末尾）会红：它比对 `actions.rs` 源文本里
    /// 出现的 `pub const X: &str = "..."` 与这张清单，故「加了常量忘了加进来」不会静默。
    const BACKEND_CONTEXT_KINDS: [&str; 10] = [
        crate::actions::kinds::DUPLICATE_RISK,
        crate::actions::kinds::DEVELOP_DESIGN_INPUT_INSUFFICIENT,
        crate::actions::kinds::TEST_DESIGN_INPUT_INSUFFICIENT,
        crate::actions::kinds::JUDGE_DISAGREEMENT,
        crate::actions::kinds::REVIEW,
        crate::actions::kinds::TEST_CODE_ISSUE,
        crate::actions::kinds::GATE_RECHECK,
        crate::actions::kinds::DIRTY_WORKTREE,
        crate::actions::kinds::DEPENDENCY_FAILED,
        crate::actions::kinds::DEPENDENCY_CANCELLED,
    ];

    /// `BACKEND_CONTEXT_KINDS` 覆盖了 `actions::kinds` 里的每一个常量。
    ///
    /// 判据是**读源码文本**（`include_str!` + 正则式扫描），不是再抄一份数组——同一种病
    /// （手写清单自己会漏）不能用同一种药再治一遍。扫描的形态与 `actions.rs` 里那族的写法
    /// 一致：`pub const NAME: &str = "value";`。
    #[test]
    fn every_actions_kind_is_covered_here() {
        let source = include_str!("actions.rs");
        let mut declared: Vec<String> = Vec::new();
        for line in source.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("pub const ") else {
                continue;
            };
            // 只看 `&str` 常量且值里没有格式串——`kinds` 那一族的写法。
            if !rest.contains(": &str = \"") {
                continue;
            }
            if let Some(value) = rest.split("= \"").nth(1).and_then(|v| v.split('"').next()) {
                declared.push(value.to_string());
            }
        }
        assert!(
            declared.len() >= 10,
            "扫 actions.rs 只取到 {} 个常量——扫描式可能过期了：{declared:?}",
            declared.len()
        );
        for value in &declared {
            assert!(
                BACKEND_CONTEXT_KINDS.contains(&value.as_str()),
                "actions.rs 里的常量 {value} 不在 BACKEND_CONTEXT_KINDS 里——\
                 新增/改动了 context.kind 就要同步那张清单（它驱动 user_decision 子类的\
                 反方向断言）"
            );
        }
    }
}
