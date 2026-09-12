//! E2E 闸门场景（testing.md §8）：E2E-06a / 06b / 07 / 08。
//!
//! 覆盖决策 85 / 86 / 108 / 109 / 125 / 139：merge 测试闸门失败跳 test.execute 复检、
//! code_issue 交用户、lint 失败直接打回 develop、gate_failures 耗尽收口与 retry 复位。

mod common;

use agentpipeline_core::git::Git;
use agentpipeline_core::storage::decisions::ResumeAction;
use agentpipeline_core::types::{
    Approval, FailureCause, Gate, GateFailureKind, Node, PendingKind, Stage, TaskStatus,
    TestFailure, TestResult, TransitionTrigger,
};
use common::{full_pass_script, Flow};
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
    // 第 1 次（develop 闸门）通过，第 2 次（merge 闸门首跑）失败，第 3 次（复检后重跑）通过
    let gate = write_stateful_gate(&f, "gate", &[2], "GATE_FAIL_OUTPUT: test_login failed");
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
                && r.user_prompt.contains("GATE_FAIL_OUTPUT")),
        "复检 prompt 应含闸门输出：{:?}",
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
    f.store
        .apply_resume(&cursor, ResumeAction::Continue, None, None)
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
    f.store
        .apply_resume(&cursor, ResumeAction::Continue, None, None)
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

    // 用户裁决「修改业务代码」→ goto develop.execute（决策 85 动作集）
    f.store
        .apply_resume(
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
            f.store
                .apply_resume(&cursor, ResumeAction::Continue, None, None)
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
