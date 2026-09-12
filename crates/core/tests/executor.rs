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
    let home = TestHome::new().unwrap();
    let clock = ManualClock::fixed();
    let store = Store::open(home.home().clone(), Arc::new(clock.clone()))
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

// ─────────────────────────── 分支 pending 隔离（决策 89 / 82）───────────────────────────

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
