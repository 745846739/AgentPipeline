//! 迁移 0050 在**既有数据**上打开（票 foreman-work-record 04）。
//!
//! 读法列是**可空、不回填**的：存量卡本来就没有「读法」这一说，NULL 就是「当时没说」
//! ——不许拿空串或占位文冒充（决策 415「读不到不编数」同款，与迁移 0048 的姿态一致）。
//! 做法照 foreman_started_at_migration.rs 的先例（样板已收进
//! `testkit::migrations::migrate_up_to`）：只跑到 0050 之前的最新一版——仓里 0049
//! 尚未落位时即 0048，并行线的 0049（提议来路列）先合入的话会自动包含在过滤里——
//! 用原始 SQL 造出老形态的行，再走 `Store::open`（跑全量迁移）核对。

use std::sync::Arc;

use agentpipeline_core::storage::Store;
use testkit::{migrations::migrate_up_to, ManualClock, TestHome};

#[tokio::test]
async fn the_reading_column_opens_nullable_and_old_rows_read_as_none() {
    let home = TestHome::new().unwrap();
    let mut migrator = sqlx::migrate!("src/storage/migrations");
    let pool = migrate_up_to(&home.home().db_path(), 49, &mut migrator).await;

    // 老形态：一个班次 + 一条提议（0050 之前的那套列，没有 reading）。
    sqlx::query(
        "INSERT INTO kanban_foreman_sessions
         (id, title, kind, engine, created_at, last_active_at)
         VALUES ('s1', '老班次', 'talk', 'api', '2026-01-01T00:00:00+00:00',
                 '2026-01-01T00:01:00+00:00')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO kanban_foreman_proposals
         (id, session_id, tool, args_json, summary, kind, claimed_at, stopped_round,
          created_at, expires_at, status, resolved_at)
         VALUES ('01OLD', 's1', 'task', '{\"action\":\"resume\"}', '让任务继续',
                 'api_call', NULL, 0,
                 '2026-01-01T00:00:30+00:00', '2026-01-01T00:10:30+00:00',
                 'pending', NULL)",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;

    // ── 正式打开：跑全量迁移（含 0050）──
    let clock = ManualClock::fixed();
    let store = Store::open(home.home().clone(), Arc::new(clock))
        .await
        .expect("在既有数据上打开必须成功");

    let proposals = store.list_foreman_proposals("s1", 10).await.unwrap();
    assert_eq!(proposals.len(), 1);
    let old = &proposals[0];
    assert_eq!(old.id, "01OLD");
    assert_eq!(old.reading, None, "存量卡读法为 NULL——没说就是不编");
    // 其余列原样：迁移只加列，不动任何既有读数。
    assert_eq!(old.tool, "task");
    assert_eq!(old.summary, "让任务继续");
    assert_eq!(old.created_at.to_rfc3339(), "2026-01-01T00:00:30+00:00");
    assert_eq!(old.status.as_str(), "pending");
}
