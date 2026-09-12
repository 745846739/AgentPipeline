//! join / skip 场景（testing.md §8：E2E-02 / E2E-11 / E2E-12 尾段）。
//!
//! FakeAgent 驱动完整流水线到 sync-check 汇聚点，覆盖：
//! - **E2E-02** sync-check backtrack：双游标归档 → main 指 architect.validate_input、
//!   设计文档标过期（决策 83）、`backtrack-feedback.md` 写入（决策 126）、
//!   重入 prompt 含反馈段、attempts 归零（决策 43）；
//! - **E2E-11** skip 落点矩阵：architect skip → 分裂；develop-design / test-design
//!   skip → `waiting_join` + `skipped_to_join`，sync-check 视 readiness=true（决策 93），
//!   不伪造产出元数据，下游 prompt 降级语义（决策 115）；
//! - **E2E-12** 尾段：pending 分支 resume 后 join 正常（前半段「互不阻塞」在
//!   core L2 `one_branch_pending_does_not_stop_the_other`）。
//!
//! 观测窗口说明：executor.run() 内部会持续推进，join 后的游标位置是瞬态；
//! 各用例让流程停在紧随 join 的自然暂停点（脚本耗尽 → pending(retry_exhausted)
//! 或 info_insufficient），并经由 sync_decision 产出、run 行与请求快照断言。

mod common;

use agentpipeline_core::storage::decisions::ResumeAction;
use agentpipeline_core::types::{
    CursorStatus, DevelopDesignMetadata, Node, NodeCursor, PendingKind, ScenarioPriority, Stage,
    TaskStatus, TestDesignMetadata, TestScenario, TransitionTrigger, ValidateInputMetadata,
    ValidateOutputMetadata,
};
use common::{architect_ok, dev_design_ok, Flow};
use testkit::Script;

// ─────────────────────────── 脚本片段 ───────────────────────────

/// test-design 通过，但场景用 Medium 优先级且 `design_refs` 为空——sync-check 的
/// design_refs 完整性校验（决策 136）因此不产生悬空 blocker，join 场景可干净推进。
fn test_design_ok(script: &mut Script) {
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
                priority: ScenarioPriority::Medium,
                design_refs: vec![],
            }],
        });
    script
        .for_node(Stage::TestDesign, Node::ValidateOutput)
        .submit(&ValidateOutputMetadata {
            passed: true,
            ..Default::default()
        });
}

// ─────────────────────────── E2E-11：architect skip → 分裂 ───────────────────────────

#[tokio::test]
async fn e2e_11_architect_skip_splits_into_parallel_branches() {
    let f = Flow::new().await;
    let mut script = Script::new();
    // 输入不足 → pending(info_insufficient)（决策 94），用户选择 skip
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: false,
            blockers: vec!["需要明确部署环境".into()],
        });
    f.agent.set_script(script);

    testkit::seed_task(&f.store, "js1", "p1").await.unwrap();
    f.admit("js1").await;
    f.executor.run("js1").await.unwrap();

    let cursor = f.sole_cursor("js1").await;
    assert_eq!(
        cursor.pending_reason.as_ref().unwrap().kind,
        PendingKind::InfoInsufficient
    );

    // skip 落点表（决策 93）：architect-design → 分裂，不越过 join
    f.store
        .apply_resume(&cursor, ResumeAction::Skip, None, None)
        .await
        .unwrap();

    let live = f.store.load_live_cursors("js1").await.unwrap();
    assert_eq!(live.len(), 2, "分裂为两条并行分支");
    let dev = live
        .iter()
        .find(|c| c.branch == NodeCursor::BRANCH_DEVELOP_DESIGN)
        .unwrap();
    assert_eq!(
        (dev.stage, dev.node),
        (Stage::DevelopDesign, Node::ValidateInput)
    );
    let test = live
        .iter()
        .find(|c| c.branch == NodeCursor::BRANCH_TEST_DESIGN)
        .unwrap();
    assert_eq!(
        (test.stage, test.node),
        (Stage::TestDesign, Node::ValidateInput)
    );
    assert_eq!(test.status, CursorStatus::Active);
    assert!(
        live.iter().all(|c| !c.skipped_to_join),
        "分裂不是 skip 到 join"
    );
}

// ─────────────────────────── E2E-11：develop-design skip ───────────────────────────

#[tokio::test]
async fn e2e_11_develop_design_skip_marks_skipped_to_join_and_proceeds() {
    let f = Flow::new().await;
    let mut script = Script::new();
    architect_ok(&mut script);
    // develop-design 输入不足 → pending(user_decision)（决策 94）
    script
        .for_node(Stage::DevelopDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: false,
            blockers: vec!["缺少数据流定义".into()],
        });
    test_design_ok(&mut script);
    // develop.execute 不给脚本：join 后脚本耗尽 → pending(retry_exhausted)，
    // 恰好是观察 join 后状态的暂停点
    f.agent.set_script(script);

    testkit::seed_task(&f.store, "js2", "p1").await.unwrap();
    f.admit("js2").await;
    f.executor.run("js2").await.unwrap();

    // 一分支 pending、另一分支停在 join 边界（E2E-12 前半段在 core L2 锁定）
    let dev = f
        .live_cursor("js2", NodeCursor::BRANCH_DEVELOP_DESIGN)
        .await;
    assert_eq!(dev.status, CursorStatus::Pending);
    let test = f.live_cursor("js2", NodeCursor::BRANCH_TEST_DESIGN).await;
    assert_eq!(test.status, CursorStatus::WaitingJoin);

    // 用户对 develop-design 分支 skip：置 waiting_join + skipped_to_join（决策 93）
    f.store
        .apply_resume(&dev, ResumeAction::Skip, None, None)
        .await
        .unwrap();
    let dev = f
        .live_cursor("js2", NodeCursor::BRANCH_DEVELOP_DESIGN)
        .await;
    assert_eq!(dev.status, CursorStatus::WaitingJoin);
    assert!(dev.skipped_to_join, "skip 分支带 skipped_to_join 标志");

    // join 后进入 develop.execute：脚本耗尽 → pending(retry_exhausted)，
    // 恰好是观察 join 后状态的暂停点
    f.agent.set_script(Script::new());
    f.executor.run("js2").await.unwrap();

    // join 执行恰一次，skipped 分支视 readiness=true（决策 93）→ proceed
    let runs = f
        .store
        .list_runs_at("js2", Stage::SyncCheck, Node::Execute)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].agent_type, "system", "sync-check 是纯代码节点");
    let decision = f.sync_decision("js2").await;
    assert_eq!(decision["decision"], "proceed");
    assert_eq!(
        decision["dev_readiness"], true,
        "skip 分支视 readiness=true"
    );
    assert_eq!(decision["test_readiness"], true);

    // skip 不改写产出元数据：没有 develop-design 产出行，更没有伪造的 readiness
    assert!(
        f.store
            .get_stage_output("js2", Stage::DevelopDesign, "dev_doc")
            .await
            .unwrap()
            .is_none(),
        "被 skip 的分支不得伪造 stage output"
    );

    // 游标：两条分支已归档，main 在 develop.execute
    let all = f.store.load_all_cursors("js2").await.unwrap();
    assert_eq!(
        all.iter()
            .filter(|c| c.status == CursorStatus::Archived)
            .count(),
        2,
        "两条分支游标归档（决策 113）"
    );
    let main = f.sole_cursor("js2").await;
    assert_eq!((main.stage, main.node), (Stage::Develop, Node::Execute));

    // 下游 prompt 降级语义（决策 115）：develop.execute 的 system prompt 显式写明
    let dev_requests: Vec<_> = f
        .agent
        .request_log()
        .into_iter()
        .filter(|r| r.stage == Stage::Develop && r.node == Node::Execute)
        .collect();
    assert!(
        !dev_requests.is_empty(),
        "join 后应进入 develop.execute（即便脚本耗尽）"
    );
    assert!(
        dev_requests[0].system_prompt.contains("跳过了开发方案阶段"),
        "develop.execute 的 prompt 应含决策 115 降级说明"
    );
}

// ─────────────────────────── E2E-11：test-design skip ───────────────────────────

#[tokio::test]
async fn e2e_11_test_design_skip_is_readiness_true_without_refs_check() {
    let f = Flow::new().await;
    let mut script = Script::new();
    architect_ok(&mut script);
    dev_design_ok(&mut script);
    // test-design 输入不足 → pending(user_decision)
    script
        .for_node(Stage::TestDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: false,
            blockers: vec!["需求缺少异常流程描述".into()],
        });
    f.agent.set_script(script);

    testkit::seed_task(&f.store, "js3", "p1").await.unwrap();
    f.admit("js3").await;
    f.executor.run("js3").await.unwrap();

    let test = f.live_cursor("js3", NodeCursor::BRANCH_TEST_DESIGN).await;
    assert_eq!(test.status, CursorStatus::Pending);
    f.store
        .apply_resume(&test, ResumeAction::Skip, None, None)
        .await
        .unwrap();

    // join 后脚本耗尽 → pending(retry_exhausted)，停在 develop.execute 便于断言
    f.agent.set_script(Script::new());
    f.executor.run("js3").await.unwrap();

    // test-design 被 skip：readiness=true 且 design_refs 校验不生效（决策 136 只校验真实产出）
    let decision = f.sync_decision("js3").await;
    assert_eq!(decision["decision"], "proceed");
    assert_eq!(decision["dev_readiness"], true);
    assert_eq!(
        decision["test_readiness"], true,
        "skip 分支视 readiness=true"
    );
    assert_eq!(
        decision["test_blockers"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0),
        0,
        "skip 分支不产生 blockers"
    );
    assert!(f
        .store
        .get_stage_output("js3", Stage::TestDesign, "test_scenarios")
        .await
        .unwrap()
        .is_none());
    let main = f.sole_cursor("js3").await;
    assert_eq!((main.stage, main.node), (Stage::Develop, Node::Execute));
}

// ─────────────────────────── E2E-12 尾段：resume 后 join 正常 ───────────────────────────

#[tokio::test]
async fn e2e_12_pending_branch_resumes_then_join_proceeds() {
    let f = Flow::new().await;
    let mut script_a = Script::new();
    architect_ok(&mut script_a);
    // develop-design VI 输入不足 → pending(user_decision)；其余照常
    script_a
        .for_node(Stage::DevelopDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: false,
            blockers: vec!["缺少数据流定义".into()],
        });
    test_design_ok(&mut script_a);
    f.agent.set_script(script_a);

    testkit::seed_task(&f.store, "js4", "p1").await.unwrap();
    f.admit("js4").await;
    f.executor.run("js4").await.unwrap();

    // 前半段：develop-design pending，test-design 停在 join（core L2 已锁定，此处复核）
    let dev = f
        .live_cursor("js4", NodeCursor::BRANCH_DEVELOP_DESIGN)
        .await;
    assert_eq!(dev.status, CursorStatus::Pending);
    assert!(
        f.store
            .list_runs_at("js4", Stage::SyncCheck, Node::Execute)
            .await
            .unwrap()
            .is_empty(),
        "有 pending 不汇聚（决策 83 / G5）"
    );

    // resume（continue）→ 分支续跑 → 两分支到界 → join proceed。
    // 换脚本（FakeAgent 的 attempt 会耗尽队列，多轮行为必须分脚本投喂）
    let mut script_b = Script::new();
    script_b
        .for_node(Stage::DevelopDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    dev_design_ok(&mut script_b);
    f.agent.set_script(script_b);
    f.store
        .apply_resume(&dev, ResumeAction::Continue, None, None)
        .await
        .unwrap();
    f.executor.run("js4").await.unwrap();

    assert_eq!(
        f.store
            .list_runs_at("js4", Stage::SyncCheck, Node::Execute)
            .await
            .unwrap()
            .len(),
        1,
        "resume 后 join 恰好执行一次"
    );
    let decision = f.sync_decision("js4").await;
    assert_eq!(decision["decision"], "proceed");
    assert!(
        f.store
            .get_stage_output("js4", Stage::DevelopDesign, "dev_doc")
            .await
            .unwrap()
            .is_some(),
        "真实执行的分支有产出行（对比 skip 场景）"
    );
    let main = f.sole_cursor("js4").await;
    assert_eq!((main.stage, main.node), (Stage::Develop, Node::Execute));
    let all = f.store.load_all_cursors("js4").await.unwrap();
    assert_eq!(
        all.iter()
            .filter(|c| c.status == CursorStatus::Archived)
            .count(),
        2
    );
}

// ─────────────────────────── E2E-02：sync-check backtrack ───────────────────────────

#[tokio::test]
async fn e2e_02_sync_check_backtrack_resets_to_architect_and_marks_docs_stale() {
    let f = Flow::new().await;
    let mut script_a = Script::new();
    // 第一轮 architect 正常；双分支均报 blockers → backtrack（决策 83 / G5 的另一条边真正可达）
    architect_ok(&mut script_a);
    script_a
        .for_node(Stage::DevelopDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    script_a
        .for_node(Stage::DevelopDesign, Node::Execute)
        .write_file("dev-plan.md", "# 开发计划\n")
        .submit(&DevelopDesignMetadata {
            readiness: true,
            blockers: vec!["缺少数据流定义".into()],
            dev_doc_path: Some("dev-plan.md".into()),
            ..Default::default()
        });
    script_a
        .for_node(Stage::DevelopDesign, Node::ValidateOutput)
        .submit(&ValidateOutputMetadata {
            passed: true,
            ..Default::default()
        });
    script_a
        .for_node(Stage::TestDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    script_a
        .for_node(Stage::TestDesign, Node::Execute)
        .write_file("test-scenarios.md", "# 测试场景\n")
        .submit(&TestDesignMetadata {
            readiness: true,
            blockers: vec!["缺少异常流程场景".into()],
            test_scenarios_path: Some("test-scenarios.md".into()),
            test_scenarios: vec![],
        });
    script_a
        .for_node(Stage::TestDesign, Node::ValidateOutput)
        .submit(&ValidateOutputMetadata {
            passed: true,
            ..Default::default()
        });
    // 重入的 architect.validate_input 不给脚本：脚本耗尽 → pending(retry_exhausted)，
    // 流程停在 backtrack 目标上便于断言
    f.agent.set_script(script_a);

    testkit::seed_task(&f.store, "js5", "p1").await.unwrap();
    f.admit("js5").await;
    f.executor.run("js5").await.unwrap();

    // main 游标回到 architect-design.validate_input（决策 83）
    let main = f.sole_cursor("js5").await;
    assert_eq!(
        (main.stage, main.node),
        (Stage::ArchitectDesign, Node::ValidateInput),
        "backtrack 后 main 指 architect.validate_input"
    );
    assert_eq!(
        main.validate_attempts, 0,
        "跨阶段跳转 attempts 归零（决策 43）"
    );

    // 重入（resume）：仍不足 → pending(info_insufficient)
    let mut script_b = Script::new();
    script_b
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: false,
            blockers: vec!["补充：明确部署环境".into()],
        });
    f.agent.set_script(script_b);
    f.store
        .apply_resume(&main, ResumeAction::Continue, None, None)
        .await
        .unwrap();
    f.executor.run("js5").await.unwrap();

    let main = f.sole_cursor("js5").await;
    assert_eq!(
        main.pending_reason.as_ref().unwrap().kind,
        PendingKind::InfoInsufficient
    );
    assert_eq!(
        f.store.get_task("js5").await.unwrap().status,
        TaskStatus::Pending
    );

    // 两条分支游标已归档（决策 113）
    let all = f.store.load_all_cursors("js5").await.unwrap();
    assert_eq!(
        all.len(),
        3,
        "初始 main + 分裂 test-design + backtrack 新 main"
    );
    assert_eq!(
        all.iter()
            .filter(|c| c.status == CursorStatus::Archived)
            .count(),
        2
    );

    // 决策 83：dev-plan.md / test-scenarios.md 标过期，文件保留；design.md 不受影响
    let dev_doc = f
        .store
        .get_stage_output("js5", Stage::DevelopDesign, "dev_doc")
        .await
        .unwrap()
        .unwrap();
    assert!(dev_doc.stale, "dev-plan.md 应标过期");
    let scenarios = f
        .store
        .get_stage_output("js5", Stage::TestDesign, "test_scenarios")
        .await
        .unwrap()
        .unwrap();
    assert!(scenarios.stale, "test-scenarios.md 应标过期");
    let design = f
        .store
        .get_stage_output("js5", Stage::ArchitectDesign, "design_doc")
        .await
        .unwrap()
        .unwrap();
    assert!(!design.stale, "design.md 不标过期");
    assert!(
        f.home.home().task_file("js5", "dev-plan.md").exists(),
        "文件保留供回溯"
    );
    assert!(
        f.home.home().task_file("js5", "test-scenarios.md").exists(),
        "文件保留供回溯"
    );

    // 决策 126：blockers 写入 backtrack-feedback.md
    let feedback =
        std::fs::read_to_string(f.home.home().task_file("js5", "backtrack-feedback.md")).unwrap();
    assert!(
        feedback.contains("缺少数据流定义"),
        "dev blockers 进反馈文件"
    );
    assert!(
        feedback.contains("缺少异常流程场景"),
        "test blockers 进反馈文件"
    );

    // sync-check 恰执行一次，产出 backtrack 决策
    let runs = f
        .store
        .list_runs_at("js5", Stage::SyncCheck, Node::Execute)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].agent_type, "system");
    let decision = f.sync_decision("js5").await;
    assert_eq!(decision["decision"], "backtrack");
    assert_eq!(
        decision["dev_blockers"][0], "缺少数据流定义",
        "blockers 进入 SyncDecision"
    );

    // 流转时间线记录 backtrack（决策 84：branch 字段贯穿）
    let transitions = f.store.list_transitions("js5").await.unwrap();
    assert!(transitions
        .iter()
        .any(|t| t.trigger == TransitionTrigger::AutoResume
            && t.to_stage == Stage::ArchitectDesign
            && t.to_node == Node::ValidateInput
            && t.branch == NodeCursor::BRANCH_MAIN
            && t.reason.as_deref().unwrap_or("").contains("判定回溯")));

    // 决策 126：重入 prompt 含反馈段——首轮不渲染，重入渲染。
    // 请求构成：首轮 VI 2 次（提交轮 + 队列耗尽收尾）+ 重入脚本耗尽的 3 次重试
    // + resume 后 script_b 重入 2 次，共 7 次
    let vi_requests: Vec<_> = f
        .agent
        .request_log()
        .into_iter()
        .filter(|r| r.stage == Stage::ArchitectDesign && r.node == Node::ValidateInput)
        .collect();
    assert_eq!(vi_requests.len(), 7);
    assert!(
        vi_requests
            .iter()
            .take(2)
            .all(|r| !r.user_prompt.contains("上游回溯反馈")),
        "首轮（无反馈文件）不渲染追加段"
    );
    assert!(
        vi_requests
            .iter()
            .skip(2)
            .all(|r| r.user_prompt.contains("上游回溯反馈")),
        "重入 prompt 应注入 backtrack 反馈段（决策 126）"
    );
    assert!(
        vi_requests[2].user_prompt.contains("缺少数据流定义"),
        "反馈段内容来自 backtrack-feedback.md"
    );
}
