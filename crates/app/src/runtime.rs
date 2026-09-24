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
use agentpipeline_core::storage::Store;

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
        let resume_hook: ResumeHook = Arc::new(move |task_id: &str| {
            let executor = hook_executor.clone();
            let task_id = task_id.to_string();
            // spawn 而非 await：HTTP 端点与 scheduler tick 都不阻塞在整条流水线上。
            tokio::spawn(async move {
                // 决策 36 的非阻塞抢占会丢弃「旧 executor 已读完游标、尚未释放注册表时
                // 到达的 resume」——一次性触发被静默吞掉会让任务永久停在 pending。
                // 端点（resume / review / merge-decision）与 scheduler 都依赖这次触发，
                // 故做有界重试；旧 executor 退出后重试即可取得执行权。
                // 预算见 `RESUME_RETRY_BUDGET`（决策 226：从约 2.3 秒提到 30 秒）。
                let deadline = tokio::time::Instant::now() + RESUME_RETRY_BUDGET;
                let mut backoff = RESUME_RETRY_BACKOFF_START;
                loop {
                    match executor.try_run(&task_id).await {
                        Ok(true) => return, // 真正跑了一轮
                        Ok(false) => {
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
                            // 没有待办 / 未到窗口 / 判定无需处理：都是「这一趟不说话」
                            Ok(None) => {}
                            Err(e) => tracing::warn!(error = %e, "值守轮失败（待办未消费，下一趟重试）"),
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
}
