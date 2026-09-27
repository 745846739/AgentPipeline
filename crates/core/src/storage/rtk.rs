//! 命令执行走 rtk 的全局开关（决策 297 / 票 03–04）。
//!
//! 单行表 `kanban_rtk`（迁移 0035），与 [`super::server_bind`] / [`super::notify_channel`] /
//! `kanban_foreman_watch` 同构：**机器级事实** + 三动作形状（读 / 写 / 清）。
//!
//! 行不存在 = 从没碰过设置 = **关**。与那几张表的差别只有缺省值的方向：值守轮缺省开
//! （它本来是自动跑的），rtk 缺省关（一个会改写命令串的优化器要人显式打开）。
//!
//! 这一行**不做两级结构**：没有 `config.toml` 那一级。rtk 是「这台机器上要不要用这个
//! 二进制」，配置文件那一级在决策 185 已裁「二进制怎么用、能不能用，由 `run_command` 与
//! 系统权限决定」——再造一份 `[pipeline] rtk = true` 就是同一件事的第二处真相。

use std::path::{Path, PathBuf};

use super::{ts, Store};
use crate::Result;

/// 开关那一行的读数。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RtkSwitch {
    /// 全局开关：关掉 = 命令按原样跑（**逐字等于这个功能出现之前**）。
    pub enabled: bool,
    /// 手填的兜底路径（绝对路径）。`None` = 自动解析。
    pub path: Option<PathBuf>,
}

impl Store {
    /// 读开关。**行不存在 = 缺省关**（不是错误）。
    pub async fn rtk_switch(&self) -> Result<RtkSwitch> {
        let row: Option<(bool, Option<String>)> =
            sqlx::query_as("SELECT enabled, path FROM kanban_rtk WHERE id = 1")
                .fetch_optional(self.pool())
                .await?;
        Ok(match row {
            None => RtkSwitch::default(),
            Some((enabled, path)) => RtkSwitch {
                enabled,
                // 空串归一成「没填过」：空串不是路径，它是「没有」的伪装（迁移 0004 的同一条）。
                path: path
                    .map(PathBuf::from)
                    .filter(|p| !p.as_os_str().is_empty()),
            },
        })
    }

    /// 写开关（`path` 为 `None` = 回到自动解析）。
    ///
    /// **探测失败也照写**（决策 297 / 票 03）：决策 185 已裁「二进制能不能用由系统权限
    /// 决定，本系统不另设一层」，一个输出优化器不该有权限拦人；而开发机上
    /// 「先开开关、后装二进制」是常见顺序。探测结果由端点回给界面，由界面原样摆出来。
    pub async fn set_rtk_switch(&self, enabled: bool, path: Option<&Path>) -> Result<()> {
        let path = path
            .map(|p| p.to_string_lossy().trim().to_string())
            .filter(|p| !p.is_empty());
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO kanban_rtk (id, enabled, path, updated_at) VALUES (1, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET enabled = excluded.enabled,
                                           path = excluded.path,
                                           updated_at = excluded.updated_at",
        )
        .bind(enabled)
        .bind(&path)
        .bind(ts(self.now()))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// 这一行**存不存在**：`true` = 界面保存过（哪怕值与缺省相同）。
    ///
    /// 「从没碰过设置」与「碰过、结果是关」在读数上要分得开（诚实口径，决策 257）——
    /// 设置页要说得出「这颗钮是我按的」还是「它本来就是关的」。
    pub async fn rtk_switch_has_override(&self) -> Result<bool> {
        let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM kanban_rtk WHERE id = 1")
            .fetch_optional(self.pool())
            .await?;
        Ok(exists.is_some())
    }

    /// 清掉这一行（回到缺省：关 + 自动解析）——「从没碰过设置」与「碰过、结果是关」
    /// 在读数上要分得开（`origin` 那一格），故清是一个独立的动作。
    pub async fn clear_rtk_switch(&self) -> Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("DELETE FROM kanban_rtk WHERE id = 1")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> (tempfile::TempDir, Store) {
        let tmp = tempfile::tempdir().unwrap();
        let home = crate::home::Home::new(tmp.path().join("home"));
        home.ensure_dirs().unwrap();
        let store = Store::open(home, std::sync::Arc::new(crate::clock::SystemClock))
            .await
            .unwrap();
        (tmp, store)
    }

    /// 缺省关，且「没碰过」与「碰过、结果是关」分得开（票 03 的判据：默认关时行为逐字等于今天）。
    #[tokio::test]
    async fn absent_row_means_off_and_automatic() {
        let (_tmp, store) = store().await;
        let switch = store.rtk_switch().await.unwrap();
        assert!(!switch.enabled, "缺省关");
        assert_eq!(switch.path, None);

        store.set_rtk_switch(false, None).await.unwrap();
        assert!(!store.rtk_switch().await.unwrap().enabled);
        // 值一样，但 provenance 变了：是界面按的，不是缺省
        assert!(store.rtk_switch_has_override().await.unwrap());

        // 清掉之后又回到缺省（读起来一样，但它是「没碰过」）
        store.clear_rtk_switch().await.unwrap();
        assert_eq!(store.rtk_switch().await.unwrap(), RtkSwitch::default());
        assert!(!store.rtk_switch_has_override().await.unwrap());
    }

    #[tokio::test]
    async fn switch_round_trips_with_a_manual_path() {
        let (_tmp, store) = store().await;
        store
            .set_rtk_switch(true, Some(Path::new("/usr/local/bin/rtk")))
            .await
            .unwrap();
        let switch = store.rtk_switch().await.unwrap();
        assert!(switch.enabled);
        assert_eq!(switch.path, Some(PathBuf::from("/usr/local/bin/rtk")));

        // 空白串不是路径：归一成「没填过」，而不是填了一个空路径
        store
            .set_rtk_switch(true, Some(Path::new("   ")))
            .await
            .unwrap();
        let switch = store.rtk_switch().await.unwrap();
        assert!(switch.enabled);
        assert_eq!(switch.path, None);
    }
}
