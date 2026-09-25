//! 「离线通知」设置页的存储（决策 272⑥⑦⑧；284② 添第二个单元）。
//!
//! 单行表 `kanban_notify_channel`（迁移 0027 / 0029）。与 [`super::server_bind`] /
//! [`super::market_repos`] 同构：机器级事实 + 两级结构——本表是**界面那一级**，
//! `config.toml` 的 `[notify]` 是基层。两级关系是**整体覆盖**（决策 272⑥）：
//! 通道四件（类型 + 端点 + password + 收件人）要么全来自界面、要么全来自配置，
//! 不允许混——`NotifyFormat` 与端点分属两级会让「界面指向 BlueBubbles 而配置说
//! feishu」这种状态有地方藏。
//!
//! **同表两单元**（决策 284②）：礼貌两件（节流 / 免打扰）是第二组，**各自成立**——
//! 通道可以来自界面而礼貌来自 `config.toml`（反之亦然）；不混的仍是**组内**。
//!
//! `enabled` 是一颗总开关（272⑧）：它**独立于两个单元存在**（都没有也能只关开关），
//! 故允许「有开关、无单元」的行。行不存在 = 从未碰过设置 = 开关开、两组单元都空
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
    /// 礼貌两件（决策 284）——三列同生同死；`cooldown_sec` 缺席即「整组没保存过」。
    pub cooldown_sec: Option<i64>,
    pub quiet_start: Option<i64>,
    pub quiet_end: Option<i64>,
}

#[derive(Debug, sqlx::FromRow)]
struct NotifyChannelSqlRow {
    enabled: bool,
    channel: Option<String>,
    webhook_url: Option<String>,
    bluebubbles_url: Option<String>,
    bluebubbles_password: Option<String>,
    bluebubbles_recipient: Option<String>,
    cooldown_sec: Option<i64>,
    quiet_start: Option<i64>,
    quiet_end: Option<i64>,
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
            cooldown_sec: self.cooldown_sec,
            quiet_start: self.quiet_start,
            quiet_end: self.quiet_end,
        }
    }

    /// 礼貌单元（决策 284）：三列齐 = 有单元；否则按「没保存过」处理——
    /// 半行只可能来自手改库，读取侧不该因此崩，也不该把半份单元当生效值。
    fn politeness(row: &NotifyChannelRow) -> Option<crate::notify::NotifyPoliteness> {
        match (row.cooldown_sec, row.quiet_start, row.quiet_end) {
            (Some(cooldown_sec), Some(quiet_start), Some(quiet_end)) => {
                Some(crate::notify::NotifyPoliteness {
                    cooldown_sec: cooldown_sec.max(0) as u64,
                    quiet_hours: [
                        quiet_start.clamp(0, 255) as u8,
                        quiet_end.clamp(0, 255) as u8,
                    ],
                })
            }
            _ => None,
        }
    }
}

/// 两级状态（行不存在 = 开关开、单元空）。
///
/// 单元**作为整体**存在：`channel IS NULL` 就是「没保存过」，不存在「保存了通道
/// 却丢了端点」的半行——写入路径只收四件齐的整体。礼貌单元（284②）同一条纪律、
/// **各自成立**：一组来自界面不影响另一组来自 `config.toml`。
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
                politeness: None,
            },
            Some(row) => crate::notify::NotifySettingsState {
                enabled: row.enabled,
                unit: row
                    .channel
                    .as_deref()
                    .and_then(crate::notify::NotifyFormat::parse)
                    .map(|channel| crate::notify::NotifyChannelOverride {
                        channel,
                        webhook_url: row.webhook_url.clone(),
                        bluebubbles_url: row.bluebubbles_url.clone(),
                        bluebubbles_password: row.bluebubbles_password.clone(),
                        bluebubbles_recipient: row.bluebubbles_recipient.clone(),
                    }),
                politeness: NotifyChannelSqlRow::politeness(&row),
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
    /// 「关掉通知」是两个动作，混在一起会让只想要前者的人丢掉后者。礼貌单元
    /// （284②）也不动：两组各自交还。
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

impl Store {
    /// 写**礼貌单元**（决策 284②，整体覆盖；开关与通道单元都不动）。
    ///
    /// 范围校验在设置端点落库前做（284⑤ 报错不静默）——这一层只管「三列一起写」。
    pub async fn set_notify_politeness(
        &self,
        politeness: &crate::notify::NotifyPoliteness,
    ) -> Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO kanban_notify_channel (id, cooldown_sec, quiet_start, quiet_end, updated_at)
             VALUES (1, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET cooldown_sec = excluded.cooldown_sec,
                                           quiet_start = excluded.quiet_start,
                                           quiet_end = excluded.quiet_end,
                                           updated_at = excluded.updated_at",
        )
        .bind(politeness.cooldown_sec as i64)
        .bind(politeness.quiet_hours[0] as i64)
        .bind(politeness.quiet_hours[1] as i64)
        .bind(ts(self.now()))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// 清掉礼貌单元（回到 `config.toml` 的 `[notify]`）；开关与通道单元都不动。
    pub async fn clear_notify_politeness(&self) -> Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "UPDATE kanban_notify_channel
             SET cooldown_sec = NULL, quiet_start = NULL, quiet_end = NULL, updated_at = ?
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

    /// 礼貌单元（决策 284）：与开关、通道单元**互不牵动**——三件事各有各的钮。
    #[tokio::test]
    async fn the_politeness_unit_is_independent_from_the_switch_and_the_channel() {
        let store = store().await;
        store.set_notify_enabled(false).await.unwrap();
        store
            .set_notify_channel(&NotifyChannelOverride {
                channel: NotifyFormat::Generic,
                webhook_url: Some("https://chat.example/hook".into()),
                bluebubbles_url: None,
                bluebubbles_password: None,
                bluebubbles_recipient: None,
            })
            .await
            .unwrap();
        store
            .set_notify_politeness(&crate::notify::NotifyPoliteness {
                cooldown_sec: 60,
                quiet_hours: [23, 7],
            })
            .await
            .unwrap();

        let state = store.notify_settings_state().await.unwrap();
        assert!(!state.enabled, "写礼貌不动开关");
        assert_eq!(
            state.unit.as_ref().map(|u| u.channel),
            Some(NotifyFormat::Generic),
            "写礼貌不动通道单元"
        );
        let politeness = state.politeness.expect("礼貌单元应当读得回来");
        assert_eq!(politeness.cooldown_sec, 60);
        assert_eq!(politeness.quiet_hours, [23, 7]);

        // 清礼貌：开关与通道单元都留在原处。
        store.clear_notify_politeness().await.unwrap();
        let state = store.notify_settings_state().await.unwrap();
        assert!(state.politeness.is_none());
        assert!(!state.enabled);
        assert!(state.unit.is_some(), "交还礼貌不动通道单元");

        // 反向：交还通道不动礼貌单元。
        store
            .set_notify_politeness(&crate::notify::NotifyPoliteness {
                cooldown_sec: 0,
                quiet_hours: [22, 8],
            })
            .await
            .unwrap();
        store.clear_notify_channel().await.unwrap();
        let state = store.notify_settings_state().await.unwrap();
        assert!(state.unit.is_none());
        assert_eq!(
            state.politeness.map(|p| p.cooldown_sec),
            Some(0),
            "交还通道不动礼貌单元（0 也是合法值，不是缺省）"
        );
    }

    /// 半行（只可能来自手改库）按「没保存过」处理：读取侧不崩，也不把半份当生效值。
    #[tokio::test]
    async fn a_half_politeness_row_reads_as_absent() {
        let store = store().await;
        store
            .set_notify_politeness(&crate::notify::NotifyPoliteness::default())
            .await
            .unwrap();
        // 手改库：抹掉其中一列。
        let mut tx = store.begin_write().await.unwrap();
        sqlx::query("UPDATE kanban_notify_channel SET quiet_end = NULL WHERE id = 1")
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let state = store.notify_settings_state().await.unwrap();
        assert!(
            state.politeness.is_none(),
            "三列不齐 = 没有单元（回落 config.toml）"
        );
    }
}
