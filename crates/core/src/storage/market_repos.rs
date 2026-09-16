//! 界面上的技能来源**仓名单**存储（决策 194，继承决策 187 的两级结构）。
//!
//! 单行表 `kanban_market_repos`（迁移 0010）。与 [`super::server_bind`] 同构：
//! 「允许从哪些仓装技能」是界面能改的一件机器级事实，不是任何任务 / 项目的属性。
//!
//! 读取路径不设缓存：列技能与安装才用它，频度是「人点一下」，不值得为它引入
//! 「缓存与库漂移」这一种新的不一致。

use super::{ts, Store};
use crate::Result;

impl Store {
    /// 界面设定的仓名单；从未保存过 → `None`（此时回落 `config.toml` 的 `[market] github_repos`）。
    ///
    /// **空数组与 `None` 是两回事**：`Some(vec![])` = 用户显式清空了 = 不从任何仓安装
    /// （界面上的「保存」就是一次显式动作）；`None` = 没保存过，读配置。把两者混为一谈
    /// 会让「清空仓名单」变成「回到配置文件那一级」——那是一个用户没有要求过的行为。
    /// 这条区分是迁移 0009 的注释里已经写明的，不在新表上丢掉。
    pub async fn market_repos_override(&self) -> Result<Option<Vec<String>>> {
        let raw: Option<String> =
            sqlx::query_scalar("SELECT repos_json FROM kanban_market_repos WHERE id = 1")
                .fetch_optional(self.pool())
                .await?;
        match raw {
            None => Ok(None),
            Some(json) => Ok(Some(serde_json::from_str(&json).map_err(|e| {
                crate::Error::Config(format!("界面保存的仓名单无法解析（{e}）：{json}"))
            })?)),
        }
    }

    /// 写下界面设定的仓名单（归一后的值，由调用方先过 `validate_market_repos`）。
    pub async fn set_market_repos_override(&self, repos: &[String]) -> Result<()> {
        let encoded = serde_json::to_string(repos)?;
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO kanban_market_repos (id, repos_json, updated_at) VALUES (1, ?, ?)
             ON CONFLICT(id) DO UPDATE SET repos_json = excluded.repos_json, \
                                          updated_at = excluded.updated_at",
        )
        .bind(&encoded)
        .bind(ts(self.now()))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// 清掉界面设定，回到 `config.toml` 的 `[market] github_repos`。返回是否真的清掉一行。
    pub async fn clear_market_repos_override(&self) -> Result<bool> {
        let mut tx = self.begin_write().await?;
        let rows = sqlx::query("DELETE FROM kanban_market_repos WHERE id = 1")
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        Ok(rows > 0)
    }
}
