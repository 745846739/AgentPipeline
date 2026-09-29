//! L2 集成：存储 I/O 预算（决策 321）。
//!
//! 用真 SQLite 临时文件库 + 全量迁移（决策 145）验证三件事：
//! `synchronous = NORMAL` 真的落在连接上、维护作业真的走维护专用连接
//! （维护期间主池照常可用）、checkpoint 真的把 WAL 收缩回零并带回水位读数。

use agentpipeline_core::storage::{io_budget, Store};
use testkit::TestHome;

async fn setup() -> (TestHome, Store) {
    let home = TestHome::new().unwrap();
    let (store, _clock) = home.setup().await.unwrap();
    (home, store)
}

/// 决策 321①：`synchronous = NORMAL` 必须是**连接上的事实**，不是注释里的愿望。
/// 判 `PRAGMA synchronous` = 1（FULL=2 / OFF=0）。
#[tokio::test]
async fn store_opens_with_synchronous_normal() {
    let (_home, store) = setup().await;
    let value: i64 = sqlx::query_scalar("PRAGMA synchronous")
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert_eq!(value, 1, "synchronous 应为 NORMAL(1)，读到 {value}");

    // 维护连接与主池**同参**（connect_options 唯一出处）：两边任何一边漂移都在
    // 制造两类连接——那正是本票要消灭的东西。
    let mut conn = store.maintenance_connection().await.unwrap();
    let value: i64 = sqlx::query_scalar("PRAGMA synchronous")
        .fetch_one(&mut conn)
        .await
        .unwrap();
    assert_eq!(value, 1, "维护连接的 synchronous 也应为 NORMAL(1)");
}

/// 决策 321②：保留期 DELETE 的专用连接**不向主池借道**——判据：主池被占满时，
/// 维护连接照常立刻可用，而主池自己的 acquire 只能干等（这正是旧实现里
/// 「重 DELETE 占池 → 主流程慢 acquire 叠到 24s」的反面）。
///
/// 范围声明：维护作业里**小表清理**（proposals / attention / 指标聚合）仍走主池，
/// 本票只把重 DELETE 与 checkpoint 移出——所以这里断言的是「隔离存在」，
/// 不是「维护全程零池依赖」。
#[tokio::test]
async fn maintenance_connection_is_isolated_from_pool() {
    let (_home, store) = setup().await;
    // 占满主池的 5 条连接（只占不锁：不制造锁竞争，只制造「池里没座了」）。
    let held: Vec<_> = futures::future::join_all((0..5).map(|_| store.pool().acquire()))
        .await
        .into_iter()
        .collect::<Result<_, _>>()
        .unwrap();

    // 主池此刻确实借不到座：acquire 在 300ms 内不可能返回（acquire_timeout 30s）。
    let starved = tokio::time::timeout(
        std::time::Duration::from_millis(300),
        store.pool().acquire(),
    )
    .await;
    assert!(starved.is_err(), "池应该已被占满");

    // 而维护连接**立刻**可用，且重 DELETE 的语义在它上面照常成立。
    let mut conn = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        store.maintenance_connection(),
    )
    .await
    .expect("维护连接不该排队等池")
    .unwrap();
    let affected = sqlx::query(
        "DELETE FROM kanban_node_conversations
         WHERE task_id IN (SELECT id FROM kanban_tasks WHERE status IN ('done','failed','cancelled'))
           AND created_at < ?",
    )
    .bind("2026-09-01T00:00:00Z")
    .execute(&mut conn)
    .await
    .unwrap()
    .rows_affected();
    assert_eq!(affected, 0, "空库没有可清的行——语义正常即可");

    drop(held);
}

/// 决策 321③：checkpoint 把 WAL 收缩回零，并带回**真实**的水位读数。
#[tokio::test]
async fn checkpoint_truncates_wal_and_reports_watermarks() {
    let (home, store) = setup().await;
    // 制造一段真实的 WAL：写若干条自动迁移之外的行。
    {
        let mut conn = store.maintenance_connection().await.unwrap();
        sqlx::query("CREATE TABLE io_budget_probe (id INTEGER PRIMARY KEY, v TEXT)")
            .execute(&mut conn)
            .await
            .unwrap();
        for i in 0..50 {
            sqlx::query("INSERT INTO io_budget_probe (v) VALUES (?)")
                .bind(format!("row-{i}"))
                .execute(&mut conn)
                .await
                .unwrap();
        }
    }
    let outcome = store.checkpoint_wal().await.unwrap();
    assert!(!outcome.busy, "测试进程没有长读者，TRUNCATE 不该被挡");
    let db_path = home.home().db_path();
    let wal = db_path.with_file_name(format!(
        "{}-wal",
        db_path.file_name().unwrap().to_string_lossy()
    ));
    if wal.exists() {
        let size = std::fs::metadata(&wal).unwrap().len();
        assert_eq!(size, 0, "TRUNCATE 后 WAL 文件应为 0 字节，实际 {size}");
    }
    assert!(outcome.db_bytes.unwrap_or(0) > 0, "库文件字节数该是正数");
    assert!(
        outcome.disk_free_bytes.unwrap_or(0) > 0,
        "磁盘剩余空间该是正数"
    );

    // 水位读数函数与 checkpoint 口径一致（同一拼法找 -wal）。
    assert_eq!(
        io_budget::wal_bytes(&db_path),
        outcome.wal_bytes_after,
        "wal_bytes 读数与 checkpoint 回报不一致"
    );
}
