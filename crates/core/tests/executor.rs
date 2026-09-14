//! executor 循环集成测试（票 11；testing.md §6 执行器循环 + §8 E2E-01 的 L2 层）。
//!
//! FakeAgent 只替换 LLM 响应流，工具层 / git / 命令记录全部真实执行（决策 148）。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agentpipeline_core::agent::client::{AgentResponse, LlmClient, LlmRequest};
use agentpipeline_core::config::Settings;
use agentpipeline_core::pipeline::Executor;
use agentpipeline_core::scheduler::KanbanScheduler;
use agentpipeline_core::sse::{SseEvent, SseEventType};
use agentpipeline_core::storage::decisions::MergeDecision;
use agentpipeline_core::storage::Store;
use agentpipeline_core::types::{
    AcceptanceCriterion, Approval, ArchitectExecuteMetadata, CodeChanges, CursorStatus,
    DevelopDesignMetadata, Gate, Node, NodeCursor, NodeStatus, PendingKind, Project, ReviewResult,
    Stage, TaskStatus, TestDesignMetadata, TestResult, TestScenario, TransitionTrigger,
    ValidateInputMetadata, ValidateOutputMetadata,
};
use futures::future::BoxFuture;
use testkit::{FakeAgent, ManualClock, RecordingKiller, Repo, Script, SseRecorder, TestHome};

struct Ctx {
    _home: TestHome,
    repo: Repo,
    store: Store,
    clock: ManualClock,
    killer: RecordingKiller,
    sse: SseRecorder,
    agent: FakeAgent,
    executor: Executor,
}

/// 建项目（test_framework 用原始命令 `true`，让系统闸门零噪声通过）。
async fn setup(framework: &str, settings: Settings) -> Ctx {
    setup_impl(framework, settings, None).await
}

/// 同 [`setup`]，但技能根被 `[skills] dir` 覆盖到外部目录（决策 172）。
async fn setup_with_skills_dir(
    framework: &str,
    settings: Settings,
    skills_dir: &std::path::Path,
) -> Ctx {
    setup_impl(framework, settings, Some(skills_dir.to_path_buf())).await
}

async fn setup_impl(
    framework: &str,
    settings: Settings,
    skills_dir: Option<std::path::PathBuf>,
) -> Ctx {
    let home = TestHome::new().unwrap();
    let clock = ManualClock::fixed();
    // 技能根覆盖经 `Home` 注入 —— executor 与启动校验都读同一处（决策 172）
    let home_handle = match &skills_dir {
        Some(dir) => home.home_with_skills_dir(dir.clone()),
        None => home.home().clone(),
    };
    let store = Store::open(home_handle, Arc::new(clock.clone()))
        .await
        .unwrap();
    let repo = Repo::clean().unwrap();
    let project = Project {
        id: "p1".into(),
        name: "示例".into(),
        local_path: repo.path().display().to_string(),
        default_branch: "main".into(),
        language: None,
        test_framework: Some(framework.into()),
        lint_command: None,
        agents_md_path: None,
        created_at: store.now(),
    };
    store.create_project(&project).await.unwrap();

    let sse = SseRecorder::new();
    let killer = RecordingKiller::new();
    let agent = FakeAgent::new(Script::new());
    let executor = Executor::new(
        store.clone(),
        settings,
        Arc::new(sse.clone()),
        Arc::new(agent.clone()),
        Arc::new(killer.clone()),
    );
    Ctx {
        _home: home,
        repo,
        store,
        clock,
        killer,
        sse,
        agent,
        executor,
    }
}

/// architect + 两条设计分支的最小脚本（分支产出均 readiness，且 test 场景引用 AC-1）。
fn design_scripts(script: &mut Script) {
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    script
        .for_node(Stage::ArchitectDesign, Node::Execute)
        .write_file("design.md", "# 设计\n## 验收标准\n- AC-1 能登录\n")
        .submit(&ArchitectExecuteMetadata {
            readiness: true,
            affected_files: vec!["src/lib.rs".into()],
            new_symbols: vec![],
            acceptance_criteria: vec![AcceptanceCriterion {
                id: "AC-1".into(),
                description: "能登录".into(),
            }],
            ..Default::default()
        });
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateOutput)
        .submit(&ValidateOutputMetadata {
            passed: true,
            ..Default::default()
        });

    script
        .for_node(Stage::DevelopDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    script
        .for_node(Stage::DevelopDesign, Node::Execute)
        .write_file("dev-plan.md", "# 开发计划\n")
        .submit(&DevelopDesignMetadata {
            readiness: true,
            dev_doc_path: Some("dev-plan.md".into()),
            ..Default::default()
        });
    script
        .for_node(Stage::DevelopDesign, Node::ValidateOutput)
        .submit(&ValidateOutputMetadata {
            passed: true,
            ..Default::default()
        });

    script
        .for_node(Stage::TestDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    script
        .for_node(Stage::TestDesign, Node::Execute)
        .write_file("test-scenarios.md", "# 测试场景\n")
        .submit(&TestDesignMetadata {
            readiness: true,
            blockers: vec![],
            test_scenarios_path: Some("test-scenarios.md".into()),
            test_scenarios: vec![TestScenario {
                id: "S-1".into(),
                name: "登录成功".into(),
                description: "登录".into(),
                preconditions: vec![],
                steps: vec![],
                expected_result: "成功".into(),
                priority: agentpipeline_core::types::ScenarioPriority::High,
                design_refs: vec!["AC-1".into()],
            }],
        });
    script
        .for_node(Stage::TestDesign, Node::ValidateOutput)
        .submit(&ValidateOutputMetadata {
            passed: true,
            ..Default::default()
        });
}

/// develop / review / test 的最小脚本：真写代码 + 真提交。
fn implementation_scripts(script: &mut Script, task_id: &str) {
    script
        .for_node(Stage::Develop, Node::Execute)
        .write_file(
            "src/lib.rs",
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n#[test]\nfn adds() { assert_eq!(add(1, 2), 3); }\n",
        )
        .write_file("tests/acceptance.rs", "#[test]\nfn ok() {}\n")
        .run_command(&format!(
            "git add -A && git -c user.name=f -c user.email=f@l commit -m 'feat: task {task_id}'"
        ))
        .submit(&CodeChanges {
            branch_name: format!("kanban/{task_id}"),
            changed_files: vec![],
            unit_test_files: vec![],
        });
    script
        .for_node(Stage::Review, Node::Execute)
        .submit(&ReviewResult {
            approved: true,
            review_report_path: Some("review-report.md".into()),
            required_changes: vec![],
        });
    script
        .for_node(Stage::Test, Node::Execute)
        .submit(&TestResult {
            passed: true,
            test_report_path: Some("test-report.md".into()),
            failures: vec![],
            gate_recheck: false,
        });
}

async fn admit(ctx: &Ctx, task_id: &str) {
    let scheduler = KanbanScheduler::new(
        ctx.store.clone(),
        Settings::default(),
        Arc::new(ctx.clock.clone()),
        Arc::new(ctx.killer.clone()),
        Arc::new(ctx.sse.clone()),
        Arc::new(|_| {}),
    );
    let report = scheduler.tick().await.unwrap();
    assert_eq!(report.admitted, vec![task_id.to_string()]);
}

// ─────────────────────────── 串行 + 并行 happy path ───────────────────────────

#[tokio::test]
async fn executor_drives_full_happy_path_to_done() {
    let ctx = setup("true", Settings::default()).await;
    let base_commit = ctx.repo.head("main");
    let mut script = Script::new();
    design_scripts(&mut script);
    implementation_scripts(&mut script, "t1");
    ctx.agent.set_script(script);

    testkit::seed_task(&ctx.store, "t1", "p1").await.unwrap();
    admit(&ctx, "t1").await;

    // 一次 run：init → architect → 分裂 → 并行设计 → join → develop → review → test → merge A
    ctx.executor.run("t1").await.unwrap();

    // merge 阶段 A 完成：等审批（决策 95 / 119）
    let task = ctx.store.get_task("t1").await.unwrap();
    assert_eq!(task.status, TaskStatus::Pending);
    let actions = ctx.store.allowed_actions_for_task("t1").await.unwrap();
    let names: Vec<&str> = actions.iter().map(|a| a.action.as_str()).collect();
    assert_eq!(names, vec!["approve", "return"]);

    // 提案已生成，闸门通过
    let merge = ctx.store.merge_metadata("t1").await.unwrap().unwrap();
    assert_eq!(merge.approval, Approval::Pending);
    assert_eq!(merge.gate, Some(Gate::Pass));
    assert!(merge.diff_stats.files_changed >= 1);
    let proposal =
        std::fs::read_to_string(ctx._home.home().task_file("t1", "merge-proposal.diff")).unwrap();
    assert!(proposal.contains("src/lib.rs"));

    // 审批 → 阶段 B 合入 → done
    ctx.store
        .apply_merge_decision("t1", MergeDecision::Approve)
        .await
        .unwrap();
    ctx.executor.run("t1").await.unwrap();

    let task = ctx.store.get_task("t1").await.unwrap();
    assert_eq!(task.status, TaskStatus::Done);
    let worktree = ctx._home.home().worktree_path("t1");
    assert!(!worktree.exists(), "done 应清理 worktree（决策 3）");
    assert!(!ctx.repo.branch_exists("kanban/t1"), "done 应删除任务分支");
    // 主干真的前进了（决策 97 回归点）
    assert_ne!(ctx.repo.head("main"), base_commit);
    assert_eq!(
        ctx.repo.git(&["log", "--format=%s", "-1", "main"]).trim(),
        "feat: task t1"
    );

    // run 行：agent 节点 + 纯代码节点（agent_type=system，决策 99/114）
    let runs = ctx.store.list_runs("t1").await.unwrap();
    let agent_runs: Vec<_> = runs.iter().filter(|r| r.agent_type == "main").collect();
    let system_runs: Vec<_> = runs.iter().filter(|r| r.agent_type == "system").collect();
    assert!(
        agent_runs.len() >= 9,
        "agent 节点 run 行：{}",
        agent_runs.len()
    );
    assert!(
        system_runs
            .iter()
            .any(|r| r.stage == Stage::SyncCheck && r.node == Node::Execute),
        "sync-check 应落 system run（决策 107 / 114）"
    );
    assert!(system_runs.iter().any(|r| r.stage == Stage::Init));
    assert!(system_runs.iter().any(|r| r.stage == Stage::Done));
    assert!(runs.iter().all(|r| r.status == NodeStatus::Success));

    // total_tokens / total_calls 汇总（决策 100 / 130 ②）
    let task = ctx.store.get_task("t1").await.unwrap();
    assert!(task.total_tokens > 0);
    assert_eq!(
        task.total_calls as usize,
        agent_runs.len(),
        "system run 不计调用次数"
    );

    // SSE：节点 / 阶段 / 终态事件有生产者（决策 76 / 84）
    assert!(ctx.sse.count_of(SseEventType::NodeStarted) >= 10);
    assert!(ctx.sse.count_of(SseEventType::NodeFinished) >= 10);
    assert!(ctx.sse.count_of(SseEventType::StageChanged) >= 5);
    assert_eq!(ctx.sse.count_of(SseEventType::TaskDone), 1);
    for ev in ctx.sse.events() {
        assert!(!ev.branch().is_empty(), "所有事件都带 branch（决策 84）");
    }

    // join 恰好执行一次（决策 83 / G5）
    let sync_runs = ctx
        .store
        .list_runs_at("t1", Stage::SyncCheck, Node::Execute)
        .await
        .unwrap();
    assert_eq!(sync_runs.len(), 1);

    // 游标：归档行保留，活跃行指向 done（决策 113）
    let all = ctx.store.load_all_cursors("t1").await.unwrap();
    assert!(
        all.iter()
            .filter(|c| c.status == CursorStatus::Archived)
            .count()
            >= 2
    );
    let live = ctx.store.load_live_cursors("t1").await.unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].stage, Stage::Done);

    // 流转时间线含用户审批（决策 119）
    let transitions = ctx.store.list_transitions("t1").await.unwrap();
    assert!(transitions
        .iter()
        .any(|t| t.trigger == TransitionTrigger::UserResume));
}

// ─────────────────────────── merge「返回修改」的投影同步（主流程票 06）───────────────────────────

#[tokio::test]
async fn merge_return_updates_task_projection_immediately() {
    // 决策 119 的 return 打回 develop.execute。回归点：apply_merge_decision 落库后
    // 任务投影必须立刻翻转——否则 /tasks/{id}（与看板）在执行器下次写库前一直显示
    // 旧的「等待审批合入」，用户对着陈旧状态再次提交决策（浏览器 e2e 实测二次合入 404）。
    let ctx = setup("true", Settings::default()).await;
    let task_id = "t-merge-return";
    let mut script = Script::new();
    design_scripts(&mut script);
    implementation_scripts(&mut script, task_id);
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, task_id, "p1").await.unwrap();
    admit(&ctx, task_id).await;

    ctx.executor.run(task_id).await.unwrap();
    let task = ctx.store.get_task(task_id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Pending);
    assert_eq!(task.current_stage, Stage::Merge);

    ctx.store
        .apply_merge_decision(task_id, MergeDecision::Return)
        .await
        .unwrap();

    // 投影立刻翻转（不再等执行器）：游标在 develop.execute，pending 已清
    let task = ctx.store.get_task(task_id).await.unwrap();
    assert_eq!(task.current_stage, Stage::Develop, "投影应显示打回后的落点");
    assert_eq!(task.current_node, Node::Execute);
    assert_eq!(
        task.status,
        TaskStatus::Running,
        "pending 已清，任务回到执行态"
    );

    // 动作面一致：merge_approval 的动作集不再可用
    let actions = ctx.store.allowed_actions_for_task(task_id).await.unwrap();
    assert!(
        !actions.iter().any(|a| a.action == "approve"),
        "返回修改后不应再呈现合入动作"
    );
}

// ─────────────────────────── 分支 pending 隔离（决策 89 / 82）───────────────────────────

#[tokio::test]
async fn tool_events_are_emitted_around_real_tool_execution() {
    // 决策 123：tool_event（start / end + 参数摘要）由 executor 的工具循环发射
    let ctx = setup("true", Settings::default()).await;
    // 注册表以 task_id 为进程全局键：与并行的 happy path 测试错开
    let task_id = "t-tool-events";
    let mut script = Script::new();
    design_scripts(&mut script);
    implementation_scripts(&mut script, task_id);
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, task_id, "p1").await.unwrap();
    admit(&ctx, task_id).await;

    ctx.executor.run(task_id).await.unwrap();

    let tool_events: Vec<SseEvent> = ctx
        .sse
        .events()
        .into_iter()
        .filter(|e| e.event_type() == SseEventType::ToolEvent)
        .collect();
    assert!(
        !tool_events.is_empty(),
        "工具事件应有生产者：{:?}",
        ctx.sse.type_sequence()
    );

    // start / end 成对：write_file 的 start 在 end 之前，摘要只含参数概要
    let starts: Vec<&SseEvent> = tool_events
        .iter()
        .filter(|e| matches!(e, SseEvent::ToolEvent { phase: agentpipeline_core::sse::ToolPhase::Start, tool, .. } if tool == "write_file"))
        .collect();
    let ends: Vec<&SseEvent> = tool_events
        .iter()
        .filter(|e| matches!(e, SseEvent::ToolEvent { phase: agentpipeline_core::sse::ToolPhase::End, tool, .. } if tool == "write_file"))
        .collect();
    assert_eq!(starts.len(), ends.len(), "start/end 应成对");
    assert!(
        matches!(
            starts.first(),
            Some(SseEvent::ToolEvent { args_summary, .. }) if args_summary.contains("design.md")
        ),
        "参数摘要应含文件名：{:?}",
        starts.first()
    );
}

#[tokio::test]
async fn one_branch_pending_does_not_stop_the_other() {
    let ctx = setup("true", Settings::default()).await;
    let mut script = Script::new();
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    script
        .for_node(Stage::ArchitectDesign, Node::Execute)
        .write_file("design.md", "# 设计\n")
        .submit(&ArchitectExecuteMetadata {
            readiness: true,
            ..Default::default()
        });
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateOutput)
        .submit(&ValidateOutputMetadata {
            passed: true,
            ..Default::default()
        });
    // develop-design 输入不足 → 该分支 pending(user_decision)（决策 94）
    script
        .for_node(Stage::DevelopDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: false,
            blockers: vec!["缺少数据流定义".into()],
        });
    // test-design 正常走完，停在 join 边界
    script
        .for_node(Stage::TestDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    script
        .for_node(Stage::TestDesign, Node::Execute)
        .write_file("test-scenarios.md", "# 测试场景\n")
        .submit(&TestDesignMetadata {
            readiness: true,
            blockers: vec![],
            ..Default::default()
        });
    script
        .for_node(Stage::TestDesign, Node::ValidateOutput)
        .submit(&ValidateOutputMetadata {
            passed: true,
            ..Default::default()
        });
    ctx.agent.set_script(script);

    testkit::seed_task(&ctx.store, "t2", "p1").await.unwrap();
    admit(&ctx, "t2").await;
    ctx.executor.run("t2").await.unwrap();

    let live = ctx.store.load_live_cursors("t2").await.unwrap();
    let dev = live
        .iter()
        .find(|c| c.branch == NodeCursor::BRANCH_DEVELOP_DESIGN)
        .unwrap();
    let test = live
        .iter()
        .find(|c| c.branch == NodeCursor::BRANCH_TEST_DESIGN)
        .unwrap();
    assert_eq!(
        dev.status,
        CursorStatus::Pending,
        "develop-design 分支被阻塞"
    );
    assert_eq!(
        test.status,
        CursorStatus::WaitingJoin,
        "另一分支跑完本阶段后停在 join 边界（决策 82）"
    );
    assert_eq!(
        dev.pending_reason.as_ref().unwrap().kind,
        PendingKind::UserDecision
    );
    // join 未执行（决策 83：有 pending 就不汇聚）
    assert!(ctx
        .store
        .list_runs_at("t2", Stage::SyncCheck, Node::Execute)
        .await
        .unwrap()
        .is_empty());
    // 任务投影为 pending，executor 已退出等 resume
    assert_eq!(
        ctx.store.get_task("t2").await.unwrap().status,
        TaskStatus::Pending
    );

    // Pending SSE 带分支信息（决策 84）
    let pending = ctx.sse.last_of(SseEventType::Pending).unwrap();
    assert!(matches!(&pending, SseEvent::Pending { branch, .. } if branch == "develop-design"));
}

// ─────────────────────────── 单执行者（决策 36）───────────────────────────

/// 在第一次 LLM 调用处挂起的替身，用于观察 executor_owner 与进程内去重。
struct BlockingAgentSimple {
    gate: Arc<tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>>,
    calls: Arc<AtomicUsize>,
}

impl LlmClient for BlockingAgentSimple {
    fn complete(
        &self,
        _request: LlmRequest,
    ) -> BoxFuture<'static, agentpipeline_core::Result<AgentResponse>> {
        let gate = self.gate.clone();
        let calls = self.calls.clone();
        Box::pin(async move {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                if let Some(rx) = gate.lock().await.take() {
                    let _ = rx.await;
                }
            }
            Ok(AgentResponse::default())
        })
    }
}

#[tokio::test]
async fn concurrent_executors_deduplicate_on_the_same_task() {
    let ctx = setup("true", Settings::default()).await;
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let calls = Arc::new(AtomicUsize::new(0));
    let agent = BlockingAgentSimple {
        gate: Arc::new(tokio::sync::Mutex::new(Some(rx))),
        calls: calls.clone(),
    };
    let llm: Arc<dyn LlmClient> = Arc::new(agent);

    let ex1 = Arc::new(Executor::new(
        ctx.store.clone(),
        Settings::default(),
        Arc::new(ctx.sse.clone()),
        llm.clone(),
        Arc::new(ctx.killer.clone()),
    ));
    let ex2 = Executor::new(
        ctx.store.clone(),
        Settings::default(),
        Arc::new(ctx.sse.clone()),
        llm.clone(),
        Arc::new(ctx.killer.clone()),
    );

    testkit::seed_task(&ctx.store, "t3", "p1").await.unwrap();
    admit(&ctx, "t3").await;

    let jh = {
        let e = ex1.clone();
        tokio::spawn(async move { e.run("t3").await })
    };
    // 等 executor 阻塞在第一次 LLM 调用
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if calls.load(Ordering::SeqCst) >= 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("第一个 executor 应进行到第一次 LLM 调用");

    // 第二个 executor：进程内注册表命中 → 立即返回，不执行任何节点（决策 36）
    ex2.run("t3").await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // 放行：脚本耗尽 → validate_input 无元数据 → 重试耗尽 → pending(retry_exhausted)
    drop(tx);
    jh.await.unwrap().unwrap();
    let live = ctx.store.load_live_cursors("t3").await.unwrap();
    assert_eq!(live[0].status, CursorStatus::Pending);
    assert_eq!(
        live[0].pending_reason.as_ref().unwrap().kind,
        PendingKind::RetryExhausted
    );
}

/// `try_run` 必须把「被在跑的 executor 挡下」报告给调用方，否则 resume 钩子无法重试，
/// 审批/恢复请求落在旧 executor 退出窗口内会被静默丢弃（任务永久 pending）。
#[tokio::test]
async fn try_run_reports_skip_so_the_resume_hook_can_retry() {
    let ctx = setup("true", Settings::default()).await;
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let calls = Arc::new(AtomicUsize::new(0));
    let agent = BlockingAgentSimple {
        gate: Arc::new(tokio::sync::Mutex::new(Some(rx))),
        calls: calls.clone(),
    };
    let llm: Arc<dyn LlmClient> = Arc::new(agent);

    let ex1 = Arc::new(Executor::new(
        ctx.store.clone(),
        Settings::default(),
        Arc::new(ctx.sse.clone()),
        llm.clone(),
        Arc::new(ctx.killer.clone()),
    ));
    let ex2 = Executor::new(
        ctx.store.clone(),
        Settings::default(),
        Arc::new(ctx.sse.clone()),
        llm.clone(),
        Arc::new(ctx.killer.clone()),
    );

    testkit::seed_task(&ctx.store, "t4", "p1").await.unwrap();
    admit(&ctx, "t4").await;

    let jh = {
        let e = ex1.clone();
        tokio::spawn(async move { e.run("t4").await })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("第一个 executor 应进行到第一次 LLM 调用");

    // 旧 executor 未退出 → 本次未执行（调用方据此重试）
    assert!(
        !ex2.try_run("t4").await.unwrap(),
        "已有 executor 在跑时必须报告「未执行」"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1, "不得并发跑第二个节点");

    drop(tx);
    jh.await.unwrap().unwrap();

    // 旧 executor 退出后重试 → 真正取得执行权（即便任务已无可推进节点也应报告 true）
    assert!(
        ex2.try_run("t4").await.unwrap(),
        "旧 executor 退出后重试应取得执行权"
    );
}

// ─────────────────────────── 节点重试耗尽（决策 33 / G13）───────────────────────────

#[tokio::test]
async fn agent_metadata_failure_retries_then_pends() {
    let ctx = setup("true", Settings::default()).await;
    let mut script = Script::new();
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    // execute 只给文本、不给元数据 → 每次尝试都失败
    script
        .for_node(Stage::ArchitectDesign, Node::Execute)
        .text("没有元数据");
    ctx.agent.set_script(script);

    testkit::seed_task(&ctx.store, "t4", "p1").await.unwrap();
    admit(&ctx, "t4").await;
    ctx.executor.run("t4").await.unwrap();

    // agent_retry_max 次干净对话重试后 → pending(retry_exhausted)
    let live = ctx.store.load_live_cursors("t4").await.unwrap();
    let cursor = live.first().unwrap();
    assert_eq!(cursor.stage, Stage::ArchitectDesign);
    assert_eq!(cursor.status, CursorStatus::Pending);
    assert_eq!(
        cursor.pending_reason.as_ref().unwrap().kind,
        PendingKind::RetryExhausted
    );
    let runs = ctx
        .store
        .list_runs_at("t4", Stage::ArchitectDesign, Node::Execute)
        .await
        .unwrap();
    assert_eq!(
        runs.len(),
        Settings::default().agent_retry_max as usize,
        "每个 attempt 都落 run 行"
    );
    assert!(runs.iter().all(|r| r.status == NodeStatus::Failed));
}

// ─────────────────────────── 会话截断（§12.4.3 conversation_max_chars）───────────────────────────

#[tokio::test]
async fn conversations_are_truncated_to_max_chars() {
    let settings = Settings {
        conversation_max_chars: 120,
        ..Default::default()
    };
    let ctx = setup("true", settings).await;
    let mut script = Script::new();
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    script
        .for_node(Stage::ArchitectDesign, Node::Execute)
        .write_file("design.md", &"x".repeat(2000))
        .submit(&ArchitectExecuteMetadata {
            readiness: true,
            ..Default::default()
        });
    ctx.agent.set_script(script);

    testkit::seed_task(&ctx.store, "t5", "p1").await.unwrap();
    admit(&ctx, "t5").await;
    ctx.executor.run("t5").await.unwrap();

    // write_file 的 2000 字符参数应让会话触顶截断
    let runs = ctx.store.list_runs("t5").await.unwrap();
    let execute_run = runs
        .iter()
        .find(|r| r.stage == Stage::ArchitectDesign && r.node == Node::Execute)
        .unwrap();
    let conv = ctx
        .store
        .get_conversation("t5", execute_run.id)
        .await
        .unwrap()
        .expect("execute 节点应有会话行");
    let raw = conv.messages_json.to_string();
    assert!(
        raw.chars().count() < 400,
        "会话应被截断：{}",
        raw.chars().count()
    );
    assert!(
        !raw.contains("xxxxxxxxxx"),
        "超长内容不得进入落库会话（应被丢弃轮次或截断）"
    );
}

// ─────────────────────────── 小工具单测 ───────────────────────────

#[test]
fn test_command_mapping() {
    use agentpipeline_core::pipeline::executor::test_command_for;
    assert_eq!(test_command_for(None), "true");
    assert_eq!(test_command_for(Some("")), "true");
    assert_eq!(test_command_for(Some("cargo")), "cargo test --quiet");
    assert_eq!(test_command_for(Some("pytest")), "python3 -m pytest -q");
    // 自定义框架值视作原始命令（e2e fixture 用 `true` 零噪声通过闸门）
    assert_eq!(test_command_for(Some("true")), "true");
    assert_eq!(test_command_for(Some("make check")), "make check");
}

#[test]
fn diff_stat_summary_is_parsed() {
    use agentpipeline_core::pipeline::executor::parse_diff_stats;
    let stats = parse_diff_stats(" src/a.rs | 2 ++\n 1 file changed, 2 insertions(+)\n");
    assert_eq!(stats.files_changed, 1);
    assert_eq!(stats.insertions, 2);
    assert_eq!(stats.deletions, 0);
}

// ──────────────────── prompt 组装消费模板 / 配置（票 12）────────────────────

#[tokio::test]
async fn prompt_assembly_consumes_templates_stage_configs_and_agents_md() {
    let ctx = setup("true", Settings::default()).await;
    // G3：项目 AGENTS.md 进入 system prompt
    std::fs::write(
        ctx.repo.path().join("AGENTS.md"),
        "项目约定：提交前跑 just test",
    )
    .unwrap();

    // 阶段配置消费（§10.6.3 / 决策 22 / 46 / 111）
    ctx.store
        .upsert_stage_config(&agentpipeline_core::types::StageConfig {
            stage: "architect-design".into(),
            provider_id: None,
            temperature: Some(0.25),
            max_tokens: Some(1234),
            persona_path: None,
            persona_append: Some("附加要求：所有注释用中文。".into()),
            tools_json: Some(serde_json::json!(["read_file", "not_implemented_tool"])),
            skills_json: None,
            idle_timeout_sec: None,
            max_duration_sec: None,
            node_overrides_json: None,
            updated_at: ctx.store.now(),
        })
        .await
        .unwrap();
    // review 用 persona_path 显式指定 persona（相对 home 根解析，§10.6.3）
    let review_persona = ctx._home.home().root().join("review-persona.md");
    std::fs::write(&review_persona, "你是资深评审，聚焦回归风险。").unwrap();
    ctx.store
        .upsert_stage_config(&agentpipeline_core::types::StageConfig {
            stage: "review".into(),
            provider_id: None,
            temperature: None,
            max_tokens: None,
            persona_path: Some("review-persona.md".into()),
            persona_append: None,
            tools_json: None,
            skills_json: None,
            idle_timeout_sec: None,
            max_duration_sec: None,
            node_overrides_json: None,
            updated_at: ctx.store.now(),
        })
        .await
        .unwrap();

    let mut script = Script::new();
    design_scripts(&mut script);
    implementation_scripts(&mut script, "t6");
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t6", "p1").await.unwrap();
    admit(&ctx, "t6").await;
    ctx.executor.run("t6").await.unwrap();

    let requests = ctx.agent.request_log();
    let vi = requests
        .iter()
        .find(|r| r.stage == Stage::ArchitectDesign && r.node == Node::ValidateInput)
        .expect("architect validate_input 请求");

    // G3：AGENTS.md 内容注入；内嵌 §10.3 模板生效（不再是一句话 persona）
    assert!(vi.system_prompt.contains("项目约定：提交前跑 just test"));
    assert!(vi
        .system_prompt
        .contains("判断任务信息是否足够进行架构设计"));
    // persona_append 追加为额外指令段
    assert!(vi.system_prompt.contains("附加要求：所有注释用中文。"));
    // G12：system 与 user prompt 都显式包含任务目录绝对路径
    let task_dir = ctx._home.home().task_dir("t6").display().to_string();
    assert!(vi.system_prompt.contains("## 工作目录"));
    assert!(vi.system_prompt.contains(&task_dir));
    assert!(vi.user_prompt.contains("## 环境路径"));
    assert!(vi.user_prompt.contains(&task_dir));
    // 阶段配置采样参数透传给 LLM 适配层
    assert_eq!(vi.temperature, Some(0.25));
    assert_eq!(vi.max_tokens, Some(1234));
    // 工具并集语义：mandatory 一个不少，声明未实现的被忽略
    assert!(vi.tools.iter().any(|t| t.name == "submit_metadata"));
    assert!(vi.tools.iter().any(|t| t.name == "read_file"));
    assert!(!vi.tools.iter().any(|t| t.name == "not_implemented_tool"));

    // §10.3 user 模板变量：validate_output 拿到设计文档绝对路径
    let vo = requests
        .iter()
        .find(|r| r.stage == Stage::ArchitectDesign && r.node == Node::ValidateOutput)
        .expect("architect validate_output 请求");
    assert!(vo.user_prompt.contains(&format!("{task_dir}/design.md")));

    // persona_path 显式指定的 persona 覆盖内嵌模板（§10.6.3）
    let re = requests
        .iter()
        .find(|r| r.stage == Stage::Review && r.node == Node::Execute)
        .expect("review execute 请求");
    assert!(re.system_prompt.contains("你是资深评审，聚焦回归风险。"));
    assert!(!re.system_prompt.contains("你是代码评审 agent。"));

    // test.execute 的 user prompt：测试命令（§10.3 模板）+ 场景文档路径
    let te = requests
        .iter()
        .find(|r| r.stage == Stage::Test && r.node == Node::Execute)
        .expect("test execute 请求");
    assert!(te.user_prompt.contains("测试命令：true"));
    assert!(te
        .user_prompt
        .contains(&format!("{task_dir}/test-scenarios.md")));
    // 共享脚本的 CodeChanges 未列文件 → 决策 115 降级说明而非空白
    assert!(te.user_prompt.contains("按决策 115 降级处理"));

    // prompt_template_hash 反映最终组装内容（决策 137）
    let runs = ctx.store.list_runs("t6").await.unwrap();
    let run = runs
        .iter()
        .find(|r| r.stage == Stage::ArchitectDesign && r.node == Node::ValidateInput)
        .unwrap();
    let hash = run.prompt_template_hash.as_ref().expect("模板哈希已落库");
    assert_eq!(hash.len(), 16);
}

/// 节点级技能注入（决策 170）：同一阶段的不同节点拿到**不同**技能正文。
///
/// architect-design 的 validate_input 配 `grilling`、execute 配 `to-spec`，
/// 断言两个节点的 system prompt 各自含对应正文、且**不含**对方的——
/// 这是「阶段级 skills_json 无法区分节点」的直接反证。
#[tokio::test]
async fn node_scoped_skills_inject_different_bodies_per_node() {
    let ctx = setup("true", Settings::default()).await;
    ctx.store
        .upsert_stage_config(&agentpipeline_core::types::StageConfig {
            stage: "architect-design".into(),
            node_overrides_json: Some(serde_json::json!({
                "validate_input": {"skills": ["grilling"]},
                "execute": {"skills": ["to-spec"]}
            })),
            updated_at: ctx.store.now(),
            ..Default::default()
        })
        .await
        .unwrap();

    let mut script = Script::new();
    design_scripts(&mut script);
    implementation_scripts(&mut script, "t-node-skills");
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t-node-skills", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-node-skills").await;
    ctx.executor.run("t-node-skills").await.unwrap();

    let requests = ctx.agent.request_log();
    let vi = requests
        .iter()
        .find(|r| r.stage == Stage::ArchitectDesign && r.node == Node::ValidateInput)
        .expect("architect validate_input 请求");
    let ex = requests
        .iter()
        .find(|r| r.stage == Stage::ArchitectDesign && r.node == Node::Execute)
        .expect("architect execute 请求");

    // 两个节点都带上了「已启用技能」段，但内容各不同
    assert!(
        vi.system_prompt.contains("## 已启用技能"),
        "{}",
        vi.system_prompt
    );
    assert!(
        ex.system_prompt.contains("## 已启用技能"),
        "{}",
        ex.system_prompt
    );

    // validate_input：拷问协议的正文（经 pending 回路提问）
    assert!(
        vi.system_prompt.contains("### grilling"),
        "{}",
        vi.system_prompt
    );
    assert!(
        vi.system_prompt.contains("frontier"),
        "{}",
        vi.system_prompt
    );
    assert!(!vi.system_prompt.contains("### to-spec"));

    // execute：综合成规格的正文（不再提问，守决策 136 的验收标准）
    assert!(
        ex.system_prompt.contains("### to-spec"),
        "{}",
        ex.system_prompt
    );
    assert!(
        ex.system_prompt.contains("acceptance_criteria"),
        "{}",
        ex.system_prompt
    );
    assert!(!ex.system_prompt.contains("### grilling"));

    // validate_output 未声明技能 → 无技能段
    let vo = requests
        .iter()
        .find(|r| r.stage == Stage::ArchitectDesign && r.node == Node::ValidateOutput)
        .expect("architect validate_output 请求");
    assert!(
        !vo.system_prompt.contains("## 已启用技能"),
        "{}",
        vo.system_prompt
    );
}

/// 阶段级 `skills_json` 仍然生效（旧行为不回归），且与节点级**取并集**（只增不减）。
#[tokio::test]
async fn stage_level_skills_still_apply_and_union_with_node_level() {
    let ctx = setup("true", Settings::default()).await;
    ctx.store
        .upsert_stage_config(&agentpipeline_core::types::StageConfig {
            stage: "architect-design".into(),
            // 阶段级声明一个知识型技能，节点级再叠一个
            skills_json: Some(serde_json::json!(["rtk"])),
            node_overrides_json: Some(serde_json::json!({
                "validate_input": {"skills": ["grilling"]}
            })),
            updated_at: ctx.store.now(),
            ..Default::default()
        })
        .await
        .unwrap();

    let mut script = Script::new();
    design_scripts(&mut script);
    implementation_scripts(&mut script, "t-skills-union");
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t-skills-union", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-skills-union").await;
    ctx.executor.run("t-skills-union").await.unwrap();

    let requests = ctx.agent.request_log();
    let vi = requests
        .iter()
        .find(|r| r.stage == Stage::ArchitectDesign && r.node == Node::ValidateInput)
        .expect("architect validate_input 请求");
    // 阶段级工具型技能（`- rtk`）与节点级知识型技能（`### grilling`）并存
    assert!(vi.system_prompt.contains("- rtk"), "{}", vi.system_prompt);
    assert!(
        vi.system_prompt.contains("### grilling"),
        "{}",
        vi.system_prompt
    );

    // execute 只有阶段级技能（节点级未声明）→ 无 grilling
    let ex = requests
        .iter()
        .find(|r| r.stage == Stage::ArchitectDesign && r.node == Node::Execute)
        .expect("architect execute 请求");
    assert!(ex.system_prompt.contains("- rtk"), "{}", ex.system_prompt);
    assert!(!ex.system_prompt.contains("### grilling"));
}

/// 票 01（决策 172）：`[skills] dir` 指到外部技能根后，该目录下的技能全链路可用
/// ——被发现、被启动校验放行、正文进入 system prompt。
#[tokio::test]
async fn external_skills_dir_is_discovered_and_injected() {
    // 外部技能根：不在临时 home 之下（模拟 `~/.zcode/skills`）
    let external = tempfile::tempdir().unwrap();
    let skill_dir = external.path().join("my-grill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: my-grill\ndescription: 外部技能\n---\n\n外部技能正文：拷问用户",
    )
    .unwrap();

    let ctx = setup_with_skills_dir("true", Settings::default(), external.path()).await;

    // 启动校验：外部目录的技能进入可用集且正文校验通过
    // （store 持有的 Home 就是被覆盖的那个，与 executor 读同一处）
    let names =
        agentpipeline_core::config::discover_available_skills(&ctx.store.home().skills_dir());
    assert!(
        names.iter().any(|n| n == "my-grill"),
        "外部技能应被发现：{names:?}"
    );

    ctx.store
        .upsert_stage_config(&agentpipeline_core::types::StageConfig {
            stage: "architect-design".into(),
            node_overrides_json: Some(serde_json::json!({
                "validate_input": {"skills": ["my-grill"]}
            })),
            updated_at: ctx.store.now(),
            ..Default::default()
        })
        .await
        .unwrap();

    let mut script = Script::new();
    design_scripts(&mut script);
    implementation_scripts(&mut script, "t-external-skills");
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t-external-skills", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-external-skills").await;
    ctx.executor.run("t-external-skills").await.unwrap();

    let requests = ctx.agent.request_log();
    let vi = requests
        .iter()
        .find(|r| r.stage == Stage::ArchitectDesign && r.node == Node::ValidateInput)
        .expect("architect validate_input 请求");
    assert!(
        vi.system_prompt.contains("### my-grill"),
        "外部技能正文应进 system prompt：{}",
        vi.system_prompt
    );
    assert!(
        vi.system_prompt.contains("外部技能正文：拷问用户"),
        "{}",
        vi.system_prompt
    );
}

// ──────────────────── 闸门失败 → test 复检 → 重跑闸门（票 15 / 决策 85 / 109）────────────────────

#[tokio::test]
async fn merge_gate_failure_routes_to_test_recheck_then_reruns_gate() {
    use agentpipeline_core::storage::decisions::ResumeAction;
    use agentpipeline_core::types::GateFailureKind;

    let ctx = setup("true", Settings::default()).await;
    // 有状态闸门命令：第 1 次（develop 闸门）通过、第 2 次（merge 闸门首跑）失败、
    // 第 3 次（复检后重跑 merge 闸门）通过。
    let home_root = ctx._home.home().root().to_path_buf();
    let gate = home_root.join("gate.sh");
    std::fs::write(
        &gate,
        format!(
            "#!/bin/sh\n\
             d=\"{}\"\n\
             n=$(cat \"$d\" 2>/dev/null || echo 0)\n\
             n=$((n+1))\n\
             echo $n > \"$d\"\n\
             if [ \"$n\" -eq 2 ]; then\n\
               echo \"GATE_FAIL_OUTPUT: integration test_login failed\"\n\
               exit 1\n\
             fi\n\
             exit 0\n",
            home_root.join("gate-count").display()
        ),
    )
    .unwrap();
    ctx.store
        .update_project(
            "p1",
            None,
            None,
            Some(&format!("sh {}", gate.display())),
            None,
        )
        .await
        .unwrap();

    let mut script = Script::new();
    design_scripts(&mut script);
    implementation_scripts(&mut script, "t7");
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t7", "p1").await.unwrap();
    admit(&ctx, "t7").await;

    // run #1：merge 测试闸门失败 → GotoTest → test.execute 复检（脚本已耗尽 → pending 收尾）
    ctx.executor.run("t7").await.unwrap();

    let merge = ctx.store.merge_metadata("t7").await.unwrap().unwrap();
    assert_eq!(merge.gate, Some(Gate::Fail));
    assert_eq!(merge.gate_failure_kind, Some(GateFailureKind::Test));
    assert_eq!(merge.gate_failures, 1);
    let transitions = ctx.store.list_transitions("t7").await.unwrap();
    assert!(
        transitions.iter().any(|t| t.to_stage == Stage::Test
            && t.to_node == Node::Execute
            && t.trigger == TransitionTrigger::Kickback),
        "应有 merge → test.execute 的 kickback 流转：{transitions:?}"
    );
    let live = ctx.store.load_live_cursors("t7").await.unwrap();
    assert_eq!(live[0].stage, Stage::Test);
    assert_eq!(live[0].node, Node::Execute);
    assert_eq!(
        live[0].pending_reason.as_ref().unwrap().kind,
        PendingKind::RetryExhausted
    );

    // run #2：复检脚本（agent 未自报 gate_recheck）→ 系统置位 → test.validate_output 放行 → merge 重跑闸门通过
    let mut recheck = Script::new();
    recheck
        .for_node(Stage::Test, Node::Execute)
        .submit(&TestResult {
            passed: true,
            test_report_path: Some("test-report.md".into()),
            failures: vec![],
            gate_recheck: false,
        });
    ctx.agent.set_script(recheck);
    let cursor = ctx
        .store
        .load_live_cursors("t7")
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    ctx.store
        .apply_resume(&cursor, ResumeAction::Continue, None, None)
        .await
        .unwrap();
    ctx.executor.run("t7").await.unwrap();

    // 复检 prompt 注入闸门完整日志（决策 85 / 109）
    let requests = ctx.agent.request_log();
    let first = requests
        .iter()
        .find(|r| r.stage == Stage::Test && r.node == Node::Execute)
        .expect("首轮 test.execute");
    assert!(
        !first.user_prompt.contains("## 合入闸门失败复检上下文"),
        "首轮不渲染复检段（首轮为空不渲染）"
    );
    let recheck_req = requests
        .iter()
        .rev()
        .find(|r| r.stage == Stage::Test && r.node == Node::Execute)
        .expect("复检 test.execute");
    assert!(
        recheck_req
            .user_prompt
            .contains("## 合入闸门失败复检上下文"),
        "{}",
        recheck_req.user_prompt
    );
    assert!(recheck_req.user_prompt.contains("GATE_FAIL_OUTPUT"));

    // 系统置位 test_result.gate_recheck = true（决策 109）
    let test_meta = ctx
        .store
        .stage_output_metadata("t7", Stage::Test, "test_report")
        .await
        .unwrap()
        .expect("test_result 元数据");
    assert_eq!(test_meta["gate_recheck"], true);

    // 闸门重跑通过 → proposal 待审批
    let merge = ctx.store.merge_metadata("t7").await.unwrap().unwrap();
    assert_eq!(merge.gate, Some(Gate::Pass));
    assert_eq!(merge.approval, Approval::Pending);
    assert_eq!(
        ctx.store.get_task("t7").await.unwrap().status,
        TaskStatus::Pending
    );
    let actions = ctx.store.allowed_actions_for_task("t7").await.unwrap();
    let names: Vec<&str> = actions.iter().map(|a| a.action.as_str()).collect();
    assert!(names.contains(&"approve") && names.contains(&"return"));
}

// ──────────────────── conflict_check 语义第二层（票 16 / 决策 60 / 67 / 100）────────────────────

#[tokio::test]
async fn architect_semantic_conflict_check_pends_with_duplicate_risk() {
    use agentpipeline_core::pipeline::pseudo::ConflictCheckResult;
    use agentpipeline_core::types::{DuplicateRisk, NewSymbol, SymbolKind};

    let ctx = setup("true", Settings::default()).await;

    // 先准入本任务，再播种另一条活跃任务（避免 scheduler 一次准入两条）
    let mut script = Script::new();
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    script
        .for_node(Stage::ArchitectDesign, Node::Execute)
        .write_file("design.md", "# 设计\n")
        .submit(&ArchitectExecuteMetadata {
            readiness: true,
            affected_files: vec!["src/t.rs".into()],
            new_symbols: vec![NewSymbol {
                name: "login_b".into(),
                kind: SymbolKind::Function,
                module_path: "auth".into(),
                file_path: "src/t.rs".into(),
            }],
            ..Default::default()
        });
    script
        .for_pseudo("pseudo:conflict_check")
        .submit(&ConflictCheckResult {
            duplicate_risk: DuplicateRisk::High,
            reason: Some("两边都在实现登录".into()),
        });
    ctx.agent.set_script(script);

    testkit::seed_task(&ctx.store, "tc-b", "p1").await.unwrap();
    admit(&ctx, "tc-b").await;

    // 另一条活跃任务：同模块 auth、符号名不同、受影响文件不重叠
    testkit::seed_task(&ctx.store, "tc-a", "p1").await.unwrap();
    ctx.store
        .upsert_stage_output(
            "tc-a",
            Stage::ArchitectDesign,
            "design_doc",
            "design.md",
            Some(&serde_json::json!({
                "readiness": true,
                "affected_files": ["src/other.rs"],
                "new_symbols": [{
                    "name": "login_a",
                    "kind": "function",
                    "module_path": "auth",
                    "file_path": "src/other.rs"
                }],
                "acceptance_criteria": []
            })),
        )
        .await
        .unwrap();

    ctx.executor.run("tc-b").await.unwrap();

    let live = ctx.store.load_live_cursors("tc-b").await.unwrap();
    assert_eq!(live[0].stage, Stage::ArchitectDesign);
    assert_eq!(live[0].node, Node::Execute);
    let reason = live[0].pending_reason.as_ref().unwrap();
    assert_eq!(reason.kind, PendingKind::UserDecision);
    let ctx_reason = reason.context.as_ref().unwrap();
    assert_eq!(ctx_reason.kind.as_deref(), Some("duplicate_risk"));
    assert!(
        ctx_reason.conflict_task_ids.contains(&"tc-a".to_string()),
        "全部冲突任务 id 应记录：{:?}",
        ctx_reason.conflict_task_ids
    );

    // 伪阶段独立 run + 会话行，cursor_id 继承父游标（决策 100 / 113）
    let runs = ctx.store.list_runs("tc-b").await.unwrap();
    let pseudo = runs
        .iter()
        .find(|r| r.agent_type == "pseudo:conflict_check")
        .expect("伪阶段 run 应落库");
    assert_eq!(
        pseudo.cursor_id.as_deref(),
        Some(live[0].cursor_id.as_str())
    );
    assert_eq!(pseudo.stage, Stage::ArchitectDesign);
    assert_eq!(pseudo.node, Node::Execute);
    assert!(pseudo.parent_run_id.is_some(), "parent_run_id 指向父 run");
    assert!(
        ctx.store
            .get_conversation("tc-b", pseudo.id)
            .await
            .unwrap()
            .is_some(),
        "伪阶段应有独立会话行"
    );
    // total_calls 计入伪阶段（决策 130②）
    assert!(ctx.store.get_task("tc-b").await.unwrap().total_calls >= 2);
}

// ──────────────────── validator_cross_check 异族复判（票 16 / 决策 134 / 135）────────────────────

/// 造出「agent 型 validate_output 首判不合格 + 异族复判合格」的 judge_disagreement pending。
async fn judge_disagreement_ctx(task_id: &str) -> Ctx {
    use agentpipeline_core::pipeline::pseudo::CrossCheckResult;

    let settings = Settings {
        cross_family_judge: true,
        ..Default::default()
    };
    let ctx = setup("true", settings).await;
    let mut script = Script::new();
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    script
        .for_node(Stage::ArchitectDesign, Node::Execute)
        .submit(&ArchitectExecuteMetadata {
            readiness: true,
            ..Default::default()
        });
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateOutput)
        .submit(&ValidateOutputMetadata {
            passed: false,
            blockers: vec!["设计不完整".into()],
            feedback: None,
        });
    script
        .for_pseudo("pseudo:validator_cross_check")
        .submit(&CrossCheckResult {
            passed: true,
            blockers: vec!["复判认为合格".into()],
        });
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, task_id, "p1").await.unwrap();
    admit(&ctx, task_id).await;
    ctx.executor.run(task_id).await.unwrap();
    ctx
}

#[tokio::test]
async fn validator_cross_check_disagreement_pends_and_continue_advances_stage() {
    use agentpipeline_core::storage::decisions::ResumeAction;

    let ctx = judge_disagreement_ctx("td-continue").await;
    let cursor = ctx
        .store
        .load_live_cursors("td-continue")
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let reason = cursor.pending_reason.as_ref().unwrap();
    assert_eq!(reason.kind, PendingKind::UserDecision);
    assert_eq!(
        reason.context.as_ref().unwrap().kind.as_deref(),
        Some("judge_disagreement")
    );

    // 复判伪阶段独立 run 落库，父 run 指向 validate_output（决策 100）
    let runs = ctx.store.list_runs("td-continue").await.unwrap();
    let pseudo = runs
        .iter()
        .find(|r| r.agent_type == "pseudo:validator_cross_check")
        .expect("复判 run 应落库");
    assert!(pseudo.parent_run_id.is_some());
    assert_eq!(pseudo.stage, Stage::ArchitectDesign);
    assert_eq!(pseudo.node, Node::ValidateOutput);

    // continue = 用户裁决合格 → 特判直接放行下一阶段，不重跑 validate_output（决策 135）
    let calls_before = ctx
        .agent
        .calls_for(Stage::ArchitectDesign, Node::ValidateOutput);
    ctx.store
        .apply_resume(&cursor, ResumeAction::Continue, None, None)
        .await
        .unwrap();
    let live = ctx.store.load_live_cursors("td-continue").await.unwrap();
    assert_eq!(live.len(), 2, "architect 放行应分裂到两条设计分支");
    let stages: Vec<Stage> = live.iter().map(|c| c.stage).collect();
    assert!(stages.contains(&Stage::DevelopDesign));
    assert!(stages.contains(&Stage::TestDesign));
    assert!(live.iter().all(|c| c.status == CursorStatus::Active));
    assert_eq!(
        ctx.agent
            .calls_for(Stage::ArchitectDesign, Node::ValidateOutput),
        calls_before,
        "continue 放行不得重跑校验"
    );
}

#[tokio::test]
async fn validator_cross_check_disagreement_goto_execute_increments_attempts() {
    use agentpipeline_core::storage::decisions::ResumeAction;

    let ctx = judge_disagreement_ctx("td-goto").await;
    let cursor = ctx
        .store
        .load_live_cursors("td-goto")
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    ctx.store
        .apply_resume(
            &cursor,
            ResumeAction::Goto,
            Some((Stage::ArchitectDesign, Node::Execute)),
            None,
        )
        .await
        .unwrap();
    let live = ctx.store.get_cursor(&cursor.cursor_id).await.unwrap();
    assert_eq!(live.stage, Stage::ArchitectDesign);
    assert_eq!(live.node, Node::Execute);
    assert_eq!(live.status, CursorStatus::Active);
    assert_eq!(
        live.validate_attempts, 1,
        "goto execute → attempts +1（决策 135）"
    );
}

/// 决策 172 / 票 14：伪阶段 run 复用父节点的 stage/node，**不得**虚增该节点的 `attempt`。
///
/// 复现路径：validator_cross_check 在 `architect-design.validate_output` 坐标下落一行
/// （`agent_type = pseudo:*`），随后 goto execute 打回、该节点重跑一遍。旧口径下
/// `next_attempt` 把伪阶段那行也算成一次尝试，第二次 validate_output 会拿到 `attempt = 3`
/// ——序号里凭空跳掉一个 2。修正后节点自身的 attempt 序列是连续的 1、2。
#[tokio::test]
async fn pseudo_stage_run_does_not_inflate_next_attempt() {
    use agentpipeline_core::storage::decisions::ResumeAction;
    use agentpipeline_core::types::Node;

    let ctx = judge_disagreement_ctx("td-attempt").await;

    // 前提：复判伪阶段确实在 validate_output 坐标下落了 run（决策 100 / 134）
    let before = ctx.store.list_runs("td-attempt").await.unwrap();
    let cross = before
        .iter()
        .find(|r| r.agent_type == "pseudo:validator_cross_check")
        .expect("复判 run 应落库");
    assert_eq!(cross.stage, Stage::ArchitectDesign);
    assert_eq!(cross.node, Node::ValidateOutput);

    // 用户裁决「不合格」→ 打回 execute 重跑（决策 135）
    let cursor = ctx
        .store
        .load_live_cursors("td-attempt")
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    ctx.store
        .apply_resume(
            &cursor,
            ResumeAction::Goto,
            Some((Stage::ArchitectDesign, Node::Execute)),
            None,
        )
        .await
        .unwrap();

    // 第二轮脚本：execute 再产出，validate_output 这次通过
    let mut script = Script::new();
    script
        .for_node(Stage::ArchitectDesign, Node::Execute)
        .submit(&ArchitectExecuteMetadata {
            readiness: true,
            ..Default::default()
        });
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateOutput)
        .submit(&ValidateOutputMetadata {
            passed: true,
            blockers: vec![],
            feedback: None,
        });
    ctx.agent.set_script(script);
    // 第二轮不得再触发复判（本次 validate_output 直接通过）
    ctx.executor.run("td-attempt").await.unwrap();

    let runs = ctx.store.list_runs("td-attempt").await.unwrap();
    let mut attempts: Vec<u32> = runs
        .iter()
        .filter(|r| r.agent_type == "main" && r.stage == Stage::ArchitectDesign)
        .filter(|r| r.node == Node::ValidateOutput)
        .map(|r| r.attempt)
        .collect();
    attempts.sort_unstable();
    assert_eq!(
        attempts,
        vec![1, 2],
        "复判伪阶段不得把第二次 validate_output 顶成 attempt 3（决策 172）"
    );
}

// ──────────────────── project_analysis LLM 摘要（票 16 / 决策 48 / 78 / 130）────────────────────
#[tokio::test]
async fn project_analysis_merges_llm_summary_into_facts() {
    use agentpipeline_core::pipeline::pseudo::ProjectAnalysisResult;

    let ctx = setup("true", Settings::default()).await;
    let mut script = Script::new();
    script
        .for_pseudo("pseudo:project_analysis")
        .submit(&ProjectAnalysisResult {
            summary: "这是一个 Rust 项目".into(),
            suspicious: vec!["检测到多套测试框架".into()],
        });
    ctx.agent.set_script(script);

    let project = ctx.store.get_project("p1").await.unwrap().unwrap();
    let facts = serde_json::json!({
        "language": "rust",
        "test_framework": "cargo",
        "suspicious": []
    });
    let merged = ctx
        .executor
        .project_analysis(&project, facts)
        .await
        .unwrap();
    assert_eq!(merged["summary"], "这是一个 Rust 项目");
    assert_eq!(merged["suspicious"][0], "检测到多套测试框架");
    assert_eq!(merged["language"], "rust", "确定性探测事实必须保留");
}

// ──────────────────── §6 执行器循环：单游标失败隔离 / join 恰一次（决策 89 / 107）────────────────────

#[tokio::test]
async fn single_branch_node_failure_is_isolated_to_that_cursor() {
    // 决策 89：一条游标的节点失败只把该游标置 pending，另一分支继续跑完停在 join。
    let ctx = setup("true", Settings::default()).await;
    let mut script = Script::new();
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    script
        .for_node(Stage::ArchitectDesign, Node::Execute)
        .write_file("design.md", "# 设计\n")
        .submit(&ArchitectExecuteMetadata {
            readiness: true,
            ..Default::default()
        });
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateOutput)
        .submit(&ValidateOutputMetadata {
            passed: true,
            ..Default::default()
        });
    // develop-design.validate_input 永远给不出可解析元数据 → 节点重试耗尽（节点失败）
    script
        .for_node(Stage::DevelopDesign, Node::ValidateInput)
        .text("没有元数据");
    // test-design 正常走完 → 停在 join 边界
    script
        .for_node(Stage::TestDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    script
        .for_node(Stage::TestDesign, Node::Execute)
        .write_file("test-scenarios.md", "# 测试场景\n")
        .submit(&TestDesignMetadata {
            readiness: true,
            ..Default::default()
        });
    script
        .for_node(Stage::TestDesign, Node::ValidateOutput)
        .submit(&ValidateOutputMetadata {
            passed: true,
            ..Default::default()
        });
    ctx.agent.set_script(script);

    testkit::seed_task(&ctx.store, "t-isolate", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-isolate").await;
    ctx.executor.run("t-isolate").await.unwrap();

    let live = ctx.store.load_live_cursors("t-isolate").await.unwrap();
    let dev = live
        .iter()
        .find(|c| c.branch == NodeCursor::BRANCH_DEVELOP_DESIGN)
        .expect("develop-design 分支");
    let test = live
        .iter()
        .find(|c| c.branch == NodeCursor::BRANCH_TEST_DESIGN)
        .expect("test-design 分支");
    assert_eq!(dev.status, CursorStatus::Pending, "失败游标只阻塞自己");
    assert_eq!(
        dev.pending_reason.as_ref().unwrap().kind,
        PendingKind::RetryExhausted
    );
    assert_eq!(
        test.status,
        CursorStatus::WaitingJoin,
        "另一分支不受失败影响，跑完停在 join"
    );
    // 节点失败不向上传播：另一分支的 run 全部成功
    let test_runs = ctx
        .store
        .list_runs_at("t-isolate", Stage::TestDesign, Node::ValidateOutput)
        .await
        .unwrap();
    assert_eq!(test_runs.len(), 1);
    assert_eq!(test_runs[0].status, NodeStatus::Success);
    // 有 pending 不汇聚（决策 83 / 107）
    assert!(ctx
        .store
        .list_runs_at("t-isolate", Stage::SyncCheck, Node::Execute)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        ctx.store.get_task("t-isolate").await.unwrap().status,
        TaskStatus::Pending
    );
}

#[tokio::test]
async fn advance_join_runs_exactly_once_even_across_repeated_runs() {
    // 决策 107 / G5：所有游标到界后 join 恰执行一次；再次调用 run 不重复汇聚。
    let ctx = setup("true", Settings::default()).await;
    let mut script = Script::new();
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: false,
            blockers: vec!["占位".into()],
        });
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t-join-once", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-join-once").await;

    // 直接造出「两分支都到界」的 join-ready 状态
    let split = ctx.store.split_cursors("t-join-once").await.unwrap();
    for c in &split {
        ctx.store
            .set_cursor_waiting_join(&c.cursor_id)
            .await
            .unwrap();
    }
    // sync-check 后脚本耗尽 → pending(retry_exhausted) 停在 develop.execute
    ctx.agent.set_script(Script::new());
    ctx.executor.run("t-join-once").await.unwrap();
    assert_eq!(
        ctx.store
            .list_runs_at("t-join-once", Stage::SyncCheck, Node::Execute)
            .await
            .unwrap()
            .len(),
        1
    );

    // 再次 run：不得再落第二条 sync-check run
    ctx.executor.run("t-join-once").await.unwrap();
    assert_eq!(
        ctx.store
            .list_runs_at("t-join-once", Stage::SyncCheck, Node::Execute)
            .await
            .unwrap()
            .len(),
        1,
        "join 只执行一次"
    );
}

// ──────────────────── G13：工具失败分层，单次工具失败不触发节点重试（决策 33）────────────────────

#[tokio::test]
async fn tool_failure_within_budget_does_not_retry_the_node() {
    let ctx = setup("true", Settings::default()).await;
    let mut script = Script::new();
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    // execute：第 1 次 write_file 调用失败（fail_tool_n 注入），随后提交合法元数据
    script
        .for_node(Stage::ArchitectDesign, Node::Execute)
        .write_file("design.md", "# 设计\n")
        .fail_tool_n("write_file", 1)
        .submit(&ArchitectExecuteMetadata {
            readiness: true,
            ..Default::default()
        });
    ctx.agent.set_script(script);

    testkit::seed_task(&ctx.store, "t-toolfail", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-toolfail").await;
    ctx.executor.run("t-toolfail").await.unwrap();

    // 工具失败在 agent loop 内消化：节点只跑一次且成功（无 agent_retry）
    let runs = ctx
        .store
        .list_runs_at("t-toolfail", Stage::ArchitectDesign, Node::Execute)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1, "单次工具失败不触发节点重试（G13）");
    assert_eq!(runs[0].status, NodeStatus::Success);
    // 失败以 tool_event error 形态外发（决策 123）
    let errors = ctx
        .sse
        .events()
        .into_iter()
        .filter(|e| {
            matches!(
                e,
                SseEvent::ToolEvent { phase: agentpipeline_core::sse::ToolPhase::Error, tool, .. }
                    if tool == "write_file"
            )
        })
        .count();
    assert_eq!(errors, 1, "工具失败应发 error 事件");
    assert!(ctx.sse.count_of(SseEventType::ToolEvent) >= 2);
}

// ──────────────────── 超长工具结果：L1 裁剪 / L2 卸载（决策 110）────────────────────

#[tokio::test]
async fn long_tool_result_triggers_l1_trim() {
    // 300 行输出 > L1 阈值（前 50 + 后 100）；未达 L2 阈值 → 裁剪保留在会话中
    let ctx = setup("true", Settings::default()).await;
    let mut script = Script::new();
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    script
        .for_node(Stage::ArchitectDesign, Node::Execute)
        .long_tool_result(300)
        .submit(&ArchitectExecuteMetadata {
            readiness: true,
            ..Default::default()
        });
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t-l1", "p1").await.unwrap();
    admit(&ctx, "t-l1").await;
    ctx.executor.run("t-l1").await.unwrap();

    let run = ctx
        .store
        .list_runs_at("t-l1", Stage::ArchitectDesign, Node::Execute)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let conv = ctx
        .store
        .get_conversation("t-l1", run.id)
        .await
        .unwrap()
        .expect("会话行");
    let raw = conv.messages_json.to_string();
    assert!(raw.contains("已省略"), "L1 裁剪标记应进入会话：{raw:.300}");
    assert!(!raw.contains("已卸载"), "300 行未到 L2 阈值");
}

#[tokio::test]
async fn long_tool_result_triggers_l2_offload_to_disk() {
    // 20000 行 ≈ 10 万字符 ≫ offload_threshold_tokens(4000 token) → L2 卸载落盘
    let ctx = setup("true", Settings::default()).await;
    let mut script = Script::new();
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    script
        .for_node(Stage::ArchitectDesign, Node::Execute)
        .long_tool_result(20_000)
        .submit(&ArchitectExecuteMetadata {
            readiness: true,
            ..Default::default()
        });
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t-l2", "p1").await.unwrap();
    admit(&ctx, "t-l2").await;
    ctx.executor.run("t-l2").await.unwrap();

    let run = ctx
        .store
        .list_runs_at("t-l2", Stage::ArchitectDesign, Node::Execute)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let conv = ctx
        .store
        .get_conversation("t-l2", run.id)
        .await
        .unwrap()
        .expect("会话行");
    let raw = conv.messages_json.to_string();
    assert!(raw.contains("已卸载"), "L2 卸载标记应进入会话");
    // 卸载文件真实落盘（决策 148：L2 卸载真落盘）
    let ctx_dir = ctx._home.home().context_dir("t-l2");
    assert!(
        ctx_dir.exists(),
        "context 目录应存在：{}",
        ctx_dir.display()
    );
    let files: Vec<_> = std::fs::read_dir(&ctx_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert!(!files.is_empty(), "离线文件应真实落盘");
    let any_content = files.iter().any(|f| {
        std::fs::read_to_string(f.path())
            .map(|s| s.contains("19999") || s.contains("20000"))
            .unwrap_or(false)
    });
    assert!(any_content, "卸载文件应含完整输出（尾部行）");
}

// ──────────────────── 配置 fail fast（决策 47 / 103 / 134）────────────────────

#[tokio::test]
async fn cross_family_judge_without_provider_refuses_startup() {
    use agentpipeline_core::config::{validate_startup, StartupInputs};
    use agentpipeline_core::types::{Provider, StageConfig};

    let provider = Provider {
        id: "p1".into(),
        vendor: "deepseek".into(),
        model: "deepseek-chat".into(),
        context_window: 64_000,
        base_url: None,
        api_key: None,
        enabled: true,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    // 开关开但没注册 validator_cross_check 伪阶段 → 拒绝启动（决策 134 ⑤）
    let err = validate_startup(&StartupInputs {
        settings: Settings {
            cross_family_judge: true,
            ..Default::default()
        },
        providers: vec![provider.clone()],
        stage_configs: vec![],
        available_skills: vec![],
        home_root: None,
        skills_root: None,
    })
    .unwrap_err();
    assert!(
        matches!(err, agentpipeline_core::Error::Config(_)),
        "{err:?}"
    );

    // 注册了伪阶段但没配 provider → 同样拒绝
    let err = validate_startup(&StartupInputs {
        settings: Settings {
            cross_family_judge: true,
            ..Default::default()
        },
        providers: vec![provider],
        stage_configs: vec![StageConfig {
            stage: "validator_cross_check".into(),
            ..Default::default()
        }],
        available_skills: vec![],
        home_root: None,
        skills_root: None,
    })
    .unwrap_err();
    assert!(
        matches!(err, agentpipeline_core::Error::Config(_)),
        "{err:?}"
    );
}

#[tokio::test]
async fn referenced_missing_skill_refuses_startup() {
    use agentpipeline_core::config::{validate_startup, StartupInputs};
    use agentpipeline_core::types::{Provider, StageConfig};

    let inputs = StartupInputs {
        settings: Settings::default(),
        providers: vec![Provider {
            id: "p1".into(),
            vendor: "deepseek".into(),
            model: "deepseek-chat".into(),
            context_window: 64_000,
            base_url: None,
            api_key: None,
            enabled: true,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }],
        stage_configs: vec![StageConfig {
            stage: "develop".into(),
            provider_id: Some("p1".into()),
            skills_json: Some(serde_json::json!(["definitely-not-installed"])),
            ..Default::default()
        }],
        available_skills: vec!["rtk".into()],
        home_root: None,
        skills_root: None,
    };
    let err = validate_startup(&inputs).unwrap_err();
    assert!(matches!(err, agentpipeline_core::Error::Config(_)));
    assert!(err.to_string().contains("skill"), "{err}");
}

/// 节点级技能引用了不存在的技能名 → 同样 fail fast（决策 170），
/// 且报错要指明是哪个阶段、哪个节点。
#[tokio::test]
async fn missing_node_skill_refuses_startup_with_node_in_message() {
    use agentpipeline_core::config::{validate_startup, StartupInputs};
    use agentpipeline_core::types::{Provider, StageConfig};

    let inputs = StartupInputs {
        settings: Settings::default(),
        providers: vec![Provider {
            id: "p1".into(),
            vendor: "deepseek".into(),
            model: "deepseek-chat".into(),
            context_window: 64_000,
            base_url: None,
            api_key: None,
            enabled: true,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }],
        stage_configs: vec![StageConfig {
            stage: "architect-design".into(),
            provider_id: Some("p1".into()),
            node_overrides_json: Some(serde_json::json!({
                "validate_input": {"skills": ["no-such-skill"]}
            })),
            ..Default::default()
        }],
        available_skills: vec!["grilling".into()],
        home_root: None,
        skills_root: None,
    };
    let err = validate_startup(&inputs).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("no-such-skill"), "{msg}");
    assert!(msg.contains("validate_input"), "报错须定位到节点：{msg}");
}

/// 知识型技能正文为空（用户写了空文件）→ 启动拒绝（决策 170，同 persona_path 口径）。
#[tokio::test]
async fn empty_knowledge_skill_body_refuses_startup() {
    use agentpipeline_core::config::{validate_startup, StartupInputs};
    use agentpipeline_core::types::StageConfig;

    let home = tempfile::tempdir().unwrap();
    // 技能根 = `{home}/skills`（决策 172：skills_root 就是技能根本身）
    let skills_root = home.path().join("skills");
    let skill_dir = skills_root.join("my-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "   \n").unwrap();

    let inputs = StartupInputs {
        stage_configs: vec![StageConfig {
            stage: "architect-design".into(),
            node_overrides_json: Some(serde_json::json!({
                "validate_input": {"skills": ["my-skill"]}
            })),
            ..Default::default()
        }],
        available_skills: vec!["my-skill".into()],
        home_root: None,
        skills_root: Some(skills_root),
        ..Default::default()
    };
    let err = validate_startup(&inputs).unwrap_err();
    assert!(err.to_string().contains("正文为空"), "{err}");
}

/// frontmatter `name` 与目录名不一致 → 启动拒绝（决策 172，对齐 Agent Skills 规范）。
#[tokio::test]
async fn mismatched_frontmatter_name_refuses_startup() {
    use agentpipeline_core::config::{validate_startup, StartupInputs};

    let home = tempfile::tempdir().unwrap();
    let skills_root = home.path().join("skills");
    let skill_dir = skills_root.join("grilling");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: something-else\n---\n\n正文",
    )
    .unwrap();

    let inputs = StartupInputs {
        available_skills: vec!["grilling".into()],
        skills_root: Some(skills_root),
        ..Default::default()
    };
    let err = validate_startup(&inputs).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("grilling") && msg.contains("something-else"),
        "报错须点明目录名与 frontmatter name：{msg}"
    );
}
