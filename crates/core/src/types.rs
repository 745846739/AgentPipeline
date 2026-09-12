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
    /// 回溯：sync-check 判定两个设计分支冲突，退回 architect-design.validate_input（决策 83）。
    ///
    /// 与 [`EdgeKind::KickbackDevelop`] 区分：那条边只回开发，这条回架构重跑设计。
    Backtrack,
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
            other => {
                return Err(crate::Error::Validation(format!(
                    "未知 pending 类型：{other}"
                )))
            }
        })
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
    #[serde(default)]
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

/// review.execute 的 submit_metadata。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewResult {
    pub approved: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_report_path: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_changes: Vec<FileChangeSpec>,
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

/// 节点执行记录（决策 63 / 99 / 114）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeRun {
    pub id: i64,
    pub task_id: String,
    pub cursor_id: String,
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
    pub process_group_id: Option<i32>,
    pub last_activity_at: Option<DateTime<Utc>>,
    pub prompt_template_hash: Option<String>,
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
}

impl NodeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            NodeStatus::Running => "running",
            NodeStatus::Success => "success",
            NodeStatus::Failed => "failed",
            NodeStatus::Timeout => "timeout",
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeConversation {
    pub id: i64,
    pub task_id: String,
    pub run_id: i64,
    pub stage: Stage,
    pub node: Node,
    pub attempt: u32,
    pub agent_type: String,
    pub parent_run_id: Option<i64>,
    pub messages_json: serde_json::Value,
    pub metadata_json: Option<serde_json::Value>,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub created_at: DateTime<Utc>,
}

/// 命令日志（§12.4.4）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeCommand {
    pub id: i64,
    pub task_id: String,
    pub run_id: Option<i64>,
    pub stage: Stage,
    pub node: Node,
    pub source: CommandSource,
    pub command: String,
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
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
