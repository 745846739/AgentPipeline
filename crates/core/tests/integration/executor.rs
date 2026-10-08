//! executor 循环集成测试（票 11；testing.md §6 执行器循环 + §8 E2E-01 的 L2 层）。
//!
//! FakeAgent 只替换 LLM 响应流，工具层 / git / 命令记录全部真实执行（决策 148）。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agentpipeline_core::agent::client::{AgentResponse, LlmClient, LlmRequest, Role};
use agentpipeline_core::agent::providers::LlmErrorKind;
use agentpipeline_core::config::Settings;
use agentpipeline_core::pipeline::Executor;
use agentpipeline_core::pipeline::LLM_TRANSPORT_RESEND_MAX;
use agentpipeline_core::scheduler::KanbanScheduler;
use agentpipeline_core::sse::{SseEvent, SseEventType};
use agentpipeline_core::storage::decisions::MergeDecision;
use agentpipeline_core::storage::decisions::ResumeAction;
use agentpipeline_core::storage::observability::NewProjectRun;
use agentpipeline_core::storage::Store;
use agentpipeline_core::types::{
    AcceptanceCriterion, Approval, ArchitectExecuteMetadata, CodeChanges, CursorStatus,
    DevelopDesignMetadata, FileAction, FileChangeSpec, Gate, Node, NodeCursor, NodeStatus,
    PendingKind, Project, ReviewRequiredChange, ReviewResult, Stage, TaskStatus,
    TestDesignMetadata, TestResult, TestScenario, TransitionTrigger, ValidateInputMetadata,
    ValidateOutputMetadata,
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
    setup_with_repo(framework, settings, Repo::clean().unwrap()).await
}

/// 同 [`setup`]，但仓由调用方提供（决策 393 的 push 测试要用带 remote 的仓）。
async fn setup_with_repo(framework: &str, settings: Settings, repo: Repo) -> Ctx {
    setup_impl(framework, settings, None, repo).await
}

/// 同 [`setup`]，但技能根被 `[skills] dir` 覆盖到外部目录（决策 172）。
async fn setup_with_skills_dir(
    framework: &str,
    settings: Settings,
    skills_dir: &std::path::Path,
) -> Ctx {
    setup_impl(
        framework,
        settings,
        Some(skills_dir.to_path_buf()),
        Repo::clean().unwrap(),
    )
    .await
}

async fn setup_impl(
    framework: &str,
    settings: Settings,
    skills_dir: Option<std::path::PathBuf>,
    repo: Repo,
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
                        changed_files: vec![
                FileChangeSpec {
                    path: "src/lib.rs".into(),
                    action: FileAction::Create,
                    content_hash: None,
                },
                FileChangeSpec {
                    path: "tests/acceptance.rs".into(),
                    action: FileAction::Create,
                    content_hash: None,
                },
            ],
            unit_test_files: vec![],
            no_changes: false,
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
        .apply_merge_decision("t1", MergeDecision::Approve, false)
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

/// 把任务一路推到 merge 阶段 A 末尾（等审批）——决策 393 两条 push 测试的公共前缀。
/// task_id 必须全进程唯一：executor 的执行权注册表（决策 36）按 task_id 去重，
/// 两条测试并行跑同名任务会让后到的那次 `run` 直接让权、任务原地不动。
async fn drive_to_merge_approval(ctx: &Ctx, task_id: &str) {
    let mut script = Script::new();
    design_scripts(&mut script);
    implementation_scripts(&mut script, task_id);
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, task_id, "p1").await.unwrap();
    admit(ctx, task_id).await;
    ctx.executor.run(task_id).await.unwrap();
    let task = ctx.store.get_task(task_id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Pending, "应停在等审批");
}

// ─────────────────────────── 合入后 push（决策 393）───────────────────────────

/// 勾了 push 但仓没有 remote：跳过、不算失败，合入照常收尾到 done。
#[tokio::test]
async fn merge_push_flag_with_no_remote_skips_and_still_lands_done() {
    let ctx = setup("true", Settings::default()).await; // 默认仓没有配置任何 remote
    drive_to_merge_approval(&ctx, "t-push-noremote").await;

    ctx.store
        .apply_merge_decision("t-push-noremote", MergeDecision::Approve, true)
        .await
        .unwrap();
    ctx.executor.run("t-push-noremote").await.unwrap();

    let task = ctx.store.get_task("t-push-noremote").await.unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Done,
        "无 remote 只跳过 push，不拦收尾"
    );
    let merge = ctx
        .store
        .merge_metadata("t-push-noremote")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(merge.status, agentpipeline_core::types::MergeStatus::Merged);
    assert!(merge.push_after_merge, "开关随决策落库");
}

/// 勾了 push 且有 remote：合入后默认分支真的推到了远端。
#[tokio::test]
async fn merge_push_flag_pushes_default_branch_to_the_remote() {
    let (repo, remote) = Repo::with_remote().unwrap();
    let ctx = setup_with_repo("true", Settings::default(), repo).await;
    let local_main = ctx.repo.head("main");
    drive_to_merge_approval(&ctx, "t-push-remote").await;

    ctx.store
        .apply_merge_decision("t-push-remote", MergeDecision::Approve, true)
        .await
        .unwrap();
    ctx.executor.run("t-push-remote").await.unwrap();

    let task = ctx.store.get_task("t-push-remote").await.unwrap();
    assert_eq!(task.status, TaskStatus::Done);
    // 远端 main 与本地合入后的 main 指到同一个 commit（推的就是合入结果）
    assert_eq!(remote.head("main"), ctx.repo.head("main"));
    assert_ne!(remote.head("main"), local_main, "远端应前进");
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
        .apply_merge_decision(task_id, MergeDecision::Return, false)
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
    // 决策 301：详情原文与结果也随流水线的事件出去（摘要给收起行，详情给展开）。
    assert!(
        matches!(
            starts.first(),
            Some(SseEvent::ToolEvent { args, .. }) if args.contains("design.md")
        ),
        "完整参数原文应含文件名：{:?}",
        starts.first()
    );
    assert!(
        matches!(
            ends.first(),
            Some(SseEvent::ToolEvent { result: Some(r), .. }) if !r.is_empty()
        ),
        "end 事件要带非空结果：{:?}",
        ends.first()
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
    tokio::time::timeout(Duration::from_secs(30), async {
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
    tokio::time::timeout(Duration::from_secs(30), async {
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

/// **永不返回**的替身（票 02 / 决策 303）：executor 的 future 停在这里，永远不回来。
///
/// 与 `StallingAgent` 的差别是**没有闸**——那个放闸后还会回来（协作式中止够得着），
/// 而这条要造的是「协作式中止**够不着**」的形态：`run_inner` 无论等多久都不会返回，
/// 于是执行权只能靠「判终态那一处当场放开」，不能靠它自己收口让出来。
struct PendingAgent;

impl LlmClient for PendingAgent {
    fn complete(
        &self,
        _request: LlmRequest,
    ) -> BoxFuture<'static, agentpipeline_core::Result<AgentResponse>> {
        Box::pin(std::future::pending())
    }
}

/// 等执行体真的起来：任务被持有执行权 + 落了一条 running run（票 02 的用例都要这个起点）。
/// 抓任务**第一条** active run + 执行权，两样都在场才返回 run id。
///
/// ⚠️ 它抓的是**第一条**：`init.execute`（纯代码节点）的 run 完成得快但**不是零耗时**，
/// 起跑的一瞬可能先抓到**它**——如果随后才拨时钟，真正挂起的 agent run 起在**拨后**
/// （started_at 是新时刻），看门狗看它是新鲜的，「判超时」的断言就落空
/// （2026-09-30 runner 实测，决策 343）。要看门狗判「这个 run 超时」的用例，
/// 用 [`wait_for_running_validate_input`] 把等待钉在真正会挂起的那个 agent 节点上；
/// 只等「跑起来了」、且断言与拨时钟无关的（如 t-ok 那条）才用它。
async fn wait_for_a_held_running_run(ctx: &Ctx, task_id: &str) -> i64 {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let owned = ctx
                .store
                .get_task(task_id)
                .await
                .unwrap()
                .executor_owner
                .is_some();
            let run = ctx
                .store
                .active_runs()
                .await
                .unwrap()
                .into_iter()
                .find(|r| r.task_id.as_deref() == Some(task_id));
            if owned {
                if let Some(run) = run {
                    return run.id;
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("执行体应当起来：持有执行权 + 落一条 running run")
}

/// **票 02（决策 303，显式修订决策 226 的一格）**：run 被判终态的**同一处**放开执行权，
/// 与那个 future 会不会返回**无关**——哪怕它永远不返回。
///
/// 这是 2026-09-27 那次的直接反面：执行体停在一次不返回的同步文件读里，看门狗把 run 判了
/// timeout，而执行权的两半（进程内去重 + `executor_owner`）仍被它占着；03:49 的 resume 被
/// 逐次拒掉，30 秒后放弃，任务僵死 2 小时 27 分。
///
/// 断言链：判终态 → `executor_owner` 为 NULL → 去重登记也放了（探法 = 另一个执行体抢得到）
/// → 紧接着的 `try_run` **真的取得执行权**。
#[tokio::test]
async fn a_terminal_run_frees_its_ownership_even_when_the_future_never_returns() {
    let ctx = setup("true", Settings::default()).await;
    let llm: Arc<dyn LlmClient> = Arc::new(PendingAgent);
    let stuck = Arc::new(Executor::new(
        ctx.store.clone(),
        Settings::default(),
        Arc::new(ctx.sse.clone()),
        llm,
        Arc::new(ctx.killer.clone()),
    ));

    testkit::seed_task(&ctx.store, "t-stuck", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-stuck").await;
    let hanging = {
        let e = stuck.clone();
        tokio::spawn(async move { e.run("t-stuck").await })
    };
    // 不能抓「第一条 active run」（可能是 init 的短命 run，见该助手的警示）——
    // 钉在会挂起的 agent 节点上，拨时钟之后它才必然是陈旧的（决策 343）。
    let run_id = wait_for_running_validate_input(&ctx, "t-stuck").await;
    assert!(
        ctx.store
            .get_task("t-stuck")
            .await
            .unwrap()
            .executor_owner
            .is_some(),
        "前提：执行权已被那个卡住的执行体持有"
    );

    // 心跳在这段时间里一次都没刷新过 → 空闲超时判它终态（ManualClock 手动推进）。
    ctx.clock.advance_secs(400);
    let scheduler = KanbanScheduler::new(
        ctx.store.clone(),
        Settings::default(),
        Arc::new(ctx.clock.clone()),
        Arc::new(ctx.killer.clone()),
        Arc::new(ctx.sse.clone()),
        Arc::new(|_| {}),
    );
    let report = scheduler.tick().await.unwrap();
    assert!(
        report.timed_out_runs.contains(&run_id),
        "看门狗要判它终态：{:?}",
        report.timed_out_runs
    );

    // ① DB 那一半：乐观锁放开了（原来只有 `run_inner` 返回才清）。
    assert!(
        ctx.store
            .get_task("t-stuck")
            .await
            .unwrap()
            .executor_owner
            .is_none(),
        "判终态的同一处就要清 executor_owner，不等那个 future 回来"
    );

    // ② 去重那一半 + ③ 紧接着的 try_run 真的取得执行权：一个**新的**执行体（脚本为空，
    //    跑完即退）必须抢得到——旧的那个还停在永不返回的 future 上，一个字都没收。
    let fresh = Executor::new(
        ctx.store.clone(),
        Settings::default(),
        Arc::new(ctx.sse.clone()),
        Arc::new(FakeAgent::new(Script::new())),
        Arc::new(ctx.killer.clone()),
    );
    assert!(
        fresh.try_run("t-stuck").await.unwrap(),
        "判终态之后，紧接着的 try_run 应当取得执行权（去重与乐观锁两半都要放开）"
    );

    // 收尾：那个永不返回的 future 还在跑，没人能叫它回来，abort 掉免得留到尾。
    hanging.abort();
}

/// **主链集成验收（决策 302 / 305，票 08）**：卡住 → 自愈，**全程不重启**。
///
/// 这是 2026-09-27 那次故障的完整反面：03:30 卡住 → 03:49 续跑被挡满预算放弃 →
/// 此后 2 小时 27 分零调度活动 → 只能重启。
///
/// 断言链（一条用例串完，免得三格各自为政）：
/// 1. future 不返回、run 被判终态 → **执行权为 NULL**（票 02 那一层）；
/// 2. **反向断言**：此刻启动期那条「清残留持有者」的路**无事可清**——自愈不靠重启；
/// 3. 紧接着的 `try_run` 真的取得执行权并**落出新 run 行**（不是「取得执行权」这种半句话）；
/// 4. 第三类判据认得出「游标 `pending` + run 已终态 + 执行权仍持有」，且 `unstick` 解得开。
///
/// **诚实标注**：测试里没有可阻塞的受保护路径，验的是**可观察契约**而不是真 TCC
/// ——「future 不返回 + run 已判终态 → 执行权仍被清、下一次 `try_run` 仍能拿到」用
/// `PendingAgent` + `Clock` 推进造出来。真授权那一层不进默认门。
#[tokio::test]
async fn the_stuck_to_self_healed_chain_needs_no_restart() {
    use agentpipeline_core::pipeline::unstick::{stuck_evidence, unstick};

    let ctx = setup("true", Settings::default()).await;
    let llm: Arc<dyn LlmClient> = Arc::new(PendingAgent);
    let stuck = Arc::new(Executor::new(
        ctx.store.clone(),
        Settings::default(),
        Arc::new(ctx.sse.clone()),
        llm,
        Arc::new(ctx.killer.clone()),
    ));
    testkit::seed_task(&ctx.store, "t-chain", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-chain").await;
    let hanging = {
        let e = stuck.clone();
        tokio::spawn(async move { e.run("t-chain").await })
    };
    // 不能抓「第一条 active run」（可能是 init 的短命 run，见该助手的警示）——
    // 钉在会挂起的 agent 节点上，拨时钟之后它才必然是陈旧的（决策 343）。
    let run_id = wait_for_running_validate_input(&ctx, "t-chain").await;

    // 心跳一次都没刷过 → 看门狗判它终态。
    ctx.clock.advance_secs(400);
    let scheduler = KanbanScheduler::new(
        ctx.store.clone(),
        Settings::default(),
        Arc::new(ctx.clock.clone()),
        Arc::new(ctx.killer.clone()),
        Arc::new(ctx.sse.clone()),
        Arc::new(|_| {}),
    );
    let report = scheduler.tick().await.unwrap();
    assert!(
        report.timed_out_runs.contains(&run_id),
        "看门狗要判它终态：{:?}",
        report.timed_out_runs
    );

    // ① 执行权为 NULL。
    assert!(
        ctx.store
            .get_task("t-chain")
            .await
            .unwrap()
            .executor_owner
            .is_none(),
        "判终态的同一处就要清 executor_owner"
    );

    // ② **反向断言**：不靠重启——启动期那条路（清全表持有者）此刻无事可清。
    //    它的语义是「kill -9 残留」，而这里执行权是被**运行期**那一处放开的。
    assert_eq!(
        ctx.store.clear_executor_owners().await.unwrap(),
        0,
        "自愈不靠重启：没有残留持有者等着启动期去清"
    );

    // ③ **真的跑起来**：新执行体取得执行权并落出新 run 行。
    let before = ctx.store.list_runs("t-chain").await.unwrap().len();
    let fresh = Executor::new(
        ctx.store.clone(),
        Settings::default(),
        Arc::new(ctx.sse.clone()),
        Arc::new(FakeAgent::new(Script::new())),
        Arc::new(ctx.killer.clone()),
    );
    assert!(
        fresh.try_run("t-chain").await.unwrap(),
        "判终态之后，紧接着的 try_run 应当取得执行权"
    );
    assert!(
        ctx.store.list_runs("t-chain").await.unwrap().len() > before,
        "「取得执行权」要落成一条真的 run 行，不是半句话"
    );

    // ④ 第三类判据 + unstick 解得开（另一条任务上单独构造那一格，免得与上面那条互相污染）。
    testkit::seed_task(&ctx.store, "t-third", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-third").await;
    ctx.store
        .set_task_status("t-third", TaskStatus::Running)
        .await
        .unwrap();
    assert!(ctx
        .store
        .try_claim_executor("t-third", "executor:dead")
        .await
        .unwrap());
    let cursor3 = ctx.store.load_live_cursors("t-third").await.unwrap()[0].clone();
    let dead = ctx
        .store
        .insert_run(&agentpipeline_core::storage::observability::NewRun {
            task_id: "t-third".into(),
            cursor_id: cursor3.cursor_id.clone(),
            stage: cursor3.stage,
            node: cursor3.node,
            attempt: 3,
            agent_type: "main".into(),
            parent_run_id: None,
            prompt_template_hash: None,
            process_group_id: None,
        })
        .await
        .unwrap();
    ctx.store
        .finish_run(
            dead,
            &agentpipeline_core::storage::observability::RunOutcome {
                status: Some(NodeStatus::Timeout),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let finished =
        agentpipeline_core::clock::Clock::now(&ctx.clock) - chrono::Duration::minutes(30);
    sqlx::query("UPDATE kanban_node_runs SET finished_at = ? WHERE id = ?")
        .bind(agentpipeline_core::storage::ts(finished))
        .bind(dead)
        .execute(ctx.store.pool())
        .await
        .unwrap();
    ctx.store
        .set_cursor_pending(
            &cursor3.cursor_id,
            &agentpipeline_core::types::PendingReason::new(
                PendingKind::Timeout,
                cursor3.stage,
                cursor3.node,
                "执行超时（attempt 3）",
            ),
        )
        .await
        .unwrap();
    ctx.store.sync_task_projection("t-third").await.unwrap();

    let task3 = ctx.store.get_task("t-third").await.unwrap();
    let evidence = stuck_evidence(
        &ctx.store,
        &task3,
        agentpipeline_core::clock::Clock::now(&ctx.clock),
        chrono::Duration::minutes(10),
    )
    .await
    .unwrap()
    .expect("第三类判据必须认得出这一格（游标 pending + run 已终态 + 执行权仍持有）");
    assert_eq!(evidence.detail["shape"], "terminal_run");

    let unstuck = unstick(
        &ctx.store,
        &agentpipeline_core::pipeline::executor::force_release,
        "t-third",
        agentpipeline_core::clock::Clock::now(&ctx.clock),
        chrono::Duration::minutes(10),
    )
    .await
    .expect("unstick 要解得开它");
    assert_eq!(
        unstuck.kind,
        agentpipeline_core::storage::AttentionKind::OwnerStuck
    );
    assert!(
        ctx.store
            .get_task("t-third")
            .await
            .unwrap()
            .executor_owner
            .is_none(),
        "解得开 = 执行权被清掉"
    );
    hanging.abort();
}

/// **反向断言（票 02）**：健康在跑的任务（owner 持有、心跳新鲜、未超阈值）**不被这条链误伤**。
///
/// 这正是 `unstick` 文件头写的那句警告——把「有健康执行体在跑」判成卡住会**清掉健康占用**，
/// 于是同一任务被两个执行体同时写库。所以 `release_ownership` 只挂在判终态那一处，
/// 而不是挂在「有主」或「跑得久」上。
#[tokio::test]
async fn a_healthy_running_executor_keeps_its_ownership_across_a_tick() {
    let ctx = setup("true", Settings::default()).await;
    let llm: Arc<dyn LlmClient> = Arc::new(PendingAgent);
    let running = Arc::new(Executor::new(
        ctx.store.clone(),
        Settings::default(),
        Arc::new(ctx.sse.clone()),
        llm,
        Arc::new(ctx.killer.clone()),
    ));

    testkit::seed_task(&ctx.store, "t-ok", "p1").await.unwrap();
    admit(&ctx, "t-ok").await;
    let inflight = {
        let e = running.clone();
        tokio::spawn(async move { e.run("t-ok").await })
    };
    wait_for_a_held_running_run(&ctx, "t-ok").await;

    // 一次 tick：心跳是新鲜的（时钟没推），它不该被扫成超时。
    let scheduler = KanbanScheduler::new(
        ctx.store.clone(),
        Settings::default(),
        Arc::new(ctx.clock.clone()),
        Arc::new(ctx.killer.clone()),
        Arc::new(ctx.sse.clone()),
        Arc::new(|_| {}),
    );
    let report = scheduler.tick().await.unwrap();
    assert!(report.timed_out_runs.is_empty(), "健康在跑的 run 不判超时");

    let owner = ctx.store.get_task("t-ok").await.unwrap().executor_owner;
    assert!(
        owner.is_some(),
        "健康在跑的任务：执行权一个字都不许动（owner = {owner:?}）"
    );
    // 去重那一半同样没动：另一个执行体抢不到。
    let fresh = Executor::new(
        ctx.store.clone(),
        Settings::default(),
        Arc::new(ctx.sse.clone()),
        Arc::new(FakeAgent::new(Script::new())),
        Arc::new(ctx.killer.clone()),
    );
    assert!(
        !fresh.try_run("t-ok").await.unwrap(),
        "健康在跑时 try_run 必须被进程内去重拒掉"
    );

    inflight.abort();
}

/// **票 02 的「与启动恢复不重复也不漏」**：判终态那一处只放开执行权、不替恢复序列干活；
/// 而恢复序列够不着「启动之后才出现的持有者」——两条路各管一格，谁也替代不了谁。
///
/// 判据的**对象**不同才是它们不重复的根据：恢复序列（决策 127 / 212）是启动期的一次
/// **全表扫**，对象是「上一个进程留下的任何持有者」；这一处是运行期针对**某一个刚被判终态的
/// 任务**。全表扫只在启动那一刻跑一次，所以运行期新出现的持有者它**永远看不到**——那正是
/// 2026-09-27 那次的形状（持有者产生于运行期，重启才被清掉）。
#[tokio::test]
async fn releasing_at_run_terminal_and_the_startup_recovery_cover_different_ground() {
    let ctx = setup("true", Settings::default()).await;
    testkit::seed_task(&ctx.store, "t-x", "p1").await.unwrap();

    // ① 启动那一刻：全表扫一次，此时无人持有。
    let at_boot = agentpipeline_core::pipeline::foreman_actions::run_recovery_sequence(&ctx.store)
        .await
        .unwrap();
    assert_eq!(at_boot.cleared, 0, "启动时没有任何持有者");

    // ② 启动**之后**才出现的持有者：恢复序列已经跑过了，它看不到这一格。
    ctx.store
        .set_task_status("t-x", TaskStatus::Running)
        .await
        .unwrap();
    assert!(
        ctx.store
            .try_claim_executor("t-x", "owner-x")
            .await
            .unwrap(),
        "前提：乐观锁拿得到（此刻无人持有）"
    );

    // 判终态那一处放开它（本用例不起执行体，只验 DB 那一半）。
    let had = agentpipeline_core::pipeline::executor::release_ownership(&ctx.store, "t-x")
        .await
        .unwrap();
    assert!(!had, "进程内没有这一号登记：本用例只验 DB 那一半");
    assert!(
        ctx.store
            .get_task("t-x")
            .await
            .unwrap()
            .executor_owner
            .is_none(),
        "运行期这一处要清得掉恢复序列够不着的那个持有者"
    );
    assert_eq!(
        ctx.store.get_task("t-x").await.unwrap().status,
        TaskStatus::Running,
        "它不越权替恢复序列归队：状态一个字不动"
    );

    // ③ 再跑一次恢复序列：不重复清，但归队仍是它的活（运行期那一处没做、也不该做）。
    let after = agentpipeline_core::pipeline::foreman_actions::run_recovery_sequence(&ctx.store)
        .await
        .unwrap();
    assert_eq!(after.cleared, 0, "持有者已被运行期那一处清掉，不重复清");
    assert!(
        after.requeued.contains(&"t-x".to_string()),
        "归队归恢复序列管（运行期那一处只放执行权）：{:?}",
        after.requeued
    );
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
    tokio::time::timeout(Duration::from_secs(30), async {
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
    tokio::time::timeout(Duration::from_secs(30), jh)
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
    {
        // 先一次成功的调用（FakeAgent 每步报 10 prompt / 5 completion），再让调用当场失败。
        //
        // 失败要**连着三次**：传输类失败先在**轮内**就地重发（决策 373，上限
        // `LLM_TRANSPORT_RESEND_MAX`），预算耗尽这一轮才真的失败。只给一次的话，
        // 重发会去取脚本的下一步（耗尽 → 收尾纯文本），这一轮反倒成功了——
        // 那测的就不是「失败轮怎么记账」了。
        let mut b = script
            .for_node(Stage::ArchitectDesign, Node::ValidateInput)
            .list_dir(".");
        for _ in 0..=LLM_TRANSPORT_RESEND_MAX {
            b = b.fail_llm("llm_network", "模型服务不可达", "connect timed out");
        }
    }
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

// ─────────────────────── 上下文超窗：不再盲目重试（决策 295 / 票 10）───────────────────────

/// 超窗**只发生一次调用**（票 10）：不再按 `agent_retry_max` 把同一份放不下的转录重发一遍。
///
/// 从前这条路径一路重试到耗尽——同一份转录发 N 遍不会变小，烧的是 N 倍的时间与钱；
/// 而唯一该做的事（换更大窗口的模型 / 调大配置）在那之前一个字都没写出来。
#[tokio::test]
async fn a_context_window_failure_is_not_retried_to_exhaustion() {
    // 重试预算给足 5：从前会打 5 次，用例据此判「不再盲目重试」
    let settings = Settings {
        agent_retry_max: 5,
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
        // 第一次调用就撞窗：转录里还没有可压的旧轮（本轮刚开始），故压缩救不回来。
        .fail_llm(
            "llm_context_window",
            LlmErrorKind::ContextWindow.advice(),
            "HTTP 400：This model's maximum context length is 8192 tokens",
        )
        // 重试才会走到这一步；用例靠「它没被消费」验证「没有第二次调用」。
        .text("（这一条只在重试时才会被消费）");
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t-ctxwin", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-ctxwin").await;
    ctx.executor.run("t-ctxwin").await.unwrap();

    assert_eq!(
        ctx.agent.calls_for(Stage::ArchitectDesign, Node::Execute),
        1,
        "超窗只打一次调用：压不动就报错，不重试到 agent_retry_max"
    );
    let cursor = ctx
        .store
        .load_live_cursors("t-ctxwin")
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("游标还在（挂着等处置）");
    let reason = cursor.pending_reason.as_ref().expect("应当挂了 pending");
    assert_eq!(reason.kind, PendingKind::RetryExhausted);
    // 台账里读得出**下一步做什么**（票面要求）：类别是可归因的那一格，message 是它的人话。
    assert!(
        reason.message.contains("上下文窗口") && reason.message.contains("模型"),
        "超窗那一条要带可操作指引：{}",
        reason.message
    );
    assert!(
        reason.context.as_ref().is_some_and(|c| c
            .diagnostic
            .as_deref()
            .is_some_and(|d| d.contains("maximum context length"))),
        "原始诊断照旧进 context（可搜）：{:?}",
        reason.context
    );
}

/// 压得动就**压一次再试这一次调用**（票 10，与值班长 06(c) 同一处置）：一次真实可救的超窗
/// （算术低估）因此不必让整个节点失败——而它**不是**整轮重试：run 行仍只有一条。
#[tokio::test]
async fn a_context_window_failure_compacts_and_retries_that_one_call() {
    let ctx = setup("true", Settings::default()).await;
    let mut script = Script::new();
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    let mut exec = script.for_node(Stage::ArchitectDesign, Node::Execute);
    // 先攒够**可压的**轮次（`keep_recent_rounds` 缺省 5，每轮两条消息 = assistant + tool_result；
    // 压缩要 `len > keep + 2` 才动得了）：5 轮工具往返之后，前面的轮次才成了「旧轮」。
    for i in 0..5 {
        // 决策 395：architect 的写入面只有 design.md——往返轮次重复写它（内容带序号）
        exec = exec.write_file("design.md", &format!("# 设计 v{i}"));
    }
    exec.fail_llm(
        "llm_context_window",
        LlmErrorKind::ContextWindow.advice(),
        "HTTP 400：This model's maximum context length is 8192 tokens",
    )
    // 压缩之后重试的**那一次调用**走到这里：这一轮照常收口。
    .submit(&ArchitectExecuteMetadata {
        readiness: true,
        ..Default::default()
    });
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t-ctxwin-ok", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-ctxwin-ok").await;
    ctx.executor.run("t-ctxwin-ok").await.unwrap();

    assert_eq!(
        ctx.agent.calls_for(Stage::ArchitectDesign, Node::Execute),
        8,
        "5 次工具往返 + 1 次撞窗 + 1 次重试 + 1 次收口（提交之后模型还会被叫一次说收尾的话）\
         ——压一次再试这一次调用，不多不少"
    );
    let runs = ctx
        .store
        .list_runs_at("t-ctxwin-ok", Stage::ArchitectDesign, Node::Execute)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1, "重试发生在**这一次调用**上，不是整轮重跑");
    assert_eq!(
        runs[0].status,
        NodeStatus::Success,
        "压缩之后那一次调用收了口：这一轮不该因为算术低估而整段失败"
    );
}

/// 票 03 验收（long-run-budget 票 02 改 token 口径）：只跑工具往返的长节点（ux-audit-3
/// 的形状——纯只读走查在 develop 节点烧了 90 分钟、单 run prompt_tokens 1490 万）必须被
/// token 硬底拦住，**即使在无 provider 的机器上**（FakeAgent 路径，capacity=None：不看
/// 窗口登记的脸色）。
///
/// 60 轮 × 8 千字符的写文件往返 ≈ 转录 12 万 token（ASCII 4 字符 ≈ 1 token），两度撞上
/// 4 万 token 的硬底（注入的小值；生产缺省 30 万）。断言：节点照常收口；压缩真实发生
/// （转录里出现 `[摘要]`）；每个请求都压在「硬底 + 可解释余量」内；请求总量压在
/// 「无压缩理想值」（Σ 每轮累积重发，O(n²)）之下。
#[tokio::test]
async fn a_long_tool_round_trip_node_is_capped_by_the_token_floor() {
    use agentpipeline_core::agent::context::transcript_chars;

    let ctx = setup(
        "true",
        Settings {
            conversation_max_tokens: 40_000,
            ..Default::default()
        },
    )
    .await;
    let mut script = Script::new();
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    let mut exec = script.for_node(Stage::ArchitectDesign, Node::Execute);
    for _i in 0..60 {
        let body = "x".repeat(8_000);
        // 决策 395：architect 的写入面只有 design.md——往返轮次重复写它
        exec = exec.write_file("design.md", &body);
    }
    exec.submit(&ArchitectExecuteMetadata {
        readiness: true,
        ..Default::default()
    });
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t-charfloor", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-charfloor").await;
    ctx.executor.run("t-charfloor").await.unwrap();

    let runs = ctx
        .store
        .list_runs_at("t-charfloor", Stage::ArchitectDesign, Node::Execute)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(
        runs[0].status,
        NodeStatus::Success,
        "硬底拦的是体量，不是产出：节点照常收口"
    );

    let requests: Vec<_> = ctx
        .agent
        .request_log()
        .into_iter()
        .filter(|r| r.stage == Stage::ArchitectDesign && r.node == Node::Execute)
        .collect();
    assert_eq!(
        requests.len(),
        62,
        "60 次工具往返 + 1 次提交 + 1 次收尾，一次不少"
    );

    // 压缩真实发生：硬底触发后，后续请求的转录里出现了 `[摘要]`
    assert!(
        requests.iter().any(|r| r.messages.iter().any(|m| m
            .content
            .as_deref()
            .unwrap_or("")
            .starts_with("[摘要]"))),
        "硬底触发后转录里应出现压缩摘要"
    );
    // 锚点/摘要不空转：任一请求里 [摘要] 至多一条（反复压缩只会重写它，不会翻倍）
    for r in &requests {
        let summaries = r
            .messages
            .iter()
            .filter(|m| m.content.as_deref().unwrap_or("").starts_with("[摘要]"))
            .count();
        assert!(summaries <= 1, "摘要至多一条，得到 {summaries}");
    }

    // 票 106-stability/09：每个请求的转录必须 **wire 合法**——tool 消息的
    // tool_call_id 必须能在前置 assistant 的 tool_calls 里找到，assistant 声明的
    // call 必须在下一个非 tool 消息之前收到回执。违反即 OpenAI 兼容上游整请求
    // 400（2026-10-04 事故：压缩切点落在 tool 结果上→孤儿 tool 消息→毒转录
    // 落库重载→该节点此后每个请求恒 400、重试耗尽）。FakeAgent 不做这个校验，
    // 这里替它做——本用例的 60 轮工具往返正是当时的负载形状。
    for r in &requests {
        let mut pending: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for m in &r.messages {
            match m.role {
                agentpipeline_core::agent::client::Role::Assistant => {
                    pending = m.tool_calls.iter().map(|c| c.id.as_str()).collect();
                }
                agentpipeline_core::agent::client::Role::Tool => {
                    assert!(
                        m.tool_call_id
                            .as_deref()
                            .is_some_and(|id| pending.contains(id)),
                        "孤儿 tool 消息进了请求（tool_call_id={:?}）：{}.{}/attempt {}",
                        m.tool_call_id,
                        r.stage,
                        r.node,
                        r.attempt
                    );
                    if let Some(id) = m.tool_call_id.as_deref() {
                        pending.remove(id);
                    }
                }
                _ => assert!(
                    pending.is_empty(),
                    "assistant 的 tool_call 没等到回执就被打断：{}.{}/attempt {}",
                    r.stage,
                    r.node,
                    r.attempt
                ),
            }
        }
    }

    // 体量上界：每个请求压在「硬底 + 可解释余量」内（token 口径，票 02）——一轮的
    // 参数 ~8 千字符 ≈ 2 千 token，余量放宽到 6 轮以容纳 keep 窗口与摘要的形状差。
    // 字符读数只作对照（4:1 折算），不再是触发口径。
    let per_round = 8_200usize;
    let per_round_tokens = per_round / 4; // ASCII 4 字符 ≈ 1 token（count_tokens 的老规则）
    let floor_tokens = 40_000usize;
    for r in &requests {
        let tokens = estimate_messages_tokens(&r.system_prompt, &r.user_prompt, &r.messages);
        assert!(
            tokens <= floor_tokens + per_round_tokens * 6,
            "请求转录 {tokens} token，超过硬底的可解释余量"
        );
    }
    // 总量：无压缩理想值 = 每轮都整卷重发（O(n²)）；压缩后必须明显低于它。
    let ideal: usize = (1..=60).map(|i| i * per_round).sum();
    let total: usize = requests.iter().map(|r| transcript_chars(&r.messages)).sum();
    assert!(
        total * 2 < ideal,
        "压缩后的请求总量 {total} 应压在无压缩理想值 {ideal} 的一半之下"
    );

    // 票面验收的**token 口径**（FakeAgent 的 prompt_tokens 是固定假数，取同一请求快照
    // 按 `estimate_messages_tokens` 的生产算术折算）：静态两段 + 消息，与理想值同形对比。
    use agentpipeline_core::agent::context::estimate_messages_tokens;
    let statics =
        estimate_messages_tokens(&requests[0].system_prompt, &requests[0].user_prompt, &[]);
    let ideal_tokens: usize = (1..=60).map(|i| statics + i * per_round_tokens).sum();
    let total_tokens: usize = requests
        .iter()
        .map(|r| estimate_messages_tokens(&r.system_prompt, &r.user_prompt, &r.messages))
        .sum();
    assert!(
        total_tokens * 2 < ideal_tokens,
        "压缩后的 prompt 估算总量 {total_tokens} 应压在无压缩理想值 {ideal_tokens} 的一半之下"
    );
}

// ─────────── 失败重试按错误类别分流（决策 298，收窄决策 278 的适用边界）───────────

/// 造一个「provider 行写着 128,000」的现场——**事故当时那一行的值**（决策 309 的实测底稿：
/// 561,210 的输入照样 `ok`，而那一行写着 128,000）。
///
/// 后面两条用例共用它：一条钉「撞墙按证据重登记（双向，决策 378）」，一条钉「别的错误动不了它」。
async fn setup_with_misconfigured_window() -> Ctx {
    use agentpipeline_core::types::{Provider, StageConfig};

    let ctx = setup("true", Settings::default()).await;
    ctx.store
        .upsert_provider(&Provider {
            id: "prov-ctx".into(),
            vendor: "deepseek".into(),
            model: "deepseek-chat".into(),
            context_window: 128_000,
            base_url: None,
            api_key: None,
            enabled: true,
            created_at: ctx.store.now(),
            updated_at: ctx.store.now(),
        })
        .await
        .unwrap();
    // 显式把这一阶段指到那一行：校准「改哪一行」必须与算出触发线的那一行同一行
    // （`resolve_provider_id` 四级里的第三级）。
    ctx.store
        .upsert_stage_config(&StageConfig {
            stage: "architect-design".into(),
            provider_id: Some("prov-ctx".into()),
            updated_at: ctx.store.now(),
            ..Default::default()
        })
        .await
        .unwrap();
    ctx
}

/// 撞墙自校准（决策 309；票 03 修订为**双向**，决策 378）：上下文超长那一类会把
/// provider 行的窗口**按撞墙那次请求的规模重登记**——那次报错就是「这一份放不下」的
/// 书面证据，登记值该贴着它走。这里的撞墙发生在 ValidateInput 的第一次调用上，转录
/// 还很小（observed = 静态两段的估算）→ 登记值从 128,000 **下调**到那个小读数：正是
/// ux-audit-3 的病（登记虚高 → 软限虚高 → 压缩永不触发）的反向演练。上调方向由
/// `window_calibration` 的单测钉住（`the_wall_calibration_follows_the_evidence_in_both_directions`）。
#[tokio::test]
async fn a_context_window_failure_moves_the_provider_row_to_the_observed_size() {
    let ctx = setup_with_misconfigured_window().await;
    let mut script = Script::new();
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .fail_llm(
            "llm_context_window",
            LlmErrorKind::ContextWindow.advice(),
            "HTTP 400：This model's maximum context length is 8192 tokens",
        );
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t-wall", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-wall").await;
    ctx.executor.run("t-wall").await.unwrap();

    let row = ctx
        .store
        .get_provider("prov-ctx")
        .await
        .unwrap()
        .expect("provider 行还在");
    assert!(
        row.context_window < 128_000,
        "撞墙的请求很小（转录还是空的）→ 登记值按证据**下调**，不再守着虚高的配置值：{}",
        row.context_window
    );
    assert!(
        row.context_window > 0,
        "下调到 observed 的真实读数，不是清零"
    );
}

/// 反向断言（同票）：传输类**一个字节都不改** provider 行——判据就是 `is_context_window`，
/// 那几类到不了校准那个分支。
#[tokio::test]
async fn a_transport_failure_leaves_the_provider_row_untouched() {
    let ctx = setup_with_misconfigured_window().await;
    let mut script = Script::new();
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .fail_llm(
            "llm_network",
            LlmErrorKind::Network.advice(),
            "HTTP 请求失败：connect: connection refused",
        );
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t-nowall", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-nowall").await;
    ctx.executor.run("t-nowall").await.unwrap();

    let row = ctx
        .store
        .get_provider("prov-ctx")
        .await
        .unwrap()
        .expect("provider 行还在");
    assert_eq!(
        row.context_window, 128_000,
        "连不上与窗口值无关：这一行必须原样（改它就等于把「机器忙」记成「窗口更大」）"
    );
}

/// 传输类失败（连不上）：**重试，但不追加错误 turn**——请求根本没送到模型，转录末尾是
/// 上一次成功的完好回合，`retry_prompt` 那句「上一轮的输出未按输出契约提交、已判废」
/// 对它是假话，括号里那句（「请检查 base_url…」）还是写给人看的运维指引。
///
/// 同一用例里的对照组：第 2 轮由**输出契约类**失败（元数据抽不出）触发，那条照旧
/// 「转录 + 错误 turn」——分流只收窄传输类，没动决策 278 的本体。
#[tokio::test]
async fn a_transport_failure_retries_without_an_error_turn() {
    let settings = Settings {
        agent_retry_max: 3,
        ..Default::default()
    };
    let ctx = setup("true", settings).await;
    let mut script = Script::new();
    {
        // 第 1 轮先走一次真实工具往返（失败时转录非空，「不追加」的断言才有牙齿），再连不上。
        //
        // 连不上要**连着三次**：轮内的就地重发（决策 373）会先把预算花掉，之后这一轮
        // 才带着传输类的死因退场——这正是本用例要观察的那条路。
        let mut b = script
            .for_node(Stage::ArchitectDesign, Node::ValidateInput)
            .list_dir(".");
        for _ in 0..=LLM_TRANSPORT_RESEND_MAX {
            b = b.fail_llm(
                "llm_network",
                LlmErrorKind::Network.advice(),
                "HTTP 请求失败：connect: connection refused",
            );
        }
    }
    // 第 2/3 轮走「脚本耗尽」→ 收尾纯文本 → 元数据抽不出 → 输出契约类失败（对照组）
    ctx.agent.set_script(script);

    testkit::seed_task(&ctx.store, "t-transport", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-transport").await;
    ctx.executor.run("t-transport").await.unwrap();

    let runs = ctx
        .store
        .list_runs_at("t-transport", Stage::ArchitectDesign, Node::ValidateInput)
        .await
        .unwrap();
    assert_eq!(
        runs.len(),
        3,
        "传输类照旧按 agent_retry_max 重试（它是「等一等会好」的那一类）：{} 行 run",
        runs.len()
    );

    // 按 run 分组取每轮的**首条请求**（一次 attempt 内的后续请求是工具往来，不算起点）
    let requests = ctx.agent.request_log();
    let mut firsts: Vec<&LlmRequest> = Vec::new();
    for r in requests
        .iter()
        .filter(|r| r.stage == Stage::ArchitectDesign && r.node == Node::ValidateInput)
        .filter(|r| {
            r.run
                .as_ref()
                .is_some_and(|c| c.agent_type.as_str() == "main")
        })
    {
        let run_id = r.run.as_ref().expect("主 agent 请求须带 run 上下文").run_id;
        if firsts.last().map(|p| p.run.as_ref().unwrap().run_id) != Some(run_id) {
            firsts.push(r);
        }
    }
    assert_eq!(firsts.len(), 3, "三轮各一条起点：{}", firsts.len());
    assert_eq!(firsts[0].messages.len(), 0, "首轮起点为空");

    // 决策 298：第 1 轮是传输类失败 → 第 2 轮从**那份转录原样**接着跑，一条不多
    assert_eq!(
        firsts[1].messages.len(),
        2,
        "传输类重试的起点是失败时的转录（assistant + tool_result），没有错误 turn"
    );
    assert!(
        firsts[1]
            .messages
            .iter()
            .all(|m| !m.content.as_deref().unwrap_or("").contains("已判废")),
        "传输类不许把「已判废」这句假话灌给模型：{:?}",
        firsts[1].messages
    );

    // 对照组：第 2 轮是输出契约类失败 → 第 3 轮照旧 +2（assistant 回复 + 错误 turn）
    assert_eq!(
        firsts[2].messages.len(),
        firsts[1].messages.len() + 2,
        "输出契约类失败的重试轮仍带错误 turn（决策 278 本体不动）"
    );
    let last = firsts[2].messages.last().expect("重试请求不应为空");
    assert!(
        last.content.as_deref().unwrap_or("").contains("已判废"),
        "错误 turn 照旧给输出契约类失败：{:?}",
        last
    );
}

/// 配置类失败（鉴权）：**一次都不重试**——等一等没用，重试烧的是同一份坏密钥的 N 倍
/// token，而改 api_key 这件事一个字都不会在重试里发生（与超窗同一处置，决策 295 / 298）。
/// 台账里读得出下一步做什么（分类自带的人话），原始诊断照旧进 context（可搜）。
#[tokio::test]
async fn a_config_failure_fails_fast_without_burning_retries() {
    // 重试预算给足 5：从前会打 5 次，用例据此判「不再盲目重试」
    let settings = Settings {
        agent_retry_max: 5,
        ..Default::default()
    };
    let ctx = setup("true", settings).await;
    let mut script = Script::new();
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .fail_llm(
            "llm_auth",
            LlmErrorKind::Auth.advice(),
            "HTTP 401：{\"error\":{\"message\":\"Incorrect API key provided\"}}",
        )
        // 只有真的重试才会走到这一步；用例靠「它没被消费」验证「没有第二次调用」
        .text("（这一条只在重试时才会被消费）");
    ctx.agent.set_script(script);

    testkit::seed_task(&ctx.store, "t-auth", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-auth").await;
    ctx.executor.run("t-auth").await.unwrap();

    assert_eq!(
        ctx.agent
            .calls_for(Stage::ArchitectDesign, Node::ValidateInput),
        1,
        "鉴权失败只打一次调用：等一等没用，不重试到 agent_retry_max"
    );
    let runs = ctx
        .store
        .list_runs_at("t-auth", Stage::ArchitectDesign, Node::ValidateInput)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1, "台账里只该有这一条失败 run");

    let cursor = ctx
        .store
        .load_live_cursors("t-auth")
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("游标还在（挂着等处置）");
    let reason = cursor.pending_reason.as_ref().expect("应当挂了 pending");
    assert_eq!(reason.kind, PendingKind::RetryExhausted);
    assert!(
        reason.message.contains("api_key"),
        "台账里读得出下一步做什么（分类自带的人话，不写「重试耗尽」）：{}",
        reason.message
    );
    assert!(
        reason.context.as_ref().is_some_and(|c| c
            .diagnostic
            .as_deref()
            .is_some_and(|d| d.contains("Incorrect API key"))),
        "原始诊断照旧进 context（可搜）：{:?}",
        reason.context
    );
}

/// 组装层的**配置类**失败（票 05 那条反向断言的落点）：节点**转 pending**，
/// 且**不产生超时记账**。
///
/// 缺完全磁盘访问那条快速失败就是这个类：`assemble` 在任何模型调用之前返回
/// `Error::Config`（判词与「改了设置也要重启」那一句由
/// `model_request::tests::a_denied_disk_access_snapshot_fails_fast_with_the_next_step` 钉住）。
/// 它到不了模型调用，故「一次调用都没发」与「一条 `timeout` 的 run 行都没有」说的是同一件事
/// ——这正是票面那条反向断言要的形态。
///
/// **为什么不用真缺授权在这里端到端跑一遍**：那条路还要项目根落在真实 `$HOME/Documents`
/// 下，而用例的仓库建在临时目录里（家目录隔离是决策 143 的另一半）；为了造这个现场去写
/// 开发机真实的 `~/Documents` 是拿环境换覆盖。故这里用同一类错误的**另一个来源**——
/// provider 行没登记 `context_window`（决策 110），它在组装期同步返回同一个 `Error::Config`。
#[tokio::test]
async fn an_assembly_config_failure_pends_without_any_timeout_accounting() {
    use agentpipeline_core::types::{Provider, StageConfig};

    let ctx = setup("true", Settings::default()).await;
    // 未登记窗口（0）⇒ 组装期 `Error::Config`（决策 110：显式失败，不静默取默认）
    ctx.store
        .upsert_provider(&Provider {
            id: "prov-unset".into(),
            vendor: "deepseek".into(),
            model: "deepseek-chat".into(),
            context_window: 0,
            base_url: None,
            api_key: None,
            enabled: true,
            created_at: ctx.store.now(),
            updated_at: ctx.store.now(),
        })
        .await
        .unwrap();
    ctx.store
        .upsert_stage_config(&StageConfig {
            stage: "architect-design".into(),
            provider_id: Some("prov-unset".into()),
            updated_at: ctx.store.now(),
            ..Default::default()
        })
        .await
        .unwrap();
    ctx.agent.set_script(Script::new());

    testkit::seed_task(&ctx.store, "t-cfg", "p1").await.unwrap();
    admit(&ctx, "t-cfg").await;
    ctx.executor.run("t-cfg").await.unwrap();

    assert_eq!(
        ctx.agent
            .calls_for(Stage::ArchitectDesign, Node::ValidateInput),
        0,
        "组装期的失败在模型调用之前：一次调用都不该发出去"
    );
    let runs = ctx
        .store
        .list_runs_at("t-cfg", Stage::ArchitectDesign, Node::ValidateInput)
        .await
        .unwrap();
    assert!(
        runs.iter().all(|r| r.status != NodeStatus::Timeout),
        "不产生超时记账：{:?}",
        runs.iter().map(|r| r.status).collect::<Vec<_>>()
    );
    let cursor = ctx
        .store
        .load_live_cursors("t-cfg")
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("游标还在（挂着等处置）");
    let reason = cursor.pending_reason.as_ref().expect("应当挂了 pending");
    assert_eq!(reason.kind, PendingKind::RetryExhausted);
    assert!(
        reason.message.contains("context_window"),
        "台账里读得出下一步做什么：{}",
        reason.message
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

// ─────────────────── 工具参数被截断（票 01②）───────────────────

/// 上游把 `submit_metadata` 的参数腰斩 → 诊断必须说「被截断」，不许说「缺字段」；
/// 回灌给模型的错误 turn 也要换成「精简正文」的说法。
///
/// 为什么这条值得一个端到端用例：那句误导性的 `missing field` 会把模型引向
/// 「换个字段名再发一次」，而它真正该做的是把正文缩短——2026-09-30 那条链上
/// `validate_output` 连挂三次就是这么来的（`.scratch/silent-degradation/spec.md` 缺陷 1）。
#[tokio::test]
async fn truncated_submit_metadata_arguments_are_diagnosed_as_truncation() {
    let settings = Settings {
        agent_retry_max: 2,
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
    // 现场实测残片（conv 99）：救得回来但**丢掉了必填的 `readiness`**
    script
        .for_node(Stage::ArchitectDesign, Node::Execute)
        .submit_metadata_raw(r#"{"blockers": [], "feedback": "#);
    ctx.agent.set_script(script);

    testkit::seed_task(&ctx.store, "trunc", "p1").await.unwrap();
    admit(&ctx, "trunc").await;
    ctx.executor.run("trunc").await.unwrap();

    let runs = ctx
        .store
        .list_runs_at("trunc", Stage::ArchitectDesign, Node::Execute)
        .await
        .unwrap();
    let first = runs.first().expect("至少一条 run");
    assert_eq!(first.status, NodeStatus::Failed);
    let err = first.error.as_deref().unwrap_or("");
    assert!(
        err.contains("工具参数被截断"),
        "截断型失败要说「被截断」：{err}"
    );
    assert!(
        !err.contains("missing field"),
        "截断的假象不许当成诊断（它会把模型引向改字段名）：{err}"
    );

    // 下一轮的第一条请求末尾是错误 turn：文案换成「精简正文」而不是「换个字段发」
    let requests = ctx.agent.request_log();
    let last_request = requests
        .iter()
        .rfind(|r| r.stage == Stage::ArchitectDesign && r.node == Node::Execute)
        .expect("重试也发过请求");
    let turn = last_request
        .messages
        .last()
        .and_then(|m| m.content.as_deref())
        .unwrap_or("");
    assert!(
        turn.contains("上游截断") && turn.contains("精简"),
        "错误 turn 要给出「精简正文」的可行指引：{turn}"
    );
}

/// 另一半：救援成功且**必填齐全** → 判成功（`{"readiness": true}` 够 architect.execute）。
/// 残片仍是残片，标记由工具层那行 warn 留下——这里钉的是「别把可救的当成失败」。
#[tokio::test]
async fn a_rescued_truncation_with_all_required_fields_still_succeeds() {
    let ctx = setup("true", Settings::default()).await;
    let mut script = Script::new();
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    // conv 88 形态：`readiness` 是 architect.execute 唯一的必填，救回它就够了
    script
        .for_node(Stage::ArchitectDesign, Node::Execute)
        .submit_metadata_raw(r#"{"readiness": true, "test_scenarios_path": "#);
    ctx.agent.set_script(script);

    testkit::seed_task(&ctx.store, "rescued", "p1")
        .await
        .unwrap();
    admit(&ctx, "rescued").await;
    ctx.executor.run("rescued").await.unwrap();

    let runs = ctx
        .store
        .list_runs_at("rescued", Stage::ArchitectDesign, Node::Execute)
        .await
        .unwrap();
    assert_eq!(
        runs.first().map(|r| r.status),
        Some(NodeStatus::Success),
        "必填齐全的救援结果算成功：{:?}",
        runs.first().and_then(|r| r.error.clone())
    );
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
    {
        // 连着三次：传输类失败先在轮内就地重发（决策 373），预算耗尽这一轮才真的失败，
        // 那条「为什么没跑起来」的会话行才落得下来。
        let mut b = script.for_node(Stage::ArchitectDesign, Node::Execute);
        for _ in 0..=LLM_TRANSPORT_RESEND_MAX {
            b = b.fail_llm(
                "llm_network",
                "LLM 服务不可达，请检查网络或 base_url",
                "connect: connection refused",
            );
        }
    }
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

// ─────────────────── 思考留痕落库（决策 360 / 迁移 0038）───────────────────

#[tokio::test]
async fn reasoning_from_every_call_is_kept_beside_the_conversation() {
    // 决策 244 只给了流水线思考一条去处（实时增量）——run 落地或刷新的那一刻整段
    // 消失，现场时间线始终没有「折叠的思考过程」可摆（用户 2026-10-01 实机报障）。
    // 决策 360 补上第二条去处：随会话行落库。多次调用的思考按到达序以空行相连，
    // 且**绝不进转录**（不回灌是决策 244 的红线）。
    let ctx = setup("true", Settings::default()).await;
    let mut script = Script::new();
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .thinking("先读输入，信息是齐的")
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    script
        .for_node(Stage::ArchitectDesign, Node::Execute)
        .thinking("先想 design.md 的结构")
        .write_file("design.md", "# 设计\n## 验收标准\n- AC-1 能登录\n")
        .thinking("再想元数据怎么交")
        .submit(&ArchitectExecuteMetadata {
            readiness: true,
            ..Default::default()
        });
    ctx.agent.set_script(script);

    testkit::seed_task(&ctx.store, "t-think", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-think").await;
    ctx.executor.run("t-think").await.unwrap();

    let run = ctx
        .store
        .list_runs_at("t-think", Stage::ArchitectDesign, Node::Execute)
        .await
        .unwrap()
        .remove(0);
    let conv = ctx
        .store
        .get_conversation("t-think", run.id)
        .await
        .unwrap()
        .expect("execute 节点应有会话行");
    let reasoning = conv.reasoning.expect("思考要随会话行落地（决策 360）");
    assert_eq!(
        reasoning, "先想 design.md 的结构\n\n再想元数据怎么交",
        "两次调用的思考按到达序以空行相连"
    );
    assert!(
        !conv.messages_json.to_string().contains("先想 design.md"),
        "思考绝不进转录：messages_json 是下一轮的上下文（决策 244 红线）"
    );
    // 另一个节点的会话行各带各的（不串台）
    let validate_run = ctx
        .store
        .list_runs_at("t-think", Stage::ArchitectDesign, Node::ValidateInput)
        .await
        .unwrap()
        .remove(0);
    let validate_conv = ctx
        .store
        .get_conversation("t-think", validate_run.id)
        .await
        .unwrap()
        .expect("validate_input 节点应有会话行");
    assert_eq!(
        validate_conv.reasoning.as_deref(),
        Some("先读输入，信息是齐的"),
        "各 run 的会话行带各自的思考"
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
            watch_token_budget: None,
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
            watch_token_budget: None,
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
    // 共享脚本的 CodeChanges 已诚实申报（决策 397 后空申报会打回）→ prompt 列出文件清单
    assert!(te.user_prompt.contains("src/lib.rs"), "{}", te.user_prompt);
    assert!(te.user_prompt.contains("tests/acceptance.rs"));

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

// ───────────── 提交契约：零提交守卫与零变更申报（决策 391 / 票 commit-contract 01）─────────────
//
// 原缺陷（106 任务 01M47RQG4M9533F5TMF1AGJXC8）：develop.execute 全绿但变更从未落进任务
// 分支，闸门只看 lint + 单测就放行，一路穿越 review/test 在 merge 才撞「diff 为空」。

/// 决策 391：develop.execute 交了元数据、闸门全绿，但任务分支**零自有提交**
/// → validate_output 确定性打回 develop.execute（不前进），重入 prompt 带零提交事实段。
#[tokio::test]
async fn develop_gate_kicks_back_when_changes_are_never_committed() {
    let ctx = setup("true", Settings::default()).await;
    let mut script = Script::new();
    design_scripts(&mut script);
    // 现场复刻：写了文件却**不提交**——git status 脏、分支相对基准零提交。
    script
        .for_node(Stage::Develop, Node::Execute)
        .write_file(
            "src/lib.rs",
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n#[test]\nfn adds() { assert_eq!(add(1, 2), 3); }\n",
        )
        .write_file("tests/acceptance.rs", "#[test]\nfn ok() {}\n")
        .submit(&CodeChanges {
            branch_name: "kanban/t-zero".into(),
            changed_files: vec![],
            unit_test_files: vec![],
            no_changes: false,
        });
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t-zero", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-zero").await;

    ctx.executor.run("t-zero").await.unwrap();

    // ① 打回 develop.execute：任务不得越过 develop（这是事故的洞——此前这里放行）
    let transitions = ctx.store.list_transitions("t-zero").await.unwrap();
    assert!(
        transitions.iter().any(|t| t.to_stage == Stage::Develop
            && t.to_node == Node::Execute
            && t.trigger == TransitionTrigger::NodeRetry),
        "零提交必须打回 develop.execute：{transitions:?}"
    );
    assert!(
        !transitions
            .iter()
            .any(|t| t.to_stage == Stage::Review || t.to_stage == Stage::Merge),
        "零提交不得穿越到 review / merge：{transitions:?}"
    );
    let live = ctx.store.load_live_cursors("t-zero").await.unwrap();
    assert_eq!(
        (live[0].stage, live[0].node),
        (Stage::Develop, Node::Execute)
    );
    assert_eq!(
        live[0].validate_attempts, 1,
        "守卫失败计入 validate_attempts"
    );

    // ② 事实段已落盘（重入渲染的取数源）
    let facts_path = ctx.store.home().task_file("t-zero", "zero-commit-facts.md");
    let facts = std::fs::read_to_string(&facts_path).expect("零提交事实段应已落盘");
    assert!(facts.contains("自有提交数：**0**"), "{facts}");
    assert!(facts.contains("src/lib.rs"), "应列出未提交清单：{facts}");

    // ③ 重入 develop.execute 的 prompt 注入事实段；首轮不渲染
    let requests = ctx.agent.request_log();
    let devex: Vec<_> = requests
        .iter()
        .filter(|r| r.stage == Stage::Develop && r.node == Node::Execute)
        .collect();
    assert!(
        devex.len() >= 2,
        "至少应有两轮 develop.execute：{}",
        devex.len()
    );
    assert!(
        !devex[0].user_prompt.contains("## 零提交事实与落提交指令"),
        "首轮不渲染事实段"
    );
    let reentry = devex.last().expect("重入请求");
    assert!(
        reentry.user_prompt.contains("## 零提交事实与落提交指令"),
        "打回后的重入必须带事实段：{}",
        reentry.user_prompt
    );
    assert!(
        reentry.user_prompt.contains("git commit"),
        "事实段要带落提交指令"
    );
}

/// 决策 391：**工作区干净、也没申报**（agent 什么都没做）时，零自有提交同样在 develop
/// 处被拦——「全绿但分支等于基准」无论工作区脏否都不该穿越 review / test。
///
/// 这条比票面形状 3 的三项条件（「…**且工作区有变更**」）宽一档，有意为之：票面
/// 「What to build」写的目标就是「在 develop.validate_output 就被打回」，晚了就又要多穿
/// 两个阶段；而事实段里的「或申报 no_changes」一句对干净工作区同样成立（见决策 391）。
#[tokio::test]
async fn develop_gate_kicks_back_even_when_the_tree_is_clean_and_nothing_declared() {
    let ctx = setup("true", Settings::default()).await;
    let mut script = Script::new();
    design_scripts(&mut script);
    script
        .for_node(Stage::Develop, Node::Execute)
        .submit(&CodeChanges {
            branch_name: "kanban/t-nc".into(),
            changed_files: vec![],
            unit_test_files: vec![],
            no_changes: false,
        });
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t-nc", "p1").await.unwrap();
    admit(&ctx, "t-nc").await;

    ctx.executor.run("t-nc").await.unwrap();

    let transitions = ctx.store.list_transitions("t-nc").await.unwrap();
    assert!(
        transitions.iter().any(|t| t.to_stage == Stage::Develop
            && t.to_node == Node::Execute
            && t.trigger == TransitionTrigger::NodeRetry),
        "干净工作区 + 零提交也不得放行：{transitions:?}"
    );
    assert!(
        !transitions
            .iter()
            .any(|t| t.to_stage == Stage::Review || t.to_stage == Stage::Merge),
        "不得穿越到 review / merge：{transitions:?}"
    );
    let facts = std::fs::read_to_string(ctx.store.home().task_file("t-nc", "zero-commit-facts.md"))
        .expect("零提交事实段应已落盘");
    assert!(facts.contains("工作区未提交改动：0 处"), "{facts}");
}

/// 决策 391：申报 `no_changes` → develop.validate_output 挂 pending(user_decision)，
/// 用户确认（cancel）→ 任务以 cancelled 终态收口（不经 done，`do_done` 的 merged 硬校验
/// 对零变更无语义）。动作集是「继续修改 / 确认取消」两条。
#[tokio::test]
async fn develop_declared_no_changes_pends_then_cancelled_terminal() {
    let ctx = setup("true", Settings::default()).await;
    let mut script = Script::new();
    design_scripts(&mut script);
    script
        .for_node(Stage::Develop, Node::Execute)
        .submit(&CodeChanges {
            branch_name: "kanban/t-zc".into(),
            changed_files: vec![],
            unit_test_files: vec![],
            no_changes: true,
        });
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t-zc", "p1").await.unwrap();
    admit(&ctx, "t-zc").await;

    ctx.executor.run("t-zc").await.unwrap();

    // 挂在 develop 等用户确认
    let live = ctx.store.load_live_cursors("t-zc").await.unwrap();
    let reason = live[0].pending_reason.as_ref().expect("应挂 pending");
    assert_eq!(reason.kind, PendingKind::UserDecision);
    assert_eq!(
        reason.context.as_ref().and_then(|c| c.kind.as_deref()),
        Some("zero_changes")
    );
    let actions = ctx.store.allowed_actions_for_task("t-zc").await.unwrap();
    let names: Vec<&str> = actions.iter().map(|a| a.action.as_str()).collect();
    assert_eq!(names, vec!["goto", "cancel"]);

    // 用户确认零变更 → cancelled 终态（不经过 done / 不需要 merge 结果）
    ctx.store.cancel_task("t-zc").await.unwrap();
    assert_eq!(
        ctx.store.get_task("t-zc").await.unwrap().status,
        TaskStatus::Cancelled
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
    // 项目级 run（决策 100 / 迁移 0004）：app 路由在调之前先落它，再把 id 透进来。
    let run_id = ctx
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
    let merged = ctx
        .executor
        .project_analysis(&project, facts, Some(run_id))
        .await
        .unwrap();
    assert_eq!(merged["summary"], "这是一个 Rust 项目");
    assert_eq!(merged["suspicious"][0], "检测到多套测试框架");
    assert_eq!(merged["language"], "rust", "确定性探测事实必须保留");

    // 决策 329：这次调用的请求台账挂在**它自己的**项目级 run 上。
    // 修改前这里是空的——`RunContext.run_id` 填死 0，归一成 NULL，那几行三个归属键全空：
    // 读不出是哪一次分析的调用，删项目时也带不走。
    let rows = ctx.store.model_requests_for_run(run_id, 10).await.unwrap();
    assert_eq!(
        rows.len(),
        1,
        "这次分析的请求该按项目级 run 读得到：{rows:?}"
    );
    assert_eq!(rows[0].agent_type, "pseudo:project_analysis");
    assert!(rows[0].task_id.is_none(), "项目级调用没有任务归属");
}

/// 决策 329 的反向面：调用方没能落 run 行（`insert_project_run` 失败）时传 `None`，
/// 请求照落账、**不撞外键**（哨兵 0 归一成 NULL），只是没有归属可指。
#[tokio::test]
async fn project_analysis_without_a_run_row_still_logs_an_unowned_request() {
    use agentpipeline_core::pipeline::pseudo::ProjectAnalysisResult;

    let ctx = setup("true", Settings::default()).await;
    let mut script = Script::new();
    script
        .for_pseudo("pseudo:project_analysis")
        .submit(&ProjectAnalysisResult {
            summary: "摘要".into(),
            suspicious: vec![],
        });
    ctx.agent.set_script(script);

    let project = ctx.store.get_project("p1").await.unwrap().unwrap();
    let merged = ctx
        .executor
        .project_analysis(&project, serde_json::json!({"language": "rust"}), None)
        .await
        .unwrap();
    assert_eq!(merged["summary"], "摘要");

    let rows: Vec<(Option<i64>, Option<String>, String)> = sqlx::query_as(
        "SELECT run_id, task_id, agent_type FROM kanban_model_requests
         WHERE agent_type = 'pseudo:project_analysis'",
    )
    .fetch_all(ctx.store.pool())
    .await
    .unwrap();
    assert_eq!(rows.len(), 1, "无 run 行时请求也要落账：{rows:?}");
    assert_eq!(rows[0].0, None, "没有 run 行可指时 run_id 为 NULL，不是 0");
    assert_eq!(rows[0].1, None);
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
    // 失败以 tool_event error 形态外发（决策 123），且错误文本进 `result`（决策 301）
    let events = ctx.sse.events();
    let error_events: Vec<&SseEvent> = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                SseEvent::ToolEvent { phase: agentpipeline_core::sse::ToolPhase::Error, tool, .. }
                    if tool == "write_file"
            )
        })
        .collect();
    assert_eq!(error_events.len(), 1, "工具失败应发 error 事件");
    assert!(
        matches!(
            error_events.first(),
            Some(SseEvent::ToolEvent { result: Some(r), .. }) if r.contains("工具执行失败")
        ),
        "错误文本进 result：{:?}",
        error_events.first()
    );
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
            no_changes: false,
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

/// 决策 407：子代理轮数上限是**配置**——配成 2 轮，子代理在第 2 轮就打满收场，
/// 「未收口」的报数跟着配置走（不再写死 12），父代理拿到的回执里也是同一个数。
#[tokio::test]
async fn subagent_round_cap_follows_the_configured_number() {
    let settings = Settings {
        sub_agent_max_rounds: 2,
        ..Settings::default()
    };
    let ctx = setup("true", settings).await;
    declare_sub_agent(&ctx).await;

    let worktree = ctx.store.home().worktree_path("t-cap");
    std::fs::create_dir_all(&worktree).unwrap();
    std::fs::write(worktree.join("NOTES.md"), "关键结论：入口在 main()\n").unwrap();

    let mut script = Script::new();
    design_scripts(&mut script);
    script
        .for_node(Stage::Develop, Node::Execute)
        .push(testkit::Step::Tool {
            name: "spawn_sub_agent".into(),
            arguments: serde_json::json!({"task": "翻 NOTES.md"}),
        })
        .write_file(
            "src/lib.rs",
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n#[test]\nfn adds() { assert_eq!(add(1, 2), 3); }\n",
        )
        .write_file("tests/acceptance.rs", "#[test]\nfn ok() {}\n")
        .run_command(
            "git add -A && git -c user.name=f -c user.email=f@l commit -m 'feat: task t-cap'",
        )
        .submit(&CodeChanges {
            branch_name: "kanban/t-cap".into(),
            changed_files: vec![],
            unit_test_files: vec![],
            no_changes: false,
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
    // 子代理**两轮都发工具调用**：永远走不到「不再发起 tool_call」的自然收口，
    // 于是第 2 轮就是配置给的顶。
    script.push_subagent(testkit::Step::Tool {
        name: "read_file".into(),
        arguments: serde_json::json!({"path": "NOTES.md"}),
    });
    script.push_subagent(testkit::Step::Tool {
        name: "read_file".into(),
        arguments: serde_json::json!({"path": "NOTES.md"}),
    });

    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t-cap", "p1").await.unwrap();
    admit(&ctx, "t-cap").await;
    ctx.executor.run("t-cap").await.unwrap();

    // ① 子代理 run 行：失败，且报的是**配置的**轮数
    let runs = ctx.store.list_runs("t-cap").await.unwrap();
    let sub = runs
        .iter()
        .find(|r| r.agent_type == "subagent")
        .expect("应有子代理 run 行");
    assert_eq!(sub.status, NodeStatus::Failed);
    let err = sub.error.clone().unwrap_or_default();
    assert!(err.contains("2 轮内未收口"), "报数要跟着配置: {err}");
    assert!(!err.contains("12 轮"), "旧常量 12 不该再出现: {err}");

    // ② 父代理拿到的回执是同一句话（工具层文本通道，父的下一轮 messages 里能读到）
    let requests = ctx.agent.request_log();
    let carried = requests.iter().any(|r| {
        r.stage == Stage::Develop
            && r.node == Node::Execute
            && r.messages.iter().any(|m| {
                m.role == Role::Tool
                    && m.content
                        .as_deref()
                        .is_some_and(|c| c.contains("2 轮内未收口"))
            })
    });
    assert!(carried, "父的下一轮 messages 应含「2 轮内未收口」的回执");
}

/// 决策 408：子代理手里真的多了一只检索的手——它在工作区里搜一把（`search_content`
/// **真执行**），命中随回执行文本进它的转录，然后才给摘要收口。
///
/// 这是这次事故的正解：子任务常是「扫一遍某目录找出…」，没有检索工具的只读子代理只能
/// 一个目录一个目录地翻，12 轮全用在翻文件上（2026-10-08，任务 01M4CD59）。
#[tokio::test]
async fn subagent_can_search_the_worktree_with_search_content() {
    let ctx = setup("true", Settings::default()).await;
    declare_sub_agent(&ctx).await;

    let worktree = ctx.store.home().worktree_path("t-search");
    std::fs::create_dir_all(&worktree).unwrap();
    std::fs::write(
        worktree.join("NOTES.md"),
        "关键结论：入口在 main()\n与检索无关的一行\n",
    )
    .unwrap();

    let mut script = Script::new();
    design_scripts(&mut script);
    script
        .for_node(Stage::Develop, Node::Execute)
        .push(testkit::Step::Tool {
            name: "spawn_sub_agent".into(),
            arguments: serde_json::json!({"task": "在工作区里搜「入口在」落在哪"}),
        })
        .write_file(
            "src/lib.rs",
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n#[test]\nfn adds() { assert_eq!(add(1, 2), 3); }\n",
        )
        .write_file("tests/acceptance.rs", "#[test]\nfn ok() {}\n")
        .run_command(
            "git add -A && git -c user.name=f -c user.email=f@l commit -m 'feat: task t-search'",
        )
        .submit(&CodeChanges {
            branch_name: "kanban/t-search".into(),
            changed_files: vec![],
            unit_test_files: vec![],
            no_changes: false,
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
    // 子代理：真搜一把，再按摘要收口。
    script.push_subagent(testkit::Step::Tool {
        name: "search_content".into(),
        arguments: serde_json::json!({"pattern": "入口在"}),
    });
    script.push_subagent(testkit::Step::Text(
        "命中 NOTES.md：入口在 main()（源：search_content）".into(),
    ));

    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t-search", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-search").await;
    ctx.executor.run("t-search").await.unwrap();

    // ① 回执行文本带着命中（相对路径 + 行文本）——「真执行」的证据
    let requests = ctx.agent.request_log();
    let hits = requests
        .iter()
        .filter(|r| r.run.as_ref().is_some_and(|c| c.agent_type == "subagent"))
        .flat_map(|r| r.messages.iter())
        .filter(|m| m.role == Role::Tool)
        .filter_map(|m| m.content.as_deref())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(hits.contains("NOTES.md"), "命中要带相对路径：{hits}");
    assert!(hits.contains("入口在"), "命中要带行文本：{hits}");

    // ② 检索不是子代理的失败源：它照常按摘要收口。
    let runs = ctx.store.list_runs("t-search").await.unwrap();
    let sub = runs
        .iter()
        .find(|r| r.agent_type == "subagent")
        .expect("应有子代理 run 行");
    assert_eq!(sub.status, NodeStatus::Success);
}

/// 决策 409 的公共形状：父节点派出子代理 → 子代理停在第 2 轮的模型调用上 → 请求中止
/// → 收口 / 台账 / 回执三处按同一条规则落。`hold` 选来路（人按停 / 判超时）。
///
/// 任务 id 由调用方给：执行体的进程内登记是**全局**的（同 id 并发跑两个执行体会被
/// 去重逐个拒掉），两条来路的用例必须各用各的 id。
async fn stalled_subagent_stops_with(task_id: &str, hold: bool) {
    let ctx = setup("true", Settings::default()).await;
    declare_sub_agent(&ctx).await;

    let worktree = ctx.store.home().worktree_path(task_id);
    std::fs::create_dir_all(&worktree).unwrap();
    std::fs::write(worktree.join("NOTES.md"), "关键结论：入口在 main()\n").unwrap();

    let mut script = Script::new();
    design_scripts(&mut script);
    script
        .for_node(Stage::Develop, Node::Execute)
        .push(testkit::Step::Tool {
            name: "spawn_sub_agent".into(),
            arguments: serde_json::json!({"task": "读 NOTES.md 并总结入口"}),
        });
    // 子代理第 1 轮真实读一次，第 2 轮的模型调用**停住**——中止正打在这一轮上。
    // 「停在第 2 轮」是个可辨读数：它证明叫停的是模型调用那个观察点，而不是别处。
    script.push_subagent(testkit::Step::Tool {
        name: "read_file".into(),
        arguments: serde_json::json!({"path": "NOTES.md"}),
    });
    script.push_subagent(testkit::Step::Stall);

    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, task_id, "p1").await.unwrap();
    admit(&ctx, task_id).await;

    // 执行体挪进后台任务（中止要打在它跑着的时候）。**部分移动**：`admit` 那类
    // 整结构借用必须在这之前调完，之后只按字段用 `ctx`。
    let executor = Arc::new(ctx.executor);
    let jh = {
        let e = executor.clone();
        let id = task_id.to_string();
        tokio::spawn(async move { e.run(&id).await })
    };
    // 等到子代理的第 2 轮请求已经发出（它此刻正停在 `Stall` 上，永不返回）。
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let subs = ctx
                .agent
                .request_log()
                .iter()
                .filter(|r| r.run.as_ref().is_some_and(|c| c.agent_type == "subagent"))
                .count();
            if subs >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("子代理应走到第 2 轮（就是停住的那次模型调用）");

    let requested = if hold {
        agentpipeline_core::pipeline::executor::request_hold(task_id)
    } else {
        agentpipeline_core::pipeline::executor::request_cancel(task_id)
    };
    assert!(requested, "在跑的执行体应当找得到，才谈得上通知它收口");
    tokio::time::timeout(Duration::from_secs(30), jh)
        .await
        .expect("收到中止请求的执行体应当在有界时间内收口（子代理不得再跑满自己的预算）")
        .unwrap()
        .unwrap();

    // ① 子代理自己的 run 行：中止**不是失败**（决策 226 的同一姿态），来路与轮号可读。
    let sub = ctx
        .store
        .list_runs(task_id)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.agent_type == "subagent")
        .expect("子代理应有自己的 run 行");
    assert_eq!(
        sub.status,
        NodeStatus::Cancelled,
        "被中止的子代理记 cancelled，不记 failed"
    );
    // 来路落 `cancel_origin` 列（判据读它，不读报文字样——决策 259/276 的同一口径）。
    // 读模型（`NodeRun`）不暴露这一列，故直接问库。
    let origin: Option<String> =
        sqlx::query_scalar("SELECT cancel_origin FROM kanban_node_runs WHERE id = ?")
            .bind(sub.id)
            .fetch_one(ctx.store.pool())
            .await
            .unwrap();
    let expected_origin = if hold {
        agentpipeline_core::storage::observability::CANCEL_ORIGIN_HOLD
    } else {
        agentpipeline_core::storage::observability::CANCEL_ORIGIN_TIMEOUT
    };
    assert_eq!(
        origin.as_deref(),
        Some(expected_origin),
        "来路落在 cancel_origin 列上"
    );
    let error = sub.error.clone().unwrap_or_default();
    assert!(error.contains("第 2 轮"), "台账要读得出停在哪一轮：{error}");
    let expected_word = if hold { "人工暂停" } else { "节点超时" };
    assert!(error.contains(expected_word), "来路也写进 error：{error}");

    // ② 回给父代理的回执标「未完成」——父转录里那一行就是它。
    let transcript = ctx
        .store
        .latest_own_transcript(task_id, Stage::Develop, Node::Execute)
        .await
        .unwrap()
        .expect("父节点的消息日志应当有行");
    let receipt = transcript
        .messages
        .iter()
        .filter_map(|m| m.content.as_deref())
        .find(|c| c.contains("子代理被中止"))
        .unwrap_or_else(|| panic!("父转录里应有子代理的中止回执"));
    assert!(
        receipt.contains("第 2 轮，未完成"),
        "回执标「未完成」：{receipt}"
    );

    // ③ 父节点让路而不是失败：中止的落点归发出请求的那一方（决策 276）。
    let parent = ctx
        .store
        .list_runs_at(task_id, Stage::Develop, Node::Execute)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.agent_type == "main")
        .expect("父节点的 run 行");
    assert_eq!(parent.status, NodeStatus::Cancelled, "父节点让路而非失败");
    let cursors = ctx.store.load_live_cursors(task_id).await.unwrap();
    assert!(
        cursors.iter().all(|c| !c.is_pending()),
        "中止不得把节点挂成 pending（那是失败才有的落点）"
    );
}

/// 决策 409：**人按停要按得住子代理**。
///
/// 2026-10-08 的实证（任务 `01M4CD59`）：暂停请求 08:18:47 写下，子代理又跑了 80 秒
/// ——它手里根本没有中止观察点，父节点那条轮边界检查管不到别人 await 着的工具调用。
/// 这一条把三处一起钉住：**模型调用**上的观察点（`select!` 把它叫醒，不需要动
/// `max_duration`）、子代理 run 行的记法（`cancelled` + 来路 `hold` + 停在第几轮）、
/// 回给父代理的回执（标「未完成」）。
#[tokio::test]
async fn a_hold_stops_a_stalled_subagent_at_the_model_call() {
    stalled_subagent_stops_with("t-stop", true).await;
}

/// 决策 409：**判超时**走同一条通道（`request_cancel` → `CancelOrigin::Timeout`）。
///
/// 来路不同、收口相同——只有 `cancel_origin` 那一列与 error 上的人话不同（决策 276
/// 把两种来路分开记的理由：人按停是介入，判超时的中止是超时自己的副产品）。
#[tokio::test]
async fn a_timeout_cancel_stops_a_stalled_subagent_with_its_own_origin() {
    stalled_subagent_stops_with("t-stop-t", false).await;
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
            no_changes: false,
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

    // 子代理那一次请求的工具集：固定只读的三件（决策 408 加了检索——它只读，
    // 且是「把读 20 个文件的原文挡在父上下文之外」这件本职最省上下文的那只手）
    let requests = ctx.agent.request_log();
    let sub_req = requests
        .iter()
        .find(|r| r.run.as_ref().is_some_and(|c| c.agent_type == "subagent"))
        .expect("应有子代理 LLM 请求");
    let names: Vec<&str> = sub_req.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["read_file", "list_dir", "search_content"],
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
    // 检索工具的广告**不是空壳**（决策 353 的姿态）：描述非空，schema 与值班长共用
    // 同一份（`pattern` 是必填的那个字段），认下这条才谈得上「模型会用」。
    let search = sub_req
        .tools
        .iter()
        .find(|t| t.name == "search_content")
        .expect("应有 search_content 定义");
    assert!(
        !search.description.trim().is_empty(),
        "检索工具的广告语不得为空壳"
    );
    assert!(
        search.parameters["required"]
            .as_array()
            .is_some_and(|req| req.iter().any(|v| v == "pattern")),
        "参数 schema 应要求 pattern：{}",
        search.parameters
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
            no_changes: false,
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
    // 思考留痕（决策 360）：子代理的思考同样随它自己的会话行落地
    script.push_subagent_thinking("先翻 NOTES.md，再看入口");

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
    // 思考随子代理自己的会话行落地（决策 360），不混进父会话
    assert_eq!(
        sub_conv.reasoning.as_deref(),
        Some("先翻 NOTES.md，再看入口"),
        "子代理的思考随它自己的会话行落地"
    );
    assert!(
        !convs
            .iter()
            .filter(|c| c.agent_type != "subagent")
            .any(|c| c.reasoning.as_deref() == Some("先翻 NOTES.md，再看入口")),
        "子代理的思考不串进父会话"
    );
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
            no_changes: false,
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
            no_changes: false,
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
        vec!["read_file", "list_dir", "search_content"],
        "阶段声明的工具不得传给子代理（它拿到的是**固定**只读集，决策 172③ / 408）"
    );
}

/// 验收（决策 400）：设计阶段**未配置** `tools_json` 时，`spawn_sub_agent` 默认进广告集
/// （与运行器注入同一来源取值）；develop / review / test 维持默认关闭——请求里没有它。
#[tokio::test]
async fn spawn_sub_agent_default_on_for_design_stages_only() {
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
    for stage in [
        Stage::ArchitectDesign,
        Stage::DevelopDesign,
        Stage::TestDesign,
    ] {
        let stage_reqs: Vec<_> = requests.iter().filter(|r| r.stage == stage).collect();
        assert!(!stage_reqs.is_empty(), "{stage:?} 应有请求");
        assert!(
            stage_reqs
                .iter()
                .all(|r| r.tools.iter().any(|t| t.name == "spawn_sub_agent")),
            "设计阶段 {stage:?} 未配置时应默认广告 spawn_sub_agent"
        );
    }
    for stage in [Stage::Develop, Stage::Review, Stage::Test] {
        assert!(
            requests
                .iter()
                .filter(|r| r.stage == stage)
                .all(|r| !r.tools.iter().any(|t| t.name == "spawn_sub_agent")),
            "非默认阶段 {stage:?} 不得出现 spawn_sub_agent"
        );
    }
}

/// 验收（决策 400）：显式配置**原样生效**——设计阶段给 `[]` 就是显式关闭，全流水线的
/// 请求里都不再出现。「没配过」的默认与「配了」的原样是两种状态，后者说了算。
#[tokio::test]
async fn spawn_sub_agent_absent_when_design_stages_opted_out() {
    let ctx = setup("true", Settings::default()).await;
    for stage in ["architect-design", "develop-design", "test-design"] {
        ctx.store
            .upsert_stage_config(&agentpipeline_core::types::StageConfig {
                stage: stage.into(),
                tools_json: Some(serde_json::json!([])),
                updated_at: ctx.store.now(),
                ..Default::default()
            })
            .await
            .unwrap();
    }

    let mut script = Script::new();
    design_scripts(&mut script);
    implementation_scripts(&mut script, "t-optout");
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t-optout", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-optout").await;
    ctx.executor.run("t-optout").await.unwrap();

    let requests = ctx.agent.request_log();
    for req in &requests {
        assert!(
            !req.tools.iter().any(|t| t.name == "spawn_sub_agent"),
            "显式 [] 后不得出现 spawn_sub_agent（stage={:?}）：{:?}",
            req.stage,
            req.tools.iter().map(|t| &t.name).collect::<Vec<_>>()
        );
    }
}

/// 验收（决策 400）：设计阶段的默认开启不只是「广告里有」——运行器按同一份生效值注入，
/// 父在 architect-design 派子代理，摘要照常经 tool_result 回灌（无需任何显式声明）。
#[tokio::test]
async fn parent_spawns_subagent_on_architect_design_by_default() {
    let ctx = setup("true", Settings::default()).await;

    // 子代理要读的文件：真实存在于 worktree（工具层全真执行，决策 148）
    let worktree = ctx.store.home().worktree_path("t-archsub");
    std::fs::create_dir_all(&worktree).unwrap();
    std::fs::write(worktree.join("NOTES.md"), "关键结论：入口在 main()\n").unwrap();

    let mut script = Script::new();
    // architect-design.execute 队列最前插一步派子代理，design_scripts 再追加 write/submit
    script
        .for_node(Stage::ArchitectDesign, Node::Execute)
        .push(testkit::Step::Tool {
            name: "spawn_sub_agent".into(),
            arguments: serde_json::json!({"task": "读 NOTES.md 并总结入口"}),
        });
    design_scripts(&mut script);
    implementation_scripts(&mut script, "t-archsub");
    // 子代理自己的脚本：先 read_file（真实读），再给摘要收口
    script.push_subagent(testkit::Step::Tool {
        name: "read_file".into(),
        arguments: serde_json::json!({"path": "NOTES.md"}),
    });
    script.push_subagent(testkit::Step::Text("入口在 main()（源：NOTES.md）".into()));

    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "t-archsub", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-archsub").await;
    ctx.executor.run("t-archsub").await.unwrap();

    // 摘要进入了父 architect-design 的下一轮 messages（tool_result 通道，决策 172③）
    let requests = ctx.agent.request_log();
    let arch: Vec<_> = requests
        .iter()
        .filter(|r| r.stage == Stage::ArchitectDesign && r.node == Node::Execute)
        .collect();
    assert!(
        arch.iter().any(|r| {
            r.messages.iter().any(|m| {
                m.role == agentpipeline_core::agent::Role::Tool
                    && m.content
                        .as_deref()
                        .is_some_and(|c| c.contains("入口在 main()"))
            })
        }),
        "父 architect-design 的后续轮 messages 应含子代理摘要"
    );
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
            no_changes: false,
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

/// 决策 277④：info_insufficient 的 pending 消息携带 blockers 摘要——
/// 「要问什么」直接出现在看板卡片的 pending 原因里，用户不必翻会话记录才知道要答什么。
#[tokio::test]
async fn info_insufficient_pending_message_carries_blockers_summary() {
    let ctx = setup("true", Settings::default()).await;
    let mut script = Script::new();
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: false,
            blockers: vec!["部署目标是什么？推荐：本地 Docker".into()],
        });
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "vi-blockers", "p1")
        .await
        .unwrap();
    admit(&ctx, "vi-blockers").await;
    ctx.executor.run("vi-blockers").await.unwrap();

    let cursor = ctx
        .store
        .load_live_cursors("vi-blockers")
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let reason = cursor
        .pending_reason
        .expect("readiness=false 应挂 info_insufficient");
    assert_eq!(reason.kind, PendingKind::InfoInsufficient);
    assert!(
        reason
            .message
            .contains("1. 部署目标是什么？推荐：本地 Docker"),
        "pending 消息须带 blockers 摘要：{}",
        reason.message
    );
}

/// 票 05②：答复 `info_insufficient` 时，落盘的 `user-input.md` 要把**问题 + 推荐答案**
/// 和**用户答复**放在一起，下游才看得见「同意」同意的是什么。
///
/// 现场正是这两样各走各的：文件里只有孤零零一个「同意」（问题清单只活在 pending 消息
/// 和会话转录里），architect-design 于是在 60 分钟里翻了 89 次仓库猜范围。问了两个问题
/// （症状 / 验收）是照事故的问答形状取的。
#[tokio::test]
async fn an_info_insufficient_answer_is_recorded_together_with_its_questions() {
    let ctx = setup("true", Settings::default()).await;
    let mut script = Script::new();
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: false,
            blockers: vec![
                "闪屏出现在哪个页面？推荐：三次登录页".into(),
                "验收标准按什么算？推荐：首屏 1 秒内无白屏".into(),
            ],
        });
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "cont-qa", "p1")
        .await
        .unwrap();
    admit(&ctx, "cont-qa").await;
    ctx.executor.run("cont-qa").await.unwrap();

    let cursor = ctx
        .store
        .load_live_cursors("cont-qa")
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    // 用户答复就是事故现场的原话
    agentpipeline_core::pipeline::resume::apply_action(
        &ctx.store,
        &cursor,
        ResumeAction::Continue,
        None,
        Some("同意"),
    )
    .await
    .unwrap();

    let recorded =
        std::fs::read_to_string(ctx._home.home().task_file("cont-qa", "user-input.md")).unwrap();
    for expected in [
        "闪屏出现在哪个页面",
        "推荐：三次登录页",
        "验收标准按什么算",
        "推荐：首屏 1 秒内无白屏",
        "## 用户答复",
        "同意",
    ] {
        assert!(recorded.contains(expected), "缺 `{expected}`：\n{recorded}");
    }

    // 三样都要能进下游 prompt：让重入的 validate_input 通过，读 architect-design.execute
    // 那条请求的 user prompt（重入段渲染在那里，决策 79）。
    let mut rerun = Script::new();
    rerun
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    ctx.agent.set_script(rerun);
    let _ = ctx.executor.run("cont-qa").await;

    let execute = ctx
        .agent
        .request_log()
        .into_iter()
        .find(|r| r.stage == Stage::ArchitectDesign && r.node == Node::Execute)
        .expect("重入后应走到 architect-design.execute");
    for expected in ["闪屏出现在哪个页面", "推荐：三次登录页", "同意"] {
        assert!(
            execute.user_prompt.contains(expected),
            "execute 的 user prompt 缺 `{expected}`：\n{}",
            execute.user_prompt
        );
    }
}

// ── 决策 387 · 评审打回反馈改「转录末尾 user turn」──

/// 首轮跑到 review 判定不通过、人按「打回开发修复」把游标放回 develop.execute。
async fn run_to_review_reject(ctx: &Ctx, task_id: &str, review: &ReviewResult) {
    let mut script = Script::new();
    design_scripts(&mut script);
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
            changed_files: vec![
                FileChangeSpec {
                    path: "src/lib.rs".into(),
                    action: FileAction::Create,
                    content_hash: None,
                },
                FileChangeSpec {
                    path: "tests/acceptance.rs".into(),
                    action: FileAction::Create,
                    content_hash: None,
                },
            ],
            unit_test_files: vec![],
            no_changes: false,
        });
    script.for_node(Stage::Review, Node::Execute).submit(review);
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, task_id, "p1").await.unwrap();
    admit(ctx, task_id).await;
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
        PendingKind::UserDecision,
        "review 不通过应 pend 等人裁决"
    );
    agentpipeline_core::pipeline::resume::apply_action(
        &ctx.store,
        &cursor,
        ResumeAction::Goto,
        Some((Stage::Develop, Node::Execute)),
        None,
    )
    .await
    .unwrap();
}

/// 打回重入的 develop.execute 脚本（真写代码真提交）+ 复审通过 + test 通过。
fn rework_scripts(script: &mut Script, task_id: &str) {
    script
        .for_node(Stage::Develop, Node::Execute)
        .write_file("src/lib.rs", "pub fn add(a: i32, b: i32) -> i32 { a.saturating_add(b) }\n#[test]\nfn adds() { assert_eq!(add(1, 2), 3); }\n")
        .run_command(&format!(
            "git add -A && git -c user.name=f -c user.email=f@l commit -m 'fix: task {task_id}'"
        ))
        .submit(&CodeChanges {
            branch_name: format!("kanban/{task_id}"),
            changed_files: vec![],
            unit_test_files: vec![],
            no_changes: false,
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

/// 决策 387：评审打回 develop 重入时，反馈是**续接转录末尾**的一条带前缀 user turn
/// （finding 内联 + 评审报告绝对路径），首条消息逐字不变、段不再渲染进 user prompt。
#[tokio::test]
async fn review_rework_feedback_is_a_prefixed_turn_at_the_end_of_the_carried_transcript() {
    let ctx = setup("true", Settings::default()).await;
    run_to_review_reject(
        &ctx,
        "t-rework",
        &ReviewResult {
            approved: false,
            review_report_path: Some("review-report.md".into()),
            required_changes: vec![ReviewRequiredChange {
                path: "src/lib.rs".into(),
                action: agentpipeline_core::types::FileAction::Modify,
                finding: Some("add 未按设计处理负数入参：按 AC-1 改为 saturating".into()),
            }],
        },
    )
    .await;

    let mut rework = Script::new();
    rework_scripts(&mut rework, "t-rework");
    ctx.agent.set_script(rework);
    ctx.executor.run("t-rework").await.unwrap();

    let requests = ctx.agent.request_log();
    let mut develop = requests
        .iter()
        .filter(|r| r.stage == Stage::Develop && r.node == Node::Execute)
        .peekable();
    assert!(develop.peek().is_some(), "develop.execute 应有请求");
    let first_run = develop.peek().unwrap().run.as_ref().unwrap().run_id;
    let develop: Vec<_> = develop.collect();
    let first_round_last = develop
        .iter()
        .rev()
        .find(|r| r.run.as_ref().unwrap().run_id == first_run)
        .expect("首轮 develop.execute 应有请求");
    let reentry = develop
        .iter()
        .find(|r| r.run.as_ref().unwrap().run_id != first_run)
        .expect("打回后 develop.execute 应重入");
    let reentry = *reentry;
    assert!(
        reentry.messages.len() > first_round_last.messages.len(),
        "重入转录应比首轮长（续接 + 追加 turn）"
    );
    assert_eq!(
        reentry.messages[..first_round_last.messages.len()],
        first_round_last.messages[..],
        "重入转录的前缀必须与首轮转录逐字相同（prompt cache 承诺）"
    );

    // ② 转录末尾是带结构化前缀的 user turn，finding 内联 + 报告绝对路径。
    let last = reentry.messages.last().unwrap();
    assert_eq!(
        last.role,
        agentpipeline_core::agent::Role::User,
        "打回反馈应是转录末尾的 user turn"
    );
    let report = ctx
        ._home
        .home()
        .task_file("t-rework", "review-report.md")
        .display()
        .to_string();
    let body = last.content.as_deref().unwrap_or("");
    for expected in [
        agentpipeline_core::types::REVIEW_REWORK_TURN_PREFIX,
        "src/lib.rs",
        "add 未按设计处理负数入参：按 AC-1 改为 saturating",
        &report,
    ] {
        assert!(
            body.contains(expected),
            "打回 turn 缺 `{expected}`：\n{body}"
        );
    }

    // ③ 段不再渲染进首条消息：重入的 user prompt 无「评审必须修改项」标题。
    assert!(
        !reentry.user_prompt.contains("评审必须修改项"),
        "打回反馈已 turn 化，user prompt 不应再渲染该段：\n{}",
        reentry.user_prompt
    );
    // 首轮（无 review 产出）user prompt 与转录里都不应出现该反馈。
    assert!(!first_round_last.user_prompt.contains("评审必须修改项"));
    assert!(first_round_last.messages.iter().all(|m| !m
        .content
        .as_deref()
        .is_some_and(|c| c.contains(agentpipeline_core::types::REVIEW_REWORK_TURN_PREFIX))));
}

/// 旧格式评审产出（required_changes 无 finding）向后兼容：turn 降级为
/// 「只列路径 + 报告绝对路径」，不空转、不静默。
#[tokio::test]
async fn review_rework_turn_degrades_when_required_changes_carry_no_finding() {
    let ctx = setup("true", Settings::default()).await;
    run_to_review_reject(
        &ctx,
        "t-legacy",
        &ReviewResult {
            approved: false,
            review_report_path: Some("review-report.md".into()),
            required_changes: vec![ReviewRequiredChange {
                path: "src/lib.rs".into(),
                action: agentpipeline_core::types::FileAction::Modify,
                finding: None,
            }],
        },
    )
    .await;

    let mut rework = Script::new();
    rework_scripts(&mut rework, "t-legacy");
    ctx.agent.set_script(rework);
    ctx.executor.run("t-legacy").await.unwrap();

    let requests = ctx.agent.request_log();
    let mut develop = requests
        .iter()
        .filter(|r| r.stage == Stage::Develop && r.node == Node::Execute)
        .peekable();
    assert!(develop.peek().is_some(), "develop.execute 应有请求");
    let first_run = develop.peek().unwrap().run.as_ref().unwrap().run_id;
    let reentry = requests
        .iter()
        .find(|r| {
            r.stage == Stage::Develop
                && r.node == Node::Execute
                && r.run.as_ref().unwrap().run_id != first_run
        })
        .expect("打回后 develop.execute 应重入");
    let report = ctx
        ._home
        .home()
        .task_file("t-legacy", "review-report.md")
        .display()
        .to_string();
    let last = reentry.messages.last().unwrap();
    let body = last.content.as_deref().unwrap_or("");
    for expected in [
        agentpipeline_core::types::REVIEW_REWORK_TURN_PREFIX,
        "src/lib.rs",
        &report,
    ] {
        assert!(
            body.contains(expected),
            "降级 turn 缺 `{expected}`：\n{body}"
        );
    }
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

/// 重试轮续接转录＋错误 turn（决策 278，显式修订决策 205 裁决②）：
/// `agent_retry_max` 的自动重试不再空起步——上一轮转录原样保留（不折叠、不省略），
/// 末尾多一条明示「已判废、本轮必须交元数据」的错误 turn。
///
/// 现场：resume 重入后脚本只有一句纯文本，抽不出元数据——三次尝试全部失败。此前
/// 「干净重试」的每次起点都是空；如今每次起点都比上一次多两条（assistant 回复 + 错误
/// turn）。耗尽后的 pending 文案也换成人话（不再外露裸诊断串）。
#[tokio::test]
async fn a_failed_retry_carries_the_previous_transcript_and_the_error_turn() {
    let ctx = info_insufficient_ctx("cont-retry").await;
    let cursor = ctx
        .store
        .load_live_cursors("cont-retry")
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();

    // 重入这一轮：唯一的脚本是纯文本（抽不出元数据），后续尝试走「脚本耗尽」收尾响应——同样失败
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
    // 按 run 分组取**首条请求**：首轮（info_insufficient_ctx）/ resume 那次 / 重试 1 / 重试 2
    //（agent_retry_max = 3）。一次 attempt 内的后续请求（工具往来）不算起点。
    let mut firsts: Vec<&LlmRequest> = Vec::new();
    for r in &vi {
        let run_id = r.run.as_ref().expect("主 agent 请求须带 run 上下文").run_id;
        if firsts.last().map(|p| p.run.as_ref().unwrap().run_id) != Some(run_id) {
            firsts.push(r);
        }
    }
    assert!(
        firsts.len() >= 4,
        "首轮 + resume + 两次自动重试：{}",
        firsts.len()
    );
    assert_eq!(firsts[0].messages.len(), 0, "首轮起点为空");
    assert_eq!(
        firsts[1].messages.len(),
        3,
        "resume 起点带上一轮的完整转录（工具往来 2 条 + 收尾文本 1 条，决策 205 不变）"
    );
    // 决策 278：每次失败重试的起点都比上一次多两条——assistant 回复 + 错误 turn。
    for w in 2..firsts.len() {
        assert_eq!(
            firsts[w].messages.len(),
            firsts[w - 1].messages.len() + 2,
            "重试轮 {w} 的起点应是上一轮转录 + 错误 turn（+2）"
        );
    }
    // 重试请求的末条消息就是错误 turn：user 角色、明示判废与契约要求
    let retried = firsts.last().unwrap();
    let last = retried.messages.last().expect("重试请求不应为空");
    assert_eq!(
        last.role,
        agentpipeline_core::agent::client::Role::User,
        "错误 turn 是一条 user 消息：{last:?}"
    );
    let turn = last.content.as_deref().unwrap_or("");
    assert!(turn.contains("已判废"), "明示上一轮已判废：{turn}");
    assert!(
        turn.contains("submit_metadata"),
        "明示本轮最终必须交元数据：{turn}"
    );
    assert!(
        turn.contains("未找到结构化元数据"),
        "原始诊断照给模型（人话化只面向用户）：{turn}"
    );
    // 转录原样保留：上一轮的 assistant 文本一字不动地在请求里
    assert!(
        retried
            .messages
            .iter()
            .any(|m| m.content.as_deref() == Some("这不是结构化元数据，抽取必然失败")),
        "上一轮的输出原样保留，不折叠不省略（决策 278）"
    );

    // 耗尽后的 pending 文案人话化（决策 278）：裸诊断串不再直接外露
    let cursor = ctx
        .store
        .load_live_cursors("cont-retry")
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let reason = cursor.pending_reason.expect("重试耗尽应挂 pending");
    assert_eq!(reason.kind, PendingKind::RetryExhausted);
    assert!(
        reason.message.contains("输出未按契约提交"),
        "用户看到的是人话（措辞对 execute 节点也通用）：{}",
        reason.message
    );
    assert!(
        !reason.message.contains("未找到结构化元数据"),
        "裸诊断串不再外露：{}",
        reason.message
    );

    // 红线③不变：链接只落续接边界那一轮（resume 那次），重试轮不落链
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
        "恰有一条 run 记下续接来源（重试轮不落链，决策 278 不改链接语义）：{linked:?}"
    );
}

/// 决策 279：补充输入作为 user turn 追加到转录末尾——续接请求的首条消息（system +
/// user_prompt）与上一轮**逐字一致**（segment 停用，prompt cache 的前缀承诺延伸到
/// resume），转录里则真的有用户那句发言（run40 的教训：用户的话只活在重渲染的开场白里，
/// 43 条归档消息与上一轮逐字相同，查无此人）。
#[tokio::test]
async fn supplement_input_rides_the_transcript_tail_and_leaves_the_first_message_verbatim() {
    let ctx = info_insufficient_ctx("supp-turn").await;
    let cursor = ctx
        .store
        .load_live_cursors("supp-turn")
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
        Some("部署在 k8s，单机即可"),
    )
    .await
    .unwrap();
    ctx.executor.run("supp-turn").await.unwrap();

    // 两条 run 各自的首条请求（一轮内的后续请求不算）
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
    let mut firsts: Vec<&LlmRequest> = Vec::new();
    for r in &vi {
        let run_id = r.run.as_ref().expect("主 agent 请求须带 run 上下文").run_id;
        if firsts.last().map(|p| p.run.as_ref().unwrap().run_id) != Some(run_id) {
            firsts.push(r);
        }
    }
    assert!(firsts.len() >= 2, "首轮 + 续接那轮：{}", firsts.len());
    let prev = firsts[0];
    let resumed = firsts[1];

    // ① 首条消息逐字一致：segment 停用后，补充输入不再重渲染进开场白
    assert_eq!(prev.system_prompt, resumed.system_prompt, "system 不变");
    assert_eq!(
        prev.user_prompt, resumed.user_prompt,
        "user_prompt 逐字一致——缓存前缀不打穿（run40 实测：25,800 prompt 只命中 126）"
    );
    // ② user turn 在转录末尾，内容就是用户说的那句话（verbatim，不带标题行）
    let last = resumed.messages.last().expect("续接请求不应为空");
    assert_eq!(
        last.role,
        agentpipeline_core::agent::client::Role::User,
        "补充输入是一条真实的 user 消息：{last:?}"
    );
    assert_eq!(last.content.as_deref(), Some("部署在 k8s，单机即可"));
    // ③ 末尾之前的消息 = 上一轮会话行逐字
    let prev_run_id = prev.run.as_ref().unwrap().run_id;
    let convs = ctx
        .store
        .list_conversations("supp-turn", true)
        .await
        .unwrap();
    let prev_conv = convs
        .iter()
        .find(|c| c.run_id == prev_run_id)
        .expect("上一轮的会话行");
    let prev_msgs: Vec<agentpipeline_core::agent::client::Message> =
        serde_json::from_value(prev_conv.messages_json.clone()).unwrap();
    assert!(!prev_msgs.is_empty(), "上一轮须有工具往来可断言");
    assert_eq!(
        &resumed.messages[..prev_msgs.len()],
        prev_msgs.as_slice(),
        "续接装配逐字保留上一轮转录"
    );
    assert_eq!(resumed.messages.len(), prev_msgs.len() + 1);
}

/// 决策 79 落盘纪律的边缘（评审发现）：user-input.md 只写不清——同一
/// info_insufficient 第二次「继续」且**不带新输入**时，文件里还是上一次的补充，
/// 而它已经作为 user turn 在续接转录里了。重放会让同一段发言出现两遍。
#[tokio::test]
async fn a_second_resume_without_new_input_does_not_replay_the_old_supplement() {
    let ctx = setup("true", Settings::default()).await;
    // run 1：readiness=false → pending(info_insufficient)
    let mut script = Script::new();
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: false,
            blockers: vec!["还缺部署口径".into()],
        });
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "supp-replay", "p1")
        .await
        .unwrap();
    admit(&ctx, "supp-replay").await;
    ctx.executor.run("supp-replay").await.unwrap();

    // run 2：带补充输入续接，但 readiness 仍 false → 再次 pending
    let mut script = Script::new();
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: false,
            blockers: vec!["还缺并发口径".into()],
        });
    ctx.agent.set_script(script);
    let cursor = ctx
        .store
        .load_live_cursors("supp-replay")
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
        Some("部署在 k8s"),
    )
    .await
    .unwrap();
    ctx.executor.run("supp-replay").await.unwrap();

    // run 3：第二次续接**不带新输入**（文件里还是「部署在 k8s」）
    let cursor = ctx
        .store
        .load_live_cursors("supp-replay")
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let mut script = Script::new();
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    ctx.agent.set_script(script);
    agentpipeline_core::pipeline::resume::apply_action(
        &ctx.store,
        &cursor,
        ResumeAction::Continue,
        None,
        None,
    )
    .await
    .unwrap();
    ctx.executor.run("supp-replay").await.unwrap();

    // 最后一条 run 的首条请求：「部署在 k8s」恰好一次（转录携带的那条），不重放
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
    let mut firsts: Vec<&LlmRequest> = Vec::new();
    for r in &vi {
        let run_id = r.run.as_ref().expect("主 agent 请求须带 run 上下文").run_id;
        if firsts.last().map(|p| p.run.as_ref().unwrap().run_id) != Some(run_id) {
            firsts.push(r);
        }
    }
    let last_first = firsts.last().expect("第三次续接的首条请求");
    let count = last_first
        .messages
        .iter()
        .filter(|m| {
            m.role == agentpipeline_core::agent::client::Role::User
                && m.content.as_deref() == Some("部署在 k8s")
        })
        .count();
    assert_eq!(count, 1, "旧补充不重放：只有转录携带的那一条");
}

/// 决策 280：退化护栏的整环行为——判废那一轮的 run 行带 degraded 标记（复用
/// error 列，API 可检索），并立即按决策 278 续接转录＋错误 turn 重试（此处判废
/// 发生在任何响应之前，故重试起点就是那条错误 turn；流中途判废的形态由
/// production_llm 的真 SSE 流用例钉住）。
#[tokio::test]
async fn a_degenerated_round_is_marked_and_retried_with_the_transcript() {
    let ctx = setup("true", Settings::default()).await;
    let mut script = Script::new();
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .degenerate("片段「Playwright 或」连续重复 150 次")
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    ctx.agent.set_script(script);
    testkit::seed_task(&ctx.store, "degraded", "p1")
        .await
        .unwrap();
    admit(&ctx, "degraded").await;
    ctx.executor.run("degraded").await.unwrap();

    let runs = ctx.store.list_runs("degraded").await.unwrap();
    let vi_runs: Vec<_> = runs
        .iter()
        .filter(|r| {
            r.stage == Stage::ArchitectDesign
                && r.node == Node::ValidateInput
                && r.agent_type == "main"
        })
        .collect();
    assert_eq!(vi_runs.len(), 2, "判废一轮 + 重试成功一轮：{runs:?}");
    assert_eq!(vi_runs[0].status, NodeStatus::Failed);
    assert!(
        vi_runs[0]
            .error
            .as_deref()
            .unwrap_or("")
            .contains("degraded"),
        "run 行的 error 带 degraded 标记（API 可检索）：{:?}",
        vi_runs[0].error
    );
    assert_eq!(vi_runs[1].status, NodeStatus::Success, "重试轮成功");

    // 重试请求从错误 turn 起步（取重试那条 run 的**首条**请求——末条请求已带工具往来）
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
    assert!(vi.len() >= 2, "至少两次请求：{}", vi.len());
    let mut firsts: Vec<&LlmRequest> = Vec::new();
    for r in &vi {
        let run_id = r.run.as_ref().expect("主 agent 请求须带 run 上下文").run_id;
        if firsts.last().map(|p| p.run.as_ref().unwrap().run_id) != Some(run_id) {
            firsts.push(r);
        }
    }
    let retried = firsts.last().expect("重试那条 run 的首条请求");
    let last = retried.messages.last().expect("重试请求带错误 turn");
    assert_eq!(
        last.role,
        agentpipeline_core::agent::client::Role::User,
        "错误 turn 是一条 user 消息：{last:?}"
    );
    let turn = last.content.as_deref().unwrap_or("");
    assert!(turn.contains("degraded"), "错误 turn 引用判废依据：{turn}");
    assert!(turn.contains("submit_metadata"), "明示契约要求：{turn}");
}

/// 必要条件二：续接的 run 打上 `continued_from_run_id`，任务 token 总量不双算。
#[tokio::test]
async fn continued_run_links_back_and_its_tokens_count_as_real_cost() {
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

    // 真实账语义（决策 375）：汇总不再排除被续接的历史 run——那些 token 是模型
    // 真实烧掉的（重喂的转录 provider 照单收费），盲求和就是全量。
    let naive: u64 = runs
        .iter()
        .map(agentpipeline_core::metrics::run_tokens)
        .sum();
    let counted = total_tokens(&runs);
    assert_eq!(
        counted, naive,
        "排除规则已删：counted={counted} naive={naive}"
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
    tokio::time::timeout(Duration::from_secs(30), async {
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
    assert!(unstuck.run_id.unwrap() > 0);
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
    tokio::time::timeout(Duration::from_secs(30), hanging)
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

/// **决策 305（票 04）的第三类真的解得开**：游标 `pending` + run 已终态 + 执行权仍持有
/// → `unstick` 清执行权 + 游标留 pending（可 resume）+ **后续 resume 真的跑起来**。
///
/// 「认出来」与「解得开」是同一份判据的两端：这一条把第二端钉住——否则会出现
/// 「它说卡了、我却解不开」（`unstick` 文件头那句警告）。
#[tokio::test]
async fn unsticking_the_pending_cursor_shape_frees_the_owner_and_resume_runs() {
    use agentpipeline_core::pipeline::unstick::unstick;

    let ctx = setup("true", Settings::default()).await;
    testkit::seed_task(&ctx.store, "t3", "p1").await.unwrap();
    admit(&ctx, "t3").await;
    ctx.store
        .set_task_status("t3", TaskStatus::Running)
        .await
        .unwrap();
    assert!(ctx
        .store
        .try_claim_executor("t3", "executor:dead")
        .await
        .unwrap());
    let cursor = ctx.store.load_live_cursors("t3").await.unwrap()[0].clone();

    // 30 分钟前就终态的 run（远过 unstick 的 10 分钟宽限）+ 游标已挂 pending。
    let run_id = ctx
        .store
        .insert_run(&agentpipeline_core::storage::observability::NewRun {
            task_id: "t3".into(),
            cursor_id: cursor.cursor_id.clone(),
            stage: cursor.stage,
            node: cursor.node,
            attempt: 3,
            agent_type: "main".into(),
            parent_run_id: None,
            prompt_template_hash: None,
            process_group_id: None,
        })
        .await
        .unwrap();
    ctx.store
        .finish_run(
            run_id,
            &agentpipeline_core::storage::observability::RunOutcome {
                status: Some(NodeStatus::Timeout),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let finished =
        agentpipeline_core::clock::Clock::now(&ctx.clock) - chrono::Duration::minutes(30);
    sqlx::query("UPDATE kanban_node_runs SET finished_at = ? WHERE id = ?")
        .bind(agentpipeline_core::storage::ts(finished))
        .bind(run_id)
        .execute(ctx.store.pool())
        .await
        .unwrap();
    let reason = agentpipeline_core::types::PendingReason::new(
        PendingKind::Timeout,
        cursor.stage,
        cursor.node,
        "执行超时（attempt 3）",
    );
    ctx.store
        .set_cursor_pending(&cursor.cursor_id, &reason)
        .await
        .unwrap();
    ctx.store.sync_task_projection("t3").await.unwrap();

    // 前提：旧判据解不开这一格（游标 pending，`is_runnable` 为假）。
    assert_eq!(
        ctx.store
            .get_cursor(&cursor.cursor_id)
            .await
            .unwrap()
            .status,
        CursorStatus::Pending,
        "前提：游标已挂 pending——正是旧两条判据都够不着的那一格"
    );

    let unstuck = unstick(
        &ctx.store,
        &agentpipeline_core::pipeline::executor::force_release,
        "t3",
        agentpipeline_core::clock::Clock::now(&ctx.clock),
        chrono::Duration::minutes(10),
    )
    .await
    .expect("第三类必须解得开");
    assert_eq!(
        unstuck.kind,
        agentpipeline_core::storage::AttentionKind::OwnerStuck
    );
    assert!(
        unstuck.finished_runs.is_empty(),
        "run 早已终态，没有要再标终态的东西"
    );
    assert!(
        ctx.store
            .get_task("t3")
            .await
            .unwrap()
            .executor_owner
            .is_none(),
        "执行权要清掉"
    );
    assert_eq!(
        ctx.store
            .get_cursor(&cursor.cursor_id)
            .await
            .unwrap()
            .status,
        CursorStatus::Pending,
        "解完仍是 pending（等人 resume）——unstick 不替人拍板"
    );

    // 后续 resume 真的跑起来：清 pending → 一个干净执行体取得执行权并落下新 run 行。
    let before = ctx.store.list_runs("t3").await.unwrap().len();
    agentpipeline_core::pipeline::resume::apply_action(
        &ctx.store,
        &ctx.store.get_cursor(&cursor.cursor_id).await.unwrap(),
        ResumeAction::Continue,
        None,
        None,
    )
    .await
    .unwrap();
    let fresh = Executor::new(
        ctx.store.clone(),
        Settings::default(),
        Arc::new(ctx.sse.clone()),
        Arc::new(FakeAgent::new(Script::new())),
        Arc::new(ctx.killer.clone()),
    );
    assert!(
        fresh.try_run("t3").await.unwrap(),
        "unstick 之后新执行体应当取得执行权"
    );
    assert!(
        ctx.store.list_runs("t3").await.unwrap().len() > before,
        "resume 之后真的重跑了"
    );
}

// ─────────────────── 超时重试的三段梯子（决策 320，票 timeout-continuation 01）───────────────────

/// 奇数次调用正常回一个 tool_call，偶数次（每一轮 run 的第二次）永不返回。
///
/// 直接用 `PendingAgent`（第一次调用就挂）会让超时 run 的转录为空——初始 prompt 不在
/// `trace.messages` 里，空转录的续接被 `take_continuation` 的「空转录不续」正确回退。
/// 生产里被判超时的节点几乎总是已经干了活（工具轮、正文轮），这里的形状与之对齐：
/// 每一轮都先真实执行一次工具、再停在一次不返回的调用上。
struct ToolThenStall {
    calls: Arc<AtomicUsize>,
}

impl LlmClient for ToolThenStall {
    fn complete(
        &self,
        _request: LlmRequest,
    ) -> BoxFuture<'static, agentpipeline_core::Result<AgentResponse>> {
        let calls = self.calls.clone();
        Box::pin(async move {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            if n % 2 == 1 {
                std::future::pending::<()>().await;
                unreachable!();
            }
            Ok(AgentResponse {
                content: None,
                tool_calls: vec![agentpipeline_core::agent::client::ToolCall {
                    id: format!("c{n}"),
                    name: "read_file".into(),
                    arguments: r#"{"path":"."}"#.into(),
                }],
                prompt_tokens: 7,
                completion_tokens: 3,
                ..Default::default()
            })
        })
    }
}

/// 同 [`ToolThenStall`]，但把每一次请求抄一份留给调用方——票 04 要断言第 3 档
/// （空白重跑）那一轮的 prompt 形状，这是唯一的取证口。
struct RecordingToolThenStall {
    calls: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<LlmRequest>>>,
}

impl LlmClient for RecordingToolThenStall {
    fn complete(
        &self,
        request: LlmRequest,
    ) -> BoxFuture<'static, agentpipeline_core::Result<AgentResponse>> {
        let calls = self.calls.clone();
        let requests = self.requests.clone();
        Box::pin(async move {
            requests.lock().unwrap().push(request);
            let n = calls.fetch_add(1, Ordering::SeqCst);
            if n % 2 == 1 {
                std::future::pending::<()>().await;
                unreachable!();
            }
            Ok(AgentResponse {
                content: None,
                tool_calls: vec![agentpipeline_core::agent::client::ToolCall {
                    id: format!("c{n}"),
                    name: "read_file".into(),
                    arguments: r#"{"path":"."}"#.into(),
                }],
                prompt_tokens: 7,
                completion_tokens: 3,
                ..Default::default()
            })
        })
    }
}

/// 起一轮执行体、等它先真实跑完一次工具轮、推时钟判超时、等它按中止请求收口——返回该轮 run_id。
///
/// 两条梯子用例共用这一段；`llm_calls` 是跨轮同一个计数（`2 * round` = 该轮 call B 已进场）。
async fn drive_one_timed_out_round(
    ctx: &Ctx,
    scheduler: &KanbanScheduler,
    task_id: &str,
    llm: Arc<dyn LlmClient>,
    llm_calls: &Arc<AtomicUsize>,
    round: usize,
) -> i64 {
    let ex = Arc::new(Executor::new(
        ctx.store.clone(),
        Settings::default(),
        Arc::new(ctx.sse.clone()),
        llm,
        Arc::new(ctx.killer.clone()),
    ));
    let jh = {
        let ex = ex.clone();
        let task_id = task_id.to_string();
        tokio::spawn(async move { ex.run(&task_id).await })
    };
    let run_id = wait_for_running_validate_input(ctx, task_id).await;
    // 等这一轮**真的先干了活**再判超时：中止请求若抢在第一次工具轮写完转录之前落地，
    // 这一轮的转录就是空的，下一轮按「空转录不续」回退成空白起跑——梯子形状就测不真了
    // （实测约 1/3 概率的竞态）。ToolThenStall 每轮 call A 真跑工具、call B 停在不返回
    // 的调用上；计数到 `2 * round` = call B 已进场，此刻转录必然非空、执行体必然停在
    // 可被打断的 await 上。
    tokio::time::timeout(Duration::from_secs(30), async {
        while llm_calls.load(Ordering::SeqCst) < round * 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("每轮应当先真实跑完一次工具轮、停在第二次调用上");
    // 心跳一次都没刷过 → 空闲超时判它终态（ManualClock 手动推进）
    ctx.clock.advance_secs(400);
    let report = scheduler.tick().await.unwrap();
    assert!(
        report.timed_out_runs.contains(&run_id),
        "第 {round} 轮的 run 要被看门狗判超时：{:?}",
        report.timed_out_runs
    );
    // 等执行体按中止请求收口——转录的落库（失败 attempt 照记会话行）在收口之前完成，
    // 下一轮的续接才读得到它。
    tokio::time::timeout(Duration::from_secs(30), jh)
        .await
        .expect("收口的执行体应当在有界时间内退出")
        .unwrap()
        .unwrap();
    run_id
}

/// 等执行体真的跑到 `architect-design.validate_input`（首个 agent 节点）并在跑。
///
/// [`wait_for_a_held_running_run`] 抓任务的第一条 active run——`init.execute`（纯代码
/// 节点）完成得快但**不是零耗时**，起跑的一瞬可能先抓到它；超时续接的梯子只对 agent
/// 节点成立（`take_continuation` 只在 agent 节点入口读），等待必须钉在同一个节点上。
async fn wait_for_running_validate_input(ctx: &Ctx, task_id: &str) -> i64 {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let owned = ctx
                .store
                .get_task(task_id)
                .await
                .unwrap()
                .executor_owner
                .is_some();
            let run = ctx
                .store
                .list_runs_at(task_id, Stage::ArchitectDesign, Node::ValidateInput)
                .await
                .unwrap()
                .into_iter()
                .find(|r| r.status == NodeStatus::Running);
            if owned {
                if let Some(run) = run {
                    return run.id;
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("执行体应当进行到 architect-design.validate_input")
}

/// 超时重试不再一律空白重跑（**显式修订决策 298 的超时支**）：形态由该节点**连续超时
/// 的轮数**决定（写死不配）——连续 1–2 次自动续接上一轮转录（`continued_from_run_id`
/// 落链，与手动「继续」同路）→ 第 3 次降级空白重跑 → 第 4 次起挂起 pending(timeout)
/// 交回人工。
///
/// 全程用真执行体 + `PendingAgent`（每次模型调用都永不返回）：每一轮 run 都停在一次
/// 不返回的调用上、被看门狗判超时、按中止请求收口（转录随之落库）——这正是 2026-09-19
/// 那类现场。断言链：
/// 1. run2 / run3 带 `continued_from_run_id`（指向前一条超时 run）——续接是真的；
/// 2. run4 **不**带链接——空白重跑是真的；
/// 3. run4 超时后游标 pending、原因 kind = timeout——止损交回人工是真的。
#[tokio::test]
async fn timeout_retry_ladder_continues_twice_then_blank_then_pending() {
    let ctx = setup("true", Settings::default()).await;
    testkit::seed_task(&ctx.store, "t-ladder", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-ladder").await;
    let scheduler = KanbanScheduler::new(
        ctx.store.clone(),
        Settings::default(),
        Arc::new(ctx.clock.clone()),
        Arc::new(ctx.killer.clone()),
        Arc::new(ctx.sse.clone()),
        Arc::new(|_| {}),
    );

    let llm_calls = Arc::new(AtomicUsize::new(0));
    let mut prev_run: Option<i64> = None;
    for round in 1..=4 {
        let run_id = drive_one_timed_out_round(
            &ctx,
            &scheduler,
            "t-ladder",
            Arc::new(ToolThenStall {
                calls: llm_calls.clone(),
            }),
            &llm_calls,
            round,
        )
        .await;

        let row = ctx
            .store
            .get_run(run_id)
            .await
            .unwrap()
            .expect("这条 run 应当在台账里");
        match round {
            1 => assert_eq!(row.continued_from_run_id, None, "第一轮是干净起跑"),
            2 | 3 => assert_eq!(
                row.continued_from_run_id, prev_run,
                "第 {round} 轮应当自动续接上一轮（continued_from_run_id 指向被续接的历史 run）"
            ),
            4 => assert_eq!(
                row.continued_from_run_id, None,
                "连续第 3 次超时降级空白重跑：run4 不得带续接链接"
            ),
            _ => unreachable!(),
        }
        prev_run = Some(run_id);
    }

    // run4 超时后（连续第 4 次）：挂起交回人工，不再起任何 run。
    let cursor = ctx.store.load_live_cursors("t-ladder").await.unwrap()[0].clone();
    let after = ctx.store.get_cursor(&cursor.cursor_id).await.unwrap();
    assert_eq!(after.status, CursorStatus::Pending);
    assert_eq!(
        after.pending_reason.as_ref().unwrap().kind,
        PendingKind::Timeout
    );
    let runs = ctx.store.list_runs("t-ladder").await.unwrap();
    assert_eq!(
        runs.iter()
            .filter(|r| r.stage == Stage::ArchitectDesign && r.node == Node::ValidateInput)
            .count(),
        4,
        "止损之后不得再自动起 run（init 的 system run 不算）：{runs:?}"
    );
}

// ─────────────── 106-stability 票 04：超时第 3 档改带简报起跑（决策 376 裁决②）───────────────

/// 超时梯子第 3 档的空白重跑**不再带全卷转录**，改带一份简报——任务描述 + 阶段产物
/// 文件清单 + 未提交改动清单 + 最近收口摘要（决策 376 裁决②）。
///
/// 造法同上面那条梯子用例（真执行体 + 每轮先干一次工具轮再停在第二次调用上），
/// 第 4 轮起跑前在 worktree 里留一处「已改好、未提交」的改动——事故里那份
/// `ux-audit.spec.ts` 修复正是这个形状（只活在 worktree 里，差点随重跑被无视）。
/// 断言：
/// 1. 第 4 轮**首个**请求不带任何转录消息（`messages` 为空）——空白重跑真的不带全卷；
/// 2. 它的 user prompt 里简报四要素齐备，未提交改动点得出文件名；
/// 3. 对照：前面几档仍是全卷转录续接——只有第 3 档换了形态（决策 376 的边界）。
#[tokio::test]
async fn the_blank_restart_round_starts_from_a_brief_not_the_transcript() {
    const BRIEF_TITLE: &str = "## 续接简报（超时空白重跑，不带全卷转录）";
    let ctx = setup("true", Settings::default()).await;
    // 描述非空（票 05 的空白描述闸门拦得住；这里要的是「简报把描述带上了」）。
    let mut new_task =
        agentpipeline_core::storage::tasks::NewTask::new("t-brief", "任务 t-brief", "p1");
    new_task.description = "把走查清单落到任务目录，逐条补实，别攒在转录里。".into();
    ctx.store.create_task(&new_task).await.unwrap();
    admit(&ctx, "t-brief").await;
    let scheduler = KanbanScheduler::new(
        ctx.store.clone(),
        Settings::default(),
        Arc::new(ctx.clock.clone()),
        Arc::new(ctx.killer.clone()),
        Arc::new(ctx.sse.clone()),
        Arc::new(|_| {}),
    );

    let worktree = ctx.store.home().worktree_path("t-brief");
    let requests: Arc<Mutex<Vec<LlmRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let llm_calls = Arc::new(AtomicUsize::new(0));
    for round in 1..=4 {
        if round == 4 {
            // 第 3 档起跑前，worktree 里留两处「上一轮的进度」：一处已改好未提交的改动、
            // 一处任务自身产物目录（事故里 `.scratch/ux-audit-3/` 与那份 e2e 修复的形状）。
            std::fs::create_dir_all(worktree.join(".scratch/brief-fixture")).unwrap();
            std::fs::write(
                worktree.join(".scratch/brief-fixture/notes.md"),
                "走查清单（上一轮已落盘）\n",
            )
            .unwrap();
            std::fs::write(worktree.join("STALE_FIX.txt"), "已改好，未提交\n").unwrap();
        }
        drive_one_timed_out_round(
            &ctx,
            &scheduler,
            "t-brief",
            Arc::new(RecordingToolThenStall {
                calls: llm_calls.clone(),
                requests: requests.clone(),
            }),
            &llm_calls,
            round,
        )
        .await;
    }

    let reqs = requests.lock().unwrap();
    // 第 4 轮首个请求：user prompt 的段在 plan 里冻着，故该轮两个请求都带简报标题；
    // `find` 取到的是**先落下的那个**——它才是这次 attempt 的起跑请求。
    let brief = reqs
        .iter()
        .find(|r| r.user_prompt.contains(BRIEF_TITLE))
        .expect("第 3 档（空白重跑）起跑那一轮应当带简报");
    assert!(
        brief.messages.is_empty(),
        "空白重跑的首个请求不带全卷转录，得到 {} 条：{:?}",
        brief.messages.len(),
        brief.messages
    );
    assert!(brief.user_prompt.contains("## 任务描述"), "简报含任务描述");
    assert!(brief.user_prompt.contains("任务 t-brief"));
    assert!(
        brief.user_prompt.contains("把走查清单落到任务目录"),
        "简报带上任务描述正文：{}",
        brief.user_prompt
    );
    assert!(
        brief.user_prompt.contains("## 阶段产物文件清单"),
        "简报含阶段产物文件清单"
    );
    assert!(
        brief
            .user_prompt
            .contains(".scratch/brief-fixture/notes.md"),
        "产物清单看得出工作区 `.scratch/` 的现状（票面点名的位置）：{}",
        brief.user_prompt
    );
    assert!(
        brief.user_prompt.contains("## 未提交改动清单"),
        "简报含未提交改动清单"
    );
    assert!(
        brief.user_prompt.contains("STALE_FIX.txt"),
        "未提交改动点得出文件名（事故的直接教训）：{}",
        brief.user_prompt
    );
    assert!(
        brief.user_prompt.contains("## 最近收口摘要"),
        "简报含最近收口摘要"
    );
    // 摘要说的是**上一轮已收口**的 run（attempt 3 超时），不是起跑这一轮自己
    // （attempt 4 此刻还是 running）——「running，耗时 0 秒」是当下，不是历史。
    assert!(
        brief.user_prompt.contains("上一轮 attempt 3") && brief.user_prompt.contains("timeout"),
        "收口摘要须取最后一条**非 running** 的 run：{}",
        brief.user_prompt
    );
    // 对照：第 1–2 档仍带全卷转录——存在「没有简报标题、但 messages 非空」的请求。
    assert!(
        reqs.iter()
            .any(|r| !r.user_prompt.contains(BRIEF_TITLE) && !r.messages.is_empty()),
        "第 1–2 档仍是全卷转录续接（决策 320 / 376 的边界）"
    );
}

// ─────────────────────────── 票 04：sync-check fail-closed ───────────────────────────

/// 设计阶段的最小脚本：三处 execute 的元数据由调用方以**原始参数串**给出。
///
/// 走 `submit_metadata_raw` 而不是类型化 `submit`，是因为本票要造的是**键不在**的形状
/// （`{"readiness": true}`）——类型化序列化永远会把 `acceptance_criteria: []` 发出去，
/// 表达不了"被掏空"；而现场（`kanban_stage_outputs.metadata_json`）正是缺键的那个样子。
fn design_scripts_with_raw_execute(script: &mut Script, arch_execute: &str, test_execute: &str) {
    for (stage, node) in [
        (Stage::ArchitectDesign, Node::ValidateInput),
        (Stage::DevelopDesign, Node::ValidateInput),
        (Stage::TestDesign, Node::ValidateInput),
    ] {
        script.for_node(stage, node).submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    }
    for (stage, node) in [
        (Stage::ArchitectDesign, Node::ValidateOutput),
        (Stage::DevelopDesign, Node::ValidateOutput),
        (Stage::TestDesign, Node::ValidateOutput),
    ] {
        script
            .for_node(stage, node)
            .submit(&ValidateOutputMetadata {
                passed: true,
                ..Default::default()
            });
    }
    script
        .for_node(Stage::ArchitectDesign, Node::Execute)
        .write_file("design.md", "# 设计\n")
        .submit_metadata_raw(arch_execute);
    // develop-design 的字段闸门不消费，与事故同形即可
    script
        .for_node(Stage::DevelopDesign, Node::Execute)
        .write_file("dev-plan.md", "# 开发计划\n")
        .submit_metadata_raw(r#"{"readiness": true}"#);
    script
        .for_node(Stage::TestDesign, Node::Execute)
        .write_file("test-scenarios.md", "# 测试场景\n")
        .submit_metadata_raw(test_execute);
}

/// 2026-10-01 事故的等价 fixture：设计元数据被掏空到 `{"readiness": true}`，闸门必须拦下。
///
/// 现场那一轮 `sync-check` 判的是 `Proceed`——`test_scenarios` / `acceptance_criteria`
/// 两个键不在，引用完整性校验（决策 136）整段进不去，而 `readiness` 又恰好都在，
/// 于是残缺被当成合法。真库副本在 106 上（本地没有这条任务），这里用等价 fixture；
/// 判据本身是纯函数（`sync_metadata_gaps`），fixture 与现场差在数据来源、不在形状。
#[tokio::test]
async fn degraded_stage_metadata_blocks_the_sync_gate() {
    let ctx = setup("true", Settings::default()).await;
    let mut script = Script::new();
    design_scripts_with_raw_execute(
        &mut script,
        r#"{"readiness": true}"#,
        r#"{"readiness": true}"#,
    );
    ctx.agent.set_script(script);

    testkit::seed_task(&ctx.store, "t-gap", "p1").await.unwrap();
    admit(&ctx, "t-gap").await;

    // 回溯后脚本耗尽、流水线会停住——本用例只关心闸门那一刻，不要求这一趟跑成功
    let _ = ctx.executor.run("t-gap").await;

    let decision = ctx
        .store
        .stage_output_metadata("t-gap", Stage::SyncCheck, "sync_decision")
        .await
        .unwrap()
        .expect("闸门应当落了 sync-decision");
    assert_eq!(decision["decision"], "backtrack", "{decision}");
    let gaps: Vec<String> = decision["metadata_gaps"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|g| g.as_str().map(String::from))
        .collect();
    assert!(
        gaps.iter().any(|g| g.contains("acceptance_criteria")),
        "缺项要点名 acceptance_criteria：{gaps:?}"
    );
    assert!(
        gaps.iter().any(|g| g.contains("test_scenarios")),
        "缺项要点名 test_scenarios：{gaps:?}"
    );

    // 缺项随 backtrack-feedback.md 一起回到设计阶段（决策 126 的同一通道）
    let feedback =
        std::fs::read_to_string(ctx._home.home().task_file("t-gap", "backtrack-feedback.md"))
            .unwrap();
    assert!(feedback.contains("元数据缺项"), "{feedback}");
    assert!(feedback.contains("test_scenarios"), "{feedback}");
}

/// 元数据齐全、但 high 场景的 `design_refs` 指向不存在的 AC → 照旧拦下（决策 136 不回归）。
///
/// 这条钉的是本票的**反面**：fail-closed 只添一道"键在不在"的判定，
/// 不许把既有的引用完整性校验挤掉、也不许因为加了缺项判据就漏判悬空引用。
#[tokio::test]
async fn intact_metadata_with_a_dangling_ref_still_blocks() {
    let ctx = setup("true", Settings::default()).await;
    let arch = serde_json::to_string(&ArchitectExecuteMetadata {
        readiness: true,
        acceptance_criteria: vec![AcceptanceCriterion {
            id: "AC-1".into(),
            description: "能登录".into(),
        }],
        ..Default::default()
    })
    .unwrap();
    // high 场景引用 AC-9：AC-1 是唯一存在的验收标准
    let test = serde_json::to_string(&TestDesignMetadata {
        readiness: true,
        test_scenarios: vec![TestScenario {
            id: "S-1".into(),
            name: "登录成功".into(),
            description: "登录".into(),
            preconditions: vec![],
            steps: vec![],
            expected_result: "成功".into(),
            priority: agentpipeline_core::types::ScenarioPriority::High,
            design_refs: vec!["AC-9".into()],
        }],
        ..Default::default()
    })
    .unwrap();

    let mut script = Script::new();
    design_scripts_with_raw_execute(&mut script, &arch, &test);
    ctx.agent.set_script(script);

    testkit::seed_task(&ctx.store, "t-ref", "p1").await.unwrap();
    admit(&ctx, "t-ref").await;
    let _ = ctx.executor.run("t-ref").await;

    let decision = ctx
        .store
        .stage_output_metadata("t-ref", Stage::SyncCheck, "sync_decision")
        .await
        .unwrap()
        .expect("闸门应当落了 sync-decision");
    assert_eq!(decision["decision"], "backtrack", "{decision}");
    // 空清单会被 `skip_serializing_if` 省掉——键不在与空数组都算「没有缺项」
    assert!(
        decision["metadata_gaps"]
            .as_array()
            .is_none_or(|gaps| gaps.is_empty()),
        "元数据齐全，缺项清单必须为空：{decision}"
    );
    let blockers: Vec<&str> = decision["test_blockers"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|b| b.as_str())
        .collect();
    assert!(
        blockers
            .iter()
            .any(|b| b.contains("悬空") || b.contains("缺失")),
        "悬空引用要走 test_blockers：{blockers:?}"
    );
}

// ─────────────────────────── 票 05：prompt cache 的记账链路 ───────────────────────────

/// 「第二轮起命中」的每轮命中量（取一个一眼能认出来的数）。
const CACHE_READ_PER_HIT: u32 = 4096;

/// 模拟 provider 前缀缓存的替身：同一 `(stage, node)` 的第 1 次调用冷启动（未命中），
/// 此后每次报同一份命中读数。
///
/// 为什么是**模拟**而非测量：真实命中率由 provider 决定（106 的历史账单里整站命中
/// 95.7%，见 `.scratch/106-stability/cache-findings.md`），替身这一侧的活是把
/// 「响应里的缓存读数一路落到运行台账」这条记账链路钉住。
///
/// 三步走完一个节点：跑一次工具（建立非空转录）→ 交卷 → 一段**不带 tool_call** 的正文
/// （轮循环只在「这一轮没有任何工具调用」时收口，见 `model_invoke`）。
/// 只参与 architect-design.validate_input；其余节点一律给抽不出元数据的文本。
struct PrefixCachingClient {
    calls: Arc<AtomicUsize>,
}

/// 走完一个节点需要的模型调用次数（冷启动 + 交卷 + 收口）。
const CALLS_ONE_NODE: u32 = 3;

impl LlmClient for PrefixCachingClient {
    fn complete(
        &self,
        request: LlmRequest,
    ) -> BoxFuture<'static, agentpipeline_core::Result<AgentResponse>> {
        if request.stage != Stage::ArchitectDesign || request.node != Node::ValidateInput {
            return Box::pin(async move {
                Ok(AgentResponse {
                    content: Some("（替身只参与 validate_input）".into()),
                    ..Default::default()
                })
            });
        }
        let calls = self.calls.clone();
        Box::pin(async move {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            // 第 1 次冷启动（未命中），此后每次命中。
            let hit = if n == 0 { 0 } else { CACHE_READ_PER_HIT };
            let call = |id: String, name: &str, arguments: String| {
                agentpipeline_core::agent::client::ToolCall {
                    id,
                    name: name.into(),
                    arguments,
                }
            };
            let tool_calls = match n {
                0 => vec![call("c0".into(), "list_dir", r#"{"path":"."}"#.into())],
                1 => vec![call(
                    "c1".into(),
                    "submit_metadata",
                    serde_json::to_string(&ValidateInputMetadata {
                        readiness: true,
                        blockers: vec![],
                    })
                    .unwrap(),
                )],
                // 收口那一轮：没有工具调用，轮循环到此为止（元数据已在上一轮交过）。
                _ => Vec::new(),
            };
            Ok(AgentResponse {
                content: (n >= 2).then(|| "交卷完事".to_string()),
                tool_calls,
                prompt_tokens: 100,
                completion_tokens: 5,
                cache_read_tokens: hit,
                cache_write_tokens: 0,
                ..Default::default()
            })
        })
    }
}

#[tokio::test]
async fn provider_cache_readings_land_on_the_run_row() {
    // 票 05：缓存命中不只是 provider 的账——它必须一路落到运行台账（响应 → RunTokens →
    // run 行）。详情页与滚动投影（决策 375）读的都是这一份，断了就没人看得见命中。
    //
    // 只等这一条 run 收口就中止执行体（本仓「只看一个节点」的既有形状）：替身不参与别的
    // 节点，让它把整条流水线跑完没有意义——architect-design.execute 拿不到元数据会触发
    // 回溯，那是另一件事的现场。
    let settings = Settings {
        agent_retry_max: 1,
        ..Default::default()
    };
    let ctx = setup("true", settings.clone()).await;
    testkit::seed_task(&ctx.store, "t-cache", "p1")
        .await
        .unwrap();
    admit(&ctx, "t-cache").await;

    let calls = Arc::new(AtomicUsize::new(0));
    let client: Arc<dyn LlmClient> = Arc::new(PrefixCachingClient {
        calls: calls.clone(),
    });
    let (store, sse, killer) = (
        ctx.store.clone(),
        Arc::new(ctx.sse.clone()),
        Arc::new(ctx.killer.clone()),
    );
    let jh = tokio::spawn(async move {
        Executor::new(store, settings, sse, client, killer)
            .run("t-cache")
            .await
    });

    let settled = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let settled = ctx
                .store
                .list_runs_at("t-cache", Stage::ArchitectDesign, Node::ValidateInput)
                .await
                .unwrap()
                .into_iter()
                .find(|r| r.status != NodeStatus::Running);
            if let Some(run) = settled {
                return run;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    let run = match settled {
        Ok(run) => run,
        Err(_) => {
            let runs = ctx
                .store
                .list_runs_at("t-cache", Stage::ArchitectDesign, Node::ValidateInput)
                .await
                .unwrap();
            let task = ctx.store.get_task("t-cache").await.unwrap();
            panic!(
                "30s 内未收口：runs={:?} task_status={:?} owner={:?} llm_calls={}",
                runs.iter()
                    .map(|r| (r.id, r.attempt, r.status, r.error.clone()))
                    .collect::<Vec<_>>(),
                task.status,
                task.executor_owner,
                calls.load(Ordering::SeqCst),
            );
        }
    };
    jh.abort();

    assert_eq!(run.status, NodeStatus::Success, "交卷即收口");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        CALLS_ONE_NODE as usize,
        "冷启动 + 交卷 + 收口三轮"
    );
    assert_eq!(
        run.cache_read_tokens,
        CACHE_READ_PER_HIT * (CALLS_ONE_NODE - 1),
        "第二轮起报的命中读数要原样落 run 行（冷启动那轮不算，也不能丢）"
    );
    assert_eq!(
        run.prompt_tokens,
        100 * CALLS_ONE_NODE,
        "每轮各 100 prompt token"
    );
}

// ────────── 节点内消息日志（`.scratch/node-message-resume` 票 01）──────────

/// 回一个会**挂住的工具**：`run_command` 跑一条睡得够久的命令，于是「工具在跑、结果还没
/// 回来」这个窗口有几十秒宽——足够读库取证。第二次调用永不返回（测试结束即 abort）。
#[derive(Default)]
struct HangOnACommandTool {
    calls: AtomicUsize,
}

impl LlmClient for HangOnACommandTool {
    fn complete(
        &self,
        _request: LlmRequest,
    ) -> BoxFuture<'static, agentpipeline_core::Result<AgentResponse>> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            if n == 0 {
                return Ok(AgentResponse {
                    content: None,
                    tool_calls: vec![agentpipeline_core::agent::client::ToolCall {
                        id: "c-sleep".into(),
                        name: "run_command".into(),
                        arguments: r#"{"command":"sleep 20"}"#.into(),
                    }],
                    prompt_tokens: 7,
                    completion_tokens: 3,
                    ..Default::default()
                });
            }
            std::future::pending::<()>().await;
            unreachable!()
        })
    }
}

/// 票 01 的**承重顺序**：assistant 响应在**这一批工具的第一个执行之前**已经提交入库。
///
/// 缺了这条顺序，崩溃会连模型刚说的话一起丢——日志里只剩半截工具结果，没有任何东西说
/// 它们是为了什么而跑的，那一轮只能整段作废（而这正是本功能要修的「节点内没有任何
/// checkpoint」）。断言直接用「工具挂住的那一刻读库」取证：assistant 行在，tool 行不在。
#[tokio::test]
async fn the_assistant_row_is_committed_before_its_first_tool_runs() {
    let ctx = setup("true", Settings::default()).await;
    testkit::seed_task(&ctx.store, "t-log", "p1").await.unwrap();
    admit(&ctx, "t-log").await;

    let ex = Arc::new(Executor::new(
        ctx.store.clone(),
        Settings::default(),
        Arc::new(ctx.sse.clone()),
        Arc::new(HangOnACommandTool::default()),
        Arc::new(ctx.killer.clone()),
    ));
    let jh = {
        let ex = ex.clone();
        tokio::spawn(async move { ex.run("t-log").await })
    };

    let run_id = wait_for_running_validate_input(&ctx, "t-log").await;
    let deadline = Instant::now() + Duration::from_secs(30);
    while ctx.store.count_node_messages(run_id).await.unwrap() == 0 {
        assert!(
            Instant::now() < deadline,
            "assistant 行没在 30s 内落库——工具开跑之前就该落"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // 工具还挂在 `sleep` 里：这一刻库里**只该有**那条 assistant。
    assert_eq!(
        ctx.store.count_node_messages(run_id).await.unwrap(),
        1,
        "工具结果还没回来，它那一行不该存在"
    );
    let got = ctx
        .store
        .latest_own_transcript("t-log", Stage::ArchitectDesign, Node::ValidateInput)
        .await
        .unwrap()
        .expect("日志里有行");
    assert_eq!(got.run_id, run_id);
    assert_eq!(got.messages.len(), 1);
    let Role::Assistant = got.messages[0].role else {
        panic!("第一条应当是 assistant 响应：{:?}", got.messages[0]);
    };
    assert_eq!(got.messages[0].tool_calls.len(), 1);
    assert_eq!(got.messages[0].tool_calls[0].name, "run_command");
    assert_eq!(
        got.messages[0].tool_calls[0].arguments, r#"{"command":"sleep 20"}"#,
        "工具调用的参数是**原始串**，不做二次序列化"
    );

    jh.abort();
}

/// 票 01：日志记的是**原始转录**——压缩改写内存里那一份（丢最旧的消息、换成摘要）之后，
/// 日志行**仍含**被压掉的那些消息（它只追加，没有压缩事件可记）。
#[tokio::test]
async fn the_log_keeps_messages_that_compaction_rewrites_away() {
    let ctx = setup("true", Settings::default()).await;
    testkit::seed_task(&ctx.store, "t-raw", "p1").await.unwrap();
    let cursor = ctx.store.load_live_cursors("t-raw").await.unwrap()[0].clone();
    ctx.store
        .set_cursor_stage(
            &cursor.cursor_id,
            Stage::ArchitectDesign,
            Node::ValidateInput,
        )
        .await
        .unwrap();
    let run_id = ctx
        .store
        .insert_run(&agentpipeline_core::storage::observability::NewRun {
            task_id: "t-raw".into(),
            cursor_id: cursor.cursor_id.clone(),
            stage: Stage::ArchitectDesign,
            node: Node::ValidateInput,
            attempt: 1,
            agent_type: "main".into(),
            parent_run_id: None,
            prompt_template_hash: None,
            process_group_id: None,
        })
        .await
        .unwrap();

    // 起跑时承接一份足够长的前缀（模拟「跑了很久的节点」）。
    let prefix: Vec<agentpipeline_core::agent::client::Message> = (0..40)
        .map(|i| {
            if i % 2 == 0 {
                agentpipeline_core::agent::client::Message::user(format!("第 {i} 问"))
            } else {
                agentpipeline_core::agent::client::Message::assistant(
                    Some(format!("第 {i} 答")),
                    Vec::new(),
                )
            }
        })
        .collect();
    ctx.store
        .append_node_messages(
            "t-raw",
            run_id,
            Stage::ArchitectDesign,
            Node::ValidateInput,
            agentpipeline_core::storage::AGENT_TYPE_MAIN,
            0,
            &prefix,
            None,
        )
        .await
        .unwrap();

    // 压缩改写的是内存里那一份（这里直接调压缩器，等价于 `check_budget` 触发的那一次）。
    let settings = Settings::default();
    let outcome = agentpipeline_core::agent::context::compact_messages_from(
        &prefix,
        settings.keep_recent_rounds,
        0,
    );
    let in_memory = outcome.messages.clone();
    assert!(
        outcome.compacted_messages > 0,
        "这份前缀应当真被压掉一些（否则本用例证明不了任何事）"
    );
    assert!(
        in_memory.len() < prefix.len(),
        "内存里那一份变短了：压缩真的改写了它"
    );

    // 日志一行不动：读出来还是原来那 40 条。
    let got = ctx
        .store
        .latest_own_transcript("t-raw", Stage::ArchitectDesign, Node::ValidateInput)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.messages, prefix, "日志是原始转录：压缩一个字都不许碰它");
}

// ────── 重启连击止损（`.scratch/node-message-resume` 票 04）──────

/// 同一个节点被进程重启连续打断到上限之后，**不再重放同一份转录**，转 pending 交回人工。
///
/// 不这样做的后果是死循环：节点每次跑到同一处就把服务搞崩（OOM / 不返回的调用），
/// 重启恢复重放同一份转录 → 再崩。超时梯子是为同一类「续接救不回来」造的（决策 320），
/// 但它够不着这一种——服务在判超时之前就已经死了。
///
/// 断言三件：① 不再起新的 run；② 游标挂 `retry_exhausted` 交回人工；③ 超时梯子的口径
/// 一点没被搅动（重启对它照旧既不计数也不清零）。
#[tokio::test]
async fn three_restarts_stop_the_replay_loop_and_hand_back_to_a_human() {
    use agentpipeline_core::storage::observability::{NewRun, RunOutcome, CANCEL_ORIGIN_RESTART};
    const STAGE: Stage = Stage::ArchitectDesign;
    const NODE: Node = Node::ValidateInput;

    let ctx = setup("true", Settings::default()).await;
    testkit::seed_task(&ctx.store, "t-stop", "p1")
        .await
        .unwrap();
    let cursor = ctx.store.load_live_cursors("t-stop").await.unwrap()[0].clone();
    ctx.store
        .set_cursor_stage(&cursor.cursor_id, STAGE, NODE)
        .await
        .unwrap();

    // 三条「进程退出时还在跑」的收尾——启动恢复写下的正是这一形态（票 02② 的语义）。
    for attempt in 1..=3 {
        let run_id = ctx
            .store
            .insert_run(&NewRun {
                task_id: "t-stop".into(),
                cursor_id: cursor.cursor_id.clone(),
                stage: STAGE,
                node: NODE,
                attempt,
                agent_type: "main".into(),
                parent_run_id: None,
                prompt_template_hash: None,
                process_group_id: None,
            })
            .await
            .unwrap();
        ctx.store
            .finish_run(
                run_id,
                &RunOutcome {
                    status: Some(NodeStatus::Cancelled),
                    error: Some("进程重启：这一轮在上一进程退出时还在跑，标终态".into()),
                    cancel_origin: Some(CANCEL_ORIGIN_RESTART),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }
    // 启动恢复给中断的游标置的续接原因（票 03）。
    ctx.store
        .mark_cursor_continuation(
            &cursor.cursor_id,
            agentpipeline_core::types::ResumeCause::ProcessRestart,
        )
        .await
        .unwrap();

    admit(&ctx, "t-stop").await;
    ctx.executor.run("t-stop").await.unwrap();

    let after = ctx.store.get_cursor(&cursor.cursor_id).await.unwrap();
    assert_eq!(
        after.status,
        CursorStatus::Pending,
        "止损的意思是「这一轮不要跑」：转 pending 交回人工"
    );
    assert_eq!(
        after.pending_reason.as_ref().unwrap().kind,
        PendingKind::RetryExhausted
    );
    assert!(
        after
            .pending_reason
            .as_ref()
            .unwrap()
            .message
            .contains("进程重启"),
        "报文要说清是哪一类反复：{:?}",
        after.pending_reason
    );
    assert_eq!(
        ctx.store
            .list_runs_at("t-stop", STAGE, NODE)
            .await
            .unwrap()
            .len(),
        3,
        "止损之后不得再起 run（起了就是把同一份转录又重放一遍）"
    );
    assert_eq!(
        ctx.store
            .trailing_timeout_streak("t-stop", STAGE, NODE)
            .await
            .unwrap(),
        0,
        "超时梯子的口径不变：重启既不升档也不清零"
    );
}

// ── 票 02：一次**普通恢复**不许凭空注入 turn（判据从「承接转录非空」换成按续接原因）──

/// 记录每一次请求、回一段无可解析元数据的正文（节点会失败重试，但请求已经留下）。
#[derive(Default)]
struct RecordingProse {
    requests: Arc<Mutex<Vec<LlmRequest>>>,
}

impl LlmClient for RecordingProse {
    fn complete(
        &self,
        request: LlmRequest,
    ) -> BoxFuture<'static, agentpipeline_core::Result<AgentResponse>> {
        let requests = self.requests.clone();
        Box::pin(async move {
            requests.lock().unwrap().push(request);
            Ok(AgentResponse {
                content: Some("这一轮没交卷。".into()),
                prompt_tokens: 3,
                completion_tokens: 2,
                ..Default::default()
            })
        })
    }
}

/// 起一个节点、把日志与续接原因备好、跑一轮执行体，返回它发出去的全部请求。
async fn requests_for_a_resumed_node(
    task_id: &str,
    stage: Stage,
    node: Node,
    seed: impl FnOnce(&Ctx, &str) -> BoxFuture<'static, ()>,
    with_sleep: impl FnOnce(&Ctx, &str) -> BoxFuture<'static, ()>,
) -> Vec<LlmRequest> {
    let ctx = setup("true", Settings::default()).await;
    testkit::seed_task(&ctx.store, task_id, "p1").await.unwrap();
    let cursor = ctx.store.load_live_cursors(task_id).await.unwrap()[0].clone();
    ctx.store
        .set_cursor_stage(&cursor.cursor_id, stage, node)
        .await
        .unwrap();
    // 一条 run + 一条消息行（续接素材），再置上「进程重启」。
    let run_id = ctx
        .store
        .insert_run(&agentpipeline_core::storage::observability::NewRun {
            task_id: task_id.into(),
            cursor_id: cursor.cursor_id.clone(),
            stage,
            node,
            attempt: 1,
            agent_type: "main".into(),
            parent_run_id: None,
            prompt_template_hash: None,
            process_group_id: None,
        })
        .await
        .unwrap();
    ctx.store
        .append_node_messages(
            task_id,
            run_id,
            stage,
            node,
            agentpipeline_core::storage::AGENT_TYPE_MAIN,
            0,
            &[
                agentpipeline_core::agent::client::Message::user("上一轮的提问"),
                agentpipeline_core::agent::client::Message::assistant(
                    Some("上一轮的回答".into()),
                    Vec::new(),
                ),
            ],
            None,
        )
        .await
        .unwrap();
    seed(&ctx, task_id).await;
    ctx.store
        .mark_cursor_continuation(
            &cursor.cursor_id,
            agentpipeline_core::types::ResumeCause::ProcessRestart,
        )
        .await
        .unwrap();
    with_sleep(&ctx, task_id).await;

    let requests = Arc::new(Mutex::new(Vec::new()));
    let ex = Executor::new(
        ctx.store.clone(),
        Settings::default(),
        Arc::new(ctx.sse.clone()),
        Arc::new(RecordingProse {
            requests: requests.clone(),
        }),
        Arc::new(ctx.killer.clone()),
    );
    admit(&ctx, task_id).await;
    // 节点本身会失败（正文没有元数据），但请求已经留下——本用例只读请求。
    let _ = ex.run(task_id).await;
    let out = requests.lock().unwrap().clone();
    assert!(!out.is_empty(), "执行体应当至少发过一次请求");
    out
}

/// 决策 387 的 turn 注入**按续接原因判**（票 02）：一次普通的进程重启恢复**不许**在
/// develop.execute 上凭空插一条「评审打回反馈」——人根本没打回过任何东西。
///
/// 判据原来挂在「承接的转录非空」上，那在会话表当源时够用（没有 recover 就没有非空转录）；
/// 日志源接通之后**任何**一次恢复都会让转录非空，不换判据就会把没发生过的评审当作事实
/// 喂给模型。
#[tokio::test]
async fn an_ordinary_restart_does_not_inject_the_review_rework_turn() {
    use agentpipeline_core::types::{ReviewRequiredChange, ReviewResult};
    let requests = requests_for_a_resumed_node(
        "t-rework",
        Stage::Develop,
        Node::Execute,
        |ctx, task_id| {
            let task_id = task_id.to_string();
            let store = ctx.store.clone();
            Box::pin(async move {
                store
                    .upsert_stage_output(
                        &task_id,
                        Stage::Review,
                        "review_report",
                        "review-report.md",
                        Some(
                            &serde_json::to_value(ReviewResult {
                                approved: false,
                                review_report_path: Some("review-report.md".into()),
                                required_changes: vec![ReviewRequiredChange {
                                    path: "src/lib.rs".into(),
                                    action: FileAction::Modify,
                                    finding: Some("边界没处理".into()),
                                }],
                            })
                            .unwrap(),
                        ),
                    )
                    .await
                    .unwrap();
            })
        },
        |_ctx, _task_id| Box::pin(async {}),
    )
    .await;
    let last = requests.last().unwrap();
    assert!(
        !last
            .messages
            .iter()
            .any(|m| m.content.as_deref().is_some_and(
                |c| c.starts_with(agentpipeline_core::types::REVIEW_REWORK_TURN_PREFIX)
            )),
        "普通重启不是评审打回：不许注入那条 turn（这正是「按原因判而非按非空判」要防的）"
    );
    assert!(
        last.messages
            .iter()
            .any(|m| m.content.as_deref() == Some("上一轮的回答")),
        "承接的转录本身照旧带上"
    );
}

/// 决策 279 的补充输入 turn 注入**按续接原因判**（票 02）：一次普通的进程重启恢复不许把
/// `user-input.md` 当作「人刚补充的输入」塞进转录——那是另一次信息不足打回留下的旧档。
#[tokio::test]
async fn an_ordinary_restart_does_not_inject_the_supplement_input_turn() {
    let requests = requests_for_a_resumed_node(
        "t-supp",
        Stage::ArchitectDesign,
        Node::ValidateInput,
        |ctx, task_id| {
            let task_id = task_id.to_string();
            let home = ctx.store.home().clone();
            Box::pin(async move {
                home.ensure_task_dirs(&task_id).unwrap();
                std::fs::write(
                    home.task_file(&task_id, "user-input.md"),
                    "# 用户补充输入\n\n请把登录也覆盖上。\n",
                )
                .unwrap();
            })
        },
        |_ctx, _task_id| Box::pin(async {}),
    )
    .await;
    let last = requests.last().unwrap();
    assert!(
        !last
            .messages
            .iter()
            .any(|m| m.content.as_deref() == Some("请把登录也覆盖上。")),
        "普通重启不是「信息不足被打回」：不许把旧档当作人刚补充的输入塞进转录"
    );
}

/// 票 01：工具**失败也落行**——「这个调用跑过、以失败收场」与「它根本没跑」是两件事，
/// 续接时同理（模型看到的应当是那条失败回执，而不是调用凭空消失）。
#[tokio::test]
async fn a_failed_tool_is_logged_too() {
    let ctx = setup("true", Settings::default()).await;
    testkit::seed_task(&ctx.store, "t-fail", "p1")
        .await
        .unwrap();
    let cursor = ctx.store.load_live_cursors("t-fail").await.unwrap()[0].clone();
    ctx.store
        .set_cursor_stage(
            &cursor.cursor_id,
            Stage::ArchitectDesign,
            Node::ValidateInput,
        )
        .await
        .unwrap();

    let mut script = Script::new();
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .failing_tool("no_such_tool", serde_json::json!({}))
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    ctx.agent.set_script(script);
    admit(&ctx, "t-fail").await;
    // 节点本身跑成什么样不是本用例的事（失败回执会被回灌、模型的下一步是交卷）。
    let _ = ctx.executor.run("t-fail").await;

    let got = ctx
        .store
        .latest_own_transcript("t-fail", Stage::ArchitectDesign, Node::ValidateInput)
        .await
        .unwrap()
        .expect("日志里有行");
    let failure = got
        .messages
        .iter()
        .find(|m| {
            m.role == Role::Tool
                && m.content
                    .as_deref()
                    .is_some_and(|c| c.starts_with("工具执行失败："))
        })
        .unwrap_or_else(|| panic!("工具失败也要落一行（内容是失败文本）：{:?}", got.messages));
    let declared = got
        .messages
        .iter()
        .flat_map(|m| m.tool_calls.iter())
        .find(|c| c.name == "no_such_tool")
        .expect("assistant 声明过这次调用");
    assert_eq!(
        failure.tool_call_id.as_deref(),
        Some(declared.id.as_str()),
        "回执要指回它回应的是哪一次调用"
    );
    assert_eq!(failure.name.as_deref(), Some("no_such_tool"));
}
