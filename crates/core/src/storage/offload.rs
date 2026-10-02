//! 重活外发 GitHub 的全局开关（票 runner-offload/05）。
//!
//! 单行表 `kanban_offload`（迁移 0040），与 [`super::rtk`]（决策 297）同构：
//! **机器级事实** + 三动作形状（读 / 写 / 清）。唯一旋钮是 enabled——外发的
//! 对象（哪些操作）、回退语义、结果形态都在票 06 的工具层钉死，不进配置：
//! 那些是行为不是偏好，配置面上多一个旋钮就多一处「设置与行为漂移」的机会。
//!
//! 行不存在 = 从没碰过设置 = **关 = 全部本机运行**。同一行不做 `config.toml`
//! 两级结构：理由与 rtk 一致（决策 297 的注释原话），再造一份就是第二处真相。

use super::{ts, Store};
use crate::Result;

/// 开关那一行的读数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OffloadSwitch {
    /// 全局开关：关 = 全部本机运行（**逐字等于这个功能出现之前**）。
    pub enabled: bool,
}

impl Store {
    /// 读开关。**行不存在 = 缺省关**（不是错误）。
    pub async fn offload_switch(&self) -> Result<OffloadSwitch> {
        let row: Option<(bool,)> =
            sqlx::query_as("SELECT enabled FROM kanban_offload WHERE id = 1")
                .fetch_optional(self.pool())
                .await?;
        Ok(match row {
            None => OffloadSwitch::default(),
            Some((enabled,)) => OffloadSwitch { enabled },
        })
    }

    /// 写开关。**探测失败也照写**（与 rtk 同一条纪律，决策 297）：
    /// 「先开开关、后在 106 登录 gh」是共识里写明的顺序，设置层不拦。
    pub async fn set_offload_switch(&self, enabled: bool) -> Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO kanban_offload (id, enabled, updated_at) VALUES (1, ?, ?)
             ON CONFLICT(id) DO UPDATE SET enabled = excluded.enabled,
                                           updated_at = excluded.updated_at",
        )
        .bind(enabled)
        .bind(ts(self.now()))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// 这一行**存不存在**：`true` = 界面保存过（哪怕值与缺省相同）。
    /// 「从没碰过」与「碰过、结果是关」分得开（诚实口径，决策 257）。
    pub async fn offload_switch_has_override(&self) -> Result<bool> {
        let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM kanban_offload WHERE id = 1")
            .fetch_optional(self.pool())
            .await?;
        Ok(exists.is_some())
    }

    /// 清掉这一行（回到缺省：关）。
    pub async fn clear_offload_switch(&self) -> Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("DELETE FROM kanban_offload WHERE id = 1")
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

    /// 缺省关，且「没碰过」与「碰过、结果是关」分得开（共识：默认本机运行）。
    #[tokio::test]
    async fn absent_row_means_off() {
        let (_tmp, store) = store().await;
        assert!(!store.offload_switch().await.unwrap().enabled, "缺省关");

        store.set_offload_switch(false).await.unwrap();
        assert!(!store.offload_switch().await.unwrap().enabled);
        assert!(store.offload_switch_has_override().await.unwrap());

        store.clear_offload_switch().await.unwrap();
        assert!(!store.offload_switch().await.unwrap().enabled);
        assert!(!store.offload_switch_has_override().await.unwrap());
    }

    #[tokio::test]
    async fn switch_round_trips() {
        let (_tmp, store) = store().await;
        store.set_offload_switch(true).await.unwrap();
        assert!(store.offload_switch().await.unwrap().enabled);
        store.set_offload_switch(false).await.unwrap();
        assert!(!store.offload_switch().await.unwrap().enabled);
        assert!(store.offload_switch_has_override().await.unwrap());
    }
}
