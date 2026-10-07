//! 重活外发 GitHub 的全局开关（票 runner-offload/05）+ 白名单模式（决策 398）。
//!
//! 单行表 `kanban_offload`（迁移 0040，白名单两列在 0043），与 [`super::rtk`]
//! （决策 297）同构：**机器级事实** + 三动作形状（读 / 写 / 清）。旋钮只有两个——
//! 主开关 `enabled` 与白名单模式（`whitelist_enabled` + `whitelist_pattern`）：
//! 外发的对象、回退语义、结果形态都在工具层钉死，不进配置：那些是行为不是偏好，
//! 配置面上多一个旋钮就多一处「设置与行为漂移」的机会（白名单正则是唯一的例外——
//! 它本来就是用户口味的路由规则，决策 398）。
//!
//! 行不存在 = 从没碰过设置 = **关 = 全部本机运行**。同一行不做 `config.toml`
//! 两级结构：理由与 rtk 一致（决策 297 的注释原话），再造一份就是第二处真相。

use super::{ts, Store};
use crate::Result;

/// 开关那一行的读数。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OffloadSwitch {
    /// 全局开关：关 = 全部本机运行（**逐字等于这个功能出现之前**）。
    pub enabled: bool,
    /// 白名单模式（决策 398）：开着时，`run_command` 命中正则的命令自动改道外发。
    /// 只在外发开关开着时才生效（白名单模式是外发功能的一层，不是独立通道）。
    pub whitelist_enabled: bool,
    /// 白名单正则原文。`None` = 从没填过；命中判定在执行层现编译（保存时已验过）。
    pub whitelist_pattern: Option<String>,
}

impl Store {
    /// 读开关。**行不存在 = 缺省关**（不是错误）。
    pub async fn offload_switch(&self) -> Result<OffloadSwitch> {
        let row: Option<(bool, bool, Option<String>)> = sqlx::query_as(
            "SELECT enabled, whitelist_enabled, whitelist_pattern \
             FROM kanban_offload WHERE id = 1",
        )
        .fetch_optional(self.pool())
        .await?;
        Ok(match row {
            None => OffloadSwitch::default(),
            Some((enabled, whitelist_enabled, whitelist_pattern)) => OffloadSwitch {
                enabled,
                whitelist_enabled,
                whitelist_pattern,
            },
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

    /// 写白名单模式（决策 398）。与主开关同一个 upsert 形状：**探测失败也照写**、
    /// 保存即建行（`offload_switch_has_override` 随之变 true——「界面保存过」的口径）。
    /// 正则的合法性校验在设置 API 一层做（fail fast），本层只存原文。
    pub async fn set_offload_whitelist(&self, enabled: bool, pattern: Option<&str>) -> Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO kanban_offload (id, enabled, whitelist_enabled, whitelist_pattern, updated_at)
             VALUES (1, 0, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET whitelist_enabled = excluded.whitelist_enabled,
                                           whitelist_pattern = excluded.whitelist_pattern,
                                           updated_at = excluded.updated_at",
        )
        .bind(enabled)
        .bind(pattern)
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

    /// 最近一次**链路**失败的时间戳（票 08）。`None` = 从没失败过（读数是
    /// 「无」，不是「0」——诚实口径，决策 257）。
    pub async fn offload_last_failure(&self) -> Result<Option<String>> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT last_failure_at FROM kanban_offload WHERE id = 1")
                .fetch_optional(self.pool())
                .await?;
        Ok(row.and_then(|(t,)| t))
    }

    /// 落一笔链路失败（外发回退本机的 WARN 点顺带调用，票 08）。
    ///
    /// **UPDATE-only**：行不存在就什么都不写。能跑到外发这一步的前提是开关开着
    /// （开关打开即建行），行不在 = 有人刚清了设置，那种竞态不值得为它造行——
    /// 造行会把「界面保存过」的口径（[`Self::offload_switch_has_override`]）搅浑。
    pub async fn record_offload_link_failure(&self) -> Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("UPDATE kanban_offload SET last_failure_at = ?, updated_at = ? WHERE id = 1")
            .bind(ts(self.now()))
            .bind(ts(self.now()))
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// 清链路失败读数：外发**成功**跑完一轮即清（链路刚被证明是通的，包括远端
    /// 命令跑红那一路——push/dispatch/轮询/拉日志都走通了，票 08 验收第三条）。
    pub async fn clear_offload_link_failure(&self) -> Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("UPDATE kanban_offload SET last_failure_at = NULL WHERE id = 1")
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

    /// 白名单模式（决策 398）的读写回环：缺省关 / 无正则；保存落值；
    /// 动白名单不碰主开关；保存即建行（「界面保存过」口径）。
    #[tokio::test]
    async fn whitelist_round_trips_without_touching_the_main_switch() {
        let (_tmp, store) = store().await;
        let sw = store.offload_switch().await.unwrap();
        assert!(!sw.whitelist_enabled, "缺省关");
        assert!(sw.whitelist_pattern.is_none(), "缺省无正则");

        store
            .set_offload_whitelist(true, Some(r"^cargo (test|clippy)"))
            .await
            .unwrap();
        let sw = store.offload_switch().await.unwrap();
        assert!(sw.whitelist_enabled);
        assert_eq!(
            sw.whitelist_pattern.as_deref(),
            Some(r"^cargo (test|clippy)")
        );
        assert!(!sw.enabled, "动白名单不改主开关");
        assert!(store.offload_switch_has_override().await.unwrap());

        // 主开关随后独立保存：白名单的值不被冲掉。
        store.set_offload_switch(true).await.unwrap();
        let sw = store.offload_switch().await.unwrap();
        assert!(sw.enabled);
        assert!(sw.whitelist_enabled);
        assert_eq!(
            sw.whitelist_pattern.as_deref(),
            Some(r"^cargo (test|clippy)")
        );

        // 关掉并清空正则：回缺省态，但「保存过」仍在。
        store.set_offload_whitelist(false, None).await.unwrap();
        let sw = store.offload_switch().await.unwrap();
        assert!(!sw.whitelist_enabled);
        assert!(sw.whitelist_pattern.is_none());
        assert!(sw.enabled, "清白名单不动主开关");
    }

    /// 链路失败读数（票 08）：从没失败过是「无」（None）；落一笔带时间戳；
    /// 成功一轮清回 None。
    #[tokio::test]
    async fn link_failure_readout_round_trips() {
        let (_tmp, store) = store().await;
        assert!(
            store.offload_last_failure().await.unwrap().is_none(),
            "从没失败过的机器读「无」"
        );

        store.set_offload_switch(true).await.unwrap();
        store.record_offload_link_failure().await.unwrap();
        let stamped = store.offload_last_failure().await.unwrap();
        assert!(stamped.is_some(), "链路失败要带出时间戳");
        crate::storage::parse_ts(stamped.as_deref().unwrap()).unwrap();

        store.clear_offload_link_failure().await.unwrap();
        assert!(store.offload_last_failure().await.unwrap().is_none());
        assert!(
            store.offload_switch().await.unwrap().enabled,
            "清读数不动开关"
        );
    }

    /// 行不在时落失败是**无操作**：不偷偷造行（造行会搅浑「界面保存过」的口径）。
    #[tokio::test]
    async fn link_failure_record_on_absent_row_is_a_no_op() {
        let (_tmp, store) = store().await;
        store.record_offload_link_failure().await.unwrap();
        assert!(store.offload_last_failure().await.unwrap().is_none());
        assert!(
            !store.offload_switch_has_override().await.unwrap(),
            "不该替人建行"
        );
    }
}
