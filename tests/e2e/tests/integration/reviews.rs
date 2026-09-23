//! E2E 评审场景（testing.md §8）：E2E-03 review 打回循环、E2E-04 human review。
//!
//! 决策 2 / 43 / 124 / 133：review 不通过交用户裁决（不是节点重试）；人工评审
//! pending(human_review)、approve → test / reject → develop。

use super::common::{design_ok, Flow};
use agentpipeline_core::storage::decisions::ResumeAction;
use agentpipeline_core::types::{
    Approval, Node, PendingKind, ReviewMode, ReviewResult, Stage, TaskStatus, TestResult,
    TransitionTrigger,
};
use testkit::Script;

/// 到 review 为止的脚本（develop 真写代码 + review.execute 给出指定结论）。
fn to_review(script: &mut Script, task_id: &str, review: ReviewResult) {
    design_ok(script);
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
        .submit(&agentpipeline_core::types::CodeChanges {
            branch_name: format!("kanban/{task_id}"),
            changed_files: vec![],
            unit_test_files: vec![],
        });
    script
        .for_node(Stage::Review, Node::Execute)
        .submit(&review);
}

// ─────────────────────────── E2E-03 ───────────────────────────

#[tokio::test]
async fn e2e_03_review_rejection_pends_then_goto_develop_and_re_review_passes() {
    let f = Flow::new().await;
    let mut script = Script::new();
    to_review(
        &mut script,
        "t3",
        ReviewResult {
            approved: false,
            review_report_path: Some("review-report.md".into()),
            required_changes: vec![agentpipeline_core::types::FileChangeSpec {
                path: "src/lib.rs".into(),
                action: agentpipeline_core::types::FileAction::Modify,
                content_hash: None,
            }],
        },
    );
    f.agent.set_script(script);
    testkit::seed_task(&f.store, "t3", "p1").await.unwrap();
    f.admit("t3").await;

    // 首轮：review 判定不通过 → pending(user_decision)（不是节点重试，决策 2 / 131）
    f.executor.run("t3").await.unwrap();
    let cursor = f.sole_cursor("t3").await;
    assert_eq!(
        (cursor.stage, cursor.node),
        (Stage::Review, Node::ValidateOutput)
    );
    assert_eq!(
        cursor.pending_reason.as_ref().unwrap().kind,
        PendingKind::UserDecision
    );

    // 决策 130 ① / 票 05：review 打回必须带 `context.kind = review`，否则
    // allowed_actions 落 `(user_decision, _)` 通用兜底行（skip / cancel）。
    let pending = f.pending_of("t3").await;
    assert_eq!(
        pending.context.as_ref().and_then(|c| c.kind.as_deref()),
        Some("review"),
        "review 打回带 context.kind = review"
    );
    let actions = f.store.allowed_actions_for_task("t3").await.unwrap();
    let names: Vec<&str> = actions.iter().map(|a| a.action.as_str()).collect();
    assert_eq!(
        names,
        vec!["goto", "skip"],
        "权威表 review 行：打回开发修复 / 强制通过评审"
    );
    let labels: Vec<&str> = actions.iter().map(|a| a.label.as_str()).collect();
    assert_eq!(labels, vec!["打回开发修复", "强制通过评审"]);
    // 打回落点 = develop.execute（决策 130 ①）
    let goto = &actions[0];
    let target = goto.target.as_ref().expect("goto 应带落点");
    assert_eq!(
        (target.stage, target.node),
        (Stage::Develop, Node::Execute),
        "打回开发修复落 develop.execute"
    );

    // review.execute 只跑一次（不通过不是节点重试）
    assert_eq!(
        f.store
            .list_runs_at("t3", Stage::Review, Node::Execute)
            .await
            .unwrap()
            .len(),
        1
    );
    let review_meta = f
        .store
        .stage_output_metadata("t3", Stage::Review, "review_report")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(review_meta["approved"], false);
    assert_eq!(review_meta["required_changes"][0]["path"], "src/lib.rs");

    // 用户 goto develop.execute 打回修复
    agentpipeline_core::pipeline::resume::apply_action(
        &f.store,
        &cursor,
        ResumeAction::Goto,
        Some((Stage::Develop, Node::Execute)),
        None,
    )
    .await
    .unwrap();
    let after = f.store.get_cursor(&cursor.cursor_id).await.unwrap();
    assert_eq!((after.stage, after.node), (Stage::Develop, Node::Execute));
    assert_eq!(
        after.validate_attempts, 0,
        "跨阶段跳转 attempts 归零（决策 43）"
    );

    // 重新走 develop → review，这次通过
    let mut rework = Script::new();
    to_review(
        &mut rework,
        "t3",
        ReviewResult {
            approved: true,
            review_report_path: Some("review-report.md".into()),
            required_changes: vec![],
        },
    );
    rework
        .for_node(Stage::Test, Node::Execute)
        .submit(&TestResult {
            passed: true,
            test_report_path: Some("test-report.md".into()),
            failures: vec![],
            gate_recheck: false,
        });
    f.agent.set_script(rework);
    f.executor.run("t3").await.unwrap();

    // 决策 133 / 票 07：develop 重入的 user prompt 含本轮必须修改项；首轮不渲染该段。
    let dev_prompts: Vec<String> = f
        .requests_for(Stage::Develop, Node::Execute)
        .into_iter()
        .map(|r| r.user_prompt)
        .collect();
    assert!(
        !dev_prompts[0].contains("评审必须修改项"),
        "首轮 develop 不渲染必须修改项段（无上游打回）：{}",
        dev_prompts[0]
    );
    assert!(
        dev_prompts
            .iter()
            .skip(1)
            .any(|p| p.contains("评审必须修改项") && p.contains("src/lib.rs")),
        "review 打回后 develop 重入 prompt 应含必须修改项：{dev_prompts:?}"
    );

    assert_eq!(
        f.store
            .list_runs_at("t3", Stage::Review, Node::ValidateOutput)
            .await
            .unwrap()
            .len(),
        2,
        "re-review 再判定一次"
    );
    let review_meta = f
        .store
        .stage_output_metadata("t3", Stage::Review, "review_report")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(review_meta["approved"], true, "re-review 通过");
    // 一路推进到 merge_approval
    let merge = f.store.merge_metadata("t3").await.unwrap().unwrap();
    assert_eq!(merge.approval, Approval::Pending);
    assert_eq!(
        f.store.get_task("t3").await.unwrap().status,
        TaskStatus::Pending
    );
}

// ─────────────────────────── E2E-04 ───────────────────────────

async fn human_review_flow_task(f: &Flow, task_id: &str) {
    let mut script = Script::new();
    to_review(
        &mut script,
        task_id,
        ReviewResult {
            approved: true,
            review_report_path: Some("review-report.md".into()),
            required_changes: vec![],
        },
    );
    f.agent.set_script(script);
    testkit::seed_task_full(&f.store, task_id, "p1", ReviewMode::Human, &[])
        .await
        .unwrap();
    f.admit(task_id).await;
    f.executor.run(task_id).await.unwrap();
}

#[tokio::test]
async fn e2e_04_human_review_pends_then_reject_goes_to_develop() {
    let f = Flow::new().await;
    human_review_flow_task(&f, "t4r").await;

    let cursor = f.sole_cursor("t4r").await;
    assert_eq!(
        (cursor.stage, cursor.node),
        (Stage::Review, Node::ValidateOutput)
    );
    assert_eq!(
        cursor.pending_reason.as_ref().unwrap().kind,
        PendingKind::HumanReview
    );

    // 决策 124 / 票 13：人工评审前系统生成 review-diff.diff（基准 → 任务分支），
    // 落 stage output（output_type = review_diff），经 /files/{path} 可取。
    let diff_path = f.home.home().task_file("t4r", "review-diff.diff");
    assert!(diff_path.exists(), "人工评审前应生成 review-diff.diff");
    let diff = std::fs::read_to_string(&diff_path).unwrap();
    assert!(
        diff.contains("src/lib.rs"),
        "diff 应为基准到任务分支的真实差异：{diff}"
    );
    let out = f
        .store
        .get_stage_output("t4r", Stage::Review, "review_diff")
        .await
        .unwrap()
        .expect("review_diff stage output 应存在");
    assert_eq!(out.file_path, "review-diff.diff");
    // 记为系统来源命令（决策 124）
    let commands = f.store.list_commands("t4r", None, None).await.unwrap();
    assert!(
        commands.iter().any(
            |c| c.source == agentpipeline_core::types::CommandSource::System
                && c.command.contains("git diff")
        ),
        "应记一条系统来源的 git diff 命令：{:?}",
        commands
            .iter()
            .map(|c| (&c.command, c.source))
            .collect::<Vec<_>>()
    );

    // 动作集：approve / reject，端点按行配对（决策 101）
    let actions = f.store.allowed_actions_for_task("t4r").await.unwrap();
    let names: Vec<&str> = actions.iter().map(|a| a.action.as_str()).collect();
    assert_eq!(names, vec!["approve", "reject"]);

    // reject → develop.execute，comments 进流转原因（§12.5 / 决策 124）
    let after = f
        .store
        .apply_human_review("t4r", false, Some("实现与设计不符"))
        .await
        .unwrap();
    assert_eq!((after.stage, after.node), (Stage::Develop, Node::Execute));
    assert_eq!(
        after.status,
        agentpipeline_core::types::CursorStatus::Active
    );
    let transitions = f.store.list_transitions("t4r").await.unwrap();
    assert!(
        transitions
            .iter()
            .any(|t| t.trigger == TransitionTrigger::UserResume
                && t.to_stage == Stage::Develop
                && t.reason.as_deref().unwrap_or("").contains("实现与设计不符")),
        "comments 应写进流转原因：{transitions:?}"
    );
}

#[tokio::test]
async fn e2e_04_human_review_approve_goes_to_test() {
    let f = Flow::new().await;
    human_review_flow_task(&f, "t4a").await;
    let cursor = f.sole_cursor("t4a").await;
    assert_eq!(
        cursor.pending_reason.as_ref().unwrap().kind,
        PendingKind::HumanReview
    );

    // approve → test.execute
    let after = f.store.apply_human_review("t4a", true, None).await.unwrap();
    assert_eq!((after.stage, after.node), (Stage::Test, Node::Execute));
    assert_eq!(
        after.status,
        agentpipeline_core::types::CursorStatus::Active
    );

    // 继续跑：test → merge 阶段 A 待审批
    let mut test_script = Script::new();
    test_script
        .for_node(Stage::Test, Node::Execute)
        .submit(&TestResult {
            passed: true,
            test_report_path: Some("test-report.md".into()),
            failures: vec![],
            gate_recheck: false,
        });
    f.agent.set_script(test_script);
    f.executor.run("t4a").await.unwrap();
    let merge = f.store.merge_metadata("t4a").await.unwrap().unwrap();
    assert_eq!(merge.approval, Approval::Pending);
    assert_eq!(
        f.store.get_task("t4a").await.unwrap().status,
        TaskStatus::Pending
    );
}
