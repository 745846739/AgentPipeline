//! KanbanScheduler（决策 55 / 57 / 66 / 92 / 100 / 102 / 117 / 127）。
//!
//! 职责边界：executor 是 DAG 执行引擎，不承担定时任务；超时、冲突恢复、依赖、准入、
//! 提醒都归 scheduler 的 10s tick。**`tick()` 是手动驱动接缝**（决策 143 接缝④）——
//! 测试直接调用它，生产由 [`KanbanScheduler::run_loop`] 按 `tick_interval_sec` 驱动。

use std::collections::HashSet;
use std::sync::Arc;

use chrono::Duration;

use crate::clock::Clock;
use crate::config::{effective_idle_timeout, effective_max_duration, node_timeouts, Settings};
use crate::process::ProcessKiller;
use crate::sse::{SseEvent, SseSink};
use crate::storage::attention::AttentionKind;
use crate::storage::observability::is_timed_out;
use crate::storage::tasks::DependencyState;
use crate::storage::Store;
use crate::types::{NodeRun, NodeStatus, PendingContext, PendingKind, PendingReason, TaskStatus};
use crate::Result;

/// resume 钩子：scheduler 通过它拉起 executor，不直接依赖 executor 实现。
pub type ResumeFn = Arc<dyn Fn(&str) + Send + Sync>;

/// 一次 tick 的产出（测试逐项断言六项职责）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TickReport {
    /// 判定超时并处理的 run id。
    pub timed_out_runs: Vec<i64>,
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
        // 值班长对话走**同一个保留天数**（票 05 / 决策 182）：对讲台不做全仓唯一一张
        // 不设保留期的表。两者分开计数，使维护报告说得出清掉的是哪一类。
        let cutoff =
            self.clock.now() - Duration::days(self.settings.conversation_retention_days as i64);
        let purged_foreman = self.store.purge_foreman_messages(cutoff).await?;
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
        // 待办表与其余各表**同一口径**的年龄清理（票 05）：不做全仓唯一一张不设保留期的表。
        let purged_attention = self.store.purge_attention(cutoff).await?;
        let aggregated = self.aggregate_node_metrics().await?;
        Ok(MaintenanceReport {
            purged_conversations: purged,
            purged_foreman_messages: purged_foreman,
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

        for run in self.store.active_runs().await? {
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
            let max_duration = effective_max_duration(
                self.settings.node_max_duration_sec,
                stage_cfg.and_then(|c| c.max_duration_sec),
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
        let elapsed_ms = (now - run.started_at).num_milliseconds().max(0) as u64;
        if !crate::metrics::should_alert_slow(elapsed_ms, p) {
            return Ok(());
        }
        let detail = format!(
            "{}.{} 已运行 {}ms，超过该节点 P90（{}ms）的 3 倍（P50 {}ms，样本 {}）",
            run.stage, run.node, elapsed_ms, p.p90_ms, p.p50_ms, p.samples
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
                    "elapsed_ms": elapsed_ms,
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

    /// 超时处理：杀进程组 → 未耗尽则干净对话重试 → 耗尽才 pending(timeout)。
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
                    error: Some(timeout_detail(run, "超时")),
                    ..Default::default()
                },
            )
            .await?;

        if run.attempt < self.settings.agent_retry_max {
            // 未耗尽：干净对话重试当前节点（决策 33），不计 validate_attempts
            self.store
                .insert_transition(
                    task_id,
                    &branch_of(cursor_id, &self.store).await?,
                    Some((run.stage, run.node)),
                    (run.stage, run.node),
                    crate::types::TransitionTrigger::Timeout,
                    Some("节点超时，干净对话重试"),
                )
                .await?;
            (self.resume)(task_id);
        } else {
            // 耗尽：pending 挂在该 run 所属的**游标**上（决策 82）
            self.store
                .set_cursor_pending(
                    cursor_id,
                    &PendingReason::new(
                        PendingKind::Timeout,
                        run.stage,
                        run.node,
                        format!(
                            "{}（attempt {}）",
                            timeout_detail(run, "执行超时"),
                            run.attempt
                        ),
                    ),
                )
                .await?;
            self.store.sync_task_projection(task_id).await?;
            report.timeout_pending_cursors.push(cursor_id.to_string());
            self.emit_pending(task_id, cursor_id).await?;
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
                            "项目级 run（{}）心跳停止 {idle}s，判定中断并标终态（决策 212）：\
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
            if let Some(reason) = &task.pending_reason {
                let live = self.store.load_live_cursors(&task.id).await?;
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
            if now - task.updated_at <= window {
                if let Some(merge) = self.store.merge_metadata(&task.id).await? {
                    if merge.gate == Some(crate::types::Gate::Fail)
                        && self
                            .store
                            .note_attention(
                                &task.id,
                                AttentionKind::GateFailure,
                                task.updated_at,
                                Some(&serde_json::json!({
                                    "gate_failure_kind": merge.gate_failure_kind.map(|k| match k {
                                        crate::types::GateFailureKind::Lint => "lint",
                                        crate::types::GateFailureKind::Test => "test",
                                    }),
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

            // ⑤ 调度器处置未生效 + ⑥ owner 持有超时：两条缝，都要求「任务还在 running」。
            //    实测（2026-09-17）：run 已被标 timeout、transition 也写了「干净对话重试」，
            //    但**没有 attempt-2 的 run 行**，任务从此停在 running——check_timeouts 只看
            //    active_runs()（run 已是终态就不再被扫），remind_pending_tasks 的 stalled
            //    判据也不成立。这条缝此前零信号。
            if task.status == TaskStatus::Running {
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
        let cutoff =
            self.clock.now() - Duration::days(self.settings.conversation_retention_days as i64);
        let purged = sqlx::query(
            "DELETE FROM kanban_node_conversations
             WHERE task_id IN (SELECT id FROM kanban_tasks WHERE status IN ('done','failed','cancelled'))
               AND created_at < ?",
        )
        .bind(crate::storage::ts(cutoff))
        .execute(self.store.pool())
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
    pub purged_conversations: usize,
    /// 被保留期清掉的值班长会话行（票 05）。与上一项分开计数——它们挂在不同表上，
    /// 混成一个数就看不出是哪一类在增长。
    pub purged_foreman_messages: usize,
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
        _ => AttentionKind::TaskPending,
    }
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
