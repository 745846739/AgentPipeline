//! 自定义 executor（docs/implementation.md §11.2，docs/pipeline-spec.md §6–§9）。
//!
//! 职责边界：executor 是 DAG 执行引擎——并发驱动一个任务的全部活跃游标，
//! 把节点结果交给路由函数推进游标；定时职责（超时 / 冲突恢复 / 准入）归 scheduler。
//!
//! 关键语义（出处见行内标注）：
//! - **单执行者保证**（决策 36）：进程内注册表（`task_id` → 代次 + 取消观察点）
//!   非阻塞去重 + DB `executor_owner` 乐观锁兜底跨进程；
//! - **判超时先通知执行体收口，并在同一处放开执行权**（决策 226，**由决策 303 补后半句**）：
//!   超时是从台账**外面**判的，而执行体可能正停在一个不返回的模型调用上——那种 run 的
//!   `process_group_id` 是 NULL（只有 `run_command` 起过子进程才回填），
//!   `kill_process_group` 没有东西可杀。[`request_cancel`] 是让 run **真的停下来**的那条线；
//!   缺了它，去重与 `executor_owner` 会被一个已判死的执行体占住。而「协作收口」这个前提
//!   在 2026-09-27 被证伪（执行体停在不返回的同步文件读里、到不了 await 点），故
//!   [`release_ownership`] 由判终态的调度器**当场**放掉两半——不再等那个 future 回来；
//! - **单游标失败不传播**（决策 89）：一条游标的节点失败只把该游标置 pending，
//!   另一分支继续跑完本阶段后停在 join 边界；
//! - **`waiting_join` 只由 `advance_cursor` 写入**（决策 107）；join 由
//!   [`Executor::advance_join`] 在所有游标到界后执行一次（决策 83 / G5）；
//! - 纯代码节点同样落 run 行（`agent_type = "system"`，决策 99 / 114）；
//! - agent 节点按 `agent_retry_max` 干净对话重试，耗尽才 pending（决策 33 / G13）。

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use futures::StreamExt;
use tokio::sync::Notify;

use crate::agent::client::LlmClient;
use crate::agent::tools::{CommandFinish, CommandRecorder, CommandStart};
use crate::clock::Clock;
use crate::config::Settings;
use crate::git::Git;
use crate::process::ProcessKiller;
use crate::sse::{SseEvent, SseSink};
use crate::storage::Store;
use crate::types::{
    Approval, CommandSource, DiffStats, EdgeKind, GateFailureKind, MergeResult, MergeStatus, Node,
    NodeCursor, PendingContext, PendingKind, PendingReason, Project, ReviewMode, Stage,
    SyncDecision, SyncDecisionKind, Task, TestResult,
};
use crate::{Error, Result};

use super::events::{
    emit_node_started, finish_run_with_sse, OUTPUT_CODE_CHANGES, OUTPUT_DESIGN_DOC, OUTPUT_DEV_DOC,
    OUTPUT_REVIEW_DIFF, OUTPUT_REVIEW_REPORT, OUTPUT_SYNC_DECISION, OUTPUT_TEST_REPORT,
    OUTPUT_TEST_SCENARIOS,
};
use super::merge::test_command_for;
use super::model_invoke::ModelInvoke;
use super::run_ledger::RunLedger;

/// 闸门结果（决策 62 / 139）：非零退出是**闸门结果**，不是节点错误。
pub(crate) struct GateOutcome {
    pub(crate) passed: bool,
    pub(crate) failure_kind: GateFailureKind,
    pub(crate) output: String,
}

/// 零提交事实段文件（决策 391）：develop_code_gate 守卫失败时落盘、重入段渲染、
/// 放行 / 申报零变更时清除——「先落文件、再由重入渲染」的决策 126 形状。
pub(crate) const ZERO_COMMIT_FACTS_FILE: &str = "zero-commit-facts.md";

/// 申报比对事实段文件（决策 397）：develop_code_gate 的申报比对判定有漏报时落盘、
/// develop.execute 重入时读回注入——与 [`ZERO_COMMIT_FACTS_FILE`] 同一形状。
pub(crate) const UNDECLARED_CHANGES_FACTS_FILE: &str = "undeclared-changes-facts.md";

/// 申报路径归一化（决策 397）：剥 `./` 前缀、统一分隔符、去首尾空白——
/// 申报面与 diff 面的书写差异不参与判定（`./src/lib.rs` 与 `src/lib.rs` 是同一个）。
fn normalize_declared_path(raw: &str) -> String {
    raw.trim().trim_start_matches("./").replace('\\', "/")
}

/// 申报单向比对（决策 397 · grilling Q11）：diff 面里有、申报面里没有、且不命中
/// 噪音过滤的文件 = 漏报。**单向**——申报面多报的条目容忍（多报无害；双向严格
/// 会被「申报了但最终没改」的正常漂移打脸）。
///
/// 噪音过滤按 [`crate::agent::file_policy::matches_pattern`] 的轻量 glob 语义
/// （`settings.declare_ignore_globs`，缺省 lockfile 家族）：lockfile 漂移几乎每个
/// 任务都有，无过滤的单向比对会立刻假红——106 实证假红空转的代价是跨阶段两小时。
fn undeclared_changes(
    actual: &[String],
    declared: &[String],
    ignore_globs: &[String],
) -> Vec<String> {
    let declared: std::collections::HashSet<String> = declared
        .iter()
        .map(|s| normalize_declared_path(s))
        .collect();
    actual
        .iter()
        .filter(|f| {
            let f = normalize_declared_path(f);
            !declared.contains(&f)
                && !ignore_globs
                    .iter()
                    .any(|g| crate::agent::file_policy::matches_pattern(g, Path::new(&f)))
        })
        .cloned()
        .collect()
}

/// 命中噪音过滤的未申报条目（决策 397 · 票 03 改动二）：**不拦但留痕**——它们不进
/// 漏报清单（不拦 develop），但落进事实段的「已忽略」小节，审计面上「干净通过」与
/// 「有噪音被滤」是两回事。
fn ignored_noise(
    actual: &[String],
    declared: &std::collections::HashSet<String>,
    ignore_globs: &[String],
) -> Vec<String> {
    actual
        .iter()
        .filter(|f| {
            let f = normalize_declared_path(f);
            !declared.contains(&f)
                && ignore_globs
                    .iter()
                    .any(|g| crate::agent::file_policy::matches_pattern(g, Path::new(&f)))
        })
        .cloned()
        .collect()
}

/// 申报比对事实段正文（决策 397）：事实读数 + 补申报指令，与 [`zero_commit_facts`]
/// 同一形状。`headline` 是差异来源的交代（申报缺失 / 不可解析 / 正常漏报各自一句），
/// `undeclared` 是漏报清单；`ignored` 是命中噪音过滤的未申报条目——**不拦但留痕**，
/// 审计要能区分「干净漏报零」与「有噪音被滤」，否则过滤清单就是个静默丢弃的口袋。
pub(crate) fn undeclared_changes_facts(
    undeclared: &[String],
    ignored: &[String],
    headline: &str,
) -> String {
    let mut facts = format!("# 未申报变更事实（确定性检查，决策 397）\n\n{headline}");
    facts.push_str(&format!(
        "- diff 中存在但申报清单里没有的文件：**{}** 个\n",
        undeclared.len()
    ));
    for f in undeclared {
        facts.push_str(&format!("  - `{f}`\n"));
    }
    if !ignored.is_empty() {
        facts.push_str(&format!(
            "- 另有 {} 个未申报文件命中 declare_ignore_globs（不拦，留痕）：\n",
            ignored.len()
        ));
        for f in ignored {
            facts.push_str(&format!("  - `{f}`\n"));
        }
    }
    facts.push_str(
        "\n## 指令\n\
         review 按你申报的 changed_files / unit_test_files 逐文件评审——漏报的文件就是 review 的盲区：\n\
         1. 变更属于本任务：把该文件 git add + commit（先 cd 到系统注入的 worktree 绝对路径），\n\
            再重交 submit_metadata，把漏掉的文件补进 changed_files / unit_test_files\n\
         2. 变更属于误改 / 副产物：撤销这些变更后重交元数据\n\n\
         禁止不补申报也不撤销就直接重交元数据。",
    );
    facts
}

/// [`Executor::declaration_check`] 的三态（决策 397）：读数不可用不冒充任何一端
/// （决策 209 姿态，与 [`ZeroCommitCheck`] 同款）。
enum DeclarationCheck {
    Ok,
    Undeclared { facts: String },
    Unavailable,
}

/// [`Executor::zero_commit_check`] 的三态（决策 391）：读数不可用不冒充任何一端
/// （决策 209 姿态——守卫读数失败既不冒充「有提交」，也不冒充「空分支」）。
enum ZeroCommitCheck {
    HasCommits,
    Empty { facts: String },
    Unavailable,
}

/// develop.execute 是否显式申报了「本任务零变更」（决策 391）：develop 守卫与 merge
/// 空分支改道**共用同一读法**——申报语义（缺省 false、非布尔值当未申报）只在这一处定义，
/// 两条路径不会各自漂移。
pub(crate) async fn declared_no_changes(store: &Store, task_id: &str) -> Result<bool> {
    Ok(store
        .stage_output_metadata(task_id, Stage::Develop, OUTPUT_CODE_CHANGES)
        .await?
        .and_then(|m| m.get("no_changes").and_then(|v| v.as_bool()))
        .unwrap_or(false))
}

/// 零提交事实段正文（决策 391）：事实读数 + 落提交指令，**两条改道路径共用**——
/// develop 守卫（自有提交数 0）与 merge 空分支（净差异 0 文件）事实不同、指令同一。
/// `headline` 是第一条事实（调用方各自组读数的措辞），`dirty` 是 worktree 的
/// `git status --porcelain` 清单。**指令里不带 message 格式**——提交 message 属项目域。
pub(crate) fn zero_commit_facts(headline: &str, dirty: &[String]) -> String {
    let mut facts = format!("# 零提交事实（确定性检查，决策 391）\n\n{headline}");
    if !dirty.is_empty() {
        facts.push_str("- 未提交清单：\n");
        for f in dirty {
            facts.push_str(&format!("  - `{f}`\n"));
        }
    }
    facts.push_str(
        "\n## 指令\n\
         本轮变更必须落成任务分支上的 git 提交后再交 submit_metadata：\n\
         1. 先 cd 到系统注入的 worktree 绝对路径（相对 cwd 会解析到别的 checkout）\n\
         2. git add + git commit（message 遵循本仓提交惯例）\n\
         3. 重交 submit_metadata 前自查 `git rev-list --count <基准>..HEAD` > 0\n\n\
         确无任何变更时不得为凑提交而造假变更：在正文中说明依据并申报 no_changes = true。\n\
         禁止不落提交也不申报零变更就直接重交元数据。",
    );
    facts
}

/// 落盘零提交事实段（决策 391 的落点单点）：守卫与 merge 改道都经它，重入段据此渲染。
pub(crate) fn write_zero_commit_facts(
    home: &crate::home::Home,
    task_id: &str,
    facts: &str,
) -> Result<()> {
    home.ensure_task_dirs(task_id)?;
    std::fs::write(home.task_file(task_id, ZERO_COMMIT_FACTS_FILE), facts)?;
    Ok(())
}

/// 清掉零提交事实段（放行 / 申报零变更 / 守卫已恢复时调用）——不存在的文件不是错误。
pub(crate) fn clear_zero_commit_facts(home: &crate::home::Home, task_id: &str) {
    let _ = std::fs::remove_file(home.task_file(task_id, ZERO_COMMIT_FACTS_FILE));
}

/// 落盘申报比对事实段（决策 397 的落点单点）：守卫判定有漏报时经它，重入段据此渲染。
pub(crate) fn write_undeclared_changes_facts(
    home: &crate::home::Home,
    task_id: &str,
    facts: &str,
) -> Result<()> {
    home.ensure_task_dirs(task_id)?;
    std::fs::write(
        home.task_file(task_id, UNDECLARED_CHANGES_FACTS_FILE),
        facts,
    )?;
    Ok(())
}

/// 清掉申报比对事实段（放行 / 补申报通过 / 读数降级 / 申报零变更时调用）——
/// 留着会在下一轮重入里注入**过期**的漏报清单。
pub(crate) fn clear_undeclared_changes_facts(home: &crate::home::Home, task_id: &str) {
    let _ = std::fs::remove_file(home.task_file(task_id, UNDECLARED_CHANGES_FACTS_FILE));
}

/// develop 闸门最近一次失败的**成因读数**（决策 392 ④）。
///
/// develop 没有 `merge_result` 那样的产出行可给 pending 侧读，故按「先落盘、后读」的
/// 既有形状（决策 126 / 391 同款）落一个小 JSON；闸门通过 / 申报零变更时删除——留着
/// 会在下一轮的 pending 载体里报**过期**成因。
pub(crate) const GATE_FAILURE_FACTS_FILE: &str = "gate-failure-develop.json";

pub(crate) fn write_gate_failure_facts(
    home: &crate::home::Home,
    task_id: &str,
    kind: crate::types::GateFailureKind,
) -> Result<()> {
    home.ensure_task_dirs(task_id)?;
    let value = serde_json::json!({
        "kind": kind.as_str(),
        "log": format!("gate-output-{}.log", Stage::Develop.as_str()),
    });
    std::fs::write(
        home.task_file(task_id, GATE_FAILURE_FACTS_FILE),
        value.to_string(),
    )?;
    Ok(())
}

pub(crate) fn clear_gate_failure_facts(home: &crate::home::Home, task_id: &str) {
    let _ = std::fs::remove_file(home.task_file(task_id, GATE_FAILURE_FACTS_FILE));
}

/// 门失败类 pending 的诊断摘要上限（决策 392 ④）：超界**显式标注**截断量，不静默丢内容
/// （与 `truncate_gate_log` 同一条纪律）。
const GATE_FAILURE_SUMMARY_LIMIT: usize = 2000;

fn truncate_for_context(text: &str, limit: usize) -> String {
    let count = text.chars().count();
    if count <= limit {
        return text.to_string();
    }
    let head: String = text.chars().take(limit).collect();
    format!(
        "{head}\n…（其余 {} 字符已截断，全文见闸门日志）",
        count - limit
    )
}

/// 门失败类 pending 的成因摘要（决策 392 ④）：**读数**，不是建议。
fn gate_failure_summary(kind: &str, stage: Stage, failures: u32, output: Option<&str>) -> String {
    let label = match kind {
        "lint" => "lint（确定性）",
        "test" => "测试用例",
        "empty_branch" => "空分支（分支相对基准零变化）",
        "environment" => "环境（工具链 / 构建环境）",
        other => other,
    };
    let mut s = format!(
        "闸门在 `{}` 阶段累计失败 {failures} 次（最近分类：{label}）。",
        stage.as_str()
    );
    if let Some(out) = output.map(str::trim).filter(|s| !s.is_empty()) {
        s.push_str("\n\n");
        s.push_str(&truncate_for_context(out, GATE_FAILURE_SUMMARY_LIMIT));
    }
    s
}

// ─────────────────────────────── 单执行者注册表（决策 36 / 226）───────────────────────────────

/// 一次执行体登记：代次 + 它的取消观察点。
///
/// **代次**让收尾只摘掉**自己那一格**：`force_release`（`unstick`）会把同名登记强行摘走，
/// 随后的重试取到的是同名的新一格；若旧执行体的 `Drop` 按名字无差别地删，它顺手摘掉的
/// 是新执行体的取消通道。
struct RegistryEntry {
    generation: u64,
    cancel: CancelSignal,
}

/// 一次「请求中止」的观察点（决策 226）。
///
/// 用 `AtomicBool` + `Notify` 而不是单一个 `Notify`：**两条路都要走通**——执行体停在
/// 模型调用上时靠 `notified()` 把它唤醒；它正走在两轮之间（或信号先于观察者到达）时
/// 靠那个布尔值在下一轮开头拦住它。只用 `notify_waiters()` 会**丢信号**（它只唤醒当时
/// 已登记的等待者），只用布尔值则要等到下一轮开头才行——而停住的恰恰就是那一轮。
///
/// **来路也记在这一格里**（决策 276）：同一个中止通道现在有两个发出方，而执行体对它们的
/// 处置**不同**——人按停要求「这一轮的结果一个字都不许写」（见 [`Executor::run_inner`]），
/// 判超时那条路则保持决策 226 的既有行为（结果照常推进，另一条分支的已完成工作不丢）。
/// 不记来路的话，两者共用一个布尔值，执行体只能猜。
#[derive(Clone, Default)]
pub(crate) struct CancelSignal {
    requested: Arc<AtomicBool>,
    origin: Arc<AtomicU8>,
    notify: Arc<Notify>,
}

/// 中止请求的来路（决策 276）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelOrigin {
    /// 判超时（调度器 `handle_timeout`，决策 226）。
    Timeout,
    /// **人按停**：手动暂停 / 重跑本阶段（决策 276）。
    Hold,
}

impl CancelOrigin {
    fn as_u8(self) -> u8 {
        match self {
            CancelOrigin::Timeout => 0,
            CancelOrigin::Hold => 1,
        }
    }

    fn from_u8(raw: u8) -> Self {
        if raw == 1 {
            CancelOrigin::Hold
        } else {
            CancelOrigin::Timeout
        }
    }

    /// 人话（进 run 行的 `error`）：台账里要读得出**是谁**把它按停的。
    pub fn as_str(self) -> &'static str {
        match self {
            CancelOrigin::Timeout => "节点超时",
            CancelOrigin::Hold => "人工暂停 / 重跑",
        }
    }

    /// **落库取值**（票 02①）：`kanban_node_runs.cancel_origin` 那一列写什么。
    ///
    /// 与 [`Self::as_str`] 是两件事，别合并：`as_str` 是给人读的句子（进 `error` 列），
    /// 这个值给判据读（超时梯子据此决定「跳过还是清零」）。判据按报文字样分流是决策 259
    /// 明确不要走的路——改一次措辞就把梯子悄悄改坏，而这种坏法没有任何测试会报警。
    pub fn as_slug(self) -> &'static str {
        match self {
            CancelOrigin::Timeout => crate::storage::observability::CANCEL_ORIGIN_TIMEOUT,
            CancelOrigin::Hold => crate::storage::observability::CANCEL_ORIGIN_HOLD,
        }
    }
}

impl CancelSignal {
    fn request(&self, origin: CancelOrigin) {
        // 先记来路再置位：读的那一方看到 `is_requested()` 为真时，来路一定已经写好了。
        self.origin.store(origin.as_u8(), Ordering::SeqCst);
        self.requested.store(true, Ordering::SeqCst);
        // `notify_one` 而非 `notify_waiters`：无人等待时它**存一个许可**，
        // 于是「信号先到、观察者后建」这个窗口也不会丢。
        self.notify.notify_one();
    }

    pub(crate) fn is_requested(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }

    pub(crate) fn origin(&self) -> CancelOrigin {
        CancelOrigin::from_u8(self.origin.load(Ordering::SeqCst))
    }

    pub(crate) async fn wait(&self) {
        self.notify.notified().await;
    }
}

static EXECUTOR_REGISTRY: LazyLock<Mutex<HashMap<String, RegistryEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// 代次发号器（只服务「这一格是不是我的」这一处判断）。
static REGISTRY_GENERATION: AtomicU64 = AtomicU64::new(0);

struct ExecutorGuard {
    task_id: String,
    generation: u64,
    /// 自己这一格的取消信号（决策 320 修订）：执行体**持有**它而不是在每轮按名字重取。
    /// `force_release` 摘走登记之后（决策 303），按名字取就取不到自己的观察点——
    /// 旧执行体若正走在两轮之间，中止请求就此丢失，它停在下一个不返回的调用里
    /// 再也出不来（转录不落库、会话行缺失）。持有之后，摘登记只影响「新执行体能不能
    /// 进来」，不再影响「旧执行体还能不能被叫停」。
    cancel: CancelSignal,
}

impl Drop for ExecutorGuard {
    fn drop(&mut self) {
        let mut registry = EXECUTOR_REGISTRY.lock().unwrap();
        if registry.get(&self.task_id).map(|e| e.generation) == Some(self.generation) {
            registry.remove(&self.task_id);
        }
    }
}

/// 把 `task_id` 从**进程内去重**里摘掉（决策 210⑧ / 票 09）。
///
/// 这是 `unstick` 的第一个动作，也是它存在的理由：去重集合是「同一任务只跑一个执行体」的
/// 进程内那一半（DB 那一半是 `executor_owner`），而它**只在执行体返回时**才释放。执行体
/// 卡在一个不返回的系统调用里（2026-09-17 实证：`git2` 的 `open()` 被 macOS 拦住）时，
/// 去重永远摘不掉——清了 DB 也没用，重试会被逐次拒掉。
///
/// **代价如实记**：摘掉之后，那个仍然卡着的旧执行体若哪天活过来，可能与新执行体同时写库。
/// **决策 226 把这条代价收窄了**：摘之前先 [`request_cancel`]——旧执行体停在可被打断的
/// await 点（模型调用就是这样）时会自己收口退出、不再写库；只有停在不返回的**阻塞**调用里
/// 的那一类才剩下这条代价。
pub fn force_release(task_id: &str) -> bool {
    request_cancel(task_id);
    EXECUTOR_REGISTRY.lock().unwrap().remove(task_id).is_some()
}

/// **判 run 终态时放开执行权**（决策 303，显式修订决策 226 的一格）。
///
/// 决策 226 原来的口径是「判超时那条路**只发请求、不摘登记**」——它把「执行体会收口」
/// 当成前提（协作式中止：执行体在下一个 await 点自己退出）。2026-09-27 的实测把这个前提
/// 打掉了：执行体停在一次**同步文件读**里（`open()` 挂在完全磁盘访问的授权弹窗上），
/// 既到不了 await 点、也返回不了。于是执行权的**两半同时被占死**：
///
/// - 进程内去重登记永远摘不掉 → `try_run` 逐个拒掉紧随其后的 resume；
/// - `executor_owner` 只在 `run_inner` 返回后才清 → 乐观锁 `IS NULL` 恒为假。
///
/// 两半合起来 = 任务永久停住，只能重启应用或按 `unstick`——而那两件事本该是例外。
/// 注意「执行权」在本仓就是这两半（[`try_acquire`] 与 `Store::try_claim_executor`），
/// 只清一半等于没清：resume 会先被去重拒掉，压根走不到乐观锁那一问。
///
/// 动作与 [`force_release`]（`unstick` 用的那个）**同一套**：先 `request_cancel` 再摘登记，
/// 随后清 owner；代价也同一份（那个仍卡着的旧执行体若哪天活过来，可能与新执行体同时写库
/// ——决策 210⑧ 已记档，决策 226 用「先请求中止」把它收窄）。区别只在**触发者**：
/// 这一处是「台账已经判它终态」，`unstick` 是「人按的」。
///
/// **与恢复序列（决策 127 / 212）不重复也不漏**——两者判据的**对象**不同，不是同一件事的两个写法：
///
/// - `run_recovery_sequence`（`foreman_actions.rs`）是**启动期的一次全表扫**，对象是「上一个
///   进程留下的任何持有者」（`kill -9` 残留），副作用有三步（清 owner / `running` 归队 /
///   项目级 run 标终态），它**不看 run 的终态**、也不认具体是哪个任务；
/// - 本函数是**运行期针对某一个任务**的一格：触发者恰好是「这一行 run 刚被判终态」，对象是
///   那个已被判死的执行体，副作用只有清执行权那一件（不归队、不收项目级 run）。
///
/// 因此不漏：启动扫不到「运行中才判死的持有者」（它只在启动那一刻跑一次），本函数补这一格；
/// 也不重复：本函数不碰状态与游标（归队那步归恢复序列），单跑本函数不会把任务变成 `queued`。
/// 运行期真正的重复风险是**本函数与 `unstick` 撞在同一任务上**，而两者都走
/// [`force_release`] 的幂等路径（摘不到登记返回 `false`、`release_executor` 命中的是同一行
/// 且置 `NULL`），重复调用没有第二种后果。
///
/// 返回值：进程内**当时确实有一个在跑的执行体**（与 [`request_cancel`] 同一个读数）。
pub async fn release_ownership(store: &Store, task_id: &str) -> Result<bool> {
    let had_executor = force_release(task_id);
    // owner 此刻只可能是那个已判死的执行体：`try_claim_executor` 要求 `IS NULL`，
    // 于是没有任何新执行体能在这一句之前抢进来（清完才可能被抢）。
    store.release_executor(task_id).await?;
    Ok(had_executor)
}

/// **有界等在飞执行体自然退出**（决策 320 的顺序保证），界满即返回。
///
/// 判超时那一处在「发中止请求」与「摘登记交出执行权」之间调它：转录落库
/// （`record_failed_attempt`）排在执行体退出**之前**，而超时续接的读
/// （`take_continuation`）排在新执行体起来**之后**——中间不隔这一等，读就可能抢在写
/// 前面，那一轮续接静默退化成空白起跑（原因列读-清一次，被白白消费掉）。
///
/// 只等得到**协作式收口**的那一类：停在不返回的同步调用里的执行体（决策 303 的现场）
/// 到点也退不出来，界满返回，调用方照旧走 [`release_ownership`] 兜底——那份代价原样
/// 保留，这里只是把「等得到」的常见情形排成确定的先后。
pub async fn await_teardown(task_id: &str, bound: std::time::Duration) {
    let deadline = tokio::time::Instant::now() + bound;
    loop {
        if !EXECUTOR_REGISTRY.lock().unwrap().contains_key(task_id) {
            return; // 登记随 guard 落下：转录与 owner 都在此之前写完 / 清完
        }
        if tokio::time::Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

/// 请求中止 `task_id` 正在跑的执行体（决策 226），返回「当时确实有一个在跑」。
///
/// 存在的理由是一条实测：2026-09-19 任务 `01M2QH0DHKGSGNVHC0WT2Q4CG0` 的
/// architect-design.execute 被判超时（run 23）后，它的执行体**又活了 8 小时以上**——
/// 停在一个不返回的模型调用上，没有进程组可杀。于是「超时」只改了台账：run 行是终态，
/// 去重与 `executor_owner` 仍被它占着，调度器接着调 resume 被逐次拒掉，2.3 秒后钩子
/// 放弃，任务就此僵死到有人按 `unstick`。
///
/// **这不是硬中止**，两处边界要说清：
/// - 执行体在**下一个 await 点**收口（[`Error::Cancelled`]），已烧掉的 token 照常落库；
///   停在不返回的**阻塞**系统调用里的执行体收不到——那一类只能靠 `kill_process_group`
///   或重启进程；
/// - 收口是**协作**的：它收口之前，去重与 `executor_owner` 仍被占着。调用方因此不能
///   假定「通知完就立刻能起来」——紧跟其后的 resume 仍要按自己的重试预算等它。
pub fn request_cancel(task_id: &str) -> bool {
    request_cancel_with(task_id, CancelOrigin::Timeout)
}

/// **人按停**（手动暂停 / 重跑本阶段）时请求中止在飞的执行体（决策 276）。
///
/// 与 [`request_cancel`] 走同一个通道、同一份协作语义，唯一差别是**来路**：执行体据此
/// 决定「这一轮的结果一个字都不许写」（[`Executor::run_inner`]）——人按停的意图正是
/// 「停在原处」，而判超时那一方已经自己处置过台账，多写一笔只会覆盖它。
///
/// 返回值同 [`request_cancel`]：`false` = 进程内没有这一号登记（执行体已退出，或本就
/// 没有在跑）——调用方照常落自己那一笔。
pub fn request_hold(task_id: &str) -> bool {
    request_cancel_with(task_id, CancelOrigin::Hold)
}

fn request_cancel_with(task_id: &str, origin: CancelOrigin) -> bool {
    let registry = EXECUTOR_REGISTRY.lock().unwrap();
    match registry.get(task_id) {
        Some(entry) => {
            entry.cancel.request(origin);
            // 决策 406：**请求发出**的时刻要能在日志流里查到。执行体那一侧只在**看到**
            // 信号时留一句（`run_inner` 的 held 分支），而那一刻可能在一分多钟之后
            // （2026-10-08 实测按停延迟 1m43s）——排障不该靠翻库反推按停时刻。
            // 两条来路（人按停 / 判超时）共用这一个出口，故 `origin` 一并带上。
            tracing::info!(
                task = %task_id,
                origin = origin.as_str(),
                notified = true,
                "中止请求已发出"
            );
            true
        }
        None => {
            // 同样是承重的一笔：`notified=false` 正是「按了没反应」要排查的形态
            //（执行体早已退出，或正卡在不返回的阻塞调用里）。
            tracing::info!(
                task = %task_id,
                origin = origin.as_str(),
                notified = false,
                "中止请求已发出，但进程内没有在飞的执行体可通知"
            );
            false
        }
    }
}

/// 人按停的请求已经发出吗（决策 276）——是的话，本轮结论**一个字都不许写台账**。
///
/// 判据收在这一处：两个发出方共用一条通道，而**执行体只对 `Hold` 让路**——判超时
/// 那一方（决策 226）自己已经处置过台账（重试流转 / 挂起），让路反而会把另一条并行
/// 分支已完成的工作一起丢掉。观察点是**执行体自己持有**的那份信号（决策 320 修订：
/// 不再按名字重取，`force_release` 摘登记后按名字取会落空）。
pub(crate) fn held_by_human_signal(cancel: &CancelSignal) -> bool {
    cancel.is_requested() && cancel.origin() == CancelOrigin::Hold
}

/// [`held_by_human_signal`] 的按名字形态：单元测试断言「登记里那一格被请求过」用。
#[cfg(test)]
pub(crate) fn held_by_human(task_id: &str) -> bool {
    cancel_signal(task_id).is_some_and(|s| held_by_human_signal(&s))
}

/// 按名字取登记里的观察点。**只在测试里用**：决策 320 起，执行体的观察点是它
/// `try_run` 时持有（guard 携带）的那份信号，生产路径不再按名字重取——按名字取在
/// `force_release` 摘走登记之后会落空（跨进程 / 已被摘），旧执行体的中止请求就此丢失
/// （它停在下一个不返回的调用里再也出不来，转录也不落库）。这曾经的「两轮之间丢失」
/// 代价（决策 210⑧ 记的老样子）由持有制收掉；登记本身只剩「请求中止」与「去重」两职。
#[cfg(test)]
pub(crate) fn cancel_signal(task_id: &str) -> Option<CancelSignal> {
    EXECUTOR_REGISTRY
        .lock()
        .unwrap()
        .get(task_id)
        .map(|entry| entry.cancel.clone())
}

/// 非阻塞抢占：已有 executor 在跑同一任务时返回 `None`（调用方直接退出）。
fn try_acquire(task_id: &str) -> Option<ExecutorGuard> {
    let mut registry = EXECUTOR_REGISTRY.lock().unwrap();
    if registry.contains_key(task_id) {
        return None;
    }
    let generation = REGISTRY_GENERATION.fetch_add(1, Ordering::Relaxed);
    let cancel = CancelSignal::default();
    registry.insert(
        task_id.to_string(),
        RegistryEntry {
            generation,
            cancel: cancel.clone(),
        },
    );
    Some(ExecutorGuard {
        task_id: task_id.to_string(),
        generation,
        cancel,
    })
}

// ─────────────────────────────── 执行器 ───────────────────────────────

/// executor 的协作依赖。`llm` 是测试接缝（决策 142/143）：生产传真实适配层，
/// 测试传 testkit 的 FakeAgent——工具层始终真实执行（决策 148）。
pub struct Executor {
    store: crate::storage::Store,
    settings: Settings,
    sse: Arc<dyn SseSink>,
    llm: Arc<dyn LlmClient>,
    killer: Arc<dyn ProcessKiller>,
    /// 计时读数的唯一来源（决策 143 接缝① / 票 02）：与 Store 共用同一个 `Clock` 实例。
    clock: Arc<dyn Clock>,
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
        // 模型请求留痕（决策 231）包在**构造处**而不是各调用点：本执行体的所有 LLM 出口
        // （节点工具循环 / 伪阶段 / 项目分析 / 子代理）都由这一层拿到同一个 `llm`，
        // 包一次就全都在账上，调用点一个字不动。测试注入的 FakeAgent 同样过它——
        // 于是「请求可归位」这件事在 L2/L4 的用例里也是真的。
        let llm = Arc::new(crate::agent::recording::RecordingLlm::new(
            llm,
            store.clone(),
        ));
        let clock = store.clock().clone();
        Executor {
            store,
            settings,
            sse,
            llm,
            killer,
            clock,
        }
    }

    /// 台账入口（决策 249 · 票 02）：run 行的开立/收口/步标记/用量/续接都经它——
    /// 顺序与计时的规则收在 `run_ledger` 模块 doc，本侧只留观测面（SSE）。
    fn ledger(&self) -> RunLedger<'_> {
        RunLedger::new(&self.store, self.clock.as_ref())
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
        let result = self.run_inner(task_id, &_guard.cancel).await;
        self.store.release_executor(task_id).await?;
        result?;
        Ok(true)
    }

    /// 核心循环（§11.2 伪码）。
    async fn run_inner(&self, task_id: &str, cancel: &CancelSignal) -> Result<()> {
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
                        let out = this.execute_node(task_ref, &cursor, cancel).await;
                        (cursor, out)
                    }
                })
                .buffer_unordered(4)
                .collect()
                .await;

            let before = self.cursor_snapshot(task_id).await?;
            // **人按停的请求一旦发出，本轮的一切结论都不许写台账**（决策 276）：落点与
            // 挂起都归发出请求的那一方（`pipeline::pause` / `rerun`）。不加这一句的话，
            // 一个「刚好在这一瞬跑完」的节点会把游标推走——人按下的暂停/重跑会被这一笔
            // 静默覆盖，而屏上看起来像是按了没反应。
            //
            // 只认 `Hold`：判超时那条路（决策 226）**行为不变**——它自己已经处置过台账
            // （重试流转或挂起），而另一条并行分支若恰好在这一瞬跑完，那个结果照旧推进，
            // 不该被这一轮的中止连坐。
            let held = held_by_human_signal(cancel);
            // 本轮的某个游标是否被「中止请求」收了口（决策 226）。
            let mut cancelled = false;
            for (cursor, outcome) in results {
                if held {
                    tracing::info!(
                        task = %task.id,
                        stage = %cursor.stage,
                        node = %cursor.node,
                        "本轮已有「人按停」的中止请求：不推进游标，让出执行权"
                    );
                    cancelled = true;
                    continue;
                }
                match outcome {
                    Ok(output) => {
                        if let Err(e) = self.advance_cursor(&task, &cursor, output).await {
                            // 推进失败同样只阻塞本游标（决策 89）
                            self.pend_cursor(&cursor, PendingKind::RetryExhausted, e.to_string())
                                .await?;
                        }
                    }
                    Err(node_error) => {
                        // **被中止 ≠ 这个节点失败了**（决策 226）：判超时那一方
                        // （`handle_timeout`）已经把 run 判终态、写好转重试的 transition、
                        // 并另外叫了一次 resume。这里若照常挂 pending，会把刚放出去的
                        // 那次重试立刻打回去——重试起来只会看到游标 pending 然后原地退出。
                        // 故只记账、不挂 pending，随后让出执行权。
                        if node_error.is_cancelled() {
                            tracing::info!(
                                task = %task.id,
                                stage = %cursor.stage,
                                node = %cursor.node,
                                "执行体按中止请求收口，让出执行权"
                            );
                            cancelled = true;
                            continue;
                        }
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

            // 被中止：本轮的账已经落完，这里退出，把执行权（去重 + `executor_owner`）
            // 让给判超时那一方叫来的重试。**不能靠下面那条「先后快照相等」的防御退出**：
            // 中止发生时台账刚被调度器改过，两次快照往往不相等。
            if cancelled {
                return Ok(());
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
        pend_reason(&self.store, &*self.sse, cursor, reason).await
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
        // 决策 392 ④：门失败类待办（重试耗尽）随 pending 带上**成因读数**。此前这里
        // 只有 `reason_override` 一条成因通路，`route_merge` / `route_code_gate` 的耗尽
        // 分支落到静态文案上——界面上只看到「重试耗尽，需要用户介入」，成因得 ssh 去挖。
        if kind == PendingKind::RetryExhausted {
            return self.retry_exhausted_context(task, cursor).await;
        }
        if kind != PendingKind::UserDecision {
            return Ok(None);
        }
        match (cursor.stage, cursor.node) {
            (Stage::Review, Node::ValidateOutput) => Ok(Some(PendingContext::with_kind(
                crate::actions::kinds::REVIEW,
            ))),
            (Stage::Develop, Node::ValidateOutput) => {
                // 决策 391：develop.validate_output 的 UserDecision 唯一来源是
                // 零变更申报确认（常规 lint/单测失败走 Retry，不经 pending）。
                Ok(Some(PendingContext::with_kind(
                    crate::actions::kinds::ZERO_CHANGES,
                )))
            }
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

    /// 重试耗尽的成因载体（决策 392 ④）。
    ///
    /// 两处来源：merge 读 `merge_result` 行（分类 + 计数 + 失败全文）；develop / test
    /// 读 develop 闸门落盘的成因文件（`GATE_FAILURE_FACTS_FILE`，可能没有——那不是错，
    /// 返回 `None` 落回静态文案）。**不设 `context.kind`**：耗尽类动作集按
    /// `(PendingKind, _)` 匹配，加一个键只会让续接判定表多一个认不出的值。
    async fn retry_exhausted_context(
        &self,
        task: &Task,
        cursor: &NodeCursor,
    ) -> Result<Option<PendingContext>> {
        if cursor.stage == Stage::Merge {
            let Some(merge) = self.store.merge_metadata(&task.id).await? else {
                return Ok(None);
            };
            let kind = merge
                .gate_failure_kind
                .map(|k| k.as_str().to_string())
                .unwrap_or_else(|| "unknown".into());
            let log_path = self
                .store
                .home()
                .task_file(
                    &task.id,
                    &format!("gate-output-{}.log", Stage::Merge.as_str()),
                )
                .display()
                .to_string();
            let summary = gate_failure_summary(
                &kind,
                cursor.stage,
                merge.gate_failures,
                merge.gate_failure_output.as_deref(),
            );
            return Ok(Some(PendingContext::with_gate_failure(
                &kind,
                merge.gate_failures,
                log_path,
                summary,
            )));
        }
        let path = self
            .store
            .home()
            .task_file(&task.id, GATE_FAILURE_FACTS_FILE);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Ok(None);
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            return Ok(None);
        };
        let kind = value
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let log_rel = value
            .get("log")
            .and_then(|v| v.as_str())
            .unwrap_or("gate-output-develop.log");
        let log_path = self
            .store
            .home()
            .task_file(&task.id, log_rel)
            .display()
            .to_string();
        // develop / test 的耗尽计数在游标上（merge 的在 merge_result 行上，决策 108）
        let failures = cursor.validate_attempts;
        let summary = gate_failure_summary(&kind, cursor.stage, failures, None);
        Ok(Some(PendingContext::with_gate_failure(
            &kind, failures, log_path, summary,
        )))
    }

    // ─────────────────────── 节点分发 ───────────────────────

    /// 执行一个节点的"工作"部分，返回交给路由的结论。
    async fn execute_node(
        &self,
        task: &Task,
        cursor: &NodeCursor,
        cancel: &CancelSignal,
    ) -> Result<NodeOutput> {
        let (stage, node) = (cursor.stage, cursor.node);
        match (stage, node) {
            // ── 纯代码节点（G7；决策 99/114：同样落 run 行，agent_type = system）──
            (Stage::Init, Node::Execute) => self.init_execute(task, cursor).await,
            (Stage::Done, Node::Execute) => self.done_execute(task, cursor).await,
            (Stage::SyncCheck, Node::Execute) => {
                // join 由 advance_join 统一执行（决策 107），游标永远不该指向这里
                Err(Error::Validation("sync-check 不占游标行".into()))
            }
            (Stage::Merge, Node::Execute) => self.merge().execute(task, cursor).await,

            // ── 纯代码 validate_output（决策 62）──
            (Stage::Develop, Node::ValidateOutput) => self.develop_code_gate(task, cursor).await,
            (Stage::Test, Node::ValidateOutput) => self.test_code_gate(task, cursor).await,
            (Stage::Review, Node::ValidateOutput) => self.review_verdict(task, cursor).await,

            // ── agent 节点 ──
            _ => self.invoke().agent_node(task, cursor, Some(cancel)).await,
        }
    }

    // ─────────────────────── 纯代码节点 ───────────────────────

    /// init.execute：创建 worktree 隔离工作区（§6；worktree 已存在则复用，§8）。
    async fn init_execute(&self, task: &Task, cursor: &NodeCursor) -> Result<NodeOutput> {
        let project = project_or_err(&self.store, &task.project_id).await?;
        let (run_id, attempt) = self.begin_run(task, cursor, "system").await?;
        let started = self.clock.now();
        let result = self.do_init(task, &project, run_id).await;
        self.finish_run(
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
        let started = self.clock.now();
        let result = self.do_done(task, run_id).await;
        self.finish_run(
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
            let project = project_or_err(&self.store, &task.project_id).await?;
            Git.remove_worktree(Path::new(&project.local_path), Path::new(worktree), true)
                .await?;
            if let Some(branch) = &task.branch_name {
                Git.delete_branch(Path::new(&project.local_path), branch)
                    .await?;
            }
            // 构建缓存回收（票 runner-offload/03 / B5）：任务构建活动结束的确定性时刻。
            crate::prune::prune_build_cache(self.store.home()).await;
        }
        self.mark_step(run_id, "置任务终态").await;
        self.store
            .mark_terminal(&task.id, crate::types::TaskStatus::Done)
            .await?;
        self.store.refresh_task_totals(&task.id).await
    }

    // ─────────────────────── 纯代码 validate_output（决策 62）───────────────────────

    /// develop.validate_output：lint（如配置）+ 单元测试 + 零提交检查，全过才放行
    /// （§6 / 决策 139 / 决策 391）。
    ///
    /// 零提交检查（决策 391）：execute 交了元数据但任务分支相对基准**没有任何自有
    /// 提交**时，lint/单测再绿也是空放行——2026-10-06 的 01M47RQG4M9533F5TMF1AGJXC8
    /// 正是带着全绿闸门穿越 review/test，在 merge 才撞上「diff 为空」。这里在最早能
    /// 发现的点位确定性拦截：事实落 `zero-commit-facts.md`，重入段注入给 execute。
    /// execute 显式申报 `no_changes` 时不拦——改挂 pending(user_decision) 交用户确认
    /// 收尾（路由侧据 `MetadataView.zero_changes` 分流）。
    async fn develop_code_gate(&self, task: &Task, cursor: &NodeCursor) -> Result<NodeOutput> {
        let project = project_or_err(&self.store, &task.project_id).await?;
        let (run_id, attempt) = self.begin_run(task, cursor, "system").await?;
        let started = self.clock.now();
        let worktree = task.worktree_path.clone().unwrap_or_default();
        let gate = run_code_gate(
            &self.store,
            &self.settings,
            self.clock.as_ref(),
            &self.killer,
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
            started,
            gate.as_ref().err().map(|e| e.to_string()),
            &RunTokens::default(),
        )
        .await?;
        let gate = gate?;

        // 申报零变更 → 用户确认收尾（决策 391）。顺手清掉上一轮守卫失败留下的
        // 事实段文件——申报成立后它就是过期证据，留着会在后续重入里误注入。
        if declared_no_changes(&self.store, &task.id).await? {
            clear_zero_commit_facts(self.store.home(), &task.id);
            clear_gate_failure_facts(self.store.home(), &task.id);
            clear_undeclared_changes_facts(self.store.home(), &task.id);
            return Ok(NodeOutput::Route(crate::pipeline::MetadataView {
                zero_changes: true,
                ..Default::default()
            }));
        }

        if gate.passed {
            match self.zero_commit_check(task, &project, &worktree).await {
                ZeroCommitCheck::HasCommits => {
                    // 分支非空：先过申报比对（决策 397），再谈放行；同样清掉上一轮
                    // 守卫失败留下的事实段文件
                    clear_zero_commit_facts(self.store.home(), &task.id);
                    clear_gate_failure_facts(self.store.home(), &task.id);
                    match self.declaration_check(task, &project, &worktree).await {
                        DeclarationCheck::Ok => {
                            clear_undeclared_changes_facts(self.store.home(), &task.id);
                            Ok(NodeOutput::Route(crate::pipeline::MetadataView::passed(
                                true,
                            )))
                        }
                        DeclarationCheck::Undeclared { facts } => {
                            write_undeclared_changes_facts(self.store.home(), &task.id, &facts)?;
                            Ok(NodeOutput::Route(crate::pipeline::MetadataView::passed(
                                false,
                            )))
                        }
                        // git / 库读数不可用：按「读不到」降级放行，不因这一条卡死
                        // （决策 209 姿态，与零提交守卫的 Unavailable 同款）
                        DeclarationCheck::Unavailable => {
                            clear_undeclared_changes_facts(self.store.home(), &task.id);
                            Ok(NodeOutput::Route(crate::pipeline::MetadataView::passed(
                                true,
                            )))
                        }
                    }
                }
                ZeroCommitCheck::Empty { facts } => {
                    write_zero_commit_facts(self.store.home(), &task.id, &facts)?;
                    // 闸门本身过了，失败成因换成「零提交」这一条（决策 392 ④）
                    clear_gate_failure_facts(self.store.home(), &task.id);
                    clear_undeclared_changes_facts(self.store.home(), &task.id);
                    Ok(NodeOutput::Route(crate::pipeline::MetadataView::passed(
                        false,
                    )))
                }
                // git 读数不可用：按「读不到」降级，不因这一条卡死放行（决策 209 姿态）
                ZeroCommitCheck::Unavailable => {
                    clear_gate_failure_facts(self.store.home(), &task.id);
                    clear_undeclared_changes_facts(self.store.home(), &task.id);
                    Ok(NodeOutput::Route(crate::pipeline::MetadataView::passed(
                        true,
                    )))
                }
            }
        } else {
            // 决策 392 ④：develop 侧没有 merge_result 那样的产出行，闸门失败的**成因**
            // 落盘——耗尽转 pending 时 `pending_context_for` 按它填载体（此前只剩静态文案）。
            write_gate_failure_facts(self.store.home(), &task.id, gate.failure_kind)?;
            clear_undeclared_changes_facts(self.store.home(), &task.id);
            Ok(NodeOutput::Route(crate::pipeline::MetadataView::passed(
                false,
            )))
        }
    }

    /// 决策 391 的读数：分支自有提交数 > 0？为 0 时把事实（提交数 + 工作区脏清单）
    /// 组成重入注入文本。git 侧出错一律 [`ZeroCommitCheck::Unavailable`]——
    /// 守卫读数失败不冒充「有提交」，也不冒充「空分支」。
    async fn zero_commit_check(
        &self,
        task: &Task,
        project: &Project,
        worktree: &str,
    ) -> ZeroCommitCheck {
        let Some(branch) = task.branch_name.clone() else {
            return ZeroCommitCheck::Unavailable;
        };
        let repo = Path::new(&project.local_path);
        let Ok(base_ref) = Git.base_ref(repo, &project.default_branch).await else {
            return ZeroCommitCheck::Unavailable;
        };
        let count = match Git.ahead_count(repo, &base_ref, &branch).await {
            Ok(n) => n,
            Err(_) => return ZeroCommitCheck::Unavailable,
        };
        if count > 0 {
            return ZeroCommitCheck::HasCommits;
        }
        let dirty = Git
            .dirty_files(Path::new(worktree))
            .await
            .unwrap_or_default();
        let headline = format!(
            "- 任务分支 `{branch}` 相对基准 `{base_ref}` 的自有提交数：**0**\n\
             - 工作区未提交改动：{} 处\n",
            dirty.len(),
        );
        ZeroCommitCheck::Empty {
            facts: zero_commit_facts(&headline, &dirty),
        }
    }

    /// 决策 397 的读数：diff 面（基准…分支的 committed 差异 + worktree 的**已跟踪**
    /// 未提交改动）里有没有申报清单（`changed_files ∪ unit_test_files`）之外的文件。
    ///
    /// 申报面缺失 / 不可解析按 fail-closed 处置——全部视为漏报（对齐决策 370 的
    /// `metadata_gaps` 语义：元数据残缺时不再「静默跳过校验」）。git / 库读数出错
    /// 一律 [`DeclarationCheck::Unavailable`]，不冒充「无漏报」。
    ///
    /// dirty 面**只取已跟踪改动**（排除 untracked）：untracked 文件不会进任务分支
    /// 的 diff、也就不会进 review 的评审面与 merge 的合并面——把它们算成漏报只会
    /// 制造「无法通过补申报消除」的假红（agent 没有义务为垃圾文件补申报）。
    async fn declaration_check(
        &self,
        task: &Task,
        project: &Project,
        worktree: &str,
    ) -> DeclarationCheck {
        let (declared, declared_headline) = match self
            .store
            .stage_output_metadata(&task.id, Stage::Develop, OUTPUT_CODE_CHANGES)
            .await
        {
            Ok(Some(m)) => match serde_json::from_value::<crate::types::CodeChanges>(m) {
                Ok(c) => (
                    Some(
                        c.changed_files
                            .iter()
                            .chain(c.unit_test_files.iter())
                            .map(|f| f.path.clone())
                            .collect::<Vec<_>>(),
                    ),
                    String::new(),
                ),
                Err(_) => (
                    None,
                    "- 申报元数据不可解析（不是合法的 CodeChanges），按全漏处置\n".to_string(),
                ),
            },
            Ok(None) => (
                None,
                "- submit_metadata 未提交 changed_files / unit_test_files 申报，按全漏处置\n"
                    .to_string(),
            ),
            // 库读数失败：不冒充任何一端（决策 209 姿态）
            Err(_) => return DeclarationCheck::Unavailable,
        };

        let Some(branch) = task.branch_name.clone() else {
            return DeclarationCheck::Unavailable;
        };
        let repo = Path::new(&project.local_path);
        let Ok(base_ref) = Git.base_ref(repo, &project.default_branch).await else {
            return DeclarationCheck::Unavailable;
        };
        let mut actual = match Git.changed_files_vs_base(repo, &base_ref, &branch).await {
            Ok(v) => v,
            Err(_) => return DeclarationCheck::Unavailable,
        };
        match Git.changed_worktree_paths(Path::new(worktree)).await {
            Ok(dirty) => actual.extend(dirty),
            Err(_) => return DeclarationCheck::Unavailable,
        }
        actual.sort();
        actual.dedup();

        // fail-closed（申报面缺失）与正常比对共用同一套判定与噪音过滤——
        // 「按全漏处置」不豁免 declare_ignore_globs，否则 lockfile 漂移会借
        // 元数据残缺的壳把假红带回来。
        let (undeclared, ignored) = match &declared {
            Some(paths) => {
                let declared: std::collections::HashSet<String> =
                    paths.iter().map(|p| normalize_declared_path(p)).collect();
                (
                    undeclared_changes(&actual, paths, &self.settings.declare_ignore_globs),
                    ignored_noise(&actual, &declared, &self.settings.declare_ignore_globs),
                )
            }
            None => (
                undeclared_changes(&actual, &[], &self.settings.declare_ignore_globs),
                ignored_noise(
                    &actual,
                    &std::collections::HashSet::new(),
                    &self.settings.declare_ignore_globs,
                ),
            ),
        };
        if undeclared.is_empty() {
            return DeclarationCheck::Ok;
        }
        DeclarationCheck::Undeclared {
            facts: undeclared_changes_facts(&undeclared, &ignored, &declared_headline),
        }
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
            self.clock.now(),
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
            self.clock.now(),
            None,
            &RunTokens::default(),
        )
        .await?;
        Ok(NodeOutput::Route(crate::pipeline::MetadataView::passed(
            approved,
        )))
    }

    /// merge 状态机入口（决策 249 · 票 04）：显式四件套（store / settings / sse / clock），
    /// 闸门执行经本文件的自由函数出口调——不借 `&self`，不新开 git trait。
    fn merge(&self) -> super::merge::MergeFlow<'_> {
        super::merge::MergeFlow {
            store: &self.store,
            settings: &self.settings,
            sse: self.sse.as_ref(),
            clock: self.clock.as_ref(),
            killer: &self.killer,
        }
    }

    /// 模型调用编排入口（决策 249 · 票 03）：字段全是廉价克隆（连接池 / Arc / 配置）——
    /// 编排片只拿票面点名的那几个依赖，不借 `&self`，全仓 `&Executor` 参数归零。
    fn invoke(&self) -> ModelInvoke {
        ModelInvoke {
            store: self.store.clone(),
            settings: self.settings.clone(),
            llm: self.llm.clone(),
            killer: self.killer.clone(),
            sse: self.sse.clone(),
            clock: self.clock.clone(),
        }
    }

    /// 伪阶段·项目摘要（**外部 6 触点之一**）：内部转发到模型调用编排片（决策 249 · 票 03）。
    ///
    /// **签名在决策 329 动过一次**（加了 `run_id`）——决策 249 冻结它的原话是「要改语义
    /// **单开票**，不顺手改」，这次正是照那句话办的：单开了
    /// `.scratch/project-delete-cascade/issues/02`，app 侧同一批改完，不是为了顺手。
    pub async fn project_analysis(
        &self,
        project: &Project,
        facts: serde_json::Value,
        run_id: Option<i64>,
    ) -> Result<serde_json::Value> {
        self.invoke().project_analysis(project, facts, run_id).await
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
                "# backtrack 反馈\n\ndev blockers：{:?}\ntest blockers：{:?}\n元数据缺项：{:?}\n",
                decision.dev_blockers, decision.test_blockers, decision.metadata_gaps
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

        // sync-check 自身的 system run（决策 107 / 114）：不经 begin_run 包装——这一条不发
        // NodeStarted / NodeFinished（观测面只跟真节点走），台账本身经 RunLedger。
        let (run_id, _) = self
            .ledger()
            .begin(
                task,
                &main.cursor_id,
                Stage::SyncCheck,
                Node::Execute,
                "system",
            )
            .await?;
        self.ledger()
            .finish(run_id, false, self.clock.now(), None, &RunTokens::default())
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

        let test_skipped = skipped(NodeCursor::BRANCH_TEST_DESIGN);
        let arch_meta = self
            .store
            .stage_output_metadata(&task.id, Stage::ArchitectDesign, OUTPUT_DESIGN_DOC)
            .await?;

        // ── 票 04：闸门先看元数据是否**齐全**（fail-closed）──────────────────────────
        // 要这一道是因为 sync-check 依赖的两处字段一旦缺失，下游校验**静默跳过**而不是报错：
        //   · architect-design 的 `acceptance_criteria` 没了 → high 场景的 design_refs 无从比对；
        //   · test-design 的 `test_scenarios` 没了 → 整段引用完整性校验（决策 136）进不去。
        // 2026-10-01 事故里三行 `metadata_json` 全是 `{"readiness":true}`（票 01 的截断把字段
        // 掏空了），闸门因此真空 `Proceed`。判据只看**这两个被消费的字段在不在**，不套
        // `schemars::required`——那三张结构体除 `readiness` 外全带 `#[serde(default)]`
        //（为兼容历史产出），`required` 拦不住"只剩 readiness"这种残缺。
        let metadata_gaps =
            sync_metadata_gaps(test_skipped, arch_meta.as_ref(), test_meta.as_ref());

        // 决策 136：high 优先级场景的 design_refs 缺失/悬空 → blocker
        if !test_skipped {
            let criteria = arch_meta
                .as_ref()
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
                            test_blockers
                                .push(format!("high 场景「{name}」的 design_refs 缺失或悬空"));
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

        // 元数据缺项也算不过闸门（票 04，fail-closed）
        let proceed = dev_readiness
            && test_readiness
            && dev_blockers.is_empty()
            && test_blockers.is_empty()
            && metadata_gaps.is_empty();
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
            metadata_gaps,
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
        let project = project_or_err(&self.store, &task.project_id).await?;
        let repo = Path::new(&project.local_path);
        let path = "review-diff.diff";
        let (run_id, attempt) = self.begin_run(task, cursor, "system").await?;
        let started = self.clock.now();

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
            started,
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
                // 取消 / 归档时那条任务级清理命令（决策 131）不走收口，故不可能有改写。
                original_command: None,
            })
            .await?;
        let preview = crate::agent::tools::command_preview(output, 50, 100);
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
                let edge = crate::pipeline::route(cursor, &ctx);
                // 决策 277④：info_insufficient 的 pending 消息带上 blockers 摘要——
                // 「要问什么」在路由处本就在手（MetadataView 投影），看板卡片直接可见，
                // 用户不必翻会话记录才知道要答什么。
                let reason_override = match (&edge, &ctx.metadata.blockers) {
                    (EdgeKind::Pending(PendingKind::InfoInsufficient), blockers)
                        if !blockers.is_empty() =>
                    {
                        Some(info_insufficient_message(blockers))
                    }
                    _ => None,
                };
                (edge, reason_override)
            }
            NodeOutput::Edge(edge, reason) => (edge, reason),
            // 节点自己已经把整条 `PendingReason` 造好了（含构造者指定的 stage / node，
            // 例如 conflict_wait 写死 architect-design.execute），原样挂上。
            NodeOutput::Pending(reason) => {
                pend_reason(&self.store, &*self.sse, cursor, reason).await?;
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
                // 阶段内重试：attempts +1，回到 execute（重试回边 validate_output → execute）。
                // 三笔写收成一笔事务——这是决策 245 的直接动因。
                crate::pipeline::advance(
                    &self.store,
                    task_id,
                    cursor,
                    crate::pipeline::Landing::Retry,
                    crate::types::TransitionTrigger::NodeRetry,
                    None,
                )
                .await?;
            }
            EdgeKind::Pending(kind) => {
                // 决策 277④：路由侧可随边带自定义消息（info_insufficient 的 blockers
                // 摘要）；未带时维持静态文案。目前 Pending 边没有其他消息来源。
                let message = reason_override
                    .clone()
                    .unwrap_or_else(|| pending_message(kind).to_string());
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
                            crate::pipeline::advance(
                                &self.store,
                                task_id,
                                cursor,
                                crate::pipeline::Landing::JoinBoundary { skipped: false },
                                crate::types::TransitionTrigger::Normal,
                                Some("到达 join 边界"),
                            )
                            .await?;
                            self.emit_cursor_changed(task_id, &cursor.branch).await?;
                        }
                        crate::pipeline::StageLanding::StageEntry(next, next_node) => {
                            let from = (cursor.stage, cursor.node);
                            let to = (next, next_node);
                            crate::pipeline::advance(
                                &self.store,
                                task_id,
                                cursor,
                                crate::pipeline::Landing::Entry(next, next_node),
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

    // ─────────────────────── 小工具 ───────────────────────

    /// 下一个 `attempt` 序号 = 该 `(task, stage, node)` 上**节点自身**的 run 行数 + 1。
    ///
    /// 只数节点自身的 run（决策 172，票 14）：伪阶段 / 子代理复用父节点的 stage/node，
    /// 计入会把一次没重试的节点顶成 `attempt > 1`。取数口径见
    /// [`crate::storage::Store::count_node_owning_runs`]。
    async fn begin_run(
        &self,
        task: &Task,
        cursor: &NodeCursor,
        agent_type: &str,
    ) -> Result<(i64, u32)> {
        begin_run_with_sse(&*self.sse, &self.ledger(), task, cursor, agent_type).await
    }

    /// 系统节点的**步边界留痕**（决策 211④ / 票 04）：实现与 best-effort 姿态见
    /// [`super::run_ledger::RunLedger::mark_step`]。
    async fn mark_step(&self, run_id: i64, step: &str) {
        self.ledger().mark_step(run_id, step).await;
    }

    /// 收口 + NodeFinished（实现见 [`finish_run_with_sse`]——形状与台账读数单点）。
    #[allow(clippy::too_many_arguments)]
    async fn finish_run(
        &self,
        run_id: i64,
        task: &Task,
        cursor: &NodeCursor,
        attempt: u32,
        failed: bool,
        started: chrono::DateTime<chrono::Utc>,
        error: Option<String>,
        tokens: &RunTokens,
    ) -> Result<()> {
        finish_run_with_sse(
            &*self.sse,
            &self.ledger(),
            run_id,
            task,
            cursor,
            attempt,
            failed,
            started,
            error,
            tokens,
        )
        .await
    }
}

// ─────────────────────── 闸门命令执行（决策 62 / 139）───────────────────────────────

/// 跑系统命令并记录 `kanban_node_commands`（source=system）。返回
/// `(exit code, 输出预览)`；启动失败是节点错误，非零退出是**闸门结果**而非节点错误。
///
/// **闸门走命令收口**（决策 297 / 票 02）。它在收口之前是两个 `sh -c` 旁路（本函数与
/// `repair.rs` 那条），于是比 agent 那一侧少了四件事里的三件：没有进程组（超时杀不着
/// 子孙）、从不回填 `process_group_id`（调度器那条超时收口也就够不着它）、没有心跳。
/// 收口之后闸门与 `run_command` 是同一条管道，差别只剩「不改写」与「输出落哪儿」。
#[allow(clippy::too_many_arguments)] // 显式依赖那套（store/settings/killer）照旧；`clock` 在收口之后没有读者（时长由收口量），故退掉
async fn run_system_command(
    store: &Store,
    settings: &Settings,
    killer: &Arc<dyn ProcessKiller>,
    task: &Task,
    run_id: i64,
    stage: Stage,
    node: Node,
    command: &str,
    cwd: &Path,
) -> Result<(i32, String)> {
    // 决策 109 / 票 09：闸门命令的**完整** stdout/stderr 落可读路径（路径确定、覆盖
    // 写入可重入），复检段读全文而非首尾预览。按 stage 命名，同一阶段的闸门重跑覆盖同一文件。
    let full_path = store
        .home()
        .task_file(&task.id, &format!("gate-output-{}.log", stage.as_str()));
    store.home().ensure_task_dirs(&task.id)?;
    let timeout_sec = settings.test_command_timeout_sec;

    let runner = crate::exec::CommandRunner::new(killer.clone())
        .with_recorder(Arc::new(store.clone()))
        // 共享构建缓存（票 runner-offload/03）：闸门/修复的 cargo 也指到
        // {home}/shared-target,worktree 里不再养出第二份 target/。
        .with_extra_env(vec![(
            "CARGO_TARGET_DIR".to_string(),
            store.home().shared_target_path().display().to_string(),
        )]);
    let (out, ()) = runner
        .run(
            crate::exec::CommandRequest {
                owner: crate::exec::CommandOwner {
                    task_id: Some(task.id.clone()),
                    session_id: None,
                    run_id: Some(run_id),
                    stage,
                    node,
                    source: CommandSource::System,
                },
                command,
                cwd,
                timeout_sec,
                spawn: crate::exec::SpawnForm::Shell,
                // 闸门**不改写**（决策 297 / spec §2）：exit code 原样透传，但输出会被重排成
                // 摘要——pytest 的 `assert 1 == 2` 丢了文件与行号；运行器输出不是 rtk 认得的
                // 样子时，输出会被换成**零行**或一句误导性摘要（`Pytest: No tests collected`）。
                // 闸门的输出同时是模型改代码的**唯一证据**与 `gate-output-<stage>.log` 那份
                // **取证物**（决策 211 一脉），而一条命令只跑一次、拿不到「既过滤又原样」两份。
                // **给模型省 token 不能拿证据的面做交换。**
                rewrite: crate::exec::Rewrite::None,
            },
            |out| {
                if out.timed_out {
                    // 超时按闸门失败处理（exit = -1），错误信息进输出；不落全文日志
                    //（与收口之前逐字一致：超时那条路只记一行摘要）。
                    return Ok((
                        CommandFinish {
                            exit_code: Some(-1),
                            stdout_preview: None,
                            stderr_preview: Some(format!("命令超时（{timeout_sec}s）")),
                            duration_ms: out.duration_ms,
                            ..Default::default()
                        },
                        (),
                    ));
                }
                // 闸门命令的全文日志（决策 109 / 211）：完整 stdout/stderr 落可读路径。
                let full_log = match (out.stdout.is_empty(), out.stderr.is_empty()) {
                    (false, false) => format!("[stdout]\n{}\n[stderr]\n{}", out.stdout, out.stderr),
                    (false, true) => out.stdout.clone(),
                    (true, false) => out.stderr.clone(),
                    (true, true) => String::new(),
                };
                std::fs::write(&full_path, &full_log)?;
                Ok((
                    CommandFinish {
                        exit_code: Some(out.exit_code.unwrap_or(-1)),
                        stdout_path: Some(full_path.display().to_string()),
                        stdout_preview: Some(crate::agent::tools::command_preview(
                            &out.stdout,
                            50,
                            100,
                        )),
                        stderr_preview: Some(crate::agent::tools::command_preview(
                            &out.stderr,
                            50,
                            100,
                        )),
                        duration_ms: out.duration_ms,
                    },
                    (),
                ))
            },
        )
        .await?;

    // 启动失败是节点错误（不是闸门结果）：收口把三种结局都收成读数，这里把它们分开。
    if out.spawn_failed() {
        return Err(Error::Git(format!("闸门命令启动失败：{}", out.stderr)));
    }
    if out.timed_out {
        return Ok((-1, format!("命令超时（{timeout_sec}s）")));
    }

    let stdout_preview = crate::agent::tools::command_preview(&out.stdout, 50, 100);
    let stderr_preview = crate::agent::tools::command_preview(&out.stderr, 50, 100);
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
    Ok((out.exit_code.unwrap_or(-1), combined))
}

/// develop / merge 共用的闸门：lint（如配置）+ 测试（决策 139）。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_code_gate(
    store: &Store,
    settings: &Settings,
    clock: &dyn Clock,
    killer: &Arc<dyn ProcessKiller>,
    task: &Task,
    project: &Project,
    run_id: i64,
    stage: Stage,
    node: Node,
    cwd: &Path,
    include_lint: bool,
) -> Result<GateOutcome> {
    // 决策 392 ②：环境预检**先于**一切命令。闸门用错工具链时，lint / 测试的输出会是
    // 一堆看不懂的链接错误，归因也会落到 `Test` 上（2026-10-06 的 01M47RQG4M9533F5TMF1AGJXC8
    // 就是这么空转的）——预检把它变成一句说得清的话，且**不跑测试命令**。
    if let Some(failed) =
        toolchain_preflight(store, settings, killer, task, run_id, stage, node, cwd).await?
    {
        return Ok(failed);
    }
    if include_lint {
        if let Some(lint) = &project.lint_command {
            RunLedger::new(store, clock)
                .mark_step(run_id, &format!("跑 lint：{lint}"))
                .await;
            let (code, output) = run_system_command(
                store, settings, killer, task, run_id, stage, node, lint, cwd,
            )
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
    RunLedger::new(store, clock)
        .mark_step(run_id, &format!("跑测试：{test}"))
        .await;
    let (code, output) = run_system_command(
        store, settings, killer, task, run_id, stage, node, &test, cwd,
    )
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

/// 环境预检（决策 392 ②）：仓库声明的工具链 vs 闸门**实际**会用到的那套。
///
/// 读 `<cwd>/rust-toolchain.toml` 的 `[toolchain].channel`，与经**同一环境路径**
/// （`run_system_command`，即闸门跑 lint / 测试时那份 `PATH` + `CARGO_TARGET_DIR`）
/// 取到的 `rustc --version` 比对；不一致即 `GateFailureKind::Environment`，**不跑测试**。
///
/// **不猜**：没有声明文件、或声明不是可机械比对的版本号（`stable` / `nightly` /
/// `1.98.0-2024-…` 这类带后缀的照取版本前缀）时不判——预检是为了把「静默用错 rustc」
/// 变成一句说得清的话，不是为了在信息不足时拦路（决策 209 姿态）。
///
/// 为什么值得每次跑：106 那次是**配置漂移**（systemd unit 少了 PATH），下次可能是换机、
/// 换 drop-in、换镜像——那时候没人会记得回来看这条。
#[allow(clippy::too_many_arguments)]
async fn toolchain_preflight(
    store: &Store,
    settings: &Settings,
    killer: &Arc<dyn ProcessKiller>,
    task: &Task,
    run_id: i64,
    stage: Stage,
    node: Node,
    cwd: &Path,
) -> Result<Option<GateOutcome>> {
    let Ok(text) = std::fs::read_to_string(cwd.join("rust-toolchain.toml")) else {
        return Ok(None);
    };
    let Some(declared) = declared_toolchain_channel(&text).and_then(|c| version_like(&c)) else {
        return Ok(None);
    };
    let (code, out) = run_system_command(
        store,
        settings,
        killer,
        task,
        run_id,
        stage,
        node,
        "rustc --version",
        cwd,
    )
    .await?;
    let actual = parse_rustc_version(&out);
    if code == 0 && actual.as_deref() == Some(declared.as_str()) {
        return Ok(None);
    }
    let (_, which) = run_system_command(
        store,
        settings,
        killer,
        task,
        run_id,
        stage,
        node,
        "command -v cargo",
        cwd,
    )
    .await?;
    let target_dir = store.home().shared_target_path().display().to_string();
    let actual_show = actual.unwrap_or_else(|| "（`rustc --version` 读不到）".into());
    let output = format!(
        "工具链预检：仓库声明 `{declared}`（rust-toolchain.toml），闸门实际用的是 `{actual_show}`\n\
         - `rustc --version`（退出码 {code}）→ {}\n\
         - `command -v cargo` → {}\n\
         - `CARGO_TARGET_DIR` → {target_dir}\n\
         修法：让闸门进程的 `PATH` / `RUSTUP_TOOLCHAIN` 指向与仓库声明一致的那套\
         （决策 175 的钉法、决策 392 ① 的部署物）。",
        out.trim(),
        which.trim(),
    );
    Ok(Some(GateOutcome {
        passed: false,
        failure_kind: GateFailureKind::Environment,
        output,
    }))
}

/// `rust-toolchain.toml` 的 `[toolchain].channel`（`#` 后是注释）。认不出 → `None`。
fn declared_toolchain_channel(text: &str) -> Option<String> {
    text.lines().find_map(|raw| {
        let line = raw.split('#').next().unwrap_or("").trim();
        let rest = line.strip_prefix("channel")?.trim_start();
        let value = rest
            .strip_prefix('=')?
            .trim()
            .trim_matches(['"', '\''])
            .trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

/// 可机械比对的版本号（`1.98.0` / `1.98`；`1.98.0-x86_64-…` 取版本前缀）。
/// `stable` / `nightly` 这类**不是版本** → `None`（预检不猜）。
fn version_like(channel: &str) -> Option<String> {
    let core = channel
        .split('-')
        .next()
        .unwrap_or(channel)
        .trim()
        .to_string();
    let mut parts = core.split('.');
    let major = parts.next().unwrap_or_default();
    let minor = parts.next().unwrap_or_default();
    let ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    (ok(major) && ok(minor)).then_some(core)
}

/// `rustc --version` 输出里的版本号（`rustc 1.98.0 (…)` → `1.98.0`）。
fn parse_rustc_version(out: &str) -> Option<String> {
    let line = out.lines().find(|l| l.trim_start().starts_with("rustc "))?;
    let v = line
        .trim_start()
        .strip_prefix("rustc ")?
        .split_whitespace()
        .next()?;
    (!v.is_empty()).then(|| v.to_string())
}

// ─────────────────────────────── 节点输出 ───────────────────────────────

use crate::pipeline::subagent::RunTokens;

/// 项目行查询的唯一收口（`get_project` → `Error::Task`）：留守核、模型调用编排与
/// merge 状态机共用——三份逐字副本是 Standards 评审点名的 Duplicated Code（票 05 收口修）。
pub(crate) async fn project_or_err(store: &Store, project_id: &str) -> Result<Project> {
    store
        .get_project(project_id)
        .await?
        .ok_or_else(|| Error::Task(format!("项目不存在：{project_id}")))
}

/// 把一条**现成的** `PendingReason` 挂上（决策 82 / 决策 245）——留守核的挂起出口：
/// 落库走 [`crate::pipeline::advance`]（一笔事务），同步投影与 SSE 在门外。
/// merge 状态机（票 04）与留守核的挂起路径都经本函数，形状单点。
pub(crate) async fn pend_reason(
    store: &Store,
    sse: &dyn SseSink,
    cursor: &NodeCursor,
    reason: PendingReason,
) -> Result<()> {
    crate::pipeline::advance(
        store,
        &cursor.task_id,
        cursor,
        crate::pipeline::Landing::Pause {
            reason: reason.clone(),
        },
        // 挂起不写流转行，这个 trigger 不会被读到（门的签名对五种落点是同一个）。
        crate::types::TransitionTrigger::AutoResume,
        None,
    )
    .await?;
    store.sync_task_projection(&cursor.task_id).await?;
    sse.emit(SseEvent::Pending {
        task_id: cursor.task_id.clone(),
        branch: cursor.branch.clone(),
        cursor_id: cursor.cursor_id.clone(),
        reason,
    });
    Ok(())
}

/// `begin + NodeStarted` 的**唯一组合**（SSE 留在留守核）：留守核的 `begin_run` 包装与
/// merge 状态机（票 04）的阶段开立都经本函数——开立与事件不拆两处写。
pub(crate) async fn begin_run_with_sse(
    sse: &dyn SseSink,
    ledger: &super::run_ledger::RunLedger<'_>,
    task: &Task,
    cursor: &NodeCursor,
    agent_type: &str,
) -> Result<(i64, u32)> {
    let (run_id, attempt) = ledger
        .begin(
            task,
            &cursor.cursor_id,
            cursor.stage,
            cursor.node,
            agent_type,
        )
        .await?;
    emit_node_started(sse, task, cursor, attempt, run_id);
    Ok((run_id, attempt))
}

/// 节点执行结论：大多数走路由；少数（merge 冲突打回 / 决策 135 分歧）直接给边或 pending。
/// `Edge` 的第二个字段覆盖默认流转原因（如冲突文件清单）。
pub(crate) enum NodeOutput {
    Route(crate::pipeline::MetadataView),
    Edge(EdgeKind, Option<String>),
    Pending(PendingReason),
}

// ─────────────────────── prompt 组装辅助（票 12：§10.3 / G3 / G6 / G12）───────────────────────

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

/// `info_insufficient` 的 pending 消息（决策 277④）：静态文案 + blockers 摘要。
///
/// blockers 的语义是「要问用户的问题」（决策 277②：每条 = 问题 + 推荐答案）。此前
/// 它们只活在会话转录里，用户得翻会话才知道要答什么；摘要进 pending 消息后看板卡片
/// 直接可见。**摘要不全文**：超过 [`INFO_INSUFFICIENT_SUMMARY_LIMIT`] 字符即显式截断
/// （与 `truncate_gate_log` 同一条纪律——不静默丢内容），完整清单仍在会话记录里。
fn info_insufficient_message(blockers: &[String]) -> String {
    if blockers.is_empty() {
        return pending_message(PendingKind::InfoInsufficient).to_string();
    }
    let mut out = format!(
        "{}。要问的问题：",
        pending_message(PendingKind::InfoInsufficient)
    );
    for (i, b) in blockers.iter().enumerate() {
        let line = format!("\n{}. {}", i + 1, b.trim());
        if out.chars().count() + line.chars().count() > INFO_INSUFFICIENT_SUMMARY_LIMIT {
            out.push_str(&format!("\n…（其余 {} 条见会话记录）", blockers.len() - i));
            return out;
        }
        out.push_str(&line);
    }
    out
}

/// blockers 摘要的字符上界（决策 277④「截断」的定稿值）：pending 消息面向看板卡片，
/// 十几条问题全文塞进去会把卡片顶没；400 字符 ≈ 卡片两三行。
const INFO_INSUFFICIENT_SUMMARY_LIMIT: usize = 400;

/// 闸门失败输出（决策 109）：命令 + 退出码 + stdout/stderr 预览，注入 test.execute 复检 prompt。
fn gate_output(kind: &str, command: &str, code: i32, output: &str) -> String {
    let output = output.trim();
    if output.is_empty() {
        format!("{kind}命令 `{command}` 退出码 {code}")
    } else {
        format!("{kind}命令 `{command}` 退出码 {code}\n{output}")
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
        push_after_merge: false,
    }
}

// ─────────────────────────────── 辅助（元数据布尔 / 字符串列表）───────────────────────────────

fn meta_flag(meta: Option<&serde_json::Value>, key: &str) -> bool {
    meta.and_then(|m| m.get(key))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// 票 04：同步闸门依赖的字段**缺项清单**（纯函数，好单测）。
///
/// 只在 test-design 没被跳过时检查——那一支跳过了就没有 `test_scenarios`，也就没有引用校验。
/// 缺项的文案直说"哪一行缺哪个字段"，回溯时它会随 `backtrack-feedback.md` 一起回到设计阶段。
fn sync_metadata_gaps(
    test_skipped: bool,
    arch_meta: Option<&serde_json::Value>,
    test_meta: Option<&serde_json::Value>,
) -> Vec<String> {
    if test_skipped {
        return Vec::new();
    }
    let mut gaps = Vec::new();
    if !meta_has_key(arch_meta, "acceptance_criteria") {
        gaps.push(
            "architect-design 元数据缺 acceptance_criteria：high 场景的 design_refs 无处比对"
                .into(),
        );
    }
    if !meta_has_key(test_meta, "test_scenarios") {
        gaps.push("test-design 元数据缺 test_scenarios：引用完整性校验被整段跳过".into());
    }
    gaps
}

/// 该字段**在不在**（票 04 的 fail-closed 判据）。
///
/// 与 [`meta_flag`] 的区别：后者把"缺字段"和"值就是 false"都读成 false，闸门看不出
/// 元数据是被掏空了还是模型真的判不合格；这里只问键在不在，好把"残缺"单独点出来。
fn meta_has_key(meta: Option<&serde_json::Value>, key: &str) -> bool {
    meta.and_then(|m| m.get(key)).is_some()
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
    fn gate_output_includes_log_when_present() {
        assert_eq!(
            gate_output("测试", "cargo test", 1, "  "),
            "测试命令 `cargo test` 退出码 1"
        );
        let with_log = gate_output("测试", "cargo test", 1, "FAILED: test_login\n");
        assert!(with_log.contains("退出码 1"));
        assert!(with_log.contains("FAILED: test_login"));
    }

    /// 决策 392 ②：预检的三段读数解析（声明版本 / 可比对性 / 实际版本）。
    #[test]
    fn toolchain_preflight_readings_parse() {
        // 声明通道：带注释、带引号、单引号、缺引号都认
        assert_eq!(
            declared_toolchain_channel("[toolchain]\nchannel = \"1.98.0\" # 与 Makefile 同源\n"),
            Some("1.98.0".to_string())
        );
        assert_eq!(
            declared_toolchain_channel("channel='1.98.0'\n"),
            Some("1.98.0".to_string())
        );
        assert_eq!(
            declared_toolchain_channel("[toolchain]\nchannel = \"stable\"\n"),
            Some("stable".to_string())
        );
        // 没有 channel 行 → 认不出（预检不猜）
        assert_eq!(
            declared_toolchain_channel("[toolchain]\nprofile = \"minimal\"\n"),
            None
        );
        assert_eq!(declared_toolchain_channel("channel =\n"), None);

        // 可机械比对的版本：`1.98.0` / `1.98`；`stable` / `nightly` / 空 → 不判
        assert_eq!(version_like("1.98.0"), Some("1.98.0".to_string()));
        assert_eq!(version_like("1.98"), Some("1.98".to_string()));
        assert_eq!(
            version_like("1.98.0-x86_64-unknown-linux-gnu"),
            Some("1.98.0".to_string())
        );
        assert_eq!(version_like("stable"), None);
        assert_eq!(version_like("nightly"), None);
        assert_eq!(version_like("1"), None);
        assert_eq!(version_like("1.x"), None);

        // 实际版本从 `rustc --version` 里取
        assert_eq!(
            parse_rustc_version("rustc 1.98.0 (b4b3f0a1c 2025-01-15)\n"),
            Some("1.98.0".to_string())
        );
        assert_eq!(parse_rustc_version("cargo 1.98.0\n"), None);
        assert_eq!(parse_rustc_version(""), None);
    }

    // ── 决策 397：申报单向比对的纯函数部分 ──

    /// 申报路径归一化：`./` 前缀、反斜杠、首尾空白不参与判定。
    #[test]
    fn declared_paths_are_normalized_before_comparison() {
        assert_eq!(normalize_declared_path("src/lib.rs"), "src/lib.rs");
        assert_eq!(normalize_declared_path("./src/lib.rs"), "src/lib.rs");
        assert_eq!(normalize_declared_path(" src\\lib.rs "), "src/lib.rs");
        assert_eq!(normalize_declared_path("  "), "");
    }

    /// 单向比对的四条判据：漏报必现、多报容忍、噪音过滤、路径归一化。
    #[test]
    fn undeclared_changes_is_one_sided_with_noise_filter() {
        let actual = vec![
            "src/lib.rs".to_string(),
            "src/new.rs".to_string(),
            "Cargo.lock".to_string(),
            "tests/e2e/main.rs".to_string(),
        ];
        let declared = vec!["./src/lib.rs".to_string(), "Cargo.lock".to_string()];

        // 无过滤：new.rs 与 tests/e2e/main.rs 是漏报；Cargo.lock 已申报不算
        let out = undeclared_changes(&actual, &declared, &[]);
        assert_eq!(out, vec!["src/new.rs", "tests/e2e/main.rs"]);

        // 噪音过滤（glob 按文件名匹配）：Cargo.lock 即便未申报也不算漏报
        let actual2 = vec!["src/lib.rs".to_string(), "Cargo.lock".to_string()];
        assert_eq!(
            undeclared_changes(
                &actual2,
                &["src/lib.rs".to_string()],
                &["*.lock".to_string()]
            ),
            Vec::<String>::new()
        );

        // 多报容忍：申报了但 diff 里没有 → 不是失败
        let over_declared = vec!["src/lib.rs".to_string(), "src/ghost.rs".to_string()];
        assert_eq!(
            undeclared_changes(&["src/lib.rs".to_string()], &over_declared, &[]),
            Vec::<String>::new()
        );
    }

    /// 事实段正文带漏报清单与两条出路（补申报 / 撤销），不出现「静默」措辞。
    #[test]
    fn undeclared_facts_list_the_files_and_both_remedies() {
        let facts = undeclared_changes_facts(
            &["src/missed.rs".to_string()],
            &["Cargo.lock".to_string()],
            "- 正常漏报\n",
        );
        assert!(facts.contains("src/missed.rs"), "{facts}");
        assert!(facts.contains("changed_files"), "{facts}");
        assert!(facts.contains("撤销"), "{facts}");
        assert!(facts.contains("git add + commit"), "{facts}");
        // 噪音留痕：审计能区分「干净通过」与「有噪音被滤」（票 03 改动二）
        assert!(facts.contains("Cargo.lock"), "{facts}");
        assert!(facts.contains("declare_ignore_globs"), "{facts}");
    }

    /// 决策 392 ④：诊断摘要超界时**显式标注**截断量（不静默丢内容）。
    #[test]
    fn gate_failure_summary_truncates_loudly() {
        let short = gate_failure_summary("lint", Stage::Develop, 2, Some("一小段"));
        assert!(short.contains("lint（确定性）"));
        assert!(short.contains("累计失败 2 次"));
        assert!(short.contains("一小段"));
        assert!(!short.contains("已截断"));

        let long = "x".repeat(GATE_FAILURE_SUMMARY_LIMIT + 10);
        let out = gate_failure_summary("environment", Stage::Merge, 1, Some(&long));
        assert!(out.contains("环境（工具链 / 构建环境）"));
        assert!(out.contains("其余 10 字符已截断"));
    }

    #[test]
    fn info_insufficient_message_without_blockers_is_the_static_text() {
        // 无 blockers 时与旧文案逐字一致（决策 277④ 只做加法，不改空情形）
        assert_eq!(info_insufficient_message(&[]), "设计输入信息不足，请补充");
    }

    #[test]
    fn info_insufficient_message_lists_blockers_as_numbered_questions() {
        let m = info_insufficient_message(&[
            "部署目标是什么？推荐：本地 Docker".to_string(),
            "并发量级？推荐：单机 < 100 QPS".to_string(),
        ]);
        assert!(
            m.starts_with("设计输入信息不足，请补充。要问的问题："),
            "{m}"
        );
        assert!(m.contains("\n1. 部署目标是什么？推荐：本地 Docker"), "{m}");
        assert!(m.contains("\n2. 并发量级？推荐：单机 < 100 QPS"), "{m}");
    }

    #[test]
    fn info_insufficient_message_truncates_with_an_explicit_notice() {
        // 截断必须显式（与 truncate_gate_log 同纪律）：有「其余 N 条」的标注，
        // 且总长有上界——不静默丢，也不把看板卡片顶没。
        let blockers: Vec<String> = (0..60)
            .map(|i| {
                format!(
                    "第 {i} 个问题：{}？推荐：选项 {i}",
                    "很长的问题描述".repeat(12)
                )
            })
            .collect();
        let m = info_insufficient_message(&blockers);
        assert!(m.contains("其余"), "截断处须有显式标注：{m}");
        assert!(
            m.chars().count() <= INFO_INSUFFICIENT_SUMMARY_LIMIT + 40,
            "摘要应有上界：{}",
            m.chars().count()
        );
        // 完整清单不带全文
        assert!(!m.contains("第 59 个问题"), "{m}");
    }

    /// 中止请求的**来路**随请求一起落进那一格（决策 276）。
    ///
    /// 这条钉的是分类本身：两个发出方（判超时 / 人按停）共用一条通道，而执行体只对后者
    /// 让路——来路存错（或忘了存）时的症状是**按了暂停它照样往前跑**，一个不会报错的错。
    /// 用两个各自独占的 task_id：注册表是进程全局的，本模块的用例并行跑。
    #[test]
    fn the_cancel_origin_travels_with_the_request() {
        let hold_id = "t-cancel-origin-hold";
        let _guard = try_acquire(hold_id).expect("注册表里应当没有这一号");
        assert!(!held_by_human(hold_id), "还没发请求：不算按住");
        assert!(request_hold(hold_id), "刚登记过，应当找得到");
        assert!(held_by_human(hold_id), "人按停：执行体要让路");
        assert_eq!(cancel_signal(hold_id).unwrap().origin(), CancelOrigin::Hold);

        let timeout_id = "t-cancel-origin-timeout";
        let _guard = try_acquire(timeout_id).expect("注册表里应当没有这一号");
        assert!(request_cancel(timeout_id));
        assert_eq!(
            cancel_signal(timeout_id).unwrap().origin(),
            CancelOrigin::Timeout
        );
        assert!(
            !held_by_human(timeout_id),
            "判超时那条路**不让路**：它自己处置过台账，另一条分支的工作不该连坐"
        );
    }

    /// 票 04：只剩 `readiness` 的元数据（2026-10-01 事故的形状）必须被点出缺项。
    ///
    /// 这三行在事故里全是 `{"readiness":true}`——`readiness` 在，闸门本来就会放行，
    /// 缺的正是引用校验赖以比对的两个字段。所以判据刻意**不看 `schemars::required`**
    ///（那三张结构体除 `readiness` 外全是 `#[serde(default)]`），只看这两个键在不在。
    #[test]
    fn sync_gaps_name_the_fields_the_gate_consumes() {
        let only_readiness = serde_json::json!({"readiness": true});
        let gaps = sync_metadata_gaps(false, Some(&only_readiness), Some(&only_readiness));
        assert_eq!(gaps.len(), 2, "{gaps:?}");
        assert!(
            gaps.iter().any(|g| g.contains("acceptance_criteria")),
            "{gaps:?}"
        );
        assert!(
            gaps.iter().any(|g| g.contains("test_scenarios")),
            "{gaps:?}"
        );
    }

    /// 齐全时一个缺项都不报——不许把正常路径判死。
    #[test]
    fn sync_gaps_are_empty_when_the_gate_inputs_are_intact() {
        let arch = serde_json::json!({"readiness": true, "acceptance_criteria": []});
        let test = serde_json::json!({"readiness": true, "test_scenarios": []});
        assert!(sync_metadata_gaps(false, Some(&arch), Some(&test)).is_empty());
    }

    /// test-design 被跳过（skip_to_join）时没有 test_scenarios 可言，不该判缺项。
    #[test]
    fn sync_gaps_ignore_a_skipped_test_branch() {
        assert!(sync_metadata_gaps(true, None, None).is_empty());
    }

    /// 整行产出都没落库（None）同样算缺项——那是元数据被掏空的最彻底形态。
    #[test]
    fn sync_gaps_treat_a_missing_row_as_a_gap() {
        let gaps = sync_metadata_gaps(false, None, None);
        assert_eq!(gaps.len(), 2, "{gaps:?}");
    }

    // ── 中止请求留痕（决策 406）──
    //
    // 2026-10-08 的实证：按停的**时刻**在日志里零痕迹（执行体只在**看到**信号时留一句，
    // 而那一刻可能在一分多钟之后），排障只能靠 `kanban_node_cursors.updated_at` 反推。
    // 两个分支各钉一条：登记里有执行体（notified=true）与没有（notified=false）。

    /// 登记里**没有**在飞的执行体：同样留痕——`notified=false` 正是「按了没反应」要查的形态。
    #[tokio::test]
    async fn a_cancel_request_without_an_executor_is_still_logged() {
        let capture = testkit::log_capture::LogCapture::start();
        // 任务 id 每个用例各给一个：执行体注册表是进程全局的（同 pause.rs 的约定）。
        assert!(!request_hold("logged-without-executor"));
        let logs = capture.text();
        assert!(logs.contains("logged-without-executor"), "logs: {logs}");
        assert!(logs.contains("中止请求已发出"), "logs: {logs}");
        assert!(logs.contains("notified=false"), "logs: {logs}");
        assert!(logs.contains("人工暂停"), "来路要带上: {logs}");
    }

    /// 登记里**有**执行体：notified=true，来路照记（判超时那条路也一样）。
    #[tokio::test]
    async fn a_cancel_request_reaching_an_executor_is_logged_with_its_origin() {
        let guard = try_acquire("logged-with-executor").expect("空登记应当拿得到这一格");
        let capture = testkit::log_capture::LogCapture::start();
        assert!(request_cancel("logged-with-executor"));
        let logs = capture.text();
        assert!(logs.contains("logged-with-executor"), "logs: {logs}");
        assert!(logs.contains("notified=true"), "logs: {logs}");
        assert!(logs.contains("节点超时"), "来路要带上: {logs}");
        assert!(guard.cancel.is_requested(), "信号确实置位了");
        drop(guard);
    }
}
