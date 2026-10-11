//! 迁移 0049 在**既有数据**上打开（决策 427，票 talk-cli-engine 05）。
//!
//! 提议表加 `origin` 来路列。存量提议全部落 `api`：0049 之前世界上只有 API 值班长
//! 一条提议通道（CLI 通道是决策 427 才开的），把旧行标成别的就是篡改账目。做法照
//! `foreman_started_at_migration.rs` 的先例：只跑到 0048，原始 SQL 造老形态的行，
//! 再走 `Store::open`（跑全量迁移）核对。CHECK 约束的写入点挡法一并钉住。

use std::borrow::Cow;
use std::sync::Arc;

use agentpipeline_core::storage::proposals::{
    NewForemanProposal, FOREMAN_PROPOSAL_ORIGIN_API, FOREMAN_PROPOSAL_ORIGIN_CLI,
};
use agentpipeline_core::storage::Store;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use testkit::{ManualClock, TestHome};

/// 把库迁到 `0048`（0049 之前的那一版）。
async fn migrate_to_0048(db: &std::path::Path) -> sqlx::SqlitePool {
    let mut migrator = sqlx::migrate!("src/storage/migrations");
    migrator.migrations = Cow::Owned(
        migrator
            .migrations
            .iter()
            .filter(|m| m.version <= 48)
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
async fn the_origin_backfill_marks_every_existing_proposal_api() {
    let home = TestHome::new().unwrap();
    let pool = migrate_to_0048(&home.home().db_path()).await;

    sqlx::query(
        "INSERT INTO kanban_foreman_sessions (id, title, kind, engine, created_at, last_active_at)
         VALUES ('s1', '老班次', 'talk', 'api', '2026-01-01T00:00:00+00:00',
                 '2026-01-01T00:00:00+00:00')",
    )
    .execute(&pool)
    .await
    .unwrap();
    // 老形态：提议行没有 origin 列——0049 之前的形状。
    sqlx::query(
        "INSERT INTO kanban_foreman_proposals
         (id, session_id, tool, args_json, summary, situation_json, kind, payload_json,
          claimed_at, created_at, expires_at, status, resolved_at)
         VALUES ('p1', 's1', 'task', '{}', '取消任务 t1', NULL, 'api_call', NULL,
                 NULL, '2026-01-01T00:01:00+00:00', '2026-01-01T00:11:00+00:00',
                 'pending', NULL)",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;

    // ── 正式打开：跑全量迁移（含 0049）──
    let clock = ManualClock::fixed();
    let store = Store::open(home.home().clone(), Arc::new(clock))
        .await
        .expect("在既有数据上打开必须成功");

    let proposal = store
        .get_foreman_proposal("p1")
        .await
        .unwrap()
        .expect("迁移不动任何一行");
    assert_eq!(proposal.origin, FOREMAN_PROPOSAL_ORIGIN_API);
    // 其余字段原样：迁移只加列，不改写任何既有读数。
    assert_eq!(proposal.tool, "task");
    assert_eq!(proposal.summary, "取消任务 t1");
    assert_eq!(proposal.status.as_str(), "pending");
}

#[tokio::test]
async fn origin_is_checked_at_the_write_point() {
    let home = TestHome::new().unwrap();
    let clock = ManualClock::fixed();
    let store = Store::open(home.home().clone(), Arc::new(clock))
        .await
        .unwrap();

    // 提议表挂着班次外键：先落一个 CLI 班次。
    let session = store
        .create_foreman_session_with_engine(
            agentpipeline_core::storage::foreman::FOREMAN_SESSION_KIND_TALK,
            "",
            agentpipeline_core::storage::foreman::FOREMAN_ENGINE_CLI,
        )
        .await
        .unwrap();

    // 合法来路照常落库，读回来一字不差。
    let proposal = store
        .create_foreman_proposal(NewForemanProposal {
            session_id: session.id.clone(),
            tool: "task".into(),
            args: serde_json::json!({"action": "cancel", "task_id": "t1"}),
            summary: "取消任务 t1".into(),
            situation: None,
            kind: agentpipeline_core::storage::proposals::ForemanProposalKind::ApiCall,
            payload: None,
            origin: FOREMAN_PROPOSAL_ORIGIN_CLI,
        })
        .await
        .unwrap();
    assert_eq!(proposal.origin, FOREMAN_PROPOSAL_ORIGIN_CLI);
    let read_back = store
        .get_foreman_proposal(&proposal.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read_back.origin, FOREMAN_PROPOSAL_ORIGIN_CLI);

    // 非法来路在写入点被挡住（CHECK），读取侧不防坏数据。
    let bad = store
        .create_foreman_proposal(NewForemanProposal {
            session_id: session.id.clone(),
            tool: "task".into(),
            args: serde_json::json!({"action": "cancel", "task_id": "t1"}),
            summary: "取消任务 t1".into(),
            situation: None,
            kind: agentpipeline_core::storage::proposals::ForemanProposalKind::ApiCall,
            payload: None,
            origin: "mcp",
        })
        .await;
    assert!(bad.is_err(), "CHECK 约束必须挡住 origin 之外的取值");
}
