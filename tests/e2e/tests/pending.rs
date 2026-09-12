//! E2E pending 语义场景（testing.md §8）：E2E-15 judge_disagreement、E2E-16 design_refs
//! 完整性、E2E-21 info_insufficient、E2E-22 context_overflow。
//!
//! 决策 79 / 94 / 105 / 110 / 134 / 135 / 136。

mod common;

use agentpipeline_core::config::Settings;
use agentpipeline_core::pipeline::pseudo::CrossCheckResult;
use agentpipeline_core::storage::decisions::ResumeAction;
use agentpipeline_core::types::{
    ArchitectExecuteMetadata, Node, PendingKind, ScenarioPriority, Stage, TestDesignMetadata,
    TestScenario, ValidateInputMetadata, ValidateOutputMetadata,
};
use common::{architect_ok, Flow};
use testkit::Script;

// ─────────────────────────── E2E-21 ───────────────────────────

#[tokio::test]
async fn e2e_21_info_insufficient_requires_input_then_reruns_validate_input() {
    let f = Flow::new().await;
    let mut script = Script::new();
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: false,
            blockers: vec!["需要明确部署环境".into()],
        });
    f.agent.set_script(script);
    testkit::seed_task(&f.store, "t21", "p1").await.unwrap();
    f.admit("t21").await;
    f.executor.run("t21").await.unwrap();

    let cursor = f.sole_cursor("t21").await;
    assert_eq!(
        cursor.pending_reason.as_ref().unwrap().kind,
        PendingKind::InfoInsufficient
    );
    // 唯一带自由输入的动作（决策 79）：continue 需输入 + cancel
    let actions = f.store.allowed_actions_for_task("t21").await.unwrap();
    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0].action, "continue");
    assert!(actions[0].requires_input, "info_insufficient 需自由输入");
    assert_eq!(actions[1].action, "cancel");

    // continue 带输入 → 用户输入落库为流转原因（决策 79）
    let input = "部署环境是生产 k8s";
    f.store
        .apply_resume(&cursor, ResumeAction::Continue, None, Some(input))
        .await
        .unwrap();
    let transitions = f.store.list_transitions("t21").await.unwrap();
    assert!(
        transitions
            .iter()
            .any(|t| t.reason.as_deref() == Some(input)),
        "补充信息应进流转原因：{transitions:?}"
    );

    // validate_input 重跑：脚本给出 readiness=true → 推进到 execute
    let mut rerun = Script::new();
    rerun
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    f.agent.set_script(rerun);
    f.executor.run("t21").await.unwrap();

    // 决策 79 / 票 08：补充输入注入 validate_input 重入的 user prompt（不只落流转原因）。
    let vi_prompts: Vec<String> = f
        .requests_for(Stage::ArchitectDesign, Node::ValidateInput)
        .into_iter()
        .map(|r| r.user_prompt)
        .collect();
    assert!(
        !vi_prompts[0].contains("用户补充输入"),
        "首轮（无补充输入）不渲染该段：{}",
        vi_prompts[0]
    );
    assert!(
        vi_prompts
            .iter()
            .skip(1)
            .any(|p| p.contains("用户补充输入") && p.contains(input)),
        "重入 prompt 应含补充输入：{vi_prompts:?}"
    );

    assert!(
        f.agent
            .calls_for(Stage::ArchitectDesign, Node::ValidateInput)
            >= 2,
        "validate_input 应重跑"
    );
    let after = f.sole_cursor("t21").await;
    assert_eq!(after.stage, Stage::ArchitectDesign);
    assert_eq!(after.node, Node::Execute, "readiness 通过后推进到 execute");
}

// ─────────────────────────── E2E-16 ───────────────────────────

fn test_design_with(script: &mut Script, priority: ScenarioPriority, refs: Vec<String>) {
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
                name: "悬空引用场景".into(),
                description: "引用不存在的 AC".into(),
                preconditions: vec![],
                steps: vec![],
                expected_result: "成功".into(),
                priority,
                design_refs: refs,
            }],
        });
    script
        .for_node(Stage::TestDesign, Node::ValidateOutput)
        .submit(&ValidateOutputMetadata {
            passed: true,
            ..Default::default()
        });
}

async fn sync_decision(f: &Flow, task_id: &str) -> serde_json::Value {
    f.store
        .stage_output_metadata(task_id, Stage::SyncCheck, "sync_decision")
        .await
        .unwrap()
        .expect("sync_decision 产出")
}

#[tokio::test]
async fn e2e_16_high_dangling_design_ref_is_blocker_and_backtracks() {
    let f = Flow::new().await;
    let mut script = Script::new();
    architect_ok(&mut script);
    common::dev_design_ok(&mut script);
    test_design_with(&mut script, ScenarioPriority::High, vec!["AC-999".into()]);
    f.agent.set_script(script);
    testkit::seed_task(&f.store, "t16h", "p1").await.unwrap();
    f.admit("t16h").await;
    f.executor.run("t16h").await.unwrap();

    let decision = sync_decision(&f, "t16h").await;
    assert_eq!(
        decision["decision"], "backtrack",
        "high 悬空 → blocker → backtrack"
    );
    let blockers: Vec<&str> = decision["test_blockers"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        blockers.iter().any(|b| b.contains("high 场景")),
        "blocker 应指向 high 场景：{blockers:?}"
    );
    // 回溯到 architect.validate_input
    let main = f.sole_cursor("t16h").await;
    assert_eq!(
        (main.stage, main.node),
        (Stage::ArchitectDesign, Node::ValidateInput)
    );
}

#[tokio::test]
async fn e2e_16_medium_dangling_design_ref_is_only_a_warning() {
    let f = Flow::new().await;
    let mut script = Script::new();
    architect_ok(&mut script);
    common::dev_design_ok(&mut script);
    test_design_with(&mut script, ScenarioPriority::Medium, vec!["AC-999".into()]);
    f.agent.set_script(script);
    testkit::seed_task(&f.store, "t16m", "p1").await.unwrap();
    f.admit("t16m").await;
    f.executor.run("t16m").await.unwrap();

    let decision = sync_decision(&f, "t16m").await;
    assert_eq!(decision["decision"], "proceed", "medium 悬空只 warning");
    let warnings = decision["warnings"].as_array().expect("warnings 数组");
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].as_str().unwrap().contains("AC-999"));
    // 推进到 develop.execute
    let main = f.sole_cursor("t16m").await;
    assert_eq!((main.stage, main.node), (Stage::Develop, Node::Execute));
}

// ─────────────────────────── E2E-15 ───────────────────────────

async fn judge_disagreement_flow(task_id: &str) -> Flow {
    let settings = Settings {
        cross_family_judge: true,
        ..Default::default()
    };
    let f = Flow::with_settings(settings).await;
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
    f.agent.set_script(script);
    testkit::seed_task(&f.store, task_id, "p1").await.unwrap();
    f.admit(task_id).await;
    f.executor.run(task_id).await.unwrap();
    f
}

#[tokio::test]
async fn e2e_15_judge_disagreement_pends_then_continue_advances_without_rerun() {
    let f = judge_disagreement_flow("t15c").await;
    let cursor = f.sole_cursor("t15c").await;
    let reason = cursor.pending_reason.as_ref().unwrap();
    assert_eq!(reason.kind, PendingKind::UserDecision);
    assert_eq!(
        reason.context.as_ref().unwrap().kind.as_deref(),
        Some("judge_disagreement")
    );
    // 动作集：continue（裁决合格）/ goto execute（裁决不合格）（决策 135）
    let actions = f.store.allowed_actions_for_task("t15c").await.unwrap();
    let names: Vec<&str> = actions.iter().map(|a| a.action.as_str()).collect();
    assert_eq!(names, vec!["continue", "goto"]);
    let target = actions[1].target.as_ref().unwrap();
    assert_eq!(
        (target.stage, target.node),
        (Stage::ArchitectDesign, Node::Execute)
    );

    // continue = 用户裁决合格 → 直接放行下一阶段（architect 分裂），不重跑 validate_output
    let before = f
        .agent
        .calls_for(Stage::ArchitectDesign, Node::ValidateOutput);
    f.store
        .apply_resume(&cursor, ResumeAction::Continue, None, None)
        .await
        .unwrap();
    let live = f.live_cursors("t15c").await;
    assert_eq!(live.len(), 2, "architect 放行分裂为两条设计分支");
    let stages: Vec<Stage> = live.iter().map(|c| c.stage).collect();
    assert!(stages.contains(&Stage::DevelopDesign));
    assert!(stages.contains(&Stage::TestDesign));
    assert_eq!(
        f.agent
            .calls_for(Stage::ArchitectDesign, Node::ValidateOutput),
        before,
        "continue 放行不得重跑校验"
    );
}

#[tokio::test]
async fn e2e_15_judge_disagreement_goto_execute_increments_attempts() {
    let f = judge_disagreement_flow("t15g").await;
    let cursor = f.sole_cursor("t15g").await;
    f.store
        .apply_resume(
            &cursor,
            ResumeAction::Goto,
            Some((Stage::ArchitectDesign, Node::Execute)),
            None,
        )
        .await
        .unwrap();
    let after = f.store.get_cursor(&cursor.cursor_id).await.unwrap();
    assert_eq!(
        (after.stage, after.node),
        (Stage::ArchitectDesign, Node::Execute)
    );
    assert_eq!(
        after.validate_attempts, 1,
        "裁决不合格打回，attempts +1（决策 135）"
    );
}

// ─────────────────────────── E2E-22 ───────────────────────────

#[tokio::test]
async fn e2e_22_context_overflow_actions_all_have_paired_endpoints() {
    // 票 04：E2E-22 走**真实执行路径**触发——登记一个极小 `context_window` 的 provider
    // 让 L0 容量分档生效，再用超长工具结果把 messages 顶过硬限；L3 规则化压缩压不下去
    // → L4 挂 pending(context_overflow)。不再在测试里手工构造 pending。
    let f = Flow::new().await;
    // 窗口极小（1000）：软限 600 / 硬限 900，一次长工具结果即越过
    f.seed_provider("prov-ctx", 1_000).await;

    let mut script = Script::new();
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    // architect.execute 先读一个超长文件：工具结果本身经 L2 卸载后仍留预览，
    // 但成功/失败两条路径都可；这里用大 submit metadata 内嵌长文本把上下文顶爆。
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
    f.agent.set_script(script);

    testkit::seed_task(&f.store, "t22", "p1").await.unwrap();
    f.admit("t22").await;
    f.executor.run("t22").await.unwrap();

    let cursor = f.sole_cursor("t22").await;
    let reason = cursor
        .pending_reason
        .as_ref()
        .unwrap_or_else(|| panic!("应挂 pending；游标 = {cursor:?}"));
    assert_eq!(
        reason.kind,
        PendingKind::ContextOverflow,
        "真实执行路径应触发 pending(context_overflow)（§12.13 L4）"
    );

    // 动作集三项均有配对端点（决策 101 / 105）
    let actions = f.store.allowed_actions_for_task("t22").await.unwrap();
    let names: Vec<&str> = actions.iter().map(|a| a.action.as_str()).collect();
    assert_eq!(names, vec!["split_task", "model_override", "cancel"]);
    for a in &actions {
        assert_eq!(a.kind, agentpipeline_core::actions::ActionKind::SideEffect);
        assert!(
            agentpipeline_core::actions::endpoint_for(PendingKind::ContextOverflow, &a.action)
                .is_some(),
            "{} 应有配对端点",
            a.action
        );
    }
}
