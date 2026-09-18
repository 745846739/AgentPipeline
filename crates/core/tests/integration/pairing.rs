//! L2 集成：配对令牌存储（决策 182㉖㉗㉘，票 07）。
//!
//! 用真 SQLite 临时文件库 + 全量迁移（决策 145）验证存储层可观测的行为：
//! 首次读取生成并持久化、长期有效（不随启动 / 读取重生成）、重置换一枚、
//! 令牌字符集可安全落在 `?pair=` 查询里。中间件与端点的行为在 L3 契约测试里验。

use std::sync::Arc;

use agentpipeline_core::storage::Store;
use testkit::{ManualClock, TestHome};

async fn setup() -> (TestHome, Store, ManualClock) {
    let home = TestHome::new().unwrap();
    let (store, clock) = home.setup().await.unwrap();
    (home, store, clock)
}

#[tokio::test]
async fn token_is_generated_once_and_stays_stable() {
    let (_home, store, _clock) = setup().await;
    let first = store.pairing_token().await.unwrap();
    assert!(!first.is_empty());
    assert_eq!(
        store.pairing_token().await.unwrap(),
        first,
        "长期有效：读取不得重生成"
    );
}

#[tokio::test]
async fn token_survives_reopening_the_same_home() {
    // 「不随启动重生成」的存储侧证据：换一个 Store 句柄打开同一个库，读到同一枚令牌。
    let (home, store, clock) = setup().await;
    let token = store.pairing_token().await.unwrap();
    drop(store);

    let reopened = home.store(Arc::new(clock.clone())).await.unwrap();
    assert_eq!(reopened.pairing_token().await.unwrap(), token);
}

#[tokio::test]
async fn reset_rotates_the_token() {
    let (_home, store, _clock) = setup().await;
    let old = store.pairing_token().await.unwrap();
    let new = store.reset_pairing_token().await.unwrap();
    assert_ne!(old, new, "重置必须换一枚");
    assert_eq!(
        store.pairing_token().await.unwrap(),
        new,
        "重置后读取到的是新令牌"
    );
}

#[tokio::test]
async fn reset_works_without_a_prior_generation() {
    // 首启即重置：不能要求调用方先读一次，否则端点就得先「取一次再重置」。
    let (_home, store, _clock) = setup().await;
    let token = store.reset_pairing_token().await.unwrap();
    assert_eq!(store.pairing_token().await.unwrap(), token);
}

#[tokio::test]
async fn token_characters_are_url_safe() {
    // 令牌要原样落进 `{base}/?pair={token}`（无转义），故字符集必须 URL 安全。
    let (_home, store, _clock) = setup().await;
    let token = store.pairing_token().await.unwrap();
    assert!(
        token.len() >= 40,
        "两枚 ULID 拼接约 52 字符，实际 {}",
        token.len()
    );
    assert!(
        token.chars().all(|c| c.is_ascii_alphanumeric()),
        "令牌只该含 ASCII 字母数字：{token}"
    );
}
