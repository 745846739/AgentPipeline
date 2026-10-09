//! KanbanScheduler（决策 55 / 57 / 66 / 92 / 100 / 102 / 117 / 127）。
//!
//! 职责边界：executor 是 DAG 执行引擎，不承担定时任务；超时、冲突恢复、依赖、准入、
//! 提醒都归 scheduler 的 10s tick。**`tick()` 是手动驱动接缝**（决策 143 接缝④）——
//! 测试直接调用它，生产由 [`KanbanScheduler::run_loop`] 按 `tick_interval_sec` 驱动。

use std::collections::HashSet;
use std::sync::Arc;

use chrono::Duration;

use crate::clock::Clock;
use crate::config::{
    effective_idle_timeout, effective_max_duration, factory_stage_max_duration, node_timeouts,
    Settings,
};
use crate::process::ProcessKiller;
use crate::sse::{SseEvent, SseSink};
use crate::storage::attention::AttentionKind;
use crate::storage::observability::is_timed_out;
use crate::storage::tasks::DependencyState;
use crate::storage::Store;
use crate::types::{
    NodeRun, NodeStatus, PendingContext, PendingKind, PendingReason, ResumeCause, TaskStatus,
};
use crate::Result;

// 超时自动续接的次数上限（决策 320）随分档一起搬进 `pipeline::retry`（决策 356 · 票 02）：
// 梯子是纯函数 [`crate::pipeline::retry::timeout_retry`]，这里只做动作。

/// 判超时后等旧执行体自然退出的上限（决策 320，**写死不配**）：正常收口是毫秒级的
/// 几笔库写，界给足；卡在不返回的同步调用里的到点即走 303 的兜底（代价不新增）。
const EXECUTOR_TEARDOWN_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// resume 钩子：scheduler 通过它拉起 executor，不直接依赖 executor 实现。
pub type ResumeFn = Arc<dyn Fn(&str) + Send + Sync>;

/// 一次 tick 的产出（测试逐项断言六项职责）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TickReport {
    /// 判定超时并处理的 run id。
    pub timed_out_runs: Vec<i64>,
    /// 判定超时但**只收终态**的陈旧 run id（票 02①）。
    ///
    /// 这些行的尸检结论（`timeout`）照记，但它们不是「当前这一轮」——同一个游标上已经有
    /// 更新的一轮在跑。不碰执行权、不推动游标、不算梯子。
    pub stale_timeout_runs: Vec<i64>,
    /// 因超时被置 pending 的游标 id。
    pub timeout_pending_cursors: Vec<String>,
    /// 冲突恢复后放行的任务 id。
    pub conflict_resumed: Vec<String>,
    /// 依赖满足后置 queued 的任务 id。
    pub dependencies_promoted: Vec<String>,
    /// 依赖重启后从 pending 退回 waiting 的任务 id。
    pub dependencies_recovered: Vec<String>,
    /// 依赖失败被挂 pending 的任务 id。
    pub dependency_failed: Vec<String>,
    /// 本 tick 准入的任务 id。
    pub admitted: Vec<String>,
    /// 被标记 stalled 的任务 id。
    pub stalled: Vec<String>,
    /// 发出重复提醒的任务 id。
    pub reminded: Vec<String>,
    /// 本 tick 被判定「心跳已停」并标终态的项目级 run id（决策 212 / 票 13）。
    pub abandoned_project_runs: Vec<i64>,
    /// 本 tick **新写进待办表**的事件条数（决策 209③ / 票 05）。
    ///
    /// 与 `reminded` 的分工：后者是本 tick 的日志口径（内存、重启即失），前者才是
    /// 「有人看得见」的那一份。两者都留，是为了让「发现」与「推送」在测试里分得开。
    ///
    /// 两个生产者：待办那一段（②–⑥，本 tick 的常规发现）与自适应慢跑告警（它落在
    /// 超时那一段里，见 `maybe_alert_slow_run`），故计数是累加而不是赋值。
    pub attention_noted: usize,
    /// 自适应超时的「运行显著偏慢」告警（决策 66 / 票 17）：`(task_id, 描述)`。
    ///
    /// **纯告警**：不影响任何超时判定（自适应值不作强制阈值，决策 66 边界）。
    /// `adaptive_timeout_enabled = false` 时该列表恒为空。
    pub slow_run_alerts: Vec<(String, String)>,
}

pub struct KanbanScheduler {
    store: Store,
    settings: Settings,
    clock: Arc<dyn Clock>,
    killer: Arc<dyn ProcessKiller>,
    sse: Arc<dyn SseSink>,
    resume: ResumeFn,
    /// 已提醒过的任务（提醒只重复一次，不刷屏，决策 55）。
    reminded: HashSet<String>,
}

impl KanbanScheduler {
    pub fn new(
        store: Store,
        settings: Settings,
        clock: Arc<dyn Clock>,
        killer: Arc<dyn ProcessKiller>,
        sse: Arc<dyn SseSink>,
        resume: ResumeFn,
    ) -> Self {
        KanbanScheduler {
            store,
            settings,
            clock,
            killer,
            sse,
            resume,
            reminded: HashSet::new(),
        }
    }

    /// 10s 周期循环（生产）；`shutdown` 置位后停止派发新任务（决策 54）。
    pub async fn run_loop(
        mut self,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Result<()> {
        let interval = std::time::Duration::from_secs(self.settings.tick_interval_sec);
        loop {
            tokio::select! {
                _ = tokio::time::sleep(interval) => {
                    match self.tick().await {
                        Ok(report) => {
                            // 提醒只重复一次，避免刷屏（决策 55）
                            self.mark_reminded(&report.reminded);
                        }
                        Err(e) => tracing::error!(error = %e, "scheduler tick 失败"),
                    }
                }
                _ = shutdown.changed() => {
                    tracing::info!("scheduler 收到停机信号，停止派发新任务");
                    return Ok(());
                }
            }
        }
    }

    /// 一次 tick：六项职责（决策 55）+ 发现落表（决策 209③，票 05）。
    ///
    /// 落表排在最后：它读的是**这一 tick 处置完之后**的态势（超时已处置、pending 已挂上），
    /// 而不是处置之前的。它本身不改任何状态——发现与唤醒是两件事，唤醒归值守轮。
    pub async fn tick(&self) -> Result<TickReport> {
        let mut report = TickReport::default();
        self.check_timeouts(&mut report).await?;
        self.resume_conflict_waits(&mut report).await?;
        self.check_waiting_tasks(&mut report).await?;
        self.recover_dependency_failed(&mut report).await?;
        self.admit_pending_tasks(&mut report).await?;
        self.remind_pending_tasks(&mut report).await?;
        self.abandon_stale_project_runs(&mut report).await?;
        self.note_discoveries(&mut report).await?;
        Ok(report)
    }

    /// 小时级维护：会话清理 + 指标聚合（决策 55）。
    pub async fn maintenance(&self) -> Result<MaintenanceReport> {
        let purged = self.purge_expired_conversations().await?;
        // 值班长的**对话消息永久保留**（票 04，显式修订决策 182④「对讲台与全仓同一把
        // 保留期尺」与决策 204⑦「归档不保护消息」——消息表豁免之后，归档与否、超龄与否
        // 都不再删它）：`kanban_foreman_messages` 退出按 `conversation_retention_days` 的
        // 年龄清理。下面的提议 / 待办 / 任务会话与 worktree 回收照旧吃这同一个 cutoff，
        // 一张表都不豁免——维护作业只摘了消息这一张表。
        let cutoff =
            self.clock.now() - Duration::days(self.settings.conversation_retention_days as i64);
        // 节点内消息日志与任务会话**同一把尺子**（票 01）：同一个 cutoff、同一个「任务终态 +
        // 超龄」判据，不造第二把尺（决策 182④ / 204⑦ 已经吃过两把尺的亏）。**判据不许换成
        // 「run 判终态」**——启动恢复第一件事就是把被 kill 的 run 标终态，那正是最需要日志的
        // 时刻。走维护连接（决策 321：与上面那条 DELETE 同类，重负载日会把主池拖进慢语句）。
        let purged_node_messages = {
            let mut conn = self.store.maintenance_connection().await?;
            let n = sqlx::query(
                "DELETE FROM kanban_node_messages
                 WHERE task_id IN (SELECT id FROM kanban_tasks
                                   WHERE status IN ('done','failed','cancelled'))
                   AND created_at < ?",
            )
            .bind(crate::storage::ts(cutoff))
            .execute(&mut conn)
            .await?
            .rows_affected();
            n as usize
        };
        // 提议两件事分开做（决策 207）：**过期清扫**改状态、留行；**年龄清理**删行。
        // 合成一步就会让「过期只让按钮变灰、那一轮留在时间线」这条规则在维护作业里失效。
        let expired_proposals = self
            .store
            .expire_foreman_proposals(self.clock.now())
            .await?;
        // 修复 worktree 的回收（决策 212③ / 票 12）：**必须排在删行之前**——`args.project_id`
        // 与载荷是唯一知道那个目录属于谁、在哪个仓里的东西，行一删就成了无主残留。
        let recycled_repairs =
            crate::pipeline::repair::recycle_unpressed_repair_worktrees(&self.store, cutoff)
                .await?;
        let purged_proposals = self.store.purge_foreman_proposals(cutoff).await?;
        // 待办表与其余各表**同一口径**的年龄清理（票 05）：同一个 cutoff，不做第二把尺。
        let purged_attention = self.store.purge_attention(cutoff).await?;
        let aggregated = self.aggregate_node_metrics().await?;
        // 决策 321：清理走完即收缩 WAL 并打存储水位——WAL 无界增长与磁盘逼近满
        // 都是慢写的上游信号，维护作业是它们的自然观测点。
        let checkpoint = self.store.checkpoint_wal().await?;
        tracing::info!(
            purged_conversations = purged,
            purged_node_messages,
            wal_busy = checkpoint.busy,
            wal_bytes_before = checkpoint.wal_bytes_before,
            wal_bytes_after = checkpoint.wal_bytes_after,
            disk_free_bytes = checkpoint.disk_free_bytes,
            "小时级维护收口：保留期清理 + WAL checkpoint + 水位"
        );
        Ok(MaintenanceReport {
            purged_conversations: purged,
            purged_node_messages,
            expired_foreman_proposals: expired_proposals,
            purged_foreman_proposals: purged_proposals,
            recycled_repair_worktrees: recycled_repairs,
            purged_attention: purged_attention as usize,
            aggregated_tasks: aggregated,
        })
    }

    // ─────────────────────── ① 超时检测（决策 64 / 66 / 100 / 122）───────────────────────

    async fn check_timeouts(&self, report: &mut TickReport) -> Result<()> {
        let now = self.clock.now();
        let stage_configs = self.store.list_stage_configs().await?;
        // 历史 run 全量取一次，供自适应告警复用（票 17）——不要在每个 run 上重扫。
        let history = if self.settings.adaptive_timeout_enabled {
            Some(self.store.all_runs().await?)
        } else {
            None
        };

        // 在飞的 run 取一次：判「谁才是当前这一轮」要拿同一份快照比对（票 02①），
        // 逐个重查会让同一次 tick 里的两次读数来自不同时刻。
        let active = self.store.active_runs().await?;
        // 每个**游标**上最新的那一条在飞 run——游标就是「节点某一轮」的身份（重试沿用同一个
        // cursor_id），故同一个游标上 id 更大的一定是更新的那一轮。
        let mut newest_per_cursor: std::collections::HashMap<String, i64> =
            std::collections::HashMap::new();
        for r in &active {
            if let Some(cid) = r.cursor_id.as_deref() {
                let entry = newest_per_cursor.entry(cid.to_string()).or_insert(r.id);
                if r.id > *entry {
                    *entry = r.id;
                }
            }
        }
        for run in active {
            // 项目级伪阶段 run（票 10）无任务 / 游标，不属于节点超时语义，跳过
            // （其生命周期由 analyze 端点收尾，不以 executor 超时处置）。
            if run.task_id.is_none() || run.cursor_id.is_none() {
                continue;
            }
            // 子代理 run（票 08）挂着父节点的 task + cursor，但它**不是节点自身的执行**
            // ——它是父节点正在进行的工具调用。若按节点 run 处置，父节点还在正常干活时
            // 就会被判超时、写一条节点级重试 transition，等于把一次并行检索变成父节点的
            // 伪超时。超时语义只属于节点自身（与票 14 对 `attempt` 的口径同一处）。
            if !crate::metrics::is_node_owning_run(&run.agent_type) {
                continue;
            }
            let stage_cfg = stage_configs.iter().find(|c| c.stage == run.stage.as_str());
            let node_override = node_timeouts(stage_cfg, run.node.as_str());
            let idle = effective_idle_timeout(
                self.settings.node_idle_timeout_sec,
                stage_cfg.and_then(|c| c.idle_timeout_sec),
                node_override,
            );
            // 出厂阶段默认插在「DB 阶段覆盖」与「全局」之间（long-run-budget 票 01）：
            // stage_configs 里显式配过的值压过它，三级优先序（决策 66）不变。
            let stage_max = stage_cfg
                .and_then(|c| c.max_duration_sec)
                .or_else(|| factory_stage_max_duration(run.stage.as_str()));
            let max_duration = effective_max_duration(
                self.settings.node_max_duration_sec,
                stage_max,
                node_override,
            );

            // 决策 66 / 票 17：自适应 P50/P90 **只用于告警**——在任何超时判定之前
            // 独立跑一遍，且不把分位数传给 `is_timed_out`。
            if let Some(history) = &history {
                self.maybe_alert_slow_run(&run, now, history, report)
                    .await?;
            }

            if is_timed_out(&run, now, idle, max_duration).is_none() {
                continue;
            }
            // 票 02①：**陈旧的 run 只收终态**。一个游标上已经有更新的一轮在跑，说明这一条
            // 是上一进程（或上一次重启）留下的尸检对象——它的心跳确实停了，结论照记，
            // 但它**不是当前这一轮**：`request_cancel` 是按任务键发的，掐掉的是那个活着
            // 的执行体；紧随其后的 `release_ownership` 还会把它的执行权一并摘掉。
            // 2026-09-30 那条链上 attempt 2 比 attempt 1 的判死**早 10 秒起跑**，正是这样
            // 被掐死的（run 105 落一条 cancelled，再把刚记上的超时清零——梯子因此从未升档）。
            let is_current = run
                .cursor_id
                .as_deref()
                .and_then(|cid| newest_per_cursor.get(cid))
                .is_some_and(|newest| *newest == run.id);
            if !is_current {
                report.stale_timeout_runs.push(run.id);
                tracing::warn!(
                    task = ?run.task_id,
                    run = run.id,
                    stage = %run.stage,
                    node = %run.node,
                    "陈旧 run 判超时：只收终态，不碰当前在跑的那一轮"
                );
                self.store
                    .finish_run(
                        run.id,
                        &crate::storage::observability::RunOutcome {
                            status: Some(NodeStatus::Timeout),
                            duration_ms: elapsed_ms(&run, now),
                            error: Some(format!(
                                "陈旧 run：{}（进程退出时留下的这一轮，判超时只收终态，不碰当前在跑的那一轮）",
                                timeout_detail(&run, "超时")
                            )),
                            ..Default::default()
                        },
                    )
                    .await?;
                continue;
            }
            report.timed_out_runs.push(run.id);
            self.handle_timeout(&run, report).await?;
        }
        Ok(())
    }

    /// 自适应超时告警（决策 66 / 票 17）：运行时长超过该节点历史 **3×P90** 时记一条告警。
    ///
    /// 采样走 [`crate::metrics::duration_percentiles`]（仅成功运行、窗口 20、最少 5 样本；
    /// 冷启动无数据不告警）。**绝不参与超时判定**——强制阈值始终取配置值。
    ///
    /// 告警同时**落一条待办**（`slow_run`，决策 209② 的事件清单）：它此前只写日志，
    /// 于是「值班长该知道这件事」在实际运行里没有出口。这一类是**只播报不唤醒**的
    /// （`AttentionKind::wakes` 唯一返回 `false` 的那个）——它没有可操作的动作，够不上
    /// 半夜把人叫醒；下次为别的事醒来时，它在那份简报里。
    async fn maybe_alert_slow_run(
        &self,
        run: &NodeRun,
        now: chrono::DateTime<chrono::Utc>,
        history: &[NodeRun],
        report: &mut TickReport,
    ) -> Result<()> {
        let Some(task_id) = run.task_id.clone() else {
            return Ok(());
        };
        let Some(p) = crate::metrics::duration_percentiles(history, run.stage, run.node) else {
            return Ok(()); // 冷启动 / 样本不足：不展示、不告警
        };
        let elapsed = elapsed_ms(run, now);
        if !crate::metrics::should_alert_slow(elapsed, p) {
            return Ok(());
        }
        let detail = format!(
            "{}.{} 已运行 {}ms，超过该节点 P90（{}ms）的 3 倍（P50 {}ms，样本 {}）",
            run.stage, run.node, elapsed, p.p90_ms, p.p50_ms, p.samples
        );
        tracing::warn!(task = %task_id, run = run.id, "{detail}");
        // `occurred_at` 取**这条 run 的开始时刻**，不是 `now`：tick 每 10s 一次，用 `now`
        // 会让同一条慢跑每 tick 都落一行新待办（唯一键是 (task, kind, occurred_at)）。
        // 一条 run 一行，语义也正是「这一轮慢得不正常」，重跑一轮就是新的一行。
        if self
            .store
            .note_attention(
                &task_id,
                AttentionKind::SlowRun,
                run.started_at,
                Some(&serde_json::json!({
                    "run_id": run.id,
                    "stage": run.stage.as_str(),
                    "node": run.node.as_str(),
                    "elapsed_ms": elapsed,
                    "p50_ms": p.p50_ms,
                    "p90_ms": p.p90_ms,
                    "samples": p.samples,
                })),
            )
            .await?
        {
            report.attention_noted += 1;
        }
        report.slow_run_alerts.push((task_id, detail));
        Ok(())
    }

    /// 超时处理：杀进程组 / 通知执行体收口 → 按连续超时轮数分流（决策 320）：
    /// 1–2 次自动续接上一轮转录 → 第 3 次空白重跑一次（票 04 起改带简报，不带全卷转录）
    /// → 第 4 次起 pending(timeout) 交回人工。
    async fn handle_timeout(&self, run: &NodeRun, report: &mut TickReport) -> Result<()> {
        // 调用方已跳过项目级 run；这里再兜一层，避免无任务 / 游标时误用空值。
        let (Some(task_id), Some(cursor_id)) = (run.task_id.as_deref(), run.cursor_id.as_deref())
        else {
            return Ok(());
        };
        // 决策 66：杀整个进程组（测试里由记录型终止器断言）
        if let Some(pgid) = run.process_group_id {
            self.killer.kill_process_group(pgid)?;
        }
        self.store
            .finish_run(
                run.id,
                &crate::storage::observability::RunOutcome {
                    status: Some(NodeStatus::Timeout),
                    // 决策 226：**时长照实记**。此前这里留 0，而「跑了 8 小时 51 分」这种
                    // 判读只能由读的人拿 started_at 自己算——值班长 2026-09-19 就是这么
                    // 算的，同时台账里摆着的 duration_ms 是 0。
                    duration_ms: elapsed_ms(run, self.clock.now()),
                    error: Some(timeout_detail(run, "超时")),
                    ..Default::default()
                },
            )
            .await?;
        // 决策 226：**判超时要真的把 run 停下来**。上面那一刀只对「起过子进程」的 run 有效
        // （`process_group_id` 只有 `run_command` 回填过），而实测卡死的恰好是另一种：停在
        // 不返回的模型调用上的 run 根本没有进程组可杀。它在下一个 await 点按这一句收口，
        // 去重与 `executor_owner` 随之释放；缺了这一句，紧随其后的 resume 会被那个已判死的
        // 执行体逐次拒掉直到钩子放弃——2026-09-19 实测 run 23 被判超时后又活了 8 小时以上，
        // 任务就此僵死到有人按 `unstick`。
        //
        // **顺序是承重的：这一句必须在 `finish_run` 之后。** 收口是异步的，而执行体收口时
        // 会调 `record_run_usage` 把「已经烧掉的 token」补进这一行；而 `finish_run` 是按值
        // 整段写入（`RunOutcome::default()` 的 token 是 0）。先通知就先被覆盖——那正是这条
        // 要修的那类「读数看起来像真值，其实是占位符」。先用例 `a_timed_out_run_is_stopped_
        // and_reports_its_usage` 也是这个顺序（判超时写入在前、通知在后）。
        //
        // **决策 303（显式修订决策 226 的一格）：判终态的这一处放开执行权的两半**，与那个
        // `run_inner` 会不会返回**无关**。226 原来在这里只发中止请求、不摘去重登记，前提是
        // 「执行体会在下一个 await 点自己收口」——2026-09-27 实测那个前提不成立：执行体停在
        // 一次不返回的同步文件读里（`open()` 挂在完全磁盘访问的授权弹窗上），既到不了 await
        // 点也返回不了，于是进程内去重与 `executor_owner` 双双占死，紧随其后的 resume 被逐次
        // 拒掉 30 秒后放弃、任务僵死 2.5 小时，只能靠重启——正是这条要停掉的那件事。
        // **先只发中止请求、不摘登记，有界等旧执行体自然退出**（决策 320 的顺序保证）：
        // 转录落库（`record_failed_attempt`）排在它退出之前，而超时续接的读
        // （`take_continuation`）排在新执行体起来之后——中间不隔这一等，读就可能抢在写
        // 前面，那一轮续接静默退化成空白起跑（原因列读-清一次，被白白消费掉）。
        // 等不到的（卡在不返回的同步调用里，303 的现场）到点为止，下面照旧兜底。
        let had_executor = crate::pipeline::executor::request_cancel(task_id);
        if had_executor {
            crate::pipeline::executor::await_teardown(task_id, EXECUTOR_TEARDOWN_WAIT).await;
        } else {
            // 取不到登记：进程内没有这一号执行体（例如本进程刚重启，台账里留着上一进程的
            // running run）。此时既没有通道可通知，也没有人要等——照旧往下走。
            tracing::debug!(
                task = %task_id,
                run = run.id,
                "超时处置：进程内没有在跑的执行体可通知"
            );
        }
        // 摘登记 + 清 `executor_owner`（幂等：自然退出的场合两样都已空；真卡住的场合
        // 这里就是 303 的兜底）。
        crate::pipeline::executor::release_ownership(&self.store, task_id).await?;

        // 决策 320（显式修订决策 298 的超时支：重跑 → 续接；决策 33 的「干净对话重试」
        // 在这一支退居降级档）：超时重试的形态由该节点**连续超时的轮数**决定——
        // 连续 1–2 次：**自动续接**。游标记上续接原因（`mark_cursor_continuation`），
        // 重试起的 run 经 `take_continuation`（决策 180 / 205 的既有机制，与手动
        // 「继续」同路）带上一轮转录，run 链上落 `continued_from_run_id`。上一轮的
        // 转录已经落库（失败 attempt 照记会话行，含被中止的那一轮），续接的是真实进度。
        // 纯代码节点没有转录，这两档退化为**空白重跑**（不置标记），梯子计数照走。
        // 连续第 3 次：降级**空白重跑**一次——续接救了两轮都没救回来，多半不是
        // 「丢了上下文」，续第四遍只是继续烧钱。
        // 连续第 4 次起：挂起 pending(timeout) 交回人工。
        // 非超时类的失败重试（决策 278 / 298）在 `model_invoke` 的轮内循环里，形态不变。
        // 计数在 `finish_run` 之后取：刚判超时的这一条已经落库，算进连续序列里。
        let streak = self
            .store
            .trailing_timeout_streak(task_id, run.stage, run.node)
            .await?;
        // 续接标记只对 **agent 节点**有意义：`take_continuation` 只在 agent 节点入口读
        // （纯代码节点没有转录可言）。对纯代码节点置位会让标记永远无人取走、悬在列上。
        // **但梯子本身对纯代码节点照样走**（决策 320 按「该节点连续超时的轮数」计数，
        // 不分节点种类）：没有转录可续时前两档退化为空白重跑，第 4 次才挂起——
        // 第一次超时就交回人工，比旧口径 `run.attempt < agent_retry_max` 耗尽才挂还倒退。
        //
        // **分档本身是纯函数**（决策 356 · 票 02）：
        // [`crate::pipeline::retry::timeout_retry`] 给出这一档的动作，这里只做动作
        // （标续接 / 写流转 / 挂起）。
        let is_agent_node =
            crate::pipeline::model_invoke::AgentNodeKind::of(run.stage, run.node).is_some();
        match crate::pipeline::retry::timeout_retry(streak) {
            crate::pipeline::retry::TimeoutRetry::AutoContinue => {
                if is_agent_node {
                    self.store
                        .mark_cursor_continuation(cursor_id, ResumeCause::Timeout)
                        .await?;
                    self.store
                        .insert_transition(
                            task_id,
                            &branch_of(cursor_id, &self.store).await?,
                            Some((run.stage, run.node)),
                            (run.stage, run.node),
                            crate::types::TransitionTrigger::Timeout,
                            Some(&format!(
                                "节点超时，自动续接上一轮转录（连续第 {streak} 次超时）"
                            )),
                        )
                        .await?;
                } else {
                    self.store
                        .insert_transition(
                            task_id,
                            &branch_of(cursor_id, &self.store).await?,
                            Some((run.stage, run.node)),
                            (run.stage, run.node),
                            crate::types::TransitionTrigger::Timeout,
                            Some(&format!(
                            "节点超时，自动重跑（纯代码节点无转录可续，连续第 {streak} 次超时）"
                        )),
                        )
                        .await?;
                }
                (self.resume)(task_id);
            }
            crate::pipeline::retry::TimeoutRetry::BlankRestart => {
                // 空白重跑档：不带转录重起一段对话（决策 33 的原语义），给节点最后一次
                // 自己走完的机会。transition 文案明说降级，复盘时不必倒推为什么没续接。
                //
                // 票 04（决策 376 裁决②）：**空白**不再等于「从零开始」——续接原因列置
                // `TimeoutBlankRestart`，起跑那一轮据此渲染一份简报（任务描述 + 阶段产物
                // 文件清单 + 未提交改动清单 + 最近收口摘要），替掉全卷转录。置位只对
                // agent 节点有效（纯代码节点不读续接素材，标记会悬在列上；梯子计数照走）。
                if is_agent_node {
                    self.store
                        .mark_cursor_continuation(cursor_id, ResumeCause::TimeoutBlankRestart)
                        .await?;
                }
                self.store
                    .insert_transition(
                        task_id,
                        &branch_of(cursor_id, &self.store).await?,
                        Some((run.stage, run.node)),
                        (run.stage, run.node),
                        crate::types::TransitionTrigger::Timeout,
                        Some(
                            "节点超时，续接两轮未恢复，空白重跑一次（改带简报起跑，不带全卷转录）",
                        ),
                    )
                    .await?;
                (self.resume)(task_id);
            }
            crate::pipeline::retry::TimeoutRetry::Pending => {
                // 耗尽：pending 挂在该 run 所属的**游标**上（决策 82）。
                // 落库走 `advance` 那扇门（决策 245）；同步投影与事件留在门外——门不发 SSE。
                // 注意 reason 里的 stage/node 取自**run**而不是游标，所以 `Landing::Pause`
                // 整条带走 `PendingReason`，不按游标重算。
                let cursor = self.store.get_cursor(cursor_id).await?;
                crate::pipeline::advance(
                    &self.store,
                    task_id,
                    &cursor,
                    crate::pipeline::Landing::Pause {
                        reason: PendingReason::new(
                            PendingKind::Timeout,
                            run.stage,
                            run.node,
                            format!(
                                "{}（attempt {}）",
                                timeout_detail(run, "执行超时"),
                                run.attempt
                            ),
                        ),
                    },
                    // 挂起不写流转行，这个 trigger 不会被读到（门的签名对各落点是同一个）。
                    crate::types::TransitionTrigger::Timeout,
                    None,
                )
                .await?;
                self.store.sync_task_projection(task_id).await?;
                report.timeout_pending_cursors.push(cursor_id.to_string());
                self.emit_pending(task_id, cursor_id).await?;
            }
        }
        Ok(())
    }

    // ─────────────────────── ② 冲突等待恢复（决策 102）───────────────────────

    async fn resume_conflict_waits(&self, report: &mut TickReport) -> Result<()> {
        let cursors = self
            .store
            .cursors_with_pending_kind(PendingKind::ConflictWait)
            .await?;
        for cursor in cursors {
            let pending = cursor.pending_reason.clone().unwrap_or_else(|| {
                PendingReason::new(PendingKind::ConflictWait, cursor.stage, cursor.node, "")
            });
            let conflict_ids = pending
                .context
                .as_ref()
                .map(|c| c.conflict_task_ids.clone())
                .unwrap_or_default();

            // 必须**全部**终态才考虑恢复
            let mut all_terminal = true;
            for id in &conflict_ids {
                match self.store.get_task(id).await {
                    Ok(task) if task.status.is_terminal() => {}
                    _ => all_terminal = false,
                }
            }
            if !all_terminal {
                continue;
            }

            // 恢复前重跑第一层比对：仍有交集则保持 pending 并更新 id 列表（不重跑节点）
            let still = self.store.recheck_first_layer_overlap(&cursor).await?;
            if still.is_empty() {
                self.store.clear_cursor_pending(&cursor.cursor_id).await?;
                self.store.sync_task_projection(&cursor.task_id).await?;
                report.conflict_resumed.push(cursor.task_id.clone());
                (self.resume)(&cursor.task_id);
            } else {
                self.store
                    .update_cursor_pending_context(&cursor.cursor_id, "conflict_task_ids", &still)
                    .await?;
                self.sse.emit(SseEvent::PendingUpdated {
                    task_id: cursor.task_id.clone(),
                    branch: cursor.branch.clone(),
                    cursor_id: cursor.cursor_id.clone(),
                    context: PendingContext {
                        conflict_task_ids: still,
                        ..Default::default()
                    },
                });
            }
        }
        Ok(())
    }

    // ─────────────────────── ③④ 依赖（决策 57 / 116）───────────────────────

    async fn check_waiting_tasks(&self, report: &mut TickReport) -> Result<()> {
        for task in self.store.waiting_tasks().await? {
            match self.store.dependencies_satisfied(&task.id).await? {
                DependencyState::Ready => {
                    self.store
                        .set_task_status(&task.id, TaskStatus::Queued)
                        .await?;
                    report.dependencies_promoted.push(task.id);
                }
                DependencyState::Failed(failed) => {
                    // 依赖失败：pending(dependency_failed) 挂在 main 游标上（决策 90 / 116）
                    let cursor = self.store.resolve_sole_cursor(&task.id).await?.or(self
                        .store
                        .load_live_cursors(&task.id)
                        .await?
                        .into_iter()
                        .next());
                    if let Some(cursor) = cursor {
                        let all_cancelled = self.all_dependencies_cancelled(&task.id).await?;
                        let reason = PendingReason::new(
                            PendingKind::DependencyFailed,
                            cursor.stage,
                            cursor.node,
                            format!("依赖任务失败：{}", failed.join("、")),
                        )
                        .with_context(PendingContext::with_kind(if all_cancelled {
                            crate::actions::kinds::DEPENDENCY_CANCELLED
                        } else {
                            crate::actions::kinds::DEPENDENCY_FAILED
                        }));
                        self.store
                            .set_cursor_pending(&cursor.cursor_id, &reason)
                            .await?;
                        self.store.sync_task_projection(&task.id).await?;
                        report.dependency_failed.push(task.id.clone());
                        self.emit_pending(&task.id, &cursor.cursor_id).await?;
                    }
                }
                DependencyState::Waiting(_) => {}
            }
        }
        Ok(())
    }

    async fn all_dependencies_cancelled(&self, task_id: &str) -> Result<bool> {
        let deps = self.store.dependencies_of(task_id).await?;
        if deps.is_empty() {
            return Ok(false);
        }
        for dep in deps {
            match self.store.get_task(&dep).await {
                Ok(task) if task.status == TaskStatus::Cancelled => {}
                _ => return Ok(false),
            }
        }
        Ok(true)
    }

    async fn recover_dependency_failed(&self, report: &mut TickReport) -> Result<()> {
        report.dependencies_recovered = self.store.recover_dependency_failed().await?;
        Ok(())
    }

    // ─────────────────────── ⑤ 并发准入（决策 117 / 98）───────────────────────

    async fn admit_pending_tasks(&self, report: &mut TickReport) -> Result<()> {
        let max = self.settings.max_concurrent_tasks;
        let mut occupying = self.store.occupying_slots().await?;
        for task in self.store.queued_tasks().await? {
            if occupying >= max {
                break;
            }
            if self.store.try_admit(&task.id, max).await? {
                occupying += 1;
                report.admitted.push(task.id.clone());
                self.store
                    .insert_transition(
                        &task.id,
                        crate::types::NodeCursor::BRANCH_MAIN,
                        None,
                        (task.current_stage, task.current_node),
                        crate::types::TransitionTrigger::Start,
                        Some("并发准入放行"),
                    )
                    .await?;
                (self.resume)(&task.id);
            }
        }
        Ok(())
    }

    // ─────────────────────── ⑥ 提醒与 stalled（决策 34 / 92）───────────────────────

    async fn remind_pending_tasks(&self, report: &mut TickReport) -> Result<()> {
        let now = self.clock.now();
        let reminder_after = Duration::hours(self.settings.pending_reminder_hours as i64);
        let stalled_after = Duration::hours(self.settings.pending_timeout_hours as i64);

        for task in self
            .store
            .list_tasks(&crate::storage::tasks::TaskFilter {
                include_archived: false,
                ..Default::default()
            })
            .await?
        {
            let live = self.store.load_live_cursors(&task.id).await?;
            // 决策 92：谓词是 has_runnable_cursor——一个分支 pending、另一分支在跑 ≠ 卡住
            let stalled_candidate = crate::pipeline::cursor::has_pending_cursor(&live)
                && !crate::pipeline::cursor::has_runnable_cursor(&live);
            if !stalled_candidate {
                continue;
            }
            // 决策 276：**人自己按下的暂停不在这里**。停滞提醒与 `stalled` 标记服务的是
            // 「没人管的 pending」，而手动暂停恰恰是有人在管——给按住的任务挂一个「停滞 24h」
            // 的红标、还把值班长叫起来，等于用噪声回报一次明确的人工操作。
            if crate::pipeline::cursor::all_pending_are_human_holds(&live) {
                continue;
            }
            let Some(since) = live
                .iter()
                .filter(|c| c.is_pending())
                .map(|c| c.updated_at)
                .min()
            else {
                continue;
            };
            let age = now - since;

            if age > stalled_after && !task.stalled {
                self.store.set_stalled(&task.id, true).await?;
                report.stalled.push(task.id.clone());
            }
            // pending 超时提醒：只重复一次
            if age > reminder_after && !self.reminded.contains(&task.id) {
                report.reminded.push(task.id.clone());
                // 票 05 顺手修掉的既有缺陷：提醒此前**从不推 SSE**，只在内存集合里记一笔
                // ——「任务停滞超过 24h」这件事前端永远不知道。走既有 `stalled` 事件
                // （前端按它置看板高亮，不弹 toast：决策 65 的弹窗只给 pending/done/failed）。
                self.sse.emit(SseEvent::Stalled {
                    task_id: task.id.clone(),
                    branch: crate::types::NodeCursor::BRANCH_MAIN.to_string(),
                    pending_hours: age.num_hours().max(0) as u64,
                });
                // 表才是通道（票 05）：一个停了 24h 还没人管的 pending，**必须**能唤醒
                // 值守轮——上面那条「只报新鲜事」的窗口恰恰会漏掉它（事件发生在 24h 前）。
                if self
                    .store
                    .note_attention(
                        &task.id,
                        AttentionKind::TaskStale,
                        now,
                        Some(&serde_json::json!({
                            "pending_hours": age.num_hours().max(0),
                            "since": since.to_rfc3339(),
                        })),
                    )
                    .await?
                {
                    report.attention_noted += 1;
                }
            }
        }

        // reminded 是 &self 上的集合；用 interior mutability 语义上更重，这里改为
        // 每次 tick 由调用方决定是否复用（生产循环里 scheduler 是 mut 持有的）。
        Ok(())
    }

    /// 记录已提醒任务（生产循环在 tick 后调用，避免重复提醒刷屏）。
    pub fn mark_reminded(&mut self, task_ids: &[String]) {
        self.reminded.extend(task_ids.iter().cloned());
    }

    pub fn reminded_count(&self) -> usize {
        self.reminded.len()
    }

    // ────────────── ⑦ 项目级 run 的终止者（决策 212 / 票 13）──────────────

    /// 给项目级 run 一个明确的终止者。
    ///
    /// **这不是「加观测」，是修调度器的洞**：`check_timeouts` 有意跳过它们
    /// （没有任务 / 游标，不属节点超时语义），而 `requeue_running_tasks` 也不认它们
    /// （那条路按 `task_id` 归队）——两条路都不管的后果是它们**跨重启永生**。
    /// 2026-09-17 的实证：三条 `pseudo:project_analysis` run 的 `last_activity_at` 冻结在
    /// 某个时刻、仍是 `running`，更早的一对还一起活了 4 小时 24 分。
    ///
    /// 判据只有一条：仍是 `running` 且**心跳停了**超过 `project_run_idle_timeout_sec`。
    /// 标 `Timeout` 并写一句可读原因——活的那些（正在跑 LLM）心跳会刷新，不会误伤。
    async fn abandon_stale_project_runs(&self, report: &mut TickReport) -> Result<()> {
        let cutoff =
            self.clock.now() - Duration::seconds(self.settings.project_run_idle_timeout_sec as i64);
        for run in self.store.stale_project_runs(cutoff).await? {
            let idle = self
                .clock
                .now()
                .signed_duration_since(run.last_activity_at.unwrap_or(run.started_at))
                .num_seconds()
                .max(0);
            self.store
                .finish_run(
                    run.id,
                    &crate::storage::observability::RunOutcome {
                        status: Some(NodeStatus::Timeout),
                        error: Some(format!(
                            "项目级 run（{}）心跳停止 {idle}s，判定中断并标终态：\
                             analyze 端点那条收尾路径在进程被杀 / 重启时跑不到",
                            run.agent_type
                        )),
                        ..Default::default()
                    },
                )
                .await?;
            report.abandoned_project_runs.push(run.id);
        }
        Ok(())
    }

    // ─────────────────── ⑦ 发现落表（决策 209③，票 05）───────────────────

    /// 把本 tick 的发现写进**值班长待办**表（一事件一行）。
    ///
    /// 三条纪律：
    /// 1. **只写「需要有人管」的**（§2.1）——节点成功、每轮心跳、每次工具调用都不写。
    /// 2. **`occurred_at` 取事件发生的时刻**，不是「现在」：去重键含它，而这一句每 10 秒
    ///    跑一次；取「现在」等于给同一次 pending 每 tick 写一行，唤醒会被自己的重试刷屏。
    /// 3. **只报新鲜事**（`watch_event_window_minutes`）：三天前就 pending 的任务不该在
    ///    每次重启后再喊一遍。
    ///
    /// 它**不唤醒任何东西**——唤醒归值守轮（票 06），那一步要花钱。
    async fn note_discoveries(&self, report: &mut TickReport) -> Result<()> {
        let now = self.clock.now();
        let window = Duration::minutes(self.settings.watch_event_window_minutes as i64);
        let stuck = Duration::minutes(self.settings.watch_owner_stuck_minutes as i64);
        let mut noted = 0usize;

        for task in self
            .store
            .list_tasks(&crate::storage::tasks::TaskFilter {
                include_archived: false,
                ..Default::default()
            })
            .await?
        {
            // ① 任务转 pending（含重试耗尽 / 上下文溢出两个子类）
            //
            // 决策 276：**人自己按下的暂停不算「新鲜事」**——待办表服务的是「值班长该看一眼
            // 什么」，而按住任务的人已经在看着它了。跳过整块（含下面的重复计数 ②）：把一次
            // 明确的人工操作记成一条待办，只会让第二天早上那份简报多一条假信号。
            if let Some(reason) = &task.pending_reason {
                let live = self.store.load_live_cursors(&task.id).await?;
                // 决策 276：**人自己按住的任务整条跳过本 tick 的发现**（① 与 ② 是它的家，
                // 后面 ③–⑦ 对一条 pending 的任务本就都不成立：它们要么要求 `running`、要么
                // 是「新鲜事」那几条——跳过它们是同一条判据的自然延伸，不是漏读）。
                if crate::pipeline::cursor::all_pending_are_human_holds(&live) {
                    continue;
                }
                let occurred = live
                    .iter()
                    .filter(|c| c.is_pending())
                    .map(|c| c.updated_at)
                    .min()
                    .filter(|t| now - *t <= window);
                if let Some(occurred) = occurred {
                    let kind = pending_attention_kind(reason.kind);
                    if self
                        .store
                        .note_attention(
                            &task.id,
                            kind,
                            occurred,
                            Some(&serde_json::json!({
                                "pending_kind": reason.kind.as_str(),
                                "stage": reason.stage.as_str(),
                                "node": reason.node.as_str(),
                                "message": reason.message,
                                "diagnostic": reason
                                    .context
                                    .as_ref()
                                    .and_then(|c| c.diagnostic.clone()),
                            })),
                        )
                        .await?
                    {
                        noted += 1;
                    }
                    // ② 同一任务在窗口内再次 pending：**自动修复没治好**（§4.9 的判据之一）。
                    //    按条数判而不是按「有没有未消费的行」判——去重键含 occurred_at，
                    //    两次真事件就是两行，这正是要数出来的东西。
                    let repeats = self
                        .store
                        .count_attention_since(&task.id, kind, now - window)
                        .await?;
                    if repeats > 1
                        && self
                            .store
                            .note_attention(
                                &task.id,
                                AttentionKind::RepeatedPending,
                                occurred,
                                Some(&serde_json::json!({"count": repeats})),
                            )
                            .await?
                    {
                        noted += 1;
                    }
                }
            }

            // ③ 闸门失败（票面点名 develop 闸门与 merge 的 gate_failure_kind，读的是同一处
            //    merge_result：develop 闸门失败也会落到那条记录上）
            //
            //    票 gate-failure-respam（决策 388）：`occurred_at` 取**事件发生**的时刻——
            //    merge_result 行写入 gate=Fail 那一刻（闸门评估落定 / kickback 过渡），
            //    不是 `task.updated_at`。复检中的任务每次工具调用都刷新 `updated_at`，
            //    拿它当去重键等于把同一份 gate=Fail 每轮巡扫都记成一条「新」失败
            //    （106 实测 11 分钟推 12+ 条内容全同的 PWA 通知）。merge 行在复检期间
            //    不动：再次巡到时键不变，唯一索引落不进第二行；闸门重新评估再次失败
            //    时行被重写、键自然换新——真失败照记照响（唤醒面 234 / 出机线 287 一字不动）。
            if now - task.updated_at <= window {
                if let Some((merge, settled_at)) = self.store.merge_stage_row(&task.id).await? {
                    if merge.gate == Some(crate::types::Gate::Fail)
                        && self
                            .store
                            .note_attention(
                                &task.id,
                                AttentionKind::GateFailure,
                                settled_at,
                                Some(&serde_json::json!({
                                    "gate_failure_kind": merge
                                        .gate_failure_kind
                                        .map(|k| k.as_str()),
                                    "gate_failures": merge.gate_failures,
                                    "output": merge.gate_failure_output,
                                })),
                            )
                            .await?
                    {
                        noted += 1;
                    }
                }
            }

            // ④ 任务转 done（收尾播报；同样是「新鲜事」才写）
            if task.status == TaskStatus::Done
                && now - task.updated_at <= window
                && self
                    .store
                    .note_attention(&task.id, AttentionKind::TaskDone, task.updated_at, None)
                    .await?
            {
                noted += 1;
            }

            // ⑦ run 落终态失败 / 超时，而任务**没有因此转 pending**（决策 234）。
            //
            //    判据必须带「任务没转 pending」这一半：任务自己转 pending 时 ①（task_pending）
            //    本来就会响，重复记等于给同一个故障写两条待办。而**重试型故障**（连着几次
            //    同形状失败、任务始终 running）只有这一条看得见——实测里它烧掉一千万
            //    prompt token 而值守一次没醒（`scheduler_no_effect` 要求 run 终态而游标仍
            //    active、`task_pending` 要求任务转 pending，两条都不成立）。
            //
            //    每条失败的 run 各记一行（`occurred_at` = 它收场那一刻，去重键天然按 run
            //    分得开）；「连着几条会不会连着唤醒」交给**既有的三重节流**收——同任务
            //    30 分钟冷却正是为它准备的（决策 234 点名要用例钉住这一条）。
            //    已收口（done / cancelled）的任务不再为历史失败报警：那是复盘，不是待办。
            if task.status == TaskStatus::Running && task.pending_reason.is_none() {
                for run in self.store.failed_runs_since(&task.id, now - window).await? {
                    let occurred = run.finished_at.unwrap_or(now);
                    if self
                        .store
                        .note_attention(
                            &task.id,
                            AttentionKind::RunFailed,
                            occurred,
                            Some(&serde_json::json!({
                                "run_id": run.id,
                                "stage": run.stage.as_str(),
                                "node": run.node.as_str(),
                                "attempt": run.attempt,
                                "status": run.status.as_str(),
                                "error": run.error,
                                "prompt_tokens": run.prompt_tokens,
                                "completion_tokens": run.completion_tokens,
                            })),
                        )
                        .await?
                    {
                        noted += 1;
                    }
                }
            }

            // ⑤ 调度器处置未生效 + ⑥ owner 持有超时（三类判据）：都要求「任务还在动」
            //    ——`running` **或** `pending`（决策 305 把这一格放宽）。
            //    实测（2026-09-17）：run 已被标 timeout、transition 也写了「干净对话重试」，
            //    但**没有 attempt-2 的 run 行**，任务从此停在 running——check_timeouts 只看
            //    active_runs()（run 已是终态就不再被扫），remind_pending_tasks 的 stalled
            //    判据也不成立。这条缝此前零信号。
            //
            //    **为什么要放 `Pending` 进来**（2026-09-27 那次的形状）：游标挂成 `pending`
            //    之后，`sync_task_projection` 会把任务投影成 `TaskStatus::Pending`——于是
            //    只看 `Running` 的话，第三类证据（执行权持有超阈值）在它本该兜的那一格
            //    永远不会被问到，任务只能靠重启（实测僵死 2.5 小时）。
            //    任务的状态是**游标的投影**，而「执行权还攥在谁手上」是另一件事：
            //    `stuck_evidence` 自己按「有主 + 持有超阈值」判，不受投影影响。
            if matches!(task.status, TaskStatus::Running | TaskStatus::Pending) {
                // 判据只有一处（`pipeline::unstick::stuck_evidence`）：报出来的卡住与解得开的
                // 卡住必须是同一个集合，否则「它说卡了、我却解不开」迟早发生（票 09）。
                if let Some(evidence) =
                    crate::pipeline::unstick::stuck_evidence(&self.store, &task, now, stuck).await?
                {
                    if self
                        .store
                        .note_attention(
                            &task.id,
                            evidence.kind,
                            evidence.occurred_at,
                            Some(&evidence.detail),
                        )
                        .await?
                    {
                        noted += 1;
                    }
                }
            }
        }

        // **累加**而不是赋值：慢跑告警（`check_timeouts` 那一段）已经先加过一次了，
        // 赋值会把那一份抹掉——两个生产者共用一个计数，就只能加。
        report.attention_noted += noted;
        Ok(())
    }

    // ─────────────────────── 心跳刷新（决策 100）───────────────────────

    /// 系统命令 / 流式 token 的心跳：由工具层与 executor 调用。
    pub async fn heartbeat(&self, run_id: i64) -> Result<()> {
        self.store.touch_run_heartbeat(run_id).await
    }

    // ─────────────────────── 小时级维护 ───────────────────────

    async fn purge_expired_conversations(&self) -> Result<usize> {
        // 决策 321：保留期 DELETE 走**维护专用连接**，不占主池——重负载日实测这条
        // DELETE 在主池里把整池拖进慢语句（326s 期间慢 acquire 叠到 24s）。
        let mut conn = self.store.maintenance_connection().await?;
        let cutoff =
            self.clock.now() - Duration::days(self.settings.conversation_retention_days as i64);
        let purged = sqlx::query(
            "DELETE FROM kanban_node_conversations
             WHERE task_id IN (SELECT id FROM kanban_tasks WHERE status IN ('done','failed','cancelled'))
               AND created_at < ?",
        )
        .bind(crate::storage::ts(cutoff))
        .execute(&mut conn)
        .await?
        .rows_affected();
        Ok(purged as usize)
    }

    async fn aggregate_node_metrics(&self) -> Result<usize> {
        let tasks = self
            .store
            .list_tasks(&crate::storage::tasks::TaskFilter {
                include_archived: true,
                ..Default::default()
            })
            .await?;
        for task in &tasks {
            self.store.refresh_task_totals(&task.id).await?;
        }
        Ok(tasks.len())
    }

    async fn emit_pending(&self, task_id: &str, cursor_id: &str) -> Result<()> {
        let cursor = self.store.get_cursor(cursor_id).await?;
        if let Some(reason) = cursor.pending_reason {
            self.sse.emit(SseEvent::Pending {
                task_id: task_id.to_string(),
                branch: cursor.branch,
                cursor_id: cursor_id.to_string(),
                reason,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MaintenanceReport {
    /// 被保留期清掉的**终态任务**会话行（与值班长消息无关——那张表已豁免，票 04）。
    pub purged_conversations: usize,
    /// 被保留期清掉的**节点内消息日志**行（票 01：与任务会话同一把尺子，同一个 cutoff）。
    ///
    /// 单列一个读数而不是并进上面那项：两张表的量级完全不同（日志一行一条消息、会话一行一整块
    /// 会话），合起来读会让人以为「会话涨了一个数量级」。
    pub purged_node_messages: usize,
    /// 被过期清扫标成 `expired` 的提议（决策 207）。**清的是状态不是行**——那一轮留在
    /// 时间线里，故它与上面两项的「清掉了多少行」不是同一个量。
    pub expired_foreman_proposals: usize,
    /// 被保留期清掉的提议行（决策 207：与对讲台对话同一个保留期）。
    pub purged_foreman_proposals: usize,
    /// 被回收的**没人按过**的修复 worktree 数（决策 212③ / 票 12）。
    ///
    /// 与上面两项不同类：它清的是**磁盘上的目录**，不是库里的行。**分支不删**——与拒绝
    /// 那条路同一理由（分支是唯一的证据）。故这个数不为零时，`git branch` 里那些
    /// `repair/*` 仍然在，知道这一点才读得懂这个数。
    pub recycled_repair_worktrees: usize,
    /// 被保留期清掉的值班长待办行（票 05：同一个 `conversation_retention_days` 口径）。
    pub purged_attention: usize,
    pub aggregated_tasks: usize,
}

async fn branch_of(cursor_id: &str, store: &Store) -> Result<String> {
    Ok(store
        .get_cursor(cursor_id)
        .await
        .map(|c| c.branch)
        .unwrap_or_else(|_| crate::types::NodeCursor::BRANCH_MAIN.to_string()))
}

/// pending 原因 → 待办类别（票 05）：两个子类单列，其余一律 `task_pending`。
///
/// 单列它们不是为了分类好看：`retry_exhausted` 与 `context_overflow` 是**自动修复盯得最紧**
/// 的两类（前者是「跑不动了」，后者是「塞不下了」），播报分级与 §4.9 的止损都按它们判。
fn pending_attention_kind(kind: PendingKind) -> AttentionKind {
    match kind {
        PendingKind::RetryExhausted => AttentionKind::RetryExhausted,
        PendingKind::ContextOverflow => AttentionKind::ContextOverflow,
        // 决策 416 C：同上，单列因为它播报分级与处置都不同——它需要人**先动手**（修环境）
        // 才谈得上重试，并进 `task_pending` 会被当成「再等等就会自己走完」。
        PendingKind::EnvironmentBlocked => AttentionKind::EnvironmentBlocked,
        _ => AttentionKind::TaskPending,
    }
}

/// run 从起跑到 `now` 的毫秒数（决策 226）。时钟回拨时按 0 记，不写负数。
///
/// 存在的理由是一条实测：2026-09-19 的两次执行一次记作「8 小时 51 分」、一次「5 分 10 秒」，
/// 而台账里两条的 `duration_ms` **都是 0**——判超时那条路此前不带时长，于是最该被一眼读到的
/// 那个数字，只能靠读的人拿 `started_at` / `finished_at` 自己相减。
fn elapsed_ms(run: &NodeRun, now: chrono::DateTime<chrono::Utc>) -> u64 {
    (now - run.started_at).num_milliseconds().max(0) as u64
}

/// 超时那句话（决策 211④ / 票 04）：**把「当时在哪一步」带上**。
///
/// 不带的话，台账里就只剩「超时」两个字——2026-09-17 那次四小时挂死的信息量正是如此，
/// 要知道卡在哪个系统调用只能拿外部 `sample` 附进程。步骤由系统节点自己写
/// （[`crate::storage::Store::set_run_step`]），LLM 节点没有它，故可空。
fn timeout_detail(run: &NodeRun, what: &str) -> String {
    match run.step.as_deref() {
        Some(step) => format!("{}.{} {what}，当时在「{step}」", run.stage, run.node),
        None => format!("{}.{} {what}", run.stage, run.node),
    }
}
