//! L2 集成（票 10 / 决策 100 / 130 ②）：项目级伪阶段的独立观测行。
//!
//! 用真 SQLite 临时文件库 + 全量迁移（决策 145）验证：
//! `project_analysis` 这类无任务、无游标的伪阶段，以 `project_id` 归属落 run +
//! 会话行，且计入 `total_calls`。

use agentpipeline_core::metrics;
use agentpipeline_core::storage::observability::{NewProjectRun, RunOutcome};
use agentpipeline_core::storage::Store;
use agentpipeline_core::types::{Node, NodeStatus, Stage};
use testkit::{seed_project, TestHome};

const PSEUDO_PROJECT_ANALYSIS: &str = "pseudo:project_analysis";

async fn setup() -> (TestHome, Store) {
    let home = TestHome::new().unwrap();
    let (store, _clock) = home.setup().await.unwrap();
    let repo = home.scratch_dir("proj");
    seed_project(&store, "p1", "示例", &repo, "main")
        .await
        .unwrap();
    (home, store)
}

#[tokio::test]
async fn project_run_and_conversation_are_queryable_after_analysis() {
    // 验收：调用项目分析后可查到该次 run / 会话行。
    let (_home, store) = setup().await;

    let run_id = store
        .insert_project_run(&NewProjectRun {
            project_id: "p1".into(),
            stage: Stage::Init,
            node: Node::Execute,
            attempt: 1,
            agent_type: PSEUDO_PROJECT_ANALYSIS.into(),
        })
        .await
        .unwrap();

    // run 行：task / cursor 为空，project_id 归属项目
    let runs = store.list_project_runs("p1").await.unwrap();
    assert_eq!(runs.len(), 1, "项目级 run 应按 project_id 可查");
    assert_eq!(runs[0].id, run_id);
    assert!(runs[0].task_id.is_none(), "项目级 run 无任务");
    assert!(runs[0].cursor_id.is_none(), "项目级 run 无游标");
    assert_eq!(runs[0].project_id.as_deref(), Some("p1"));
    assert_eq!(runs[0].agent_type, PSEUDO_PROJECT_ANALYSIS);
    assert_eq!(runs[0].status, NodeStatus::Running);

    store
        .finish_run(
            run_id,
            &RunOutcome {
                status: Some(NodeStatus::Success),
                prompt_tokens: 10,
                completion_tokens: 5,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let messages = serde_json::json!([{"role": "assistant", "content": "摘要"}]);
    let conv_id = store
        .insert_project_conversation(
            "p1",
            run_id,
            Stage::Init,
            Node::Execute,
            1,
            PSEUDO_PROJECT_ANALYSIS,
            &messages,
            Some(&serde_json::json!({"summary": "摘要"})),
            10,
            5,
        )
        .await
        .unwrap();

    // 会话行：按 project_id + run_id 取；task_id 为空
    let conv = store
        .get_project_conversation("p1", run_id)
        .await
        .unwrap()
        .expect("项目级会话应可查");
    assert_eq!(conv.id, conv_id);
    assert!(conv.task_id.is_none(), "项目级会话无任务");
    assert_eq!(conv.project_id.as_deref(), Some("p1"));
    assert_eq!(conv.messages_json, messages);
    assert_eq!(conv.agent_type, PSEUDO_PROJECT_ANALYSIS);

    assert_eq!(
        store.list_project_conversations("p1").await.unwrap().len(),
        1
    );

    // 项目级行不串进任务查询
    assert!(store.list_runs("p1").await.unwrap().is_empty());
    assert!(store
        .list_conversations("p1", false)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn project_run_counts_in_total_calls() {
    // 决策 130 ②：项目级伪阶段调了 LLM（非 system）→ 计入 total_calls。
    let (_home, store) = setup().await;
    store
        .insert_project_run(&NewProjectRun {
            project_id: "p1".into(),
            stage: Stage::Init,
            node: Node::Execute,
            attempt: 1,
            agent_type: PSEUDO_PROJECT_ANALYSIS.into(),
        })
        .await
        .unwrap();

    let all = store.all_runs().await.unwrap();
    assert_eq!(
        metrics::total_calls(&all),
        1,
        "项目级伪阶段应计入 total_calls"
    );
    assert_eq!(metrics::total_tokens(&all), 0);
}

#[tokio::test]
async fn project_run_check_rejects_dual_ownership() {
    // schema CHECK：归属二选一——task 与 project 不得同时非空。
    let (_home, store) = setup().await;
    let err = sqlx::query(
        "INSERT INTO kanban_node_runs
         (task_id, cursor_id, project_id, stage, node, attempt, agent_type, status, started_at)
         VALUES ('t1', NULL, 'p1', 'init', 'execute', 1, 'main', 'running', '2026-01-01T00:00:00Z')",
    )
    .execute(store.pool())
    .await;
    assert!(
        err.is_err(),
        "task_id 与 project_id 同时非空应被 CHECK 拒绝"
    );
}
