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
use agentpipeline_core::storage::decisions::ResumeAction;
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

    // 票 04：系统节点在步边界留痕——「它做到了哪一步」在成功路径上同样是证据
    // （失败 / 超时那条路走的是同一列，见 scheduler_tick 的 timeout 用例）。
    let init_run = system_runs
        .iter()
        .find(|r| r.stage == Stage::Init)
        .expect("init 应有 system run");
    assert_eq!(
        init_run.step.as_deref(),
        Some("把工作区与分支写回任务行"),
        "init 的最后一步要留下（步骤名是执行语义，不是观测细节）"
    );
    assert!(
        system_runs
            .iter()
            .any(|r| r.step.as_deref() == Some("跑测试：true")),
        "闸门命令也是步边界：{:?}",
        system_runs
            .iter()
            .map(|r| (r.stage, r.node, r.step.clone()))
            .collect::<Vec<_>>()
    );

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

// ─────────────────────────── 判超时之后真的停下来（决策 226）───────────────────────────

/// 第一次调用正常返回（带用量），**之后停住不返回**——2026-09-19 那次僵死的形状：
/// 卡死的 run 停在模型调用上，没有进程组可杀。
struct StallingAgent {
    calls: Arc<AtomicUsize>,
    gate: Arc<tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>>,
}

impl LlmClient for StallingAgent {
    fn complete(
        &self,
        _request: LlmRequest,
    ) -> BoxFuture<'static, agentpipeline_core::Result<AgentResponse>> {
        let calls = self.calls.clone();
        let gate = self.gate.clone();
        Box::pin(async move {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                return Ok(AgentResponse {
                    content: None,
                    tool_calls: vec![agentpipeline_core::agent::client::ToolCall {
                        id: "c1".into(),
                        name: "read_file".into(),
                        arguments: r#"{"path":"."}"#.into(),
                    }],
                    prompt_tokens: 7,
                    completion_tokens: 3,
                    ..Default::default()
                });
            }
            // 永不放行 = 永不返回。判超时那边**没有进程组可杀**，只有中止请求这条路。
            if let Some(rx) = gate.lock().await.take() {
                let _ = rx.await;
            }
            Ok(AgentResponse::default())
        })
    }
}

/// 被判超时的 run 必须**真的停下来**（决策 226）。
///
/// 停在模型调用上的 run，其 `process_group_id` 是 NULL（只有 `run_command` 回填过），
/// 于是「杀进程组」那一刀没有东西可砍：超时只改了台账，执行体照旧活着、照旧占着进程内
/// 去重与 `executor_owner`，紧接着的 resume 被逐次拒掉——2026-09-19 实测它又活了 8 小时
/// 以上，任务僵死到有人按 `unstick`。
///
/// 这一条断言收口的**三件事**：执行体在有界时间内退出、它把手里那份用量补记上去且
/// **不碰**终态与时长、执行权真的让了出来。
#[tokio::test]
async fn a_timed_out_run_is_stopped_and_reports_its_usage() {
    let ctx = setup("true", Settings::default()).await;
    // 闸门**不放行**：`_tx` 一直活着，于是第二次调用永远停在那里——这正是要的形状。
    let (_tx, rx) = tokio::sync::oneshot::channel::<()>();
    let calls = Arc::new(AtomicUsize::new(0));
    let llm: Arc<dyn LlmClient> = Arc::new(StallingAgent {
        calls: calls.clone(),
        gate: Arc::new(tokio::sync::Mutex::new(Some(rx))),
    });
    let ex = Arc::new(Executor::new(
        ctx.store.clone(),
        Settings::default(),
        Arc::new(ctx.sse.clone()),
        llm,
        Arc::new(ctx.killer.clone()),
    ));

    testkit::seed_task(&ctx.store, "t9", "p1").await.unwrap();
    admit(&ctx, "t9").await;
    let jh = {
        let e = ex.clone();
        tokio::spawn(async move { e.run("t9").await })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while calls.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("应进行到第二次模型调用（就是停住那次）");

    let run = ctx
        .store
        .list_runs_at("t9", Stage::ArchitectDesign, Node::ValidateInput)
        .await
        .unwrap()
        .pop()
        .expect("validate_input 应当已经落了 run 行");
    assert_eq!(run.status, NodeStatus::Running);

    // 判超时那一步：标终态 + 记时长（时长本身由 scheduler 写，见 scheduler_tick 用例）
    ctx.store
        .finish_run(
            run.id,
            &agentpipeline_core::storage::observability::RunOutcome {
                status: Some(NodeStatus::Timeout),
                duration_ms: 31_886_000,
                error: Some("测试：判超时".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert!(
        agentpipeline_core::pipeline::executor::request_cancel("t9"),
        "在跑的执行体应当找得到，才谈得上通知它收口"
    );
    tokio::time::timeout(Duration::from_secs(5), jh)
        .await
        .expect("收到中止请求的执行体应当在有界时间内收口")
        .unwrap()
        .unwrap();

    let after = ctx
        .store
        .list_runs_at("t9", Stage::ArchitectDesign, Node::ValidateInput)
        .await
        .unwrap()
        .pop()
        .unwrap();
    // 终态与时长归判超时那一方——补记用量是**只补读数**，不抢终态的归属
    assert_eq!(
        after.status,
        NodeStatus::Timeout,
        "中止不得把「超时」改写成「失败」"
    );
    assert_eq!(
        after.duration_ms, 31_886_000,
        "中止不得改写判超时记下的时长"
    );
    // 用量是执行体手里的读数：它不收口就没人知道（此前失败/超时路径一律记 0）
    assert_eq!(
        (after.prompt_tokens, after.completion_tokens),
        (7, 3),
        "这一轮已经烧掉的 token 要照实补记"
    );
    // 补记的用量也要进任务投影：`total_tokens` 是从 run 行**重算**的，不刷新就两面不一致
    let task = ctx.store.get_task("t9").await.unwrap();
    let from_runs: u64 = ctx
        .store
        .list_runs("t9")
        .await
        .unwrap()
        .iter()
        .map(agentpipeline_core::metrics::run_tokens)
        .sum();
    assert_eq!(
        task.total_tokens as u64, from_runs,
        "任务投影须与 run 行汇总同源"
    );
    assert!(task.total_tokens > 0, "补记的用量不得被漏掉");

    // 不挂 pending：判超时那边已经放了一次重试，这里挂 pending 会把它立刻打回去
    let cursors = ctx.store.load_live_cursors("t9").await.unwrap();
    assert!(
        cursors.iter().all(|c| !c.is_pending()),
        "中止不得把游标置 pending（那会把调度器刚放出去的重试打回去）"
    );

    // 执行权真的让出来了——这正是僵死的解法（此前会被逐次拒到钩子放弃）
    assert!(
        ex.try_run("t9").await.unwrap(),
        "旧执行体收口后，重试应当拿得到执行权"
    );
}

/// 失败的一轮也要照实记它烧掉的 token（决策 226）。
///
/// 此前失败路径给 `finish_run` 传的是 `RunTokens::default()`，于是台账里的「0」既是读数
/// 又是哨兵：2026-09-19 值班长据那个 0 推出「两次尝试连第一次 LLM 调用都没落账」，而同一个
/// 0 也长在死因完全已知的 run 上（init 的 `git 操作超时（180s）`）。
#[tokio::test]
async fn a_failed_round_records_the_tokens_it_burned() {
    // 一轮就耗尽：断言那一条 run 行不用挑
    let settings = Settings {
        agent_retry_max: 1,
        ..Default::default()
    };
    let ctx = setup("true", settings).await;
    let mut script = Script::new();
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        // 先一次成功的调用（FakeAgent 每步报 10 prompt / 5 completion），再让调用当场失败
        .list_dir(".")
        .fail_llm("llm_network", "模型服务不可达", "connect timed out");
    ctx.agent.set_script(script);

    testkit::seed_task(&ctx.store, "t9", "p1").await.unwrap();
    admit(&ctx, "t9").await;
    ctx.executor.run("t9").await.unwrap();

    let run = ctx
        .store
        .list_runs_at("t9", Stage::ArchitectDesign, Node::ValidateInput)
        .await
        .unwrap()
        .pop()
        .expect("validate_input 应当落了 run 行");
    assert_eq!(run.status, NodeStatus::Failed);
    assert_eq!(
        (run.prompt_tokens, run.completion_tokens),
        (10, 5),
        "失败的那一轮烧掉的 token 要照实落账，不能记成 0"
    );
    // 投影与 run 行必须同源：失败轮的 token 记真之后，只在成功路径刷新的那份投影会落后
    // （这一批正是被 `continued_run_links_back_so_tokens_are_not_double_counted` 顶出来的）
    let task = ctx.store.get_task("t9").await.unwrap();
    let from_runs: u64 = ctx
        .store
        .list_runs("t9")
        .await
        .unwrap()
        .iter()
        .map(agentpipeline_core::metrics::run_tokens)
        .sum();
    assert_eq!(
        task.total_tokens as u64, from_runs,
        "任务投影须与 run 行汇总同源"
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

    // 票 01（决策 211①）：失败的每一轮同样落会话行——1:1 对 run，attempt 区分。
    // 这条链路今天断在「`?` 把内存里的 messages 一起带走」，所以查不到失败现场。
    let convs: Vec<_> = ctx
        .store
        .list_conversations("t4", true)
        .await
        .unwrap()
        .into_iter()
        .filter(|c| c.stage == Stage::ArchitectDesign && c.node == Node::Execute)
        .collect();
    assert_eq!(
        convs.len(),
        runs.len(),
        "每个失败的 run 都该有且仅有一条会话行"
    );
    for conv in &convs {
        let meta = conv
            .metadata_json
            .as_ref()
            .expect("失败会话要带错误上下文（否则读会话的人不知道它为什么停在这里）");
        assert_eq!(meta["failed"], true);
        assert!(
            !meta["error"].as_str().unwrap_or_default().is_empty(),
            "失败原因不能是空串：{meta}"
        );
        assert!(
            !conv.messages_json.as_array().unwrap().is_empty(),
            "失败前的消息要留下（这里正是「模型回了文本却没给元数据」）"
        );
    }
}

// ─────────────────── 失败路径的会话落库（决策 211① / 票 01）───────────────────

#[tokio::test]
async fn llm_failure_writes_a_conversation_row_with_the_reason() {
    // 可归因的适配器失败：这一轮以前什么都不留，连「为什么没跑起来」都查不到
    let settings = Settings {
        agent_retry_max: 1,
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
        .fail_llm(
            "llm_network",
            "LLM 服务不可达，请检查网络或 base_url",
            "connect: connection refused",
        );
    ctx.agent.set_script(script);

    testkit::seed_task(&ctx.store, "t-llmfail", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-llmfail").await;
    ctx.executor.run("t-llmfail").await.unwrap();

    let run = ctx
        .store
        .list_runs_at("t-llmfail", Stage::ArchitectDesign, Node::Execute)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(run.status, NodeStatus::Failed);
    let conv = ctx
        .store
        .get_conversation("t-llmfail", run.id)
        .await
        .unwrap()
        .expect("LLM 报错的那一轮也要有会话行");
    let meta = conv.metadata_json.expect("失败会话带错误上下文");
    assert_eq!(meta["failed"], true);
    assert_eq!(
        meta["classified"]["kind"], "llm_network",
        "类别要留下来——它是「该去改什么」的线索：{meta}"
    );
    assert!(
        meta["error"].as_str().unwrap().contains("不可达"),
        "可操作提示要进 error：{meta}"
    );
}

#[tokio::test]
async fn tool_retry_exhaustion_writes_the_failed_conversation() {
    // 工具重试耗尽：失败前的最后一次工具调用必须能在会话里看见
    let settings = Settings {
        tool_retry_max: 1,
        agent_retry_max: 1,
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
    // 两次必然失败的工具调用（参数不是对象 → 解析失败）：第 2 次超过 tool_retry_max
    script
        .for_node(Stage::ArchitectDesign, Node::Execute)
        .failing_tool("run_command", serde_json::json!("__fail__"))
        .failing_tool("run_command", serde_json::json!("__fail__"));
    ctx.agent.set_script(script);

    testkit::seed_task(&ctx.store, "t-toolbox", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-toolbox").await;
    ctx.executor.run("t-toolbox").await.unwrap();

    let run = ctx
        .store
        .list_runs_at("t-toolbox", Stage::ArchitectDesign, Node::Execute)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(run.status, NodeStatus::Failed);
    let conv = ctx
        .store
        .get_conversation("t-toolbox", run.id)
        .await
        .unwrap()
        .expect("工具重试耗尽的那一轮也要有会话行");
    let raw = conv.messages_json.to_string();
    assert!(
        raw.contains("工具执行失败"),
        "失败前的最后一次工具调用要留下：{raw}"
    );
    let meta = conv.metadata_json.expect("失败会话带错误上下文");
    assert!(
        meta["error"].as_str().unwrap().contains("tool_retry_max"),
        "失败原因要点名工具重试预算：{meta}"
    );
}

// ─────────────────── prompt 原文落库（决策 211② / 票 02）───────────────────

#[tokio::test]
async fn the_assembled_prompt_is_kept_verbatim_beside_the_conversation() {
    // 「这是 prompt 问题」此前无从核对：落库的 messages 里没有 system / user 两段
    // （它们只在适配器组装 HTTP body 时才前置），用户段连 hash 都没有。
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
        .write_file("design.md", "# 设计\n## 验收标准\n- AC-1 能登录\n")
        .submit(&ArchitectExecuteMetadata {
            readiness: true,
            ..Default::default()
        });
    ctx.agent.set_script(script);

    testkit::seed_task(&ctx.store, "t-prompt", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-prompt").await;
    ctx.executor.run("t-prompt").await.unwrap();

    let run = ctx
        .store
        .list_runs_at("t-prompt", Stage::ArchitectDesign, Node::Execute)
        .await
        .unwrap()
        .remove(0);
    // hash 没被取代：它降级为「两次跑的是不是同一份」的索引，两者同时写
    assert!(
        run.prompt_template_hash.is_some(),
        "hash 仍是索引，不是被原文取代"
    );
    let conv = ctx
        .store
        .get_conversation("t-prompt", run.id)
        .await
        .unwrap()
        .expect("execute 节点应有会话行");
    // 与当次真实请求逐字相等——打在两段字符串上，不是打在长度上
    // （长度相等而内容不同，正是哈希看不见的那种漂移）
    let request = ctx
        .agent
        .request_log()
        .into_iter()
        .find(|r| r.stage == Stage::ArchitectDesign && r.node == Node::Execute)
        .expect("execute 节点调过 LLM");
    assert_eq!(
        conv.system_prompt.as_deref(),
        Some(request.system_prompt.as_str()),
        "系统段原文逐字相等"
    );
    assert_eq!(
        conv.user_prompt.as_deref(),
        Some(request.user_prompt.as_str()),
        "用户段原文逐字相等"
    );
}

#[tokio::test]
async fn prompt_snapshot_shares_the_conversation_char_account() {
    // 票 02：三段共吃 conversation_max_chars 一本账——原文先占，余量给 messages；
    // 两侧超限都留标记，不许静默截短。
    let settings = Settings {
        conversation_max_chars: 400,
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
        .write_file("design.md", &"x".repeat(3000))
        .submit(&ArchitectExecuteMetadata {
            readiness: true,
            ..Default::default()
        });
    ctx.agent.set_script(script);

    testkit::seed_task(&ctx.store, "t-account", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-account").await;
    ctx.executor.run("t-account").await.unwrap();

    let run = ctx
        .store
        .list_runs_at("t-account", Stage::ArchitectDesign, Node::Execute)
        .await
        .unwrap()
        .remove(0);
    let conv = ctx
        .store
        .get_conversation("t-account", run.id)
        .await
        .unwrap()
        .expect("execute 节点应有会话行");
    let system = conv.system_prompt.expect("系统段原文要落库");
    let user = conv.user_prompt.expect("用户段原文要落库");
    let total = system.chars().count()
        + user.chars().count()
        + conv.messages_json.to_string().chars().count();
    assert!(
        total <= 400,
        "三段共吃一本账：实际 {total} 字符（system {} + user {}）",
        system.chars().count(),
        user.chars().count()
    );
    assert!(
        system.contains("截断"),
        "截断要留标记，不许静默截短：{}",
        &system[system.len().saturating_sub(80)..]
    );
}

// ─────────────────────────── 会话截断（§12.4.3 conversation_max_chars）───────────────────────────

#[tokio::test]
async fn conversations_are_truncated_to_max_chars() {
    // 阈值要**明显大于两段 prompt**（否则测的就成了「原文吃光预算」，那是上一条用例）；
    // 60k 的 write_file 参数则一定撑破余量。账的口径见 `truncate_conversation`。
    let settings = Settings {
        conversation_max_chars: 40_000,
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
        .write_file("design.md", &"x".repeat(60_000))
        .submit(&ArchitectExecuteMetadata {
            readiness: true,
            ..Default::default()
        });
    ctx.agent.set_script(script);

    testkit::seed_task(&ctx.store, "t5", "p1").await.unwrap();
    admit(&ctx, "t5").await;
    ctx.executor.run("t5").await.unwrap();

    // write_file 的 60k 字符参数应让 messages 触顶截断
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
        !raw.contains("xxxxxxxxxx"),
        "超长内容不得进入落库会话（应被丢弃轮次或截断）：{} 字符",
        raw.chars().count()
    );
    // 整行的账（票 02 起含两段原文）不超过阈值
    let total = conv.system_prompt.unwrap_or_default().chars().count()
        + conv.user_prompt.unwrap_or_default().chars().count()
        + raw.chars().count();
    assert!(total <= 40_000, "整行共吃一本账，实际 {total} 字符");
}

// ─────────────────────────── 小工具单测 ───────────────────────────

#[test]
fn test_command_mapping() {
    use agentpipeline_core::pipeline::merge::test_command_for;
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
    use agentpipeline_core::pipeline::merge::parse_diff_stats;
    let stats = parse_diff_stats(" src/a.rs | 2 ++\n 1 file changed, 2 insertions(+)\n");
    assert_eq!(stats.files_changed, 1);
    assert_eq!(stats.insertions, 2);
    assert_eq!(stats.deletions, 0);
}

/// `deny` 档下环境层工具**连广告都不给**（决策 206）：tool 定义里就被摘掉，
/// 而不是只在执行点拒一次。
///
/// 反向那半段是同一条用例的一部分，而且是它真正的力量所在：同一条配置在 `auto` 档下
/// 这些工具**在**——否则「上面那条不广告」可能只是因为配置里压根没声明它们。
#[tokio::test]
async fn deny_tier_removes_env_tools_from_the_advertised_set() {
    use agentpipeline_core::types::{EnvMode, StageConfig};

    let ctx = setup("true", Settings::default()).await;
    let declared = serde_json::json!(["read_file", "run_command", "spawn_sub_agent"]);
    ctx.store
        .upsert_stage_config(&StageConfig {
            stage: "architect-design".into(),
            tools_json: Some(declared.clone()),
            env_mode: Some(EnvMode::Deny),
            updated_at: ctx.store.now(),
            ..Default::default()
        })
        .await
        .unwrap();

    let mut ban = vec![
        "read_file".to_string(),
        "write_file".to_string(),
        "edit_file".to_string(),
        "delete_file".to_string(),
        "list_dir".to_string(),
        "run_command".to_string(),
        "spawn_sub_agent".to_string(),
    ];
    ban.sort();

    // deny：环境层全军覆没，而**与档位无关的东西照旧**（校验工具、只读台账工具）
    let denied = advertised_tool_names(&ctx, "t-denied").await;
    for name in &ban {
        assert!(!denied.contains(name), "deny 档不得广告 {name}：{denied:?}");
    }
    assert!(
        denied.iter().any(|n| n == "submit_metadata"),
        "校验工具不受档位影响：{denied:?}"
    );

    // auto：同一条声明**在**（否则上面那条证明不了任何事）
    let row = ctx
        .store
        .get_stage_config("architect-design")
        .await
        .unwrap()
        .unwrap();
    ctx.store
        .upsert_stage_config(&StageConfig {
            env_mode: Some(EnvMode::Auto),
            ..row
        })
        .await
        .unwrap();
    let allowed = advertised_tool_names(&ctx, "t-allowed").await;
    for name in ["read_file", "run_command"] {
        assert!(
            allowed.contains(&name.to_string()),
            "auto 档应当广告 {name}：{allowed:?}"
        );
    }
}

/// 跑一个任务到 architect-design.validate_input，取那一次请求**广告出去的工具名**。
///
/// 广告集是这一档行为的一半（另一半在执行点）：`deny` 要「连广告都不给」，
/// 而那件事只能从真发出去的请求上看。
async fn advertised_tool_names(ctx: &Ctx, task: &str) -> Vec<String> {
    let mut script = Script::new();
    design_scripts(&mut script);
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, task, "p1").await.unwrap();
    admit(ctx, task).await;
    ctx.executor.run(task).await.unwrap();
    let requests = ctx.agent.request_log();
    requests
        .iter()
        .rev()
        .find(|r| r.stage == Stage::ArchitectDesign && r.node == Node::ValidateInput)
        .expect("architect validate_input 请求")
        .tools
        .iter()
        .map(|t| t.name.clone())
        .collect()
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
            // 声明的必须是 v1 已知工具（决策 154 的后续票：未知名字现在会被拒绝，
            // 含未知名字的配置根本走不到执行器）。这里用一个**非基线**的已知工具
            // （`Skill` 不在 `MANDATORY_TOOLS` 里）来钉「并集」那半段。
            tools_json: Some(serde_json::json!(["read_file", "Skill"])),
            skills_json: None,
            idle_timeout_sec: None,
            max_duration_sec: None,
            node_overrides_json: None,
            env_mode: None,
            max_rounds: None,
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
            env_mode: None,
            max_rounds: None,
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
    // 工具并集语义（§10.6.2：mandatory ∪ 阶段声明）：基线一个不少，声明的已知工具加进来
    assert!(vi.tools.iter().any(|t| t.name == "submit_metadata"));
    assert!(vi.tools.iter().any(|t| t.name == "read_file"));
    assert!(
        vi.tools.iter().any(|t| t.name == "Skill"),
        "声明的非基线已知工具须进广告集：{:?}",
        vi.tools.iter().map(|t| &t.name).collect::<Vec<_>>()
    );
    // 「声明了 v1 不存在的工具」这条路已关闭（决策 154 的后续票）：写入与启动都拒绝，
    // 故执行器里不再有「静默忽略」这条分支可测——那条契约在
    // `config.rs::unknown_tool_names_fail_startup_validation` 与
    // 本文件同目录的 `tool_defs_rejects_unknown_names_and_accepts_the_known_set` 上。

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
///
/// 票 03：断言对象是**用户目录技能**（临时 home 写 `skills/{name}/SKILL.md`），
/// 不再依赖内嵌常量——票 04 删除内嵌后本用例零改动。
#[tokio::test]
async fn node_scoped_skills_inject_different_bodies_per_node() {
    let external = tempfile::tempdir().unwrap();
    write_user_skill(
        external.path(),
        "grilling",
        "拷问",
        "拷问协议：走设计树的 frontier",
    );
    write_user_skill(
        external.path(),
        "to-spec",
        "规格",
        "综合成规格：守好验收标准",
    );
    let ctx = setup_with_skills_dir("true", Settings::default(), external.path()).await;
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

    // validate_input：拷问协议的正文
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

    // execute：综合成规格的正文
    assert!(
        ex.system_prompt.contains("### to-spec"),
        "{}",
        ex.system_prompt
    );
    assert!(
        ex.system_prompt.contains("守好验收标准"),
        "{}",
        ex.system_prompt
    );
    assert!(!ex.system_prompt.contains("### grilling"));

    // validate_output 未声明技能 → 无技能段（该节点上 grilling/to-spec 都未声明，
    // 故它们只以目录态出现：`- name: desc`，而非 `### name`）
    let vo = requests
        .iter()
        .find(|r| r.stage == Stage::ArchitectDesign && r.node == Node::ValidateOutput)
        .expect("architect validate_output 请求");
    assert!(
        !vo.system_prompt.contains("### grilling"),
        "{}",
        vo.system_prompt
    );
    assert!(
        !vo.system_prompt.contains("### to-spec"),
        "{}",
        vo.system_prompt
    );
}

/// 阶段级 `skills_json` 仍然生效（旧行为不回归），且与节点级**取并集**（只增不减）。
#[tokio::test]
async fn stage_level_skills_still_apply_and_union_with_node_level() {
    let external = tempfile::tempdir().unwrap();
    write_user_skill(external.path(), "grilling", "拷问", "拷问协议正文");
    // 阶段级声明**名字态**技能 `to-spec`（只有名字进 prompt），节点级再叠一个全文态技能
    write_user_skill(external.path(), "to-spec", "规格", "综合成规格正文");
    let ctx = setup_with_skills_dir("true", Settings::default(), external.path()).await;
    ctx.store
        .upsert_stage_config(&agentpipeline_core::types::StageConfig {
            stage: "architect-design".into(),
            // 阶段级声明名字态技能 `to-spec`，节点级再叠一个全文态用户技能
            skills_json: Some(serde_json::json!([
                {"name": "to-spec", "mode": "name", "trusted": true}
            ])),
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
    // 阶段级名字态技能（`- to-spec`）与节点级全文态技能（`### grilling`）并存
    assert!(
        vi.system_prompt.contains("- to-spec"),
        "{}",
        vi.system_prompt
    );
    assert!(
        vi.system_prompt.contains("### grilling"),
        "{}",
        vi.system_prompt
    );

    // execute 只有阶段级技能（节点级未声明）→ 无 grilling 正文
    let ex = requests
        .iter()
        .find(|r| r.stage == Stage::ArchitectDesign && r.node == Node::Execute)
        .expect("architect execute 请求");
    assert!(
        ex.system_prompt.contains("- to-spec"),
        "{}",
        ex.system_prompt
    );
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

// ──────────────────── 二档注入与目录态（票 05 / 决策 172④）────────────────────

/// 在技能根下写一个知识型技能（票 05 起断言对象一律是**用户目录技能**）。
fn write_user_skill(root: &std::path::Path, name: &str, description: &str, body: &str) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}"),
    )
    .unwrap();
}

/// 名字态（`mode = "name"`）不把正文带进 system prompt——正文由 `Skill` 工具按需拉取。
///
/// 这是二档注入的全部意义：常驻上下文只背名字，几十个技能不会把窗口挤满（选型 D）。
#[tokio::test]
async fn name_mode_injects_name_without_body() {
    let external = tempfile::tempdir().unwrap();
    write_user_skill(
        external.path(),
        "long-skill",
        "很长的技能",
        "机密正文不应进 prompt",
    );
    let ctx = setup_with_skills_dir("true", Settings::default(), external.path()).await;
    ctx.store
        .upsert_stage_config(&agentpipeline_core::types::StageConfig {
            stage: "architect-design".into(),
            node_overrides_json: Some(serde_json::json!({
                "validate_input": {"skills": [
                    {"name": "long-skill", "mode": "name", "trusted": true}
                ]}
            })),
            updated_at: ctx.store.now(),
            ..Default::default()
        })
        .await
        .unwrap();

    let mut script = Script::new();
    design_scripts(&mut script);
    implementation_scripts(&mut script, "t-skill-name-mode");
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t-skill-name-mode", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-skill-name-mode").await;
    ctx.executor.run("t-skill-name-mode").await.unwrap();

    let requests = ctx.agent.request_log();
    let vi = requests
        .iter()
        .find(|r| r.stage == Stage::ArchitectDesign && r.node == Node::ValidateInput)
        .expect("architect validate_input 请求");
    assert!(
        vi.system_prompt.contains("- long-skill"),
        "名字态应列出名字：{}",
        vi.system_prompt
    );
    assert!(
        !vi.system_prompt.contains("机密正文不应进 prompt"),
        "名字态不得把正文带进 prompt：{}",
        vi.system_prompt
    );
    assert!(
        !vi.system_prompt.contains("### long-skill"),
        "名字态不是全文态：{}",
        vi.system_prompt
    );
}

/// 目录态（渐进披露）：**未被声明**的可用技能进 prompt 时只有名字 + 描述，没有正文。
///
/// 这是票 05 新增的唯一可见输出，也是「模型知道有哪些能力可用、但不必预载全部正文」的落点。
#[tokio::test]
async fn undeclared_skills_appear_as_catalogue_without_body() {
    let external = tempfile::tempdir().unwrap();
    // 未声明：应出现在目录里
    write_user_skill(external.path(), "available", "可选的技能", "可选的机密正文");
    // 已声明为名字态：应只按名字态出现，不重复进目录
    write_user_skill(external.path(), "declared", "已声明的技能", "已声明正文");
    // disable-model-invocation：不进目录（选型 D）
    {
        let dir = external.path().join("manual-only");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: manual-only\ndescription: 手动触发\ndisable-model-invocation: true\n---\n\n正文",
        )
        .unwrap();
    }

    let ctx = setup_with_skills_dir("true", Settings::default(), external.path()).await;
    ctx.store
        .upsert_stage_config(&agentpipeline_core::types::StageConfig {
            stage: "architect-design".into(),
            node_overrides_json: Some(serde_json::json!({
                "validate_input": {"skills": ["declared"]}
            })),
            updated_at: ctx.store.now(),
            ..Default::default()
        })
        .await
        .unwrap();

    let mut script = Script::new();
    design_scripts(&mut script);
    implementation_scripts(&mut script, "t-skill-catalogue");
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t-skill-catalogue", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-skill-catalogue").await;
    ctx.executor.run("t-skill-catalogue").await.unwrap();

    let requests = ctx.agent.request_log();
    let vo = requests
        .iter()
        .find(|r| r.stage == Stage::ArchitectDesign && r.node == Node::ValidateOutput)
        .expect("architect validate_output 请求");
    let p = &vo.system_prompt;

    // 目录项：名字 + 描述。`declared` 是 **validate_input 的节点级声明**，
    // 在 validate_output 这个节点上它是未声明的 → 也以目录态出现（节点级作用域的直接体现）。
    assert!(p.contains("- available: 可选的技能"), "{p}");
    assert!(p.contains("- declared: 已声明的技能"), "{p}");
    // 目录态不含正文
    assert!(!p.contains("可选的机密正文"), "目录态不得含正文：{p}");
    assert!(!p.contains("已声明正文"), "目录态不得含正文：{p}");
    // disable-model-invocation 不进目录
    assert!(
        !p.contains("manual-only"),
        "手动触发技能不得自动进目录：{p}"
    );

    // validate_input 是声明所在节点：`declared` 在这里按全文态（裸字符串）注入，
    // 且**不再**以目录项重复出现
    let vi = requests
        .iter()
        .find(|r| r.stage == Stage::ArchitectDesign && r.node == Node::ValidateInput)
        .expect("architect validate_input 请求");
    assert!(
        vi.system_prompt.contains("### declared"),
        "{}",
        vi.system_prompt
    );
    assert!(
        !vi.system_prompt.contains("- declared: 已声明的技能"),
        "已声明的技能不得重复进目录：{}",
        vi.system_prompt
    );
    assert!(
        vi.system_prompt.contains("- available: 可选的技能"),
        "{}",
        vi.system_prompt
    );
}

/// 旧配置行（纯字符串数组）行为逐字不变：裸字符串按 `{mode: full, trusted: false}` 解释，
/// 正文照进 prompt——**零迁移**（决策 172④）。
#[tokio::test]
async fn legacy_string_array_declarations_still_inject_full_body() {
    let external = tempfile::tempdir().unwrap();
    write_user_skill(external.path(), "legacy", "老技能", "老配置行的正文");
    let ctx = setup_with_skills_dir("true", Settings::default(), external.path()).await;
    ctx.store
        .upsert_stage_config(&agentpipeline_core::types::StageConfig {
            stage: "architect-design".into(),
            // 旧格式：纯字符串数组，无对象、无 mode/trusted
            node_overrides_json: Some(serde_json::json!({
                "validate_input": {"skills": ["legacy"]}
            })),
            updated_at: ctx.store.now(),
            ..Default::default()
        })
        .await
        .unwrap();

    let mut script = Script::new();
    design_scripts(&mut script);
    implementation_scripts(&mut script, "t-skill-legacy");
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t-skill-legacy", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-skill-legacy").await;
    ctx.executor.run("t-skill-legacy").await.unwrap();

    let vi = ctx
        .agent
        .request_log()
        .into_iter()
        .find(|r| r.stage == Stage::ArchitectDesign && r.node == Node::ValidateInput)
        .expect("architect validate_input 请求");
    assert!(
        vi.system_prompt.contains("### legacy"),
        "旧配置行仍是全文态：{}",
        vi.system_prompt
    );
    assert!(
        vi.system_prompt.contains("老配置行的正文"),
        "{}",
        vi.system_prompt
    );
}

// ──────────────────── `Skill` 工具（票 06 / 决策 172③）────────────────────

/// 模型请求 `Skill` → 工具返回正文 → **下一轮的 `messages` 里出现该正文**。
///
/// 这是票 06 的核心可观察行为：正文走 `messages`（tool result）而非 system prompt。
#[tokio::test]
async fn skill_tool_injects_body_into_next_round_messages() {
    let external = tempfile::tempdir().unwrap();
    write_user_skill(external.path(), "grill", "拷问协议", "把设计树走完再动手");
    let ctx = setup_with_skills_dir("true", Settings::default(), external.path()).await;

    // validate_input 声明为**名字态**：正文不进 system prompt，只能靠 `Skill` 工具取
    ctx.store
        .upsert_stage_config(&agentpipeline_core::types::StageConfig {
            stage: "architect-design".into(),
            node_overrides_json: Some(serde_json::json!({
                "validate_input": {"skills": [
                    {"name": "grill", "mode": "name", "trusted": true}
                ]}
            })),
            updated_at: ctx.store.now(),
            ..Default::default()
        })
        .await
        .unwrap();

    let mut script = Script::new();
    // 第一轮：模型请求加载技能；第二轮：正常走完（design_scripts 补齐其余节点）
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .push(testkit::Step::Tool {
            name: "Skill".into(),
            arguments: serde_json::json!({"name": "grill"}),
        });
    design_scripts(&mut script);
    // design_scripts 会把 validate_input 的 submit 追加在 Skill 调用之后——顺序符合预期
    implementation_scripts(&mut script, "t-skill-tool");
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t-skill-tool", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-skill-tool").await;
    ctx.executor.run("t-skill-tool").await.unwrap();

    let requests = ctx.agent.request_log();
    let vi: Vec<_> = requests
        .iter()
        .filter(|r| r.stage == Stage::ArchitectDesign && r.node == Node::ValidateInput)
        .collect();
    assert!(
        vi.len() >= 2,
        "应有第二轮请求（工具结果回灌）：{}",
        vi.len()
    );

    // 第一轮：system prompt 只有名字（名字态），正文不在里面
    assert!(
        vi[0].system_prompt.contains("- grill"),
        "{}",
        vi[0].system_prompt
    );
    assert!(
        !vi[0].system_prompt.contains("把设计树走完再动手"),
        "名字态正文不得进 system prompt：{}",
        vi[0].system_prompt
    );

    // 第二轮：正文已作为 tool result 进入 messages
    let second = vi[1];
    let tool_results: Vec<&str> = second
        .messages
        .iter()
        .filter(|m| m.role == agentpipeline_core::agent::Role::Tool)
        .filter_map(|m| m.content.as_deref())
        .collect();
    assert!(
        tool_results
            .iter()
            .any(|c| c.contains("把设计树走完再动手")),
        "下一轮 messages 里应出现技能正文：{tool_results:?}"
    );
    // 正文仍不进 system prompt
    assert!(
        !second.system_prompt.contains("把设计树走完再动手"),
        "{}",
        second.system_prompt
    );
    // 正文不进 system prompt ⇒ prompt_template_hash 不变（票 06 的显式要求）
    assert_eq!(
        vi[0].system_prompt, second.system_prompt,
        "调用前后 system prompt 必须逐字相同"
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
    agentpipeline_core::pipeline::resume::apply_action(
        &ctx.store,
        &cursor,
        ResumeAction::Continue,
        None,
        None,
    )
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
    agentpipeline_core::pipeline::resume::apply_action(
        &ctx.store,
        &cursor,
        ResumeAction::Continue,
        None,
        None,
    )
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
    agentpipeline_core::pipeline::resume::apply_action(
        &ctx.store,
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

/// 决策 245：交互式 resume **不再是静默的**——人按完钮，实时流里要看得见。
///
/// 这条路径此前零 `emit`（`Store` 连 `sse` 字段都没有），界面只能靠重新拉取或轮询。
/// 用例走的是端点与托管动作共用的那份 `apply_resume`，断言 `stage_changed` 真的发出来了。
#[tokio::test]
async fn pressing_continue_emits_stage_changed() {
    use agentpipeline_core::pipeline::resume::{apply_resume, ResumeRequest};
    use agentpipeline_core::scheduler::ResumeFn;
    use agentpipeline_core::sse::SseEventType;

    // judge 分歧的 continue 会真的把游标从 architect.validate_output 放行出去（决策 135）
    let ctx = judge_disagreement_ctx("td-sse").await;
    let recorder = testkit::SseRecorder::new();
    // 实参位不自动把 `Arc<SseRecorder>` 收窄成 `Arc<dyn SseSink>`，两边都要留：
    // 断言要用具体类型，签名要的是 trait 对象（`SseRecorder` 是 Clone，共享同一份事件）。
    let sse: Arc<dyn agentpipeline_core::sse::SseSink> = Arc::new(recorder.clone());
    let noop: ResumeFn = Arc::new(|_| {});

    let applied = apply_resume(
        &ctx.store,
        &Settings::default(),
        &noop,
        &sse,
        "td-sse",
        &ResumeRequest {
            action: "continue".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(applied.action, "continue");

    assert!(
        recorder.count_of(SseEventType::StageChanged) >= 1,
        "人按 continue 之后必须收到 stage_changed：{:?}",
        recorder.type_sequence()
    );
    assert!(
        recorder.count_of(SseEventType::CursorChanged) >= 1,
        "每条受影响游标都要收到 cursor_changed：{:?}",
        recorder.type_sequence()
    );
    // 决策 245：resume 的落点没有一种会把游标挂起，所以这条路径不发 pending
    assert_eq!(
        recorder.count_of(SseEventType::Pending),
        0,
        "resume 不该把游标挂起"
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
    agentpipeline_core::pipeline::resume::apply_action(
        &ctx.store,
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

/// 反向不变量（票 01）：**没有任何路径能把游标停在 sync-check**。
///
/// sync-check 不占游标行（决策 107）——它只以 run 行（`agent_type = "system"`）存在，
/// 回溯由 `advance_join` 经 `SyncDecisionKind` 判定、`Store::backtrack_cursors` 落库，
/// 从不经过 `route()`。本条今天为真、删掉 `EdgeKind::Backtrack` 死代码之后仍为真；
/// 将来若有人「照文档」把 sync-check 做成占游标行的节点，它会变红。
#[tokio::test]
async fn no_cursor_row_ever_parks_at_sync_check() {
    let ctx = setup("true", Settings::default()).await;
    let mut script = Script::new();
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: false,
            blockers: vec!["占位".into()],
        });
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t-sync-inv", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-sync-inv").await;

    // 造出「两分支都到界」的 join-ready 状态，让 join 真的跑一次
    let split = ctx.store.split_cursors("t-sync-inv").await.unwrap();
    for c in &split {
        ctx.store
            .set_cursor_waiting_join(&c.cursor_id)
            .await
            .unwrap();
    }
    ctx.agent.set_script(Script::new());
    ctx.executor.run("t-sync-inv").await.unwrap();

    // ① 全部游标行（含已归档）都不停在 sync-check
    let cursors = ctx.store.load_all_cursors("t-sync-inv").await.unwrap();
    assert!(!cursors.is_empty(), "join 之后必须还有游标行");
    for c in &cursors {
        assert!(
            (c.stage, c.node) != (Stage::SyncCheck, Node::Execute),
            "游标 {}（{}）停在了 sync-check：sync-check 不占游标行（决策 107）",
            c.cursor_id,
            c.branch
        );
    }

    // ② sync-check 只以 system run 行存在，且 join 恰执行一次
    let runs = ctx
        .store
        .list_runs_at("t-sync-inv", Stage::SyncCheck, Node::Execute)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1, "join 恰执行一次（决策 107 / G5）");
    for r in &runs {
        assert_eq!(
            r.agent_type, "system",
            "sync-check 只以 system run 存在（决策 114）"
        );
    }
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
        available_skills: vec!["grilling".into()],
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

// ─────────────────────── 只读子代理（决策 172③，票 08）───────────────────────

/// 给 develop 阶段声明 `spawn_sub_agent`（扩展工具，默认关闭）。
async fn declare_sub_agent(ctx: &Ctx) {
    ctx.store
        .upsert_stage_config(&agentpipeline_core::types::StageConfig {
            stage: "develop".into(),
            tools_json: Some(serde_json::json!(["spawn_sub_agent"])),
            updated_at: ctx.store.now(),
            ..Default::default()
        })
        .await
        .unwrap();
}

/// 验收（票 08 主用例）：父派子代理 → 子代理用只读工具读文件 → 摘要回灌父 messages。
#[tokio::test]
async fn parent_spawns_readonly_subagent_and_gets_summary_back() {
    let ctx = setup("true", Settings::default()).await;
    declare_sub_agent(&ctx).await;

    // 子代理要读的文件：真实存在于 worktree（工具层全真执行，决策 148）
    let worktree = ctx.store.home().worktree_path("t-sub");
    std::fs::create_dir_all(&worktree).unwrap();
    std::fs::write(worktree.join("NOTES.md"), "关键结论：入口在 main()\n").unwrap();

    let mut script = Script::new();
    design_scripts(&mut script);
    // develop.execute：父先派子代理，再照常写代码提交
    script
        .for_node(Stage::Develop, Node::Execute)
        .push(testkit::Step::Tool {
            name: "spawn_sub_agent".into(),
            arguments: serde_json::json!({"task": "读 NOTES.md 并总结入口"}),
        })
        .write_file(
            "src/lib.rs",
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n#[test]\nfn adds() { assert_eq!(add(1, 2), 3); }\n",
        )
        .write_file("tests/acceptance.rs", "#[test]\nfn ok() {}\n")
        .run_command(
            "git add -A && git -c user.name=f -c user.email=f@l commit -m 'feat: task t-sub'",
        )
        .submit(&CodeChanges {
            branch_name: "kanban/t-sub".into(),
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
    // 子代理自己的脚本：先 read_file（真实读），再给摘要收口
    script.push_subagent(testkit::Step::Tool {
        name: "read_file".into(),
        arguments: serde_json::json!({"path": "NOTES.md"}),
    });
    script.push_subagent(testkit::Step::Text("入口在 main()（源：NOTES.md）".into()));

    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t-sub", "p1").await.unwrap();
    admit(&ctx, "t-sub").await;
    ctx.executor.run("t-sub").await.unwrap();

    // ① 子代理摘要进入了父的下一轮 messages（tool_result 通道，决策 172③）
    let requests = ctx.agent.request_log();
    let develop: Vec<_> = requests
        .iter()
        .filter(|r| r.stage == Stage::Develop && r.node == Node::Execute)
        .collect();
    let parent_round2 = develop
        .iter()
        .find(|r| {
            r.messages.iter().any(|m| {
                m.role == agentpipeline_core::agent::Role::Tool
                    && m.content
                        .as_deref()
                        .is_some_and(|c| c.contains("入口在 main()"))
            })
        })
        .expect("父的第二轮 messages 应含子代理摘要");
    assert!(parent_round2
        .messages
        .iter()
        .any(|m| m.role == agentpipeline_core::agent::Role::Tool));
}

/// 验收（票 08 安全断言）：子代理的工具集**只有** `read_file` / `list_dir`。
///
/// 这条是本票的安全边界：不是「子代理不调 run_command」，而是**它的工具定义里
/// 根本没有 run_command**——阶段声明什么都改不了。
#[tokio::test]
async fn subagent_tool_set_is_read_only() {
    let ctx = setup("true", Settings::default()).await;
    declare_sub_agent(&ctx).await;

    let mut script = Script::new();
    design_scripts(&mut script);
    script
        .for_node(Stage::Develop, Node::Execute)
        .push(testkit::Step::Tool {
            name: "spawn_sub_agent".into(),
            arguments: serde_json::json!({"task": "检索"}),
        })
        .write_file(
            "src/lib.rs",
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n#[test]\nfn adds() { assert_eq!(add(1, 2), 3); }\n",
        )
        .write_file("tests/acceptance.rs", "#[test]\nfn ok() {}\n")
        .run_command(
            "git add -A && git -c user.name=f -c user.email=f@l commit -m 'feat: task t-ro'",
        )
        .submit(&CodeChanges {
            branch_name: "kanban/t-ro".into(),
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
    script.push_subagent(testkit::Step::Text("摘要".into()));

    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t-ro", "p1").await.unwrap();
    admit(&ctx, "t-ro").await;
    ctx.executor.run("t-ro").await.unwrap();

    // 子代理那一次请求的工具集：必须是固定只读的两个
    let requests = ctx.agent.request_log();
    let sub_req = requests
        .iter()
        .find(|r| r.run.as_ref().is_some_and(|c| c.agent_type == "subagent"))
        .expect("应有子代理 LLM 请求");
    let names: Vec<&str> = sub_req.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["read_file", "list_dir"],
        "子代理工具集必须固定只读"
    );
    assert!(
        !names.contains(&"run_command"),
        "子代理不得拿到 run_command（无 OS 级沙箱，决策 19 修订 / 104）"
    );
    assert!(!names.contains(&"write_file"), "子代理不得写文件");
    assert!(
        !names.contains(&"spawn_sub_agent"),
        "深度固定一层：子代理不再派子代理（决策 9）"
    );

    // 父节点的工具集里有 run_command（对照组：断言不是「谁都没有」）
    let parent_req = requests
        .iter()
        .find(|r| r.stage == Stage::Develop && r.node == Node::Execute)
        .expect("应有父请求");
    assert!(parent_req.tools.iter().any(|t| t.name == "run_command"));
}

/// 验收（票 08）：`agent_type = "subagent"` + `parent_run_id` 落 run 行。
#[tokio::test]
async fn subagent_run_row_carries_parent_and_agent_type() {
    let ctx = setup("true", Settings::default()).await;
    declare_sub_agent(&ctx).await;

    let mut script = Script::new();
    design_scripts(&mut script);
    script
        .for_node(Stage::Develop, Node::Execute)
        .push(testkit::Step::Tool {
            name: "spawn_sub_agent".into(),
            arguments: serde_json::json!({"task": "检索"}),
        })
        .write_file(
            "src/lib.rs",
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n#[test]\nfn adds() { assert_eq!(add(1, 2), 3); }\n",
        )
        .write_file("tests/acceptance.rs", "#[test]\nfn ok() {}\n")
        .run_command(
            "git add -A && git -c user.name=f -c user.email=f@l commit -m 'feat: task t-prow'",
        )
        .submit(&CodeChanges {
            branch_name: "kanban/t-prow".into(),
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
    script.push_subagent(testkit::Step::Text("摘要".into()));

    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t-prow", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-prow").await;
    ctx.executor.run("t-prow").await.unwrap();

    let runs = ctx.store.list_runs("t-prow").await.unwrap();
    let subs: Vec<_> = runs.iter().filter(|r| r.agent_type == "subagent").collect();
    assert_eq!(subs.len(), 1, "恰好一行子代理 run：{runs:?}");
    let sub = subs[0];
    let parent = runs
        .iter()
        .find(|r| r.id == sub.parent_run_id.expect("子代理必须有 parent_run_id"))
        .expect("parent_run_id 应指向真实父 run");
    assert_eq!(parent.agent_type, "main", "父 run 是 main");
    assert_eq!(parent.stage, Stage::Develop);
    assert_eq!(parent.node, Node::Execute);
    assert_eq!(sub.stage, Stage::Develop, "子代理复用父节点坐标");
    assert_eq!(sub.node, Node::Execute);
    assert_eq!(sub.attempt, parent.attempt, "子代理不产生自己的 attempt");

    // 会话行同样带 agent_type / parent_run_id，且与父会话分开（决策 77）
    let convs = ctx.store.list_conversations("t-prow", false).await.unwrap();
    let sub_conv = convs
        .iter()
        .find(|c| c.agent_type == "subagent")
        .expect("子代理应有独立会话行");
    assert_eq!(sub_conv.parent_run_id, sub.parent_run_id);
    assert_eq!(sub_conv.run_id, sub.id);
}

/// 验收（票 08）：子代理 token 记在自己 run 行，父 run **不重复累加**。
#[tokio::test]
async fn subagent_tokens_are_counted_once_on_its_own_run() {
    let ctx = setup("true", Settings::default()).await;
    declare_sub_agent(&ctx).await;

    let mut script = Script::new();
    design_scripts(&mut script);
    script
        .for_node(Stage::Develop, Node::Execute)
        .push(testkit::Step::Tool {
            name: "spawn_sub_agent".into(),
            arguments: serde_json::json!({"task": "检索"}),
        })
        .write_file(
            "src/lib.rs",
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n#[test]\nfn adds() { assert_eq!(add(1, 2), 3); }\n",
        )
        .write_file("tests/acceptance.rs", "#[test]\nfn ok() {}\n")
        .run_command(
            "git add -A && git -c user.name=f -c user.email=f@l commit -m 'feat: task t-tok'",
        )
        .submit(&CodeChanges {
            branch_name: "kanban/t-tok".into(),
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
    script.push_subagent(testkit::Step::Text("摘要".into()));

    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t-tok", "p1").await.unwrap();
    admit(&ctx, "t-tok").await;
    ctx.executor.run("t-tok").await.unwrap();

    let runs = ctx.store.list_runs("t-tok").await.unwrap();
    let sub = runs
        .iter()
        .find(|r| r.agent_type == "subagent")
        .expect("应有子代理 run");
    // FakeAgent 每次调用 +10 prompt / +5 completion；子代理只跑了一轮 Text
    assert_eq!(sub.prompt_tokens, 10, "子代理 token 记在自己行上");
    assert_eq!(sub.completion_tokens, 5);
    assert_eq!(sub.status, NodeStatus::Success);

    // 任务总量 = 全部 run 行之和（子代理行计入 total_calls）
    let task_tokens: u64 = runs
        .iter()
        .map(|r| r.prompt_tokens as u64 + r.completion_tokens as u64)
        .sum();
    let task = ctx.store.get_task("t-tok").await.unwrap();
    assert_eq!(task.total_tokens, task_tokens, "总量与 run 行求和一致");
    assert_eq!(
        task.total_calls,
        runs.iter().filter(|r| r.agent_type != "system").count() as u64,
        "子代理计入 total_calls（决策 130②）"
    );
}

/// 验收（票 08）：子代理**不继承**阶段声明的工具——阶段开了 run_command 也扩不了权。
#[tokio::test]
async fn subagent_does_not_inherit_declared_tools() {
    let ctx = setup("true", Settings::default()).await;
    // 阶段显式声明：既开子代理，又额外声明 run_command / write_file
    ctx.store
        .upsert_stage_config(&agentpipeline_core::types::StageConfig {
            stage: "develop".into(),
            tools_json: Some(serde_json::json!([
                "spawn_sub_agent",
                "run_command",
                "write_file"
            ])),
            updated_at: ctx.store.now(),
            ..Default::default()
        })
        .await
        .unwrap();

    let mut script = Script::new();
    design_scripts(&mut script);
    script
        .for_node(Stage::Develop, Node::Execute)
        .push(testkit::Step::Tool {
            name: "spawn_sub_agent".into(),
            arguments: serde_json::json!({"task": "检索"}),
        })
        .write_file(
            "src/lib.rs",
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n#[test]\nfn adds() { assert_eq!(add(1, 2), 3); }\n",
        )
        .write_file("tests/acceptance.rs", "#[test]\nfn ok() {}\n")
        .run_command(
            "git add -A && git -c user.name=f -c user.email=f@l commit -m 'feat: task t-noninh'",
        )
        .submit(&CodeChanges {
            branch_name: "kanban/t-noninh".into(),
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
    script.push_subagent(testkit::Step::Text("摘要".into()));

    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t-noninh", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-noninh").await;
    ctx.executor.run("t-noninh").await.unwrap();

    let requests = ctx.agent.request_log();
    let sub_req = requests
        .iter()
        .find(|r| r.run.as_ref().is_some_and(|c| c.agent_type == "subagent"))
        .expect("应有子代理请求");
    let names: Vec<&str> = sub_req.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["read_file", "list_dir"],
        "阶段声明的工具不得传给子代理"
    );
}

/// 验收（票 08）：**未声明** `spawn_sub_agent` 时，工具定义里根本没有它
/// （默认关闭），父代理不会拿到一个假的指针。
#[tokio::test]
async fn spawn_sub_agent_absent_unless_declared() {
    let ctx = setup("true", Settings::default()).await;

    let mut script = Script::new();
    design_scripts(&mut script);
    implementation_scripts(&mut script, "t-nosub");
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t-nosub", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-nosub").await;
    ctx.executor.run("t-nosub").await.unwrap();

    let requests = ctx.agent.request_log();
    for req in &requests {
        assert!(
            !req.tools.iter().any(|t| t.name == "spawn_sub_agent"),
            "未声明时不得出现 spawn_sub_agent：{:?}",
            req.tools.iter().map(|t| &t.name).collect::<Vec<_>>()
        );
    }
}

/// 票 08（安全，回归）：子代理的工具集是**强制**的，不只是「广告里没写」。
///
/// 模型完全可能无视 tool 定义直接发一个 `run_command` tool_call——工具分发只按名字
/// 路由，因此仅限制 advertised defs 等于没有边界。这条用例让子代理**真的**去调
/// `run_command` 与 `write_file`，断言两者都被拒绝、且磁盘上没留下痕迹。
#[tokio::test]
async fn subagent_cannot_execute_tools_outside_its_readonly_set() {
    let ctx = setup("true", Settings::default()).await;
    declare_sub_agent(&ctx).await;

    let worktree = ctx.store.home().worktree_path("t-enforce");
    std::fs::create_dir_all(&worktree).unwrap();

    let mut script = Script::new();
    design_scripts(&mut script);
    script
        .for_node(Stage::Develop, Node::Execute)
        .push(testkit::Step::Tool {
            name: "spawn_sub_agent".into(),
            arguments: serde_json::json!({"task": "去跑命令并写文件"}),
        })
        .write_file(
            "src/lib.rs",
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n#[test]\nfn adds() { assert_eq!(add(1, 2), 3); }\n",
        )
        .write_file("tests/acceptance.rs", "#[test]\nfn ok() {}\n")
        .run_command(
            "git add -A && git -c user.name=f -c user.email=f@l commit -m 'feat: task t-enforce'",
        )
        .submit(&CodeChanges {
            branch_name: "kanban/t-enforce".into(),
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
    // 子代理无视自己的只读工具集，硬发写文件与跑命令
    script.push_subagent(testkit::Step::Tool {
        name: "write_file".into(),
        arguments: serde_json::json!({"path": "SUBAGENT_WROTE.txt", "content": "越权"}),
    });
    script.push_subagent(testkit::Step::Tool {
        name: "run_command".into(),
        arguments: serde_json::json!({"command": "echo pwned > SUBAGENT_RAN.txt"}),
    });
    script.push_subagent(testkit::Step::Text("摘要".into()));

    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t-enforce", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-enforce").await;
    ctx.executor.run("t-enforce").await.unwrap();

    // ① 写文件被拒：worktree 里不得出现子代理写的文件
    assert!(
        !worktree.join("SUBAGENT_WROTE.txt").exists(),
        "子代理不得写文件——工具须被强制拒绝，而不是只在 tool 定义里缺席"
    );
    // ② 跑命令被拒：磁盘上不得留下命令副作用
    assert!(
        !worktree.join("SUBAGENT_RAN.txt").exists(),
        "子代理不得执行命令"
    );
    // ③ 两次越权都作为 tool_result 文本回到子代理（不烧父节点的 tool_retry_max）
    let requests = ctx.agent.request_log();
    let sub_reqs: Vec<_> = requests
        .iter()
        .filter(|r| r.run.as_ref().is_some_and(|c| c.agent_type == "subagent"))
        .collect();
    let refusal = sub_reqs
        .last()
        .expect("应有子代理请求")
        .messages
        .iter()
        .filter(|m| m.role == agentpipeline_core::agent::Role::Tool)
        .filter_map(|m| m.content.as_deref())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        refusal.contains("write_file") || refusal.contains("只读"),
        "越权调用须有可归因的拒绝文本：{refusal}"
    );
}

// ══════════════ 会话续接（决策 180 / 205，票 13 / 01）══════════════
//
// 续不续接由**原因**决定（决策 205 的判定表，`types.rs::resume_continues`），
// 不再由阶段 / 节点参数决定——那层配置整层退场。
//
// 分界一句话：**模型的自动失败重试不给续接，人的介入才给**（决策 33 不变）。
// 故本组用例成对出现：一个「原因说 true」的现场（信息不足补充后继续）与一个
// 「原因说 false」的现场（`context_overflow` 解除后重开），两者都拿同一份
// 「上一 attempt 的会话行就在库里」的前提，差别只在原因。

/// 造出「architect-design.validate_input 挂 pending(info_insufficient)」的现场。
///
/// round 1 的脚本带一次 `submit_metadata` 工具调用，故会话行里**有** messages 可续接
/// （否则起点本来就是空的，用例会退化成什么都没证明）。
async fn info_insufficient_ctx(task_id: &str) -> Ctx {
    let ctx = setup("true", Settings::default()).await;
    let mut script = Script::new();
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: false,
            blockers: vec!["需要明确部署环境".into()],
        });
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, task_id, "p1").await.unwrap();
    admit(&ctx, task_id).await;
    ctx.executor.run(task_id).await.unwrap();
    ctx
}

/// 判定表说 **false** 的原因：重入同一节点，起点仍是空的（决策 205）。
///
/// 现场用 `context_overflow` 转「更换长上下文模型」：它是表里 false 的一档
/// （上下文溢出的人为处置之后重开一段更干净），而且它**恰好重入同一个节点**——
/// 上一 attempt 的会话行就在库里躺着（`context_overflow_path_writes_a_conversation_row`
/// 钉住了那一行真的被写下来），故这条断言有牙齿：判定表若把它写成 true，这里会立刻读到
/// 一段非空的起点。
///
/// 本用例同时是票 04 的验收：**按下去之后 pending 真的解除**、任务回到可被调度器准入的状态。
#[tokio::test]
async fn a_cause_that_says_no_starts_from_an_empty_conversation() {
    use agentpipeline_core::types::Provider;

    let ctx = context_overflow_ctx("cont-default").await;
    let cursor = ctx
        .store
        .load_live_cursors("cont-default")
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(
        cursor.pending_reason.as_ref().unwrap().kind,
        PendingKind::ContextOverflow
    );

    // 人按下「更换长上下文模型」：换一个窗口够大的 provider。
    ctx.store
        .upsert_provider(&Provider {
            id: "prov-big".into(),
            vendor: "deepseek".into(),
            model: "deepseek-chat".into(),
            context_window: 200_000,
            base_url: None,
            api_key: None,
            enabled: true,
            created_at: ctx.store.now(),
            updated_at: ctx.store.now(),
        })
        .await
        .unwrap();
    let cleared = ctx
        .store
        .apply_model_override("cont-default", "prov-big")
        .await
        .unwrap();
    assert_eq!(cleared, 1, "卡在 context_overflow 上的那一个游标应被解除");
    // 游标不再 pending、任务回 queued（`try_admit` 只认 queued）
    let cursor = ctx
        .store
        .load_live_cursors("cont-default")
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    assert_ne!(cursor.status, CursorStatus::Pending);
    assert!(cursor.pending_reason.is_none());
    let task = ctx.store.get_task("cont-default").await.unwrap();
    assert_eq!(task.status, TaskStatus::Queued);
    assert!(task.pending_reason.is_none(), "任务上的待办投影也要清掉");

    // 再跑一次（调度器准入的结果）：同一节点重入，起点为空
    admit(&ctx, "cont-default").await;
    let mut rerun = Script::new();
    rerun
        .for_node(Stage::ArchitectDesign, Node::Execute)
        .write_file("design.md", "# 设计\n## 验收标准\n- AC-1 能登录\n")
        .submit(&ArchitectExecuteMetadata {
            readiness: true,
            affected_files: vec!["src/lib.rs".into()],
            new_symbols: vec![],
            acceptance_criteria: vec![],
            ..Default::default()
        });
    ctx.agent.set_script(rerun);
    ctx.executor.run("cont-default").await.unwrap();

    // 「起点为空」只能对**每条 run 的首条请求**断言：一次 attempt 内的工具往来会逐轮累加
    // （第 2 条请求带着本轮自己的 assistant/tool 消息），那不是跨 attempt 的遗留。
    let requests = ctx.agent.request_log();
    let vi: Vec<&LlmRequest> = requests
        .iter()
        .filter(|r| r.stage == Stage::ArchitectDesign && r.node == Node::Execute)
        .filter(|r| {
            r.run
                .as_ref()
                .is_some_and(|c| c.agent_type.as_str() == "main")
        })
        .collect();
    let mut first_per_run: Vec<(i64, usize)> = Vec::new();
    for r in &vi {
        let run_id = r.run.as_ref().expect("主 agent 请求须带 run 上下文").run_id;
        if first_per_run.last().map(|(id, _)| *id) != Some(run_id) {
            first_per_run.push((run_id, r.messages.len()));
        }
    }
    assert_eq!(
        first_per_run.len(),
        2,
        "溢出那一轮 + 解除之后重入的一轮，各一条 run：{first_per_run:?}"
    );
    assert_eq!(first_per_run[0].1, 0, "首轮起点为空：{first_per_run:?}");
    assert!(
        first_per_run[1].1 == 0,
        "原因说 false（context_overflow）时重入必须干净起跑；非空说明判定表错了或链接漏了：         {first_per_run:?}"
    );
    // 上一 attempt 的会话行确实在库里——否则上面那条断言是空转（起点为空只是因为没东西可续）
    let convs = ctx
        .store
        .list_conversations("cont-default", true)
        .await
        .unwrap();
    assert!(
        convs
            .iter()
            .any(|c| c.agent_type == "main" && c.messages_json.as_array().unwrap().len() >= 2),
        "上一 attempt 须留下可续接的会话行（否则断言无意义）：{convs:?}"
    );
}

/// 判定表说 **true** 的原因：resume 重入的第一条请求带着上一轮的 messages。
///
/// 现场是 `info_insufficient`（补充输入后重入同一节点）——决策 205 的 true 那一栏
/// 第一条就是这个。**开关已不存在**：值只由原因决定，故这条用例不需要配任何东西。
#[tokio::test]
async fn a_cause_that_says_yes_carries_the_previous_attempt_messages() {
    let ctx = info_insufficient_ctx("cont-on").await;
    let cursor = ctx
        .store
        .load_live_cursors("cont-on")
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();

    let mut rerun = Script::new();
    rerun
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    ctx.agent.set_script(rerun);
    agentpipeline_core::pipeline::resume::apply_action(
        &ctx.store,
        &cursor,
        ResumeAction::Continue,
        None,
        Some("生产 k8s"),
    )
    .await
    .unwrap();
    ctx.executor.run("cont-on").await.unwrap();

    let requests = ctx.agent.request_log();
    let vi: Vec<&LlmRequest> = requests
        .iter()
        .filter(|r| r.stage == Stage::ArchitectDesign && r.node == Node::ValidateInput)
        .collect();
    assert!(vi.len() >= 2, "validate_input 应重跑：{}", vi.len());
    // 首轮为空起点；重入那一轮带回了上一轮的工具往来
    assert!(
        vi[0].messages.is_empty(),
        "首轮起点为空：{:?}",
        vi[0].messages
    );
    let carried = &vi.last().unwrap().messages;
    assert!(carried.len() >= 2, "重入须带回上一轮 messages：{carried:?}");
    // 带回的是**上一轮**的内容：那条 submit_metadata 的 assistant 消息
    assert!(
        carried
            .iter()
            .any(|m| m.tool_calls.iter().any(|c| c.name == "submit_metadata")),
        "带回的应是上一轮的工具往来：{carried:?}"
    );
    // wire 顺序不依赖它：system / user 仍由 prompt 字段承载（messages 只接在其后）
    assert!(
        carried
            .iter()
            .all(|m| m.role != agentpipeline_core::agent::client::Role::System),
        "messages 里不得混入 system（wire 顺序由适配器保证）：{carried:?}"
    );
}

/// 干净重试不受续接影响：`agent_retry_max` 的第 2、3 次仍是空起点（决策 33 不变）。
///
/// 与原因无关——**模型的自动失败重试不给续接**，哪怕这一轮的原因表说 true。
#[tokio::test]
async fn clean_retry_after_a_tool_failure_stays_empty_whatever_the_cause_says() {
    let ctx = info_insufficient_ctx("cont-retry").await;
    let cursor = ctx
        .store
        .load_live_cursors("cont-retry")
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();

    // 重入这一轮：第 1 次尝试以「元数据始终抽不出」失败，第 2 次成功 → 两次都在同一轮里
    let mut rerun = Script::new();
    rerun
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .text("这不是结构化元数据，抽取必然失败");
    ctx.agent.set_script(rerun);
    agentpipeline_core::pipeline::resume::apply_action(
        &ctx.store,
        &cursor,
        ResumeAction::Continue,
        None,
        None,
    )
    .await
    .unwrap();
    ctx.executor.run("cont-retry").await.unwrap();

    let requests = ctx.agent.request_log();
    let vi: Vec<&LlmRequest> = requests
        .iter()
        .filter(|r| r.stage == Stage::ArchitectDesign && r.node == Node::ValidateInput)
        .filter(|r| {
            r.run
                .as_ref()
                .is_some_and(|c| c.agent_type.as_str() == "main")
        })
        .collect();
    assert!(vi.len() >= 3, "首轮 + 本轮两次尝试：{}", vi.len());
    // 按 run 分组看首条请求：三条 run = 首轮 / 本轮的续接尝试 / 本轮的干净重试。
    // 续接**只**作用于第二条（resume 重入的那次 attempt），第三条必须回到空起点。
    let mut first_per_run: Vec<(i64, usize)> = Vec::new();
    for r in &vi {
        let run_id = r.run.as_ref().expect("主 agent 请求须带 run 上下文").run_id;
        if first_per_run.last().map(|(id, _)| *id) != Some(run_id) {
            first_per_run.push((run_id, r.messages.len()));
        }
    }
    assert!(
        first_per_run.len() >= 3,
        "首轮 + 续接的那次 + 至少一次干净重试：{first_per_run:?}"
    );
    assert_eq!(first_per_run[0].1, 0, "首轮起点为空：{first_per_run:?}");
    assert!(
        first_per_run[1].1 > 0,
        "续接的那次尝试须带回上一轮的对话（否则本用例是空转）：{first_per_run:?}"
    );
    assert!(
        first_per_run[2..].iter().all(|(_, n)| *n == 0),
        "干净重试的起点必须为空（决策 33）；`agent_retry_max` 有几次就几次：{first_per_run:?}"
    );
    // 票 03 的回归：**链接也只落在续接那一轮**。此前它写在循环里、只看
    // `continuation.is_some()`，于是干净重试轮也指回同一条历史，而
    // `metrics::total_tokens` 会把被指到的历史排进排除集——同一段历史被排除两次，
    // 任务是 token **少算**（不是双算），且随重试次数漂移。
    let linked: Vec<i64> = ctx
        .store
        .list_runs("cont-retry")
        .await
        .unwrap()
        .iter()
        .filter_map(|r| r.continued_from_run_id)
        .collect();
    assert_eq!(
        linked.len(),
        1,
        "恰有一条 run 记下续接来源（干净重试轮不得再落链）：{linked:?}"
    );
}

/// 必要条件二：续接的 run 打上 `continued_from_run_id`，任务 token 总量不双算。
#[tokio::test]
async fn continued_run_links_back_so_tokens_are_not_double_counted() {
    use agentpipeline_core::metrics::total_tokens;

    let ctx = info_insufficient_ctx("cont-tokens").await;
    let cursor = ctx
        .store
        .load_live_cursors("cont-tokens")
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let first_run = ctx
        .store
        .list_runs("cont-tokens")
        .await
        .unwrap()
        .last()
        .unwrap()
        .id;

    let mut rerun = Script::new();
    rerun
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    ctx.agent.set_script(rerun);
    agentpipeline_core::pipeline::resume::apply_action(
        &ctx.store,
        &cursor,
        ResumeAction::Continue,
        None,
        None,
    )
    .await
    .unwrap();
    ctx.executor.run("cont-tokens").await.unwrap();

    let runs = ctx.store.list_runs("cont-tokens").await.unwrap();
    let linked: Vec<&agentpipeline_core::types::NodeRun> = runs
        .iter()
        .filter(|r| r.continued_from_run_id.is_some())
        .collect();
    assert_eq!(linked.len(), 1, "恰有一条 run 记下续接来源：{runs:?}");
    assert_eq!(
        linked[0].continued_from_run_id,
        Some(first_run),
        "链接须指向被续接的那条历史 run"
    );

    // 汇总口径排除被续接的历史（FakeAgent 每轮 token 相同，故排除前后差一条的量）
    let naive: u64 = runs
        .iter()
        .map(agentpipeline_core::metrics::run_tokens)
        .sum();
    let counted = total_tokens(&runs);
    assert!(
        counted < naive,
        "被续接的历史须从汇总里排除：counted={counted} naive={naive}"
    );
    // 落库的任务总量与函数口径同源
    let task = ctx.store.get_task("cont-tokens").await.unwrap();
    assert_eq!(task.total_tokens as u64, counted);
}

/// 造出「`architect-design.execute` 卡在 `context_overflow`」的现场。
///
/// 窗口极小（1000）：软限 600 / 硬限 900，一次大块元数据即越过。两条用例共用它——
/// 一条钉「这条退出路径补写会话行」，一条钉「解除之后按原因表干净起跑」。
async fn context_overflow_ctx(task_id: &str) -> Ctx {
    use agentpipeline_core::types::Provider;

    let ctx = setup("true", Settings::default()).await;
    // 窗口极小（1000）：软限 600 / 硬限 900，一次大块元数据即越过
    ctx.store
        .upsert_provider(&Provider {
            id: "prov-ctx".into(),
            vendor: "deepseek".into(),
            model: "deepseek-chat".into(),
            context_window: 1_000,
            base_url: None,
            api_key: None,
            enabled: true,
            created_at: ctx.store.now(),
            updated_at: ctx.store.now(),
        })
        .await
        .unwrap();
    ctx.store
        .upsert_stage_config(&agentpipeline_core::types::StageConfig {
            stage: "architect-design".into(),
            provider_id: Some("prov-ctx".into()),
            updated_at: ctx.store.now(),
            ..Default::default()
        })
        .await
        .unwrap();

    let mut script = Script::new();
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
            affected_files: (0..4000).map(|i| format!("src/module_{i}.rs")).collect(),
            new_symbols: vec![],
            acceptance_criteria: vec![],
            ..Default::default()
        });
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, task_id, "p1").await.unwrap();
    admit(&ctx, task_id).await;
    ctx.executor.run(task_id).await.unwrap();

    let cursor = ctx
        .store
        .load_live_cursors(task_id)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(
        cursor.pending_reason.as_ref().unwrap().kind,
        PendingKind::ContextOverflow,
        "先确认真的走到了那条退出路径"
    );
    ctx
}

/// 必要条件一：`context_overflow` 这条退出路径**补写会话行**。
///
/// 它在会话落库之前返回，修复之前「该续接却读不到上一轮」是一条静默无效的路。
#[tokio::test]
async fn context_overflow_path_writes_a_conversation_row() {
    let ctx = context_overflow_ctx("cont-overflow").await;

    // 该 run 的会话行存在（修复前它是空的）
    let runs = ctx.store.list_runs("cont-overflow").await.unwrap();
    let convs = ctx
        .store
        .list_conversations("cont-overflow", true)
        .await
        .unwrap();
    let overflow_run = runs
        .iter()
        .find(|r| r.stage == Stage::ArchitectDesign && r.node == Node::Execute)
        .expect("architect-design.execute 应有 run 行");
    assert!(
        convs
            .iter()
            .any(|c| c.run_id == overflow_run.id && c.agent_type == "main"),
        "context_overflow 退出路径须补写会话行（票 13 必要条件一）：{convs:?}"
    );
}

// ─────────────── unstick：摘掉进程内去重，重跑才真的发生（决策 210⑧ / 票 09）───────────────

/// 票 09 的核心断言：**「重跑真的发生了」要打在新 run 行出现上**，不是打在「owner 列为空」上
/// ——后者清个 DB 字段就能满足，而那正是这条要防的假绿。
///
/// **决策 226 之后这一条的写法变了**：`unstick` 会先请求中止，卡住的执行体自己收口，
/// 于是「重跑」不再由那个被判死的旧执行体顺手跑出来（此前它靠放闸放行、跑完自己的三轮回试
/// 凑出新 run 行——那是巧合，不是这次要钉的东西）。现在新 run 行只能来自一次真正的恢复。
#[tokio::test]
async fn unsticking_releases_the_in_process_dedup_and_allows_a_rerun() {
    use agentpipeline_core::pipeline::unstick::unstick;

    let ctx = setup("true", Settings::default()).await;
    // 闸门不放行：执行体停在第一次模型调用上（`_tx` 一直活着）
    let (_tx, rx) = tokio::sync::oneshot::channel::<()>();
    let calls = Arc::new(AtomicUsize::new(0));
    let llm: Arc<dyn LlmClient> = Arc::new(BlockingAgentSimple {
        gate: Arc::new(tokio::sync::Mutex::new(Some(rx))),
        calls: calls.clone(),
    });
    let executor = Arc::new(Executor::new(
        ctx.store.clone(),
        Settings::default(),
        Arc::new(ctx.sse.clone()),
        llm,
        Arc::new(ctx.killer.clone()),
    ));

    testkit::seed_task(&ctx.store, "t-hang", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-hang").await;

    // 第一个执行体卡在 LLM 调用上（进程内去重持有 t-hang）
    let hanging = {
        let e = executor.clone();
        tokio::spawn(async move { e.run("t-hang").await })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while calls.load(Ordering::SeqCst) < 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("执行体应进行到第一次 LLM 调用");
    // 时钟推一分钟：心跳在这段时间里停了（ManualClock 冻结时 `now - last` 恒为 0，
    // 而「有主但心跳停了」这条判据需要它真的停过）
    ctx.clock.advance_secs(61);
    let before = ctx.store.list_runs("t-hang").await.unwrap().len();

    // 不看门：第二个执行体被**进程内去重**拒掉（这正是「清了 DB 也没用」的机制）
    assert!(
        !executor.try_run("t-hang").await.unwrap(),
        "去重仍持有它，第二次 try_run 被拒"
    );

    // unstick：摘去重 + 清 owner + 转 pending。任务此刻仍在 running（执行体卡着），
    // 这正是实证里的形状——僵死 run 判定的宽限设为 0，免得测试要等 10 分钟。
    let unstuck = unstick(
        &ctx.store,
        &agentpipeline_core::pipeline::executor::force_release,
        "t-hang",
        agentpipeline_core::clock::Clock::now(&ctx.clock),
        chrono::Duration::zero(),
    )
    .await
    .unwrap();
    assert!(unstuck.run_id > 0);
    assert!(
        ctx.store
            .get_task("t-hang")
            .await
            .unwrap()
            .executor_owner
            .is_none(),
        "占用已清"
    );

    // 决策 226：`unstick` 先**请求中止**再摘去重，所以那个卡住的执行体自己就收口了——
    // 不必再放闸。此前它只能靠人放闸或重启进程才动，这正是「清了 DB 也没用」的由来。
    tokio::time::timeout(Duration::from_secs(5), hanging)
        .await
        .expect("unstick 之后，卡住的执行体应当在有界时间内收口")
        .unwrap()
        .unwrap();

    // 关键断言：现在同一个 executor **真的能再取得执行权**
    assert!(
        executor.try_run("t-hang").await.unwrap(),
        "unstick 之后 try_run 应当取得执行权"
    );

    // 而「真的再跑一轮」得等一次 resume：`unstick` 把游标转成了 pending，那一轮本来
    // 就不该执行节点——这正是 pending 的语义（此处钉住它，免得「取得执行权」被读成
    // 「已经重跑了」）。
    assert_eq!(
        ctx.store.list_runs("t-hang").await.unwrap().len(),
        before,
        "游标还在 pending 时，取得执行权不得执行节点"
    );
    let cursor = ctx
        .store
        .load_live_cursors("t-hang")
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    agentpipeline_core::pipeline::resume::apply_action(
        &ctx.store,
        &cursor,
        ResumeAction::Continue,
        None,
        None,
    )
    .await
    .unwrap();
    executor.try_run("t-hang").await.unwrap();
    let after = ctx.store.list_runs("t-hang").await.unwrap().len();
    assert!(
        after > before,
        "恢复之后重跑发生了：run 行由 {before} 增到 {after}"
    );
}

/// 正常在跑的任务不能被 unstick（否则它会变成「随便踢一脚」）。
#[tokio::test]
async fn a_healthy_running_task_cannot_be_unstuck() {
    use agentpipeline_core::pipeline::unstick::unstick;

    let ctx = setup("true", Settings::default()).await;
    testkit::seed_task(&ctx.store, "t-ok", "p1").await.unwrap();
    admit(&ctx, "t-ok").await;
    ctx.store
        .try_claim_executor("t-ok", "executor:live")
        .await
        .unwrap();
    let cursor = ctx.store.load_live_cursors("t-ok").await.unwrap()[0].clone();
    // 一条**心跳新鲜**的 running run
    let run_id = ctx
        .store
        .insert_run(&agentpipeline_core::storage::observability::NewRun {
            task_id: "t-ok".into(),
            cursor_id: cursor.cursor_id.clone(),
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
    ctx.store.touch_run_heartbeat(run_id).await.unwrap();

    let err = unstick(
        &ctx.store,
        &agentpipeline_core::pipeline::executor::force_release,
        "t-ok",
        agentpipeline_core::clock::Clock::now(&ctx.clock),
        chrono::Duration::minutes(10),
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("没有卡住"),
        "健康任务不该被踢：{err}"
    );
    assert_eq!(
        ctx.store
            .get_task("t-ok")
            .await
            .unwrap()
            .executor_owner
            .as_deref(),
        Some("executor:live"),
        "占用没被动过"
    );
}
