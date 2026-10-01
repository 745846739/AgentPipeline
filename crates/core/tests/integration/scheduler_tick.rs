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
    CursorStatus, Node, NodeStatus, PendingContext, PendingKind, PendingReason, ResumeCause, Stage,
    TaskStatus, TransitionTrigger,
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
    ///
    /// run 的 `(stage, node)` **取自那个游标自己**，不是写死的：`init` 任务的首个游标在
    /// `init.execute`，而按 `(task, stage, node)` 找 run 的读法（`stuck_evidence` 的
    /// 「owner 持有超时 / 处置未生效」两条）会因此一条都找不到——写死 develop.execute
    /// 时那两条判据永远返回「没卡住」，用例于是要么假绿、要么在断言处莫名地红。
    async fn running_run(
        &self,
        task_id: &str,
        cursor_id: &str,
        attempt: u32,
        started_secs_ago: i64,
        idle_secs_ago: i64,
        pgid: Option<i32>,
    ) -> i64 {
        let cursor = self.store.get_cursor(cursor_id).await.unwrap();
        let run_id = self
            .store
            .insert_run(&NewRun {
                task_id: task_id.into(),
                cursor_id: cursor_id.into(),
                stage: cursor.stage,
                node: cursor.node,
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
    ///
    /// 与 [`Self::running_run`] 同理：`(stage, node)` 取自游标——分位数是**按节点**分组的，
    /// 历史样本与那条正在跑的 run 落不到同一组，告警线就永远算不出来。
    async fn finished_run(&self, task_id: &str, cursor_id: &str, duration_ms: u64) -> i64 {
        let cursor = self.store.get_cursor(cursor_id).await.unwrap();
        let run_id = self
            .store
            .insert_run(&NewRun {
                task_id: task_id.into(),
                cursor_id: cursor_id.into(),
                stage: cursor.stage,
                node: cursor.node,
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

    /// 把游标推到 `develop.execute`。
    ///
    /// 超时分层（决策 66）与自适应分位数（票 17）都是**按 `(stage, node)`** 取配置与样本的，
    /// 而 `seed_task` 建的游标停在 `init.execute`。要测那几条读数，就得先让游标真的走到那个
    /// 节点上——「run 在 develop、游标在 init」是生产里不会出现的形状。
    async fn advance_to_develop(&self, cursor_id: &str) {
        self.store
            .set_cursor_stage(cursor_id, Stage::Develop, Node::Execute)
            .await
            .unwrap();
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
    h.advance_to_develop(&cursor.cursor_id).await;
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
    h.advance_to_develop(&cursor.cursor_id).await;
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
    h.advance_to_develop(&cursor.cursor_id).await;
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

/// 慢跑告警要**落一条待办**（决策 209② 的事件清单：只播报不唤醒）。
///
/// 这一条此前只有日志——`SlowRun` 这个类别与它的 `wakes() == false` 都写好了，却没有任何
/// 生产者，「值班长该知道这件事」在实际运行里没有出口。落表的口径是**一条 run 一行**
/// （`occurred_at` 取 run 的开始时刻）：tick 每 10s 一次，用 `now` 的话同一条慢跑会每
/// 10 秒落一行。
#[tokio::test]
async fn a_slow_run_is_noted_exactly_once_per_run() {
    let h = Harness::new().await;
    h.seed_task("t1").await;
    h.mark_running("t1").await;
    let cursor = h.store.load_live_cursors("t1").await.unwrap()[0].clone();
    h.advance_to_develop(&cursor.cursor_id).await;
    for _ in 0..5 {
        h.finished_run("t1", &cursor.cursor_id, 1_000).await;
    }
    // 已跑 10s（> 3×P90），心跳新鲜（不触发空闲超时）
    h.running_run("t1", &cursor.cursor_id, 1, 10, 0, None).await;
    let settings = Settings {
        adaptive_timeout_enabled: true,
        ..Default::default()
    };

    let report = h.scheduler(settings.clone()).tick().await.unwrap();
    assert_eq!(report.slow_run_alerts.len(), 1, "先有告警");
    assert_eq!(report.attention_noted, 1, "告警同时落一条待办");
    let open = h.store.open_attention(100).await.unwrap();
    assert_eq!(open.len(), 1, "{open:?}");
    assert_eq!(
        open[0].kind,
        agentpipeline_core::storage::AttentionKind::SlowRun
    );
    assert_eq!(open[0].task_id, "t1");
    assert!(!open[0].kind.wakes(), "它不是把人叫醒的那一类");

    // 再 tick 两趟：同一条 run 不重复落行（每 tick 一次会变成刷屏）
    h.scheduler(settings.clone()).tick().await.unwrap();
    h.scheduler(settings).tick().await.unwrap();
    assert_eq!(
        h.store.open_attention(100).await.unwrap().len(),
        1,
        "一条慢跑只落一行"
    );
}

// ─────────────────────── ① 超时（决策 33 / 64 / 66 / 100 / 122）───────────────────────

#[tokio::test]
async fn timeout_kills_process_group_and_retries_per_ladder() {
    let h = Harness::new().await;
    h.seed_task("t1").await;
    h.mark_running("t1").await;
    let cursor = h.store.load_live_cursors("t1").await.unwrap()[0].clone();
    // 游标推到 develop.execute：续接标记只对 agent 节点置位（`take_continuation`
    // 只在 agent 节点入口读，纯代码节点没有转录可言）。
    h.advance_to_develop(&cursor.cursor_id).await;

    // 连续超时第 1 次：杀进程组 + 自动续接（不再看 agent_retry_max）
    h.running_run("t1", &cursor.cursor_id, 1, 400, 400, Some(4242))
        .await;
    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert_eq!(report.timed_out_runs.len(), 1);
    assert_eq!(h.killer.killed_groups(), vec![4242], "必须杀整个进程组");
    assert!(
        report.timeout_pending_cursors.is_empty(),
        "未到梯子末端不得 pending"
    );
    assert_eq!(
        h.resumes.load(Ordering::SeqCst),
        1,
        "应拉起 executor 自动续接"
    );
    assert_eq!(
        h.store
            .take_cursor_resume_cause(&cursor.cursor_id)
            .await
            .unwrap(),
        Some(ResumeCause::Timeout),
        "续接标记要落在游标上：重试起的那条 run 带上一轮转录"
    );

    // 连续超时第 2 次：仍续接（attempt 数字不再参与判断，决策 320 写死 2 次）
    h.running_run("t1", &cursor.cursor_id, 2, 400, 400, Some(4243))
        .await;
    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert_eq!(report.timed_out_runs.len(), 1);
    assert_eq!(h.resumes.load(Ordering::SeqCst), 2);
    assert_eq!(
        h.store
            .take_cursor_resume_cause(&cursor.cursor_id)
            .await
            .unwrap(),
        Some(ResumeCause::Timeout)
    );

    // 连续超时第 3 次：降级空白重跑——resume 但**不**置续接标记
    h.running_run("t1", &cursor.cursor_id, 3, 400, 400, Some(4244))
        .await;
    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert_eq!(report.timed_out_runs.len(), 1);
    assert_eq!(h.resumes.load(Ordering::SeqCst), 3);
    assert_eq!(
        h.store
            .take_cursor_resume_cause(&cursor.cursor_id)
            .await
            .unwrap(),
        None,
        "空白重跑不带上一轮转录"
    );

    // 连续超时第 4 次：挂起 pending(timeout) 交回人工
    h.running_run("t1", &cursor.cursor_id, 4, 400, 400, Some(4245))
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
}

// ─────────────────── 票 02①：梯子的计数口径与陈旧 run ───────────────────

/// 该节点尾部连续超时的轮数（梯子的计数口）。
async fn streak(h: &Harness, task_id: &str, cursor_id: &str) -> u32 {
    let cursor = h.store.get_cursor(cursor_id).await.unwrap();
    h.store
        .trailing_timeout_streak(task_id, cursor.stage, cursor.node)
        .await
        .unwrap()
}

/// 造一条「被中止」的 run：来路由调用方指定。
async fn cancelled_run(
    h: &Harness,
    task_id: &str,
    cursor_id: &str,
    attempt: u32,
    origin: &'static str,
) {
    let run = h.running_run(task_id, cursor_id, attempt, 5, 5, None).await;
    h.store
        .finish_run(
            run,
            &RunOutcome {
                status: Some(NodeStatus::Cancelled),
                error: Some(format!("{task_id} 的这一轮已按中止请求收口")),
                cancel_origin: Some(origin),
                ..Default::default()
            },
        )
        .await
        .unwrap();
}

/// 判超时**自己造出来的**中止行不许把梯子清零（票 02①）。
///
/// 现场形状（2026-09-30）：陈旧 run 的尸检判了超时 → 按任务键中止**活着的那一轮** →
/// 那一轮落一条 `cancelled`，而它 id 更大，尾部连续超时就此归零——梯子连续 7 次停在
/// 第一档，`BlankRestart` 与 `Pending` 两档从未到达。
#[tokio::test]
async fn the_ladder_still_climbs_across_a_timeout_originated_cancel() {
    let h = Harness::new().await;
    h.seed_task("t-skip").await;
    h.mark_running("t-skip").await;
    let cursor = h.store.load_live_cursors("t-skip").await.unwrap()[0].clone();
    h.advance_to_develop(&cursor.cursor_id).await;

    // 第 1 次真超时 → 续接
    h.running_run("t-skip", &cursor.cursor_id, 1, 400, 400, None)
        .await;
    h.scheduler(Settings::default()).tick().await.unwrap();
    assert_eq!(streak(&h, "t-skip", &cursor.cursor_id).await, 1);

    // 判超时顺手掐掉的那一轮：中止行，来路 = 节点超时
    cancelled_run(
        &h,
        "t-skip",
        &cursor.cursor_id,
        2,
        agentpipeline_core::storage::observability::CANCEL_ORIGIN_TIMEOUT,
    )
    .await;
    assert_eq!(
        streak(&h, "t-skip", &cursor.cursor_id).await,
        1,
        "超时自己造出来的中止行既不计数也不清零"
    );

    // 第 2、3、4 次：梯子必须接着往上爬（旧口径被那条 cancelled 清零，永远停在 1）
    for round in 3..=5u32 {
        h.running_run("t-skip", &cursor.cursor_id, round, 400, 400, None)
            .await;
        let report = h.scheduler(Settings::default()).tick().await.unwrap();
        assert_eq!(report.timed_out_runs.len(), 1, "第 {round} 轮应判超时");
        let expected = round - 1;
        assert_eq!(
            streak(&h, "t-skip", &cursor.cursor_id).await,
            expected,
            "第 {round} 轮之后应数到连续 {expected} 次"
        );
        if expected < 4 {
            assert!(
                report.timeout_pending_cursors.is_empty(),
                "连续第 {expected} 次不该挂起"
            );
        }
    }
    // 第 4 档真的到了（旧口径下这一档永远到不了）
    assert_eq!(
        h.store.get_cursor(&cursor.cursor_id).await.unwrap().status,
        CursorStatus::Pending,
        "连续第 4 次超时要挂起交回人工"
    );
    assert_eq!(
        h.store
            .get_cursor(&cursor.cursor_id)
            .await
            .unwrap()
            .pending_reason
            .as_ref()
            .unwrap()
            .kind,
        PendingKind::Timeout
    );
}

/// **人按停**照旧清零（票 02①）：那条语义不许被顺手改掉——人是对该节点的新一轮介入，
/// 梯子重新起算。
#[tokio::test]
async fn a_human_cancel_still_resets_the_ladder() {
    let h = Harness::new().await;
    h.seed_task("t-hold").await;
    h.mark_running("t-hold").await;
    let cursor = h.store.load_live_cursors("t-hold").await.unwrap()[0].clone();
    h.advance_to_develop(&cursor.cursor_id).await;

    h.running_run("t-hold", &cursor.cursor_id, 1, 400, 400, None)
        .await;
    h.scheduler(Settings::default()).tick().await.unwrap();
    assert_eq!(streak(&h, "t-hold", &cursor.cursor_id).await, 1);

    cancelled_run(
        &h,
        "t-hold",
        &cursor.cursor_id,
        2,
        agentpipeline_core::storage::observability::CANCEL_ORIGIN_HOLD,
    )
    .await;

    // 再超时一次：从人的介入之后重新起算 → 1
    h.running_run("t-hold", &cursor.cursor_id, 3, 400, 400, None)
        .await;
    h.scheduler(Settings::default()).tick().await.unwrap();
    assert_eq!(
        streak(&h, "t-hold", &cursor.cursor_id).await,
        1,
        "人按停是真介入，梯子清零重算"
    );
    assert_eq!(h.resumes.load(Ordering::SeqCst), 2, "第一档仍是自动续接");
}

/// **陈旧的 run**（同一个游标上已经有更新的一轮在跑）判超时**只收终态**：
/// 不掐当前在跑的那一轮、不摘执行权、不拉起新 attempt（票 02①）。
///
/// 这是 2026-09-30 那条链的直接形状：attempt 2 比 attempt 1 的判死**早 10 秒起跑**，
/// 而调度器对陈旧的 attempt 1 判超时后按任务键中止，把活的 attempt 2 掐死了。
#[tokio::test]
async fn a_stale_timeout_only_closes_the_row_and_leaves_the_live_attempt_alone() {
    let h = Harness::new().await;
    h.seed_task("t-stale").await;
    h.mark_running("t-stale").await;
    let cursor = h.store.load_live_cursors("t-stale").await.unwrap()[0].clone();
    h.advance_to_develop(&cursor.cursor_id).await;
    // 抢占执行器：陈旧 run 的处置若不越权，这一格就得原样留着
    assert!(h
        .store
        .try_claim_executor("t-stale", "owner-live")
        .await
        .unwrap());

    let stale = h
        .running_run("t-stale", &cursor.cursor_id, 1, 400, 400, None)
        .await;
    let live = h
        .running_run("t-stale", &cursor.cursor_id, 2, 1, 0, None)
        .await;

    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert_eq!(
        report.timed_out_runs,
        Vec::<i64>::new(),
        "陈旧的那条不走处置路径"
    );
    assert_eq!(report.stale_timeout_runs, vec![stale]);

    assert_eq!(
        h.store.get_run(stale).await.unwrap().unwrap().status,
        NodeStatus::Timeout,
        "尸检结论照记"
    );
    assert_eq!(
        h.store.get_run(live).await.unwrap().unwrap().status,
        NodeStatus::Running,
        "当前在跑的那一轮一个字都不许动"
    );
    assert_eq!(h.resumes.load(Ordering::SeqCst), 0, "不许拉起新 attempt");
    assert_eq!(h.killer.killed_groups(), Vec::<i32>::new());
    assert_eq!(
        h.store
            .get_task("t-stale")
            .await
            .unwrap()
            .executor_owner
            .as_deref(),
        Some("owner-live"),
        "不许摘掉当前执行体的执行权"
    );
}

/// 纯代码节点也走满三段梯子（决策 320）：没有转录可续，前两档退化为空白重跑、
/// 不置续接标记，第 4 次才挂起——**不是第一次超时就交回人工**。
///
/// 评审实错的回归：初版把「置位」的 agent 门控写进了分支条件本身，纯代码节点
/// `streak <= 2` 两档全落进 else、首次超时即 pending，比旧口径
/// （`run.attempt < agent_retry_max` 耗尽才挂）还倒退。
#[tokio::test]
async fn non_agent_node_ladders_blank_retries_then_pends() {
    let h = Harness::new().await;
    h.seed_task("t-code").await;
    h.mark_running("t-code").await;
    let cursor = h.store.load_live_cursors("t-code").await.unwrap()[0].clone();
    // develop.validate_output 不在 agent 节点表里（`AgentNodeKind::of` → None）
    h.store
        .set_cursor_stage(&cursor.cursor_id, Stage::Develop, Node::ValidateOutput)
        .await
        .unwrap();

    for round in 1..=4u32 {
        h.running_run("t-code", &cursor.cursor_id, round, 400, 400, None)
            .await;
        let report = h.scheduler(Settings::default()).tick().await.unwrap();
        assert_eq!(report.timed_out_runs.len(), 1, "第 {round} 轮应判超时");
        if round < 4 {
            assert!(
                report.timeout_pending_cursors.is_empty(),
                "第 {round} 次超时不得挂起：纯代码节点也走梯子"
            );
            assert_eq!(
                h.resumes.load(Ordering::SeqCst),
                round as usize,
                "第 {round} 次超时应当重跑"
            );
            assert_eq!(
                h.store
                    .take_cursor_resume_cause(&cursor.cursor_id)
                    .await
                    .unwrap(),
                None,
                "纯代码节点没有转录可续，不置标记"
            );
        } else {
            assert_eq!(
                report.timeout_pending_cursors,
                vec![cursor.cursor_id.clone()],
                "连续第 4 次超时才挂起"
            );
        }
    }
}

/// 判超时要照实记**跑了多久**（决策 226）。
///
/// 此前这条路径不带 `duration_ms`，于是「跑了 8 小时 51 分」「5 分 10 秒」这种判读只能由
/// 读的人拿 `started_at` 与自己心算相减——2026-09-19 值班长就是这么算的，而台账里摆着的
/// `duration_ms` 是 0。一个既是读数又是哨兵的字段，读的人只能猜。
#[tokio::test]
async fn a_timed_out_run_records_how_long_it_ran() {
    let h = Harness::new().await;
    h.seed_task("t1").await;
    h.mark_running("t1").await;
    let cursor = h.store.load_live_cursors("t1").await.unwrap()[0].clone();
    // 起跑在 311 秒前、心跳停在 400 秒前 → 空闲超时（默认 300s）
    let run_id = h
        .running_run("t1", &cursor.cursor_id, 1, 311, 400, None)
        .await;

    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert_eq!(report.timed_out_runs, vec![run_id]);

    let run = h
        .store
        .list_runs("t1")
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.id == run_id)
        .expect("那条 run 应当在台账里");
    assert_eq!(run.status, NodeStatus::Timeout);
    assert_eq!(run.duration_ms, 311_000, "时长要照实记，不能留 0");
}

// ─────────────────── 票 05：发现落表（决策 209③）───────────────────

/// 造一条**已终态**的 run，并把 `finished_at` 回拨（游标仍 active = 处置没生效）。
async fn terminal_run_at_init(
    h: &Harness,
    task_id: &str,
    status: NodeStatus,
    finished_minutes_ago: i64,
) -> i64 {
    let cursor = h.store.load_live_cursors(task_id).await.unwrap()[0].clone();
    let run_id = h
        .store
        .insert_run(&NewRun {
            task_id: task_id.into(),
            cursor_id: cursor.cursor_id.clone(),
            stage: Stage::Init,
            node: Node::Execute,
            attempt: 1,
            agent_type: "system".into(),
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
                status: Some(status),
                error: Some("init.execute 超时，当时在「检查项目工作区是否脏」".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let finished = h.clock.now() - chrono::Duration::minutes(finished_minutes_ago);
    sqlx::query("UPDATE kanban_node_runs SET finished_at = ? WHERE id = ?")
        .bind(agentpipeline_core::storage::ts(finished))
        .bind(run_id)
        .execute(h.store.pool())
        .await
        .unwrap();
    run_id
}

/// 「调度器处置未生效」——这一票的重点，且此前**零信号**。
///
/// 实测（2026-09-17）：run 已被标 timeout、transition 也写了「干净对话重试」，但没有
/// attempt-2 的 run 行，任务从此停在 running——`check_timeouts` 只看 active_runs()，
/// `remind_pending_tasks` 的 stalled 判据也不成立。既有调度器与既有台账之间的这条缝。
#[tokio::test]
async fn a_terminal_run_behind_an_active_cursor_is_noted() {
    let h = Harness::new().await;
    h.seed_task("t1").await;
    h.mark_running("t1").await;
    let run_id = terminal_run_at_init(&h, "t1", NodeStatus::Timeout, 30).await;

    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    // 本 tick 记两件事：`scheduler_no_effect`（这一条的主人）与 `run_failed`
    // （决策 234——同一次终态失败在两个判据下各是一个事实：调度器的处置没生效、
    // 而这条调用确实挂了。两条待办在**一次唤醒**里合并播报，故不重复花钱）。
    assert_eq!(report.attention_noted, 2, "本 tick 新记两件事");

    let open = h.store.open_attention(100).await.unwrap();
    let effect = open
        .iter()
        .find(|i| i.kind == agentpipeline_core::storage::AttentionKind::SchedulerNoEffect)
        .expect("调度器处置未生效那一条要在");
    assert_eq!(effect.task_id, "t1");
    assert_eq!(effect.detail_json.as_ref().unwrap()["run_id"], run_id);
    assert!(effect.consumed_at.is_none(), "没有人处理过它");
}

/// 宽限期内不报 `scheduler_no_effect`：run 刚失败、重试还没起来的那个瞬间是**正常**的中间态。
///
/// 但**失败本身要报**（决策 234）：`run_failed` 的判据是「落终态失败且任务没转 pending」，
/// 它没有宽限期——2026-09-19 实测里正是因为重试立刻起了新 run（游标始终 active、任务始终
/// running），`scheduler_no_effect` 那条缝**永远不成立**，于是三次失败烧掉一千万 token
/// 而值守一次没醒。故这一条同时钉住两件事：宽限期内不报 `scheduler_no_effect`，
/// 而 `run_failed` 当场就在。
#[tokio::test]
async fn a_fresh_terminal_run_is_not_yet_a_scheduler_no_effect() {
    let h = Harness::new().await;
    h.seed_task("t1").await;
    h.mark_running("t1").await;
    terminal_run_at_init(&h, "t1", NodeStatus::Failed, 1).await;

    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert_eq!(report.attention_noted, 1, "只有 run_failed 那一条");
    let kinds: Vec<_> = h
        .store
        .open_attention(100)
        .await
        .unwrap()
        .iter()
        .map(|i| i.kind)
        .collect();
    assert_eq!(
        kinds,
        vec![agentpipeline_core::storage::AttentionKind::RunFailed],
        "1 分钟前刚失败：还在正常重试窗口里（不报处置未生效），但失败本身要看得见"
    );
}

/// 心跳停跳的 owner：任务 running、有主、run 还挂着但长时间没有活动。
///
/// **这条只在「该节点的空闲超时比 owner 停跳线更长」时才可能成立**：空闲超时若先到
/// （缺省 300s < 停跳线 10 分钟），超时清扫会先把 run 标终态——那时该报的是
/// 「调度器处置未生效」，不是这一条。OwnerStuck 管的是另一种局面：清扫暂时不会来
/// （长跑节点把空闲超时按小时配），而主已经不在了。用例因此按长跑节点配。
#[tokio::test]
async fn an_owner_held_run_without_heartbeat_is_noted() {
    let h = Harness::new().await;
    h.seed_task("t1").await;
    h.mark_running("t1").await;
    assert!(h.store.try_claim_executor("t1", "owner-1").await.unwrap());
    let cursor = h.store.load_live_cursors("t1").await.unwrap()[0].clone();
    // 已跑 900s、心跳停在 900s 前（远过 watch_owner_stuck_minutes = 10）
    h.running_run("t1", &cursor.cursor_id, 1, 900, 900, None)
        .await;

    let settings = Settings {
        node_idle_timeout_sec: 7200, // 长跑节点：空闲超时不会先来收走这一条
        ..Default::default()
    };
    let report = h.scheduler(settings).tick().await.unwrap();
    assert!(report.attention_noted >= 1);
    let open = h.store.open_attention(100).await.unwrap();
    assert!(
        open.iter()
            .any(|a| a.kind == agentpipeline_core::storage::AttentionKind::OwnerStuck),
        "{open:?}"
    );
}

/// **第三类判据（决策 305，票 04）**：游标 **`pending`** + run 已终态 + 执行权仍持有。
///
/// 这是 2026-09-27 实测的形态（任务 `01M3BGVCXDWFPT0Q3BZYAGZP8Q`）。旧判据两条都不覆盖：
/// 「调度器处置未生效」被「游标必须可运行」先行挡掉，「owner 持有超时」要求 run 仍 `Running`
/// ——于是兜底机制在它本该兜的那一格失效，只能靠重启。断言的是**这一格被认出来了**。
#[tokio::test]
async fn a_held_owner_behind_a_pending_cursor_is_noted() {
    let h = Harness::new().await;
    h.seed_task("t1").await;
    h.mark_running("t1").await;
    assert!(h.store.try_claim_executor("t1", "owner-1").await.unwrap());
    let cursor = h.store.load_live_cursors("t1").await.unwrap()[0].clone();
    // run 30 分钟前就终态了（远过 watch_owner_stuck_minutes = 10），而执行权还在 owner-1 手上
    terminal_run_at_init(&h, "t1", NodeStatus::Timeout, 30).await;
    // 超时耗尽了重试预算 → 调度器把游标挂成了 pending（正是那次实测的形状）
    let reason = PendingReason::new(
        PendingKind::Timeout,
        cursor.stage,
        cursor.node,
        "执行超时（attempt 3）",
    );
    h.store
        .set_cursor_pending(&cursor.cursor_id, &reason)
        .await
        .unwrap();
    h.store.sync_task_projection("t1").await.unwrap();

    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert!(report.attention_noted >= 1, "{report:?}");
    let open = h.store.open_attention(100).await.unwrap();
    let noted = open
        .iter()
        .find(|a| a.kind == agentpipeline_core::storage::AttentionKind::OwnerStuck)
        .unwrap_or_else(|| {
            panic!("游标 pending + run 已终态 + 执行权仍持有必须被认出来：{open:?}")
        });
    assert_eq!(
        noted.detail_json.as_ref().unwrap()["shape"],
        "terminal_run",
        "第三类的这个分支要标出来（run 已终态）"
    );
    assert_eq!(noted.detail_json.as_ref().unwrap()["owner"], "owner-1");
}

/// **第三类判据的第三个形态**：已抢占执行权、**还没有 run 行**，而执行权攥着超过宽限。
///
/// 这一格旧判据同样看不到（连 run 都没有，`list_runs_at` 返回空、旧代码 `continue` 掉）。
#[tokio::test]
async fn a_held_owner_with_no_run_at_all_is_noted() {
    let h = Harness::new().await;
    h.seed_task("t1").await;
    h.mark_running("t1").await;
    assert!(h.store.try_claim_executor("t1", "owner-1").await.unwrap());
    // 持有权的换手会碰 `updated_at`，故回拨它就是回拨「攥了多久」（决策 305 的读法）。
    let held = h.clock.now() - chrono::Duration::minutes(30);
    sqlx::query("UPDATE kanban_tasks SET updated_at = ? WHERE id = ?")
        .bind(agentpipeline_core::storage::ts(held))
        .bind("t1")
        .execute(h.store.pool())
        .await
        .unwrap();

    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert!(report.attention_noted >= 1, "{report:?}");
    let open = h.store.open_attention(100).await.unwrap();
    let noted = open
        .iter()
        .find(|a| a.kind == agentpipeline_core::storage::AttentionKind::OwnerStuck)
        .unwrap_or_else(|| panic!("尚无 run 也应被认出来：{open:?}"));
    assert_eq!(
        noted.detail_json.as_ref().unwrap()["shape"],
        "no_run",
        "第三类的这个分支要标出来（还没有 run 行）"
    );
}

/// **反向断言（票 04）**：有主、心跳**新鲜**、未超阈值的在跑任务**不被误判**。
///
/// 这正是 `unstick` 文件头那句警告——判成卡住会清掉健康占用，同一任务就会跑出两个执行体。
#[tokio::test]
async fn an_owner_held_run_with_a_fresh_heartbeat_is_not_misjudged() {
    let h = Harness::new().await;
    h.seed_task("t1").await;
    h.mark_running("t1").await;
    assert!(h
        .store
        .try_claim_executor("t1", "owner-live")
        .await
        .unwrap());
    let cursor = h.store.load_live_cursors("t1").await.unwrap()[0].clone();
    // 跑了 900s（远长于 10 分钟那条线）但心跳 5 秒前刚刷过 —— 「跑得久」不是判据。
    h.running_run("t1", &cursor.cursor_id, 1, 900, 5, None)
        .await;

    let settings = Settings {
        node_idle_timeout_sec: 7200, // 长跑节点：空闲超时不会先来收走这一条
        ..Default::default()
    };
    let report = h.scheduler(settings).tick().await.unwrap();
    let open = h.store.open_attention(100).await.unwrap();
    assert!(
        !open
            .iter()
            .any(|a| a.kind == agentpipeline_core::storage::AttentionKind::OwnerStuck),
        "心跳在走 = 正常在跑，不许误判（noted={}）：{open:?}",
        report.attention_noted
    );
    assert_eq!(
        h.store
            .get_task("t1")
            .await
            .unwrap()
            .executor_owner
            .as_deref(),
        Some("owner-live"),
        "占用一个字都不许动"
    );
}

/// §2.1 那条纪律的牙齿：**正常在跑的东西不产生待办**（成功、心跳、工具调用都不写）。
#[tokio::test]
async fn healthy_activity_produces_no_attention() {
    let h = Harness::new().await;
    h.seed_task("t1").await;
    h.mark_running("t1").await;
    let cursor = h.store.load_live_cursors("t1").await.unwrap()[0].clone();
    // 正在跑、心跳新鲜、没有 owner（也照样不该报 owner_stuck）
    h.running_run("t1", &cursor.cursor_id, 1, 5, 5, None).await;

    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert_eq!(report.attention_noted, 0, "在跑的任务不该被写进待办");
    assert!(h.store.open_attention(100).await.unwrap().is_empty());
}

/// 一 tick 内多个事件 → **只写多行**（唤醒在票 06，一次）；且同一件事不会每 tick 重写。
#[tokio::test]
async fn many_discoveries_note_rows_once_each() {
    let h = Harness::new().await;
    h.seed_task("t1").await;
    h.mark_running("t1").await;
    terminal_run_at_init(&h, "t1", NodeStatus::Timeout, 30).await;
    h.seed_task("t2").await;
    h.mark_running("t2").await;
    terminal_run_at_init(&h, "t2", NodeStatus::Timeout, 40).await;

    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    // 三件：t1（30 分钟前超时，在发现窗口的边界上）两条都对得上——处置未生效 + run_failed；
    // t2（40 分钟前）已经出了 30 分钟的**发现窗口**，只剩处置未生效那一条。
    // 与其余发现项共用同一个窗口：窗口是「还值得叫醒的时距」，逐类各配一个是另一件事。
    assert_eq!(report.attention_noted, 3, "t1 两件 + t2 一件");
    assert_eq!(h.store.open_attention(100).await.unwrap().len(), 3);

    // 第二次 tick：同一件事（同一 occurred_at）不再写第二行——否则唤醒会被自己的重试刷屏
    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert_eq!(report.attention_noted, 0);
    assert_eq!(h.store.open_attention(100).await.unwrap().len(), 3);
}

/// 停滞提醒此前**从不推 SSE**（只在内存集合里记一笔）——这是本票顺手修掉的既有缺陷。
#[tokio::test]
async fn a_repeated_reminder_reaches_the_frontend() {
    let h = Harness::new().await;
    h.seed_task("t1").await;
    park_pending_aged(&h, "t1", 30).await;

    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert_eq!(report.reminded, vec!["t1".to_string()]);
    assert!(
        h.sse.count_of(SseEventType::Stalled) >= 1,
        "停滞提醒要走 SSE（前端按它置看板高亮）"
    );
    let open = h.store.open_attention(100).await.unwrap();
    assert!(
        open.iter()
            .any(|a| a.kind == agentpipeline_core::storage::AttentionKind::TaskStale),
        "停了 24h 还没人管的 pending 必须落表（恰恰是「只报新鲜事」会漏掉的那一类）：{open:?}"
    );
}

/// 把任务推成 pending 并把游标 `updated_at` 回拨（提醒判据按它算年龄）。
async fn park_pending_aged(h: &Harness, task_id: &str, hours_ago: i64) {
    let cursor = h.store.load_live_cursors(task_id).await.unwrap()[0].clone();
    let reason = PendingReason::new(
        PendingKind::UserDecision,
        cursor.stage,
        cursor.node,
        "等你拍板",
    );
    h.store
        .set_cursor_pending(&cursor.cursor_id, &reason)
        .await
        .unwrap();
    h.store.sync_task_projection(task_id).await.unwrap();
    let updated = h.clock.now() - chrono::Duration::hours(hours_ago);
    sqlx::query("UPDATE kanban_node_cursors SET updated_at = ? WHERE cursor_id = ?")
        .bind(agentpipeline_core::storage::ts(updated))
        .bind(&cursor.cursor_id)
        .execute(h.store.pool())
        .await
        .unwrap();
}

// ─────────────────────── 票 04：超时要说清「当时在哪一步」───────────────────────

/// 卡住的**系统节点**：台账里不能只有一句「超时」。
///
/// 实证（2026-09-17）：`支持rtk` 的 init run 卡了四小时，要知道卡在哪个系统调用只能拿
/// 外部 `sample` 附进程——服务自己一个字都没说。
#[tokio::test]
async fn a_timed_out_system_run_names_the_step_it_was_on() {
    let h = Harness::new().await;
    h.seed_task("t1").await;
    h.mark_running("t1").await;
    let cursor = h.store.load_live_cursors("t1").await.unwrap()[0].clone();
    // 梯子（决策 320）：挂起在**连续第 4 次**超时——垫三条已终态的超时 run，让这一轮
    // 落在挂起支上（信息量最完整的那个出口）。attempt 不再参与判断：旧口径
    // 「attempt 3 = agent_retry_max → 耗尽即挂」由 320 的连续超时计数取代。
    for attempt in 1..=3 {
        let seeded = h
            .running_run("t1", &cursor.cursor_id, attempt, 400, 400, None)
            .await;
        h.store
            .finish_run(
                seeded,
                &RunOutcome {
                    status: Some(NodeStatus::Timeout),
                    duration_ms: 400_000,
                    error: Some("垫：连续超时".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }
    let run_id = h
        .running_run("t1", &cursor.cursor_id, 4, 400, 400, Some(4242))
        .await;
    h.store
        .set_run_step(run_id, "检查项目工作区是否脏")
        .await
        .unwrap();

    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert_eq!(report.timed_out_runs, vec![run_id]);

    let runs = h.store.list_runs("t1").await.unwrap();
    let run = runs.iter().find(|r| r.id == run_id).unwrap();
    assert_eq!(run.status, NodeStatus::Timeout);
    assert!(
        run.error
            .as_deref()
            .unwrap_or_default()
            .contains("检查项目工作区是否脏"),
        "超时的 run 行要写清当时在哪一步：{:?}",
        run.error
    );
    let after = h.store.get_cursor(&cursor.cursor_id).await.unwrap();
    assert!(
        after
            .pending_reason
            .as_ref()
            .unwrap()
            .message
            .contains("检查项目工作区是否脏"),
        "pending 的那句话同样要带上：{:?}",
        after.pending_reason
    );
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
    // 分层读的是 run 所在那个节点的配置，所以它得真的在 develop.execute 上
    h.advance_to_develop(&cursor.cursor_id).await;

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

    // 梯子（决策 320）：连续超时第 4 次才挂起——垫三条已终态的超时 run，让这一轮
    // 的 run 落在挂起支上（这条用例验的是「挂起只作用于该分支」，不是梯子本身）。
    for attempt in 1..=3 {
        let id = h
            .running_run("t1", &dev.cursor_id, attempt, 400, 400, None)
            .await;
        h.store
            .finish_run(
                id,
                &RunOutcome {
                    status: Some(NodeStatus::Timeout),
                    duration_ms: 400_000,
                    error: Some("垫：连续超时".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }
    h.running_run("t1", &dev.cursor_id, 4, 400, 400, Some(9))
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
            None,
            200,
            100,
            None,
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

// ─────────── 项目级 run 的终止者（决策 212 / 票 13）───────────

/// 僵尸行：项目级 run 心跳停了就标终态——它们跨重启永生的那条路（两条既有路径都不认它们）。
#[tokio::test]
async fn a_stale_project_run_is_abandoned() {
    use agentpipeline_core::storage::observability::NewProjectRun;

    let h = Harness::new().await;
    // 项目在 Harness::new 里已经 seed 过（p1）
    let run_id = h
        .store
        .insert_project_run(&NewProjectRun {
            project_id: "p1".into(),
            stage: Stage::Init,
            node: Node::Execute,
            attempt: 1,
            agent_type: "pseudo:project_analysis".into(),
        })
        .await
        .unwrap();
    // 心跳冻结在 1 小时前（`project_run_idle_timeout_sec` 默认 900s）
    let frozen = h.clock.now() - chrono::Duration::hours(1);
    sqlx::query("UPDATE kanban_node_runs SET started_at = ?, last_activity_at = ? WHERE id = ?")
        .bind(agentpipeline_core::storage::ts(frozen))
        .bind(agentpipeline_core::storage::ts(frozen))
        .bind(run_id)
        .execute(h.store.pool())
        .await
        .unwrap();

    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert_eq!(report.abandoned_project_runs, vec![run_id]);

    let runs = h.store.list_project_runs("p1").await.unwrap();
    let run = runs.iter().find(|r| r.id == run_id).unwrap();
    assert_eq!(run.status, NodeStatus::Timeout);
    assert!(
        run.error
            .as_deref()
            .unwrap_or_default()
            .contains("心跳停止"),
        "原因要可读：{:?}",
        run.error
    );
}

/// 活着的项目级 run 不误伤（心跳新鲜 → 一次都不标）。
#[tokio::test]
async fn a_live_project_run_is_left_alone() {
    use agentpipeline_core::storage::observability::NewProjectRun;

    let h = Harness::new().await;
    let run_id = h
        .store
        .insert_project_run(&NewProjectRun {
            project_id: "p1".into(),
            stage: Stage::Init,
            node: Node::Execute,
            attempt: 1,
            agent_type: "pseudo:project_analysis".into(),
        })
        .await
        .unwrap();
    h.store.touch_run_heartbeat(run_id).await.unwrap();

    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert!(report.abandoned_project_runs.is_empty());
    let runs = h.store.list_project_runs("p1").await.unwrap();
    assert_eq!(runs[0].status, NodeStatus::Running);
}

/// 重启后不存在 running 的项目级 run（决策 212 的硬要求）。
#[tokio::test]
async fn a_restart_leaves_no_running_project_run() {
    use agentpipeline_core::storage::observability::NewProjectRun;

    let h = Harness::new().await;
    for _ in 0..3 {
        h.store
            .insert_project_run(&NewProjectRun {
                project_id: "p1".into(),
                stage: Stage::Init,
                node: Node::Execute,
                attempt: 1,
                agent_type: "pseudo:project_analysis".into(),
            })
            .await
            .unwrap();
    }
    let abandoned = h.store.abandon_stale_project_runs().await.unwrap();
    assert_eq!(abandoned.len(), 3, "重启那一刻它们都是孤儿");
    assert!(h
        .store
        .list_project_runs("p1")
        .await
        .unwrap()
        .iter()
        .all(|r| r.status == NodeStatus::Timeout));
}

/// 重启后不存在 running 的**任务级** run（票 02②）：与项目级那条是同一件事的两半。
///
/// 此前任务自己的遗留 run 谁都不管（归队只翻任务行，`abandon_stale_project_runs` 限定
/// `task_id IS NULL`），它们要等 idle 超时（默认 300s）被判死——`duration_ms` 记成
/// 「从起跑到判死」，读起来像「跑了这么久才超时」，而每判死一条又制造一个中止行，
/// 把超时梯子的计数搅乱（2026-09-30 的 13 次重启每次都留一批）。
#[tokio::test]
async fn a_restart_closes_leftover_task_runs_without_impersonating_a_timeout() {
    let h = Harness::new().await;
    h.seed_task("t-boot").await;
    h.mark_running("t-boot").await;
    let cursor = h.store.load_live_cursors("t-boot").await.unwrap()[0].clone();
    let run_id = h
        .running_run("t-boot", &cursor.cursor_id, 1, 40, 40, None)
        .await;

    let readings = agentpipeline_core::pipeline::foreman_actions::run_recovery_sequence(&h.store)
        .await
        .unwrap();
    assert_eq!(readings.abandoned_task_runs, vec![run_id]);
    assert!(
        readings.requeued.contains(&"t-boot".to_string()),
        "同一次恢复也把任务归队：{:?}",
        readings.requeued
    );

    let run = h.store.get_run(run_id).await.unwrap().unwrap();
    assert_eq!(run.status, NodeStatus::Cancelled, "不是节点超时，别冒充");
    let error = run.error.as_deref().unwrap_or("");
    assert!(error.contains("进程重启"), "文案要如实写：{error}");
    assert!(!error.contains("超时"), "不许冒充节点超时：{error}");
    assert_eq!(
        run.duration_ms, 40_000,
        "时长照实记（决策 226）：从起跑到重启，不是 0"
    );

    // 它不进超时梯子的计数（来路是 restart 而非 timeout）
    assert_eq!(streak(&h, "t-boot", &cursor.cursor_id).await, 0);
}

// ───────────────── 待办补两类（决策 234，票 05）─────────────────

/// `run_failed`：一次 run 落 `failed` / `timeout`、而任务**没有因此转 pending**。
///
/// 这条缝此前零信号，而它最贵：实测里一条任务的三次 `architect-design.execute` 全失败、
/// 合计烧掉 1,050 万 prompt token，而值守轮**一次都没醒**——`scheduler_no_effect` 要求
/// run 终态而游标仍 active、`task_pending` 要求任务转 pending，两条都不成立。
#[tokio::test]
async fn a_failed_run_that_did_not_pend_the_task_is_noted() {
    let h = Harness::new().await;
    h.seed_task("t1").await;
    h.mark_running("t1").await;
    let run_id = terminal_run_at_init(&h, "t1", NodeStatus::Failed, 1).await;

    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert_eq!(report.attention_noted, 1);
    let open = h.store.open_attention(100).await.unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(
        open[0].kind,
        agentpipeline_core::storage::AttentionKind::RunFailed
    );
    assert!(open[0].kind.wakes(), "两类都唤醒（决策 234）");
    // 归位的入口：detail 要点名是哪一条 run（决策 230 的判据①）。
    let detail = open[0].detail_json.as_ref().expect("要带上 run 的身份");
    assert_eq!(detail["run_id"], run_id);
    assert_eq!(detail["stage"], "init");
    assert_eq!(detail["status"], "failed");

    // 同一件事不会每 tick 重写（occurred_at = 那一轮的 finished_at，去重键命中）。
    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert_eq!(report.attention_noted, 0);
    assert_eq!(h.store.open_attention(100).await.unwrap().len(), 1);
}

/// 重试型故障：**每一条失败的 run 各一行**（事件是逐 run 的），而唤醒由节流收
/// ——30 分钟冷却那一条由 `foreman.rs` 的用例钉住（决策 234 点名）。
#[tokio::test]
async fn each_failed_attempt_of_a_retrying_task_is_a_row() {
    let h = Harness::new().await;
    h.seed_task("t1").await;
    h.mark_running("t1").await;
    for minutes in [3, 2, 1] {
        terminal_run_at_init(&h, "t1", NodeStatus::Failed, minutes).await;
    }

    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert_eq!(
        report.attention_noted, 3,
        "三次同形状失败 = 三件逐 run 的事件"
    );
    let open = h.store.open_attention(100).await.unwrap();
    assert_eq!(open.len(), 3);
    let run_ids: Vec<i64> = open
        .iter()
        .map(|i| i.detail_json.as_ref().unwrap()["run_id"].as_i64().unwrap())
        .collect();
    assert_eq!(
        run_ids.len(),
        run_ids
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        "每一次失败各自归位到它自己那条 run：{run_ids:?}"
    );

    // 同一刻的两次失败折叠成一行（去重键是 `(task, kind, occurred_at)`）：同一件事只写一行，
    // 而「两次失败的时长相同」在生产里罕见——真撞上了也只该播一次。
    let h2 = Harness::new().await;
    h2.seed_task("t2").await;
    h2.mark_running("t2").await;
    for _ in 0..2 {
        terminal_run_at_init(&h2, "t2", NodeStatus::Failed, 1).await;
    }
    h2.scheduler(Settings::default()).tick().await.unwrap();
    assert_eq!(h2.store.open_attention(100).await.unwrap().len(), 1);
}

/// 口径收窄的那一半：任务**自己转 pending** 时不记 `run_failed`
/// ——`task_pending` 那一条本来就会响，重复记只会让同一个故障占两条待办。
#[tokio::test]
async fn a_failed_run_that_pended_the_task_is_not_double_noted() {
    let h = Harness::new().await;
    h.seed_task("t1").await;
    h.mark_running("t1").await;
    park_pending_aged(&h, "t1", 0).await;
    terminal_run_at_init(&h, "t1", NodeStatus::Timeout, 1).await;

    let report = h.scheduler(Settings::default()).tick().await.unwrap();
    assert_eq!(report.attention_noted, 1, "只该有一条");
    let kinds: Vec<_> = h
        .store
        .open_attention(100)
        .await
        .unwrap()
        .iter()
        .map(|i| i.kind)
        .collect();
    assert!(
        !kinds.contains(&agentpipeline_core::storage::AttentionKind::RunFailed),
        "已经转 pending 的任务不再补一条 run_failed：{kinds:?}"
    );
}

/// `task_cancelled`：取消是**有人做了个决定**，而 09-18 那三条任务被一并标 cancelled 之后
/// 值守班次从 `01:25` 起再没被叫醒过（值班长只能报「是谁下的手，我没有证据」）。
///
/// 生产者不是 tick 而是取消本身（`cancel_task`），故这条用例不走 `scheduler.tick()`。
#[tokio::test]
async fn cancelling_a_task_leaves_a_waking_attention_row() {
    let h = Harness::new().await;
    h.seed_task("t1").await;

    h.store.cancel_task("t1").await.unwrap();

    let open = h.store.open_attention(100).await.unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(
        open[0].kind,
        agentpipeline_core::storage::AttentionKind::TaskCancelled
    );
    assert!(open[0].kind.wakes(), "两类都唤醒（决策 234）");
    assert_eq!(open[0].task_id, "t1");
}
