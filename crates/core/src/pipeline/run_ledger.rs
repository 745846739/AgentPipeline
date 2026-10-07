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

/// 续接的**形态**（决策 376 裁决② · 票 04）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContinuationMode {
    /// 全卷转录：带的上一轮对话原样续接（决策 180 / 205 的既有形态，梯子第 1–2 档与人按 resume）。
    Transcript,
    /// 简报：**不带转录**，改由 `continuation_brief` 段交代「任务描述 + 产物清单 +
    /// 未提交改动 + 最近收口摘要」（梯子第 3 档的空白重跑，决策 320 / 376）。
    Brief,
}

/// 一次 pending → resume 边界的续接素材（决策 180，票 13）。
///
/// `from_run_id` 是**被续接的那条历史 run**，写进新 run 的 `continued_from_run_id`。
/// 这个链接如今只做**谱系**（排障时顺着链往上爬）；指标汇总已不再据它排除——
/// 排除规则随决策 375 删除（真实账语义：重喂的输入是真实成本）。
///
/// **简报形态不带链接**（`from_run_id = None`，票 04）：空白重跑在台账语义上仍是
/// 「新的一段对话」，它只是拿到了上一轮现场的**文本投影**，不是续接那段对话。
pub(crate) struct Continuation {
    /// 这一次续接的原因（票 02）。**用途有两个**：编排侧据此判「这是不是一次进程重启」
    /// （票 04 的止损）与两条 turn 注入的判据（补充输入 / 评审打回反馈）——不再按「承接的
    /// 转录非空」猜：日志源接通之后**任何**一次恢复都会让承接转录非空，那个判据会把普通
    /// 崩溃误读成「人按了补充输入 / 打回了评审」。**形态不由它推**，仍看 [`Self::mode`]。
    pub cause: crate::types::ResumeCause,
    pub mode: ContinuationMode,
    pub messages: Vec<Message>,
    pub from_run_id: Option<i64>,
    /// 承接转录**末尾**有几条是**合成回执**（票 02）：日志断在半轮时补齐的那几条。
    ///
    /// 调用方要把这个**尾部计数**换算成下标再交给写日志那侧（见 `model_invoke` 的
    /// `synthetic_from`）：它在写前缀之前还会往末尾追加补充输入 / 打回反馈的 turn，
    /// 那些是**真消息**——按尾部计数会把标记打错人。
    pub synthetic_tail: usize,
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
            if let Some(from_run_id) = c.from_run_id {
                self.store
                    .link_run_continuation(run_id, from_run_id)
                    .await?;
            }
        }
        Ok(())
    }

    /// 取本节点的续接素材（红线④；决策 180 / 205，票 13 / 01）。
    ///
    /// **闸只有一道**（决策 205 把原来的三道砍掉一道：那个「谁来决定开不开」的配置层整层退场）：
    /// 游标**刚从 pending 被 resume**（或由自动路径置了原因），且**原因表说该续接**
    /// （[`crate::types::resume_continues`]）。取数是一次性的（取走即清零）。
    /// **注意（决策 278）**：`agent_retry_max` 的自动重试如今也续接，但那是在编排侧直接
    /// 保留上一轮转录（显式修订决策 205 裁决②），**不经本闸**——本闸仍然只管 resume 边界。
    ///
    /// **转录那份源是日志不是会话表**（`.scratch/node-message-resume` 票 02）：会话表
    /// 那条路是循环退出之后才写的（崩溃时一个字都没有），且写入前经 `truncate_messages_json`
    /// 从最旧一端整条丢消息——切口落在轮中间时留下的孤儿 tool 消息会被上游净化**静默吃掉
    /// 工具输出**。日志只追加、压缩不碰它，于是续接拿到的是完整转录。查找键与
    /// [`Store::latest_own_conversation`] 同源（`(task, stage, node, agent_type='main')`
    /// 取 id 最大的 run）——**不能按游标找**：`goto` 在同一条游标行上改 `(stage, node)`。
    ///
    /// **「读不到转录」不是第二道闸，只是转录为空**：这种时候**照样交出一份带原因的素材**
    /// （`messages` 为空）——这是票 13 必要条件一（`context_overflow` 退出路径补写会话行）
    /// 修掉的那条路的延续：升级前建的库、或这一轮一条消息都没写完就被杀，都走这一支
    /// （干净起跑，与加日志之前一致），而**原因不能跟着一起丢**——编排侧靠它区分「恢复」与
    /// 「人按了补充输入」，重启连击的止损（票 04）也靠它。只有「没有原因」与「原因说别续接」
    /// 两种情况才给 `None`。
    ///
    /// **形态由原因定**（票 04）：`timeout_blank_restart` 返回
    /// [`ContinuationMode::Brief`]——不给转录、不落链，只把「这是空白重跑」这个事实
    /// 交给编排侧去渲染简报段；其余原因照旧给全卷转录。简报**不要求日志可读**：
    /// 它本就是为了「上一轮死得连转录都不该再喂一遍」而存在的。
    ///
    /// **半轮补齐**（票 02）：日志停在「assistant 已落、这一批工具的结果没配齐」时，
    /// 由 [`crate::agent::context::complete_incomplete_round`] 补上合成回执——不补的话
    /// 这半轮重放给下一轮必然是一次 400（assistant 声明的调用没有回执）。
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
        if cause == crate::types::ResumeCause::TimeoutBlankRestart {
            return Ok(Some(Continuation {
                cause,
                mode: ContinuationMode::Brief,
                messages: Vec::new(),
                from_run_id: None,
                synthetic_tail: 0,
            }));
        }
        let Some(transcript) = self
            .store
            .latest_own_transcript(&cursor.task_id, cursor.stage, cursor.node)
            .await?
        else {
            // 读不到日志（升级前建的库 / 这一轮一条消息都没写完就被杀）→ 干净起跑。
            // **仍然交出原因**：编排侧要拿它区分「这是一次恢复」与「人按了补充输入」——
            // 返回 `None` 会把这件事一起丢掉，而重启连击的止损（票 04）正靠它。
            return Ok(Some(Continuation {
                cause,
                mode: ContinuationMode::Transcript,
                messages: Vec::new(),
                from_run_id: None,
                synthetic_tail: 0,
            }));
        };
        let mut messages = transcript.messages;
        let synthetic_tail = crate::agent::context::complete_incomplete_round(&mut messages);
        // 空转录不落链（台账里那是一条指向「没有可续内容」的假谱系），与读不到日志同一处置。
        let from_run_id = (!messages.is_empty()).then_some(transcript.run_id);
        Ok(Some(Continuation {
            cause,
            mode: ContinuationMode::Transcript,
            messages,
            from_run_id,
            synthetic_tail,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use crate::home::Home;
    use crate::storage::tasks::NewTask;
    use crate::types::{PendingKind, PendingReason, Project, ResumeCause};

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
            cause: ResumeCause::InfoInsufficient,
            mode: ContinuationMode::Transcript,
            messages: vec![Message::user("上一轮")],
            from_run_id: Some(history),
            synthetic_tail: 0,
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
        let (run_id, _attempt) = ledger
            .begin(&task, &cursor.cursor_id, cursor.stage, cursor.node, "main")
            .await
            .unwrap();
        let messages = vec![
            Message::user("上一轮的提问"),
            Message::assistant(Some("上一轮的回答".into()), vec![]),
        ];
        // 源是**节点内消息日志**（票 02），不再是会话表：一条 run 的消息行合起来就是转录。
        store
            .append_node_messages(
                &task.id,
                run_id,
                cursor.stage,
                cursor.node,
                crate::storage::AGENT_TYPE_MAIN,
                0,
                &messages,
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
        assert_eq!(first.mode, ContinuationMode::Transcript);
        assert_eq!(first.from_run_id, Some(run_id));
        assert_eq!(first.messages.len(), 2);

        let second = ledger.take_continuation(&cursor).await.unwrap();
        assert!(second.is_none(), "原因读走即清：第二次必须干净起跑");
    }

    /// 票 04：空白重跑那一档给的是**简报形态**——不带转录、不落谱系链接，且**不要求**
    /// 会话行可读（它本就是为了「上一轮死得连转录都不该再喂一遍」而存在）。
    #[tokio::test]
    async fn a_blank_restart_continuation_is_brief_and_unlinked() {
        let (_tmp, store, task, cursor, clock) = base().await;
        let ledger = RunLedger::new(&store, &clock);
        let (run_id, _) = ledger
            .begin(&task, &cursor.cursor_id, cursor.stage, cursor.node, "main")
            .await
            .unwrap();
        // 置位口与生产同路：超时梯子第 3 档直接写原因列（游标从未 pending 过）。
        store
            .mark_cursor_continuation(&cursor.cursor_id, ResumeCause::TimeoutBlankRestart)
            .await
            .unwrap();

        let got = ledger
            .take_continuation(&cursor)
            .await
            .unwrap()
            .expect("空白重跑也是一条续接边界，要给（空的）素材");
        assert_eq!(got.mode, ContinuationMode::Brief);
        assert!(got.messages.is_empty(), "简报不带全卷转录");
        assert_eq!(got.from_run_id, None, "空白重跑不落续接链接");

        // 读-清语义不变：第二次干净起跑。
        assert!(ledger.take_continuation(&cursor).await.unwrap().is_none());

        // 不落链：round 0 拿到这份素材也不写 continued_from_run_id。
        ledger
            .link_continuation(run_id, 0, Some(&got))
            .await
            .unwrap();
        let row = store.get_run(run_id).await.unwrap().unwrap();
        assert_eq!(row.continued_from_run_id, None);
    }

    /// 票 02：续接的源是**节点内消息日志**，且日志断在半轮时补齐合成回执——
    /// 交出去的必须是一份**合法**转录（每个声明的调用都有回执）。
    #[tokio::test]
    async fn take_continuation_patches_a_half_round_from_the_log() {
        let (_tmp, store, task, cursor, clock) = base().await;
        let ledger = RunLedger::new(&store, &clock);
        let (run_id, _) = ledger
            .begin(&task, &cursor.cursor_id, cursor.stage, cursor.node, "main")
            .await
            .unwrap();
        let calls = vec![
            crate::agent::client::ToolCall {
                id: "c1".into(),
                name: "read_file".into(),
                arguments: r#"{"path":"a"}"#.into(),
            },
            crate::agent::client::ToolCall {
                id: "c2".into(),
                name: "list_dir".into(),
                arguments: r#"{"path":"."}"#.into(),
            },
        ];
        // 半轮：assistant 已落、第一条工具结果已落、第二条没了——进程就是在这里被杀的。
        let half = vec![
            Message::user("干活"),
            Message::assistant(None, calls.clone()),
            Message::tool_result(&calls[0], "第一条的真回执"),
        ];
        store
            .append_node_messages(
                &task.id,
                run_id,
                cursor.stage,
                cursor.node,
                crate::storage::AGENT_TYPE_MAIN,
                0,
                &half,
                None,
            )
            .await
            .unwrap();

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

        let got = ledger.take_continuation(&cursor).await.unwrap().unwrap();
        assert_eq!(got.mode, ContinuationMode::Transcript);
        assert_eq!(
            got.cause,
            ResumeCause::InfoInsufficient,
            "原因随素材一起交出"
        );
        assert_eq!(got.from_run_id, Some(run_id));
        assert_eq!(got.messages.len(), half.len() + 1, "缺的那一条补上了");
        assert_eq!(got.synthetic_tail, 1, "补的那一条算合成回执");
        assert_eq!(
            got.messages.last().unwrap().content.as_deref(),
            Some(crate::agent::context::RESTART_INTERRUPTED_NOTE),
            "未配齐的第一个调用说「可能已部分生效」"
        );
        // 合法：直接过消毒层，零改动、零残缺。
        let (clean, stats) = crate::agent::context::sanitize_tool_sequence(&got.messages);
        assert_eq!(stats, crate::agent::context::SanitizeStats::default());
        assert_eq!(clean, got.messages);
        // 工具调用的参数是**原始串**：还原之后逐字段一致。
        match &got.messages[1].tool_calls[..] {
            [first, second] => {
                assert_eq!(first.arguments, r#"{"path":"a"}"#);
                assert_eq!(second.arguments, r#"{"path":"."}"#);
            }
            other => panic!("工具调用条数不对：{other:?}"),
        }
        // 已完成的第一次调用**不在**补齐之列（它有真回执）。
        assert_eq!(
            got.messages[2].content.as_deref(),
            Some("第一条的真回执"),
            "已完成的那条保持原样"
        );
    }

    /// 票 02：梯子的转录**不再可能被截断**。构造「一条被截断过的会话行 + 一份完整日志」，
    /// 断言续接拿到的是**完整的**那一份——今天的截断从最旧一端整条丢消息、切口还可能落在
    /// 轮中间，留下的孤儿 tool 消息会被上游净化静默吃掉工具输出。
    #[tokio::test]
    async fn the_log_is_immune_to_conversation_truncation() {
        let (_tmp, store, task, cursor, clock) = base().await;
        let ledger = RunLedger::new(&store, &clock);
        let (run_id, attempt) = ledger
            .begin(&task, &cursor.cursor_id, cursor.stage, cursor.node, "main")
            .await
            .unwrap();
        let full = vec![
            Message::user("第一问"),
            Message::assistant(Some("第一答".into()), Vec::new()),
            Message::user("第二问"),
            Message::assistant(Some("第二答".into()), Vec::new()),
        ];
        store
            .append_node_messages(
                &task.id,
                run_id,
                cursor.stage,
                cursor.node,
                crate::storage::AGENT_TYPE_MAIN,
                0,
                &full,
                None,
            )
            .await
            .unwrap();
        // 观测面那份只剩最后一轮（截断的真实形态：从最旧一端整条丢）。
        store
            .insert_conversation(
                &task.id,
                run_id,
                cursor.stage,
                cursor.node,
                attempt,
                "main",
                None,
                &serde_json::to_value(&full[2..]).unwrap(),
                None,
                None,
                0,
                0,
                None,
            )
            .await
            .unwrap();

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

        let got = ledger.take_continuation(&cursor).await.unwrap().unwrap();
        assert_eq!(
            got.messages, full,
            "续接读日志不读会话表：被截断的那一份不再有机会成为续接素材"
        );
        assert_eq!(
            got.synthetic_tail, 0,
            "一整轮齐了（没有半截的工具调用）→ 一条合成回执都不补"
        );
    }

    /// 票 02：查找键是 `(task, stage, node)` **不是游标**——`goto` 是在**同一条游标行**上改
    /// `(stage, node)`，按游标找会把**上一个节点**的对话喂给这个节点。
    #[tokio::test]
    async fn the_lookup_key_does_not_follow_a_goto_across_nodes() {
        let (_tmp, store, task, mut cursor, clock) = base().await;
        let ledger = RunLedger::new(&store, &clock);
        // 上游节点（validate_input）先跑过一轮，留下它自己的日志。
        store
            .set_cursor_stage(
                &cursor.cursor_id,
                Stage::ArchitectDesign,
                Node::ValidateInput,
            )
            .await
            .unwrap();
        cursor = store.get_cursor(&cursor.cursor_id).await.unwrap();
        let (upstream_run, _) = ledger
            .begin(&task, &cursor.cursor_id, cursor.stage, cursor.node, "main")
            .await
            .unwrap();
        let upstream = vec![
            Message::user("上游节点的提问"),
            Message::assistant(Some("上游节点的回答".into()), Vec::new()),
        ];
        store
            .append_node_messages(
                &task.id,
                upstream_run,
                Stage::ArchitectDesign,
                Node::ValidateInput,
                crate::storage::AGENT_TYPE_MAIN,
                0,
                &upstream,
                None,
            )
            .await
            .unwrap();

        // `goto`：同一条游标行改到下游节点（execute），它自己还没有任何日志。
        store
            .set_cursor_stage(&cursor.cursor_id, Stage::ArchitectDesign, Node::Execute)
            .await
            .unwrap();
        let cursor = store.get_cursor(&cursor.cursor_id).await.unwrap();
        store
            .mark_cursor_continuation(&cursor.cursor_id, ResumeCause::GateRecheck)
            .await
            .unwrap();

        let got = ledger.take_continuation(&cursor).await.unwrap().unwrap();
        assert_eq!(got.cause, ResumeCause::GateRecheck, "原因照旧交出");
        assert!(
            got.messages.is_empty(),
            "下游节点没有自己的日志 → 干净起跑；**不能**把上游节点的对话喂给它：{:?}",
            got.messages
        );
        assert_eq!(got.from_run_id, None, "没有可续内容就不落谱系链接");
    }
}
