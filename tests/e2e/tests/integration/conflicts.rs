//! E2E 冲突 / 合入场景（testing.md §8）：E2E-05 冲突打回、E2E-10 脏工作区合入、
//! E2E-17 conflict_wait、E2E-18 duplicate_risk。
//!
//! 决策 60 / 67 / 71 / 74 / 102 / 120 / 132。

use super::common::{design_ok, Flow};
use agentpipeline_core::git::Git;
use agentpipeline_core::pipeline::pseudo::ConflictCheckResult;
use agentpipeline_core::storage::decisions::{MergeDecision, ResumeAction};
use agentpipeline_core::types::{
    Approval, ArchitectExecuteMetadata, DuplicateRisk, NewSymbol, Node, PendingKind, Stage,
    SymbolKind, TaskStatus, TestResult, TransitionTrigger, ValidateInputMetadata,
};
use testkit::{Repo, Script};

// ─────────────────────────── E2E-05 ───────────────────────────

#[tokio::test]
async fn e2e_05_merge_rebase_conflict_kicks_back_develop_with_conflict_files() {
    let repo = Repo::clean().unwrap();
    repo.write("shared.txt", "base\n");
    repo.commit_all("chore: shared base");
    let f = Flow::with_repo(repo).await;

    // run #1：develop 在任务分支改 shared.txt 并提交；test.execute 不给脚本 → 暂停在 test
    let mut script = Script::new();
    design_ok(&mut script);
    script
        .for_node(Stage::Develop, Node::Execute)
        .write_file("shared.txt", "topic\n")
        .run_command(
            "git add -A && git -c user.name=f -c user.email=f@l commit -m 'feat: topic 改这一行'",
        )
        .submit(&agentpipeline_core::types::CodeChanges {
            branch_name: "kanban/t5".into(),
            changed_files: vec![],
            unit_test_files: vec![],
            no_changes: false,
        });
    script.for_node(Stage::Review, Node::Execute).submit(
        &agentpipeline_core::types::ReviewResult {
            approved: true,
            review_report_path: Some("review-report.md".into()),
            required_changes: vec![],
        },
    );
    f.agent.set_script(script);
    testkit::seed_task(&f.store, "t5", "p1").await.unwrap();
    f.admit("t5").await;
    f.executor.run("t5").await.unwrap();
    assert_eq!(
        f.sole_cursor("t5").await.stage,
        Stage::Test,
        "暂停在 test.execute（脚本耗尽）"
    );

    // 基准前移且同文件同区域改动 → rebase 不可自动合并
    f.repo.advance_main("shared.txt", "main\n");

    // run #2：test 通过 → merge 阶段 A rebase 冲突 → rebase --abort → 打回 develop
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
    let test_cursor = f.sole_cursor("t5").await;
    agentpipeline_core::pipeline::resume::apply_action(
        &f.store,
        &test_cursor,
        ResumeAction::Continue,
        None,
        None,
    )
    .await
    .unwrap();
    f.executor.run("t5").await.unwrap();

    let transitions = f.store.list_transitions("t5").await.unwrap();
    let kickback = transitions
        .iter()
        .find(|t| {
            t.to_stage == Stage::Develop
                && t.to_node == Node::Execute
                && t.trigger == TransitionTrigger::Kickback
        })
        .expect("应有 merge → develop 的冲突打回");
    assert!(
        kickback
            .reason
            .as_deref()
            .unwrap_or("")
            .contains("shared.txt"),
        "打回原因应含冲突文件：{:?}",
        kickback.reason
    );

    // 打回落点 develop.execute，attempts 归零；冲突后 worktree 已恢复干净（决策 74）
    let cursor = f.sole_cursor("t5").await;
    assert_eq!((cursor.stage, cursor.node), (Stage::Develop, Node::Execute));
    assert_eq!(cursor.validate_attempts, 0);
    let worktree = f.home.home().worktree_path("t5");
    assert!(
        !Git.is_dirty(&worktree).await.unwrap(),
        "rebase --abort 后 worktree 应干净"
    );
}

// ─────────────────────────── E2E-10 ───────────────────────────

#[tokio::test]
async fn e2e_10_dirty_worktree_pends_for_user_and_blocks_merge() {
    let f = Flow::new().await;
    let mut script = Script::new();
    super::common::full_pass_script(&mut script, "t10");
    f.agent.set_script(script);
    testkit::seed_task(&f.store, "t10", "p1").await.unwrap();
    f.admit("t10").await;
    f.executor.run("t10").await.unwrap();
    let merge = f.store.merge_metadata("t10").await.unwrap().unwrap();
    assert_eq!(merge.approval, Approval::Pending);

    let main_before = f.repo.head("main");
    // 目标分支工作区变脏
    f.repo.dirty_worktree().unwrap();
    assert!(
        Git.is_dirty(f.repo.path()).await.unwrap(),
        "fixture 工作区应为脏"
    );
    f.store
        .apply_merge_decision("t10", MergeDecision::Approve)
        .await
        .unwrap();
    f.executor.run("t10").await.unwrap();

    // pending(user_decision, context.kind = dirty_worktree)（决策 61 / 132）
    let cursor = f.sole_cursor("t10").await;
    assert_eq!(
        cursor.status,
        agentpipeline_core::types::CursorStatus::Pending,
        "脏工作区必须挂 pending"
    );
    let reason = cursor.pending_reason.as_ref().unwrap();
    assert_eq!(reason.kind, PendingKind::UserDecision);
    assert_eq!(
        reason.context.as_ref().unwrap().kind.as_deref(),
        Some("dirty_worktree")
    );
    let actions = f.store.allowed_actions_for_task("t10").await.unwrap();
    let names: Vec<&str> = actions.iter().map(|a| a.action.as_str()).collect();
    assert_eq!(
        names,
        vec!["continue", "cancel"],
        "动作集 {{continue, cancel}}"
    );

    // 未合入：主干未前进、merge 状态仍待审批（不静默合入）
    assert_eq!(f.repo.head("main"), main_before, "脏工作区不得合入");
    assert_eq!(
        f.store.merge_metadata("t10").await.unwrap().unwrap().status,
        agentpipeline_core::types::MergeStatus::PendingApproval
    );
    assert_ne!(
        f.store.get_task("t10").await.unwrap().status,
        TaskStatus::Done
    );
    // 挂起后游标必须仍停在 merge.execute（决策 61 / 132）：
    // 挂起不得被 route_merge 按 approval=approved 放行推进到 done。
    assert_eq!(cursor.stage, Stage::Merge, "挂起后游标不得离开 merge");
    assert_eq!(cursor.node, Node::Execute);

    // continue → 工作区恢复干净 → 本次 continue 即完成合入并进入终态（决策 132）
    std::fs::remove_file(f.repo.path().join("uncommitted.txt")).unwrap();
    assert!(!Git.is_dirty(f.repo.path()).await.unwrap());
    let cursor = f.sole_cursor("t10").await;
    agentpipeline_core::pipeline::resume::apply_action(
        &f.store,
        &cursor,
        ResumeAction::Continue,
        None,
        None,
    )
    .await
    .unwrap();
    f.executor.run("t10").await.unwrap();

    assert_eq!(
        f.store.merge_metadata("t10").await.unwrap().unwrap().status,
        agentpipeline_core::types::MergeStatus::Merged,
        "continue 后应完成合入"
    );
    assert_ne!(f.repo.head("main"), main_before, "continue 后主干应前进");
    assert_eq!(
        f.store.get_task("t10").await.unwrap().status,
        TaskStatus::Done
    );
}

// ─────────────────────────── E2E-17 ───────────────────────────

#[tokio::test]
async fn e2e_17_conflict_wait_yields_then_auto_recovers_when_all_terminal() {
    let f = Flow::new().await;
    testkit::seed_task(&f.store, "t-early", "p1").await.unwrap();
    f.clock.advance_secs(10);
    testkit::seed_task(&f.store, "t-late", "p1").await.unwrap();
    // t-early 已有 architect 产出（与 t-late 重叠文件）
    f.store
        .upsert_stage_output(
            "t-early",
            Stage::ArchitectDesign,
            "design_doc",
            "design.md",
            Some(&serde_json::json!({
                "readiness": true,
                "affected_files": ["src/shared.rs"],
                "new_symbols": []
            })),
        )
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
        .write_file("design.md", "# 设计\n")
        .submit(&ArchitectExecuteMetadata {
            readiness: true,
            affected_files: vec!["src/shared.rs".into()],
            new_symbols: vec![],
            ..Default::default()
        });
    f.agent.set_script(script);
    // 两条都准入（t-early 保持活跃，作为冲突对手）
    f.scheduler().tick().await.unwrap();
    f.executor.run("t-late").await.unwrap();

    // 晚者让步：pending(conflict_wait)，context 记录全部冲突任务（决策 71 / 102）
    let cursor = f.sole_cursor("t-late").await;
    assert_eq!(
        (cursor.stage, cursor.node),
        (Stage::ArchitectDesign, Node::Execute)
    );
    let reason = cursor.pending_reason.as_ref().unwrap();
    assert_eq!(reason.kind, PendingKind::ConflictWait);
    assert_eq!(
        reason.context.as_ref().unwrap().conflict_task_ids,
        vec!["t-early".to_string()],
        "全量冲突任务 id"
    );

    // 对手终态 + 复检无交集 → 自动恢复并拉起 executor（决策 102）
    f.store
        .set_task_status("t-early", TaskStatus::Done)
        .await
        .unwrap();
    let report = f.scheduler().tick().await.unwrap();
    assert_eq!(report.conflict_resumed, vec!["t-late".to_string()]);
    let after = f.store.get_cursor(&cursor.cursor_id).await.unwrap();
    assert_eq!(
        after.status,
        agentpipeline_core::types::CursorStatus::Active
    );
    assert!(after.pending_reason.is_none());
    assert!(
        f.resumes.load(std::sync::atomic::Ordering::SeqCst) >= 1,
        "恢复后应拉起 executor"
    );
}

#[tokio::test]
async fn e2e_17_pure_name_overlap_is_only_a_warning() {
    // 决策 71② / 120：符号名相同但 module_path 不同 → 只 warning（Low），不算冲突
    let f = Flow::new().await;
    testkit::seed_task(&f.store, "w-early", "p1").await.unwrap();
    f.clock.advance_secs(10);
    testkit::seed_task(&f.store, "w-late", "p1").await.unwrap();
    for (task, module) in [("w-early", "crate::auth"), ("w-late", "crate::billing")] {
        f.store
            .upsert_stage_output(
                task,
                Stage::ArchitectDesign,
                "design_doc",
                "design.md",
                Some(&serde_json::json!({
                    "readiness": true,
                    "affected_files": [],
                    "new_symbols": [{
                        "name": "login",
                        "kind": "function",
                        "module_path": module,
                        "file_path": "src/x.rs"
                    }]
                })),
            )
            .await
            .unwrap();
    }
    let conflicts = f.store.first_layer_conflicts("w-late").await.unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(
        conflicts[0].duplicate_risk,
        Some(DuplicateRisk::Low),
        "纯 name 重合降级为 warning"
    );
    assert!(conflicts[0].overlapping_files.is_empty());

    // 执行器侧必须同样只告警、不挂 conflict_wait（决策 71② / 120）：
    // 跑完 architect.execute 后游标应分裂进设计分支，而不是停在 architect 等冲突。
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
            affected_files: vec![],
            new_symbols: vec![NewSymbol {
                name: "login".into(),
                kind: SymbolKind::Function,
                module_path: "crate::billing".into(),
                file_path: "src/x.rs".into(),
            }],
            ..Default::default()
        });
    f.agent.set_script(script);
    // 两条任务都在 queued，一次 tick 会一起放行；只跑 w-late（admit 助手断言「恰好一条」，这里不适用）
    let _ = f.scheduler().tick().await.unwrap();
    f.executor.run("w-late").await.unwrap();

    let cursors = f.live_cursors("w-late").await;
    let pending: Vec<_> = cursors
        .iter()
        .filter_map(|c| c.pending_reason.clone())
        .collect();
    assert!(
        !cursors
            .iter()
            .any(|c| c.pending_reason.as_ref().map(|r| r.kind) == Some(PendingKind::ConflictWait)),
        "纯 name 重合不得触发 conflict_wait：{pending:?}"
    );
    assert!(
        !cursors
            .iter()
            .any(|c| c.stage == Stage::ArchitectDesign && c.node == Node::Execute),
        "architect.execute 不得因纯 name 重合停住（只告警）：{:?}",
        cursors
            .iter()
            .map(|c| (c.stage, c.node, c.status))
            .collect::<Vec<_>>()
    );
}

// ─────────────────────────── E2E-18 ───────────────────────────

#[tokio::test]
async fn e2e_18_semantic_duplicate_risk_pends_with_goto_and_cancel() {
    let f = Flow::new().await;
    testkit::seed_task(&f.store, "d-early", "p1").await.unwrap();
    f.clock.advance_secs(10);
    testkit::seed_task(&f.store, "d-late", "p1").await.unwrap();
    // 同模块 auth、符号名不同、文件不重叠 → 触发第二层语义比对（决策 60 / 67）
    f.store
        .upsert_stage_output(
            "d-early",
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
                }]
            })),
        )
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
    f.agent.set_script(script);
    f.scheduler().tick().await.unwrap();
    f.executor.run("d-late").await.unwrap();

    let cursor = f.sole_cursor("d-late").await;
    let reason = cursor.pending_reason.as_ref().unwrap();
    assert_eq!(reason.kind, PendingKind::UserDecision);
    assert_eq!(
        reason.context.as_ref().unwrap().kind.as_deref(),
        Some("duplicate_risk")
    );
    assert!(reason
        .context
        .as_ref()
        .unwrap()
        .conflict_task_ids
        .contains(&"d-early".to_string()));

    // 动作集 {goto develop, cancel}；「合并任务」已移出（决策 132）
    let actions = f.store.allowed_actions_for_task("d-late").await.unwrap();
    let names: Vec<&str> = actions.iter().map(|a| a.action.as_str()).collect();
    assert_eq!(names, vec!["goto", "cancel"]);
    let goto = &actions[0];
    assert_eq!(goto.target.as_ref().unwrap().stage, Stage::Develop);
    assert_eq!(goto.target.as_ref().unwrap().node, Node::Execute);

    // 伪阶段 run 独立落库（决策 100 / 113）
    let runs = f.store.list_runs("d-late").await.unwrap();
    assert!(runs
        .iter()
        .any(|r| r.agent_type == "pseudo:conflict_check" && r.parent_run_id.is_some()));
}
