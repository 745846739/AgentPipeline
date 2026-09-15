//! L2 集成：KanbanScheduler.tick 的六项职责（testing.md §6，决策 55 / 57 / 66 / 92 / 100 / 102 / 117）。
//!
//! `tick()` 是手动驱动接缝（决策 143 接缝④）——测试直接调用，不等 10s。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use agentpipeline_core::clock::Clock;
use agentpipeline_core::config::Settings;
use agentpipeline_core::scheduler::KanbanScheduler;
use agentpipeline_core::sse::SseEventType;
use agentpipeline_core::storage::observability::{NewRun, RunOutcome};
use agentpipeline_core::storage::tasks::TaskFilter;
use agentpipeline_core::storage::Store;
use agentpipeline_core::types::{
    CursorStatus, Node, NodeStatus, PendingContext, PendingKind, PendingReason, Stage, TaskStatus,
    TransitionTrigger,
};
use testkit::{ManualClock, RecordingKiller, SseRecorder, TestHome};

struct Harness {
    _home: TestHome,
    store: Store,
    clock: ManualClock,
    killer: RecordingKiller,
    sse: SseRecorder,
    resumes: Arc<AtomicUsize>,
}

impl Harness {
    async fn new() -> Self {
        let home = TestHome::new().unwrap();
        let clock = ManualClock::fixed();
        let store = Store::open(home.home().clone(), Arc::new(clock.clone()))
            .await
            .unwrap();
        let repo = home.scratch_dir("proj");
        testkit::seed_project(&store, "p1", "示例", &repo, "main")
            .await
            .unwrap();
        Harness {
            _home: home,
            store,
            clock,
            killer: RecordingKiller::new(),
            sse: SseRecorder::new(),
            resumes: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn scheduler(&self, settings: Settings) -> KanbanScheduler {
        let resumes = self.resumes.clone();
        KanbanScheduler::new(
            self.store.clone(),
            settings,
            Arc::new(self.clock.clone()),
            Arc::new(self.killer.clone()),
            Arc::new(self.sse.clone()),
            Arc::new(move |_task_id: &str| {
                resumes.fetch_add(1, Ordering::SeqCst);
            }),
        )
    }

    async fn seed_task(&self, task_id: &str) -> agentpipeline_core::types::Task {
        testkit::seed_task(&self.store, task_id, "p1")
            .await
            .unwrap()
    }

    /// 造一个"正在跑"的 run，并把 started_at / last_activity_at 回拨。
    async fn running_run(
        &self,
        task_id: &str,
        cursor_id: &str,
        attempt: u32,
        started_secs_ago: i64,
        idle_secs_ago: i64,
        pgid: Option<i32>,
    ) -> i64 {
        let run_id = self
            .store
            .insert_run(&NewRun {
                task_id: task_id.into(),
                cursor_id: cursor_id.into(),
                stage: Stage::Develop,
                node: Node::Execute,
                attempt,
                agent_type: "main".into(),
                parent_run_id: None,
                prompt_template_hash: None,
                process_group_id: pgid,
            })
            .await
            .unwrap();
        let started = self.clock.now() - chrono::Duration::seconds(started_secs_ago);
        let activity = self.clock.now() - chrono::Duration::seconds(idle_secs_ago);
        sqlx::query(
            "UPDATE kanban_node_runs SET started_at = ?, last_activity_at = ? WHERE id = ?",
        )
        .bind(agentpipeline_core::storage::ts(started))
        .bind(agentpipeline_core::storage::ts(activity))
        .bind(run_id)
        .execute(self.store.pool())
        .await
        .unwrap();
        run_id
    }

    async fn mark_running(&self, task_id: &str) {
        self.store
            .set_task_status(task_id, TaskStatus::Running)
            .await
            .unwrap();
    }

    /// 造一条**已完成**的成功 run（给定耗时），供自适应分位数采样（票 17）。
    async fn finished_run(&self, task_id: &str, cursor_id: &str, duration_ms: u64) -> i64 {
        let run_id = self
            .store
            .insert_run(&NewRun {
                task_id: task_id.into(),
                cursor_id: cursor_id.into(),
                stage: Stage::Develop,
                node: Node::Execute,
                attempt: 1,
                agent_type: "main".into(),
                parent_run_id: None,
                prompt_template_hash: None,
                process_group_id: None,
            })
            .await
            .unwrap();
        self.store
            .finish_run(
                run_id,
                &RunOutcome {
                    status: Some(NodeStatus::Success),
                    duration_ms,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        run_id
    }
}

// ─────────────────────── 票 17：自适应超时告警（决策 66）───────────────────────

#[tokio::test]
async fn adaptive_timeout_off_by_default_yields_no_alerts() {
    // 开关关闭 → 零行为变化：即便历史样本齐全、当前 run 极慢，也不产告警。
    let h = Harness::new().await;
    h.seed_task("t-adaptive-off").await;
    h.mark_running("t-adaptive-off").await;
    let cursor = h.store.load_live_cursors("t-adaptive-off").await.unwrap()[0].clone();
    // 5 条成功历史（1s 级）
    for _ in 0..5 {
        h.finished_run("t-adaptive-off", &cursor.cursor_id, 1_000)
            .await;
    }
    h.running_run("t-adaptive-off", &cursor.cursor_id, 1, 10_000, 0, None)
        .await;

    let settings = Settings::default();
    assert!(!settings.adaptive_timeout_enabled, "缺省关闭");
    let report = h.scheduler(settings).tick().await.unwrap();
    assert!(
        report.slow_run_alerts.is_empty(),
        "关闭时不得产告警：{:?}",
        report.slow_run_alerts
    );
}

#[tokio::test]
async fn adaptive_timeout_enabled_alerts_only_when_past_three_times_p90() {
    let h = Harness::new().await;
    h.seed_task("t-adaptive-on").await;
    h.mark_running("t-adaptive-on").await;
    let cursor = h.store.load_live_cursors("t-adaptive-on").await.unwrap()[0].clone();
    // 历史：5 条成功、每次 1s → P90 = 1000ms，告警线 3000ms
    for _ in 0..5 {
        h.finished_run("t-adaptive-on", &cursor.cursor_id, 1_000)
            .await;
    }
    // 当前 run 已跑 10s（> 3×P90）且心跳新鲜（不触发空闲超时）
    h.running_run("t-adaptive-on", &cursor.cursor_id, 1, 10, 0, None)
        .await;

    let settings = Settings {
        adaptive_timeout_enabled: true,
        ..Default::default()
    };
    let report = h.scheduler(settings).tick().await.unwrap();
    assert_eq!(report.slow_run_alerts.len(), 1, "应产一条慢运行告警");
    let (task_id, detail) = &report.slow_run_alerts[0];
    assert_eq!(task_id, "t-adaptive-on");
    assert!(
        detail.contains("P90") && detail.contains("3 倍"),
        "告警内容应含分位数依据：{detail}"
    );
    // 告警不影响超时判定：run 仍在跑（未进 timed_out）
    assert!(
        report.timed_out_runs.is_empty(),
        "慢运行告警不得被当作超时处理"
    );
}

#[tokio::test]
async fn adaptive_timeout_cold_start_does_not_alert() {
    // 冷启动样本不足（< 最低样本量）→ 不展示、不告警
    let h = Harness::new().await;
    h.seed_task("t-adaptive-cold").await;
    h.mark_running("t-adaptive-cold").await;
    let cursor = h.store.load_live_cursors("t-adaptive-cold").await.unwrap()[0].clone();
    // 只有 2 条历史（不足 5）
    for _ in 0..2 {
        h.finished_run("t-adaptive-cold", &cursor.cursor_id, 1_000)
            .await;
    }
    h.running_run("t-adaptive-cold", &cursor.cursor_id, 1, 10, 0, None)
        .await;
    let settings = Settings {
        adaptive_timeout_enabled: true,
        ..Default::default()
    };
    let report = h.scheduler(settings).tick().await.unwrap();
    assert!(
        report.slow_run_alerts.is_empty(),
        "冷启动样本不足不得告警：{:?}",
        report.slow_run_alerts
    );
}

// ─────────────────────── ① 超时（决策 33 / 64 / 66 / 100 / 122）───────────────────────

#[tokio::test]
async fn timeout_kills_process_group_and_retries_until_exhausted() {
    let h = Harness::new().await;
    let task = h.seed_task("t1").await;
    h.mark_running("t1").await;
    let cursor = h.store.load_live_cursors("t1").await.unwrap()[0].clone();

    // attempt 1 < agent_retry_max(3)：杀进程组 + 干净对话重试
    h.running_run("t1", &cursor.cursor_id, 1, 400, 400, Some(4242))
        .await;
    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert_eq!(report.timed_out_runs.len(), 1);
    assert_eq!(h.killer.killed_groups(), vec![4242], "必须杀整个进程组");
    assert!(
        report.timeout_pending_cursors.is_empty(),
        "未耗尽不得 pending"
    );
    assert_eq!(
        h.resumes.load(Ordering::SeqCst),
        1,
        "应拉起 executor 干净重试"
    );
    // run 落库为 timeout
    let runs = h.store.list_runs("t1").await.unwrap();
    assert_eq!(runs[0].status, NodeStatus::Timeout);

    // attempt 3 = agent_retry_max：耗尽 → pending(timeout) 挂在该游标上
    h.running_run("t1", &cursor.cursor_id, 3, 400, 400, Some(4243))
        .await;
    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert_eq!(
        report.timeout_pending_cursors,
        vec![cursor.cursor_id.clone()]
    );
    let after = h.store.get_cursor(&cursor.cursor_id).await.unwrap();
    assert_eq!(after.status, CursorStatus::Pending);
    assert_eq!(
        after.pending_reason.as_ref().unwrap().kind,
        PendingKind::Timeout
    );
    // 任务级投影 + SSE
    let projected = h.store.get_task("t1").await.unwrap();
    assert_eq!(projected.status, TaskStatus::Pending);
    assert_eq!(h.sse.count_of(SseEventType::Pending), 1);
    let _ = task;
}

#[tokio::test]
async fn long_system_command_survives_idle_timeout_when_heartbeat_refreshes() {
    // 决策 100：merge 闸门最长 600s，若按 started_at 判空闲会被 300s 误杀
    let h = Harness::new().await;
    h.seed_task("t1").await;
    h.mark_running("t1").await;
    let cursor = h.store.load_live_cursors("t1").await.unwrap()[0].clone();

    // 已跑 600s，但 10s 前刚有活动
    h.running_run("t1", &cursor.cursor_id, 1, 600, 10, Some(5555))
        .await;
    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert!(
        report.timed_out_runs.is_empty(),
        "有心跳的长命令不得被空闲超时误杀"
    );
    assert!(!h.killer.was_called());

    // 心跳停跳 400s → 立刻超时
    h.running_run("t1", &cursor.cursor_id, 1, 700, 400, Some(5556))
        .await;
    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert_eq!(report.timed_out_runs.len(), 1);
    assert!(h.killer.killed_groups().contains(&5556));
}

#[tokio::test]
async fn absolute_timeout_fires_even_with_activity() {
    // 决策 66：node_max_duration_sec 是防不收敛循环的绝对上限
    let h = Harness::new().await;
    h.seed_task("t1").await;
    h.mark_running("t1").await;
    let cursor = h.store.load_live_cursors("t1").await.unwrap()[0].clone();
    h.running_run("t1", &cursor.cursor_id, 1, 2000, 1, Some(1))
        .await;

    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert_eq!(
        report.timed_out_runs.len(),
        1,
        "持续有活动也须被绝对超时拦下"
    );
}

#[tokio::test]
async fn effective_timeout_layering_node_beats_stage_beats_global() {
    let h = Harness::new().await;
    h.seed_task("t1").await;
    h.mark_running("t1").await;
    let cursor = h.store.load_live_cursors("t1").await.unwrap()[0].clone();

    // 阶段级 idle 覆盖为 60s；节点级再覆盖为 120s（决策 66：node > stage > global）
    h.store
        .upsert_stage_config(&agentpipeline_core::types::StageConfig {
            stage: "develop".into(),
            idle_timeout_sec: Some(60),
            node_overrides_json: Some(serde_json::json!({
                "execute": {"idle_timeout_sec": 120}
            })),
            ..Default::default()
        })
        .await
        .unwrap();

    // 空闲 100s：全局 300 不会超时，阶段 60 会超时，但节点级 120 覆盖后**不**超时
    h.running_run("t1", &cursor.cursor_id, 1, 100, 100, None)
        .await;
    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert!(
        report.timed_out_runs.is_empty(),
        "节点级覆盖 120s 应生效（100s 不超时）"
    );

    // 空闲 130s：超过节点级 120s → 超时
    h.running_run("t1", &cursor.cursor_id, 1, 130, 130, None)
        .await;
    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert_eq!(report.timed_out_runs.len(), 1);
}

#[tokio::test]
async fn timeout_on_one_branch_does_not_touch_the_other() {
    // 决策 82：超时只作用于该分支的 run
    let h = Harness::new().await;
    h.seed_task("t1").await;
    h.mark_running("t1").await;
    let split = h.store.split_cursors("t1").await.unwrap();
    let dev = split
        .iter()
        .find(|c| c.branch == "develop-design")
        .unwrap()
        .clone();

    h.running_run("t1", &dev.cursor_id, 3, 400, 400, Some(9))
        .await;
    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert_eq!(report.timeout_pending_cursors, vec![dev.cursor_id.clone()]);

    let cursors = h.store.load_live_cursors("t1").await.unwrap();
    let test_branch = cursors.iter().find(|c| c.branch == "test-design").unwrap();
    assert_eq!(test_branch.status, CursorStatus::Active, "另一分支不受影响");
}

// ─────────────────────── ② 冲突恢复（决策 102）───────────────────────

async fn set_architect_files(h: &Harness, task_id: &str, files: &[&str]) {
    h.store
        .upsert_stage_output(
            task_id,
            Stage::ArchitectDesign,
            "design_doc",
            "design.md",
            Some(&serde_json::json!({
                "affected_files": files,
                "new_symbols": []
            })),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn conflict_wait_waits_for_all_terminal_then_rechecks_overlap() {
    let h = Harness::new().await;
    h.seed_task("t1").await;
    h.seed_task("t2").await;
    h.seed_task("t3").await;
    h.mark_running("t1").await;
    set_architect_files(&h, "t1", &["src/a.rs"]).await;
    set_architect_files(&h, "t2", &["src/a.rs"]).await;
    set_architect_files(&h, "t3", &["src/a.rs"]).await;

    let cursor = h.store.load_live_cursors("t1").await.unwrap()[0].clone();
    h.store
        .set_cursor_pending(
            &cursor.cursor_id,
            &PendingReason::new(
                PendingKind::ConflictWait,
                Stage::ArchitectDesign,
                Node::Execute,
                "与其他任务冲突",
            )
            .with_context(PendingContext {
                conflict_task_ids: vec!["t2".into(), "t3".into()],
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    h.store.sync_task_projection("t1").await.unwrap();

    // 一个终态、一个仍在跑 → 不恢复（t3 仍是活跃任务）
    h.store
        .set_task_status("t2", TaskStatus::Done)
        .await
        .unwrap();
    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert!(report.conflict_resumed.is_empty());
    assert_eq!(
        h.store.get_cursor(&cursor.cursor_id).await.unwrap().status,
        CursorStatus::Pending
    );

    // 两个原对手终态，但等待期间**新冒出**一个活跃冲突任务 t4 → 复检仍有交集
    h.store
        .set_task_status("t3", TaskStatus::Done)
        .await
        .unwrap();
    h.seed_task("t4").await;
    set_architect_files(&h, "t4", &["src/a.rs"]).await;
    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert!(report.conflict_resumed.is_empty(), "仍有交集不得恢复");
    let updated = h.store.get_cursor(&cursor.cursor_id).await.unwrap();
    assert_eq!(
        updated
            .pending_reason
            .unwrap()
            .context
            .unwrap()
            .conflict_task_ids,
        vec!["t4".to_string()],
        "应换成新的冲突对象且不重跑节点"
    );
    assert_eq!(h.sse.count_of(SseEventType::PendingUpdated), 1);

    // t4 也终态且不再冲突 → 全部终态 + 复检无交集 → 清 pending 并拉起 executor
    h.store
        .upsert_stage_output(
            "t4",
            Stage::ArchitectDesign,
            "design_doc",
            "design.md",
            Some(&serde_json::json!({"affected_files": ["src/other.rs"], "new_symbols": []})),
        )
        .await
        .unwrap();
    // 新冲突对象仍活跃时不得恢复（"全部终态"是硬门槛）
    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert!(
        report.conflict_resumed.is_empty(),
        "新冲突任务未终态前不得恢复"
    );

    h.store
        .set_task_status("t4", TaskStatus::Done)
        .await
        .unwrap();
    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert_eq!(report.conflict_resumed, vec!["t1".to_string()]);
    let cleared = h.store.get_cursor(&cursor.cursor_id).await.unwrap();
    assert_eq!(cleared.status, CursorStatus::Active);
    assert!(cleared.pending_reason.is_none());
    assert!(h.resumes.load(Ordering::SeqCst) >= 1);
}

// ─────────────────────── ③④ 依赖（决策 57 / 116）───────────────────────

#[tokio::test]
async fn dependencies_promote_when_all_done_and_fail_otherwise() {
    let h = Harness::new().await;
    h.seed_task("dep-ok").await;
    h.seed_task("dep-fail").await;

    let waiting_a = testkit::seed_task_full(
        &h.store,
        "wa",
        "p1",
        agentpipeline_core::types::ReviewMode::Agent,
        &["dep-ok"],
    )
    .await
    .unwrap();
    assert_eq!(waiting_a.status, TaskStatus::Waiting);

    // 依赖 done → queued（max=0 隔离准入，只验提升本身）
    h.store
        .set_task_status("dep-ok", TaskStatus::Done)
        .await
        .unwrap();
    let no_admission = Settings {
        max_concurrent_tasks: 0,
        ..Default::default()
    };
    let report = h.scheduler(no_admission).tick().await.unwrap();
    assert_eq!(report.dependencies_promoted, vec!["wa".to_string()]);
    assert_eq!(
        h.store.get_task("wa").await.unwrap().status,
        TaskStatus::Queued
    );

    // 依赖 failed → pending(dependency_failed)，挂在 main 游标上
    testkit::seed_task_full(
        &h.store,
        "wb",
        "p1",
        agentpipeline_core::types::ReviewMode::Agent,
        &["dep-fail"],
    )
    .await
    .unwrap();
    h.store
        .set_task_status("dep-fail", TaskStatus::Failed)
        .await
        .unwrap();
    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert_eq!(report.dependency_failed, vec!["wb".to_string()]);
    let cursor = h.store.resolve_sole_cursor("wb").await.unwrap().unwrap();
    let reason = cursor.pending_reason.unwrap();
    assert_eq!(reason.kind, PendingKind::DependencyFailed);
    assert_eq!(reason.stage, Stage::Init, "挂在 main 游标（stage=init）");
    assert_eq!(
        reason.context.unwrap().kind.as_deref(),
        Some("dependency_failed")
    );
    assert_eq!(
        h.store.get_task("wb").await.unwrap().status,
        TaskStatus::Pending
    );

    // 依赖 cancelled → 动作集裁剪掉"等待依赖重试"（决策 116）
    // 依赖必须先存在（FK 约束）
    h.seed_task("dep-x").await;
    testkit::seed_task_full(
        &h.store,
        "wc",
        "p1",
        agentpipeline_core::types::ReviewMode::Agent,
        &["dep-x"],
    )
    .await
    .unwrap();
    h.store
        .set_task_status("dep-x", TaskStatus::Cancelled)
        .await
        .unwrap();
    h.scheduler(Settings::default()).tick().await.unwrap();
    let cursor = h.store.resolve_sole_cursor("wc").await.unwrap().unwrap();
    let reason = cursor.pending_reason.unwrap();
    assert_eq!(
        reason.context.as_ref().unwrap().kind.as_deref(),
        Some("dependency_cancelled")
    );
    let actions: Vec<String> = agentpipeline_core::actions::allowed_actions(&reason, None)
        .into_iter()
        .map(|a| a.action)
        .collect();
    assert!(!actions.contains(&"wait_dependency_retry".to_string()));
}

#[tokio::test]
async fn dependency_failed_recovers_when_dependency_retries() {
    let h = Harness::new().await;
    h.seed_task("dep").await;
    testkit::seed_task_full(
        &h.store,
        "w",
        "p1",
        agentpipeline_core::types::ReviewMode::Agent,
        &["dep"],
    )
    .await
    .unwrap();
    h.store
        .set_task_status("dep", TaskStatus::Failed)
        .await
        .unwrap();
    h.scheduler(Settings::default()).tick().await.unwrap();
    assert_eq!(
        h.store.get_task("w").await.unwrap().status,
        TaskStatus::Pending
    );

    // 依赖被重试转回 running → 清 pending 退回 waiting（决策 57）
    h.store
        .set_task_status("dep", TaskStatus::Running)
        .await
        .unwrap();
    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert_eq!(report.dependencies_recovered, vec!["w".to_string()]);
    let task = h.store.get_task("w").await.unwrap();
    assert_eq!(task.status, TaskStatus::Waiting);
    assert!(task.pending_reason.is_none());
}

// ─────────────────────── ⑤ 准入（决策 98 / 117）───────────────────────

#[tokio::test]
async fn admission_respects_max_concurrent_and_pending_occupies_slot() {
    let h = Harness::new().await;
    for id in ["t1", "t2", "t3"] {
        h.seed_task(id).await;
    }
    let settings = Settings {
        max_concurrent_tasks: 1,
        ..Default::default()
    };
    let report = h.scheduler(settings.clone()).tick().await.unwrap();
    assert_eq!(report.admitted.len(), 1, "max=1 只放行一个");
    assert_eq!(h.store.occupying_slots().await.unwrap(), 1);

    // 再 tick 不会重复放行
    let report = h.scheduler(settings.clone()).tick().await.unwrap();
    assert!(report.admitted.is_empty());

    // pending 仍占名额（决策 117）→ 名额满，队列不动
    let admitted_id = h
        .store
        .queued_tasks()
        .await
        .unwrap()
        .first()
        .map(|t| t.id.clone())
        .unwrap_or_else(|| "t1".to_string());
    h.store
        .set_task_status(&admitted_id, TaskStatus::Pending)
        .await
        .unwrap();
    let report = h.scheduler(settings).tick().await.unwrap();
    assert!(report.admitted.is_empty());

    // 终态释放名额 → 下一个才放行
    h.store
        .set_task_status(&admitted_id, TaskStatus::Done)
        .await
        .unwrap();
    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert_eq!(report.admitted.len(), 1);
}

// ─────────────────────── ⑥ 提醒与 stalled（决策 34 / 92）───────────────────────

#[tokio::test]
async fn stall_predicate_ignores_tasks_with_a_runnable_branch() {
    let h = Harness::new().await;
    h.seed_task("t1").await;
    h.mark_running("t1").await;
    h.store.split_cursors("t1").await.unwrap();

    // 一个分支 pending，另一个仍 active → status = pending 但并未卡住（决策 92）
    let cursors = h.store.load_live_cursors("t1").await.unwrap();
    let dev = cursors
        .iter()
        .find(|c| c.branch == "develop-design")
        .unwrap()
        .clone();
    h.store
        .set_cursor_pending(
            &dev.cursor_id,
            &PendingReason::new(
                PendingKind::Timeout,
                Stage::DevelopDesign,
                Node::Execute,
                "超时",
            ),
        )
        .await
        .unwrap();
    h.store.sync_task_projection("t1").await.unwrap();
    h.clock.advance_secs(80 * 3600);

    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert!(
        report.stalled.is_empty(),
        "另一分支可推进 → 不得标记 stalled"
    );
    assert!(!h.store.get_task("t1").await.unwrap().stalled);
}

#[tokio::test]
async fn stall_and_reminder_fire_once_for_fully_blocked_task() {
    let h = Harness::new().await;
    h.seed_task("t1").await;
    h.mark_running("t1").await;
    let cursor = h.store.load_live_cursors("t1").await.unwrap()[0].clone();
    h.store
        .set_cursor_pending(
            &cursor.cursor_id,
            &PendingReason::new(
                PendingKind::InfoInsufficient,
                Stage::ArchitectDesign,
                Node::ValidateInput,
                "等待补充",
            ),
        )
        .await
        .unwrap();
    h.store.sync_task_projection("t1").await.unwrap();

    // 25h（> pending_reminder_hours = 24）但 < 72h
    h.clock.advance_secs(25 * 3600);
    let mut scheduler = h.scheduler(Settings::default());
    let report = scheduler.tick().await.unwrap();
    assert_eq!(report.reminded, vec!["t1".to_string()]);
    assert!(report.stalled.is_empty());
    scheduler.mark_reminded(&report.reminded);

    // 提醒只重复一次
    h.clock.advance_secs(3600);
    let report = scheduler.tick().await.unwrap();
    assert!(report.reminded.is_empty(), "同一任务不重复提醒");

    // 80h（> pending_timeout_hours = 72）→ stalled + SSE
    h.clock.advance_secs(55 * 3600);
    let report = scheduler.tick().await.unwrap();
    assert_eq!(report.stalled, vec!["t1".to_string()]);
    assert!(h.store.get_task("t1").await.unwrap().stalled);
    assert_eq!(h.sse.count_of(SseEventType::Pending), 0);

    // 再 tick 不重复标记
    let report = scheduler.tick().await.unwrap();
    assert!(report.stalled.is_empty());
}

#[tokio::test]
async fn tick_on_empty_database_is_a_noop() {
    let h = Harness::new().await;
    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert_eq!(report, Default::default());
}

// ─────────────────────── 维护任务（小时级）───────────────────────

#[tokio::test]
async fn maintenance_refreshes_totals_and_purges_expired_conversations() {
    let h = Harness::new().await;
    h.seed_task("t1").await;
    let cursor = h.store.load_live_cursors("t1").await.unwrap()[0].clone();
    let run_id = h
        .store
        .insert_run(&NewRun {
            task_id: "t1".into(),
            cursor_id: cursor.cursor_id.clone(),
            stage: Stage::Develop,
            node: Node::Execute,
            attempt: 1,
            agent_type: "main".into(),
            parent_run_id: None,
            prompt_template_hash: None,
            process_group_id: None,
        })
        .await
        .unwrap();
    h.store
        .finish_run(
            run_id,
            &RunOutcome {
                status: Some(NodeStatus::Success),
                prompt_tokens: 200,
                completion_tokens: 100,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    h.store
        .insert_conversation(
            "t1",
            run_id,
            Stage::Develop,
            Node::Execute,
            1,
            "main",
            None,
            &serde_json::json!([{"role": "user", "content": "x"}]),
            None,
            200,
            100,
        )
        .await
        .unwrap();
    h.store.mark_terminal("t1", TaskStatus::Done).await.unwrap();

    let scheduler = h.scheduler(Settings::default());
    let report = scheduler.maintenance().await.unwrap();
    assert_eq!(report.aggregated_tasks, 1);
    let task = h.store.get_task("t1").await.unwrap();
    assert_eq!(task.total_tokens, 300);
    assert_eq!(task.total_calls, 1, "system run 不计入 total_calls");

    // 会话保留期内不清理
    assert_eq!(report.purged_conversations, 0);
    assert_eq!(
        h.store.list_conversations("t1", false).await.unwrap().len(),
        1
    );

    // 超过 conversation_retention_days(30) → 终态任务的会话被清理
    h.clock.advance_secs(31 * 24 * 3600);
    let report = scheduler.maintenance().await.unwrap();
    assert_eq!(report.purged_conversations, 1);
    assert!(h
        .store
        .list_conversations("t1", false)
        .await
        .unwrap()
        .is_empty());
}

// ─────────────────────── 第一层冲突判定（决策 71 / 102）───────────────────────

#[tokio::test]
async fn first_layer_conflict_yields_only_to_the_later_task() {
    let h = Harness::new().await;
    h.seed_task("early").await;
    h.clock.advance_secs(10);
    h.seed_task("late").await;
    set_architect_files(&h, "early", &["src/shared.rs"]).await;
    set_architect_files(&h, "late", &["src/shared.rs", "src/only.rs"]).await;

    // 较晚者让步
    let late_conflicts = h.store.first_layer_conflicts("late").await.unwrap();
    assert_eq!(late_conflicts.len(), 1);
    assert_eq!(late_conflicts[0].task_id, "early");
    assert_eq!(late_conflicts[0].overlapping_files, vec!["src/shared.rs"]);

    // 较早者不因对方回退（环消除）
    let early_conflicts = h.store.first_layer_conflicts("early").await.unwrap();
    assert!(early_conflicts.is_empty());
}

#[tokio::test]
async fn first_layer_conflict_uses_module_symbol_pairs_not_bare_names() {
    let h = Harness::new().await;
    h.seed_task("a").await;
    h.clock.advance_secs(10);
    h.seed_task("b").await;

    h.store
        .upsert_stage_output(
            "a",
            Stage::ArchitectDesign,
            "design_doc",
            "design.md",
            Some(&serde_json::json!({
                "affected_files": [],
                "new_symbols": [
                    {"name": "new", "kind": "function", "module_path": "crate::auth", "file_path": "src/auth.rs"}
                ]
            })),
        )
        .await
        .unwrap();
    // 纯 name 重合（不同 module_path）→ 只 warning，不 conflict
    h.store
        .upsert_stage_output(
            "b",
            Stage::ArchitectDesign,
            "design_doc",
            "design.md",
            Some(&serde_json::json!({
                "affected_files": [],
                "new_symbols": [
                    {"name": "new", "kind": "function", "module_path": "crate::billing", "file_path": "src/billing.rs"}
                ]
            })),
        )
        .await
        .unwrap();
    // 纯 name 重合（不同 module_path）→ 降级为 warning（duplicate_risk=low），
    // 不作为高冲突参与环消除（决策 71② / 120）
    let conflicts = h.store.first_layer_conflicts("b").await.unwrap();
    assert_eq!(conflicts.len(), 1, "纯 name 重合必须以 warning 形式可见");
    assert_eq!(
        conflicts[0].duplicate_risk,
        Some(agentpipeline_core::types::DuplicateRisk::Low)
    );
    assert!(conflicts[0].overlapping_files.is_empty());
    assert_eq!(
        conflicts[0].overlapping_symbols,
        vec!["crate::billing::new"]
    );

    // (module_path, name) 完全一致 → conflict
    h.store
        .upsert_stage_output(
            "b",
            Stage::ArchitectDesign,
            "design_doc",
            "design.md",
            Some(&serde_json::json!({
                "affected_files": [],
                "new_symbols": [
                    {"name": "new", "kind": "function", "module_path": "crate::auth", "file_path": "src/auth.rs"}
                ]
            })),
        )
        .await
        .unwrap();
    let conflicts = h.store.first_layer_conflicts("b").await.unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].overlapping_symbols, vec!["crate::auth::new"]);
}

#[tokio::test]
async fn same_second_tasks_break_tie_by_id_order() {
    let h = Harness::new().await;
    // 同一固定时钟秒内创建的两个任务
    h.seed_task("aaa").await;
    h.seed_task("zzz").await;
    let early = h.store.get_task("aaa").await.unwrap();
    let late = h.store.get_task("zzz").await.unwrap();
    assert_eq!(early.created_at, late.created_at, "同一秒");
    assert!(h.store.yields_to(&late, &early), "同秒时 id 字典序大者让步");
    assert!(!h.store.yields_to(&early, &late));
}

// ─────────────────────── 迁移状态写入（决策 84 / 107）───────────────────────

#[tokio::test]
async fn join_boundary_is_written_only_by_the_next_landing() {
    // 决策 107：waiting_join 的唯一写入路径是 advance_cursor 的 next 落点判定
    let h = Harness::new().await;
    h.seed_task("t1").await;
    let split = h.store.split_cursors("t1").await.unwrap();
    let dev = split
        .iter()
        .find(|c| c.branch == "develop-design")
        .unwrap()
        .clone();
    assert_eq!(dev.stage, Stage::DevelopDesign);
    // 下一阶段是 join（sync-check）
    assert!(agentpipeline_core::pipeline::next_is_join(
        Stage::DevelopDesign
    ));

    h.store
        .set_cursor_waiting_join(&dev.cursor_id)
        .await
        .unwrap();
    let after = h.store.get_cursor(&dev.cursor_id).await.unwrap();
    assert_eq!(after.status, CursorStatus::WaitingJoin);
    assert!(!after.skipped_to_join, "普通 next 不置 skip 标志");

    // 两条都到边界 → join 可推进
    let other = h.store.load_live_cursors("t1").await.unwrap();
    let test_cursor = other.iter().find(|c| c.branch == "test-design").unwrap();
    assert!(!agentpipeline_core::pipeline::is_join_ready(
        &h.store.load_live_cursors("t1").await.unwrap()
    ));
    h.store
        .set_cursor_waiting_join(&test_cursor.cursor_id)
        .await
        .unwrap();
    assert!(agentpipeline_core::pipeline::is_join_ready(
        &h.store.load_live_cursors("t1").await.unwrap()
    ));

    // 合并后回到单 main 游标
    let main = h.store.merge_cursors_to_develop("t1").await.unwrap();
    assert_eq!(main.stage, Stage::Develop);
    assert_eq!(main.node, Node::Execute);
}

#[tokio::test]
async fn transitions_are_recorded_with_branch() {
    let h = Harness::new().await;
    h.seed_task("t1").await;
    h.store
        .insert_transition(
            "t1",
            "main",
            Some((Stage::Init, Node::Execute)),
            (Stage::ArchitectDesign, Node::ValidateInput),
            TransitionTrigger::Normal,
            None,
        )
        .await
        .unwrap();
    let transitions = h.store.list_transitions("t1").await.unwrap();
    assert_eq!(transitions.len(), 1);
    assert_eq!(transitions[0].branch, "main");
    assert_eq!(transitions[0].trigger, TransitionTrigger::Normal);
    assert_eq!(transitions[0].from_stage, Some(Stage::Init));

    // 任务列表筛选仍可用（回归：投影不破坏筛选字段）
    let filtered = h
        .store
        .list_tasks(&TaskFilter {
            status: Some(TaskStatus::Queued),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(filtered.len(), 1);
}

/// 票 08：子代理 run 挂着父节点的 task + cursor，但**不是节点自身的执行**——
/// 超时扫描必须跳过它。否则父节点还在正常干活（子代理在跑），scheduler 就会把
/// 这次并行检索判成父节点超时，写一条节点级重试 transition、拉起一次伪重试。
#[tokio::test]
async fn subagent_runs_are_not_swept_as_node_timeouts() {
    let h = Harness::new().await;
    h.seed_task("t1").await;
    h.mark_running("t1").await;
    let cursor = h.store.load_live_cursors("t1").await.unwrap()[0].clone();

    // 父节点正常（刚有过活动），子代理已挂了很久且没有自己的心跳
    // ——子代理的心跳是刷父 run 的（决策 88 同源做法），所以子代理行本身会显得陈旧。
    let parent = h
        .running_run("t1", &cursor.cursor_id, 1, 10, 10, None)
        .await;
    let sub = h
        .store
        .insert_run(&NewRun {
            task_id: "t1".into(),
            cursor_id: cursor.cursor_id.clone(),
            stage: Stage::Develop,
            node: Node::Execute,
            attempt: 1,
            agent_type: "subagent".into(),
            parent_run_id: Some(parent),
            prompt_template_hash: None,
            process_group_id: None,
        })
        .await
        .unwrap();
    // 把子代理行的时间戳推到远超空闲超时（stale），坐实「按节点语义会被判超时」
    let long_ago = h.clock.now() - chrono::Duration::seconds(400);
    sqlx::query("UPDATE kanban_node_runs SET started_at = ?, last_activity_at = ? WHERE id = ?")
        .bind(agentpipeline_core::storage::ts(long_ago))
        .bind(agentpipeline_core::storage::ts(long_ago))
        .bind(sub)
        .execute(h.store.pool())
        .await
        .unwrap();

    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert!(
        report.timed_out_runs.is_empty(),
        "子代理 run 不得进超时判定：{:?}",
        report.timed_out_runs
    );
    assert!(report.timeout_pending_cursors.is_empty());
    assert_eq!(h.resumes.load(Ordering::SeqCst), 0, "不得拉起伪重试");

    // 子代理行仍在自己名下为 running（它的生命周期由父节点的工具调用收尾）
    let runs = h.store.list_runs("t1").await.unwrap();
    let sub_row = runs.iter().find(|r| r.id == sub).unwrap();
    assert_eq!(sub_row.status, NodeStatus::Running);
}
