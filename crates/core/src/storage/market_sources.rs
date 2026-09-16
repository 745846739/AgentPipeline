//! 界面上的技能市场来源存储（决策 187）。
//!
//! 单行表 `kanban_market_sources`（迁移 0009）。与 [`super::server_bind`] 同构：
//! 「允许从哪些 registry 装技能」是界面能改的一件机器级事实，不是任何任务 / 项目的属性。
//!
//! 读取路径不设缓存：索引请求与安装才用它，频度是「人点一下」，不值得为它引入
//! 「缓存与库漂移」这一种新的不一致（对比配对令牌：那里每个写请求都要读，故有纯读快路径）。

use super::{ts, Store};
use crate::Result;

impl Store {
    /// 界面设定的来源白名单；从未保存过 → `None`（此时回落 `config.toml` 的 `[market]`）。
    ///
    /// **空数组与 `None` 是两回事**：`Some(vec![])` = 用户显式清空了 = 不允许远程安装
    /// （界面上的「保存」就是一次显式动作）；`None` = 没保存过，读配置。把两者混为一谈
    /// 会让「清空来源」变成「回到配置文件那一级」——那是一个用户没有要求过的行为。
    pub async fn market_sources_override(&self) -> Result<Option<Vec<String>>> {
        let raw: Option<String> =
            sqlx::query_scalar("SELECT sources_json FROM kanban_market_sources WHERE id = 1")
                .fetch_optional(self.pool())
                .await?;
        match raw {
            None => Ok(None),
            Some(json) => Ok(Some(serde_json::from_str(&json).map_err(|e| {
                crate::Error::Config(format!("界面保存的市场来源无法解析（{e}）：{json}"))
            })?)),
        }
    }

    /// 写下界面设定的来源白名单（归一后的值，由调用方先过 `validate_market_sources`）。
    pub async fn set_market_sources_override(&self, sources: &[String]) -> Result<()> {
        let encoded = serde_json::to_string(sources)?;
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO kanban_market_sources (id, sources_json, updated_at) VALUES (1, ?, ?)
             ON CONFLICT(id) DO UPDATE SET sources_json = excluded.sources_json, \
                                          updated_at = excluded.updated_at",
        )
        .bind(&encoded)
        .bind(ts(self.now()))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// 清掉界面设定，回到 `config.toml` 的 `[market] allowed_sources`。返回是否真的清掉一行。
    pub async fn clear_market_sources_override(&self) -> Result<bool> {
        let mut tx = self.begin_write().await?;
        let rows = sqlx::query("DELETE FROM kanban_market_sources WHERE id = 1")
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        Ok(rows > 0)
    }
}
