//! 配对令牌存储（决策 182㉖㉗㉘，票 07）。
//!
//! 单行表 `kanban_pairing_token`（迁移 0007）。读取与重置都挂在 [`Store`] 上：
//! 令牌是「这台机器的一个事实」，不属于任何任务 / 项目，与 `foreman` 一样是
//! 无归属的少数几项状态之一。
//!
//! 两个调用点的高频程度差得很远：`GET /pairing/token` 偶尔被点一次，而
//! `pairing_guard` 在局域网形态下**每个写请求**都要读一次。故读取路径给了纯读快路径
//! （见 [`Store::pairing_token`]），不让校验本身去抢写锁。

use super::{ts, Store};
use crate::Result;

/// 生成配对令牌。
///
/// 只用**已声明**的依赖（`crates/core/Cargo.toml`）：`ulid` 有，`getrandom` / `rand`
/// 没有，而本票不新增依赖。`ulid` 单独一枚不是秘密——48 bit 前缀是可推的时间戳，
/// 真正随机的是 80 bit，所以取**两个独立实例**拼接，随机位到 ~160 bit；结果 52 个
/// Crockford base32 字符，本身就是 URL 安全字符，可直接落在 `?pair={token}` 里而无需转义。
///
/// `sha2` 已在依赖里，但它不产生熵：对可猜的输入做摘要仍是可猜的，用它换不出更强的
/// 令牌，故不选。
fn generate_token() -> String {
    format!("{}{}", ulid::Ulid::new(), ulid::Ulid::new())
}

impl Store {
    /// 取配对令牌；库里没有就生成一个并持久化。
    ///
    /// **长期有效是硬约束**（票 07）：不随进程启动重生成。每次启动换令牌，使用者的
    /// 摩擦（每开一次服务就要在手机上重扫一次）会高到干脆不用这个机制，等于没有防护。
    /// 令牌的生命周期只由 [`Store::reset_pairing_token`] 决定。
    pub async fn pairing_token(&self) -> Result<String> {
        // 快路径：已存在时纯读返回。这个函数在配对中间件里每个写请求都会被走到，
        // 无条件开写事务会让「校验令牌」变成全局串行的写锁竞争。
        if let Some(token) = self.read_pairing_token().await? {
            return Ok(token);
        }

        // 慢路径：确认缺失才开写事务。首读与这里之间可能被并发请求插进来，
        // 故取到写锁后**再确认一次**——直接 INSERT 会撞 PK（id = 1 的 CHECK）。
        let mut tx = self.begin_write().await?;
        let existing: Option<String> =
            sqlx::query_scalar("SELECT token FROM kanban_pairing_token WHERE id = 1")
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(token) = existing {
            tx.commit().await?;
            return Ok(token);
        }
        let token = generate_token();
        sqlx::query("INSERT INTO kanban_pairing_token (id, token, created_at) VALUES (1, ?, ?)")
            .bind(&token)
            .bind(ts(self.now()))
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(token)
    }

    /// 重生成（一键重置配对）。旧令牌立即失效。返回新令牌。
    ///
    /// 有重置口是长期令牌能成立的前提：没有它，一次泄露就只剩「删库」这一条路
    /// （票 07 的「一键重置」验收项）。UPSERT 而不是先删后插——单条语句在事务里完成
    /// 替换，不存在「已删、未插」的中间态被并发校验读到一个空表。
    pub async fn reset_pairing_token(&self) -> Result<String> {
        let token = generate_token();
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO kanban_pairing_token (id, token, created_at) VALUES (1, ?, ?)
             ON CONFLICT(id) DO UPDATE SET token = excluded.token, created_at = excluded.created_at",
        )
        .bind(&token)
        .bind(ts(self.now()))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(token)
    }

    /// 只读读取：没有行 → `None`。**不生成**，供中间件快路径使用。
    async fn read_pairing_token(&self) -> Result<Option<String>> {
        let token: Option<String> =
            sqlx::query_scalar("SELECT token FROM kanban_pairing_token WHERE id = 1")
                .fetch_optional(self.pool())
                .await?;
        Ok(token)
    }
}
