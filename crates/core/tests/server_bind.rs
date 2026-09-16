//! L2 集成：界面上的绑定开关存储（决策 186）。
//!
//! 用真 SQLite 临时文件库 + 全量迁移（决策 145）验证存储层可观测的行为：
//! 未设置过 → `None`；写入后持久（换一个句柄打开同一个 home 仍读到）；覆盖是替换不是
//! 追加；清除后回到 `None`。改绑本身与端点护栏在 L2.5 的 `lan_bind.rs`（真二进制）与
//! L3 契约测试里验。

use std::sync::Arc;

use agentpipeline_core::storage::Store;
use testkit::{ManualClock, TestHome};

async fn setup() -> (TestHome, Store, ManualClock) {
    let home = TestHome::new().unwrap();
    let (store, clock) = home.setup().await.unwrap();
    (home, store, clock)
}

#[tokio::test]
async fn missing_row_means_no_override() {
    // 从未按过那颗钮 → `None`，启动解析据此回落 `[server] host`。
    let (_home, store, _clock) = setup().await;
    assert_eq!(store.server_bind_override().await.unwrap(), None);
}

#[tokio::test]
async fn override_survives_reopening_the_same_home() {
    // 「重启仍生效」的存储侧证据：换一个 Store 句柄打开同一个库，读到同一个值。
    let (home, store, clock) = setup().await;
    store.set_server_bind_override("0.0.0.0").await.unwrap();
    drop(store);

    let reopened = home.store(Arc::new(clock.clone())).await.unwrap();
    assert_eq!(
        reopened.server_bind_override().await.unwrap(),
        Some("0.0.0.0".to_string())
    );
}

#[tokio::test]
async fn writing_twice_replaces_rather_than_appends() {
    // 单行表的不变式：第二次写是替换。若它变成两行，读取会拿到旧值，
    // 表现为「关掉局域网访问之后重启还是局域网」——正是 CHECK (id = 1) 要挡的那类。
    let (_home, store, _clock) = setup().await;
    store.set_server_bind_override("0.0.0.0").await.unwrap();
    store.set_server_bind_override("127.0.0.1").await.unwrap();
    assert_eq!(
        store.server_bind_override().await.unwrap(),
        Some("127.0.0.1".to_string())
    );
}

#[tokio::test]
async fn clear_removes_the_override_and_reports_it() {
    let (_home, store, _clock) = setup().await;
    store.set_server_bind_override("0.0.0.0").await.unwrap();
    assert!(store.clear_server_bind_override().await.unwrap());
    assert_eq!(store.server_bind_override().await.unwrap(), None);
    // 再清一次：没东西可清，如实返回 false（不假装改了）
    assert!(!store.clear_server_bind_override().await.unwrap());
}

#[tokio::test]
async fn clearing_without_a_prior_write_is_not_an_error() {
    // 首启就点「恢复配置文件的值」：不能要求调用方先设置过一次。
    let (_home, store, _clock) = setup().await;
    assert!(!store.clear_server_bind_override().await.unwrap());
    assert_eq!(store.server_bind_override().await.unwrap(), None);
}
