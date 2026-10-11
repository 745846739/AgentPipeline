//! 迁移 0048 在**既有数据**上打开（票 foreman-work-record 03）。
//!
//! 最高风险的一步是回填的诚实性：旧 `created_at` 里存的恰好就是开工时刻（事故账的
//! 病根正是它），迁移把它**原样**抄进 `started_at`、`created_at` 一字不动——存量行的
//! 收口时刻已不可考，不许拿任何近似值冒充。做法照 `foreman_sessions_migration.rs` 的
//! 先例（样板已收进 `testkit::migrations::migrate_up_to`）：只跑到 0047，用原始 SQL
//! 造出老形态的行，再走 `Store::open`（跑全量迁移）核对。

use std::sync::Arc;

use agentpipeline_core::storage::foreman::FOREMAN_SESSION_KIND_TALK;
use agentpipeline_core::storage::Store;
use testkit::{migrations::migrate_up_to, ManualClock, TestHome};

#[tokio::test]
async fn the_started_at_backfill_copies_the_old_created_at_and_touches_nothing_else() {
    let home = TestHome::new().unwrap();
    let mut migrator = sqlx::migrate!("src/storage/migrations");
    let pool = migrate_up_to(&home.home().db_path(), 47, &mut migrator).await;

    // 老形态：一个班次 + 一对 user / assistant 行。assistant 行的 created_at 是**开工**
    // 时刻（事故账的口径）——00:48:23 开工的那一轮，台账里只有这个钟点。
    sqlx::query(
        "INSERT INTO kanban_foreman_sessions
         (id, title, kind, engine, created_at, last_active_at)
         VALUES ('s1', '老班次', 'talk', 'api', '2026-01-01T00:00:00+00:00',
                 '2026-01-01T01:26:30+00:00')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO kanban_foreman_messages
         (session_id, role, content, prompt_tokens, completion_tokens, created_at)
         VALUES ('s1', 'user', '修一下 t1', 0, 0, '2026-01-01T00:48:22+00:00')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO kanban_foreman_messages
         (session_id, role, content, prompt_tokens, completion_tokens, created_at)
         VALUES ('s1', 'assistant', '改好了。', 120, 8, '2026-01-01T00:48:23+00:00')",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;

    // ── 正式打开：跑全量迁移（含 0048）──
    let clock = ManualClock::fixed();
    let store = Store::open(home.home().clone(), Arc::new(clock))
        .await
        .expect("在既有数据上打开必须成功");

    let messages = store.list_foreman_messages("s1", 10, None).await.unwrap();
    assert_eq!(messages.len(), 2);
    let (user, assistant) = (&messages[0], &messages[1]);

    // started_at = 迁移前的 created_at（开工时刻保住）；created_at 未被改写——
    // 收口时刻不可考，迁移不替它编一个。
    assert_eq!(
        user.started_at.map(|t| t.to_rfc3339()),
        Some("2026-01-01T00:48:22+00:00".to_string())
    );
    assert_eq!(user.created_at.to_rfc3339(), "2026-01-01T00:48:22+00:00");
    assert_eq!(
        assistant.started_at.map(|t| t.to_rfc3339()),
        Some("2026-01-01T00:48:23+00:00".to_string())
    );
    assert_eq!(
        assistant.created_at.to_rfc3339(),
        "2026-01-01T00:48:23+00:00"
    );

    // 班次读数原样：迁移只动 messages 这一张表。
    let sessions = store
        .list_foreman_sessions(Some(FOREMAN_SESSION_KIND_TALK), false)
        .await
        .unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(
        sessions[0].last_active_at.to_rfc3339(),
        "2026-01-01T01:26:30+00:00"
    );
}
