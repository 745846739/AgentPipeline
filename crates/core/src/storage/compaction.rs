//! 「管线压缩」设置卡的 DB 覆盖层（long-run-budget 票 02）。
//!
//! 单行表 `kanban_compaction`（迁移 0042），与 [`super::server_bind`]（决策 186）/
//! [`super::offload`]（runner-offload 票 05）同构：应用从不写 config.toml
//! （settings-honesty 定下的边界），界面可改的全局设置走 DB 覆盖。
//!
//! 读数语义：**列 NULL = 没保存过 = 回落 config 值**（最终回落缺省 300_000 / 5）；
//! 非 NULL = 界面覆盖。两列独立可空——「保存了 A 不偷改 B 的出处」由列级 NULL 保证。
//! 消费点懒读（foreman_watch 先例）：流水线侧每 attempt 在 [`crate::pipeline::model_invoke`]
//! 的 `effective_settings` 叠一次，值班长侧在 TurnFacts 组装处同源叠加（决策 291）。

use super::{ts, Store};
use crate::Result;

/// 两个压缩旋钮的覆盖读数：`None` = 没保存过 = 读 config 值（诚实口径，决策 257）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CompactionOverrides {
    /// L3 压缩的 token 硬底（缺省 300_000）。
    pub conversation_max_tokens: Option<usize>,
    /// 压缩保留的最近轮数（缺省 5）。
    pub keep_recent_rounds: Option<usize>,
}

impl Store {
    /// 读覆盖。**行不存在 = 两个都是 `None`**（不是错误）。
    pub async fn compaction_overrides(&self) -> Result<CompactionOverrides> {
        let row: Option<(Option<i64>, Option<i64>)> = sqlx::query_as(
            "SELECT conversation_max_tokens, keep_recent_rounds FROM kanban_compaction WHERE id = 1",
        )
        .fetch_optional(self.pool())
        .await?;
        Ok(match row {
            None => CompactionOverrides::default(),
            Some((tokens, rounds)) => CompactionOverrides {
                conversation_max_tokens: tokens.map(|v| v as usize),
                keep_recent_rounds: rounds.map(|v| v as usize),
            },
        })
    }

    /// 写覆盖。PUT 一次保存两个旋钮（界面只有一张卡），两列同批落库。
    pub async fn set_compaction_overrides(
        &self,
        conversation_max_tokens: usize,
        keep_recent_rounds: usize,
    ) -> Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO kanban_compaction (id, conversation_max_tokens, keep_recent_rounds, updated_at)
             VALUES (1, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET conversation_max_tokens = excluded.conversation_max_tokens,
                                           keep_recent_rounds = excluded.keep_recent_rounds,
                                           updated_at = excluded.updated_at",
        )
        .bind(conversation_max_tokens as i64)
        .bind(keep_recent_rounds as i64)
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

    /// long-run-budget 票 02：行不存在 = 两个都是 None（读 config 值）；
    /// 保存后两格都读得回来；CHECK(id=1) 让第二行进不来。
    #[tokio::test]
    async fn compaction_overrides_default_to_none_and_persist() {
        let store = Store::open_in_memory(std::sync::Arc::new(crate::clock::SystemClock))
            .await
            .unwrap();
        let none = store.compaction_overrides().await.unwrap();
        assert_eq!(none.conversation_max_tokens, None, "没保存过 = None");
        assert_eq!(none.keep_recent_rounds, None, "没保存过 = None");

        store.set_compaction_overrides(400_000, 8).await.unwrap();
        let read = store.compaction_overrides().await.unwrap();
        assert_eq!(read.conversation_max_tokens, Some(400_000));
        assert_eq!(read.keep_recent_rounds, Some(8));

        // 重写：同一直行 UPSERT，不是追加。
        store.set_compaction_overrides(300_000, 5).await.unwrap();
        let read = store.compaction_overrides().await.unwrap();
        assert_eq!(read.conversation_max_tokens, Some(300_000));
        assert_eq!(read.keep_recent_rounds, Some(5));
    }
}
