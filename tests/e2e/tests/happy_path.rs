//! E2E-01 happy path（testing.md §8、决策 80 / 90 / 97 / 99 / 113 / 125）。
//!
//! **executor 已落地（票 11）**：本用例由 FakeAgent 驱动完整流水线——
//! `scheduler.tick()` 准入 → `executor.run()` 自动推进 init → architect-design →
//! 并行设计分支 → sync-check 汇聚 → develop → review → test → merge 阶段 A，
//! 停在 pending(merge_approval)；用户审批后 resume 走阶段 B 合入 → done。
//! 覆盖游标"分裂 → 合并 → 归档"、`default_branch` 前进（update-ref）、
//! worktree / 分支清理、system run 落库与 token 汇总、节点级 SSE。
//!
//! FakeAgent 只替换 LLM 响应流；工具层、git、命令记录全部真实执行（决策 148）。

mod common;

use agentpipeline_core::git::Git;
use agentpipeline_core::storage::decisions::MergeDecision;
use agentpipeline_core::types::{
    Approval, CursorStatus, Gate, MergeResult, Node, NodeStatus, PendingKind, Stage, TaskStatus,
    TransitionTrigger,
};
use common::{full_pass_script, merge_row, Flow};
use testkit::Script;

#[tokio::test]
async fn happy_path_full_flow() {
    let f = Flow::new().await;
    let base_commit = f.repo.head("main");
    // FakeAgent 脚本驱动全流程
    let mut script = Script::new();
    full_pass_script(&mut script, "t1");
    f.agent.set_script(script);

    // ── 创建：queued + 同事务 main 游标（决策 90 / 98）──
    let task = testkit::seed_task(&f.store, "t1", "p1").await.unwrap();
    assert_eq!(task.status, TaskStatus::Queued);
    let cursor = f.sole_cursor("t1").await;
    assert_eq!((cursor.stage, cursor.node), (Stage::Init, Node::Execute));

    // ── 准入：scheduler tick 放行（决策 98），然后 executor 自动推进 ──
    let report = f.scheduler().tick().await.unwrap();
    assert_eq!(report.admitted, vec!["t1".to_string()]);
    assert_eq!(
        f.store.get_task("t1").await.unwrap().status,
        TaskStatus::Running
    );

    f.executor.run("t1").await.unwrap();

    // ── merge 阶段 A：rebase + 闸门 + proposal + pending(merge_approval) ──
    let task = f.store.get_task("t1").await.unwrap();
    assert_eq!(task.status, TaskStatus::Pending, "等待审批");
    let live = f.store.load_live_cursors("t1").await.unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(
        live[0].pending_reason.as_ref().unwrap().kind,
        PendingKind::MergeApproval
    );

    // 动作集：合入 / 返回修改（决策 119）
    let actions = f.store.allowed_actions_for_task("t1").await.unwrap();
    let names: Vec<&str> = actions.iter().map(|a| a.action.as_str()).collect();
    assert_eq!(names, vec!["approve", "return"]);

    // 提案：diff 内容真实、闸门通过（决策 96 / 139）
    let merge = f.store.merge_metadata("t1").await.unwrap().unwrap();
    assert_eq!(merge.gate, Some(Gate::Pass));
    assert!(merge.diff_stats.files_changed >= 1);
    let proposal =
        std::fs::read_to_string(f.home.home().task_file("t1", "merge-proposal.diff")).unwrap();
    assert!(proposal.contains("src/lib.rs"));
    // 基准未变 → 可合入（决策 96 的前置条件）
    assert_eq!(
        Git.rev_parse(f.repo.path(), "main").await.unwrap(),
        merge.base_commit
    );

    // 阶段 A 的产物文件齐备（设计文档写任务目录，§6）
    for doc in [
        "design.md",
        "dev-plan.md",
        "test-scenarios.md",
        "review-report.md",
        "test-report.md",
    ] {
        assert!(
            f.home.home().task_file("t1", doc).exists(),
            "{doc} 应存在于任务目录"
        );
    }

    // ── merge 阶段 B：approve → 基准校验 → 合入 → update-ref 写回（决策 96 / 97）──
    let merge_cursor = f
        .store
        .apply_merge_decision("t1", MergeDecision::Approve)
        .await
        .unwrap();
    assert_eq!(
        (merge_cursor.stage, merge_cursor.node),
        (Stage::Merge, Node::Execute)
    );
    assert_eq!(merge_cursor.status, CursorStatus::Active);

    f.executor.run("t1").await.unwrap();

    // ── 终态断言 ──
    let task = f.store.get_task("t1").await.unwrap();
    assert_eq!(task.status, TaskStatus::Done);
    let worktree = f.home.home().worktree_path("t1");
    assert!(!worktree.exists(), "worktree 应被清理");
    assert!(!f.repo.branch_exists("kanban/t1"), "任务分支应被删除");
    assert!(
        !f.repo.worktree_list().contains("/worktrees/"),
        "任务 worktree 应从 git worktree list 中消失：{}",
        f.repo.worktree_list()
    );
    assert_eq!(f.repo.worktree_list().lines().count(), 1, "只应剩主工作区");
    // 主干真的前进到任务提交（决策 97 的回归点）
    assert_ne!(f.repo.head("main"), base_commit);
    assert_eq!(
        f.repo.git(&["log", "--format=%s", "-1", "main"]).trim(),
        "feat: task t1"
    );

    // token 汇总：agent run 计入调用次数，system run 不计（决策 100 / 130 ②）
    let runs = f.store.list_runs("t1").await.unwrap();
    let agent_runs: Vec<_> = runs.iter().filter(|r| r.agent_type == "main").collect();
    let system_runs: Vec<_> = runs.iter().filter(|r| r.agent_type == "system").collect();
    assert_eq!(agent_runs.len(), 12, "12 个 agent 节点各一行 run");
    assert!(runs.iter().all(|r| r.status == NodeStatus::Success));
    let task = f.store.get_task("t1").await.unwrap();
    let expected_tokens: u64 = runs
        .iter()
        .map(|r| r.prompt_tokens as u64 + r.completion_tokens as u64)
        .sum();
    assert_eq!(task.total_tokens, expected_tokens, "Σ 所有 run 的 token");
    assert!(task.total_tokens > 0);
    assert_eq!(task.total_calls as usize, agent_runs.len());

    // sync-check 恰好执行一次（决策 83 / G5）；init / done / 闸门节点落 system run（决策 99 / 114）
    assert_eq!(
        f.store
            .list_runs_at("t1", Stage::SyncCheck, Node::Execute)
            .await
            .unwrap()
            .len(),
        1
    );
    for (stage, node) in [
        (Stage::Init, Node::Execute),
        (Stage::Done, Node::Execute),
        (Stage::Develop, Node::ValidateOutput),
        (Stage::Test, Node::ValidateOutput),
        (Stage::Merge, Node::Execute),
    ] {
        assert!(
            system_runs
                .iter()
                .any(|r| r.stage == stage && r.node == node),
            "{stage}.{node} 应有 system run"
        );
    }

    // 游标行永不物理删除：1 初始 + 1 分裂 test-design + 1 合并 main = 3，全部保留
    let all = f.store.load_all_cursors("t1").await.unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(
        all.iter()
            .filter(|c| c.status == CursorStatus::Archived)
            .count(),
        2
    );

    // 节点级 SSE（决策 76 / 84）
    assert!(
        f.sse
            .count_of(agentpipeline_core::sse::SseEventType::NodeStarted)
            >= 15
    );
    assert!(
        f.sse
            .count_of(agentpipeline_core::sse::SseEventType::StageChanged)
            >= 5
    );
    assert_eq!(
        f.sse
            .count_of(agentpipeline_core::sse::SseEventType::TaskDone),
        1
    );

    // 命令日志留痕（§12.4.4）：develop 闸门 + merge 闸门 + agent 的 git 提交
    let commands = f.store.list_commands("t1", None, None).await.unwrap();
    assert!(
        commands.len() >= 3,
        "闸门与 agent 命令应留痕：{}",
        commands.len()
    );

    // 流转时间线含用户操作记录
    let transitions = f.store.list_transitions("t1").await.unwrap();
    assert!(transitions
        .iter()
        .any(|t| t.trigger == TransitionTrigger::UserResume));
}

#[tokio::test]
async fn base_moved_invalidates_approval_and_reruns_phase_a() {
    // E2E-09（决策 96 / 108）：阶段 B 入口发现基准前移 → approval 重置为 none，
    // 回阶段 A 重跑，gate_failures 保留。
    let f = Flow::new().await;
    testkit::seed_task(&f.store, "t1", "p1").await.unwrap();
    f.scheduler().tick().await.unwrap();
    let worktree = f.home.home().worktree_path("t1");
    Git.init_worktree(f.repo.path(), "t1", &worktree, "main")
        .await
        .unwrap();

    // 阶段 A 记录的 base_commit
    let stale_base = Git.rev_parse(f.repo.path(), "main").await.unwrap();
    let mut proposal = merge_row("merge-proposal.diff", &stale_base);
    proposal.gate = Some(Gate::Pass);
    proposal.gate_failures = 1; // 之前失败过一次
    proposal.approval = Approval::Pending;
    f.store
        .upsert_merge_result("t1", "merge-proposal.diff", &proposal)
        .await
        .unwrap();

    // 基准前移（另一个任务合入）
    f.repo.advance_main("src/other.rs", "pub fn other() {}\n");
    let current_base = Git.rev_parse(f.repo.path(), "main").await.unwrap();
    assert_ne!(current_base, stale_base, "基准已前移");

    // 阶段 B 入口比对不一致 → 重置 approval 回阶段 A
    let stored = f
        .store
        .stage_output_metadata(
            "t1",
            Stage::Merge,
            agentpipeline_core::types::MERGE_OUTPUT_TYPE,
        )
        .await
        .unwrap()
        .unwrap();
    let recorded: MergeResult = serde_json::from_value(stored).unwrap();
    assert_ne!(recorded.base_commit, current_base, "diff 已过期");

    let reset = MergeResult {
        approval: Approval::None,
        gate: Some(Gate::Pass),
        gate_failures: recorded.gate_failures, // 决策 108：计数保留
        ..recorded.clone()
    };
    f.store
        .upsert_merge_result("t1", "merge-proposal.diff", &reset)
        .await
        .unwrap();

    let after = f
        .store
        .stage_output_metadata(
            "t1",
            Stage::Merge,
            agentpipeline_core::types::MERGE_OUTPUT_TYPE,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after["approval"], "none");
    assert_eq!(after["gate_failures"], 1, "gate_failures 跨阶段跳转不重置");
}

#[tokio::test]
async fn gate_failures_are_not_reset_by_merge_upsert() {
    // 决策 108：merge metadata 的 upsert 路径**显式跳过** gate_failures
    let f = Flow::new().await;
    testkit::seed_task(&f.store, "t1", "p1").await.unwrap();

    let mut first = merge_row("merge-proposal.diff", "basesha");
    first.gate = Some(Gate::Fail);
    first.gate_failure_kind = Some(agentpipeline_core::types::GateFailureKind::Test);
    f.store
        .upsert_merge_result("t1", "merge-proposal.diff", &first)
        .await
        .unwrap();
    assert_eq!(f.store.increment_gate_failures("t1").await.unwrap(), 1);
    assert_eq!(f.store.increment_gate_failures("t1").await.unwrap(), 2);

    // 再次 upsert 一份 gate_failures = 0 的新结果：不得把计数清零
    let mut second = merge_row("merge-proposal.diff", "basesha");
    second.gate = Some(Gate::Fail);
    second.gate_failure_kind = Some(agentpipeline_core::types::GateFailureKind::Lint);
    let stored = f
        .store
        .upsert_merge_result("t1", "merge-proposal.diff", &second)
        .await
        .unwrap();
    assert_eq!(stored.gate_failures, 2, "upsert 必须跳过该字段");
    assert_eq!(
        stored.gate_failure_kind,
        Some(agentpipeline_core::types::GateFailureKind::Lint),
        "失败类型是普通字段，应被更新（决策 139）"
    );
}

#[tokio::test]
async fn retry_resets_worktree_and_requeues_for_admission() {
    // E2E-08 的 retry 段（决策 125 / 117）
    let f = Flow::new().await;
    testkit::seed_task(&f.store, "t1", "p1").await.unwrap();
    f.scheduler().tick().await.unwrap();
    let worktree = f.home.home().worktree_path("t1");
    Git.init_worktree(f.repo.path(), "t1", &worktree, "main")
        .await
        .unwrap();

    // 半成品：一次提交 + 未跟踪文件
    std::fs::write(worktree.join("src/lib.rs"), "pub fn broken() {}\n").unwrap();
    run_git(&worktree, &["add", "-A"]);
    run_git(&worktree, &["commit", "-m", "wip: 半成品"]);
    std::fs::write(worktree.join("half_done.txt"), "残留").unwrap();

    f.store
        .mark_terminal("t1", TaskStatus::Failed)
        .await
        .unwrap();

    // retry：游标复位 + worktree 硬重置 + 置回 queued
    f.store.reset_cursors_to_init("t1").await.unwrap();
    Git.reset_hard_clean(&worktree, "main").await.unwrap();
    f.store
        .set_task_status("t1", TaskStatus::Queued)
        .await
        .unwrap();

    let live = f.store.load_live_cursors("t1").await.unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!((live[0].stage, live[0].node), (Stage::Init, Node::Execute));
    assert!(
        !worktree.join("half_done.txt").exists(),
        "clean -fdx 清掉未跟踪文件"
    );
    assert_eq!(
        f.store.get_task("t1").await.unwrap().status,
        TaskStatus::Queued
    );

    // 重新准入
    let report = f.scheduler().tick().await.unwrap();
    assert_eq!(report.admitted, vec!["t1".to_string()]);
    assert_eq!(
        f.store.get_task("t1").await.unwrap().status,
        TaskStatus::Running
    );
}

fn run_git(cwd: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(["-c", "user.name=fixture"])
        .args(["-c", "user.email=fixture@localhost"])
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git 可执行");
    assert!(
        out.status.success(),
        "git {} 失败: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}
