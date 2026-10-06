//! E2E 闸门场景（testing.md §8）：E2E-06a / 06b / 07 / 08。
//!
//! 覆盖决策 85 / 86 / 108 / 109 / 125 / 139：merge 测试闸门失败跳 test.execute 复检、
//! code_issue 交用户、lint 失败直接打回 develop、gate_failures 耗尽收口与 retry 复位。

use super::common::{design_ok, full_pass_script, Flow};
use agentpipeline_core::git::Git;
use agentpipeline_core::storage::decisions::ResumeAction;
use agentpipeline_core::types::{
    Approval, CodeChanges, FailureCause, Gate, GateFailureKind, Node, PendingKind, ReviewResult,
    Stage, TaskStatus, TestFailure, TestResult, TransitionTrigger,
};
use testkit::Script;

/// 有状态闸门脚本：第 `fail_on` 次调用退出 1（含匹配输出），其余退出 0。
fn write_stateful_gate(flow: &Flow, name: &str, fail_on: &[u32], output: &str) -> String {
    let home = flow.home.home().root().to_path_buf();
    let script = home.join(format!("{name}.sh"));
    let counter = home.join(format!("{name}-count"));
    let fail_clauses = fail_on
        .iter()
        .map(|n| format!("if [ \"$n\" -eq {n} ]; then echo \"{output}\"; exit 1; fi\n"))
        .collect::<String>();
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nd=\"{}\"\nn=$(cat \"$d\" 2>/dev/null || echo 0)\nn=$((n+1))\necho $n > \"$d\"\n{fail_clauses}exit 0\n",
            counter.display()
        ),
    )
    .unwrap();
    format!("sh {}", script.display())
}

/// 同上，但失败时输出 200 行：中间行会被首尾预览（50/100）裁掉，
/// 用来验证决策 109 / 票 09 的「注入完整日志」而非预览。
fn write_verbose_gate(flow: &Flow, name: &str, fail_on: &[u32], middle_marker: &str) -> String {
    let home = flow.home.home().root().to_path_buf();
    let script = home.join(format!("{name}.sh"));
    let counter = home.join(format!("{name}-count"));
    // 第 1..=80 行为噪声，第 90 行为 middle_marker（落在首尾预览的省略区），其后为噪声
    let body = format!(
        "i=1\nwhile [ \"$i\" -le 80 ]; do echo \"noise line $i\"; i=$((i+1)); done\necho \"{middle_marker}\"\ni=91\nwhile [ \"$i\" -le 200 ]; do echo \"noise line $i\"; i=$((i+1)); done\nexit 1\n"
    );
    let fail_clauses = fail_on
        .iter()
        .map(|n| format!("if [ \"$n\" -eq {n} ]; then {body} fi\n"))
        .collect::<String>();
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nd=\"{}\"\nn=$(cat \"$d\" 2>/dev/null || echo 0)\nn=$((n+1))\necho $n > \"$d\"\n{fail_clauses}exit 0\n",
            counter.display()
        ),
    )
    .unwrap();
    format!("sh {}", script.display())
}

/// 把项目 test_framework 换成状态化闸门命令。
async fn set_test_framework(flow: &Flow, command: &str) {
    flow.store
        .update_project("p1", None, None, Some(command), None)
        .await
        .unwrap();
}

// ─────────────────────────── E2E-06a ───────────────────────────

#[tokio::test]
async fn e2e_06a_merge_test_gate_failure_rechecks_via_test_then_passes() {
    let f = Flow::new().await;
    // 第 1 次（develop 闸门）通过，第 2 次（merge 闸门首跑）失败，第 3 次（复检后重跑）通过。
    // 失败输出 200 行，中间标记行会被首尾预览裁掉 → 验证注入的是完整日志（票 09）。
    const MIDDLE: &str = "GATE_MIDDLE_LINE_MUST_BE_INJECTED";
    let gate = write_verbose_gate(&f, "gate", &[2], MIDDLE);
    set_test_framework(&f, &gate).await;

    let mut script = Script::new();
    full_pass_script(&mut script, "t6a");
    f.agent.set_script(script);
    testkit::seed_task(&f.store, "t6a", "p1").await.unwrap();
    f.admit("t6a").await;

    // run #1：初始 test.execute 提交 passed=true → merge 闸门失败 → GotoTest →
    // test.execute 复检（脚本耗尽 → pending 收尾）
    f.executor.run("t6a").await.unwrap();

    let merge = f.store.merge_metadata("t6a").await.unwrap().unwrap();
    assert_eq!(merge.gate, Some(Gate::Fail));
    assert_eq!(merge.gate_failure_kind, Some(GateFailureKind::Test));
    assert_eq!(merge.gate_failures, 1, "统一累加（决策 108）");
    let transitions = f.store.list_transitions("t6a").await.unwrap();
    assert!(
        transitions.iter().any(|t| t.to_stage == Stage::Test
            && t.to_node == Node::Execute
            && t.trigger == TransitionTrigger::Kickback),
        "应有 merge → test.execute 的 kickback：{transitions:?}"
    );
    assert!(
        !transitions
            .iter()
            .any(|t| t.to_stage == Stage::Develop && t.trigger == TransitionTrigger::Kickback),
        "测试闸门失败不得直接打回 develop"
    );
    let cursor = f.sole_cursor("t6a").await;
    assert_eq!((cursor.stage, cursor.node), (Stage::Test, Node::Execute));
    assert_eq!(
        cursor.pending_reason.as_ref().unwrap().kind,
        PendingKind::RetryExhausted
    );
    // 复检 prompt 含闸门完整输出（决策 109），首轮不渲染
    let requests = f.agent.request_log();
    let test_reqs: Vec<_> = requests
        .iter()
        .filter(|r| r.stage == Stage::Test && r.node == Node::Execute)
        .collect();
    assert!(
        !test_reqs[0].user_prompt.contains("合入闸门失败复检上下文"),
        "首轮不渲染复检段"
    );
    assert!(
        test_reqs
            .iter()
            .skip(1)
            .any(|r| r.user_prompt.contains("合入闸门失败复检上下文")
                && r.user_prompt.contains(MIDDLE)),
        "复检 prompt 应含**完整日志**（含被预览裁掉的中间行）：{:?}",
        test_reqs
            .iter()
            .map(|r| r.user_prompt.clone())
            .collect::<Vec<_>>()
    );

    // run #2：复检脚本（agent 未自报 gate_recheck）→ 系统置位（决策 109）→ 闸门重跑通过
    let mut recheck = Script::new();
    recheck
        .for_node(Stage::Test, Node::Execute)
        .submit(&TestResult {
            passed: true,
            test_report_path: Some("test-report.md".into()),
            failures: vec![],
            gate_recheck: false,
        });
    f.agent.set_script(recheck);
    let cursor = f.sole_cursor("t6a").await;
    agentpipeline_core::pipeline::resume::apply_action(
        &f.store,
        &cursor,
        ResumeAction::Continue,
        None,
        None,
    )
    .await
    .unwrap();
    f.executor.run("t6a").await.unwrap();

    let meta = f
        .store
        .stage_output_metadata("t6a", Stage::Test, "test_report")
        .await
        .unwrap()
        .expect("test_report 元数据");
    assert_eq!(meta["gate_recheck"], true, "系统置位 gate_recheck");
    let merge = f.store.merge_metadata("t6a").await.unwrap().unwrap();
    assert_eq!(merge.gate, Some(Gate::Pass));
    assert_eq!(merge.approval, Approval::Pending);
    assert_eq!(
        f.store.get_task("t6a").await.unwrap().status,
        TaskStatus::Pending,
        "停在 merge_approval"
    );
}

// ─────────────────────────── E2E-06b ───────────────────────────

/// 非闸门复检的直接路径：test.execute 首次即报 code_issue → `test_code_issue`。
///
/// 与 E2E-06b 区分：那条是 merge 测试闸门打回后的复检（kind = `gate_recheck`）；
/// 本条不经 merge 闸门，是 test 阶段自身的代码问题（kind = `test_code_issue`）。
#[tokio::test]
async fn e2e_06b_direct_code_issue_pends_with_test_code_issue_kind() {
    let f = Flow::new().await;
    let mut script = Script::new();
    // 全流程脚本，只在 test.execute 覆盖为失败（含 code_issue）
    full_pass_script(&mut script, "t6c");
    script
        .for_node(Stage::Test, Node::Execute)
        .write_file("test-report.md", "# 测试报告\n业务代码未实现\n")
        .submit(&TestResult {
            passed: false,
            test_report_path: Some("test-report.md".into()),
            failures: vec![TestFailure {
                test_name: "login".into(),
                error_message: "业务代码未实现".into(),
                failure_cause: FailureCause::CodeIssue,
            }],
            gate_recheck: false,
        });
    f.agent.set_script(script);
    testkit::seed_task(&f.store, "t6c", "p1").await.unwrap();
    f.admit("t6c").await;
    f.executor.run("t6c").await.unwrap();

    let cursor = f.sole_cursor("t6c").await;
    assert_eq!(
        (cursor.stage, cursor.node),
        (Stage::Test, Node::ValidateOutput)
    );
    assert_eq!(
        cursor.pending_reason.as_ref().unwrap().kind,
        PendingKind::UserDecision
    );
    let pending = f.pending_of("t6c").await;
    assert_eq!(
        pending.context.as_ref().and_then(|c| c.kind.as_deref()),
        Some("test_code_issue"),
        "非复检的直接 code_issue 带 context.kind = test_code_issue"
    );
    // 动作集同权威表 test_code_issue 行
    let actions = f.store.allowed_actions_for_task("t6c").await.unwrap();
    let labels: Vec<&str> = actions.iter().map(|a| a.label.as_str()).collect();
    assert_eq!(labels, vec!["修改测试用例", "修改业务代码"]);
}

#[tokio::test]
async fn e2e_06b_gate_failure_code_issue_pends_for_user_then_goto_develop() {
    let f = Flow::new().await;
    let gate = write_stateful_gate(&f, "gate", &[2], "GATE_FAIL_OUTPUT: unit test broke");
    set_test_framework(&f, &gate).await;

    let mut script = Script::new();
    full_pass_script(&mut script, "t6b");
    f.agent.set_script(script);
    testkit::seed_task(&f.store, "t6b", "p1").await.unwrap();
    f.admit("t6b").await;
    f.executor.run("t6b").await.unwrap();

    // run #2：复检报告存在 code_issue → 交用户决定（决策 85）
    let mut recheck = Script::new();
    recheck
        .for_node(Stage::Test, Node::Execute)
        .write_file("test-report.md", "# 复检\n")
        .submit(&TestResult {
            passed: false,
            test_report_path: Some("test-report.md".into()),
            failures: vec![TestFailure {
                test_name: "login".into(),
                error_message: "业务代码未实现".into(),
                failure_cause: FailureCause::CodeIssue,
            }],
            gate_recheck: false,
        });
    f.agent.set_script(recheck);
    let cursor = f.sole_cursor("t6b").await;
    agentpipeline_core::pipeline::resume::apply_action(
        &f.store,
        &cursor,
        ResumeAction::Continue,
        None,
        None,
    )
    .await
    .unwrap();
    f.executor.run("t6b").await.unwrap();

    let cursor = f.sole_cursor("t6b").await;
    assert_eq!(
        (cursor.stage, cursor.node),
        (Stage::Test, Node::ValidateOutput),
        "纯代码闸门在 validate_output 收口"
    );
    assert_eq!(
        cursor.pending_reason.as_ref().unwrap().kind,
        PendingKind::UserDecision,
        "存在 code_issue → pending(user_decision)"
    );

    // 决策 130 ① / 票 05：test 闸门 code_issue 带 `context.kind`，动作集落到
    // 「修改测试用例 / 修改业务代码」而非通用兜底行。本场景的 test 是**被 merge
    // 测试闸门打回后的复检**（`gate_recheck = true`，决策 109）→ kind = gate_recheck。
    let pending = f.pending_of("t6b").await;
    assert_eq!(
        pending.context.as_ref().and_then(|c| c.kind.as_deref()),
        Some("gate_recheck"),
        "闸门复检路径带 context.kind = gate_recheck"
    );
    let actions = f.store.allowed_actions_for_task("t6b").await.unwrap();
    let labels: Vec<&str> = actions.iter().map(|a| a.label.as_str()).collect();
    assert_eq!(labels, vec!["修改测试用例", "修改业务代码"]);
    let t0 = actions[0].target.as_ref().expect("goto 应带落点");
    assert_eq!(
        (t0.stage, t0.node),
        (Stage::Test, Node::Execute),
        "修改测试用例回 test.execute"
    );
    let t1 = actions[1].target.as_ref().expect("goto 应带落点");
    assert_eq!(
        (t1.stage, t1.node),
        (Stage::Develop, Node::Execute),
        "修改业务代码回 develop.execute"
    );

    // 用户裁决「修改业务代码」→ goto develop.execute（决策 85 动作集）
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
}

// ─────────────────────────── 票 08：retry_exhausted 回架构设计 ───────────────────────────

/// decision 138 / 票 08：develop 的 `retry_exhausted` 选「带失败摘要回架构设计修订」时，
/// 系统把重试历史写任务目录 `retry-feedback.md`，architect 重入的 prompt 注入该内容。
#[tokio::test]
async fn retry_exhausted_to_architect_writes_retry_feedback_and_injects_on_reentry() {
    let f = Flow::new().await;
    // 设计通过；develop.execute 脚本耗尽 → agent_retry_max 次重试全无元数据 → pending(retry_exhausted)
    let mut script = Script::new();
    design_ok(&mut script);
    f.agent.set_script(script);
    testkit::seed_task(&f.store, "t8r", "p1").await.unwrap();
    f.admit("t8r").await;
    f.executor.run("t8r").await.unwrap();

    let cursor = f.sole_cursor("t8r").await;
    assert_eq!((cursor.stage, cursor.node), (Stage::Develop, Node::Execute));
    assert_eq!(
        cursor.pending_reason.as_ref().unwrap().kind,
        PendingKind::RetryExhausted
    );
    // 动作集含「带失败摘要回架构设计修订」（决策 138）
    let actions = f.store.allowed_actions_for_task("t8r").await.unwrap();
    let labels: Vec<&str> = actions.iter().map(|a| a.label.as_str()).collect();
    assert!(
        labels.contains(&"带失败摘要回架构设计修订"),
        "动作集应含回架构设计修订：{labels:?}"
    );

    // 选该动作 → goto architect-design.validate_input
    let goto = actions
        .iter()
        .find(|a| a.label == "带失败摘要回架构设计修订")
        .unwrap();
    let target = goto.target.as_ref().expect("goto 应带落点");
    agentpipeline_core::pipeline::resume::apply_action(
        &f.store,
        &cursor,
        ResumeAction::Goto,
        Some((target.stage, target.node)),
        None,
    )
    .await
    .unwrap();

    // 决策 138：与游标置位同事务写入 retry-feedback.md（先写文件再动游标）
    let feedback_path = f.home.home().task_file("t8r", "retry-feedback.md");
    assert!(feedback_path.exists(), "retry-feedback.md 应已落盘");
    let feedback = std::fs::read_to_string(&feedback_path).unwrap();
    assert!(
        feedback.contains("重试历史摘要") && feedback.contains("attempt"),
        "摘要应含各次 attempt 记录：{feedback}"
    );
    let after = f.store.get_cursor(&cursor.cursor_id).await.unwrap();
    assert_eq!(
        (after.stage, after.node),
        (Stage::ArchitectDesign, Node::ValidateInput)
    );

    // architect 重入：prompt 注入 retry-feedback（首轮不渲染，已在首轮断言隐含）
    let mut reentry = Script::new();
    reentry
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&agentpipeline_core::types::ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    f.agent.set_script(reentry);
    f.executor.run("t8r").await.unwrap();

    let vi_prompts: Vec<String> = f
        .requests_for(Stage::ArchitectDesign, Node::ValidateInput)
        .into_iter()
        .map(|r| r.user_prompt)
        .collect();
    assert!(
        !vi_prompts[0].contains("重试历史摘要"),
        "首轮（无 retry-feedback.md）不渲染该段：{}",
        vi_prompts[0]
    );
    assert!(
        vi_prompts
            .iter()
            .skip(vi_prompts.len() - 1)
            .any(|p| p.contains("重试历史摘要")),
        "重入 prompt 应注入重试历史摘要：{vi_prompts:?}"
    );
}

// ─────────────────────────── E2E-07 ───────────────────────────

#[tokio::test]
async fn e2e_07_merge_lint_gate_failure_kicks_back_develop_without_test() {
    let f = Flow::new().await;
    // lint 第 1 次（develop 闸门）通过，第 2 次（merge 闸门）失败
    let lint = write_stateful_gate(&f, "lint", &[2], "LINT_FAIL: unused import");
    f.store
        .update_project("p1", None, None, None, Some(&lint))
        .await
        .unwrap();

    let mut script = Script::new();
    full_pass_script(&mut script, "t7");
    f.agent.set_script(script);
    testkit::seed_task(&f.store, "t7", "p1").await.unwrap();
    f.admit("t7").await;
    f.executor.run("t7").await.unwrap();

    let merge = f.store.merge_metadata("t7").await.unwrap().unwrap();
    assert_eq!(merge.gate, Some(Gate::Fail));
    assert_eq!(
        merge.gate_failure_kind,
        Some(GateFailureKind::Lint),
        "失败类型记 lint（决策 139）"
    );
    assert_eq!(merge.gate_failures, 1, "lint 与测试统一累加（决策 108）");
    let transitions = f.store.list_transitions("t7").await.unwrap();
    assert!(
        transitions.iter().any(|t| t.to_stage == Stage::Develop
            && t.to_node == Node::Execute
            && t.trigger == TransitionTrigger::Kickback),
        "lint 失败应直接打回 develop.execute：{transitions:?}"
    );
    assert!(
        !transitions
            .iter()
            .any(|t| t.to_stage == Stage::Test && t.trigger == TransitionTrigger::Kickback),
        "lint 失败不得经 test.execute"
    );
    // 打回后 develop.execute 脚本耗尽 → pending；attempts 跨阶段归零
    let cursor = f.sole_cursor("t7").await;
    assert_eq!((cursor.stage, cursor.node), (Stage::Develop, Node::Execute));
    assert_eq!(cursor.validate_attempts, 0, "打回重置 attempts（决策 43）");
    assert_eq!(
        cursor.pending_reason.as_ref().unwrap().kind,
        PendingKind::RetryExhausted
    );
}

// ─────────────────────────── E2E-08 ───────────────────────────

#[tokio::test]
async fn e2e_08_exhausts_gate_failures_then_retry_resets_worktree_and_readmits() {
    let f = Flow::new().await;
    // develop 闸门通过；merge 测试闸门从第 2 次起永远失败
    let gate = write_stateful_gate(
        &f,
        "gate",
        &[2, 3, 4, 5, 6, 7, 8],
        "GATE_FAIL_OUTPUT: flaky",
    );
    set_test_framework(&f, &gate).await;

    let mut script = Script::new();
    full_pass_script(&mut script, "t8");
    f.agent.set_script(script);
    testkit::seed_task(&f.store, "t8", "p1").await.unwrap();
    f.admit("t8").await;

    // 三个循环把 gate_failures 推到上限 3：每轮 test.execute 复检通过后 merge 闸门再失败
    let mut gate_failures_seen = Vec::new();
    for cycle in 0..3 {
        if cycle > 0 {
            let mut recheck = Script::new();
            recheck
                .for_node(Stage::Test, Node::Execute)
                .submit(&TestResult {
                    passed: true,
                    test_report_path: Some("test-report.md".into()),
                    failures: vec![],
                    gate_recheck: false,
                });
            f.agent.set_script(recheck);
            let cursor = f.sole_cursor("t8").await;
            agentpipeline_core::pipeline::resume::apply_action(
                &f.store,
                &cursor,
                ResumeAction::Continue,
                None,
                None,
            )
            .await
            .unwrap();
        }
        f.executor.run("t8").await.unwrap();
        gate_failures_seen.push(
            f.store
                .merge_metadata("t8")
                .await
                .unwrap()
                .unwrap()
                .gate_failures,
        );
    }
    assert_eq!(
        gate_failures_seen,
        vec![1, 2, 3],
        "闸门计数逐轮累加（决策 108）"
    );

    // 耗尽 → pending(retry_exhausted) 挂在 merge 游标，无 skip（决策 85 / 86）
    let cursor = f.sole_cursor("t8").await;
    assert_eq!((cursor.stage, cursor.node), (Stage::Merge, Node::Execute));
    let reason = cursor.pending_reason.as_ref().unwrap();
    assert_eq!(reason.kind, PendingKind::RetryExhausted);
    let actions = f.store.allowed_actions_for_task("t8").await.unwrap();
    let names: Vec<&str> = actions.iter().map(|a| a.action.as_str()).collect();
    assert!(names.contains(&"goto") && names.contains(&"cancel"));
    assert!(!names.contains(&"skip"), "merge 无 skip（决策 86 / 122）");

    // 用户终止 → failed（v1 无 failed 生产者，此处以 mark_terminal 显式建模）
    f.store
        .mark_terminal("t8", TaskStatus::Failed)
        .await
        .unwrap();
    assert_eq!(
        f.store.get_task("t8").await.unwrap().status,
        TaskStatus::Failed
    );

    // retry：旧游标归档 + 新 main、worktree 硬重置 + clean、置 queued 重新准入（决策 125 / 117）
    let worktree = f.home.home().worktree_path("t8");
    std::fs::write(worktree.join("half_done.txt"), "残留").unwrap();
    f.store.reset_cursors_to_init("t8").await.unwrap();
    Git.reset_hard_clean(&worktree, "main").await.unwrap();
    f.store
        .set_task_status("t8", TaskStatus::Queued)
        .await
        .unwrap();

    let live = f.sole_cursor("t8").await;
    assert_eq!((live.stage, live.node), (Stage::Init, Node::Execute));
    assert!(
        !worktree.join("half_done.txt").exists(),
        "clean -fdx 清残留"
    );
    let all = f.store.load_all_cursors("t8").await.unwrap();
    assert!(all.len() > 1, "旧游标归档保留、新 main 插入（决策 113）");
    assert_eq!(
        all.iter()
            .filter(|c| c.status == agentpipeline_core::types::CursorStatus::Archived)
            .count(),
        all.len() - 1
    );

    let report = f.scheduler().tick().await.unwrap();
    assert_eq!(report.admitted, vec!["t8".to_string()]);
    assert_eq!(
        f.store.get_task("t8").await.unwrap().status,
        TaskStatus::Running
    );
}

// ────────────── 空分支改道与零变更申报确认（决策 391 / 票 commit-contract 01）──────────────
//
// 原缺陷：develop 全绿但分支相对基准零差异，merge 把「diff 为空」误分类成 Test，
// 打回 test 复检造不出分支提交，空转一轮再交用户。决策 391 把空分支改道回 develop。

/// 决策 391：分支有自有提交（空提交）但相对基准**净差异 0 文件** → merge 判 EmptyBranch，
/// 直接打回 develop.execute，**不进 test 复检**；gate_failures 照决策 108 累计。
#[tokio::test]
async fn e2e_merge_empty_branch_kicks_back_to_develop_without_test() {
    let f = Flow::new().await;
    let mut script = Script::new();
    design_ok(&mut script);
    // develop.execute：造一个**空提交**——`rev-list` 自有提交数 > 0（过 develop 守卫），
    // 但相对基准的净差异是 0 文件（正是 merge 撞上的空分支现场）。
    script
        .for_node(Stage::Develop, Node::Execute)
        .run_command(
            "git -c user.name=f -c user.email=f@l commit --allow-empty -m 'chore: noop commit'",
        )
        .submit(&CodeChanges {
            branch_name: "kanban/t-eb".into(),
            changed_files: vec![],
            unit_test_files: vec![],
            no_changes: false,
        });
    script
        .for_node(Stage::Review, Node::Execute)
        .write_file("review-report.md", "# 评审报告\n通过\n")
        .submit(&ReviewResult {
            approved: true,
            review_report_path: Some("review-report.md".into()),
            required_changes: vec![],
        });
    script
        .for_node(Stage::Test, Node::Execute)
        .write_file("test-report.md", "# 测试报告\n通过\n")
        .submit(&TestResult {
            passed: true,
            test_report_path: Some("test-report.md".into()),
            failures: vec![],
            gate_recheck: false,
        });
    f.agent.set_script(script);
    testkit::seed_task(&f.store, "t-eb", "p1").await.unwrap();
    f.admit("t-eb").await;

    f.executor.run("t-eb").await.unwrap();

    let merge = f.store.merge_metadata("t-eb").await.unwrap().unwrap();
    assert_eq!(merge.gate, Some(Gate::Fail));
    assert_eq!(
        merge.gate_failure_kind,
        Some(GateFailureKind::EmptyBranch),
        "空分支必须归 EmptyBranch，不得冒充 Test"
    );
    assert_eq!(merge.gate_failures, 1, "统一累加（决策 108）");

    let transitions = f.store.list_transitions("t-eb").await.unwrap();
    assert!(
        transitions.iter().any(|t| t.to_stage == Stage::Develop
            && t.to_node == Node::Execute
            && t.trigger == TransitionTrigger::Kickback),
        "空分支应直接打回 develop.execute：{transitions:?}"
    );
    assert!(
        !transitions.iter().any(|t| t.to_stage == Stage::Test
            && t.to_node == Node::Execute
            && t.trigger == TransitionTrigger::Kickback),
        "空分支不得经 test 复检（修用例造不出分支提交）：{transitions:?}"
    );

    // 事实段与 develop 守卫同源落盘（重入 develop.execute 时注入）
    let facts = std::fs::read_to_string(f.home.home().task_file("t-eb", "zero-commit-facts.md"))
        .expect("merge 空分支应落零提交事实段");
    assert!(facts.contains("净差异"), "{facts}");
    assert!(facts.contains("git commit"), "{facts}");
}

/// 决策 391：空分支且 develop 已申报 `no_changes` → merge 挂 pending(user_decision)
/// 交用户确认（不写闸门失败、不烧 gate_failures）。
///
/// 正常流水线在 develop 就被守卫截住（挂同样的 pending），本支是**防御性**路径：
/// 直接从 merge 阶段入口构造，验证即使走到 merge，申报也仍被尊重。
#[tokio::test]
async fn e2e_merge_declared_no_changes_pends_for_user() {
    let f = Flow::new().await;
    let mut script = Script::new();
    design_ok(&mut script);
    script
        .for_node(Stage::Develop, Node::Execute)
        .submit(&CodeChanges {
            branch_name: "kanban/t-dc".into(),
            changed_files: vec![],
            unit_test_files: vec![],
            no_changes: true,
        });
    f.agent.set_script(script);
    testkit::seed_task(&f.store, "t-dc", "p1").await.unwrap();
    f.admit("t-dc").await;
    f.executor.run("t-dc").await.unwrap();

    // develop 已申报零变更（挂 pending）；把游标前移到 merge 阶段入口
    let cursor = f.sole_cursor("t-dc").await;
    f.store
        .clear_cursor_pending(&cursor.cursor_id)
        .await
        .unwrap();
    f.store
        .set_cursor_stage(&cursor.cursor_id, Stage::Merge, Node::Execute)
        .await
        .unwrap();
    f.executor.run("t-dc").await.unwrap();

    let live = f.store.load_live_cursors("t-dc").await.unwrap();
    let reason = live[0].pending_reason.as_ref().expect("应挂 pending");
    assert_eq!(reason.kind, PendingKind::UserDecision);
    // pending 必须带 `context.kind`：否则 `allowed_actions` 落到通用兜底行 {skip, cancel}
    assert_eq!(
        reason.context.as_ref().and_then(|c| c.kind.as_deref()),
        Some("zero_changes")
    );
    // 申报是诚实结论：**不写 merge_result**（也就没有闸门失败行、不烧 gate_failures）
    assert!(
        f.store.merge_metadata("t-dc").await.unwrap().is_none(),
        "申报零变更不得写闸门失败行"
    );
    // 动作集恰为 {goto develop.execute, cancel}——通用兜底行会给 skip，故这一条能抓住
    // 「没带 context.kind」这个缺陷
    let actions = f.store.allowed_actions_for_task("t-dc").await.unwrap();
    let names: Vec<&str> = actions.iter().map(|a| a.action.as_str()).collect();
    assert_eq!(names, vec!["goto", "cancel"], "{actions:?}");
    let target = actions[0].target.as_ref().expect("goto 应有落点");
    assert_eq!((target.stage, target.node), (Stage::Develop, Node::Execute));
}
