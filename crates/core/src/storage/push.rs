//! 浏览器推送的订阅行与 VAPID 密钥（spec `.scratch/pwa-webpush/` 票 02）。
//!
//! 两张东西的存储面（形状与理由见迁移 `0037_push_subscription.sql`）：
//!
//! - **订阅行**（`kanban_push_subscription`，每设备一行，`endpoint` 唯一键）：浏览器
//!   `pushManager.subscribe()` 的产出。同一个 `endpoint` 重复订阅**落一行**——
//!   upsert 而不是「先删后插」（同 [`super::server_bind`] 的 UPSERT 理由：中间态会被
//!   并发的清单读看见，而「清单里那台设备闪了一下」正是用户会截图来问的那类现象）。
//! - **VAPID 密钥对**（`kanban_notify_channel` 那行单行表的两列，0027 的 `CHECK (id = 1)`
//!   原样沿用）：整台机器一对，**首次启用自动生成**（[`Store::ensure_push_vapid_keys`]）。
//!
//! 读接口给的**摘要**（[`PushSubscription::endpoint_hint`]）是刻意的：清单要能认出
//! 「这是我那台 iPhone」，而完整 endpoint 是一枚能力 URL——把它交给每一个能读设置页的
//! 人，等于把「往那台设备推任何东西」的能力也交出去（`auth` 才是完整的伪造能力，
//! 但 endpoint 是它的一半）。掩码口径对齐 providers `api_key`（决策 112）。

use super::{ts, Store};
use crate::Result;
use chrono::{DateTime, Utc};

/// 一条订阅的原样读数（`p256dh` / `auth` 是秘密，只在本层与发送侧之间传）。
#[derive(Debug, Clone, PartialEq)]
pub struct PushSubscription {
    pub id: i64,
    /// 推送服务给这台设备的地址（能力 URL）。
    pub endpoint: String,
    /// 浏览器公钥（P-256 未压缩点，base64url）。
    pub p256dh: String,
    /// 鉴权秘密（16 字节，base64url）。
    pub auth: String,
    pub user_agent: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl PushSubscription {
    /// 清单里给用户看的那一小截 endpoint：**只够认出是哪台设备**，不够拿去推。
    ///
    /// 取路径的最后一段（推送服务把设备标识放在那里，如
    /// `https://push.apple.com/…/abc123def`），保留前 6 位与尾 6 位——同一台设备
    /// 两次订阅的那一段是同一个值，用户可以据此对上「设置页那行 = 我手机上这条」。
    pub fn endpoint_hint(&self) -> String {
        let tail = self
            .endpoint
            .rsplit('/')
            .find(|seg| !seg.is_empty())
            .unwrap_or(&self.endpoint);
        let chars: Vec<char> = tail.chars().collect();
        if chars.len() <= 16 {
            return format!("…{}", tail);
        }
        let head: String = chars.iter().take(6).collect();
        let back: String = chars.iter().skip(chars.len() - 6).collect();
        format!("…{head}…{back}")
    }
}

/// VAPID 密钥对（RFC 8292）：`public_key` 是未压缩点（base64url，浏览器订阅时用它），
/// `private_key` 是 PKCS#8（base64url，签名时用）——两个都是 base64url **无填充**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VapidKeys {
    pub public_key: String,
    pub private_key: String,
}

#[derive(Debug, sqlx::FromRow)]
struct PushSubscriptionRow {
    id: i64,
    endpoint: String,
    p256dh: String,
    auth: String,
    user_agent: Option<String>,
    created_at: String,
}

impl Store {
    /// 按 `endpoint` upsert 一行订阅（同 endpoint 两次订阅 = 一行）。
    ///
    /// `created_at` **只在首次插入时定**：那行说的是「这台设备什么时候开始订阅的」，
    /// 而设备每次启动都可能重新 `subscribe()` 一次（同一个 endpoint）——把它刷成「刚刚」
    /// 会让设置页的时间列失去意义（用户问的正是「这台是什么时候接进来的」）。
    /// `user_agent` 同理保留首次那一份吗？不——UA 跟着浏览器升级变（Safari 18 → 19），
    /// 而「这台设备是谁」以最新的自我描述为准，故它随每次 upsert 刷新。
    pub async fn upsert_push_subscription(
        &self,
        endpoint: &str,
        p256dh: &str,
        auth: &str,
        user_agent: Option<&str>,
    ) -> Result<i64> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO kanban_push_subscription
                 (endpoint, p256dh, auth, user_agent, created_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(endpoint) DO UPDATE SET p256dh = excluded.p256dh,
                                                  auth = excluded.auth,
                                                  user_agent = excluded.user_agent",
        )
        .bind(endpoint)
        .bind(p256dh)
        .bind(auth)
        .bind(user_agent)
        .bind(ts(self.now()))
        .execute(&mut *tx)
        .await?;
        // `RETURNING id` 而不是再查一遍：同一把事务里读，不担心并发删插之间换了一行。
        let id: i64 =
            sqlx::query_scalar("SELECT id FROM kanban_push_subscription WHERE endpoint = ?")
                .bind(endpoint)
                .fetch_one(&mut *tx)
                .await?;
        tx.commit().await?;
        Ok(id)
    }

    /// 全部订阅（订阅时间升序 = 设置页的排序）。
    pub async fn list_push_subscriptions(&self) -> Result<Vec<PushSubscription>> {
        let rows: Vec<PushSubscriptionRow> = sqlx::query_as(
            "SELECT id, endpoint, p256dh, auth, user_agent, created_at
             FROM kanban_push_subscription ORDER BY created_at, id",
        )
        .fetch_all(self.pool())
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(PushSubscription {
                    id: row.id,
                    endpoint: row.endpoint,
                    p256dh: row.p256dh,
                    auth: row.auth,
                    user_agent: row.user_agent,
                    created_at: super::parse_ts(&row.created_at)?,
                })
            })
            .collect()
    }

    /// 删一行（单个撤销 / 推送服务判死之后的清行）。返回是否真的删掉了一行。
    pub async fn delete_push_subscription(&self, id: i64) -> Result<bool> {
        let mut tx = self.begin_write().await?;
        let rows = sqlx::query("DELETE FROM kanban_push_subscription WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        Ok(rows > 0)
    }

    /// 全部清空（设置页的「全部清空」）。返回删掉了几行。
    pub async fn clear_push_subscriptions(&self) -> Result<u64> {
        let mut tx = self.begin_write().await?;
        let rows = sqlx::query("DELETE FROM kanban_push_subscription")
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        Ok(rows)
    }

    /// 读 VAPID 密钥对；没生成过 → `None`。
    pub async fn push_vapid_keys(&self) -> Result<Option<VapidKeys>> {
        let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT vapid_public_key, vapid_private_key FROM kanban_notify_channel WHERE id = 1",
        )
        .fetch_optional(self.pool())
        .await?;
        Ok(row.and_then(
            |(public_key, private_key)| match (public_key, private_key) {
                (Some(public_key), Some(private_key)) => Some(VapidKeys {
                    public_key,
                    private_key,
                }),
                // 半行（只手改过库）按「没生成过」处理：拿半对密钥去签名只会得到
                // 「推送服务回 401」这种与病因隔了三层的现象。
                _ => None,
            },
        ))
    }

    /// **首次启用自动生成**、之后恒返回同一对（幂等）。
    ///
    /// 为什么不给用户看/填这对密钥：它们是服务端身份，不是用户偏好——手工生成一对
    /// P-256 密钥并粘贴不是用户该做的事（而这正是「零手工配置」的全部内容）。
    /// 生成失败（系统随机源不可用）是**错误**而不是「先不生成」：没有密钥就没法订阅，
    /// 静默继续只会把失败推迟到用户按下订阅钮的那一刻。
    pub async fn ensure_push_vapid_keys(&self) -> Result<VapidKeys> {
        if let Some(keys) = self.push_vapid_keys().await? {
            return Ok(keys);
        }
        let keys = crate::webpush::generate_vapid_keys().map_err(crate::Error::Config)?;
        let mut tx = self.begin_write().await?;
        // `COALESCE` 而不是直接覆盖：两个并发调用（设置页保存 + 订阅上报）都在缺件时
        // 走到这里，谁先落库谁的键生效，后到的**不覆盖**——覆盖会让先到的那个浏览器
        // 已经拿去订阅的公钥对不上私钥，症状是「这台设备从此收不到推送」。
        sqlx::query(
            "INSERT INTO kanban_notify_channel (id, vapid_public_key, vapid_private_key, updated_at)
             VALUES (1, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                 vapid_public_key = COALESCE(kanban_notify_channel.vapid_public_key, excluded.vapid_public_key),
                 vapid_private_key = COALESCE(kanban_notify_channel.vapid_private_key, excluded.vapid_private_key)",
        )
        .bind(&keys.public_key)
        .bind(&keys.private_key)
        .bind(ts(self.now()))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        // 落库之后再读一遍：并发路径下可能是**别人**先落的那一对，返回它才是真的
        // （返回自己刚生成的那一对会让调用方拿着一个不会生效的公钥去订阅）。
        self.push_vapid_keys()
            .await?
            .ok_or_else(|| crate::Error::Config("VAPID 密钥写入后读不回来".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::SystemClock;
    use std::sync::Arc;

    async fn store() -> Store {
        Store::open_in_memory(Arc::new(SystemClock)).await.unwrap()
    }

    /// 同一个 endpoint 两次订阅落一行；`created_at` 钉在第一次（清单的时间列说的是
    /// 「这台设备什么时候接进来的」，不是「它最后一次自报是什么时候」）。
    #[tokio::test]
    async fn the_same_endpoint_upserts_into_one_row() {
        let store = store().await;
        let first = store
            .upsert_push_subscription(
                "https://push.example/apple-1",
                "p1",
                "a1",
                Some("iPhone Safari 18"),
            )
            .await
            .unwrap();
        let created = store.list_push_subscriptions().await.unwrap()[0].created_at;

        let second = store
            .upsert_push_subscription(
                "https://push.example/apple-1",
                "p2",
                "a2",
                Some("iPhone Safari 19"),
            )
            .await
            .unwrap();

        assert_eq!(first, second, "同 endpoint 是同一行");
        let rows = store.list_push_subscriptions().await.unwrap();
        assert_eq!(rows.len(), 1, "清单里只有一行：{rows:?}");
        assert_eq!(rows[0].p256dh, "p2", "密钥随每次上报刷新");
        assert_eq!(
            rows[0].user_agent.as_deref(),
            Some("iPhone Safari 19"),
            "UA 跟着浏览器升级变"
        );
        assert_eq!(rows[0].created_at, created, "订阅时刻只在首次定下");
    }

    /// 不同 endpoint = 不同设备 = 不同行（iPhone 与桌面浏览器各自订阅）。
    #[tokio::test]
    async fn different_endpoints_are_different_devices() {
        let store = store().await;
        store
            .upsert_push_subscription("https://push.example/apple-1", "p", "a", Some("iPhone"))
            .await
            .unwrap();
        store
            .upsert_push_subscription("https://push.example/chrome-2", "p", "a", None)
            .await
            .unwrap();
        assert_eq!(store.list_push_subscriptions().await.unwrap().len(), 2);
    }

    /// 单删与清空（设置页的两颗钮）；单删不存在的行是 `false` 而不是报错。
    #[tokio::test]
    async fn delete_one_and_clear_all() {
        let store = store().await;
        let a = store
            .upsert_push_subscription("https://push.example/a", "p", "a", None)
            .await
            .unwrap();
        store
            .upsert_push_subscription("https://push.example/b", "p", "a", None)
            .await
            .unwrap();

        assert!(store.delete_push_subscription(a).await.unwrap());
        assert!(
            !store.delete_push_subscription(a).await.unwrap(),
            "已经删掉的行再删一次是 no-op"
        );
        assert_eq!(store.list_push_subscriptions().await.unwrap().len(), 1);

        assert_eq!(store.clear_push_subscriptions().await.unwrap(), 1);
        assert!(store.list_push_subscriptions().await.unwrap().is_empty());
    }

    /// 清空删的是订阅行，**不是** VAPID 密钥：密钥是服务端身份（与订阅同生命周期，
    /// 但不清订单一说），清掉它会让所有还在浏览器里活着的订阅全部作废。
    #[tokio::test]
    async fn clearing_subscriptions_keeps_the_vapid_pair() {
        let store = store().await;
        let keys = store.ensure_push_vapid_keys().await.unwrap();
        store
            .upsert_push_subscription("https://push.example/a", "p", "a", None)
            .await
            .unwrap();
        store.clear_push_subscriptions().await.unwrap();
        assert_eq!(
            store.push_vapid_keys().await.unwrap(),
            Some(keys),
            "清订阅不动密钥对"
        );
    }

    /// 首次生成、之后幂等（同一条读了两次还是那一对）。
    #[tokio::test]
    async fn vapid_keys_are_generated_once_then_stable() {
        let store = store().await;
        assert!(
            store.push_vapid_keys().await.unwrap().is_none(),
            "没生成过时读回 None（不是伪造一对）"
        );
        let first = store.ensure_push_vapid_keys().await.unwrap();
        let second = store.ensure_push_vapid_keys().await.unwrap();
        assert_eq!(first, second, "第二次调用拿回同一对");
        assert_eq!(store.push_vapid_keys().await.unwrap(), Some(first.clone()));
        // 形状：公钥是未压缩点（65 字节 → base64url 87 个字符），私钥非空。
        assert_eq!(first.public_key.len(), 87, "{}", first.public_key);
        assert!(!first.private_key.is_empty());
    }

    /// 半行（只手改过库）按「没生成过」处理：不拿半对密钥去签名。
    #[tokio::test]
    async fn a_half_vapid_row_reads_as_absent() {
        let store = store().await;
        store.ensure_push_vapid_keys().await.unwrap();
        let mut tx = store.begin_write().await.unwrap();
        sqlx::query("UPDATE kanban_notify_channel SET vapid_private_key = NULL WHERE id = 1")
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert!(store.push_vapid_keys().await.unwrap().is_none());
        // 而且「再 ensure 一次」会把缺的那一件补回来（不是永远卡在半行上）。
        let healed = store.ensure_push_vapid_keys().await.unwrap();
        assert_eq!(store.push_vapid_keys().await.unwrap(), Some(healed));
    }

    /// 清单里的 endpoint 摘要够认出设备、不够拿去推。
    #[test]
    fn the_endpoint_hint_keeps_the_tail_but_not_the_whole_capability_url() {
        let sub = PushSubscription {
            id: 1,
            endpoint: "https://web.push.apple.com/QABC123/device-abcdefghijklmn".into(),
            p256dh: "p".into(),
            auth: "a".into(),
            user_agent: None,
            created_at: Utc::now(),
        };
        let hint = sub.endpoint_hint();
        assert!(
            !hint.contains("web.push.apple.com"),
            "主机不出现在清单里：{hint}"
        );
        assert!(hint.contains("device"), "{hint}");
        assert!(hint.starts_with('…'), "{hint}");
        assert!(hint.len() < sub.endpoint.len(), "{hint}");
    }
}
