//! 生产运行时接线（票 17，决策 54 / 55 / 127 / 130⑤）。
//!
//! 把三块「已实现但没人拉起来」的件接成一条活链路：
//! - [`ProductionLlm`]：真实 LLM 适配层（票 13），生产替换 FakeAgent；
//! - [`Executor`]：DAG 执行引擎（票 11），由 resume 钩子按需 spawn；
//! - [`KanbanScheduler`]：10s tick + 小时级维护（决策 55），与 executor 通过
//!   store + resume 解耦。
//!
//! resume 钩子的单执行者守卫**不在钩子里重复实现**：`Executor::run` 内部有
//! 进程内 `Mutex<HashSet<task_id>>` 非阻塞去重 + DB `executor_owner` 乐观锁
//! （决策 36）。resume 端点的冷却窗口在 `POST /tasks/{id}/resume` 内判定
//! （决策 91 / §3）。scheduler 的 resume 调用（准入 / 超时重试 / 冲突恢复）
//! 不走冷却——那是对系统自身状态的响应。

use std::sync::Arc;
use std::time::Duration;

use agentpipeline_core::agent::client::LlmClient;
use agentpipeline_core::agent::providers::ProductionLlm;
use agentpipeline_core::clock::SystemClock;
use agentpipeline_core::config::Settings;
use agentpipeline_core::pipeline::Executor;
use agentpipeline_core::process::RealProcessKiller;
use agentpipeline_core::scheduler::KanbanScheduler;
use agentpipeline_core::sse::{SseBus, SseSink};
use agentpipeline_core::storage::{AttentionKind, Store};

use crate::state::ResumeHook;

/// 托管放行的自动动作在 app 层的执行者（决策 210② / 票 08、票 09）。
///
/// **按动作分派**，两件各走各自那份唯一实现：
/// - `resume(continue)` → `pipeline::resume::apply_resume`（与 `POST /tasks/{id}/resume`
///   同一份——抄一份到这里等于两套「resume 做了什么」，迟早漂移成「界面按得动、它按不动」）；
/// - `unstick` → `pipeline::unstick::unstick`（与 `task` 工具的手动那一支同一份）。
///
/// 为什么分派必须收在这里：core 不认识 HTTP，也不持有 `force_release`（进程内去重住在
/// `pipeline::executor`，摘它的入口在 app 层）。曾经这里只建 `ResumeRequest`，于是被放行的
/// `unstick` 会走到 `ResumeAction::parse("")` 上——**托管放行了、执行不通**，而 core 那侧
/// 注入替身的用例看不见（替身只会数「有没有被放行」）。
pub struct StewardActions {
    store: Store,
    settings: Settings,
    resume: ResumeHook,
    /// 托管放行的 resume 与界面那颗钮共用同一份事件契约（决策 245）：
    /// 少了它，「值班长替人按了」在实时流里是无声的。
    sse: Arc<dyn SseSink>,
}

impl StewardActions {
    pub fn new(
        store: Store,
        settings: Settings,
        resume: ResumeHook,
        sse: Arc<dyn SseSink>,
    ) -> Self {
        StewardActions {
            store,
            settings,
            resume,
            sse,
        }
    }
}

impl agentpipeline_core::agent::tools::StewardActionRunner for StewardActions {
    fn run(
        &self,
        call: agentpipeline_core::agent::client::ToolCall,
        _ctx: agentpipeline_core::agent::tools::ToolCallContext,
    ) -> futures::future::BoxFuture<
        'static,
        agentpipeline_core::Result<agentpipeline_core::agent::tools::ToolOutcome>,
    > {
        use agentpipeline_core::pipeline::resume::{apply_resume, ResumeRequest};

        let store = self.store.clone();
        let settings = self.settings.clone();
        let resume = self.resume.clone();
        let sse = self.sse.clone();
        Box::pin(async move {
            let args: serde_json::Value = serde_json::from_str(&call.arguments).map_err(|e| {
                agentpipeline_core::Error::Validation(format!("托管动作的参数不是合法 JSON：{e}"))
            })?;
            let task_id = args
                .get("task_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            // `unstick` 与 `resume` 是两回事（决策 210⑧），故先把这一支分出去——
            // 两者的判据在 core 里各有一份实现，这里只做**分派**。
            if args.get("action").and_then(|v| v.as_str()) == Some("unstick") {
                let unstuck = agentpipeline_core::pipeline::unstick::unstick(
                    &store,
                    &agentpipeline_core::pipeline::executor::force_release,
                    &task_id,
                    store.now(),
                    // 宽限换算走唯一那一处（决策 255）：此前这里与提议那条各换算一次，
                    // 而提议那条按**秒**读同一个分钟设置，两条路差 60 倍。
                    agentpipeline_core::pipeline::foreman_actions::owner_stuck_window(&settings),
                )
                .await?;
                return Ok(agentpipeline_core::agent::tools::ToolOutcome::ok(format!(
                    "已自动解除僵死占用（托管）：任务 {task_id}，游标 {} 转 pending\
                     （标终态的 run {:?}），现在可以 resume",
                    unstuck.cursor_id, unstuck.finished_runs
                )));
            }
            let request = ResumeRequest {
                action: args
                    .get("resume_action")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                cursor_id: args
                    .get("cursor_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                target_stage: args
                    .get("target_stage")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                target_node: args
                    .get("target_node")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                input: args
                    .get("input")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            };
            let applied =
                apply_resume(&store, &settings, &resume, &sse, &task_id, &request).await?;
            Ok(agentpipeline_core::agent::tools::ToolOutcome::ok(format!(
                "已自动放行（托管）：任务 {task_id}，动作 {}，游标 {}，{}",
                applied.action,
                applied.cursor_id,
                if applied.requeued {
                    "已交还准入，等下一个 tick 放行"
                } else if applied.spawned {
                    "执行器已拉起"
                } else {
                    "在冷却窗口内，未重复拉起"
                }
            )))
        })
    }
}

/// 小时级维护周期（决策 55：会话清理 + 指标聚合与 10s tick 分开）。
pub const MAINTENANCE_INTERVAL: Duration = Duration::from_secs(3600);

/// 值守轮的驱动周期（票 06）：比去抖窗口短一档即可——真正的判据是窗口到了没有
/// （`ForemanRunner::watch` 自己按 `watch_debounce_sec` 判），这个周期只决定「多久看一次」。
pub const WATCH_INTERVAL: Duration = Duration::from_secs(10);

/// resume 触发的重试预算（决策 226）。
///
/// 此前是 25 次 × ≤100ms ≈ **2.3 秒**，依据是下面那句「旧 executor 退出是毫秒级」。
/// 2026-09-19 实测那句依据不成立：被判超时的执行体可能停在**不返回的模型调用**上，退出
/// 时间以小时计（run 23 被判超时之后又活了 8 小时以上）。2.3 秒的窗口意味着一次静默失败
/// 就够让任务僵死——「一次性触发被静默吞掉会让任务永久停在 pending」这句注释写的正是这个
/// 后果，只是预算给得太小。
///
/// 现在按指数退避试满这个预算。判超时那条路会先请执行体收口（`request_cancel`，决策 226），
/// 正常情况下第一次就拿到执行权；这个预算兜的是「旧执行体收口得慢一点」，而不是「它永远
/// 不退出」——后者归 `unstick`（去重与 owner 强摘 + 游标转 pending）。
const RESUME_RETRY_BUDGET: Duration = Duration::from_secs(30);
/// 起始退避（决策 226）：先紧后松——多数情形就是毫秒级的窗口，别一上来先等一秒。
const RESUME_RETRY_BACKOFF_START: Duration = Duration::from_millis(20);
/// 退避上限（决策 226）：封顶一秒，免得长尾把整个预算耗在几次长睡上。
const RESUME_RETRY_BACKOFF_MAX: Duration = Duration::from_secs(1);

/// 组装生产执行器 + resume 钩子 + 调度器所需的共享件。
pub struct Runtime {
    pub resume_hook: ResumeHook,
    pub sse: Arc<SseBus>,
    executor: Arc<Executor>,
    /// 同一个 LLM 适配器实例既供执行器用、也供值班长用。
    ///
    /// 共享而不是各建一个：`ProductionLlm` 的可见状态只有 store 与 sse，
    /// 两个实例在行为上等价，但各自持有一份就不会有人注意到它们本该是同一个出口
    /// ——将来给 LLM 出口加限流 / 计量时，两份实例会让其中一份悄悄绕过。
    llm: Arc<dyn LlmClient>,
}

impl Runtime {
    /// 构造真实执行链（store 与 sse 均为共享实例）。
    pub fn new(store: Store, settings: Settings, sse: Arc<SseBus>) -> Self {
        let sse_sink: Arc<dyn SseSink> = sse.clone();
        let llm: Arc<dyn LlmClient> = Arc::new(ProductionLlm::new(store.clone(), sse_sink.clone()));
        let executor = Arc::new(Executor::new(
            store.clone(),
            settings,
            sse_sink.clone(),
            llm.clone(),
            Arc::new(RealProcessKiller),
        ));

        let hook_executor = executor.clone();
        let hook_store = store.clone();
        let resume_hook: ResumeHook = Arc::new(move |task_id: &str| {
            let executor = hook_executor.clone();
            let store = hook_store.clone();
            let task_id = task_id.to_string();
            // spawn 而非 await：HTTP 端点与 scheduler tick 都不阻塞在整条流水线上。
            tokio::spawn(async move {
                // 决策 36 的非阻塞抢占会丢弃「旧 executor 已读完游标、尚未释放注册表时
                // 到达的 resume」——一次性触发被静默吞掉会让任务永久停在 pending。
                // 端点（resume / review / merge-decision）与 scheduler 都依赖这次触发，
                // 故做有界重试；旧 executor 退出后重试即可取得执行权。
                // 预算见 `RESUME_RETRY_BUDGET`（决策 226：从约 2.3 秒提到 30 秒）。
                let started = tokio::time::Instant::now();
                let deadline = started + RESUME_RETRY_BUDGET;
                let mut backoff = RESUME_RETRY_BACKOFF_START;
                let mut retries: u32 = 0;
                loop {
                    match executor.try_run(&task_id).await {
                        Ok(true) => return, // 真正跑了一轮
                        Ok(false) => {
                            retries += 1;
                            if tokio::time::Instant::now() >= deadline {
                                break;
                            }
                            tokio::time::sleep(backoff).await;
                            backoff = (backoff * 2).min(RESUME_RETRY_BACKOFF_MAX);
                        }
                        Err(e) => {
                            tracing::error!(task = %task_id, error = %e, "executor 执行失败");
                            return;
                        }
                    }
                }
                tracing::warn!(
                    task = %task_id,
                    "resume 触发被在跑的 executor 持续挡下，放弃本次触发"
                );
                note_resume_blocked(&store, &task_id, started.elapsed(), retries).await;
            });
        });
        Runtime {
            resume_hook,
            sse,
            executor,
            llm,
        }
    }

    /// 共享 LLM 出口（值班长的回话复用同一实例，决策 182）。
    pub fn llm(&self) -> Arc<dyn LlmClient> {
        self.llm.clone()
    }

    /// 启动时探一次完全磁盘访问（决策 306，票 05）——**后台跑，不阻塞启动**。
    ///
    /// 为什么由启动路径探一次而不是在读的那一刻探：`access()` / `stat()` 对受保护路径
    /// **同样会阻塞**，现探等于把「检查授权」变成第二个挂起现场（这次挂满 4 小时的那一处
    /// 正是 `open()`，而探测想避免的恰恰是它）。故结论只能来自启动那一刻的快照，代价是
    /// **滞后**——所以缺授权那句话里必须带「若你刚刚已开启，请重启应用」。
    ///
    /// 三种结局各自的样子：**有授权 → 一个字都不输出**（这是正常状态，不是「没消息」）；
    /// **缺授权 → 一行 warn + 打开系统设置的对应面板**（引导尽力而为，打不开不致命）；
    /// **判不出来 → 什么都不做**（保守方向：判不出来就不拦人）。
    pub fn spawn_disk_access_check() {
        use agentpipeline_core::agent::disk_access::{self, DiskAccessState};
        tokio::spawn(async move {
            if disk_access::probe_and_record().await == DiskAccessState::Denied {
                tracing::warn!("完全磁盘访问：{}", disk_access::DENIED_HINT);
                disk_access::open_system_settings();
            }
        });
    }

    /// 共享执行器（`/projects/analyze` 的 `project_analysis` 伪阶段复用同一实例）。
    pub fn executor(&self) -> Arc<Executor> {
        self.executor.clone()
    }

    /// 启动 10s tick 循环（决策 55）。`shutdown` 置位后停止派发新任务（决策 54）。
    pub fn spawn_tick_loop(
        &self,
        store: Store,
        settings: Settings,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        let scheduler = KanbanScheduler::new(
            store,
            settings,
            Arc::new(SystemClock),
            Arc::new(RealProcessKiller),
            self.sse.clone(),
            self.resume_hook.clone(),
        );
        tokio::spawn(async move {
            if let Err(e) = scheduler.run_loop(shutdown).await {
                tracing::error!(error = %e, "scheduler run_loop 退出");
            }
        });
    }

    /// 启动**值守轮**循环（决策 209④ / 票 06）：有待办且去抖窗口到了，值班长自己醒一次。
    ///
    /// 为什么由 app 层驱动而不是塞进 scheduler 的 tick：调度器不认识值班长（它只用
    /// store + resume），而「醒一次」要花模型的钱——把它挂在 tick 里会让每个调度器用例
    /// 都变成一次潜在的 LLM 调用。这里是一个独立的、可关掉的循环。
    pub fn spawn_watch_loop(
        &self,
        foreman: Arc<agentpipeline_core::pipeline::foreman::ForemanRunner>,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(WATCH_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // 首次 tick 立即完成：跳过，避免启动瞬间就值一次班（那时态势还没稳）
            interval.tick().await;
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        match foreman.watch().await {
                            Ok(Some(turn)) => tracing::info!(
                                session = %turn.session.id,
                                "值守轮播报了一轮"
                            ),
                            // 没有待办 / 未到窗口 / 判定无需处理 / **在失败退避里**：都是「这一趟不说话」
                            Ok(None) => {}
                            // 失败要按类别退避（决策 271：瞬时类 30s 起翻倍、600s 封顶；
                            // 账单 / 配置类 300s 起、1800s 封顶）——「下一趟」不是 10 秒之后
                            Err(e) => tracing::warn!(error = %e, "值守轮失败（待办未消费，按退避窗口重试）"),
                        }
                    }
                    _ = shutdown.changed() => {
                        tracing::info!("值守轮收到停机信号");
                        return;
                    }
                }
            }
        });
    }

    /// 启动小时级维护循环（决策 55）。
    pub fn spawn_maintenance_loop(
        &self,
        store: Store,
        settings: Settings,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        // 循环里要摸出口（决策 383 的摘要定时车）：给 scheduler 一份克隆，
        // 这份留给维护循环自己。
        let notifier_store = store.clone();
        let scheduler = KanbanScheduler::new(
            store,
            settings,
            Arc::new(SystemClock),
            Arc::new(RealProcessKiller),
            self.sse.clone(),
            self.resume_hook.clone(),
        );
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(MAINTENANCE_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // interval 首次 tick 立即完成：跳过，避免启动瞬间做一次重维护。
            interval.tick().await;
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if let Err(e) = scheduler.maintenance().await {
                            tracing::error!(error = %e, "scheduler 维护任务失败");
                        }
                        // 夜间失败摘要的**定时车**（决策 383）：免打扰段结束后的第一个
                        // 整点附近一定补——哪怕之后再没有一条通知去踩漏斗口的机会车。
                        // 没压下东西 / 还在段内时它自己是不操作。
                        if let Some(notifier) = notifier_store.notifier() {
                            notifier.flush_quiet_failures_digest();
                        }
                    }
                    _ = shutdown.changed() => {
                        tracing::info!("维护循环收到停机信号，退出");
                        return;
                    }
                }
            }
        });
    }
}

/// resume 被挡满预算、放弃本次触发时**落的那一条待办**（决策 304，票 03）。
///
/// 放弃这件事此前只留在日志里——2026-09-27 实测就是这么丢的：03:49:30 放弃，此后
/// 2 小时 27 分无人知晓，值守轮一次没醒；而事后倒日志，「持续挡下」那句话在那里躺了
/// **4 次**（09-19、09-26×2、09-27）。
///
/// 三条裁决写在这里：
///
/// - **专属类别，不复用 `scheduler_no_effect`**：后者说的是「调度器处置未生效」，是台账与
///   调度器之间的一条缝；这一条说的是「有人按了续跑、系统试了、没成」。混用会让
///   「这次续跑到底试过没有」永远答不出来。
/// - **放弃不等于卡住，故不自动转 `unstick`**：`try_run` 返回 false 也可能是**有健康执行体
///   正在跑**（决策 36 的进程内去重正是为此），判成卡住会清掉健康占用——`unstick` 文件头
///   为此写了整段警告。这里只落一条待办让人看见。
/// - `detail` 要够复盘：**挡了多久 / 试了几次 / 挡在谁手上**（票 03 的三件）。
///
/// 独立成函数是因为「放弃那一刻落什么账」要能被直接断言：驱动那 30 秒预算的循环在真时钟下
/// 得跑满 30 秒，而 sqlx 连接池在暂停时钟下会取超时（本仓既有实测），两个时钟都不适合
/// 拿来做这条用例的驱动。落账本身与时钟无关，故它收在这里、由用例直接喂读数。
async fn note_resume_blocked(store: &Store, task_id: &str, waited: Duration, retries: u32) {
    let owner = store
        .get_task(task_id)
        .await
        .ok()
        .and_then(|t| t.executor_owner);
    let detail = serde_json::json!({
        "waited_seconds": waited.as_secs(),
        "budget_seconds": RESUME_RETRY_BUDGET.as_secs(),
        "retries": retries,
        "owner": owner,
    });
    if let Err(e) = store
        .note_attention(
            task_id,
            AttentionKind::ResumeBlocked,
            store.now(),
            Some(&detail),
        )
        .await
    {
        tracing::error!(task = %task_id, error = %e, "resume_blocked 待办落账失败");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use testkit::TestHome;

    #[tokio::test]
    async fn resume_hook_spawns_executor_without_blocking() {
        // 钩子只做 spawn：即便任务 id 不存在，调用也立即返回（executor 内部会查库报错）。
        let home = TestHome::new().unwrap();
        let store = store_for(&home).await;
        let runtime = Runtime::new(store, Settings::default(), Arc::new(SseBus::default()));
        (runtime.resume_hook)("missing-task");
    }

    async fn store_for(home: &TestHome) -> Store {
        Store::open(home.home().clone(), Arc::new(SystemClock))
            .await
            .unwrap()
    }

    /// **放弃那一刻落什么账**（决策 304，票 03）：`resume_blocked` 一条，`detail` 够复盘。
    ///
    /// 这是 2026-09-27 那次的直接反面：03:49:30 放弃，此后 2 小时 27 分无人知晓，值守轮
    /// 一次没醒；而事后倒日志，「持续挡下」那句话在那里躺了 4 次（09-19、09-26×2、09-27）。
    ///
    /// 断言的是**落账本身**（与时钟无关），驱动 30 秒预算的那段循环由下面那条静态守卫钉住
    /// ——暂停时钟下 sqlx 取连接会超时、真时钟下要跑满 30 秒，两个时钟都不适合当驱动。
    #[tokio::test]
    async fn a_given_up_resume_leaves_one_resume_blocked_note() {
        use agentpipeline_core::storage::AttentionKind;

        let home = TestHome::new().unwrap();
        let store = store_for(&home).await;
        let repo = home.scratch_dir("proj");
        testkit::seed_project(&store, "p1", "示例", &repo, "main")
            .await
            .unwrap();
        testkit::seed_task(&store, "t1", "p1").await.unwrap();
        // 挡在谁手上要如实记：执行权被**别人**占着（跨进程那一半）。
        assert!(store
            .try_claim_executor("t1", "owner-elsewhere")
            .await
            .unwrap());

        note_resume_blocked(&store, "t1", Duration::from_secs(30), 7).await;

        let open = store.open_attention(10).await.unwrap();
        let noted = open
            .iter()
            .find(|a| a.kind == AttentionKind::ResumeBlocked)
            .unwrap_or_else(|| panic!("放弃那一刻必须落一条 resume_blocked 待办：{open:?}"));
        assert_eq!(noted.task_id, "t1");
        assert!(noted.consumed_at.is_none(), "刚落下的待办还没人处理");

        // detail 足以复盘：挡了多久、试了几次、挡在谁手上（票 03 的三件）。
        let d = noted.detail_json.as_ref().unwrap();
        assert_eq!(d["waited_seconds"], 30, "挡了多久：{d}");
        assert_eq!(d["retries"], 7, "试了几次：{d}");
        assert_eq!(d["owner"], "owner-elsewhere", "挡在谁手上：{d}");

        // **反向断言一**：不落 `scheduler_no_effect`——两者不混（那一条说的是台账与调度器
        // 之间的缝，这一条说的是「有人按了续跑、系统试了、没成」）。
        assert!(
            !open
                .iter()
                .any(|a| a.kind == AttentionKind::SchedulerNoEffect),
            "放弃不进「调度器处置未生效」那一类：{open:?}"
        );

        // **反向断言二**：放弃**不自动转 unstick**——`try_run` 返回 false 也可能是
        // 有健康执行体在跑（决策 36 的进程内去重正是为此），判成卡住会清掉健康占用。
        assert_eq!(
            store
                .get_task("t1")
                .await
                .unwrap()
                .executor_owner
                .as_deref(),
            Some("owner-elsewhere"),
            "只落一条待办，一个字节都不许动执行权"
        );

        // 落账是**幂等**的（去重键含 `occurred_at`，而这里同一次调用只写一行）：
        // 再落一次只多一行，不会把同一次放弃记成很多条。
        assert_eq!(
            open.iter()
                .filter(|a| a.kind == AttentionKind::ResumeBlocked)
                .count(),
            1
        );
    }

    /// 静态守卫：**那条落账真的挂在放弃分支上**（决策 304，票 03）。
    ///
    /// `note_resume_blocked` 自己被上一条用例钉得很死，但「它被不被调用」在运行期要花
    /// 30 秒才看得见（预算），所以这里读自己的源文本——只扫 `#[cfg(test)]` 之前，
    /// 否则会扫到本函数里的字面量。守卫自带禁止字面量时这条最要紧（同 `routes/foreman.rs`
    /// 那两条静态守卫的手法）。
    #[test]
    fn the_give_up_branch_really_notes_it() {
        let src = include_str!("runtime.rs");
        let live = src.split("#[cfg(test)]").next().expect("源文件总在");
        let warn_at = live
            .find("放弃本次触发")
            .expect("放弃那行 warn 必须还在（它是人倒日志时的锚点）");
        let call_at = live
            .find("note_resume_blocked(&store, &task_id")
            .expect("放弃分支必须真的落账（决策 304）——只留日志就是这次故障的形状");
        assert!(call_at > warn_at, "落账要在 warn 之后（同一个放弃现场）");
        assert!(
            live.contains("AttentionKind::ResumeBlocked"),
            "落账用的是专属类别，不许复用 scheduler_no_effect"
        );
    }
}
