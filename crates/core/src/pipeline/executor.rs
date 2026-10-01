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
    emit_node_started, finish_run_with_sse, OUTPUT_DESIGN_DOC, OUTPUT_DEV_DOC, OUTPUT_REVIEW_DIFF,
    OUTPUT_REVIEW_REPORT, OUTPUT_SYNC_DECISION, OUTPUT_TEST_REPORT, OUTPUT_TEST_SCENARIOS,
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
            true
        }
        None => false,
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
                Err(Error::Validation(
                    "sync-check 不占游标行（决策 107）".into(),
                ))
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
        }
        self.mark_step(run_id, "置任务终态").await;
        self.store
            .mark_terminal(&task.id, crate::types::TaskStatus::Done)
            .await?;
        self.store.refresh_task_totals(&task.id).await
    }

    // ─────────────────────── 纯代码 validate_output（决策 62）───────────────────────

    /// develop.validate_output：lint（如配置）+ 单元测试，全过才放行（§6 / 决策 139）。
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

    let runner =
        crate::exec::CommandRunner::new(killer.clone()).with_recorder(Arc::new(store.clone()));
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
        gaps.push(
            "test-design 元数据缺 test_scenarios：引用完整性校验（决策 136）被整段跳过".into(),
        );
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
}
