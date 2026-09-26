//! 迁移 0012 在**既有数据**上打开（决策 204④）。
//!
//! 这是本次迁移最高风险的一条：老库里已经有对讲台的消息与流水线的命令，
//! 迁移要新建会话表、给消息加列并回填、把命令表整个重建（`task_id` 改可空 + 加 `session_id`）。
//! 中间任何一步写错，用户打开应用看到的是一条「迁移失败」——而不是一个还能用的台账。
//!
//! 做法：用 sqlx 的迁移器**只跑到 0011**，用原始 SQL 造出老形态的行，再走 `Store::open`
//! （它跑全量迁移）确认：不炸、老消息进了第一个会话、老命令还在、且仍是任务口径。

use std::borrow::Cow;
use std::sync::Arc;

use agentpipeline_core::storage::foreman::FOREMAN_SESSION_KIND_TALK;
use agentpipeline_core::storage::Store;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use testkit::{ManualClock, TestHome};

/// 把库迁到 `0011`（即 0012 之前的那一版）。
async fn migrate_to_0011(db: &std::path::Path) -> sqlx::SqlitePool {
    let mut migrator = sqlx::migrate!("src/storage/migrations");
    migrator.migrations = Cow::Owned(
        migrator
            .migrations
            .iter()
            .filter(|m| m.version <= 11)
            .cloned()
            .collect(),
    );
    let options = SqliteConnectOptions::new()
        .filename(db)
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    migrator.run(&pool).await.unwrap();
    pool
}

#[tokio::test]
async fn a_legacy_database_migrates_and_its_messages_land_in_the_first_session() {
    let home = TestHome::new().unwrap();
    let pool = migrate_to_0011(&home.home().db_path()).await;

    // 老形态的数据：项目 / 任务（命令的归属，旧表 task_id NOT NULL）+ 两条对讲台消息 + 一条命令。
    sqlx::query(
        "INSERT INTO kanban_projects (id, name, local_path, default_branch, created_at)
         VALUES ('p1', '老项目', '/tmp/p1', 'main', '2026-01-01T00:00:00+00:00')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO kanban_tasks (id, title, project_id, status, current_stage, created_at, updated_at)
         VALUES ('t1', '老任务', 'p1', 'done', 'done', '2026-01-01T00:00:00+00:00',
                 '2026-01-01T00:00:00+00:00')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO kanban_foreman_messages (role, content, prompt_tokens, completion_tokens,
                                              briefing_json, traces_json, created_at)
         VALUES ('user', '这是我昨晚问的第一句话，后面的字只是用来看看标题会不会被截断',
                 0, 0, NULL, NULL, '2026-01-01T00:00:00+00:00')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO kanban_foreman_messages (role, content, prompt_tokens, completion_tokens,
                                              briefing_json, traces_json, created_at)
         VALUES ('assistant', '收到。', 120, 8, '{\"pending\":[]}', NULL,
                 '2026-01-01T00:01:00+00:00')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO kanban_node_commands (task_id, run_id, stage, node, source, command, cwd,
                                           exit_code, started_at)
         VALUES ('t1', NULL, 'develop', 'execute', 'system', 'cargo test', '/tmp/p1', 0,
                 '2026-01-01T00:00:30+00:00')",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;

    // ── 正式打开：跑全量迁移（含 0012）──
    let clock = ManualClock::fixed();
    let store = Store::open(home.home().clone(), Arc::new(clock))
        .await
        .expect("在既有数据上打开必须成功");

    // 老消息进了第一个会话，顺序与读数都还在。
    let sessions = store.list_foreman_sessions(Some(FOREMAN_SESSION_KIND_TALK)).await.unwrap();
    assert_eq!(sessions.len(), 1, "既有消息应当回填出一个会话");
    let session = &sessions[0];
    // 标题取自首条用户消息，截到 24 字并补省略号（与 `session_title_from` 同一条规则）。
    assert_eq!(
        session.title,
        "这是我昨晚问的第一句话，后面的字只是用来看看标题…"
    );
    // 迁移 0030（决策 286 / 票 01）：存量行全落 `talk`——裁决 12「不回填」的另一半：
    // 老的播报留在原会话里靠 `proactive` 标对，升级不改变任何一行的归属。
    assert_eq!(session.kind, FOREMAN_SESSION_KIND_TALK);

    let messages = store.list_foreman_messages(&session.id, 100).await.unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[1].role, "assistant");
    assert_eq!(messages[1].prompt_tokens, 120);
    assert!(messages[1].briefing_json.is_some(), "审计快照不丢");
    assert_eq!(
        store.foreman_session_totals(&session.id).await.unwrap(),
        (128, 1)
    );

    // 老命令仍在，且仍是任务口径（task_id 非空、session_id 空）。
    let commands = store.list_commands("t1", None, None).await.unwrap();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].task_id.as_deref(), Some("t1"));
    assert_eq!(commands[0].session_id, None);
    assert_eq!(commands[0].command, "cargo test");
    assert_eq!(commands[0].exit_code, Some(0));
}

#[tokio::test]
async fn an_empty_legacy_database_gains_no_session() {
    // 没有消息就没有「第一次值班」这回事：新库从零开始，首个会话在第一次说话时创建。
    let home = TestHome::new().unwrap();
    let pool = migrate_to_0011(&home.home().db_path()).await;
    pool.close().await;

    let clock = ManualClock::fixed();
    let store = Store::open(home.home().clone(), Arc::new(clock))
        .await
        .unwrap();
    assert!(store.list_foreman_sessions(Some(FOREMAN_SESSION_KIND_TALK)).await.unwrap().is_empty());
    assert!(store.latest_foreman_session().await.unwrap().is_none());
}
