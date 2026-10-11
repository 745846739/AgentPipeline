//! 迁移测试基建：三份逐字副本收拢的产物（票 foreman-work-record 04 的评审采纳项）。
//!
//! 「只跑到版本 N、用原始 SQL 造出老形态的行、再走 `Store::open` 跑全量迁移」是迁移
//! 用例的固定三拍——`foreman_sessions_migration.rs`（0012）与
//! `foreman_started_at_migration.rs`（0048）各抄过一份，第三份出现时按 rule of three
//! 收进来。`sqlx::migrate!` 按调用 crate 解析路径，故 migrator 由调用方（core 的集成
//! 测试，manifest 在 crates/core）构造好递进来，这里只做过滤、建池与执行。

use std::borrow::Cow;
use std::path::Path;

/// 把库迁到 `version`（**含**）为止——之后的迁移一概不跑。
pub async fn migrate_up_to(
    db: &Path,
    version: i64,
    migrator: &mut sqlx::migrate::Migrator,
) -> sqlx::SqlitePool {
    migrator.migrations = Cow::Owned(
        migrator
            .migrations
            .iter()
            .filter(|m| m.version <= version)
            .cloned()
            .collect(),
    );
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(db)
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    migrator.run(&pool).await.unwrap();
    pool
}
