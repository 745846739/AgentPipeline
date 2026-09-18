//! E2E-13 崩溃恢复（testing.md §8，决策 80 / 113 / 127 / 152）。
//!
//! **全部 in-process**（决策 152，不 spawn 真二进制做 kill -9）：spawn `executor.run`
//! → 在指定节点的 LLM 调用处 `abort()` → 断言游标停在中断节点 → 清理
//! `executor_owner` 残留（决策 127）→ 重跑 → 从游标 checkpoint 续跑至终态。
//!
//! 覆盖：
//! - 单游标：中断在 develop.execute，重跑从该节点续跑到 done；
//! - 并行互不影响（决策 80）：develop-design 中断时 test-design 照常跑到 `waiting_join`；
//! - `waiting_join` 跨重启保留，另一分支就位后 join 恰执行一次（决策 107 / G5）；
//! - `executor_owner` 残留阻断再 claim，`clear_executor_owners` 后可重新准入；
//! - 节点级幂等重跑：已完成的上游节点不重跑，产出不重复（G8/G9）。

use super::common::Flow;
use agentpipeline_core::storage::decisions::MergeDecision;
use agentpipeline_core::types::{
    AcceptanceCriterion, ArchitectExecuteMetadata, CodeChanges, CursorStatus,
    DevelopDesignMetadata, Node, ReviewResult, Stage, TaskStatus, TestDesignMetadata, TestResult,
    TestScenario, ValidateInputMetadata, ValidateOutputMetadata,
};
use testkit::script::NodeScript;
use testkit::Script;

// ─────────────────────────── 脚本（与 E2E-01 同构，可在指定节点注入挂起）───────────────────────────

/// 在 `(stage, node)` 的脚本队列最前面插入 `stall`——该节点首次 LLM 调用永久挂起。
fn staged<'a>(
    script: &'a mut Script,
    stage: Stage,
    node: Node,
    stall_at: &[(Stage, Node)],
) -> NodeScript<'a> {
    let ns = script.for_node(stage, node);
    if stall_at.contains(&(stage, node)) {
        ns.stall()
    } else {
        ns
    }
}

fn pipeline_script(task_id: &str, stall_at: &[(Stage, Node)]) -> Script {
    let mut s = Script::new();
    staged(
        &mut s,
        Stage::ArchitectDesign,
        Node::ValidateInput,
        stall_at,
    )
    .submit(&ValidateInputMetadata {
        readiness: true,
        blockers: vec![],
    });
    staged(&mut s, Stage::ArchitectDesign, Node::Execute, stall_at)
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
    staged(
        &mut s,
        Stage::ArchitectDesign,
        Node::ValidateOutput,
        stall_at,
    )
    .submit(&ValidateOutputMetadata {
        passed: true,
        ..Default::default()
    });

    staged(&mut s, Stage::DevelopDesign, Node::ValidateInput, stall_at).submit(
        &ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        },
    );
    staged(&mut s, Stage::DevelopDesign, Node::Execute, stall_at)
        .write_file("dev-plan.md", "# 开发计划\n")
        .submit(&DevelopDesignMetadata {
            readiness: true,
            dev_doc_path: Some("dev-plan.md".into()),
            ..Default::default()
        });
    staged(&mut s, Stage::DevelopDesign, Node::ValidateOutput, stall_at).submit(
        &ValidateOutputMetadata {
            passed: true,
            ..Default::default()
        },
    );

    staged(&mut s, Stage::TestDesign, Node::ValidateInput, stall_at).submit(
        &ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        },
    );
    staged(&mut s, Stage::TestDesign, Node::Execute, stall_at)
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
    staged(&mut s, Stage::TestDesign, Node::ValidateOutput, stall_at).submit(
        &ValidateOutputMetadata {
            passed: true,
            ..Default::default()
        },
    );

    staged(&mut s, Stage::Develop, Node::Execute, stall_at)
        .write_file(
            "src/lib.rs",
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n#[test]\nfn adds() { assert_eq!(add(1, 2), 3); }\n",
        )
        .run_command(&format!(
            "git add -A && git -c user.name=f -c user.email=f@f commit -m 'feat: task {task_id}'"
        ))
        .submit(&CodeChanges {
            branch_name: format!("kanban/{task_id}"),
            changed_files: vec![],
            unit_test_files: vec![],
        });
    staged(&mut s, Stage::Review, Node::Execute, stall_at)
        .write_file(
            "review-report.md",
            "# 评审报告\n## 设计符合性\n通过\n## 测试质量\n通过\n",
        )
        .submit(&ReviewResult {
            approved: true,
            review_report_path: Some("review-report.md".into()),
            required_changes: vec![],
        });
    staged(&mut s, Stage::Test, Node::Execute, stall_at)
        .write_file("test-report.md", "# 测试报告\n全部通过\n")
        .submit(&TestResult {
            passed: true,
            test_report_path: Some("test-report.md".into()),
            failures: vec![],
            gate_recheck: false,
        });
    s
}

// ─────────────────────────── ① 单游标中断恢复 + ④ owner 清理 + ⑤ 幂等 ───────────────────────────

#[tokio::test]
async fn interrupted_node_resumes_after_restart_and_owner_cleanup() {
    let f = Flow::new().await;
    f.agent
        .set_script(pipeline_script("t1", &[(Stage::Develop, Node::Execute)]));
    f.seed_and_admit("t1").await;

    // 在 develop.execute 的 LLM 调用途中 abort（模拟 kill -9）
    let calls = f
        .run_until_node_then_abort("t1", Stage::Develop, Node::Execute)
        .await;
    assert!(calls >= 1, "中断点应已被调用");
    // 中断前已完成的上游节点调用次数——重启后必须保持不变（不重跑）
    let arch_calls_before = f.agent.calls_for(Stage::ArchitectDesign, Node::Execute);

    // 决策 54 / 80：checkpoint 停在中断节点，任务状态保持 running 不改动
    let cursor = f
        .store
        .resolve_sole_cursor("t1")
        .await
        .unwrap()
        .expect("单 main 游标");
    assert_eq!((cursor.stage, cursor.node), (Stage::Develop, Node::Execute));
    assert_eq!(cursor.status, CursorStatus::Active);
    assert_eq!(
        f.store.get_task("t1").await.unwrap().status,
        TaskStatus::Running
    );

    // 决策 127：kill -9 残留的 executor_owner 让再 claim 失败
    let task = f.store.get_task("t1").await.unwrap();
    assert!(
        task.executor_owner.is_some(),
        "中断应留下 executor_owner 残留（模拟 kill -9）"
    );
    assert!(
        !f.store.try_claim_executor("t1", "probe").await.unwrap(),
        "残留 owner 未清前不得再次 claim"
    );
    let cleared = f.store.clear_executor_owners().await.unwrap();
    assert_eq!(cleared, 1, "启动恢复第一步清理残留 owner");
    assert!(f.store.try_claim_executor("t1", "probe").await.unwrap());
    f.store.release_executor("t1").await.unwrap();

    // 重启：换一份"无挂起"脚本，从 develop.execute 续跑
    f.agent.set_script(pipeline_script("t1", &[]));
    f.fresh_executor().run("t1").await.unwrap();

    // 停在 merge 阶段 A（设计行为）
    let task = f.store.get_task("t1").await.unwrap();
    assert_eq!(task.status, TaskStatus::Pending);
    f.store
        .apply_merge_decision("t1", MergeDecision::Approve)
        .await
        .unwrap();
    f.fresh_executor().run("t1").await.unwrap();

    // 到达终态
    assert_eq!(
        f.store.get_task("t1").await.unwrap().status,
        TaskStatus::Done
    );

    // ⑤ G8/G9 幂等：已完成的上游节点在重启后不重跑，产出不重复
    assert_eq!(
        f.agent.calls_for(Stage::ArchitectDesign, Node::Execute),
        arch_calls_before,
        "已完成的 architect 节点不得重跑；调用序列 = {:?}",
        f.agent.call_log()
    );
    assert_eq!(
        f.store
            .list_runs_at("t1", Stage::ArchitectDesign, Node::Execute)
            .await
            .unwrap()
            .len(),
        1,
        "上游 run 行不重复"
    );
    let outputs = f.store.list_stage_outputs("t1").await.unwrap();
    let design_docs = outputs
        .iter()
        .filter(|o| o.stage == Stage::ArchitectDesign && o.output_type == "design_doc")
        .count();
    assert_eq!(design_docs, 1, "design_doc 产出唯一（upsert 幂等）");
    // 中断节点重跑：LLM 至少被再调一次（节点级恢复）
    assert!(
        f.agent.calls_for(Stage::Develop, Node::Execute) >= 2,
        "中断节点应从入口重跑"
    );
}

// ─────────────────────────── ② 并行双游标独立恢复 + ③ waiting_join 跨重启 ───────────────────────────

#[tokio::test]
async fn parallel_branches_recover_independently_and_join_survives_restart() {
    let f = Flow::new().await;
    // executor 注册表是进程全局键（task_id），同进程并发测试必须用互不相同的 id
    // （testing.md §11 注记）——本用例用 t2。
    // 只挂起 develop-design；test-design 应在同一轮照常跑到 waiting_join
    // 两条设计分支都在 execute 处挂起：中断时两条游标各自停在中断节点
    f.agent.set_script(pipeline_script(
        "t2",
        &[
            (Stage::DevelopDesign, Node::Execute),
            (Stage::TestDesign, Node::Execute),
        ],
    ));
    f.seed_and_admit("t2").await;

    f.run_until_node_then_abort("t2", Stage::DevelopDesign, Node::Execute)
        .await;

    // 决策 80：两游标独立——各自停在中断节点，互不推进也不互相污染
    let cursors = f.store.load_live_cursors("t2").await.unwrap();
    let dev = cursors
        .iter()
        .find(|c| c.branch == "develop-design")
        .expect("develop-design 游标");
    let test = cursors
        .iter()
        .find(|c| c.branch == "test-design")
        .expect("test-design 游标");
    assert_eq!((dev.stage, dev.node), (Stage::DevelopDesign, Node::Execute));
    assert_eq!(dev.status, CursorStatus::Active);
    assert_eq!((test.stage, test.node), (Stage::TestDesign, Node::Execute));
    assert_eq!(test.status, CursorStatus::Active);

    // 决策 127：清理残留 owner（真实重启路径）
    assert_eq!(f.store.clear_executor_owners().await.unwrap(), 1);

    // 重启续跑：两分支各自从中断节点补完 → join 恰执行一次 → 下游 continue
    f.agent.set_script(pipeline_script("t2", &[]));
    f.fresh_executor().run("t2").await.unwrap();

    assert_eq!(
        f.store
            .list_runs_at("t2", Stage::SyncCheck, Node::Execute)
            .await
            .unwrap()
            .len(),
        1,
        "join（sync-check）恰执行一次（决策 107 / G5）"
    );
    let task = f.store.get_task("t2").await.unwrap();
    assert_eq!(task.status, TaskStatus::Pending, "进入 merge 阶段 A");

    f.store
        .apply_merge_decision("t2", MergeDecision::Approve)
        .await
        .unwrap();
    f.fresh_executor().run("t2").await.unwrap();
    assert_eq!(
        f.store.get_task("t2").await.unwrap().status,
        TaskStatus::Done
    );
    let _ = &f.repo;
}

// ─────────────────────────── ③ waiting_join 跨重启保留 + 另一分支就位后 join 推进 ───────────────────────────

#[tokio::test]
async fn waiting_join_survives_restart_and_join_advances_when_sibling_ready() {
    let f = Flow::new().await;
    testkit::seed_task(&f.store, "t3", "p1").await.unwrap();
    f.scheduler().tick().await.unwrap();

    // 造出 join 前的真实产出（sync-check 据此判定 readiness / design_refs）
    f.store
        .upsert_stage_output(
            "t3",
            Stage::ArchitectDesign,
            "design_doc",
            "design.md",
            Some(&serde_json::json!({
                "affected_files": ["src/lib.rs"],
                "new_symbols": [],
                "acceptance_criteria": [{"id": "AC-1", "description": "能登录"}]
            })),
        )
        .await
        .unwrap();
    f.store
        .upsert_stage_output(
            "t3",
            Stage::TestDesign,
            "test_scenarios",
            "test-scenarios.md",
            Some(&serde_json::json!({
                "readiness": true,
                "blockers": [],
                "test_scenarios": [{
                    "id": "S-1", "name": "登录成功", "description": "登录",
                    "preconditions": [], "steps": [], "expected_result": "成功",
                    "priority": "high", "design_refs": ["AC-1"]
                }]
            })),
        )
        .await
        .unwrap();

    // 分裂为两条设计分支，test-design 已停在 waiting_join（DB checkpoint）
    let split = f.store.split_cursors("t3").await.unwrap();
    let test = split
        .iter()
        .find(|c| c.branch == "test-design")
        .unwrap()
        .clone();
    f.store
        .set_cursor_waiting_join(&test.cursor_id)
        .await
        .unwrap();

    // 模拟 kill -9 残留 owner → 重启清理（决策 127）
    assert!(f.store.try_claim_executor("t3", "dead").await.unwrap());
    assert_eq!(f.store.clear_executor_owners().await.unwrap(), 1);

    // 重启后 waiting_join 仍在（reload 自 DB，不依赖进程内状态）
    let cursors = f.store.load_live_cursors("t3").await.unwrap();
    assert!(cursors
        .iter()
        .any(|c| c.branch == "test-design" && c.status == CursorStatus::WaitingJoin));

    // 重启：develop-design 分支就位 → join（sync-check）恰执行一次
    f.agent.set_script(pipeline_script("t3", &[]));
    f.fresh_executor().run("t3").await.unwrap();

    assert_eq!(
        f.store
            .list_runs_at("t3", Stage::SyncCheck, Node::Execute)
            .await
            .unwrap()
            .len(),
        1,
        "另一分支就位后 join 正常推进（决策 107 / G5）"
    );
    // join 后 two 分支归档，main 游标接管
    let cursors = f.store.load_live_cursors("t3").await.unwrap();
    assert!(cursors.iter().any(|c| c.branch == "main"));
}
