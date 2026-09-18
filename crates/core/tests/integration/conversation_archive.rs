//! L2 集成：retry 的会话归档（§12.2 / 决策 113 同构）。
//!
//! 用真 SQLite 临时文件库 + 全量迁移（决策 145）验证：
//! 重试标记旧会话、默认列表不混入、`include_archived` 可取回、行仍物理存在。

use agentpipeline_core::storage::observability::{NewRun, RunOutcome};
use agentpipeline_core::storage::Store;
use agentpipeline_core::types::{Node, NodeStatus, Stage};
use testkit::{seed_project, seed_task, TestHome};

async fn setup() -> (TestHome, Store) {
    let home = TestHome::new().unwrap();
    let (store, _clock) = home.setup().await.unwrap();
    let repo = home.scratch_dir("proj");
    let project = seed_project(&store, "p1", "示例", &repo, "main")
        .await
        .unwrap();
    seed_task(&store, "t1", &project.id).await.unwrap();
    (home, store)
}

/// 落一行会话（借用既有 run 行，保持 run_id 外键合法）。
async fn insert_conv(
    store: &Store,
    cursor_id: &str,
    stage: Stage,
    messages: serde_json::Value,
) -> i64 {
    let run_id = store
        .insert_run(&NewRun {
            task_id: "t1".into(),
            cursor_id: cursor_id.into(),
            stage,
            node: Node::Execute,
            attempt: 1,
            agent_type: "main".into(),
            parent_run_id: None,
            prompt_template_hash: None,
            process_group_id: None,
        })
        .await
        .unwrap();
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
    store
        .insert_conversation(
            "t1",
            run_id,
            stage,
            Node::Execute,
            1,
            "main",
            None,
            &messages,
            None,
            None,
            10,
            5,
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn archive_conversations_marks_rows_without_deleting() {
    // 决策 113 同构：归档后行仍物理存在，仅 archived_at 有值，外键不悬空
    let (_home, store) = setup().await;
    let cursor = store.load_live_cursors("t1").await.unwrap()[0].clone();
    let conv_id = insert_conv(
        &store,
        &cursor.cursor_id,
        Stage::ArchitectDesign,
        serde_json::json!([{"role": "user", "content": "第一轮"}]),
    )
    .await;

    let affected = store.archive_conversations("t1").await.unwrap();
    assert_eq!(affected, 1);

    // 物理行仍在
    let all = store.list_conversations("t1", true).await.unwrap();
    assert_eq!(all.len(), 1, "归档不物理删除");
    assert_eq!(all[0].id, conv_id);
    assert!(all[0].archived_at.is_some(), "归档标记应写入");

    // 幂等：重复归档不再重复标记
    assert_eq!(store.archive_conversations("t1").await.unwrap(), 0);
}

#[tokio::test]
async fn default_list_excludes_archived_and_param_includes_them() {
    let (_home, store) = setup().await;
    let cursor = store.load_live_cursors("t1").await.unwrap()[0].clone();
    insert_conv(
        &store,
        &cursor.cursor_id,
        Stage::ArchitectDesign,
        serde_json::json!([{"role": "user", "content": "旧 attempt"}]),
    )
    .await;
    store.archive_conversations("t1").await.unwrap();
    insert_conv(
        &store,
        &cursor.cursor_id,
        Stage::Init,
        serde_json::json!([{"role": "user", "content": "新 attempt"}]),
    )
    .await;

    // 默认只看未归档：查看器不再把历次 attempt 混在一起
    let live = store.list_conversations("t1", false).await.unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].stage, Stage::Init);
    assert!(live[0].archived_at.is_none());

    // 历史可取回
    let with_history = store.list_conversations("t1", true).await.unwrap();
    assert_eq!(with_history.len(), 2);
    assert_eq!(
        with_history
            .iter()
            .filter(|c| c.archived_at.is_some())
            .count(),
        1
    );
}

#[tokio::test]
async fn archive_is_scoped_to_task() {
    // 按 task 隔离：归档 t1 不得影响 t2 的会话
    let (home, store) = setup().await;
    let repo = home.scratch_dir("proj2");
    let project = seed_project(&store, "p2", "示例2", &repo, "main")
        .await
        .unwrap();
    seed_task(&store, "t2", &project.id).await.unwrap();

    let c1 = store.load_live_cursors("t1").await.unwrap()[0].clone();
    insert_conv(
        &store,
        &c1.cursor_id,
        Stage::ArchitectDesign,
        serde_json::json!([{"role": "user", "content": "t1"}]),
    )
    .await;
    let c2 = store.load_live_cursors("t2").await.unwrap()[0].clone();
    let run2 = store
        .insert_run(&NewRun {
            task_id: "t2".into(),
            cursor_id: c2.cursor_id.clone(),
            stage: Stage::ArchitectDesign,
            node: Node::Execute,
            attempt: 1,
            agent_type: "main".into(),
            parent_run_id: None,
            prompt_template_hash: None,
            process_group_id: None,
        })
        .await
        .unwrap();
    store
        .insert_conversation(
            "t2",
            run2,
            Stage::ArchitectDesign,
            Node::Execute,
            1,
            "main",
            None,
            &serde_json::json!([{"role": "user", "content": "t2"}]),
            None,
            None,
            1,
            1,
        )
        .await
        .unwrap();

    store.archive_conversations("t1").await.unwrap();

    assert!(store
        .list_conversations("t1", false)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        store.list_conversations("t2", false).await.unwrap().len(),
        1
    );
    assert!(store.list_conversations("t2", false).await.unwrap()[0]
        .archived_at
        .is_none());
}
