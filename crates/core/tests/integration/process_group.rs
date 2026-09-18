//! run_command 的进程组接线（票 17 / 决策 66）。
//!
//! 断言：命令以独立进程组启动，真实 pgid 回填 `kanban_node_runs.process_group_id`——
//! 此前该列恒为 NULL，超时路径的 `kill(0)` 是 no-op。

use std::sync::Arc;

use agentpipeline_core::agent::client::ToolCall;
use agentpipeline_core::agent::file_policy::FileToolPolicy;
use agentpipeline_core::agent::tools::{ToolCallContext, ToolExecutor};
use agentpipeline_core::config::Settings;
use agentpipeline_core::storage::observability::NewRun;
use agentpipeline_core::types::{CommandSource, Node, Stage};
use testkit::{ManualClock, RecordingKiller, TestHome};

#[tokio::test]
async fn run_command_records_real_process_group_id() {
    let home = TestHome::new().unwrap();
    let clock = ManualClock::fixed();
    let store = store_for(&home, clock).await;

    let repo = home.scratch_dir("proj");
    testkit::seed_project(&store, "p1", "示例", &repo, "main")
        .await
        .unwrap();
    testkit::seed_task(&store, "t1", "p1").await.unwrap();
    let cursor = store.load_live_cursors("t1").await.unwrap()[0].clone();

    let worktree = home.home().worktree_path("t1");
    let task_dir = home.home().task_dir("t1");
    std::fs::create_dir_all(&worktree).unwrap();
    std::fs::create_dir_all(&task_dir).unwrap();

    let run_id = store
        .insert_run(&NewRun {
            task_id: "t1".into(),
            cursor_id: cursor.cursor_id.clone(),
            stage: Stage::Develop,
            node: Node::Execute,
            attempt: 1,
            agent_type: "main".into(),
            parent_run_id: None,
            prompt_template_hash: None,
            process_group_id: None,
        })
        .await
        .unwrap();

    let killer = RecordingKiller::new();
    let executor = ToolExecutor::new(
        home.home().clone(),
        FileToolPolicy::new(vec![worktree.clone(), task_dir.clone()]),
        Settings::default(),
        Arc::new(killer),
    )
    .with_recorder(Arc::new(store.clone()));

    // `$$` 展开为 sh 的 pid；进程组组长时 pgid == pid（process_group(0) 语义）
    let call = ToolCall {
        id: "c1".into(),
        name: "run_command".into(),
        arguments: serde_json::json!({"command": "echo $$"}).to_string(),
    };
    let ctx = ToolCallContext {
        task_id: "t1".into(),
        session_id: None,
        stage: Stage::Develop,
        node: Node::Execute,
        worktree_path: worktree.clone(),
        task_dir: task_dir.clone(),
        run_id: Some(run_id),
        command_source: CommandSource::Agent,
        default_cwd: Some(worktree),
    };
    let outcome = executor.execute(&call, &ctx).await.unwrap();

    let runs = store.list_runs("t1").await.unwrap();
    let run = runs.iter().find(|r| r.id == run_id).unwrap();
    let pgid = run
        .process_group_id
        .expect("真实 pgid 必须回填 node_runs（票 17）");
    assert!(pgid > 0, "pgid 应为正数，实际 {pgid}");

    let reported: i32 = outcome.content.trim().parse().unwrap_or(-1);
    assert_eq!(
        reported, pgid,
        "命令自报的进程组 id 应等于回填值（证明以独立进程组启动）；原始输出 = {:?}",
        outcome.content
    );
}

async fn store_for(home: &TestHome, clock: ManualClock) -> agentpipeline_core::storage::Store {
    agentpipeline_core::storage::Store::open(home.home().clone(), Arc::new(clock))
        .await
        .unwrap()
}
