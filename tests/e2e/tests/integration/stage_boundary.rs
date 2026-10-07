//! 阶段内容边界（决策 395 / 396 / 397，票 stage-content-boundary）。
//!
//! 单元层已钉住机制本身（file_policy 白名单判定 / tool_defs 过滤 / run_command
//! 执行点拒绝 / 申报比对的纯函数）；这里钉的是**装配进真实流水线后**的三条端到端
//! 行为：越界写入不落盘且流程照常推进、硬发 run_command 不产生副作用、漏报在
//! develop 当轮被拦而不穿越 review / test。

use agentpipeline_core::types::{
    CodeChanges, FileAction, FileChangeSpec, Node, ReviewResult, Stage, TestResult,
};
use testkit::Script;

use crate::common::Flow;

fn design_and_develop(script: &mut Script, task_id: &str, declare_tests: bool) {
    crate::common::design_ok(script);
    let mut changed = vec![FileChangeSpec {
        path: "src/lib.rs".into(),
        action: FileAction::Create,
        content_hash: None,
    }];
    if declare_tests {
        changed.push(FileChangeSpec {
            path: "tests/acceptance.rs".into(),
            action: FileAction::Create,
            content_hash: None,
        });
    }
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
            changed_files: changed,
            unit_test_files: vec![],
            no_changes: false,
        });
}

fn review_pass(script: &mut Script) {
    script
        .for_node(Stage::Review, Node::Execute)
        .write_file(
            "review-report.md",
            "# 评审报告\n## 设计符合性\n通过\n## 测试质量\n通过\n",
        )
        .submit(&ReviewResult {
            approved: true,
            review_report_path: Some("review-report.md".into()),
            required_changes: vec![],
        });
}

fn test_pass(script: &mut Script) {
    script
        .for_node(Stage::Test, Node::Execute)
        .write_file("test-report.md", "# 测试报告\n全部通过\n")
        .submit(&TestResult {
            passed: true,
            test_report_path: Some("test-report.md".into()),
            failures: vec![],
            gate_recheck: false,
        });
}

/// 决策 395：review 越界写任务目录的其他文件 → 文件策略当场拒绝（不落盘），
/// 节点照常收口、报告照常落盘、流程照常推进——「拦下越界」不等于「卡死任务」。
#[tokio::test]
async fn e2e_review_out_of_scope_write_is_denied_and_flow_continues() {
    let f = Flow::new().await;
    let mut script = Script::new();
    design_and_develop(&mut script, "t-sb1", true);
    // review 先越界写一个未授权文件（会被白名单拒），再写合法产出并放行
    script
        .for_node(Stage::Review, Node::Execute)
        .write_file("notes.md", "越界尝试：不应落盘")
        .write_file(
            "review-report.md",
            "# 评审报告\n## 设计符合性\n通过\n## 测试质量\n通过\n",
        )
        .submit(&ReviewResult {
            approved: true,
            review_report_path: Some("review-report.md".into()),
            required_changes: vec![],
        });
    test_pass(&mut script);
    f.agent.set_script(script);
    testkit::seed_task(&f.store, "t-sb1", "p1").await.unwrap();
    f.admit("t-sb1").await;
    f.executor.run("t-sb1").await.unwrap();

    // 合法产出落了盘，越界的没落（任务目录与 worktree 都没有）
    let task_dir = f.home.home().task_dir("t-sb1");
    assert!(
        task_dir.join("review-report.md").exists(),
        "review-report.md 应照常落盘"
    );
    assert!(
        !task_dir.join("notes.md").exists(),
        "越界写入不得落进任务目录"
    );
    let worktree = f.home.home().worktree_path("t-sb1");
    assert!(
        !worktree.join("notes.md").exists(),
        "越界写入不得经 worktree 落盘"
    );
    // 流程推进到 merge 等审批（review 放行、test 通过）
    let live = f.sole_cursor("t-sb1").await;
    assert_eq!(
        (live.stage, live.node),
        (Stage::Merge, Node::Execute),
        "越界写入只拒那一次调用，不得卡死任务"
    );
}

/// 决策 396：review 硬发 `run_command`（def 层没广告它）→ 执行层兜底拒绝，
/// 命令不执行（无副作用文件），节点照常收口。
#[tokio::test]
async fn e2e_review_run_command_is_stage_denied_without_side_effects() {
    let f = Flow::new().await;
    let mut script = Script::new();
    design_and_develop(&mut script, "t-sb2", true);
    script
        .for_node(Stage::Review, Node::Execute)
        .run_command("echo pwned > pwned.txt")
        .write_file(
            "review-report.md",
            "# 评审报告\n## 设计符合性\n通过\n## 测试质量\n通过\n",
        )
        .submit(&ReviewResult {
            approved: true,
            review_report_path: Some("review-report.md".into()),
            required_changes: vec![],
        });
    test_pass(&mut script);
    f.agent.set_script(script);
    testkit::seed_task(&f.store, "t-sb2", "p1").await.unwrap();
    f.admit("t-sb2").await;
    f.executor.run("t-sb2").await.unwrap();

    let task_dir = f.home.home().task_dir("t-sb2");
    let worktree = f.home.home().worktree_path("t-sb2");
    assert!(
        !worktree.join("pwned.txt").exists() && !task_dir.join("pwned.txt").exists(),
        "被拒的命令不得产生任何副作用文件"
    );
    let live = f.sole_cursor("t-sb2").await;
    assert_eq!(
        (live.stage, live.node),
        (Stage::Merge, Node::Execute),
        "执行层拒绝只挡那一次调用，节点照常收口"
    );
}

/// 决策 397：develop 漏报文件（写了 tests/acceptance.rs 但没申报）→ validate_output
/// 当轮确定性打回（不穿越 review / test），事实段落盘供重入注入。
#[tokio::test]
async fn e2e_undeclared_changes_kick_back_at_develop_without_crossing_review() {
    let f = Flow::new().await;
    let mut script = Script::new();
    design_and_develop(&mut script, "t-sb3", false);
    // 顺带造一个命中噪音过滤的未申报文件（lockfile）：它不该进漏报清单，
    // 但要在事实段的「已忽略」小节留痕（票 03 改动二）。
    script
        .for_node(Stage::Develop, Node::Execute)
        .write_file("Cargo.lock", "# lockfile drift\n");
    f.agent.set_script(script);
    testkit::seed_task(&f.store, "t-sb3", "p1").await.unwrap();
    f.admit("t-sb3").await;
    f.executor.run("t-sb3").await.unwrap();

    // 打回 develop.execute 重试（不是 test 复检、不是 pending 交人）
    let transitions = f.store.list_transitions("t-sb3").await.unwrap();
    assert!(
        transitions.iter().any(|t| t.to_stage == Stage::Develop
            && t.to_node == Node::Execute
            && t.trigger == agentpipeline_core::types::TransitionTrigger::NodeRetry),
        "漏报应回 develop.execute 重试：{transitions:?}"
    );
    assert!(
        !transitions
            .iter()
            .any(|t| t.to_stage == Stage::Review || t.to_stage == Stage::Test),
        "漏报不得穿越 review / test：{transitions:?}"
    );
    // 事实段落盘（重入 develop.execute 时读回注入）
    let facts = std::fs::read_to_string(
        f.home
            .home()
            .task_file("t-sb3", "undeclared-changes-facts.md"),
    )
    .expect("漏报应落事实段文件");
    assert!(facts.contains("tests/acceptance.rs"), "{facts}");
    assert!(
        !facts.contains("src/lib.rs"),
        "已申报的不在漏报清单里：{facts}"
    );
    // 噪音留痕：lockfile 未申报但命中过滤——不进漏报判定，却出现在「已忽略」小节
    assert!(facts.contains("declare_ignore_globs"), "{facts}");
    assert!(facts.contains("Cargo.lock"), "{facts}");
}

/// 决策 397 的噪音容忍：只动了 lockfile（未申报）→ 命中缺省噪音过滤 → 不假红，
/// develop 照常放行（106 实证：假红空转的代价是跨阶段两小时）。
#[tokio::test]
async fn e2e_lockfile_drift_without_declaration_does_not_trip_the_gate() {
    let f = Flow::new().await;
    let mut script = Script::new();
    crate::common::design_ok(&mut script);
    script
        .for_node(Stage::Develop, Node::Execute)
        .write_file("Cargo.lock", "# lockfile drift\n")
        .run_command(
            "git add -A && git -c user.name=f -c user.email=f@l commit -m 'chore: lockfile'",
        )
        .submit(&CodeChanges {
            branch_name: "kanban/t-sb4".into(),
            changed_files: vec![],
            unit_test_files: vec![],
            no_changes: false,
        });
    review_pass(&mut script);
    test_pass(&mut script);
    f.agent.set_script(script);
    testkit::seed_task(&f.store, "t-sb4", "p1").await.unwrap();
    f.admit("t-sb4").await;
    f.executor.run("t-sb4").await.unwrap();

    let live = f.sole_cursor("t-sb4").await;
    assert_eq!(
        (live.stage, live.node),
        (Stage::Merge, Node::Execute),
        "lockfile 漂移未申报不得打回 develop（噪音过滤）"
    );
    assert!(
        !f.home
            .home()
            .task_file("t-sb4", "undeclared-changes-facts.md")
            .exists(),
        "噪音被过滤时不落漏报事实段"
    );
}
