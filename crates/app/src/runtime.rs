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

use agentpipeline_core::agent::providers::ProductionLlm;
use agentpipeline_core::clock::SystemClock;
use agentpipeline_core::config::Settings;
use agentpipeline_core::pipeline::Executor;
use agentpipeline_core::process::RealProcessKiller;
use agentpipeline_core::scheduler::KanbanScheduler;
use agentpipeline_core::sse::{SseBus, SseSink};
use agentpipeline_core::storage::Store;

use crate::state::ResumeHook;

/// 小时级维护周期（决策 55：会话清理 + 指标聚合与 10s tick 分开）。
pub const MAINTENANCE_INTERVAL: Duration = Duration::from_secs(3600);

/// 组装生产执行器 + resume 钩子 + 调度器所需的共享件。
pub struct Runtime {
    pub resume_hook: ResumeHook,
    pub sse: Arc<SseBus>,
    executor: Arc<Executor>,
}

impl Runtime {
    /// 构造真实执行链（store 与 sse 均为共享实例）。
    pub fn new(store: Store, settings: Settings, sse: Arc<SseBus>) -> Self {
        let sse_sink: Arc<dyn SseSink> = sse.clone();
        let executor = Arc::new(Executor::new(
            store.clone(),
            settings,
            sse_sink.clone(),
            Arc::new(ProductionLlm::new(store.clone(), sse_sink)),
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
                const MAX_ATTEMPTS: u32 = 25;
                for attempt in 0..MAX_ATTEMPTS {
                    match executor.try_run(&task_id).await {
                        Ok(true) => return, // 真正跑了一轮
                        Ok(false) => {
                            let backoff = std::time::Duration::from_millis(
                                20 * u64::from(attempt + 1).min(5),
                            );
                            tokio::time::sleep(backoff).await;
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
        }
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
