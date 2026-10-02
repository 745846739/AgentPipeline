//! run 台账（决策 249 · 片②，票 02）：run 行的开立 / 收口 / 步标记与用量、续接记账收成一片。
//!
//! 搬移前散在调用点旁的**承重顺序**，在这里是不变量（locality：顺序从散文变成结构）：
//!
//! 1. **判超时先收口、再通知执行体**（scheduler `handle_timeout` 侧的顺序，红线仍在那一侧
//!    执行）：它的结构半边是 [`RunLedger::finish`] **不抢已有终态**——行已是
//!    Success / Failed / Timeout 时只补用量、不碰 status / error。`RunOutcome::default()`
//!    的 0 不得覆盖执行体手里已记过的真读数（决策 226）。
//! 2. **cancel 分支只补用量、不碰终态**（[`RunLedger::finish_cancelled`] 中「已有终态」那
//!    一支）：「超时」由判超时的调度器写着，执行体收口覆盖会把台账里的「超时」读成
//!    「失败」——而顺手放出去的那次重试正按「超时」记着账（决策 226）。**判超时那条路
//!    先写、执行体后写**，故同一道红线由「谁先写谁说了算」保证；反过来（没有人写过终态，
//!    例如人按停）执行体自己收成 `Cancelled`——台账里不留一条永远在跑的假记录（决策 276）。
//! 3. **续接链接只落 round 0**（[`RunLedger::link_continuation`]）：落进干净重试轮会让
//!    `metrics::total_tokens` 把同一段历史排除两次——token 少算不是双算，且读数随重试
//!    次数漂移（决策 180 / 票 13 必要条件二）。
//! 4. **取续接素材恰好一次、在重试环之前**（[`RunLedger::take_continuation`]）：读-清在
//!    同一事务里；进了环再取，第 2、3 次按构造永远是空起点。这张 resume 闸只回答
//!    「人的介入给不给续接」（决策 180 / 205）；`agent_retry_max` 的重试轮续接不走
//!    这里——决策 278 起它在编排侧直接保留上一轮转录（显式修订决策 205 裁决②）。
//!
//! **计时只有一个来源**：[`Clock`]（决策 143 接缝①）。`duration_ms` 由本 module 用调用方
//! 交进来的 `started` 与 `clock.now()` 算（[`since_ms`]）——假时钟推进即可确定性驱动，
//! 这是 Instant 读数做不到的（本片的核心杠杆）。
//!
//! 依赖分类：store 是 local-substitutable（临时 SQLite），clock 是 in-process 注入；
//! 无 git、无 LLM——不开 port，interface 就是测面。SSE 发射**不在本片**：观测面留守核
//! （照决策 245「门不发 SSE」的先例，事件跟编排走）。

use chrono::{DateTime, Utc};

use crate::agent::client::Message;
use crate::clock::Clock;
use crate::storage::observability::{NewRun, RunOutcome};
use crate::storage::Store;
use crate::types::{Node, NodeCursor, NodeStatus, Stage, Task};
use crate::Result;

use super::subagent::RunTokens;

/// 一次 pending → resume 边界的续接素材（决策 180，票 13）。
///
/// `from_run_id` 是**被续接的那条历史 run**，写进新 run 的 `continued_from_run_id`。
/// 这个链接如今只做**谱系**（排障时顺着链往上爬）；指标汇总已不再据它排除——
/// 排除规则随决策 375 删除（真实账语义：重喂的输入是真实成本）。
pub(crate) struct Continuation {
    pub messages: Vec<Message>,
    pub from_run_id: i64,
}

/// 自 `started` 起经过的毫秒（墙钟、经 [`Clock`] 接缝——决策 143；负值按 0 记）。
pub(crate) fn since_ms(now: DateTime<Utc>, started: DateTime<Utc>) -> u64 {
    (now - started).num_milliseconds().max(0) as u64
}

/// run 台账的唯一读写入口（决策 249 · 票 02）：借用 store 与 clock，不自持状态——
/// 谁记账谁建一个，方法即不变量（见模块 doc 的四条红线）。
pub(crate) struct RunLedger<'a> {
    store: &'a Store,
    clock: &'a dyn Clock,
}

impl<'a> RunLedger<'a> {
    pub(crate) fn new(store: &'a Store, clock: &'a dyn Clock) -> Self {
        Self { store, clock }
    }

    /// 下一次尝试号 = 该 (task, stage, node) 名下已有的 run 数 + 1。
    pub(crate) async fn next_attempt(
        &self,
        task_id: &str,
        stage: Stage,
        node: Node,
    ) -> Result<u32> {
        Ok(self
            .store
            .count_node_owning_runs(task_id, stage, node)
            .await?
            + 1)
    }

    /// 开立一条 run 行（attempt 取 [`Self::next_attempt`]）。
    ///
    /// **不含 SSE**：NodeStarted 由留守核在「发事件」的那层补（观测面跟编排走，决策 245
    /// 先例）；sync-check 那种不发事件的 run 直接经这里。
    pub(crate) async fn begin(
        &self,
        task: &Task,
        cursor_id: &str,
        stage: Stage,
        node: Node,
        agent_type: &str,
    ) -> Result<(i64, u32)> {
        let attempt = self.next_attempt(&task.id, stage, node).await?;
        let run_id = self
            .store
            .insert_run(&NewRun {
                task_id: task.id.clone(),
                cursor_id: cursor_id.to_string(),
                stage,
                node,
                attempt,
                agent_type: agent_type.into(),
                parent_run_id: None,
                prompt_template_hash: None,
                process_group_id: None,
            })
            .await?;
        Ok((run_id, attempt))
    }

    /// 收口一条 run：算 duration、写终态与用量，**返回写进去的 duration_ms**（SSE 复用
    /// 同一个读数——计时只算一次）。
    ///
    /// **不抢已有终态**（红线①的结构半边）：行已被判超时那侧收口成 Timeout / 已是
    /// Success / Failed 时，只把执行体手里的用量补进去（同一条 SQL），
    /// status / error / duration 一个字不碰。行不存在时与原「UPDATE 空转」同义：
    /// 什么都不写、照常返回读数（SSE 照发）。
    pub(crate) async fn finish(
        &self,
        run_id: i64,
        failed: bool,
        started: DateTime<Utc>,
        error: Option<String>,
        tokens: &RunTokens,
    ) -> Result<u64> {
        let duration_ms = since_ms(self.clock.now(), started);
        let Some(run) = self.store.get_run(run_id).await? else {
            return Ok(duration_ms);
        };
        if run.status != NodeStatus::Running {
            // 终态已定：补用量、不碰终态（红线① / 决策 226——0 不得覆盖真读数，反过来
            // 真读数也不该把已判的终态改写成另一种语义）。
            self.store.record_run_usage(run_id, tokens).await?;
            return Ok(duration_ms);
        }
        self.store
            .finish_run(
                run_id,
                &RunOutcome {
                    status: Some(if failed {
                        NodeStatus::Failed
                    } else {
                        NodeStatus::Success
                    }),
                    duration_ms,
                    error,
                    prompt_tokens: tokens.prompt,
                    completion_tokens: tokens.completion,
                    cache_read_tokens: tokens.cache_read,
                    cache_write_tokens: tokens.cache_write,
                    ..Default::default()
                },
            )
            .await?;
        Ok(duration_ms)
    }

    /// 系统节点的**步边界留痕**（决策 211④ / 票 04）。
    ///
    /// **best-effort**：写不进去只 warn，不 `?`。上一个同族的教训是 `Git::is_dirty`
    /// 那个只值一条警告的检查把任务挂死了四小时（决策 209）——留痕本身更不能挂住关键路径。
    /// 它补偿的是那次挂死的全部信息量：卡在哪个系统调用，事后必须能从台账里读出来。
    pub(crate) async fn mark_step(&self, run_id: i64, step: &str) {
        if let Err(e) = self.store.set_run_step(run_id, step).await {
            tracing::warn!(run_id, step, "步骤留痕写不进去（不阻塞节点）：{e}");
        }
    }

    /// **中止路径的收口**（决策 276）：判超时 / 人按停之后执行体自己收口时，顺手把这一条
    /// run 收成 `Cancelled`——**但只在还没有人判过它的时候**。
    ///
    /// 为什么需要它：红线②（决策 226）把终态判给了「判超时的那一方」，因为那边同时还要
    /// 决定「重试还是挂起」。可**人按停**那条路不判终态（见 `pipeline::pause`：人按停只管
    /// 按住，不重试也不挂起）——于是被判停的那条 run 会永远留在 `running` 上：台账里那句
    /// 「还在跑」是假的，而 `check_timeouts` 日后还会把它判一次超时，把人的处置覆盖掉。
    ///
    /// 与 [`Self::finish`] 的同一道红线：**行已是终态就一个字都不碰**（只补用量）。故两方
    /// 都可调用它——谁先写谁说了算（判超时那条路先写 Timeout，执行体这一笔退化成补用量）。
    pub(crate) async fn finish_cancelled(
        &self,
        run_id: i64,
        started: DateTime<Utc>,
        error: String,
        origin: super::executor::CancelOrigin,
        tokens: &RunTokens,
    ) -> Result<()> {
        let duration_ms = since_ms(self.clock.now(), started);
        let Some(run) = self.store.get_run(run_id).await? else {
            return Ok(());
        };
        if run.status != NodeStatus::Running {
            self.store.record_run_usage(run_id, tokens).await?;
            return Ok(());
        }
        self.store
            .finish_run(
                run_id,
                &RunOutcome {
                    status: Some(NodeStatus::Cancelled),
                    duration_ms,
                    error: Some(error),
                    // 票 02①：来路落一列（判据不按报文字样——决策 259）。超时梯子据此
                    // 决定这条中止行「跳过还是清零」：人按停是真介入（清零），判超时顺手
                    // 中止的那一轮是超时自己的副产品（跳过）。
                    cancel_origin: Some(origin.as_slug()),
                    prompt_tokens: tokens.prompt,
                    completion_tokens: tokens.completion,
                    cache_read_tokens: tokens.cache_read,
                    cache_write_tokens: tokens.cache_write,
                    ..Default::default()
                },
            )
            .await
    }

    /// 续接链接**只落 round 0**（红线③；决策 180 / 票 13 必要条件二）。`round` 由编排侧
    /// 交进来、规则在本侧判：`round != 0` 一律 no-op，干净重试轮（决策 33）不落链。
    pub(crate) async fn link_continuation(
        &self,
        run_id: i64,
        round: u32,
        continuation: Option<&Continuation>,
    ) -> Result<()> {
        if round != 0 {
            return Ok(());
        }
        if let Some(c) = continuation {
            self.store
                .link_run_continuation(run_id, c.from_run_id)
                .await?;
        }
        Ok(())
    }

    /// 取本节点的续接素材（红线④；决策 180 / 205，票 13 / 01）。
    ///
    /// **两道条件**（决策 205 把原来的三道砍掉一道：那个「谁来决定开不开」的配置层整层退场）：
    ///
    /// ① 游标**刚从 pending 被 resume**，且**原因表说该续接**（[`crate::types::resume_continues`]）。
    ///    取数是一次性的（取走即清零），且只有人能按出这个边界——`validate_attempts` 的
    ///    原地重试与未耗尽的超时都不会置位。**注意（决策 278）**：`agent_retry_max` 的
    ///    自动重试如今也续接，但那是在编排侧直接保留上一轮转录（显式修订决策 205 裁决②），
    ///    **不经本闸**——本闸仍然只管 resume 边界。
    /// ② 真有一条上一 attempt 的主 agent 会话行可读。
    ///
    /// 第 ② 条在「该续接却读不到」时**静默干净起跑**而不报错：这是票 13 必要条件一
    /// （`context_overflow` 退出路径补写会话行）修掉的那条路——修复之后它不该再发生，
    /// 但真发生时让节点继续跑仍优于让整条流水线停在一个诊断性错误上。
    pub(crate) async fn take_continuation(
        &self,
        cursor: &NodeCursor,
    ) -> Result<Option<Continuation>> {
        let Some(cause) = self
            .store
            .take_cursor_resume_cause(&cursor.cursor_id)
            .await?
        else {
            return Ok(None);
        };
        if !crate::types::resume_continues(cause) {
            return Ok(None);
        }
        let Some(conv) = self
            .store
            .latest_own_conversation(&cursor.task_id, cursor.stage, cursor.node)
            .await?
        else {
            return Ok(None);
        };
        let messages: Vec<Message> =
            serde_json::from_value(conv.messages_json.clone()).unwrap_or_default();
        if messages.is_empty() {
            return Ok(None);
        }
        Ok(Some(Continuation {
            messages,
            from_run_id: conv.run_id,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use crate::home::Home;
    use crate::storage::tasks::NewTask;
    use crate::types::{PendingKind, PendingReason, Project};

    /// 假时钟：本片的核心杠杆——duration 读数从此可以确定性驱动（决策 143 接缝①）。
    #[derive(Clone)]
    struct FakeClock(Arc<Mutex<DateTime<Utc>>>);

    impl FakeClock {
        fn fixed() -> Self {
            let t = DateTime::parse_from_rfc3339("2026-09-23T10:00:00+00:00")
                .unwrap()
                .with_timezone(&Utc);
            FakeClock(Arc::new(Mutex::new(t)))
        }

        fn advance(&self, ms: i64) {
            let mut g = self.0.lock().unwrap();
            *g += chrono::Duration::milliseconds(ms);
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> DateTime<Utc> {
            *self.0.lock().unwrap()
        }
    }

    /// 临时库 + 播种的项目/任务/初始游标 + 假时钟（store 与 ledger 共用同一个实例）。
    async fn base() -> (tempfile::TempDir, Store, Task, NodeCursor, FakeClock) {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = Home::new(tmp.path().join("home"));
        let clock = FakeClock::fixed();
        let store = Store::open(home, Arc::new(clock.clone())).await.unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let project = Project {
            id: "p1".into(),
            name: "proj".into(),
            local_path: repo.display().to_string(),
            default_branch: "main".into(),
            language: None,
            test_framework: None,
            lint_command: None,
            agents_md_path: None,
            created_at: Utc::now(),
        };
        store.create_project(&project).await.unwrap();
        let task = store
            .create_task(&NewTask::new("t1", "任务 t1", "p1"))
            .await
            .unwrap();
        let cursor = store.resolve_sole_cursor("t1").await.unwrap().unwrap();
        (tmp, store, task, cursor, clock)
    }

    #[tokio::test]
    async fn finish_duration_follows_the_clock() {
        // 今天 Instant 的读数无法确定性驱动——这就是 Clock 接缝落进台账的核心杠杆。
        let (_tmp, store, task, cursor, clock) = base().await;
        let ledger = RunLedger::new(&store, &clock);
        let (run_id, attempt) = ledger
            .begin(
                &task,
                &cursor.cursor_id,
                cursor.stage,
                cursor.node,
                "system",
            )
            .await
            .unwrap();
        assert_eq!(attempt, 1);
        let started = clock.now();
        clock.advance(1500);
        let duration = ledger
            .finish(run_id, false, started, None, &RunTokens::default())
            .await
            .unwrap();
        assert_eq!(duration, 1500, "duration_ms 只由假时钟推进量决定");
        let run = store.get_run(run_id).await.unwrap().unwrap();
        assert_eq!(run.duration_ms, 1500);
        assert_eq!(run.status, NodeStatus::Success);
    }

    #[tokio::test]
    async fn finish_keeps_a_terminal_status_someone_else_wrote() {
        // 红线①：判超时那侧先落了 Timeout，执行体随后正常收口也**不许**改写它——
        // 只把手里的用量补进去（status / error / duration 都不碰）。
        let (_tmp, store, task, cursor, clock) = base().await;
        let ledger = RunLedger::new(&store, &clock);
        let (run_id, _) = ledger
            .begin(&task, &cursor.cursor_id, cursor.stage, cursor.node, "main")
            .await
            .unwrap();
        store
            .finish_run(
                run_id,
                &RunOutcome {
                    status: Some(NodeStatus::Timeout),
                    duration_ms: 999,
                    error: Some("节点超时".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let tokens = RunTokens {
            prompt: 7,
            completion: 9,
            ..Default::default()
        };
        ledger
            .finish(run_id, true, clock.now(), Some("重试耗尽".into()), &tokens)
            .await
            .unwrap();
        let run = store.get_run(run_id).await.unwrap().unwrap();
        assert_eq!(run.status, NodeStatus::Timeout, "终态不被抢");
        assert_eq!(run.error.as_deref(), Some("节点超时"), "error 不被抢");
        assert_eq!(run.duration_ms, 999, "duration 不被抢");
        assert_eq!(run.prompt_tokens, 7, "但用量照补（决策 226 真读数）");
        assert_eq!(run.completion_tokens, 9);
    }

    #[tokio::test]
    async fn cancelled_finish_writes_the_status_only_when_nobody_else_did() {
        // 决策 276：中止路径的收口有**两个来路**，这一条把两者的边界一次钉住——
        // ① 行仍是 running（人按停那条路不判终态）→ 自己收成 Cancelled（否则它永远
        //    留在「还在跑」上，日后还会被 check_timeouts 判一次超时把人的处置覆盖掉）；
        // ② 行已被判终态（判超时那侧先写了 Timeout）→ 一个字都不碰，只补用量（红线①）。
        let (_tmp, store, task, cursor, clock) = base().await;
        let ledger = RunLedger::new(&store, &clock);
        let (run_id, _) = ledger
            .begin(&task, &cursor.cursor_id, cursor.stage, cursor.node, "main")
            .await
            .unwrap();

        // ① 没人判过：中止路径自己收成 Cancelled
        let tokens = RunTokens {
            prompt: 11,
            completion: 22,
            ..Default::default()
        };
        ledger
            .finish_cancelled(
                run_id,
                clock.now(),
                "已按人工暂停 / 重跑中止".into(),
                crate::pipeline::executor::CancelOrigin::Hold,
                &tokens,
            )
            .await
            .unwrap();
        let run = store.get_run(run_id).await.unwrap().unwrap();
        assert_eq!(run.status, NodeStatus::Cancelled);
        assert_eq!(
            run.error.as_deref(),
            Some("已按人工暂停 / 重跑中止"),
            "台账里要读得出是谁把它按停的"
        );
        assert_eq!(run.prompt_tokens, 11, "中止那一轮烧掉的量照记");

        // ② 已有终态（判超时那侧先写）：只补用量，status / error / duration 不动
        let (run2, _) = ledger
            .begin(&task, &cursor.cursor_id, cursor.stage, cursor.node, "main")
            .await
            .unwrap();
        store
            .finish_run(
                run2,
                &RunOutcome {
                    status: Some(NodeStatus::Timeout),
                    duration_ms: 999,
                    error: Some("节点超时".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        ledger
            .finish_cancelled(
                run2,
                clock.now(),
                "不该写进去".into(),
                crate::pipeline::executor::CancelOrigin::Timeout,
                &tokens,
            )
            .await
            .unwrap();
        let run = store.get_run(run2).await.unwrap().unwrap();
        assert_eq!(run.status, NodeStatus::Timeout, "终态不被抢");
        assert_eq!(run.error.as_deref(), Some("节点超时"), "error 不被抢");
        assert_eq!(run.duration_ms, 999, "duration 不被抢");
        assert_eq!(run.prompt_tokens, 11, "但用量照补（决策 226 真读数）");
        assert_eq!(run.completion_tokens, 22);
    }

    #[tokio::test]
    async fn continuation_links_only_the_first_round() {
        // 红线③：round != 0 一律 no-op——链接落进干净重试轮，metrics 会把同一段历史
        // 排除两次（token 少算且随重试次数漂移）。
        let (_tmp, store, task, cursor, clock) = base().await;
        let ledger = RunLedger::new(&store, &clock);
        let begin = |agent_type: &'static str| {
            let ledger = &ledger;
            let task = &task;
            let cursor = &cursor;
            async move {
                ledger
                    .begin(
                        task,
                        &cursor.cursor_id,
                        cursor.stage,
                        cursor.node,
                        agent_type,
                    )
                    .await
                    .unwrap()
            }
        };
        let (history, _) = begin("main").await;
        let (fresh, _) = begin("main").await;
        let (retried, _) = begin("main").await;
        let continuation = Continuation {
            messages: vec![Message::user("上一轮")],
            from_run_id: history,
        };

        ledger
            .link_continuation(fresh, 0, Some(&continuation))
            .await
            .unwrap();
        let fresh_run = store.get_run(fresh).await.unwrap().unwrap();
        assert_eq!(
            fresh_run.continued_from_run_id,
            Some(history),
            "round 0 落链"
        );

        ledger
            .link_continuation(retried, 1, Some(&continuation))
            .await
            .unwrap();
        let retried_run = store.get_run(retried).await.unwrap().unwrap();
        assert_eq!(
            retried_run.continued_from_run_id, None,
            "round 1 不落链（红线③）"
        );

        let (clean, _) = begin("main").await;
        ledger.link_continuation(clean, 0, None).await.unwrap();
        let clean_run = store.get_run(clean).await.unwrap().unwrap();
        assert_eq!(
            clean_run.continued_from_run_id, None,
            "没有续接素材的 round 0 也不落链"
        );
    }

    #[tokio::test]
    async fn take_continuation_consumes_the_resume_cause_once() {
        // 红线④：读-清恰好一次；第二次进来拿不到素材（干净起跑），不会重复续接。
        let (_tmp, store, task, cursor, clock) = base().await;
        let ledger = RunLedger::new(&store, &clock);
        let (run_id, attempt) = ledger
            .begin(&task, &cursor.cursor_id, cursor.stage, cursor.node, "main")
            .await
            .unwrap();
        let messages = vec![
            Message::user("上一轮的提问"),
            Message::assistant(Some("上一轮的回答".into()), vec![]),
        ];
        store
            .insert_conversation(
                &task.id,
                run_id,
                cursor.stage,
                cursor.node,
                attempt,
                "main",
                None,
                &serde_json::to_value(&messages).unwrap(),
                None,
                None,
                0,
                0,
                None,
            )
            .await
            .unwrap();
        // 走真落点链路：置 pending → resume 清 pending（记原因列）→ take 读清。
        store
            .set_cursor_pending(
                &cursor.cursor_id,
                &PendingReason::new(
                    PendingKind::InfoInsufficient,
                    cursor.stage,
                    cursor.node,
                    "信息不足",
                ),
            )
            .await
            .unwrap();
        store.clear_cursor_pending(&cursor.cursor_id).await.unwrap();

        let first = ledger.take_continuation(&cursor).await.unwrap();
        let first = first.expect("resume 原因说「续接」且会话行可读 → 给素材");
        assert_eq!(first.from_run_id, run_id);
        assert_eq!(first.messages.len(), 2);

        let second = ledger.take_continuation(&cursor).await.unwrap();
        assert!(second.is_none(), "原因读走即清：第二次必须干净起跑");
    }
}
