//! 界面上的绑定开关存储（决策 186）。
//!
//! 单行表 `kanban_server_bind`（迁移 0008）。与配对令牌（[`super::pairing`]）同构：
//! 「这台机器绑在哪里」是一个**不属于任何任务 / 项目的事实**，界面上的两颗钮改的就是它。
//!
//! 三个动作：读（启动时解析绑定地址）、写（界面开 / 关局域网访问）、清（回到
//! `[server] host` 的声明值）。清空是必要的——没有它，一个只改过一次界面的用户就再也
//! 回不到配置文件那条路上（他手改 `host` 会发现「改了没用」，而原因不在他改的那个地方）。

use super::{ts, Store};
use crate::Result;

impl Store {
    /// 界面设定的绑定地址；从未设置过 → `None`（此时回落 `[server] host`）。
    pub async fn server_bind_override(&self) -> Result<Option<String>> {
        let host: Option<String> =
            sqlx::query_scalar("SELECT host FROM kanban_server_bind WHERE id = 1")
                .fetch_optional(self.pool())
                .await?;
        Ok(host)
    }

    /// 写下界面设定的绑定地址（开 = `0.0.0.0`，关 = `127.0.0.1`）。
    ///
    /// UPSERT 而不是先删后插：单条语句在事务里完成替换，不存在「已删、未插」的中间态
    /// 被并发的启动解析读到一个空表（那会让「点了关」变成「回到配置文件的值」）。
    pub async fn set_server_bind_override(&self, host: &str) -> Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO kanban_server_bind (id, host, updated_at) VALUES (1, ?, ?)
             ON CONFLICT(id) DO UPDATE SET host = excluded.host, updated_at = excluded.updated_at",
        )
        .bind(host)
        .bind(ts(self.now()))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// 清掉界面设定，回到配置文件声明的绑定地址。返回是否真的清掉了一行。
    pub async fn clear_server_bind_override(&self) -> Result<bool> {
        let mut tx = self.begin_write().await?;
        let rows = sqlx::query("DELETE FROM kanban_server_bind WHERE id = 1")
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        Ok(rows > 0)
    }
}
