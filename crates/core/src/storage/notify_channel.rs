//! 「离线通知」设置页的存储（决策 272⑥⑦⑧）。
//!
//! 单行表 `kanban_notify_channel`（迁移 0027）。与 [`super::server_bind`] /
//! [`super::market_repos`] 同构：机器级事实 + 两级结构——本表是**界面那一级**，
//! `config.toml` 的 `[notify]` 是基层。两级关系是**整体覆盖**（决策 272⑥）：
//! 通道四件（类型 + 端点 + password + 收件人）要么全来自界面、要么全来自配置，
//! 不允许混——`NotifyFormat` 与端点分属两级会让「界面指向 BlueBubbles 而配置说
//! feishu」这种状态有地方藏。
//!
//! `enabled` 是一颗总开关（272⑧）：它**独立于单元存在**（没有单元也能只关开关），
//! 故允许「有开关、无单元」的行。行不存在 = 从未碰过设置 = 开关开、单元空
//! （268 的既有姿态：`config.toml` 有 URL 就发）。

use super::{ts, Store};
use crate::Result;

/// 一行设置的原样读数（`channel` 还没解析成枚举——那是 `into_state` 的事）。
#[derive(Debug, Clone, PartialEq)]
pub struct NotifyChannelRow {
    pub enabled: bool,
    pub channel: Option<String>,
    pub webhook_url: Option<String>,
    pub bluebubbles_url: Option<String>,
    pub bluebubbles_password: Option<String>,
    pub bluebubbles_recipient: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct NotifyChannelSqlRow {
    enabled: bool,
    channel: Option<String>,
    webhook_url: Option<String>,
    bluebubbles_url: Option<String>,
    bluebubbles_password: Option<String>,
    bluebubbles_recipient: Option<String>,
}

impl NotifyChannelSqlRow {
    fn into_row(self) -> NotifyChannelRow {
        NotifyChannelRow {
            enabled: self.enabled,
            channel: self.channel,
            webhook_url: self.webhook_url,
            bluebubbles_url: self.bluebubbles_url,
            bluebubbles_password: self.bluebubbles_password,
            bluebubbles_recipient: self.bluebubbles_recipient,
        }
    }
}

/// 两级状态（行不存在 = 开关开、单元空）。
///
/// 单元**作为整体**存在：`channel IS NULL` 就是「没保存过」，不存在「保存了通道
/// 却丢了端点」的半行——写入路径只收四件齐的整体。
impl Store {
    pub async fn notify_settings_state(&self) -> Result<crate::notify::NotifySettingsState> {
        let row: Option<NotifyChannelSqlRow> =
            sqlx::query_as("SELECT * FROM kanban_notify_channel WHERE id = 1")
                .fetch_optional(self.pool())
                .await?;
        Ok(match row.map(NotifyChannelSqlRow::into_row) {
            None => crate::notify::NotifySettingsState {
                enabled: true,
                unit: None,
            },
            Some(row) => crate::notify::NotifySettingsState {
                enabled: row.enabled,
                unit: row
                    .channel
                    .as_deref()
                    .and_then(crate::notify::NotifyFormat::parse)
                    .map(|channel| crate::notify::NotifyChannelOverride {
                        channel,
                        webhook_url: row.webhook_url,
                        bluebubbles_url: row.bluebubbles_url,
                        bluebubbles_password: row.bluebubbles_password,
                        bluebubbles_recipient: row.bluebubbles_recipient,
                    }),
            },
        })
    }
}

impl Store {
    /// 写**总开关**（不动单元；行不存在就建一行）。
    pub async fn set_notify_enabled(&self, enabled: bool) -> Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO kanban_notify_channel (id, enabled, updated_at) VALUES (1, ?, ?)
             ON CONFLICT(id) DO UPDATE SET enabled = excluded.enabled, updated_at = excluded.updated_at",
        )
        .bind(enabled)
        .bind(ts(self.now()))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// 写**通道单元**（整体覆盖；开关不动）。
    ///
    /// 四件一起写、一起清：单元的完整性由调用方（设置端点）在落库前校验——这里只管
    /// 「要么全在、要么不在」。
    pub async fn set_notify_channel(
        &self,
        unit: &crate::notify::NotifyChannelOverride,
    ) -> Result<()> {
        let channel = unit.channel.as_str().to_string();
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO kanban_notify_channel
                 (id, channel, webhook_url, bluebubbles_url, bluebubbles_password,
                  bluebubbles_recipient, updated_at)
             VALUES (1, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET channel = excluded.channel,
                                           webhook_url = excluded.webhook_url,
                                           bluebubbles_url = excluded.bluebubbles_url,
                                           bluebubbles_password = excluded.bluebubbles_password,
                                           bluebubbles_recipient = excluded.bluebubbles_recipient,
                                           updated_at = excluded.updated_at",
        )
        .bind(&channel)
        .bind(&unit.webhook_url)
        .bind(&unit.bluebubbles_url)
        .bind(&unit.bluebubbles_password)
        .bind(&unit.bluebubbles_recipient)
        .bind(ts(self.now()))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// 清掉单元（回到 `config.toml` 那一级）；**开关不动**——「交还配置文件」与
    /// 「关掉通知」是两个动作，混在一起会让只想要前者的人丢掉后者。
    pub async fn clear_notify_channel(&self) -> Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "UPDATE kanban_notify_channel
             SET channel = NULL, webhook_url = NULL, bluebubbles_url = NULL,
                 bluebubbles_password = NULL, bluebubbles_recipient = NULL, updated_at = ?
             WHERE id = 1",
        )
        .bind(ts(self.now()))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::SystemClock;
    use crate::notify::{NotifyChannelOverride, NotifyFormat};
    use std::sync::Arc;

    async fn store() -> Store {
        Store::open_in_memory(Arc::new(SystemClock)).await.unwrap()
    }

    /// 行不存在 = 开关开、单元空（268 的既有姿态原样保留：没碰过设置就照 config 走）。
    #[tokio::test]
    async fn a_missing_row_means_enabled_with_no_unit() {
        let store = store().await;
        let state = store.notify_settings_state().await.unwrap();
        assert!(state.enabled);
        assert!(state.unit.is_none());
    }

    /// 只写开关：得到「有开关、无单元」的行——关掉通知不必先配通道。
    #[tokio::test]
    async fn the_switch_can_be_persisted_without_a_unit() {
        let store = store().await;
        store.set_notify_enabled(false).await.unwrap();
        let state = store.notify_settings_state().await.unwrap();
        assert!(!state.enabled);
        assert!(state.unit.is_none());
    }

    /// 单元整体落库、整体清空；清单元不动开关。
    #[tokio::test]
    async fn the_unit_is_saved_and_cleared_as_a_whole() {
        let store = store().await;
        store.set_notify_enabled(true).await.unwrap();
        store
            .set_notify_channel(&NotifyChannelOverride {
                channel: NotifyFormat::BlueBubbles,
                webhook_url: None,
                bluebubbles_url: Some("http://127.0.0.1:1234".into()),
                bluebubbles_password: Some("pw".into()),
                bluebubbles_recipient: Some("me@icloud.com".into()),
            })
            .await
            .unwrap();
        let state = store.notify_settings_state().await.unwrap();
        assert!(state.enabled);
        let unit = state.unit.expect("单元应当读得回来");
        assert_eq!(unit.channel, NotifyFormat::BlueBubbles);
        assert_eq!(unit.bluebubbles_password.as_deref(), Some("pw"));

        store.clear_notify_channel().await.unwrap();
        let state = store.notify_settings_state().await.unwrap();
        assert!(state.enabled, "清单元不动开关");
        assert!(state.unit.is_none());
    }
}
