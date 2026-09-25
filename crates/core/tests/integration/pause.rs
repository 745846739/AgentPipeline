//! L2 集成：手动暂停 / 续跑 / 重跑本阶段（决策 276）。
//!
//! 三件东西的判据各在**一处分界**上，逐条钉住：
//! - **暂停**只对「已准入 + 有 active 游标」的任务生效，落在 `pending(user_paused)`；
//! - **续跑**不是新实现，而是 pending 动作表里 `continue` 那一颗走既有 resume 唯一实现；
//! - **重跑**落回**本阶段入口**，且那一轮不算（`user_rerun` → 不续接对话）。

use std::sync::Arc;
use std::time::Duration;

use agentpipeline_core::config::Settings;
use agentpipeline_core::pipeline::pause::{pause, rerun};
use agentpipeline_core::pipeline::resume::{apply_resume, ResumeRequest};
use agentpipeline_core::pipeline::Executor;
use agentpipeline_core::scheduler::ResumeFn;
use agentpipeline_core::sse::{SseEventType, SseSink};
use agentpipeline_core::storage::Store;
use agentpipeline_core::types::{
    CursorStatus, Node, NodeStatus, PendingKind, ResumeCause, Stage, TaskStatus, TransitionTrigger,
};
use testkit::{FakeAgent, ManualClock, RecordingKiller, Repo, Script, SseRecorder, TestHome};

struct Ctx {
    _home: TestHome,
    store: Store,
    clock: ManualClock,
    sse: SseRecorder,
    agent: FakeAgent,
    executor: Arc<Executor>,
}

impl Ctx {
    /// `&Arc<SseBus>` 在实参位不会自动收窄成 `&Arc<dyn SseSink>`，测试这边同样要立一个绑定。
    fn sse_sink(&self) -> Arc<dyn SseSink> {
        Arc::new(self.sse.clone())
    }
}

/// 建项目 + 任务，并把任务**推成已准入的 running**（暂停的前提）。
///
/// `task_id` 由用例各给一个：执行体注册表是**进程全局**的、按 task_id 去重（决策 36），
/// 而 `cargo test` 在同一个进程里并行跑本模块的用例——共用一个 id 会让两条用例互相看见
/// 对方在跑的 run（`executor.rs` 的既有约定同此）。
async fn setup(task_id: &str, script: Script) -> Ctx {
    let home = TestHome::new().unwrap();
    let clock = ManualClock::fixed();
    let store = Store::open(home.home().clone(), Arc::new(clock.clone()))
        .await
        .unwrap();
    let repo = Repo::clean().unwrap();
    let project = agentpipeline_core::types::Project {
        id: "p1".into(),
        name: "示例".into(),
        local_path: repo.path().display().to_string(),
        default_branch: "main".into(),
        language: None,
        // 系统闸门用原始命令 `true`：零噪声通过（同 executor.rs 的既有约定）
        test_framework: Some("true".into()),
        lint_command: None,
        agents_md_path: None,
        created_at: store.now(),
    };
    store.create_project(&project).await.unwrap();
    testkit::seed_task(&store, task_id, "p1").await.unwrap();
    store
        .set_task_status(task_id, TaskStatus::Running)
        .await
        .unwrap();

    let sse = SseRecorder::new();
    let agent = FakeAgent::new(script);
    let executor = Executor::new(
        store.clone(),
        Settings::default(),
        Arc::new(sse.clone()),
        Arc::new(agent.clone()),
        Arc::new(RecordingKiller::new()),
    );
    Ctx {
        _home: home,
        store,
        clock,
        sse,
        agent,
        executor: Arc::new(executor),
    }
}

/// 把游标挪到 `(stage, node)`（造出「正跑在别处」的现场）。
async fn move_cursor(store: &Store, task_id: &str, stage: Stage, node: Node) {
    let cursor = store.resolve_sole_cursor(task_id).await.unwrap().unwrap();
    store
        .set_cursor_stage(&cursor.cursor_id, stage, node)
        .await
        .unwrap();
}

/// 等一条 `running` 的 run 出现（执行体真的开始跑那一轮了）。
async fn wait_for_running_run(store: &Store) -> i64 {
    for _ in 0..200 {
        if let Some(run) = store.active_runs().await.unwrap().first() {
            return run.id;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("等了 2 秒也没等到在跑的 run");
}

/// 等**模型调用真的在飞**：`begin_run` 之后、请求发出去之前还有几步（组装 prompt、查阶段
/// 配置），在那段里按停会走 round-0 的中止检查、根本到不了模型调用。用例要的是「挂在模型
/// 调用上被按停」这一种现场，故等到替身记下这一轮的请求快照再动手。
async fn wait_for_in_flight_call(agent: &FakeAgent, run_id: i64) {
    for _ in 0..200 {
        if agent
            .request_log()
            .iter()
            .any(|r| r.run.as_ref().is_some_and(|c| c.run_id == run_id))
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("等了 2 秒也没等到模型调用在飞");
}

// ─────────────────────────── 暂停 ───────────────────────────

/// 暂停的**全链路**：在飞的那一轮真的被叫停、位置保留、台账不留「还在跑」的假记录。
///
/// 现场用 `Step::Stall` 造（模型流不返回——这正是判超时那条路面对的形状，决策 226）。
#[tokio::test]
async fn pausing_a_running_task_stops_the_turn_and_keeps_the_position() {
    let mut script = Script::new();
    // 游标停在 develop.execute，模型流挂住
    script.for_node(Stage::Develop, Node::Execute).stall();
    let ctx = setup("t-hold-stall", script).await;
    move_cursor(&ctx.store, "t-hold-stall", Stage::Develop, Node::Execute).await;

    let executor = ctx.executor.clone();
    let running = tokio::spawn(async move { executor.run("t-hold-stall").await });
    let run_id = wait_for_running_run(&ctx.store).await;
    wait_for_in_flight_call(&ctx.agent, run_id).await;

    let paused = pause(&ctx.store, &ctx.sse_sink(), "t-hold-stall")
        .await
        .unwrap();
    assert_eq!(paused.cursor_ids.len(), 1);
    assert!(paused.notified, "在跑的执行体应当收到中止请求");

    // 游标：挂着、原因对、位置一个字没动
    let cursor = ctx.store.get_cursor(&paused.cursor_ids[0]).await.unwrap();
    assert_eq!(cursor.status, CursorStatus::Pending);
    assert_eq!(cursor.stage, Stage::Develop);
    assert_eq!(cursor.node, Node::Execute);
    let reason = cursor.pending_reason.expect("暂停必须留下原因");
    assert_eq!(reason.kind, PendingKind::UserPaused);
    assert!(reason.message.contains("位置保留"), "{}", reason.message);

    // 任务投影跟着走（看板那一格）
    let task = ctx.store.get_task("t-hold-stall").await.unwrap();
    assert_eq!(task.status, TaskStatus::Pending);
    assert_eq!(task.current_stage, Stage::Develop);

    // 实时流：这一条按下去要有声（决策 245 的同一姿态）
    assert_eq!(ctx.sse.count_of(SseEventType::Pending), 1);

    // 执行体收口退出；那一轮**不留「还在跑」**——它被记成人按停，不是超时
    running.await.unwrap().unwrap();
    let run = ctx.store.get_run(run_id).await.unwrap().unwrap();
    assert_eq!(
        run.status,
        NodeStatus::Cancelled,
        "人按停的那一轮要自成一档，不能混进 timeout"
    );
    assert!(
        run.error.as_deref().is_some_and(|e| e.contains("人工暂停")),
        "台账里要读得出是谁按停的：{:?}",
        run.error
    );
    // 按停发生在**模型调用已经开始之后**：`request_log` 里有这一轮的请求快照
    // （`calls` 只在调用收尾时计数，而这一轮被按停了、永远不收尾——那正是本用例的现场）。
    assert!(
        ctx.agent
            .request_log()
            .iter()
            .any(|r| r.run.as_ref().is_some_and(|c| c.run_id == run_id)),
        "这一轮真发过模型请求，不是空壳"
    );

    // 屏上那两颗钮由 allowed_actions 下发（与其余每一种 pending 同一套机制）
    let actions = ctx
        .store
        .allowed_actions_for_task("t-hold-stall")
        .await
        .unwrap();
    let names: Vec<&str> = actions.iter().map(|a| a.action.as_str()).collect();
    assert_eq!(names, vec!["continue", "goto", "cancel"]);
}

/// 暂停的前提：**已准入**。队列里 / 等依赖 / 终态的任务都按住不了——报文要说清为什么。
#[tokio::test]
async fn only_admitted_tasks_can_be_held() {
    let ctx = setup("t-hold-gate", Script::new()).await;
    let sse = ctx.sse_sink();

    ctx.store
        .set_task_status("t-hold-gate", TaskStatus::Queued)
        .await
        .unwrap();
    let err = pause(&ctx.store, &sse, "t-hold-gate").await.unwrap_err();
    assert!(
        err.to_string().contains("还没被准入"),
        "队列里的任务要报「还没开跑」：{err}"
    );
    let err = rerun(&ctx.store, &noop_resume(), &sse, "t-hold-gate")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("还没被准入"), "{err}");

    ctx.store
        .set_task_status("t-hold-gate", TaskStatus::Waiting)
        .await
        .unwrap();
    assert!(pause(&ctx.store, &sse, "t-hold-gate").await.is_err());

    ctx.store
        .mark_terminal("t-hold-gate", TaskStatus::Done)
        .await
        .unwrap();
    let err = pause(&ctx.store, &sse, "t-hold-gate").await.unwrap_err();
    assert!(err.to_string().contains("终态"), "{err}");
}

/// 暂停**不发** resume：人按住的这段时间里，没有任何东西该被拉起来。
#[tokio::test]
async fn pausing_does_not_dispatch_an_executor() {
    let ctx = setup("t-hold-nodispatch", Script::new()).await;
    // 没有在跑的 run（游标是 active，但没有执行体）——按住照样成立
    let paused = pause(&ctx.store, &ctx.sse_sink(), "t-hold-nodispatch")
        .await
        .unwrap();
    assert!(!paused.notified, "进程里没有执行体可通知");
    let task = ctx.store.get_task("t-hold-nodispatch").await.unwrap();
    assert_eq!(task.status, TaskStatus::Pending);
}

// ─────────────────────────── 续跑 ───────────────────────────

/// 续跑走的是既有 resume 唯一实现：游标回 `active`、位置不动、**续接**那段对话。
#[tokio::test]
async fn resuming_a_held_task_continues_from_the_same_place() {
    let ctx = setup("t-hold-resume", Script::new()).await;
    move_cursor(&ctx.store, "t-hold-resume", Stage::Develop, Node::Execute).await;
    pause(&ctx.store, &ctx.sse_sink(), "t-hold-resume")
        .await
        .unwrap();

    let applied = apply_resume(
        &ctx.store,
        &Settings::default(),
        &noop_resume(),
        &ctx.sse_sink(),
        "t-hold-resume",
        &ResumeRequest {
            action: "continue".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(applied.action, "continue");

    let cursor = ctx
        .store
        .resolve_sole_cursor("t-hold-resume")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cursor.status, CursorStatus::Active);
    assert_eq!(cursor.stage, Stage::Develop, "续跑接着原处走");
    assert_eq!(cursor.node, Node::Execute);
    assert!(cursor.pending_reason.is_none());
    // 「人松开自己按下的暂停」→ 续接上一段对话（决策 205 的判定表 + 决策 276）
    assert_eq!(
        ctx.store
            .take_cursor_resume_cause(&cursor.cursor_id)
            .await
            .unwrap(),
        Some(ResumeCause::UserPaused)
    );
    assert!(agentpipeline_core::types::resume_continues(
        ResumeCause::UserPaused
    ));
    // 被准入过的任务不回落准入（决策 117：它一直占着自己的名额）
    assert!(!applied.requeued, "已准入的任务续跑不该交还队列");
}

// ─────────────────────────── 重跑本阶段 ───────────────────────────

/// 重跑：落到**本阶段入口**、attempts 归零、那一轮不算（`user_rerun` → 不续接）。
#[tokio::test]
async fn rerun_lands_on_the_stage_entry_and_starts_a_fresh_turn() {
    use agentpipeline_core::storage::observability::NewRun;

    let ctx = setup("t-rerun-entry", Script::new()).await;
    move_cursor(
        &ctx.store,
        "t-rerun-entry",
        Stage::Develop,
        Node::ValidateOutput,
    )
    .await;
    // 本阶段跑过（有 run 行）——重跑的前提
    let cursor = ctx
        .store
        .resolve_sole_cursor("t-rerun-entry")
        .await
        .unwrap()
        .unwrap();
    ctx.store
        .insert_run(&NewRun {
            task_id: "t-rerun-entry".into(),
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

    let out = rerun(&ctx.store, &noop_resume(), &ctx.sse_sink(), "t-rerun-entry")
        .await
        .unwrap();
    assert_eq!(out.moved.len(), 1);
    assert_eq!(out.moved[0].stage, Stage::Develop);

    let cursor = ctx.store.get_cursor(&cursor.cursor_id).await.unwrap();
    assert_eq!(cursor.stage, Stage::Develop, "重跑的是本阶段");
    assert_eq!(
        cursor.node,
        Node::Execute,
        "落点必须是本阶段入口（决策 69）"
    );
    assert_eq!(
        cursor.validate_attempts, 0,
        "重跑把 attempts 归零（决策 43）"
    );
    // 从**在跑**的状态重跑时原因列是空的：那一列记的是「为什么离开 pending」，而这条
    // 游标本来就没挂着——新起的一段对话由构造决定（`take_cursor_resume_cause` 给 None）。
    assert_eq!(
        ctx.store
            .take_cursor_resume_cause(&cursor.cursor_id)
            .await
            .unwrap(),
        None
    );

    // 流转行说清是人按的
    let transitions = ctx.store.list_transitions("t-rerun-entry").await.unwrap();
    let last = transitions.last().expect("要留一条流转行");
    assert_eq!(last.trigger, TransitionTrigger::UserResume);
    assert_eq!(last.reason.as_deref(), Some("人工重跑本阶段"));
}

/// 重跑从 pending 出发时，原因列写 `user_rerun`（与续跑的 `user_paused` 分开）——
/// 这是同一个 pending 两颗出口键的去向差异，只有按键的那一方知道。
#[tokio::test]
async fn rerunning_a_held_task_is_not_a_resume() {
    use agentpipeline_core::storage::observability::NewRun;

    let ctx = setup("t-rerun-held", Script::new()).await;
    move_cursor(
        &ctx.store,
        "t-rerun-held",
        Stage::Develop,
        Node::ValidateOutput,
    )
    .await;
    let cursor = ctx
        .store
        .resolve_sole_cursor("t-rerun-held")
        .await
        .unwrap()
        .unwrap();
    ctx.store
        .insert_run(&NewRun {
            task_id: "t-rerun-held".into(),
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

    pause(&ctx.store, &ctx.sse_sink(), "t-rerun-held")
        .await
        .unwrap();
    // 从**停住**的状态重跑走 pending 动作表那一行（决策 276）：动作表里那颗 goto 就是
    // 「重跑本阶段」，经 resume 唯一实现落地——端点那条路只管在跑的任务（它拒绝时会把
    // 这条分工说清）。
    let err = rerun(&ctx.store, &noop_resume(), &ctx.sse_sink(), "t-rerun-held")
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("走 resume"),
        "停着的任务要指路而不是含糊拒绝：{err}"
    );
    let actions = ctx
        .store
        .allowed_actions_for_task("t-rerun-held")
        .await
        .unwrap();
    let goto = actions
        .iter()
        .find(|a| a.action == "goto")
        .expect("暂停那一行要有「重跑本阶段」");
    let target = goto.target.as_ref().unwrap();

    apply_resume(
        &ctx.store,
        &Settings::default(),
        &noop_resume(),
        &ctx.sse_sink(),
        "t-rerun-held",
        &ResumeRequest {
            action: "goto".into(),
            target_stage: Some(target.stage.as_str().into()),
            target_node: Some(target.node.as_str().into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let cursor = ctx.store.get_cursor(&cursor.cursor_id).await.unwrap();
    assert_eq!(cursor.status, CursorStatus::Active);
    assert_eq!(cursor.stage, Stage::Develop);
    assert_eq!(cursor.node, Node::Execute, "重跑落在本阶段入口");
    assert_eq!(
        ctx.store
            .take_cursor_resume_cause(&cursor.cursor_id)
            .await
            .unwrap(),
        Some(ResumeCause::UserRerun),
        "从挂起处重跑要写 user_rerun（不是 user_paused——那是续跑的去向）"
    );
    assert!(
        !agentpipeline_core::types::resume_continues(ResumeCause::UserRerun),
        "重跑 = 这一轮不算：不带着上一段对话重来"
    );
}

/// **跑过才有可重跑的**：台账里本阶段一条 run 都没有时，重跑被拒——「按了没反应」
/// 比一句拒绝糟糕得多。
#[tokio::test]
async fn rerun_is_refused_when_the_stage_never_ran() {
    let ctx = setup("t-rerun-norun", Script::new()).await;
    let err = rerun(&ctx.store, &noop_resume(), &ctx.sse_sink(), "t-rerun-norun")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("还没跑过"), "{err}");
}

/// 人被按住的任务不该被调度器当成「没人管的待办」：不挂 stalled、不落待办、不叫他。
#[tokio::test]
async fn a_held_task_is_not_reported_as_stale() {
    use agentpipeline_core::scheduler::KanbanScheduler;

    let ctx = setup("t-hold-stale", Script::new()).await;
    pause(&ctx.store, &ctx.sse_sink(), "t-hold-stale")
        .await
        .unwrap();
    // 时钟推过提醒阈值与停滞阈值（默认 24h / 48h），两条判据都本该成立
    ctx.clock.advance_secs(72 * 3600);
    let settings = Settings {
        pending_reminder_hours: 1,
        pending_timeout_hours: 2,
        ..Default::default()
    };
    let scheduler = KanbanScheduler::new(
        ctx.store.clone(),
        settings,
        Arc::new(ctx.clock.clone()),
        Arc::new(RecordingKiller::new()),
        Arc::new(ctx.sse.clone()),
        Arc::new(|_: &str| {}),
    );
    let report = scheduler.tick().await.unwrap();
    assert!(
        report.stalled.is_empty() && report.reminded.is_empty(),
        "人按住的暂停不是停滞：{report:?}"
    );
    let task = ctx.store.get_task("t-hold-stale").await.unwrap();
    assert!(!task.stalled, "不挂停滞红标");
}

fn noop_resume() -> ResumeFn {
    Arc::new(|_: &str| {})
}
