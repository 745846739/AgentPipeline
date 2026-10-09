//! L2 集成：值班长（决策 182，票 01 / 02 / 05）。
//!
//! 测的是**外部可观测的行为**：端点无关的那一层——快照装了什么、历史怎么裁、
//! 工具真的执行了没有、越权工具是不是在执行点被拒、对话动没动全局指标、保留期到点
//! 行有没有被清、以及**会话之间的隔离**（决策 204：隔离上下文，不隔离权限）。**不测 prompt 的效果**（答得好不好、判断准不准）：那是外部不确定性，
//! 本仓的口径是 prompt 效果回归属 v2 离线 eval（docs/testing.md §1）。
//!
//! FakeAgent 的替换边界照旧（决策 148）：**只替换 LLM 响应流，工具与存储全部真跑**。
//! 所以「read_task 真读到了台账」这件事由真 SQL 保证，不是脚本演出来的。

use std::sync::Arc;
use std::time::Duration;

use agentpipeline_core::agent::client::{AgentResponse, LlmClient, LlmRequest};
use agentpipeline_core::agent::tools::{is_env_write_tool, is_service_write_tool, ENV_TOOLS};
use agentpipeline_core::clock::Clock;
use agentpipeline_core::config::Settings;
use agentpipeline_core::metrics;
use agentpipeline_core::pipeline::foreman::{
    build_briefing, foreman_turn_in_flight, parse_attribution, situation_fingerprint, trim_history,
    Attribution, AttributionKind, ForemanRunner, ForemanSegment, COMPACTION_MARK,
    FOREMAN_AGENT_TYPE, FOREMAN_ATTRIBUTION_MARK, FOREMAN_FAILED_TURN_MARK,
    FOREMAN_LOOP_REMINDER_MARK, FOREMAN_LOOP_TURN_MARK, FOREMAN_MAX_ROUNDS, FOREMAN_NO_ACTION_MARK,
    FOREMAN_PARTIAL_TURN_MARK, FOREMAN_PERSONA, FOREMAN_STAGE_KEY, FOREMAN_STOPPED_TURN_MARK,
    FOREMAN_TALK_DIGEST_MARK, FOREMAN_TOOL_SPECS, FOREMAN_WATCH_DIGEST_MARK,
    FOREMAN_WATCH_FAILED_TURN_MARK, FOREMAN_WATCH_MARK, FOREMAN_WATCH_TOKEN_BUDGET,
    OPERATION_LOG_MARK,
};
use agentpipeline_core::sse::{SseEvent, SseEventType, ToolPhase};
use agentpipeline_core::storage::foreman::{
    InFlightPatch, NewForemanMessage, FOREMAN_MESSAGE_INTERRUPTED, FOREMAN_MESSAGE_IN_FLIGHT,
    FOREMAN_SESSION_KIND_TALK, FOREMAN_SESSION_KIND_WATCH, FOREMAN_WATCH_SESSION_TITLE,
};
use agentpipeline_core::storage::model_requests::{
    ModelRequestStatus, ModelRequestUsage, NewModelRequest,
};
use agentpipeline_core::storage::tasks::TaskFilter;
use agentpipeline_core::storage::Store;
use agentpipeline_core::types::{
    Node, PendingKind, PendingReason, Stage, StageConfig, Stewardship, TaskStatus,
    STEWARDSHIP_MAX_AUTO_RESUMES,
};
use agentpipeline_core::Error;
use testkit::{FakeAgent, ManualClock, Repo, Script, TestHome};

use crate::web_fetch::TinyHttp;

struct Harness {
    _home: TestHome,
    store: Store,
    clock: ManualClock,
    /// 修复闸门走命令收口之后要显式拿到这两件（决策 297 / 票 02）。
    settings: Settings,
    killer: Arc<dyn agentpipeline_core::process::ProcessKiller>,
}

impl Harness {
    /// 空 home：**没有项目、没有任务**。这是本特性最初的诉求形态
    /// （「对话不需要依赖任务」），也是用户故事 11 的首启场景。
    async fn empty() -> Self {
        let home = TestHome::new().unwrap();
        let clock = ManualClock::fixed();
        let store = Store::open(home.home().clone(), Arc::new(clock.clone()))
            .await
            .unwrap();
        Harness {
            _home: home,
            store,
            clock,
            settings: Settings::default(),
            killer: Arc::new(agentpipeline_core::process::RealProcessKiller),
        }
    }

    /// 带项目与任务：用来测快照装了什么。
    async fn seeded() -> Self {
        let h = Self::empty().await;
        let repo = h._home.scratch_dir("proj");
        testkit::seed_project(&h.store, "p1", "示例项目", &repo, "main")
            .await
            .unwrap();
        testkit::seed_task(&h.store, "t1", "p1").await.unwrap();
        h
    }

    /// 再建一个任务（都在 `p1` 下）。
    ///
    /// 待办表的 `task_id` 有真外键（一条「关于不存在任务的待办」不是事件，是坏数据），
    /// 所以凡是要造多任务事件的用例都得**先把任务建出来**，不能只写个 id 串。
    async fn task(&self, task_id: &str) {
        testkit::seed_task(&self.store, task_id, "p1")
            .await
            .unwrap();
    }

    /// 把一个**真 git 仓**接成 `p1`（修复那条路的前提：`repair_supported` 要路径下真有 `.git`）。
    ///
    /// 与 [`Harness::seeded`] 是两件事：后者给的是 `scratch_dir` 下的**裸目录**，够用来看快照；
    /// 而修复必须真起 worktree。也**不走** `testkit::seed_project` 的语言探测——它会把这个仓判成
    /// `cargo`，于是闸门变成真的 `cargo test --quiet`（分钟级），而用例要测的是「闸门过了 /
    /// 没过之后各发生什么」。`test_command_for` 对认不出的名字**原样返回**，故这里给一条恒真的命令。
    async fn git_project(&self, repo: &testkit::Repo, test_framework: &str) {
        let project = agentpipeline_core::types::Project {
            id: "p1".into(),
            name: "示例项目".into(),
            local_path: repo.path().display().to_string(),
            default_branch: "main".into(),
            language: None,
            test_framework: Some(test_framework.into()),
            lint_command: None,
            agents_md_path: None,
            created_at: self.store.now(),
        };
        self.store.create_project(&project).await.unwrap();
    }

    /// 值班长的环境档位（`repair` 归环境层：默认的 `ask` 会把它转成一条提议，
    /// 而修复那条用例要断言的是「worktree 真的拉起来了」——故显式配 `auto`）。
    async fn foreman_env(&self, mode: agentpipeline_core::types::EnvMode) {
        self.store
            .upsert_stage_config(&StageConfig {
                stage: FOREMAN_STAGE_KEY.to_string(),
                env_mode: Some(mode),
                ..Default::default()
            })
            .await
            .unwrap();
    }

    /// 新开一个班次（会话）。
    ///
    /// 本文件里凡是要「往库里塞几句话」的用例都先开一个——会话是这些行的**必填**
    /// 归属参数，不是可选项：靠缺省值猜「哪句属于哪一班」正是这次要修掉的东西。
    async fn session(&self) -> String {
        self.store.create_foreman_session("").await.unwrap().id
    }

    /// 值守台账的 id（决策 286 / 票 01：值守轮写它自己的班次）。
    async fn latest_watch_session(&self) -> String {
        self.store
            .latest_foreman_session_of_kind(FOREMAN_SESSION_KIND_WATCH)
            .await
            .unwrap()
            .expect("值守轮应当已经建出值守台账")
            .id
    }

    fn runner(&self, agent: FakeAgent) -> ForemanRunner {
        self.runner_with(Settings::default(), agent)
    }

    /// 配一个**窗口很小**的 provider（决策 291 / 票 06(b)：轮内压缩要能到线）。
    ///
    /// 窗口是唯一决定触发线的量（`estimate_context_capacity` 按 `Settings` 的比例算），
    /// 故用例拿它当旋钮——不必造一套「让上下文长大」的假读数。
    async fn provider_with_window(&self, window: u32) {
        self.store
            .upsert_provider(&agentpipeline_core::types::Provider {
                id: "p-win".into(),
                vendor: "openai".into(),
                model: "window-model".into(),
                context_window: window,
                base_url: None,
                api_key: None,
                enabled: true,
                created_at: self.store.now(),
                updated_at: self.store.now(),
            })
            .await
            .unwrap();
    }

    /// 注入托管动作执行替身的那一种构造（票 08）。
    fn runner_with_steward(
        &self,
        script: Script,
        calls: Arc<std::sync::atomic::AtomicUsize>,
    ) -> ForemanRunner {
        self.runner_with(Settings::default(), FakeAgent::new(script))
            .with_steward_actions(Arc::new(TestSteward { calls }))
    }

    /// 非缺省设置的那一种构造（节流阈值要按用例配）。
    fn runner_with(&self, settings: Settings, agent: FakeAgent) -> ForemanRunner {
        ForemanRunner::new(
            self.store.clone(),
            settings,
            self._home.home().clone(),
            Arc::new(agent) as Arc<dyn LlmClient>,
            Arc::new(testkit::SseRecorder::new()),
        )
    }

    /// 带**可读的事件录制器**的构造（决策 244：断言工具调用是**在轮次进行中**推出去的，
    /// 而不是等它落库）。
    ///
    /// 与 [`Self::runner_with`] 的差别只有一处：那个扔掉录制器（大多数用例只关心收口结果），
    /// 这个把它交出来。
    fn runner_recording(&self, agent: FakeAgent) -> (ForemanRunner, Arc<testkit::SseRecorder>) {
        let recorder = Arc::new(testkit::SseRecorder::new());
        let runner = ForemanRunner::new(
            self.store.clone(),
            Settings::default(),
            self._home.home().clone(),
            Arc::new(agent) as Arc<dyn LlmClient>,
            recorder.clone() as Arc<dyn agentpipeline_core::sse::SseSink>,
        );
        (runner, recorder)
    }

    /// 接任意替身的构造（决策 244：给 [`Thinking`] 那类「包一层 FakeAgent」的替身用）。
    fn runner_with_llm(&self, llm: Arc<dyn LlmClient>) -> ForemanRunner {
        ForemanRunner::new(
            self.store.clone(),
            Settings::default(),
            self._home.home().clone(),
            llm,
            Arc::new(testkit::SseRecorder::new()),
        )
    }
}

/// 把某个任务推到 pending（借真实游标路径，不伪造任务状态）。
///
/// **两步都要走**：`set_cursor_pending` 只动游标行（它是执行状态的唯一事实来源），
/// 任务行上的 `status` / `pending_reason` 是**焦点游标的投影**，由 `sync_task_projection`
/// 写——生产里 executor 每次节点转换后都会调它。漏了第二步，快照读到的就是一份
/// 「游标说卡住了、任务说还在排队」的自相矛盾读数。
async fn park_task(store: &Store, task_id: &str, kind: PendingKind, message: &str) {
    let cursors = store.load_live_cursors(task_id).await.unwrap();
    let cursor = cursors
        .first()
        .expect("seed_task 应当建了 main 游标")
        .clone();
    let reason = PendingReason::new(kind, cursor.stage, cursor.node, message);
    store
        .set_cursor_pending(&cursor.cursor_id, &reason)
        .await
        .unwrap();
    store.sync_task_projection(task_id).await.unwrap();
}

// ─────────────────────────── 快照组装（票 01）───────────────────────────

#[tokio::test]
async fn empty_home_produces_an_empty_briefing_without_erroring() {
    let h = Harness::empty().await;
    let briefing = build_briefing(&h.store).await.unwrap();
    assert!(briefing.pending.is_empty());
    assert!(briefing.running.is_empty());
    assert!(briefing.failed.is_empty());
    assert!(briefing.projects.is_empty());
    assert_eq!(briefing.done_count, 0);
    // 渲染必须给出可读的「空班」而不是空字符串——值班长看到空白会开始自己编现状。
    let rendered = briefing.render();
    assert!(rendered.contains("一个都没有"), "渲染：{rendered}");
    assert!(
        rendered.contains("没有任何需要你处理的事"),
        "渲染：{rendered}"
    );
}

#[tokio::test]
async fn briefing_carries_the_pending_reason_verbatim() {
    let h = Harness::seeded().await;
    park_task(
        &h.store,
        "t1",
        PendingKind::MergeApproval,
        "合入提案等你拍板：3 个文件",
    )
    .await;

    let briefing = build_briefing(&h.store).await.unwrap();
    assert_eq!(briefing.pending.len(), 1);
    let p = &briefing.pending[0];
    assert_eq!(p.task_id, "t1");
    assert_eq!(p.kind, "merge_approval");
    // 判据是**原因原文**，不是枚举名：只说「merge_approval」对人没有信息量。
    assert_eq!(p.message, "合入提案等你拍板：3 个文件");
    assert!(p.message.contains("3 个文件"));

    // 渲染里同时出现任务号、原因原文与内部类型——前者给人看，后者供它自己对照工具返回。
    let rendered = briefing.render();
    assert!(rendered.contains("t1"), "渲染：{rendered}");
    assert!(rendered.contains("3 个文件"), "渲染：{rendered}");
    assert!(rendered.contains("merge_approval"), "渲染：{rendered}");
}

#[tokio::test]
async fn briefing_lists_projects_and_counts_done_but_not_cancelled() {
    let h = Harness::seeded().await;
    testkit::seed_task(&h.store, "t2", "p1").await.unwrap();
    h.store
        .set_task_status("t2", TaskStatus::Done)
        .await
        .unwrap();
    testkit::seed_task(&h.store, "t3", "p1").await.unwrap();
    h.store
        .set_task_status("t3", TaskStatus::Cancelled)
        .await
        .unwrap();

    let briefing = build_briefing(&h.store).await.unwrap();
    assert_eq!(briefing.projects.len(), 1);
    assert_eq!(briefing.projects[0].name, "示例项目");
    // 已完成只计数不列清单（收工的不需要有人管）；已取消既不算完成也不列失败。
    assert_eq!(briefing.done_count, 1);
    assert!(briefing.failed.is_empty());
    assert!(briefing.pending.is_empty());
    assert!(briefing.running.is_empty());
}

#[tokio::test]
async fn briefing_separates_running_pending_and_failed() {
    let h = Harness::seeded().await;
    testkit::seed_task(&h.store, "t2", "p1").await.unwrap();
    testkit::seed_task(&h.store, "t3", "p1").await.unwrap();

    park_task(&h.store, "t1", PendingKind::UserDecision, "两条路你选一条").await;
    h.store
        .set_task_status("t2", TaskStatus::Running)
        .await
        .unwrap();
    h.store
        .set_task_status("t3", TaskStatus::Failed)
        .await
        .unwrap();

    let briefing = build_briefing(&h.store).await.unwrap();
    assert_eq!(briefing.pending.len(), 1);
    assert_eq!(briefing.running.len(), 1);
    assert_eq!(briefing.failed.len(), 1);
    assert_eq!(briefing.running[0].task_id, "t2");
    assert_eq!(briefing.failed[0].task_id, "t3");
}

// ─────────────────────────── 历史裁剪（票 01）───────────────────────────

#[tokio::test]
async fn history_is_trimmed_by_character_budget_but_stays_in_the_store() {
    let h = Harness::empty().await;
    let sid = h.session().await;
    // 三条各 100 字的消息；预算只够容纳最新的两条。
    let body = "字".repeat(100);
    for role in ["user", "assistant", "user"] {
        h.store
            .append_foreman_message(NewForemanMessage {
                session_id: sid.clone(),
                role: role.to_string(),
                content: body.clone(),
                prompt_tokens: 0,
                completion_tokens: 0,
                briefing_json: None,
                traces_json: None,
                segments_json: None,
                changed_files_json: None,
                thinking: None,
                ask_json: None,
            })
            .await
            .unwrap();
    }
    let all = h
        .store
        .list_foreman_messages(&sid, 100, None)
        .await
        .unwrap();
    assert_eq!(all.len(), 3);

    let kept = trim_history(&all, 250);
    assert_eq!(kept.len(), 2, "预算 250 只装得下两条 100 字的");
    assert_eq!(kept[0].id, all[1].id, "保留的应是最新的两条");
    assert_eq!(kept[1].id, all[2].id);

    // 被裁掉的历史**仍在库里**：裁剪只影响这一轮注入了什么。
    let after = h
        .store
        .list_foreman_messages(&sid, 100, None)
        .await
        .unwrap();
    assert_eq!(after.len(), 3);
}

#[tokio::test]
async fn history_trimming_always_keeps_the_newest_message_even_over_budget() {
    let h = Harness::empty().await;
    let sid = h.session().await;
    h.store
        .append_foreman_user_message(&sid, "短")
        .await
        .unwrap();
    h.store
        .append_foreman_user_message(&sid, &"长".repeat(500))
        .await
        .unwrap();
    let all = h
        .store
        .list_foreman_messages(&sid, 100, None)
        .await
        .unwrap();

    // 预算小到连最新一条都装不下——仍必须保留它，否则值班长会答非所问。
    let kept = trim_history(&all, 10);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].id, all[1].id);
}

// ─────────────── 上下文压缩（决策 269 / 票 foreman-within-boundary 03）───────────────

/// 按标记找注入的那条 user 轮（锚点 / 互喂摘要）——决策 299 后它们尾部注入，
/// 下标会随历史长度漂，标记才是它们的身份。
fn find_marked(
    msgs: &[agentpipeline_core::agent::client::Message],
    mark: &str,
) -> Option<(usize, String)> {
    msgs.iter().enumerate().find_map(|(i, m)| {
        let text = m.content.clone().unwrap_or_default();
        text.contains(mark).then_some((i, text))
    })
}

/// 把会话塞到**超预算**（24k）：首条带「最早标记」，其后 30 条各约千字。
/// 掉出预算的必然包含首条——摘要器的输入里要看得见它。
async fn seed_over_budget(h: &Harness, sid: &str) {
    h.store
        .append_foreman_user_message(sid, "最早标记：我们决定用方案甲")
        .await
        .unwrap();
    let filler = "史".repeat(990);
    for i in 0..30 {
        let content = format!("第{i}轮 {filler}");
        if i % 2 == 0 {
            h.store
                .append_foreman_user_message(sid, &content)
                .await
                .unwrap();
        } else {
            h.store
                .append_foreman_message(NewForemanMessage::assistant(sid, content))
                .await
                .unwrap();
        }
    }
}

#[tokio::test]
async fn over_budget_history_is_summarized_into_a_tail_anchor_within_budget() {
    let h = Harness::empty().await;
    let sid = h.session().await;
    seed_over_budget(&h, &sid).await;

    let mut script = Script::new();
    script
        .for_foreman()
        .text("压缩后的摘要") // 第一次调用 = 摘要器
        .text("收口了。"); // 第二次 = 主轮
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());
    runner.say(Some(&sid), "现在怎么样？").await.unwrap();

    let reqs = agent.request_log();
    assert_eq!(reqs.len(), 2, "摘要一次 + 主轮一次：{}", reqs.len());
    // 摘要器：无工具、专用指令、输入里有掉出预算的最老内容
    assert!(reqs[0].tools.is_empty(), "摘要器不该带工具");
    assert!(
        reqs[0].system_prompt.contains("压缩"),
        "摘要器要认得出自己的指令：{}",
        reqs[0].system_prompt
    );
    assert!(
        reqs[0].user_prompt.contains("最早标记"),
        "掉出预算的最老轮次是摘要输入：{}",
        reqs[0].user_prompt
    );
    // 主轮：锚点 user 轮 = 标记 + 摘要正文；落在**历史之后、本轮问题之前**
    // （spec .scratch/prompt-cache：每轮会变的内容在稳定前缀之后，决策 269 的注入位置随之修订）。
    let (pos, head) = find_marked(&reqs[1].messages, COMPACTION_MARK).expect("锚点要在场");
    assert_eq!(
        reqs[1].messages[pos].role,
        agentpipeline_core::agent::client::Role::User,
        "锚点按 system 行重注入的先例走 user 角色"
    );
    assert!(
        head.contains(COMPACTION_MARK) && head.contains("压缩后的摘要"),
        "锚点要同时有标记与摘要正文：{head}"
    );
    assert!(
        pos > 0 && pos < reqs[1].messages.len() - 1,
        "锚点在历史窗口之后、本轮问题之前——不再占 wire 头：{pos}"
    );
    let total: usize = reqs[1]
        .messages
        .iter()
        .map(|m| m.content.as_deref().unwrap_or_default().chars().count())
        .sum();
    assert!(
        total <= agentpipeline_core::pipeline::foreman::FOREMAN_HISTORY_BUDGET_CHARS,
        "锚点自身计入预算算术（269②），锚点 + 窗口不许超：{total}"
    );
}

#[tokio::test]
async fn budget_internal_history_needs_no_anchor_and_no_extra_call() {
    let h = Harness::empty().await;
    let sid = h.session().await;
    h.store
        .append_foreman_user_message(&sid, "很短的历史")
        .await
        .unwrap();
    h.store
        .append_foreman_message(NewForemanMessage::assistant(&sid, "也很短"))
        .await
        .unwrap();

    let mut script = Script::new();
    script.for_foreman().text("好。");
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());
    runner.say(Some(&sid), "在吗？").await.unwrap();

    let reqs = agent.request_log();
    assert_eq!(
        reqs.len(),
        1,
        "预算内逐字照旧——一次调用都不许多（269①）：{}",
        reqs.len()
    );
    assert!(
        reqs[0].messages.iter().all(|m| !m
            .content
            .as_deref()
            .unwrap_or_default()
            .contains(COMPACTION_MARK)),
        "预算内不许出现锚点"
    );
}

#[tokio::test]
async fn successive_over_budget_rounds_incrementally_recompress_only_new_drops() {
    let h = Harness::empty().await;
    let sid = h.session().await;
    seed_over_budget(&h, &sid).await;

    let mut script = Script::new();
    // 回合一够长（+1500 字 > 窗口的最大余量 1006）：第二轮的裁剪边界**必然**前进，
    // 才有「新掉队的轮次」可增量压——边界不动时缓存直接复用（那是另一条语义，见下）。
    let long_reply = format!("回合一 {}", "重".repeat(1500));
    script
        .for_foreman()
        .text("摘要一")
        .text(&long_reply)
        .text("摘要二")
        .text("回合二");
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());
    runner.say(Some(&sid), "第一问？").await.unwrap();
    runner.say(Some(&sid), "第二问？").await.unwrap();

    let reqs = agent.request_log();
    assert_eq!(reqs.len(), 4, "两轮 ×（摘要 + 主轮）：{}", reqs.len());
    // 第二次摘要 = 旧摘要 + 新掉队的种子老轮（269③ 增量再压）——最老原文不整段重发
    assert!(
        reqs[2].user_prompt.contains("摘要一"),
        "旧摘要要进增量输入：{}",
        reqs[2].user_prompt
    );
    assert!(
        reqs[2].user_prompt.contains("史"),
        "新掉队的轮次（种子里的千字轮）要进增量输入：{}",
        reqs[2].user_prompt
    );
    assert!(
        !reqs[2].user_prompt.contains("最早标记"),
        "最老原文已经压进旧摘要，不整段重发（每条轮次一生只被压一次）：{}",
        reqs[2].user_prompt
    );
    // 第二轮主轮的锚点换成了新摘要（尾部注入：按标记找，不按下标）
    let (_, head2) = find_marked(&reqs[3].messages, COMPACTION_MARK).expect("第二轮锚点要在场");
    assert!(head2.contains("摘要二"), "锚点要跟上最新摘要：{head2}");
}

#[tokio::test]
async fn a_failed_summarizer_falls_back_to_plain_dropping() {
    let h = Harness::empty().await;
    let sid = h.session().await;
    seed_over_budget(&h, &sid).await;

    let mut script = Script::new();
    script
        .for_foreman()
        .push(testkit::Step::Fail {
            kind: "llm_network".into(),
            message: "摘要器挂了".into(),
            raw: "boom".into(),
        })
        .text("收口了。");
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());
    let turn = runner.say(Some(&sid), "在吗？").await;
    assert!(
        turn.is_ok(),
        "轮次绝不因摘要挂掉而挂掉（269④ 回退现状）：{turn:?}"
    );

    let reqs = agent.request_log();
    assert_eq!(reqs.len(), 2, "失败的摘要尝试 + 主轮：{}", reqs.len());
    assert!(
        reqs[1].messages.iter().all(|m| !m
            .content
            .as_deref()
            .unwrap_or_default()
            .contains(COMPACTION_MARK)),
        "回退路径不许有锚点——与现状（从头丢）逐字相同"
    );
}

// ─────────── 注入位置（spec .scratch/prompt-cache：每轮会变的内容在稳定前缀之后）───────────

/// 快照不再占 `user_prompt` 槽（wire 第 2 条、全部历史之前）——它并进本轮最后一条
/// user 轮，历史窗口逐字在前。落实决策 182⑤ 的字面承诺（「快照夹在历史之后」）。
#[tokio::test]
async fn the_briefing_lands_in_the_final_turn_not_before_the_history() {
    let h = Harness::empty().await;
    let sid = h.session().await;
    h.store
        .append_foreman_user_message(&sid, "第一句")
        .await
        .unwrap();
    h.store
        .append_foreman_message(NewForemanMessage::assistant(&sid, "答了"))
        .await
        .unwrap();

    let mut script = Script::new();
    script.for_foreman().text("收口。");
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());
    runner.say(Some(&sid), "第二句").await.unwrap();

    let reqs = agent.request_log();
    assert_eq!(reqs.len(), 1, "{}", reqs.len());
    let req = &reqs[0];
    assert!(
        req.user_prompt.is_empty(),
        "user_prompt 槽退化为空（每轮变化的内容不在历史之前）：{}",
        req.user_prompt
    );
    assert_eq!(req.messages[0].content.as_deref(), Some("第一句"));
    assert_eq!(req.messages[1].content.as_deref(), Some("答了"));
    let last = req.messages.last().expect("末尾轮在场");
    let text = last.content.as_deref().unwrap_or_default();
    assert!(text.contains("夜班态势快照"), "快照并进末尾轮：{text}");
    assert!(
        text.ends_with("第二句"),
        "问题仍是末尾轮的最后一句（人格「问题由最后一条承担」）：{text}"
    );
    assert!(
        req.messages[..2].iter().all(|m| !m
            .content
            .as_deref()
            .unwrap_or_default()
            .contains("夜班态势快照")),
        "历史前半一处快照都不许有"
    );
}

/// 前缀稳定：上一轮除末尾合并轮之外的全部消息是下一轮的**逐字前缀**——
/// 无压缩、无互喂的相邻两轮之间，历史窗口逐轮命中（本 spec 的核心判据）。
#[tokio::test]
async fn the_history_prefix_is_byte_stable_across_rounds() {
    let h = Harness::empty().await;
    let sid = h.session().await;
    h.store
        .append_foreman_user_message(&sid, "第一句")
        .await
        .unwrap();
    h.store
        .append_foreman_message(NewForemanMessage::assistant(&sid, "答了"))
        .await
        .unwrap();

    let mut script = Script::new();
    script.for_foreman().text("回一").text("回二");
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());
    runner.say(Some(&sid), "第二句").await.unwrap();
    runner.say(Some(&sid), "第三句").await.unwrap();

    let reqs = agent.request_log();
    assert_eq!(reqs.len(), 2, "{}", reqs.len());
    let (r1, r2) = (&reqs[0], &reqs[1]);
    assert!(r1.user_prompt.is_empty() && r2.user_prompt.is_empty());
    let head1 = &r1.messages[..r1.messages.len() - 1];
    assert!(!head1.is_empty(), "第一轮要有历史在前");
    let key = |m: &agentpipeline_core::agent::client::Message| (m.role, m.content.clone());
    assert_eq!(
        head1.iter().map(key).collect::<Vec<_>>(),
        r2.messages[..head1.len()]
            .iter()
            .map(key)
            .collect::<Vec<_>>(),
        "上一轮的历史段是下一轮的逐字前缀"
    );
    // 分歧点之后，落库的原问以**原文**（不带快照）续在前缀之后——库里那行不被改写。
    assert_eq!(
        r2.messages[head1.len()].content.as_deref(),
        Some("第二句"),
        "原问以原文续在前缀之后：{:?}",
        r2.messages[head1.len()].content
    );
}

#[tokio::test]
async fn session_listing_returns_the_newest_tail_in_chronological_order() {
    let h = Harness::empty().await;
    let sid = h.session().await;
    for i in 0..5 {
        h.store
            .append_foreman_user_message(&sid, &format!("第{i}句"))
            .await
            .unwrap();
    }
    let tail = h.store.list_foreman_messages(&sid, 2, None).await.unwrap();
    // LIMIT 必须作用在最新那一端，再按时间升序交出去。
    assert_eq!(tail.len(), 2);
    assert_eq!(tail[0].content, "第3句");
    assert_eq!(tail[1].content, "第4句");
    assert!(h
        .store
        .list_foreman_messages(&sid, 0, None)
        .await
        .unwrap()
        .is_empty());
}

// ──────────────────── 工具调用的实时声道（决策 244）────────────────────

/// 工具调用**在轮次进行中**就推事件出去，而不是等它落库。
///
/// 诉求的原话是「把对讲台的 thinking 和工具调用都实时展示出来，不要像现在这样在对话
/// 完结后展示」。改动之前，值班长的工具痕迹只有两条路到界面：落库的 `traces_json`
/// （那一轮收口之后才有）——即「对话完结后」。本用例钉的就是这条改变：`start` / `end`
/// 两个相位都在 `say()` 返回**之前**已经发出去了。
#[tokio::test]
async fn foreman_tool_calls_are_published_live_not_only_after_the_turn() {
    let h = Harness::seeded().await;
    let mut script = Script::new();
    script
        .for_foreman()
        .read_task("t1")
        .text("t1 还在排队，没别的事。");
    let (runner, recorder) = h.runner_recording(FakeAgent::new(script));

    let turn = runner.say(None, "t1 怎么样了？").await.unwrap();
    assert_eq!(turn.traces.len(), 1, "痕迹照旧落库（审计那条路不动）");

    let tool_events: Vec<_> = recorder
        .events()
        .into_iter()
        .filter(|e| e.event_type() == SseEventType::ToolEvent)
        .collect();
    assert_eq!(tool_events.len(), 2, "一次调用两个相位：{tool_events:?}");
    for (i, event) in tool_events.iter().enumerate() {
        match event {
            SseEvent::ToolEvent {
                agent_type,
                session_id,
                tool,
                phase,
                args_summary,
                args,
                result,
                task_id,
                branch,
                ..
            } => {
                // 身份串与会话是这条事件走得到 `/foreman/stream` 的唯一凭据
                // （`is_foreman_event` 的判据）。
                assert_eq!(agent_type, "foreman");
                assert_eq!(session_id, &turn.session.id);
                assert_eq!(tool, "read_task");
                assert!(args_summary.contains("t1"));
                // 决策 301：详情原文与结果随事件**当场**到（start 只有参数，end 才有结果）。
                assert!(args.contains("t1"), "start 也带完整参数原文：{args:?}");
                // 空 task id / 空分支（决策 182⑥）：工头事件不挂流水线的坐标。
                assert_eq!(task_id, "");
                assert_eq!(branch, "");
                let expected = if i == 0 {
                    assert!(result.is_none(), "start 还没有结果：{result:?}");
                    ToolPhase::Start
                } else {
                    let detail = result.as_ref().expect("end 事件要带结果详情");
                    assert!(!detail.is_empty(), "结果详情不是空串");
                    ToolPhase::End
                };
                assert_eq!(*phase, expected, "相位顺序必须是 start → end");
            }
            other => panic!("应是工具事件：{other:?}"),
        }
    }
    // 这条事件真的会被对讲台那条路由收下（判据本身，不只是字段长得对）。
    assert!(tool_events.iter().all(|e| e.is_foreman_event()));
    // 任务级流照旧收不到它：空 task id 永不等于真实任务 id（决策 182⑥ 的零干扰）。
    assert!(tool_events.iter().all(|e| e.task_id() != "t1"));
}

/// 工具**失败**也走实时声道，且相位是 `error` 而不是静默。
///
/// 失败不该上升为整轮回话失败（§12.8），但它必须让人看得见「这一下没查到」——
/// 否则界面上那次调用会停在「正在查…」不动。
///
/// 用 `read_file` 而不是 `read_task` 来造这个失败，是**故意的**：读一个不存在的任务
/// **不算失败**（`read_task` 自己把「没这个任务」当正常回答交回去，见它的实现注释——
/// 模型记错 id 不该让整轮报错），而读一个不存在的文件走的是真 `Err` 通道。
#[tokio::test]
async fn foreman_reports_a_failed_tool_call_live_with_the_error_phase() {
    let h = Harness::seeded().await;
    let mut script = Script::new();
    script
        .for_foreman()
        .tool("read_file", serde_json::json!({"path": "没有这个文件.md"}))
        .text("那个文件不在，我换个线索查。");
    let (runner, recorder) = h.runner_recording(FakeAgent::new(script));

    let turn = runner.say(None, "看看那个文件").await.unwrap();
    assert!(turn.reply.contains("换个线索"));
    assert_eq!(turn.traces.len(), 1);
    assert!(!turn.traces[0].ok, "这一条痕迹该记成失败");

    let phases: Vec<_> = recorder
        .events()
        .into_iter()
        .filter_map(|e| match e {
            SseEvent::ToolEvent { phase, .. } => Some(phase),
            _ => None,
        })
        .collect();
    assert_eq!(phases, vec![ToolPhase::Start, ToolPhase::Error]);

    // 决策 301：失败的详情也进 `result`（error 事件与留痕同一份）——
    // 否则界面点开那次调用只有摘要一行，「为什么没读到」看不见。
    let error_result = recorder
        .events()
        .iter()
        .find_map(|e| match e {
            SseEvent::ToolEvent {
                phase: ToolPhase::Error,
                result: Some(r),
                ..
            } => Some(r.clone()),
            _ => None,
        })
        .expect("error 事件带错误文本");
    assert!(
        error_result.contains("工具执行失败"),
        "错误文本进 result：{error_result}"
    );
    assert_eq!(turn.traces[0].result, error_result, "事件与留痕同一份");
}

// ─────────────────────────── 会话（票 01 / 02）───────────────────────────

#[tokio::test]
async fn empty_home_can_hold_a_conversation_and_it_survives_a_reload() {
    // 本 spec 的验收锚点：**一台全新机器上（没有项目、没有任务）第一次打开就能对话**。
    let h = Harness::empty().await;
    let mut script = Script::new();
    script
        .for_foreman()
        .text("现在什么都没有在跑。先建一个项目，再建任务。");
    let runner = h.runner(FakeAgent::new(script));

    // 不带班次说话：服务端开一个（首启空 home 的第一次说话就是这条），
    // 并在返回值里告诉客户端它落进了哪个班次。
    let turn = runner.say(None, "现在能做什么？").await.unwrap();
    assert!(turn.reply.contains("先建一个项目"));
    assert!(turn.prompt_tokens > 0);
    assert_eq!(turn.traces.len(), 0, "这一轮没有调工具");
    let sid = turn.session.id.clone();
    assert!(!sid.is_empty(), "第一次说话必须落到一个确定的班次上");
    // 标题取自首条用户消息（决策 204②）。
    assert_eq!(turn.session.title, "现在能做什么？");

    // 两句都落库，且顺序是「人先说、值班长后答」。
    let messages = h
        .store
        .list_foreman_messages(&sid, 100, None)
        .await
        .unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[0].content, "现在能做什么？");
    assert_eq!(messages[1].role, "assistant");
    // 回话那一行带上了它当时看到的快照（审计：依据什么）。
    assert!(messages[1].briefing_json.is_some());
    assert!(messages[0].briefing_json.is_none(), "用户行没有快照");

    // 「刷新页面还在」= 换一个 runner 从库里重读，历史仍在。
    let mut again = Script::new();
    again.for_foreman().text("还是什么都没有。");
    let runner2 = h.runner(FakeAgent::new(again));
    // 指定同一个班次：这就是「切换 / 重开页面后接着上一班说」的存储侧形态。
    runner2.say(Some(&sid), "再问一次").await.unwrap();
    let after = h
        .store
        .list_foreman_messages(&sid, 100, None)
        .await
        .unwrap();
    assert_eq!(after.len(), 4);
}

#[tokio::test]
async fn the_llm_request_carries_the_foreman_identity_and_a_placeholder_stage() {
    let h = Harness::empty().await;
    let mut script = Script::new();
    script.for_foreman().text("收到。");
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());
    let turn = runner.say(None, "在吗").await.unwrap();

    let requests = agent.request_log();
    assert_eq!(requests.len(), 1);
    let req = &requests[0];
    let run = req.run.as_ref().expect("必须有 run 上下文（流式要用）");
    assert_eq!(run.agent_type, FOREMAN_AGENT_TYPE);
    assert_eq!(run.task_id, "", "空 task id 是既有任务级 SSE 零干扰的前提");
    // 会话身份随增量出去（决策 204⑥）：手机与电脑同时连着时，
    // 前端靠它把回话归到正确的班次，而不是混成一段。
    assert_eq!(run.session_id, turn.session.id);
    // 阶段枚举**没有**新增变体（决策 182①）：占位阶段是既有的 Init。
    assert_eq!(req.stage, Stage::Init);
    // 工具集是**清单驱动**的（票 01）：广告集与执行点白名单同源，都从
    // `FOREMAN_TOOL_SPECS` 来。断言打的是那份清单而不是字面量——漂移的表现是模型看得见
    // 一个调用就被拒的工具（或反过来，一个能调但没人告诉它的工具），两种都很难从现象定位。
    let names: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();
    let listed: Vec<&str> = FOREMAN_TOOL_SPECS
        .iter()
        .map(|s| s.name)
        .collect::<Vec<_>>();
    assert_eq!(names, listed, "广告的工具集必须与清单逐字一致（且同序）");
    // 反向断言（本票最该保留的一条）：**够不到手的工具名一个都不得进广告集**。
    // 逐个名字列出来而不是只断言数量：数量对得上、名字换了一个的情况，只断言数量看不出来。
    //
    // 这一列随票 04 / 05 / 06 收窄过一次：`write_file` / `edit_file` / `run_command` /
    // `task` / `config` / `skills` **都不在这一列了**——它们进了清单，走的是确认钮（决策
    // 188 / 207），而不是「不给」。留下的是**真的不该给**的那些：
    // - `delete_file`：删除不可逆，决策 207 的一份清单里没有它，本票也不给它开；
    // - `spawn_sub_agent`：要注入一个子代理运行器才有意义，值班长手上没有；
    // - `submit_metadata`：它是流水线节点向状态机提交结构化元数据的口子，
    //   值班长没有状态机可提交（它是面向人的对话者）。
    for forbidden in ["delete_file", "spawn_sub_agent", "submit_metadata"] {
        assert!(
            !names.contains(&forbidden),
            "值班长的工具集不得含 {forbidden}：{names:?}"
        );
    }
    assert!(
        names.contains(&"read_file"),
        "B 层环境只读应当在广告集里（决策 206）：{names:?}"
    );
    // 写面按档位广告：缺省 `ask` 下三个 C / D / E 层的名字**都在**（它们靠确认钮兜住，
    // 不是靠不给）——这与「D 层排除清单」是两件事，排除的那三项压根没有工具名。
    for present in [
        "write_file",
        "edit_file",
        "run_command",
        "task",
        "config",
        "skills",
    ] {
        assert!(
            names.contains(&present),
            "{present} 缺省档位下应当在广告集里（走确认钮）：{names:?}"
        );
    }
}

#[tokio::test]
async fn say_persists_the_user_message_even_when_the_model_fails() {
    // 「LLM 报错时输入框内容不清空」的前提是后端**记住了他说过什么**：
    // 否则刷新页面后那句话就没了（审计要的是「他说了什么」，不是「哪句话被答复了」）。
    struct Boom;
    impl LlmClient for Boom {
        fn complete(
            &self,
            _request: LlmRequest,
        ) -> futures::future::BoxFuture<
            'static,
            agentpipeline_core::Result<agentpipeline_core::agent::client::AgentResponse>,
        > {
            Box::pin(async { Err(Error::Llm("模型没配".into())) })
        }
    }
    let h = Harness::empty().await;
    let runner = ForemanRunner::new(
        h.store.clone(),
        Settings::default(),
        h._home.home().clone(),
        Arc::new(Boom) as Arc<dyn LlmClient>,
        Arc::new(testkit::SseRecorder::new()),
    );
    let sid = h.session().await;
    let err = runner.say(Some(&sid), "喂").await.unwrap_err();
    assert!(matches!(err, Error::Llm(_)));

    // 票 04 改了这条口径：user 行**不再孤立**——失败当场落一条 `system` 账，
    // 把「为什么没回话」写下来（2026-09-17 实测里那两次静默失败，库里一个字都没有）。
    let messages = h
        .store
        .list_foreman_messages(&sid, 100, None)
        .await
        .unwrap();
    assert_eq!(messages.len(), 2, "用户那一行 + 失败那一行：{messages:?}");
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[0].content, "喂");
    assert_eq!(messages[1].role, "system");
    assert!(
        messages[1].content.starts_with(FOREMAN_FAILED_TURN_MARK),
        "失败轮要有标记，前端靠它渲染成失败轮：{}",
        messages[1].content
    );
    assert!(
        messages[1].content.contains("llm_network") && messages[1].content.contains("模型没配"),
        "失败原因要能归因，且带回原始串：{}",
        messages[1].content
    );
}

/// 空闲判死的失败**按瞬时类重试一次**（决策 288 / 票 05）：同一份请求原样再发一次，
/// 仍失败才让这一轮失败。这里用替身直接产出 `llm_idle_timeout` 类——生产里它由
/// provider 流循环的 watchdog 判出（`production_llm.rs` 那一组用例钉的就是那半）。
#[tokio::test]
async fn an_idle_timeout_is_retried_once_and_then_succeeds() {
    let h = Harness::empty().await;
    let mut script = Script::new();
    script
        .for_foreman()
        .push(testkit::Step::Fail {
            kind: "llm_idle_timeout".into(),
            message: "模型很久没有给出任何内容（空闲判死）".into(),
            raw: "流上 1 秒没有任何新字节".into(),
        })
        .text("在的，我看完了。");
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());
    let sid = h.session().await;

    let turn = runner.say(Some(&sid), "盯着 t1").await.unwrap();
    assert_eq!(turn.reply, "在的，我看完了。");
    assert_eq!(agent.total_calls(), 2, "判死一次 + 重试一次");

    // 用户那一句 + 成功的回话。
    let messages = h
        .store
        .list_foreman_messages(&sid, 100, None)
        .await
        .unwrap();
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert_eq!(messages[1].role, "assistant");
}

/// 空闲判死**重试仍失败**才是这一轮的失败（决策 288 / 票 05）：类别原样进失败账，
/// 人的那句话照旧落库——失败外框的既有行为只换了一个触发类别。
#[tokio::test]
async fn an_idle_timeout_that_fails_twice_is_recorded_with_its_kind() {
    let h = Harness::empty().await;
    let boom = || testkit::Step::Fail {
        kind: "llm_idle_timeout".into(),
        message:
            "模型很久没有给出任何内容（空闲判死）：多半是 provider 临时卡住——稍等片刻重试通常能过"
                .into(),
        raw: "流上 300 秒没有任何新字节（本次已收 1200 字节）：已中止这一次调用".into(),
    };
    let mut script = Script::new();
    script.for_foreman().push(boom()).push(boom());
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());
    let sid = h.session().await;

    let err = runner.say(Some(&sid), "盯着 t1").await.unwrap_err();
    match &err {
        Error::LlmClassified { kind, raw, .. } => {
            assert_eq!(kind, "llm_idle_timeout");
            assert!(raw.contains("没有任何新字节"), "{raw}");
        }
        other => panic!("应当是空闲判死这一类，实际：{other:?}"),
    }
    assert_eq!(agent.total_calls(), 2, "判死一次 + 重试一次，不再更多");

    // 用户那一句 + 失败那一句：这一轮**不再**只剩孤立的用户行。
    let messages = h
        .store
        .list_foreman_messages(&sid, 100, None)
        .await
        .unwrap();
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert_eq!(messages[0].role, "user");
    assert!(
        messages[1].content.starts_with(FOREMAN_FAILED_TURN_MARK)
            && messages[1].content.contains("空闲判死"),
        "失败那一行要带类别：{}",
        messages[1].content
    );
}

/// 被 panic 带走的那一轮也要留痕（决策 223）：`say()` 的失败外框本身就在 panic 里没了，
/// 能补这一条的只有调用方——HTTP 端点拿到 `JoinError` 时调的就是这个方法。
#[tokio::test]
async fn an_interrupted_turn_is_recorded_in_the_latest_session() {
    let h = Harness::empty().await;
    let runner = h.runner(FakeAgent::new(Script::new()));
    let sid = h.session().await;
    // 用户那一句先落库（panic 发生在它之后、回话之前）；落库会更新会话的
    // `last_active_at`，故「最近活动的班次」就是它。
    h.store
        .append_foreman_user_message(&sid, "盯着 t1")
        .await
        .unwrap();

    runner.record_interrupted_turn("内部错误").await;

    let messages = h
        .store
        .list_foreman_messages(&sid, 100, None)
        .await
        .unwrap();
    let last = messages.last().expect("至少两行");
    assert_eq!(last.role, "system");
    assert!(
        last.content.starts_with(FOREMAN_FAILED_TURN_MARK) && last.content.contains("内部错误"),
        "中断那一行要可归因：{}",
        last.content
    );
}

/// 失败回合的归因用 `LlmClassified` 的 kind 机制（票 04）：空回话是**模型行为**，
/// 不是内部故障——类别正是排查的入口。
#[tokio::test]
async fn a_silent_model_is_recorded_with_its_own_kind() {
    let h = Harness::empty().await;
    let mut script = Script::new();
    // 空文本步：内容过滤后为空 → 这一轮没有回话
    script.for_foreman().text("");
    let runner = h.runner(FakeAgent::new(script));
    let sid = h.session().await;

    let err = runner.say(Some(&sid), "在吗").await.unwrap_err();
    let kind = err.llm_classified().map(|(k, _)| k.to_string());
    assert_eq!(
        kind.as_deref(),
        Some("model_empty_reply"),
        "空回话要带自己的类别：{err}"
    );

    let messages = h
        .store
        .list_foreman_messages(&sid, 100, None)
        .await
        .unwrap();
    let failed = messages
        .iter()
        .find(|m| m.role == "system")
        .expect("失败回合要落一条 system 账");
    assert!(
        failed.content.contains("model_empty_reply"),
        "{}",
        failed.content
    );
    assert!(failed.content.contains("没有回话"), "{}", failed.content);
}

/// 轮数耗尽与空回话是**两种不同的失败**（票 04 的 kind 机制）：脚本每一轮都发起工具调用，
/// 模型一直在查台账、从不收口——到上限必须有人喊停，否则它会烧 token 直到 HTTP 超时
/// （决策 182④）。
///
/// 脚本按常量声明而不是写死轮数：本用例钉的是「耗尽就报这一类别、并且落一条可归因的账、
/// 模型确实被叫了整整数轮」，不是「上限恰好是几」——那个数没有用例值得钉。
///
/// **每一轮查的是另一个 id**（`read_task` 对不存在的 id 回一句带 id 的「台账里没有任务…」），
/// 于是调用与结果都各不相同：这个用例要的是「叫满整整数轮」那条路，而「同一个调用重复」
/// 那条路已由票 08 的循环检测提前拦下（`a_repeating_tool_call_is_reminded_once_then_closed_out`）
/// ——从前两者共用同一份「一查到底」的脚本，加了检测之后必须分开。
#[tokio::test]
async fn a_foreman_that_never_wraps_up_is_capped_and_named() {
    let h = Harness::empty().await;
    let mut script = Script::new();
    for i in 0..FOREMAN_MAX_ROUNDS {
        script
            .for_foreman()
            .tool("read_task", serde_json::json!({"task_id": format!("t{i}")}));
    }
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());
    let sid = h.session().await;

    let err = runner.say(Some(&sid), "盯着这个任务").await.unwrap_err();
    let kind = err.llm_classified().map(|(k, _)| k.to_string());
    assert_eq!(
        kind.as_deref(),
        Some("model_no_reply"),
        "轮数耗尽要带自己的类别：{err}"
    );
    // 整整数轮模型调用：上限就是「模型被叫了几次」，不是「工具被调了几次」。
    assert_eq!(
        agent.calls_for(Stage::Init, Node::Execute),
        FOREMAN_MAX_ROUNDS as u32,
        "该停在上限上"
    );

    let messages = h
        .store
        .list_foreman_messages(&sid, 100, None)
        .await
        .unwrap();
    let failed = messages
        .iter()
        .find(|m| m.role == "system")
        .expect("失败回合要落一条 system 账");
    assert!(
        failed.content.contains("model_no_reply") && failed.content.contains("轮内没有给出回话"),
        "{}",
        failed.content
    );
}

#[tokio::test]
async fn empty_message_is_rejected_and_not_persisted() {
    let h = Harness::empty().await;
    let runner = h.runner(FakeAgent::new(Script::new()));
    assert!(runner.say(None, "   ").await.is_err());
    // 空消息连班次都不该开——「一句空话」不构成一次值班。
    assert!(h
        .store
        .list_foreman_sessions(Some(FOREMAN_SESSION_KIND_TALK), false)
        .await
        .unwrap()
        .is_empty());

    let sid = h.session().await;
    assert!(runner.say(Some(&sid), "   ").await.is_err());
    assert!(h
        .store
        .list_foreman_messages(&sid, 10, None)
        .await
        .unwrap()
        .is_empty());
}

// ─────────────────────────── 只读工具（票 02）───────────────────────────

#[tokio::test]
async fn read_task_tool_actually_reads_the_ledger_and_feeds_the_reply() {
    // 本仓的替换边界（决策 148）：只替换 LLM 响应流，**工具层全部真实执行**。
    // 所以这里的断言是「它真读到了 SQL 里的东西」，不是「脚本演了一段话」。
    let h = Harness::seeded().await;
    park_task(
        &h.store,
        "t1",
        PendingKind::UserDecision,
        "冲突了两条路，你挑一条",
    )
    .await;

    let mut script = Script::new();
    script.for_foreman().read_task("t1");
    script
        .for_foreman()
        .text("t1 停在冲突上，后端给的理由是「冲突了两条路，你挑一条」。");
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());

    let turn = runner.say(None, "t1 怎么了？").await.unwrap();
    assert_eq!(turn.traces.len(), 1);
    assert_eq!(turn.traces[0].tool, "read_task");
    assert!(turn.traces[0].ok, "工具应执行成功");
    assert!(turn.traces[0].args_summary.contains("t1"));
    assert!(turn.reply.contains("冲突了两条路"));

    // 工具结果**真的回灌进了对话**：第二次请求的 messages 里能读到台账正文
    // （「冲突了两条路」只在 SQL 里，脚本没写过它）。
    let requests = agent.request_log();
    assert_eq!(requests.len(), 2, "一次工具往返 = 两次 LLM 请求");
    let second = serde_json::to_string(&requests[1].messages).unwrap();
    assert!(
        second.contains("冲突了两条路"),
        "工具返回的台账内容应回灌进第二轮 messages"
    );

    // 痕迹落库（票 05）。
    let messages = h
        .store
        .list_foreman_messages(&turn.session.id, 100, None)
        .await
        .unwrap();
    let assistant = messages.iter().find(|m| m.role == "assistant").unwrap();
    let traces = assistant.traces_json.as_ref().expect("痕迹应落库");
    assert_eq!(traces[0]["tool"], "read_task");
    assert_eq!(traces[0]["ok"], true);
}

// ─────────────────────── 值守轮（决策 209④ / 票 06）───────────────────────

/// 造一条待办（真库，不伪造）。
async fn note(h: &Harness, task_id: &str, kind: agentpipeline_core::storage::AttentionKind) {
    h.store
        .note_attention(
            task_id,
            kind,
            h.clock.now(),
            Some(&serde_json::json!({"message": "测试事件"})),
        )
        .await
        .unwrap();
}

/// 决策 287 / 票 02：全局开关关掉的是「跑」——有待办也不醒、不消费、不花钱；
/// 打开后下一趟立即恢复。这道门在 `watch()` 的最前面，故待办表原封不动。
#[tokio::test]
async fn the_global_switch_stops_the_watch_round_and_backlog_stays_put() {
    let h = Harness::seeded().await;
    note(
        &h,
        "t1",
        agentpipeline_core::storage::AttentionKind::RetryExhausted,
    )
    .await;
    h.clock.advance_secs(61);
    h.store.set_foreman_watch_enabled(false).await.unwrap();

    let mut script = Script::new();
    script
        .for_foreman()
        .text("t1 重试耗尽了，需要值班经理看一眼。");
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());

    assert!(runner.watch().await.unwrap().is_none(), "关掉：这一趟不醒");
    assert_eq!(agent.total_calls(), 0, "关掉 = 零次模型调用，一分钱不花");
    assert_eq!(
        h.store.open_attention(100).await.unwrap().len(),
        1,
        "待办不消费：打开后这一批还在"
    );

    h.store.set_foreman_watch_enabled(true).await.unwrap();
    let turn = runner.watch().await.unwrap().expect("打开后下一趟立即恢复");
    assert!(turn.reply.contains("重试耗尽"));
    assert!(
        h.store.open_attention(100).await.unwrap().is_empty(),
        "照常消费"
    );
}

/// 决策 289 / 票 03：(a) 人的那一轮读到**值守摘要**（带标记的 user 轮，且不是台账原文）；
/// 摘要调用的留痕按**源会话**（值守台账）归属。
#[tokio::test]
async fn the_human_turn_sees_a_marked_watch_digest_not_the_verbatim_ledger() {
    let h = Harness::empty().await;
    // 先有一轮真对话：有正文才有「正文之后」可言（决策 299 的位次判据要钉住）。
    let sid = h.session().await;
    h.store
        .append_foreman_user_message(&sid, "上一句")
        .await
        .unwrap();
    h.store
        .append_foreman_message(NewForemanMessage::assistant(&sid, "上一轮的回答"))
        .await
        .unwrap();
    // 值守台账先醒过两次（直接落库模拟既有播报）。
    let watch_sid = h
        .store
        .create_foreman_session_of_kind(FOREMAN_SESSION_KIND_WATCH, FOREMAN_WATCH_SESSION_TITLE)
        .await
        .unwrap()
        .id;
    for content in [
        format!("{FOREMAN_WATCH_MARK}t1 重试耗尽了，建议重跑。"),
        format!("{FOREMAN_WATCH_MARK}t2 的闸门挂了三次，需要人看。"),
    ] {
        h.store
            .append_foreman_message(NewForemanMessage::assistant(&watch_sid, content))
            .await
            .unwrap();
    }

    let mut script = Script::new();
    script
        .for_foreman()
        .text("值守摘要：t1 重试耗尽、t2 闸门连挂。") // 第一次调用 = 互喂摘要器
        .text("这两件我都看过了。"); // 第二次 = 主轮
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());
    let turn = runner
        .say(Some(&sid), "刚才播报的两件事怎么办")
        .await
        .unwrap();

    let reqs = agent.request_log();
    assert_eq!(reqs.len(), 2, "摘要一次 + 主轮一次：{}", reqs.len());
    // 摘要器：无工具、留痕归属**值守台账**（决策 231 的口径：读的是那本台账）。
    assert!(reqs[0].tools.is_empty(), "摘要器不该带工具");
    assert_eq!(
        reqs[0].run.as_ref().map(|r| r.session_id.as_str()),
        Some(watch_sid.as_str()),
        "摘要调用的留痕按源会话归属"
    );
    assert!(
        reqs[0].user_prompt.contains("t1 重试耗尽了"),
        "台账原文是摘要输入：{}",
        reqs[0].user_prompt
    );
    // 主轮：一条带标记的 user 轮 = 标记 + 摘要正文，落在**正文之后、问题之前**
    // （位次精确判——尾部注入与头部 splice 在这条历史里恰好差两位）；**不是**台账原文。
    let (digest_pos, head) =
        find_marked(&reqs[1].messages, FOREMAN_WATCH_DIGEST_MARK).expect("带标记的值守摘要要在场");
    assert_eq!(
        digest_pos,
        reqs[1].messages.len() - 2,
        "摘要在正文之后、本轮问题之前（尾部注入）：{digest_pos}"
    );
    assert!(
        head.contains(FOREMAN_WATCH_DIGEST_MARK) && head.contains("值守摘要："),
        "要同时有标记与摘要正文：{head}"
    );
    assert!(
        !head.contains(FOREMAN_WATCH_MARK),
        "摘要是重述，不是把播报原样搬进上下文：{head}"
    );
    // 回话照旧落在人的班次。
    assert_eq!(turn.session.kind, FOREMAN_SESSION_KIND_TALK);
}

/// 决策 289 / 票 03：(b) 值守轮读到**人的对话摘要**（裁决 2：它仍读得到人说的话——
/// 以摘要形态），同样带标记、不是原文。
#[tokio::test]
async fn the_watch_round_sees_a_marked_talk_digest() {
    let h = Harness::seeded().await;
    let mut script = Script::new();
    script
        .for_foreman()
        .text("收到，我看一下。") // say 主轮
        .text("人的对话摘要：值班经理在盯 t1。") // watch 轮的互喂摘要器
        .text("t1 重试耗尽了，需要值班经理看一眼。"); // watch 主轮
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());

    runner.say(None, "t1 现在什么情况").await.unwrap();
    note(
        &h,
        "t1",
        agentpipeline_core::storage::AttentionKind::RetryExhausted,
    )
    .await;
    // 去抖窗口从**最早那件**（note 的时刻）算——先记事件再拨钟。
    h.clock.advance_secs(61);
    let turn = runner.watch().await.unwrap().expect("应当醒一次");
    assert_eq!(turn.session.kind, FOREMAN_SESSION_KIND_WATCH);

    // watch 的调用序 = 互喂摘要 + 主轮（say 那次在前面）：第三条是 watch 主轮。
    let reqs = agent.request_log();
    assert_eq!(
        reqs.len(),
        3,
        "say 主轮 + watch 摘要 + watch 主轮：{}",
        reqs.len()
    );
    let main = &reqs[2];
    let (_, head) =
        find_marked(&main.messages, FOREMAN_TALK_DIGEST_MARK).expect("带标记的人话摘要要在场");
    assert!(
        head.contains(FOREMAN_TALK_DIGEST_MARK) && head.contains("值班经理在盯 t1"),
        "值守轮的上下文里要有人话的摘要：{head}"
    );
}

/// 决策 289 / 票 03：(c) 人在跑时值守轮**排队**——不起轮、待办不消费；人一落它就照常醒
/// （与失败退避同一姿态：留给下一趟，一条不丢）。
#[tokio::test]
async fn the_watch_round_queues_while_a_human_turn_is_in_flight() {
    let h = Harness::seeded().await;
    note(
        &h,
        "t1",
        agentpipeline_core::storage::AttentionKind::RetryExhausted,
    )
    .await;
    h.clock.advance_secs(61);

    let mut script = Script::new();
    script
        .for_foreman()
        .text("t1 重试耗尽了，需要值班经理看一眼。");
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());

    // 不靠真并发（旗子是进程级的，会把并跑的其它用例一起排队）：直接把「人在跑」
    // 的现场摆出来——生产里这一步由 `respond` 的人的那一支登记。
    let human = runner.begin_human_turn();
    assert!(runner.watch().await.unwrap().is_none(), "人在跑：值守排队");
    assert_eq!(agent.total_calls(), 0, "排队不花钱");
    assert_eq!(
        h.store.open_attention(100).await.unwrap().len(),
        1,
        "待办不消费：留给下一趟"
    );
    drop(human);

    let turn = runner.watch().await.unwrap().expect("人落了：下一趟照常醒");
    assert!(turn.reply.contains("重试耗尽"));
    assert!(
        h.store.open_attention(100).await.unwrap().is_empty(),
        "照常消费"
    );
}

#[tokio::test]
async fn a_due_attention_wakes_the_foreman_exactly_once() {
    let h = Harness::seeded().await;
    note(
        &h,
        "t1",
        agentpipeline_core::storage::AttentionKind::RetryExhausted,
    )
    .await;
    // 去抖窗口过了（默认 60s）
    h.clock.advance_secs(61);

    let mut script = Script::new();
    script
        .for_foreman()
        .text("t1 重试耗尽了，需要值班经理看一眼。");
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());

    let turn = runner.watch().await.unwrap().expect("应当醒一次");
    assert_eq!(agent.total_calls(), 1, "恰好一次模型调用");
    assert!(turn.reply.contains("重试耗尽"));

    // 播报落进班次，且**带主动播报的标记**（前端靠它把两种轮分开）
    let messages = h
        .store
        .list_foreman_messages(&turn.session.id, 100, None)
        .await
        .unwrap();
    assert_eq!(messages.len(), 1, "只有播报那一行：{messages:?}");
    assert_eq!(messages[0].role, "assistant");
    assert!(
        messages[0].content.starts_with(FOREMAN_WATCH_MARK),
        "播报的开头要让人一眼看出这是主动播报：{}",
        messages[0].content
    );
    // 待办被消费（「这条我处理过没有」有了答案）
    assert!(h.store.open_attention(100).await.unwrap().is_empty());

    // 第二趟：没有待办了 → 零次模型调用
    assert!(runner.watch().await.unwrap().is_none());
    assert_eq!(agent.total_calls(), 1, "空闲时零成本");
}

/// 决策 286 / 票 01：值守轮写的话落**它自己的班次**（`kind = watch`），人的班次读不到它。
///
/// 数据分家是两条时间线的地基：播报不再混进人的时间线，值守台账也**不许被当成聊天室**
/// ——往里说话被拒、说话面的缺省落点也不落到它。
#[tokio::test]
async fn the_watch_round_writes_to_its_own_session_and_the_talk_session_never_sees_it() {
    let h = Harness::seeded().await;
    note(
        &h,
        "t1",
        agentpipeline_core::storage::AttentionKind::RetryExhausted,
    )
    .await;
    h.clock.advance_secs(61);

    // 先有人的班次与一轮对话（分家之后的对照组）。
    // 值守轮醒来时会对人的班次做一次互喂摘要（决策 289 / 票 03），故多备一步。
    let mut script = Script::new();
    script
        .for_foreman()
        .text("收到，我看一下。")
        .text("值守摘要：值班经理刚问过 t1。")
        .text("t1 重试耗尽了，需要值班经理看一眼。");
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());
    let human_turn = runner.say(None, "t1 现在什么情况").await.unwrap();
    assert_eq!(human_turn.session.kind, FOREMAN_SESSION_KIND_TALK);

    // 值守轮醒一次：播报落进值守台账。
    let watch_turn = runner.watch().await.unwrap().expect("应当醒一次");
    assert_eq!(watch_turn.session.kind, FOREMAN_SESSION_KIND_WATCH);
    assert_eq!(watch_turn.session.title, FOREMAN_WATCH_SESSION_TITLE);
    let watch_messages = h
        .store
        .list_foreman_messages(&watch_turn.session.id, 100, None)
        .await
        .unwrap();
    assert!(watch_messages
        .iter()
        .any(|m| m.content.starts_with(FOREMAN_WATCH_MARK)));

    // 人的班次一个字节没变：播报不在里面。
    let human_messages = h
        .store
        .list_foreman_messages(&human_turn.session.id, 100, None)
        .await
        .unwrap();
    assert!(
        !human_messages
            .iter()
            .any(|m| m.content.starts_with(FOREMAN_WATCH_MARK)),
        "播报不该混进人的班次：{human_messages:?}"
    );

    // 两个列表各回各的；「最近的班次」也不串（值守轮刷新过它自己的 last_active_at）。
    let talk = h
        .store
        .list_foreman_sessions(Some(FOREMAN_SESSION_KIND_TALK), false)
        .await
        .unwrap();
    let watch = h
        .store
        .list_foreman_sessions(Some(FOREMAN_SESSION_KIND_WATCH), false)
        .await
        .unwrap();
    assert_eq!(
        talk.iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
        vec![human_turn.session.id.clone()]
    );
    assert_eq!(watch.len(), 1);
    assert_eq!(watch[0].kind, FOREMAN_SESSION_KIND_WATCH);
    assert_eq!(
        h.store.latest_foreman_session().await.unwrap().unwrap().id,
        human_turn.session.id,
        "说话面的缺省落点仍是人的班次"
    );
    assert_eq!(
        h.store
            .latest_foreman_session_of_kind(FOREMAN_SESSION_KIND_WATCH)
            .await
            .unwrap()
            .unwrap()
            .id,
        watch_turn.session.id
    );

    // 值守台账是只读的一本账：往里说话被拒（不落 user 行、不开模型调用）。
    let calls_before = agent.total_calls();
    let err = runner
        .say(Some(&watch_turn.session.id), "对着播报说句话")
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Validation(_)), "{err}");
    assert_eq!(agent.total_calls(), calls_before, "被拒的说话不开模型调用");
    let watch_messages_after = h
        .store
        .list_foreman_messages(&watch_turn.session.id, 100, None)
        .await
        .unwrap();
    assert_eq!(
        watch_messages_after.len(),
        watch_messages.len(),
        "被拒的说话不落库"
    );

    // 第二趟值守：仍写同一本台账（不存在就建、存在就沿用）。
    // 换一个任务：t1 刚被消费过，还在同任务冷却里（决策 209⑤）。
    h.task("t2").await;
    note(
        &h,
        "t2",
        agentpipeline_core::storage::AttentionKind::GateFailure,
    )
    .await;
    h.clock.advance_secs(61);
    let second = runner.watch().await.unwrap().expect("应当再醒一次");
    assert_eq!(second.session.id, watch_turn.session.id);
}

#[tokio::test]
async fn several_events_are_batched_into_one_brief() {
    // 攒批**不许丢事件**：窗口内连来三件，一次唤醒里三条都在。
    let h = Harness::seeded().await;
    h.task("t2").await;
    h.task("t3").await;
    note(
        &h,
        "t1",
        agentpipeline_core::storage::AttentionKind::TaskPending,
    )
    .await;
    h.clock.advance_secs(5);
    note(
        &h,
        "t2",
        agentpipeline_core::storage::AttentionKind::GateFailure,
    )
    .await;
    h.clock.advance_secs(5);
    note(
        &h,
        "t3",
        agentpipeline_core::storage::AttentionKind::SchedulerNoEffect,
    )
    .await;
    h.clock.advance_secs(61);

    let mut script = Script::new();
    script.for_foreman().text("三件事：t1 / t2 / t3 都要看。");
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());

    let turn = runner.watch().await.unwrap().expect("应当醒一次");
    assert_eq!(agent.total_calls(), 1, "三件事只唤醒一次");
    assert_eq!(
        h.store
            .list_foreman_messages(&turn.session.id, 100, None)
            .await
            .unwrap()
            .len(),
        1
    );

    // 简报里三条都在（打在这一轮真正发给模型的 messages 上）
    let fed = serde_json::to_string(&agent.request_log()[0].messages).unwrap();
    for (task, kind) in [
        ("t1", "task_pending"),
        ("t2", "gate_failure"),
        ("t3", "scheduler_no_effect"),
    ] {
        assert!(fed.contains(task), "简报缺 {task}：{fed}");
        assert!(fed.contains(kind), "简报缺 {kind}：{fed}");
    }
    assert!(
        fed.contains("系统生成的"),
        "简报要标明来路——否则值班长会把它当成有人在对它下指令：{fed}"
    );
    assert!(h.store.open_attention(100).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_fresh_event_is_not_woken_yet() {
    // 去抖窗口内不唤醒（攒批），窗口一到才醒
    let h = Harness::seeded().await;
    note(
        &h,
        "t1",
        agentpipeline_core::storage::AttentionKind::TaskDone,
    )
    .await;
    let mut script = Script::new();
    script.for_foreman().text("播报");
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());

    assert!(
        runner.watch().await.unwrap().is_none(),
        "窗口没过，不吵醒它"
    );
    assert_eq!(agent.total_calls(), 0, "这一趟一次模型调用都没有");
    assert_eq!(
        h.store.open_attention(100).await.unwrap().len(),
        1,
        "事件还在"
    );

    h.clock.advance_secs(60);
    assert!(runner.watch().await.unwrap().is_some());
    assert_eq!(agent.total_calls(), 1);
}

#[tokio::test]
async fn no_attention_means_no_model_call() {
    // 「空闲时零成本」的牙齿：连一次 LLM 调用都不发生
    let h = Harness::seeded().await;
    let mut script = Script::new();
    script.for_foreman().text("不该发生");
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());
    assert!(runner.watch().await.unwrap().is_none());
    assert_eq!(agent.total_calls(), 0);
    let sid = h.session().await;
    assert!(h
        .store
        .list_foreman_messages(&sid, 100, None)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn a_no_action_verdict_is_recorded_silently() {
    // §2.4：诊断结论是「无需处理」→ 静默入库、**不播报**——一次自愈的风吹草动不该变成
    // 一条消息，而消息本身会挤占 24k 的历史窗口预算。
    let h = Harness::seeded().await;
    note(
        &h,
        "t1",
        agentpipeline_core::storage::AttentionKind::TaskDone,
    )
    .await;
    h.clock.advance_secs(61);

    let mut script = Script::new();
    script
        .for_foreman()
        .text("【无需处理】这一件按设计走完了。");
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());

    assert!(runner.watch().await.unwrap().is_none(), "静默：不返回播报");
    assert_eq!(agent.total_calls(), 1, "它仍然醒了一次并做了判断");
    let sid = h
        .store
        .latest_foreman_session_of_kind(FOREMAN_SESSION_KIND_WATCH)
        .await
        .unwrap()
        .unwrap()
        .id;
    assert!(
        h.store
            .list_foreman_messages(&sid, 100, None)
            .await
            .unwrap()
            .is_empty(),
        "静默 = 不落播报行"
    );
    // 但这件事**被处理过了**：不消费的话它会一夜被反复唤醒
    assert!(h.store.open_attention(100).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_failing_watch_backs_off_and_notes_the_burst_once() {
    // 决策 271：2026-09-24 实测——provider 断供 6 分钟，对讲台多了 37 行一模一样的失败账
    // （每 10 秒一条）。四条行为一起钉：退避期内零调用 / 退避到期才再试 / 台账只有一行 /
    // 恢复时一条汇总。
    let h = Harness::seeded().await;
    note(
        &h,
        "t1",
        agentpipeline_core::storage::AttentionKind::TaskPending,
    )
    .await;
    h.clock.advance_secs(61);

    let boom = || testkit::Step::Fail {
        kind: "llm_network".into(),
        message: "连不上 provider".into(),
        raw: "HTTP 请求失败".into(),
    };
    let mut script = Script::new();
    script
        .for_foreman()
        .push(boom())
        .push(boom())
        .text("t1 的待办看过了，需要你拍板。");
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());

    // 第一趟：真尝试、真失败；落一行失败账，待办不消费
    assert!(runner.watch().await.is_err());
    assert_eq!(agent.total_calls(), 1, "第一趟真的叫了模型");
    let sid = h.latest_watch_session().await;
    let after_first = h
        .store
        .list_foreman_messages(&sid, 100, None)
        .await
        .unwrap();
    assert_eq!(after_first.len(), 1, "只有那一行失败账：{after_first:?}");
    assert!(
        after_first[0]
            .content
            .starts_with(FOREMAN_WATCH_FAILED_TURN_MARK),
        "值守轮的失败账带自己的标记：{}",
        after_first[0].content
    );
    assert!(
        after_first[0].content.contains("不再逐条落账"),
        "首行要说清「为什么只看到一行」：{}",
        after_first[0].content
    );
    assert_eq!(
        h.store.open_attention(100).await.unwrap().len(),
        1,
        "失败不消费"
    );

    // 同一时刻再来一趟：退避期内**不问、不看不说话**
    assert!(runner.watch().await.unwrap().is_none());
    assert_eq!(agent.total_calls(), 1, "退避期内零模型调用");

    // 瞬时类 30s 到期 → 允许再试；仍失败，但**不再落第二行**
    h.clock.advance_secs(30);
    assert!(runner.watch().await.is_err());
    assert_eq!(agent.total_calls(), 2, "窗口到了才再试");
    assert_eq!(
        h.store
            .list_foreman_messages(&sid, 100, None)
            .await
            .unwrap()
            .len(),
        1,
        "同批同类只落一行"
    );

    // 第二次失败后窗口翻倍到 60s：差一秒都不许试
    h.clock.advance_secs(59);
    assert!(runner.watch().await.unwrap().is_none());
    assert_eq!(agent.total_calls(), 2, "翻倍窗口没到");
    h.clock.advance_secs(1);

    // 第三趟成功 → 播报 + 恢复汇总 + 待办被消费
    let turn = runner.watch().await.unwrap().expect("第三趟应当醒一次");
    assert_eq!(agent.total_calls(), 3);
    assert!(turn.reply.contains("拍板"), "{}", turn.reply);
    let after = h
        .store
        .list_foreman_messages(&sid, 100, None)
        .await
        .unwrap();
    assert_eq!(after.len(), 3, "失败账 + 播报 + 恢复汇总：{after:?}");
    assert!(
        after[1].content.starts_with(FOREMAN_WATCH_MARK),
        "第二行是播报：{}",
        after[1].content
    );
    assert!(
        after[2].content.contains("已恢复")
            && after[2].content.contains("2 次")
            && after[2].content.contains("llm_network"),
        "第三行是恢复汇总（次数与类别链都要在）：{}",
        after[2].content
    );
    assert!(
        h.store.open_attention(100).await.unwrap().is_empty(),
        "成功才消费"
    );
}

#[tokio::test]
async fn a_billing_class_failure_backs_off_longer_than_a_network_one() {
    // 决策 271 的分档：账单 / 配置类**等也不会自己好**，起步就是 5 分钟——
    // 按网络的节奏重试余额不足只是把噪声放大。
    let h = Harness::seeded().await;
    note(
        &h,
        "t1",
        agentpipeline_core::storage::AttentionKind::TaskPending,
    )
    .await;
    h.clock.advance_secs(61);
    let mut script = Script::new();
    script
        .for_foreman()
        .push(testkit::Step::Fail {
            kind: "llm_quota".into(),
            message: "provider 额度不足（余额 / 配额）".into(),
            raw: "HTTP 400：insufficient credits".into(),
        })
        .text("t1 的待办看过了。");
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());

    assert!(runner.watch().await.is_err());
    let sid = h.latest_watch_session().await;
    let first = h
        .store
        .list_foreman_messages(&sid, 100, None)
        .await
        .unwrap();
    assert!(
        first[0].content.contains("llm_quota"),
        "类别要落进台账：{}",
        first[0].content
    );

    // 瞬时类此刻（30s）就该允许了，而它是账单类——不许
    h.clock.advance_secs(30);
    assert!(runner.watch().await.unwrap().is_none());
    assert_eq!(agent.total_calls(), 1, "账单类不按网络的节奏重试");

    // 300s 到期才允许再试
    h.clock.advance_secs(270);
    let turn = runner
        .watch()
        .await
        .unwrap()
        .expect("300s 到了应当再试一次");
    assert_eq!(agent.total_calls(), 2);
    assert!(turn.reply.contains("看过了"), "{}", turn.reply);
}

#[tokio::test]
async fn the_watch_mark_is_added_once_and_silence_survives_the_model_writing_it() {
    // 决策 271：2026-09-25 实测——模型从历史里学会 `【值守播报】` 自己写了一遍，库里存成
    // 「【值守播报】【值守播报】**无需你处置。**…」；而回话以那个前缀开头时，本该静默的一轮
    // 被当成播报发了出去（静默判据被自家标记顶掉）。
    //
    // ① 模型自己写前缀：库里只留一个，回给调用方的正文是剥过的
    let h = Harness::seeded().await;
    note(
        &h,
        "t1",
        agentpipeline_core::storage::AttentionKind::TaskPending,
    )
    .await;
    h.clock.advance_secs(61);
    let mut script = Script::new();
    script
        .for_foreman()
        .text(&format!("{FOREMAN_WATCH_MARK}三条任务在跑，两条已完工。"));
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());
    let turn = runner.watch().await.unwrap().expect("应当醒一次");
    assert_eq!(
        turn.reply, "三条任务在跑，两条已完工。",
        "回给调用方的正文不带我们自己的前缀"
    );
    let sid = h.latest_watch_session().await;
    let rows = h
        .store
        .list_foreman_messages(&sid, 100, None)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].content,
        format!("{FOREMAN_WATCH_MARK}三条任务在跑，两条已完工。"),
        "前缀只加一次"
    );

    // ② 前缀 + 静默哨兵：仍然静默（不落播报行），事件照样被消费
    let h2 = Harness::seeded().await;
    note(
        &h2,
        "t1",
        agentpipeline_core::storage::AttentionKind::TaskPending,
    )
    .await;
    h2.clock.advance_secs(61);
    let mut script2 = Script::new();
    script2
        .for_foreman()
        .text(&format!("{FOREMAN_WATCH_MARK} {FOREMAN_NO_ACTION_MARK}"));
    let agent2 = FakeAgent::new(script2);
    let runner2 = h2.runner(agent2.clone());
    assert!(runner2.watch().await.unwrap().is_none(), "静默轮不返回回话");
    let sid2 = h2.latest_watch_session().await;
    assert!(
        h2.store
            .list_foreman_messages(&sid2, 100, None)
            .await
            .unwrap()
            .is_empty(),
        "静默 = 不落播报行"
    );
    assert!(
        h2.store.open_attention(100).await.unwrap().is_empty(),
        "但这件事被处理过了"
    );
}

#[tokio::test]
async fn a_failed_watch_turn_keeps_the_events_unconsumed() {
    // 一次网络抖动不该等于把这批事件丢了
    struct Boom;
    impl LlmClient for Boom {
        fn complete(
            &self,
            _request: LlmRequest,
        ) -> futures::future::BoxFuture<
            'static,
            agentpipeline_core::Result<agentpipeline_core::agent::client::AgentResponse>,
        > {
            Box::pin(async { Err(Error::Llm("模型没配".into())) })
        }
    }
    let h = Harness::seeded().await;
    note(
        &h,
        "t1",
        agentpipeline_core::storage::AttentionKind::TaskPending,
    )
    .await;
    h.clock.advance_secs(61);
    let runner = ForemanRunner::new(
        h.store.clone(),
        Settings::default(),
        h._home.home().clone(),
        Arc::new(Boom) as Arc<dyn LlmClient>,
        Arc::new(testkit::SseRecorder::new()),
    );

    assert!(runner.watch().await.is_err());
    assert_eq!(
        h.store.open_attention(100).await.unwrap().len(),
        1,
        "失败不消费：下一趟还看得见同一批"
    );
}

/// `slow_run` 这类「只播报不唤醒」的事件不该把我们叫醒（§2.1 的牙齿长在类别上）。
#[tokio::test]
async fn notice_only_events_do_not_wake_it() {
    let h = Harness::seeded().await;
    note(
        &h,
        "t1",
        agentpipeline_core::storage::AttentionKind::SlowRun,
    )
    .await;
    h.clock.advance_secs(600);
    let mut script = Script::new();
    script.for_foreman().text("不该发生");
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());
    assert!(runner.watch().await.unwrap().is_none());
    assert_eq!(agent.total_calls(), 0, "慢跑只播报不唤醒");
}

// ─────────────── 任务级托管与 D 层例外（决策 210② / 票 08）───────────────

/// 托管动作的执行替身：只记「被叫了几次」——**执行**本身由 app 层实现（走 resume 的
/// 唯一实现），core 这边要证的是「闸放行了、执行者被叫到了、账留下了」。
struct TestSteward {
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

impl agentpipeline_core::agent::tools::StewardActionRunner for TestSteward {
    fn run(
        &self,
        _call: agentpipeline_core::agent::client::ToolCall,
        _ctx: agentpipeline_core::agent::tools::ToolCallContext,
    ) -> futures::future::BoxFuture<
        'static,
        agentpipeline_core::Result<agentpipeline_core::agent::tools::ToolOutcome>,
    > {
        let calls = self.calls.clone();
        Box::pin(async move {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(agentpipeline_core::agent::tools::ToolOutcome::ok(
                "已自动放行（托管，测试替身）",
            ))
        })
    }
}

/// 把任务推成 pending（有下一次拍板可谈）。
async fn stewarded_task(h: &Harness, stewardship: Option<Stewardship>) {
    park_task(
        &h.store,
        "t1",
        PendingKind::RetryExhausted,
        "重试耗尽，等你拍板",
    )
    .await;
    if let Some(s) = stewardship {
        h.store.set_stewardship("t1", Some(&s)).await.unwrap();
    }
}

#[tokio::test]
async fn a_stewarded_resume_runs_without_a_button() {
    let h = Harness::seeded().await;
    stewarded_task(&h, Some(Stewardship::enabled_now(h.clock.now()))).await;

    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut script = Script::new();
    script.for_foreman().tool(
        "task",
        serde_json::json!({"action": "resume", "task_id": "t1", "resume_action": "continue"}),
    );
    script.for_foreman().text("已放行。");
    let runner = h.runner_with_steward(script, calls.clone());
    let turn = runner.say(None, "t1 卡住了").await.unwrap();

    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "托管放行：直接执行"
    );
    // 没有提议（人不用按键）
    assert!(h
        .store
        .list_pending_foreman_proposals(&turn.session.id)
        .await
        .unwrap()
        .is_empty());
    // 留账：会话里一条【托管】行 + 任务行上的计数
    let messages = h
        .store
        .list_foreman_messages(&turn.session.id, 100, None)
        .await
        .unwrap();
    let ledger = messages
        .iter()
        .find(|m| m.content.starts_with("【托管】"))
        .expect("每次自动动手都必须在班次里留一条账");
    assert!(ledger.content.contains("第 1/2 次"), "{}", ledger.content);
    let task = h.store.get_task("t1").await.unwrap();
    let s = task.stewardship.expect("托管状态要落库");
    assert_eq!(s.auto_resumes, 1);
    assert!(s.last_fingerprint.is_some());
}

#[tokio::test]
async fn without_stewardship_the_same_call_is_still_a_proposal() {
    let h = Harness::seeded().await;
    stewarded_task(&h, None).await;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut script = Script::new();
    script.for_foreman().tool(
        "task",
        serde_json::json!({"action": "resume", "task_id": "t1", "resume_action": "continue"}),
    );
    script.for_foreman().text("提了，等你按键。");
    let runner = h.runner_with_steward(script, calls.clone());
    let turn = runner.say(None, "t1 卡住了").await.unwrap();

    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "未托管：不动手"
    );
    let pending = h
        .store
        .list_pending_foreman_proposals(&turn.session.id)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1, "D 层照旧恒提议：{pending:?}");
    assert_eq!(pending[0].tool, "task");
    let task = h.store.get_task("t1").await.unwrap();
    assert_eq!(task.status, TaskStatus::Pending, "提议不是执行");
}

/// 人按住的任务，托管**不许替他松开**（决策 276）。
///
/// 形状与自动集里的 `resume(continue)` 同名——但那是同一个 pending 两颗出口键之一，
/// 人按住的意思正是「谁也别动它」：托管自动放行等于把一次明确的人工操作撤回去。
#[tokio::test]
async fn a_held_task_is_never_auto_released_by_stewardship() {
    let h = Harness::seeded().await;
    stewarded_task(&h, Some(Stewardship::enabled_now(h.clock.now()))).await;
    // 把待办换成「人按下的暂停」：除原因外与上一条用例逐字相同，故红的只会是这条判据。
    park_task(&h.store, "t1", PendingKind::UserPaused, "已按暂停（手动）").await;

    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut script = Script::new();
    script.for_foreman().tool(
        "task",
        serde_json::json!({"action": "resume", "task_id": "t1", "resume_action": "continue"}),
    );
    script.for_foreman().text("提了，等你按键。");
    let runner = h.runner_with_steward(script, calls.clone());
    let turn = runner.say(None, "要不要放行").await.unwrap();

    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "人按住的暂停：托管不自动放行"
    );
    let pending = h
        .store
        .list_pending_foreman_proposals(&turn.session.id)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1, "照常生成提议，按键仍是人的：{pending:?}");
    let task = h.store.get_task("t1").await.unwrap();
    assert_eq!(task.status, TaskStatus::Pending, "提议不是执行");
    assert!(
        task.stewardship.is_some_and(|s| s.auto_resumes == 0),
        "一次自动动作都不该记"
    );
}

#[tokio::test]
async fn a_stewarded_task_still_cannot_auto_retry_or_merge() {
    // 托管放开的**恰好一个动作**（决策 210②）：retry / merge / review / cancel 永不自动。
    let h = Harness::seeded().await;
    for action in [
        serde_json::json!({"action": "retry", "task_id": "t1"}),
        serde_json::json!({"action": "merge", "task_id": "t1", "decision": "approve"}),
        serde_json::json!({"action": "review", "task_id": "t1", "approved": true}),
        serde_json::json!({"action": "cancel", "task_id": "t1"}),
        // resume 但不是 continue（skip / goto 会替人重排流水线）
        serde_json::json!({"action": "resume", "task_id": "t1", "resume_action": "skip"}),
    ] {
        stewarded_task(&h, Some(Stewardship::enabled_now(h.clock.now()))).await;
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut script = Script::new();
        script.for_foreman().tool("task", action.clone());
        script.for_foreman().text("提了。");
        let runner = h.runner_with_steward(script, calls.clone());
        // 每一轮**各开一班**：`say(None, …)` 会续用最近的那个会话，提议于是会跨轮累加，
        // 「这一轮提了几条」就再也数不准了。
        let sid = h.session().await;
        let turn = runner.say(Some(&sid), "动手吧").await.unwrap();
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "托管不该放行这个动作：{action}"
        );
        assert_eq!(
            h.store
                .list_pending_foreman_proposals(&turn.session.id)
                .await
                .unwrap()
                .len(),
            1,
            "{action} 仍应是提议"
        );
    }
}

#[tokio::test]
async fn the_auto_resume_stops_at_the_cap_and_at_the_same_fingerprint() {
    let h = Harness::seeded().await;
    // ① 次数触顶：已经自动动过 2 次 → 停手，转回提议（任务仍停在 pending 等人）
    stewarded_task(
        &h,
        Some(Stewardship {
            enabled: true,
            auto_resumes: STEWARDSHIP_MAX_AUTO_RESUMES,
            last_fingerprint: Some("旧的指纹".into()),
            updated_at: Some(h.clock.now()),
        }),
    )
    .await;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut script = Script::new();
    script.for_foreman().tool(
        "task",
        serde_json::json!({"action": "resume", "task_id": "t1", "resume_action": "continue"}),
    );
    script.for_foreman().text("到上限了，等你按键。");
    let runner = h.runner_with_steward(script, calls.clone());
    let sid = h.session().await;
    let turn = runner.say(Some(&sid), "接着修").await.unwrap();
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "触顶即停手"
    );
    assert_eq!(
        h.store
            .list_pending_foreman_proposals(&turn.session.id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        h.store.get_task("t1").await.unwrap().status,
        TaskStatus::Pending,
        "停手 = 任务留在 pending 等值班经理"
    );

    // ② 同一指纹：态势没变就不重复动手（单靠次数挡不住「同一件事被反复触发」）
    let fingerprint = situation_fingerprint(&h.store, "t1")
        .await
        .unwrap()
        .to_string();
    stewarded_task(
        &h,
        Some(Stewardship {
            enabled: true,
            auto_resumes: 1,
            last_fingerprint: Some(fingerprint),
            updated_at: Some(h.clock.now()),
        }),
    )
    .await;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut script = Script::new();
    script.for_foreman().tool(
        "task",
        serde_json::json!({"action": "resume", "task_id": "t1", "resume_action": "continue"}),
    );
    script.for_foreman().text("同一个指纹，不动。");
    let runner = h.runner_with_steward(script, calls.clone());
    let sid = h.session().await;
    let turn = runner.say(Some(&sid), "接着修").await.unwrap();
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "同一指纹不重复动手"
    );
    assert_eq!(
        h.store
            .list_pending_foreman_proposals(&turn.session.id)
            .await
            .unwrap()
            .len(),
        1
    );
}

// ───────────────── 节流与分级诊断（决策 209⑤⑥ / 票 07）─────────────────

#[tokio::test]
async fn the_same_task_is_not_woken_twice_within_the_cooldown() {
    // 同任务冷却：刚处理过的任务，新事件不单独唤醒（事件仍在表里，等冷却到期合并播报）。
    let h = Harness::seeded().await;
    note(
        &h,
        "t1",
        agentpipeline_core::storage::AttentionKind::TaskPending,
    )
    .await;
    h.clock.advance_secs(61);

    let mut script = Script::new();
    script.for_foreman().text("第一轮播报");
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());
    assert!(runner.watch().await.unwrap().is_some());
    assert_eq!(agent.total_calls(), 1);

    // 冷却期内又来一条同任务事件
    note(
        &h,
        "t1",
        agentpipeline_core::storage::AttentionKind::TaskPending,
    )
    .await;
    h.clock.advance_secs(61);
    assert!(
        runner.watch().await.unwrap().is_none(),
        "冷却期内不再单独唤醒"
    );
    assert_eq!(agent.total_calls(), 1, "没有第二次模型调用");
    assert_eq!(
        h.store.open_attention(100).await.unwrap().len(),
        1,
        "事件留着"
    );

    // 冷却到期（默认 30 分钟）后合并播报
    h.clock.advance_secs(30 * 60);
    let mut second = Script::new();
    second.for_foreman().text("第二轮播报");
    agent.set_script(second);
    assert!(runner.watch().await.unwrap().is_some());
    assert_eq!(agent.total_calls(), 2);
    assert!(h.store.open_attention(100).await.unwrap().is_empty());
}

#[tokio::test]
async fn hitting_the_hourly_cap_reports_instead_of_dropping_silently() {
    // 触顶**不是**让值班长闭嘴：留一行「本小时已达上限，N 条待办未播报」，待办不消费。
    let settings = Settings {
        watch_max_wakes_per_hour: 1,
        ..Default::default()
    };
    let h = Harness::seeded().await;
    h.task("t2").await;
    note(
        &h,
        "t1",
        agentpipeline_core::storage::AttentionKind::TaskPending,
    )
    .await;
    h.clock.advance_secs(61);

    let mut script = Script::new();
    script.for_foreman().text("第一轮播报");
    let agent = FakeAgent::new(script);
    let runner = h.runner_with(settings, agent.clone());
    assert!(runner.watch().await.unwrap().is_some());
    assert_eq!(agent.total_calls(), 1);

    // 第二件（另一个任务，绕开同任务冷却）
    note(
        &h,
        "t2",
        agentpipeline_core::storage::AttentionKind::TaskPending,
    )
    .await;
    h.clock.advance_secs(61);
    assert!(runner.watch().await.unwrap().is_none(), "触顶：这一次不醒");
    assert_eq!(agent.total_calls(), 1, "触顶不花钱");

    let messages = h
        .store
        .list_foreman_messages(&h.latest_watch_session().await, 100, None)
        .await
        .unwrap();
    let note_row = messages
        .iter()
        .find(|m| m.role == "system" && m.content.contains("已达上限"))
        .expect("触顶要留一行给值班经理，不能静默丢弃");
    assert!(
        note_row.content.contains("1 条待办未播报"),
        "{}",
        note_row.content
    );
    assert_eq!(
        h.store.open_attention(100).await.unwrap().len(),
        1,
        "未播报的待办**不消费**：下一小时继续"
    );

    // 同一小时内不重复刷这条提示
    h.clock.advance_secs(61);
    assert!(runner.watch().await.unwrap().is_none());
    let again = h
        .store
        .list_foreman_messages(&h.latest_watch_session().await, 100, None)
        .await
        .unwrap()
        .iter()
        .filter(|m| m.content.contains("已达上限"))
        .count();
    assert_eq!(again, 1, "触顶提示一小时只留一条");
}

#[tokio::test]
async fn the_automatic_turn_cannot_reach_the_expensive_tools() {
    // 分级诊断（票 07）：自动那一轮的工具集里**没有** read_conversation 与 run_command；
    // 被追问（人的那一轮）时同一份名单里**有**它们。
    let h = Harness::seeded().await;
    note(
        &h,
        "t1",
        agentpipeline_core::storage::AttentionKind::TaskPending,
    )
    .await;
    h.clock.advance_secs(61);

    let mut script = Script::new();
    script.for_foreman().text("播报");
    script.for_foreman().text("回话");
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());
    runner.watch().await.unwrap();

    let watch_tools: Vec<String> = agent.request_log()[0]
        .tools
        .iter()
        .map(|t| t.name.clone())
        .collect();
    // 决策 265 / 266：值守轮既不问人（`ask`）也不外发（`web_fetch`）——
    // 「这一次你不在场」对提问与网络同样成立。
    for forbidden in ["read_conversation", "run_command", "ask", "web_fetch"] {
        assert!(
            !watch_tools.contains(&forbidden.to_string()),
            "自动轮不该拿到 {forbidden}：{watch_tools:?}"
        );
    }
    assert!(
        watch_tools.contains(&"read_diagnosis".to_string()),
        "自动轮仍要看得到诊断包（它是台账类）：{watch_tools:?}"
    );
    // 决策 232 / 237：自主轮拿得到**只读取证**的手——正是这一份白名单命令
    // （`sample` / `pgrep` / `lsof`）让「夜里自己发现并定死」在工具面上成立；
    // 而写的那只手（`run_command`）照旧被挡着。
    assert!(
        watch_tools.contains(&"run_readonly".to_string()),
        "自动轮要拿得到只读取证：{watch_tools:?}"
    );

    runner.say(None, "t1 怎么了？").await.unwrap();
    let human_tools: Vec<String> = agent
        .request_log()
        .last()
        .expect("人的那一轮也调了模型")
        .tools
        .iter()
        .map(|t| t.name.clone())
        .collect();
    for wanted in ["read_conversation", "run_command", "ask", "web_fetch"] {
        assert!(
            human_tools.contains(&wanted.to_string()),
            "被追问时该拿得到 {wanted}：{human_tools:?}"
        );
    }
}

// ───────────────── 结构化选项提问（决策 265，票 foreman-capability-gaps 01） ─────────────────

#[tokio::test]
async fn an_ask_lands_on_its_row_with_options_and_never_becomes_a_proposal() {
    let h = Harness::seeded().await;
    let mut script = Script::new();
    script.for_foreman().tool(
        "ask",
        serde_json::json!({
            "question": "这张票怎么处理？",
            "options": ["修一下", "重开", "搁置"]
        }),
    );
    script.for_foreman().text("等你选。");
    let runner = h.runner(FakeAgent::new(script));
    let turn = runner.say(None, "拿个主意").await.unwrap();

    let messages = h
        .store
        .list_foreman_messages(&turn.session.id, 100, None)
        .await
        .unwrap();
    let ask_row = messages
        .iter()
        .find(|m| m.ask_json.is_some())
        .expect("问了就要落库——刷新后选项钮靠这一行重建");
    assert_eq!(
        ask_row.ask_json.as_ref().unwrap(),
        &serde_json::json!({"question": "这张票怎么处理？", "options": ["修一下", "重开", "搁置"]}),
        "载荷按 canonical 形状落（问句 + 选项数组）"
    );
    assert_eq!(
        ask_row.content, "等你选。",
        "回话照常落，问题只是多带了一份结构"
    );
    assert_eq!(
        ask_row.role,
        agentpipeline_core::storage::foreman::FOREMAN_ROLE_ASSISTANT
    );
    // 问话永不进提议通道：它不是打算执行的动作，人点选项走的是「下一条 user 消息」。
    assert!(
        h.store
            .list_pending_foreman_proposals(&turn.session.id)
            .await
            .unwrap()
            .is_empty(),
        "ask 不该生成提议"
    );
}

#[tokio::test]
async fn only_the_first_ask_of_a_round_lands() {
    let h = Harness::seeded().await;
    let mut script = Script::new();
    script.for_foreman().tool(
        "ask",
        serde_json::json!({"question": "第一个？", "options": ["甲", "乙"]}),
    );
    script.for_foreman().tool(
        "ask",
        serde_json::json!({"question": "第二个？", "options": ["丙", "丁"]}),
    );
    script.for_foreman().text("收口。");
    let runner = h.runner(FakeAgent::new(script));
    let turn = runner.say(None, "问吧").await.unwrap();

    let messages = h
        .store
        .list_foreman_messages(&turn.session.id, 100, None)
        .await
        .unwrap();
    let asks: Vec<_> = messages.iter().filter(|m| m.ask_json.is_some()).collect();
    assert_eq!(asks.len(), 1, "一轮只许问一个问题：{asks:?}");
    assert_eq!(asks[0].ask_json.as_ref().unwrap()["question"], "第一个？");

    // 第二次调用在执行点被拒：错误回给模型（轮次照常收口），痕迹里记一笔 ok=false。
    let fm_row = messages
        .iter()
        .rev()
        .find(|m| m.role == agentpipeline_core::storage::foreman::FOREMAN_ROLE_ASSISTANT)
        .expect("回话行在场");
    let traces_raw = fm_row.traces_json.clone().expect("这一轮跑过工具");
    let traces: Vec<agentpipeline_core::pipeline::foreman::ForemanTrace> =
        serde_json::from_value(traces_raw).unwrap();
    let ask_traces: Vec<_> = traces.iter().filter(|t| t.tool == "ask").collect();
    assert_eq!(ask_traces.len(), 2, "两次调用都要留痕：{ask_traces:?}");
    assert!(ask_traces[0].ok, "第一个落库成功");
    assert!(!ask_traces[1].ok, "第二个被执行点拒掉");
}

#[tokio::test]
async fn a_malformed_ask_is_refused_to_the_model_and_lands_nothing() {
    let h = Harness::seeded().await;
    let mut script = Script::new();
    // 只有一个选项：schema 说 2–4，模型偶尔会违反——校验在执行点，不信任 schema。
    script.for_foreman().tool(
        "ask",
        serde_json::json!({"question": "只有一个选项？", "options": ["甲"]}),
    );
    script.for_foreman().text("算了，先这样。");
    let runner = h.runner(FakeAgent::new(script));
    let turn = runner.say(None, "问个坏问题").await.unwrap();

    let messages = h
        .store
        .list_foreman_messages(&turn.session.id, 100, None)
        .await
        .unwrap();
    assert!(
        messages.iter().all(|m| m.ask_json.is_none()),
        "坏载荷不落库——落了就是给界面一个渲染不了的半成品"
    );
    let fm_row = messages
        .iter()
        .rev()
        .find(|m| m.role == agentpipeline_core::storage::foreman::FOREMAN_ROLE_ASSISTANT)
        .expect("回话行在场");
    assert_eq!(fm_row.content, "算了，先这样。", "工具失败不毁掉这一轮");
    let traces: Vec<agentpipeline_core::pipeline::foreman::ForemanTrace> =
        serde_json::from_value(fm_row.traces_json.clone().unwrap()).unwrap();
    let ask_trace = traces.iter().find(|t| t.tool == "ask").expect("调用要留痕");
    assert!(!ask_trace.ok, "坏载荷对模型是失败");
}

#[tokio::test]
async fn the_watch_cost_is_accounted_separately_from_human_turns() {
    // 「这周值守花了多少」：醒过的次数与 token 在一张**单独的**账上，且静默那一轮也在
    // （它花了钱，只是没说话）。
    let h = Harness::seeded().await;
    note(
        &h,
        "t1",
        agentpipeline_core::storage::AttentionKind::TaskPending,
    )
    .await;
    h.clock.advance_secs(61);
    let mut script = Script::new();
    script.for_foreman().text("播报");
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());
    assert!(runner.watch().await.unwrap().is_some());

    // 人的一轮不该进这张账
    runner.say(None, "在吗").await.unwrap();

    let since = h.clock.now() - chrono::Duration::days(7);
    let (wakes, prompt, completion) = h.store.watch_cost_since(since).await.unwrap();
    assert_eq!(wakes, 1, "值守这边只记了自动那一轮");
    assert!(
        prompt > 0 && completion > 0,
        "token 也要记：{prompt}/{completion}"
    );

    // 静默那一轮同样入账（花了钱没说话）
    h.task("t2").await;
    note(
        &h,
        "t2",
        agentpipeline_core::storage::AttentionKind::TaskPending,
    )
    .await;
    h.clock.advance_secs(61);
    let mut silent = Script::new();
    // 第二次 watch 会对人的班次做一次互喂摘要（say 那句「在吗」是新增量）——故多备一步。
    silent
        .for_foreman()
        .text("人的对话摘要：值班经理打了个招呼。")
        .text("【无需处理】");
    agent.set_script(silent);
    assert!(runner.watch().await.unwrap().is_none());
    let (wakes, _, _) = h.store.watch_cost_since(since).await.unwrap();
    assert_eq!(wakes, 2, "静默轮也花了钱，必须入账");
}

// ─────────────────────── 诊断包（决策 211③ / 票 03）───────────────────────

/// 造一个「失败得能定因」的任务：一条失败 run（带 error）、一条命令台账、一份闸门输出。
///
/// 四项证据**必须来自真库 / 真文件**——本仓的替换边界是「只替换 LLM 响应流」，
/// 诊断包这条链的验证要打在「一次调用够不够定因」上。
async fn seed_failed_task(h: &Harness, task_id: &str) -> i64 {
    let cursor_id = h.store.load_live_cursors(task_id).await.unwrap()[0]
        .cursor_id
        .clone();
    // 失败的那一轮：error 是定因的入口，process_group_id 为空是「超时杀不掉」的唯一线索
    let run_id = h
        .store
        .insert_run(&agentpipeline_core::storage::observability::NewRun {
            task_id: task_id.into(),
            cursor_id: cursor_id.clone(),
            stage: Stage::Test,
            node: Node::Execute,
            attempt: 1,
            agent_type: "main".into(),
            parent_run_id: None,
            prompt_template_hash: Some("deadbeefdeadbeef".into()),
            process_group_id: None,
        })
        .await
        .unwrap();
    h.store
        .finish_run(
            run_id,
            &agentpipeline_core::storage::observability::RunOutcome {
                status: Some(agentpipeline_core::types::NodeStatus::Failed),
                error: Some("闸门失败：测试命令退出码 1".into()),
                duration_ms: 4321,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    // 会话行带两段 prompt 原文（票 02）——「这是 prompt 问题」的证据
    h.store
        .insert_conversation(
            task_id,
            run_id,
            Stage::Test,
            Node::Execute,
            1,
            "main",
            None,
            &serde_json::json!([
                {"role": "assistant", "tool_calls": [{"id": "c1", "name": "run_command",
                  "arguments": "{\"command\":\"cargo test --quiet\"}"}]},
                {"role": "tool", "completion": "exit code 1: assertion failed at src/lib.rs:12"}
            ]),
            Some(agentpipeline_core::storage::observability::PromptSnapshot {
                system: "你是测试工位。",
                user: "跑一遍闸门，把失败原文带回来。",
            }),
            Some(&serde_json::json!({"failed": true, "error": "闸门失败：测试命令退出码 1"})),
            900,
            120,
            None,
        )
        .await
        .unwrap();
    // 命令台账 + 闸门输出的**真文件**（路径与 executor 落的那一份同源）
    let gate_log = h._home.home().task_file(
        task_id,
        &format!("gate-output-{}.log", Stage::Test.as_str()),
    );
    h._home.home().ensure_task_dirs(task_id).unwrap();
    std::fs::write(&gate_log, "[stdout]\nassertion failed at src/lib.rs:12\n").unwrap();
    record_command(h, task_id, Some(run_id), "cargo test --quiet").await;
    run_id
}

/// 落一条命令台账（走既有 `record_start`，不手写 SQL）。
async fn record_command(h: &Harness, task_id: &str, run_id: Option<i64>, command: &str) {
    use agentpipeline_core::agent::tools::{CommandRecorder, CommandStart};

    h.store
        .record_start(CommandStart {
            task_id: Some(task_id.to_string()),
            session_id: None,
            run_id,
            stage: Stage::Test,
            node: Node::Execute,
            source: agentpipeline_core::types::CommandSource::System,
            command: command.to_string(),
            cwd: h._home.path().display().to_string(),
            original_command: None,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn one_diagnosis_call_answers_why_it_stalled() {
    // 票 03 的口径：验证打的是「一次调用够不够定因」，不是「字段齐不齐」。
    let h = Harness::seeded().await;
    let run_id = seed_failed_task(&h, "t1").await;
    park_task(
        &h.store,
        "t1",
        PendingKind::RetryExhausted,
        "测试工位重试耗尽",
    )
    .await;

    let mut script = Script::new();
    script.for_foreman().read_diagnosis("t1");
    script
        .for_foreman()
        .text("它在 test.execute 上闸门没过：测试命令退出码 1，看 gate-output-test.log。");
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());

    let turn = runner.say(None, "t1 为什么卡住了？").await.unwrap();
    assert_eq!(turn.traces.len(), 1);
    assert_eq!(turn.traces[0].tool, "read_diagnosis");
    assert!(turn.traces[0].ok, "诊断包应执行成功");

    // 一次调用的回执里，四类证据同时在场（打在同一份文本上）
    let requests = agent.request_log();
    assert_eq!(requests.len(), 2, "一次工具往返 = 两次 LLM 请求");
    // 断言打在**模型读到的正文**上，不是 `serde_json::to_string` 之后那一份：
    // 序列化会把正文里的引号转义成 `\"`，于是 `"run_id":7` 这种带引号的证据永远搜不到
    // （曾经就是这么红的一条：正文里有，转义后匹配不上）。
    let fed_back = requests[1]
        .messages
        .iter()
        .filter_map(|m| m.content.as_deref())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        fed_back.contains("闸门失败：测试命令退出码 1"),
        "失败 run 的 error 要在同一次调用里：{fed_back}"
    );
    assert!(
        fed_back.contains(&format!("\"run_id\":{run_id}"))
            || fed_back.contains(&format!("\"run_id\": {run_id}")),
        "证据要指名是哪一条 run"
    );
    assert!(
        fed_back.contains("has_process_group"),
        "「超时杀不杀得掉」的唯一线索要给出（run 行上有 process_group_id 时为 true）：{fed_back}"
    );
    assert!(
        fed_back.contains("gate-output-test.log"),
        "闸门输出的路径要给出：{fed_back}"
    );
    assert!(
        fed_back.contains("cargo test --quiet"),
        "命令台账要在同一次调用里：{fed_back}"
    );
    assert!(
        fed_back.contains("测试工位重试耗尽"),
        "pending 原因的原文要在同一次调用里：{fed_back}"
    );
    // 两段 prompt 原文（票 02）——「是 prompt 问题」这句话的根据
    assert!(
        fed_back.contains("跑一遍闸门，把失败原文带回来。"),
        "组装后的用户段原文要带出来：{fed_back}"
    );
}

#[tokio::test]
async fn the_diagnosis_pack_carries_the_model_requests_of_each_run() {
    // 决策 231 的硬要求：这张表的读数**必须进 `read_diagnosis`**——不进就是白做
    // （值班长看不见的表等于不存在）。判据是决策 230 的前两项在**一次调用**里答得上：
    // ① 哪一个 run（`run_id` + 序号落在同一行）② 卡在哪一环（这一次调用 / 还在飞）。
    let h = Harness::seeded().await;
    let run_id = seed_failed_task(&h, "t1").await;
    let now = h.store.now();

    // 一条收场的请求（带量速读数） + 一条仍在飞的请求（此刻的现状）。
    let settled = h
        .store
        .begin_model_request(&NewModelRequest {
            run_id: Some(run_id),
            session_id: None,
            task_id: Some("t1".into()),
            agent_type: "main".into(),
            stage: "test".into(),
            node: "execute".into(),
            attempt: 1,
        })
        .await
        .unwrap();
    h.store
        .finish_model_request(
            settled,
            ModelRequestStatus::Ok,
            &ModelRequestUsage {
                prompt_tokens: Some(3_772_456),
                completion_tokens: Some(812),
                bytes_received: Some(4_096),
                last_byte_at: Some(now),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    h.store
        .begin_model_request(&NewModelRequest {
            run_id: Some(run_id),
            session_id: None,
            task_id: Some("t1".into()),
            agent_type: "main".into(),
            stage: "test".into(),
            node: "execute".into(),
            attempt: 2,
        })
        .await
        .unwrap();
    park_task(&h.store, "t1", PendingKind::RetryExhausted, "重试耗尽").await;

    let mut script = Script::new();
    script.for_foreman().read_diagnosis("t1");
    script.for_foreman().text("第 2 次调用还挂在流上。");
    let agent = FakeAgent::new(script);
    let turn = h
        .runner(agent.clone())
        .say(None, "t1 卡在哪一次调用？")
        .await
        .unwrap();
    assert!(turn.traces[0].ok);

    let fed_back = agent.request_log()[1]
        .messages
        .iter()
        .filter_map(|m| m.content.as_deref())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        fed_back.contains("model_requests"),
        "诊断包要有模型请求那一节：{fed_back}"
    );
    // ① 归位：这一行指名它属于哪一条 run（2026-09-19 实测里正是这一条缺了，才把 run 27
    //    的活栈记到 run 26 名下）。
    assert!(
        fed_back.contains(&format!("\"run_id\": {run_id}")),
        "请求要归位到它真正所属的 run：{fed_back}"
    );
    assert!(fed_back.contains("\"seq\": 1"), "序号要说清是第几次调用");
    // ② 量速：收字节总量与最后一次收字节的时刻。
    assert!(
        fed_back.contains("\"bytes_received\": 4096"),
        "收字节总量要在场：{fed_back}"
    );
    assert!(
        fed_back.contains("\"prompt_tokens\": 3772456"),
        "这一次调用烧掉的用量要在场：{fed_back}"
    );
    // ③ 现状：`finished_at IS NULL` 的那一条就是「现在在飞什么」。
    assert!(
        fed_back.contains("\"in_flight\": true"),
        "在飞的请求要能被认出来：{fed_back}"
    );
}

#[tokio::test]
async fn the_diagnosis_pack_keeps_the_reason_when_truncated() {
    // 一次调用给的证据可能超过 12k：截断**必须**发生在「为什么卡住」之后——
    // 否则最要紧的那一屏会被长 tail 挤掉。
    let h = Harness::seeded().await;
    let run_id = seed_failed_task(&h, "t1").await;
    // 用一堆超长命令台账把总量顶过 12k（每条的 command 都是长串）
    for i in 0..40 {
        record_command(
            &h,
            "t1",
            Some(run_id),
            &format!("echo {}{}", "x".repeat(600), i),
        )
        .await;
    }
    park_task(&h.store, "t1", PendingKind::RetryExhausted, "重试耗尽").await;

    let mut script = Script::new();
    script.for_foreman().read_diagnosis("t1");
    script.for_foreman().text("看诊断包。");
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());
    runner.say(None, "t1 怎么了？").await.unwrap();

    let requests = agent.request_log();
    let fed_back = serde_json::to_string(&requests[1].messages).unwrap();
    // 人的那一轮**不预截**（决策 291 / 票 06(a)）：包超过卸载阈值就落盘，回灌的是
    // 「回执 + 预览 + 路径」。顺序那条牙齿因此改挂到**预览**上——被切掉的必须是长尾，
    // 不是「为什么卡住」那一屏（原话见 `read_diagnosis` 的分节顺序注释）。
    assert!(
        fed_back.contains("已卸载") && fed_back.contains("foreman/context/"),
        "人的那一轮超过阈值要走 L2 卸载：{fed_back}"
    );
    assert!(
        !fed_back.contains("已截断"),
        "人的那一轮不预截：体量归卸载管，不归 12k 截断管"
    );
    assert!(
        fed_back.contains("闸门失败：测试命令退出码 1"),
        "失败原因是重点证据，必须在卸载回执的预览里：{}",
        &fed_back[..fed_back.len().min(400)]
    );
    assert!(
        fed_back.contains("重试耗尽"),
        "pending 原因必须在卸载回执的预览里"
    );
}

/// 值守轮**不放开**（裁决 4 / 票 06(a)）：`read_diagnosis` 的 12k 是「只读台账与诊断包
/// **摘要**」那条分级纪律的一部分（决策 265 / 266），本次一字不改。
///
/// 故同一个包、同一个工具，两轮的收口不同：人的那一轮走卸载 + 逐字，值守轮走 12k 预截。
#[tokio::test]
async fn the_watch_round_keeps_the_12k_truncation_on_ledger_reads() {
    let h = Harness::seeded().await;
    let run_id = seed_failed_task(&h, "t1").await;
    for i in 0..40 {
        record_command(
            &h,
            "t1",
            Some(run_id),
            &format!("echo {}{}", "x".repeat(600), i),
        )
        .await;
    }
    park_task(&h.store, "t1", PendingKind::RetryExhausted, "重试耗尽").await;

    let outcome = ledger_tool(
        &h,
        "s1",
        false,
        "read_diagnosis",
        serde_json::json!({"task_id": "t1"}),
    )
    .await;
    assert!(
        outcome.contains("已截断"),
        "值守轮超过 12k 要留截断标记，不许静默截短"
    );
    assert!(
        !outcome.contains("已卸载"),
        "值守轮不走卸载（诊断包只读摘要形态）：{outcome}"
    );
    assert!(
        outcome.contains("闸门失败：测试命令退出码 1") && outcome.contains("重试耗尽"),
        "截断不许吃掉最要紧的那一屏"
    );
}

// ───────────── 窗口界（决策 291 / 票 06）：卸载放开 · 轮内压缩 · 撞墙恢复 ─────────────

/// 直接拿工头的工具集跑一次台账工具（票 06：两轮的收口差异在**执行器**上，
/// 故不必绕一整轮对话来验它）。
async fn ledger_tool(
    h: &Harness,
    session_id: &str,
    ledger_unbounded: bool,
    tool: &str,
    args: serde_json::Value,
) -> String {
    use agentpipeline_core::agent::client::ToolCall;
    use agentpipeline_core::pipeline::foreman::{
        foreman_available_tools_except, foreman_tooling, ForemanMoment,
    };

    let env_mode = agentpipeline_core::types::EnvMode::Ask;
    let available = foreman_available_tools_except(env_mode, &[]);
    let (tools, ctx) = foreman_tooling(
        &h.store,
        &Settings::default(),
        h._home.home(),
        Arc::new(testkit::SseRecorder::new()),
        session_id,
        env_mode,
        ForemanMoment::Conversation,
        &available,
        None,
        ledger_unbounded,
    );
    let call = ToolCall {
        id: "call-1".into(),
        name: tool.into(),
        arguments: args.to_string(),
    };
    tools.execute(&call, &ctx).await.unwrap().content
}

/// 卸载回执里的路径（`完整内容：{path}（{n} token）` 那一段）。
fn offloaded_path(receipt: &str) -> String {
    receipt
        .lines()
        .find_map(|l| l.strip_prefix("完整内容："))
        .and_then(|rest| rest.split('（').next())
        .expect("回执里应当有「完整内容：<路径>」那一行")
        .to_string()
}

/// 人的那一轮读台账：**大结果真卸载，且可按路径回读全文**（票 06(a) 的验收面）。
///
/// 「可按路径回读」是这条改动的全部意义——12k 预截时模型丢了那半截就真没了（没有路径
/// 可回读），卸载之后它丢的是**可再取回**的那半截。故用例必须把回读那一步也走一遍，
/// 而不是只断言回执里有路径。
#[tokio::test]
async fn a_human_turn_offloads_ledger_reads_and_can_read_them_back() {
    use agentpipeline_core::agent::client::ToolCall;

    let h = Harness::seeded().await;
    let sid = h.session().await;
    let run_id = seed_failed_task(&h, "t1").await;
    // 40 条超长命令台账把包顶过卸载阈值（4000 token ≈ 16000 字符）
    for i in 0..40 {
        record_command(
            &h,
            "t1",
            Some(run_id),
            &format!("echo {}{}", "x".repeat(600), i),
        )
        .await;
    }
    park_task(&h.store, "t1", PendingKind::RetryExhausted, "重试耗尽").await;

    let receipt = ledger_tool(
        &h,
        &sid,
        true,
        "read_diagnosis",
        serde_json::json!({"task_id": "t1"}),
    )
    .await;
    assert!(
        receipt.contains("已卸载"),
        "人的那一轮大结果要卸载：{receipt}"
    );
    let path = offloaded_path(&receipt);

    // 回读：同一个执行器的 `read_file`（分段读中段——200 行的默认读法看不到命令台账中段）。
    let env_mode = agentpipeline_core::types::EnvMode::Ask;
    let available =
        agentpipeline_core::pipeline::foreman::foreman_available_tools_except(env_mode, &[]);
    let (tools, ctx) = agentpipeline_core::pipeline::foreman::foreman_tooling(
        &h.store,
        &Settings::default(),
        h._home.home(),
        Arc::new(testkit::SseRecorder::new()),
        &sid,
        env_mode,
        agentpipeline_core::pipeline::foreman::ForemanMoment::Conversation,
        &available,
        None,
        true,
    );
    // 中段那一条（第 20 条命令）只在盘上：预览是头 30 + 尾 30 行，够不着它。
    let middle_marker = format!("echo {}{}", "x".repeat(600), 20);
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(on_disk.contains(&middle_marker), "全文应当落在盘上");
    assert!(
        !receipt.contains(&middle_marker),
        "中段不该出现在回执预览里（否则这条用例证明不了「回读拿到了更多」）"
    );
    // 回读按行定位、只取窄窄一段：整份读回来又会超阈值、再被卸载一次（这本身是对的
    // 行为，但那样证不了「回读拿到了预览里没有的东西」）。
    let line = on_disk
        .lines()
        .position(|l| l.contains(&middle_marker))
        .expect("中段行号") as u64;
    let read = ToolCall {
        id: "call-2".into(),
        name: "read_file".into(),
        arguments: serde_json::json!({"path": path, "offset": line, "limit": 3}).to_string(),
    };
    let window = tools.execute(&read, &ctx).await.unwrap().content;
    assert!(
        window.contains(&middle_marker),
        "按路径分段回读应当能拿到预览之外的中段：{window}"
    );
}

/// 轮内到线就压缩（票 06(b)），且**锚点是本轮那句问题**（决策 180 的语义）：
/// 摘要插在它之后，被压掉的是它之前的历史——历史不得顶替本轮起点。
#[tokio::test]
async fn an_over_budget_transcript_compacts_inline_anchored_at_the_current_question() {
    let h = Harness::seeded().await;
    let sid = h.session().await;
    // 窗口 8000 → 轮内触发线 80% = 6400 token ≈ 25600 字符。
    h.provider_with_window(8_000).await;
    // 历史（够长到有东西可压）：6 条
    for i in 0..3 {
        h.store
            .append_foreman_user_message(&sid, &format!("旧问题{i} {}", "史".repeat(80)))
            .await
            .unwrap();
        h.store
            .append_foreman_message(NewForemanMessage::assistant(
                &sid,
                format!("旧回答{i} {}", "案".repeat(80)),
            ))
            .await
            .unwrap();
    }
    // 一个够大但**低于卸载阈值**的文件：结果整份进 transcript（每份 ≈ 2000 token）
    // 每份 ≈ 12000 字符（read_file 留 200 行）× 4 次 ≈ 12000 token —— 稳稳过线，
    // 而单份 3000 token 又低于卸载阈值（4000），于是它们**整份进 transcript**。
    std::fs::write(
        h._home.path().join("notes.txt"),
        (1..=260)
            .map(|i| format!("第{i}行：这一段是现场笔记，用来把上下文顶过触发线，写得长一点。{i}"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();

    let question = "现在怎么办？";
    let mut script = Script::new();
    for _ in 0..4 {
        script
            .for_foreman()
            .tool("read_file", serde_json::json!({"path": "notes.txt"}));
    }
    script.for_foreman().text("看完了。");
    let agent = FakeAgent::new(script);
    h.runner(agent.clone())
        .say(Some(&sid), question)
        .await
        .unwrap();

    let requests = agent.request_log();
    let last = requests.last().unwrap();
    let texts: Vec<String> = last
        .messages
        .iter()
        .map(|m| m.content.clone().unwrap_or_default())
        .collect();
    let summary_at = texts
        .iter()
        .position(|t| t.starts_with("[摘要] 已完成的操作："))
        .expect("过线之后应当出现规则化摘要");
    let question_at = texts
        .iter()
        .position(|t| t.ends_with(question))
        .expect("本轮那句问题必须在场（快照并进末尾轮，问题在该轮结尾）");
    assert_eq!(
        question_at + 1,
        summary_at,
        "摘要要插在本轮起点（那句问题）之后、最近轮次之前：{texts:?}"
    );
    assert!(
        texts[..question_at]
            .iter()
            .all(|t| !t.starts_with("旧问题")),
        "被压掉的必须是本轮起点之前的载入历史：{texts:?}"
    );
}

/// 撞墙恢复（票 06(c)）：provider 报上下文超长 → 压缩本轮转录后**重试这一次调用**，
/// 而不是原地判败。
#[tokio::test]
async fn a_context_window_error_compacts_the_transcript_and_retries_the_call() {
    let h = Harness::seeded().await;
    let sid = h.session().await;
    for i in 0..3 {
        h.store
            .append_foreman_user_message(&sid, &format!("旧问题{i} {}", "史".repeat(40)))
            .await
            .unwrap();
        h.store
            .append_foreman_message(NewForemanMessage::assistant(
                &sid,
                format!("旧回答{i} {}", "案".repeat(40)),
            ))
            .await
            .unwrap();
    }
    std::fs::write(
        h._home.path().join("notes.txt"),
        (1..=200)
            .map(|i| format!("第{i}行：现场笔记。"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();

    let mut script = Script::new();
    script
        .for_foreman()
        .tool("read_file", serde_json::json!({"path": "notes.txt"}));
    script.for_foreman().push(testkit::Step::Fail {
        kind: "llm_context_window".into(),
        message: "这次请求放不下模型窗口".into(),
        raw: "context_length_exceeded".into(),
    });
    script.for_foreman().text("压缩之后能跑了。");
    let agent = FakeAgent::new(script);
    let turn = h
        .runner(agent.clone())
        .say(Some(&sid), "帮我看看")
        .await
        .expect("压缩之后这一次调用应当成功");

    assert_eq!(turn.reply, "压缩之后能跑了。");
    let requests = agent.request_log();
    assert_eq!(requests.len(), 3, "工具那一轮 + 撞墙 + 重试");
    let failed = &requests[1].messages;
    let retried = &requests[2].messages;
    assert!(
        retried.len() < failed.len(),
        "重试那一趟必须是压过的（{} → {}）",
        failed.len(),
        retried.len()
    );
    assert!(
        retried.iter().any(|m| m
            .content
            .as_deref()
            .is_some_and(|c| c.starts_with("[摘要]"))),
        "压缩留痕要看得见：{retried:?}"
    );
    assert!(
        retried.iter().any(|m| m
            .content
            .as_deref()
            .is_some_and(|c| c.ends_with("帮我看看"))),
        "本轮那句问题在压缩后仍在场（快照并进末尾轮，问题在该轮结尾）"
    );
}

// ─────────────────────── 会话回执（票 02）───────────────────────

#[tokio::test]
async fn read_conversation_tool_returns_the_workshop_receipt() {
    let h = Harness::seeded().await;
    // 真造一条节点会话行（走既有存储 API，不手写 SQL）。
    let run_id = h
        .store
        .insert_run(&agentpipeline_core::storage::observability::NewRun {
            task_id: "t1".into(),
            cursor_id: h.store.load_live_cursors("t1").await.unwrap()[0]
                .cursor_id
                .clone(),
            stage: Stage::Develop,
            node: Node::Execute,
            attempt: 1,
            agent_type: "main".into(),
            parent_run_id: None,
            prompt_template_hash: None,
            process_group_id: None,
        })
        .await
        .unwrap();
    h.store
        .insert_conversation(
            "t1",
            run_id,
            Stage::Develop,
            Node::Execute,
            1,
            "main",
            None,
            &serde_json::json!([
                {"role": "user", "content": "把登录改一下"},
                {"role": "assistant", "content": "改完了，在 src/auth.rs:42"}
            ]),
            None,
            None,
            120,
            45,
            None,
        )
        .await
        .unwrap();

    let mut script = Script::new();
    // run_id 省略 = 最近一次：人是按「那个货箱卡哪儿了」提问的，不是按运行编号。
    script.for_foreman().read_conversation("t1", None);
    script
        .for_foreman()
        .text("develop 那次它说改完了，在 src/auth.rs:42。");
    let runner = h.runner(FakeAgent::new(script));

    let turn = runner.say(None, "它上次干了啥？").await.unwrap();
    assert_eq!(turn.traces.len(), 1);
    assert!(turn.traces[0].ok);
    assert!(turn.reply.contains("src/auth.rs:42"));

    let messages = h
        .store
        .list_foreman_messages(&turn.session.id, 100, None)
        .await
        .unwrap();
    let assistant = messages.iter().find(|m| m.role == "assistant").unwrap();
    let traces = assistant.traces_json.as_ref().unwrap();
    assert_eq!(traces[0]["tool"], "read_conversation");
}

#[tokio::test]
async fn out_of_whitelist_tools_are_rejected_at_the_execution_point() {
    // 白名单必须在**执行点**生效，不是在工具定义层过滤：模型可以无视 tool 定义
    // 直接发一个越权工具。票 02 的硬约束，与只读子代理同一处检查。
    //
    // 用例在票 06 之后换了个工具名：原来的 `run_command` 现在**在清单里**了（它走确认钮，
    // 不靠不给），故边界改由 `spawn_sub_agent` 取证——它永远不在值班长的清单里。
    let h = Harness::seeded().await;
    let mut script = Script::new();
    script
        .for_foreman()
        .tool("spawn_sub_agent", serde_json::json!({"task": "去干点别的"}));
    script.for_foreman().text("我读不到那个。");
    let runner = h.runner(FakeAgent::new(script));

    let turn = runner.say(None, "帮我开个子代理").await.unwrap();
    assert_eq!(turn.traces.len(), 1);
    assert_eq!(turn.traces[0].tool, "spawn_sub_agent");
    // 工具被拒 → 痕迹记 ok = false；且没有任何东西被落下来。
    assert!(!turn.traces[0].ok, "越权工具必须在执行点被拒");
    assert!(h
        .store
        .list_pending_foreman_proposals(&turn.session.id)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn unknown_task_id_answers_with_text_instead_of_failing_the_turn() {
    // 模型记错一个 id 是最常见的失败；它不该把整次回话打挂（决策 33 的 tool_retry_max）。
    let h = Harness::seeded().await;
    let mut script = Script::new();
    script.for_foreman().read_task("t-nonexistent");
    script.for_foreman().text("台账里没有这个号。");
    let runner = h.runner(FakeAgent::new(script));

    let turn = runner.say(None, "t-nonexistent 呢？").await.unwrap();
    assert_eq!(turn.traces.len(), 1);
    assert!(turn.traces[0].ok, "「查无此任务」是正常回答，不是工具故障");
    assert!(turn.reply.contains("没有这个号"));
}

#[test]
fn the_foreman_tool_set_matches_the_frozen_contract() {
    // 清单的**名字与顺序**（24 个）逐条钉住：这是安全边界本身（`foreman.rs` 的注释原话），
    // 加一个工具必须先改这里，从而在任何 diff 里显式可见。
    //
    // 分组（你能直接用 / 会改动东西）**不再手标**——`ForemanToolLayer` 已删（决策 247），
    // 两组由档位谓词从这份名单派生，派生对不对由下一条用例钉。
    let names: Vec<&str> = FOREMAN_TOOL_SPECS.iter().map(|s| s.name).collect();
    assert_eq!(
        names,
        [
            "read_task",
            "read_conversation",
            "read_board",
            "read_metrics",
            "read_projects",
            "read_stage_configs",
            "read_skills",
            "read_providers",
            // B 层环境只读（决策 206）：域 = 家目录根，`data/` 与 `logs/` 按前缀拒。
            // `spawn_sub_agent` **不在列**——它要注入子代理运行器才有意义，而值班长
            // 手上没有（也不该有）：加一个只会回「未启用」的工具是噪声。
            "read_file",
            "list_dir",
            "Skill",
            // 诊断包（决策 211③ / 票 03）：一族一个工具，一次调用给出定因所需的全部证据。
            // 它**不扩 `read_task`**——后者是每轮值守都会调的高频、便宜读数，混进来会让
            // 「看一眼任务状态」开始烧 12k 字符。
            "read_diagnosis",
            // 只读取证的命令（决策 237 / 票 02）：白名单就是它的全部能力（不经 shell、
            // 按命令名判定，`sample` 只许对本服务的 pid 与其子进程）。它属**只读层**，
            // 故不受档位管、也不在值守轮的 deny 清单里——「自主轮能取证」正是它为的。
            "run_readonly",
            // 内容搜索（决策 267 / 票 01）：纯 Rust 正则找内容，不碰系统二进制。同属
            // 只读层：不受档位管、值守轮 deny 清单**不摘**它（run_readonly 先例）。
            "search_content",
            // 受治理的网口（决策 266 / 票 02）：GET-only、https 出环、白名单走 `NetworkPolicy`
            // 同一张、每次都落命令台账。同属只读层故**不受档位管**，但值守轮的 deny 清单
            // 收它（夜间外发无人盯）——与 `run_readonly` 的区别正在这一条上。
            "web_fetch",
            // C 层：环境写（决策 206 / 207）。`ask` 档下生成提议、`auto` 直通、`deny` 摘掉。
            "write_file",
            "edit_file",
            "run_command",
            // 修复轮（决策 210③④ / 票 10–12）：归环境层由档位管（`ask` 每步要按键、
            // `auto` 整轮自己跑），**合入永远人按**。没有进托管自动集。
            "repair",
            // D 层：本服务写接口（决策 207④ 的「一族一个工具 + 动作参数」）。
            // **不读档位**——写接口恒为提议，配成 `auto` 只放开环境层。
            // 全局动作 `service`（票 09）：`action=restart`——**永远只提议**（会打断所有
            // 在跑的任务），既不受档位影响，也不在托管自动集里。
            "service",
            // 三个排除项（重置配对令牌 / 局域网开关 / 仓名单增删）**没有对应名字**，
            // 由本文件末尾的用例单独断言。
            "task",
            "config",
            "skills",
            // 结构化选项提问（决策 265 / 票 01）：不在两段写清单里——问话不是打算执行的
            // 动作，`gate_decision` 恒 Execute；值守轮的 deny 清单收它（在叫人、不在问人）。
            "ask",
        ]
    );
    assert_eq!(FOREMAN_STAGE_KEY, "foreman");
    assert_eq!(FOREMAN_AGENT_TYPE, "foreman");
}

/// 清单里的参数 schema 必须是合法 JSON —— `tool_defs()` 里那次解析会 panic，
/// 而它发生在**每次回话**的构造路径上。这条断言把失败提前到测试。
#[test]
fn every_listed_tool_has_a_parseable_parameter_schema() {
    for spec in FOREMAN_TOOL_SPECS.iter() {
        let parsed: serde_json::Value = serde_json::from_str(spec.parameters)
            .unwrap_or_else(|e| panic!("{} 的参数 schema 不是合法 JSON：{e}", spec.name));
        assert_eq!(
            parsed["type"], "object",
            "{} 的参数须是 JSON-Schema 对象",
            spec.name
        );
        assert!(
            !spec.description.trim().is_empty(),
            "{} 缺广告语——模型只能靠它决定要不要调",
            spec.name
        );
        // 编译器管 `label` 有没有（必填字段，决策 247④），这里管它是不是空串：
        // 空串会让回执上凭空少一个词，而「少一个词」与「没登记」在界面上分不开。
        assert!(
            !spec.label.trim().is_empty(),
            "{} 的 label 是空的——回执上会只剩一个裸工具名",
            spec.name
        );
    }
    // 名字不得重复：重复会让「清单里 N 个」与「实际可用几个」对不上
    let mut names: Vec<&str> = FOREMAN_TOOL_SPECS.iter().map(|s| s.name).collect();
    let before = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), before, "清单里有重名工具");
}

// ─────────────────────── A 层环境读数（决策 188 / 207，票 01）───────────────────────

/// 六个新读数各跑一遍：都该正常返回（脚本里它们一律排在与台账工具同一轮）。
#[tokio::test]
async fn the_environment_read_tools_all_answer() {
    let h = Harness::seeded().await;
    let mut script = Script::new();
    for tool in [
        "read_board",
        "read_metrics",
        "read_projects",
        "read_stage_configs",
        "read_skills",
        "read_providers",
    ] {
        script.for_foreman().tool(tool, serde_json::json!({}));
    }
    script.for_foreman().text("读完了。");
    let runner = h.runner(FakeAgent::new(script));

    let turn = runner.say(None, "现在是什么情况？").await.unwrap();
    assert_eq!(turn.traces.len(), 6, "六个读数各一次：{:?}", turn.traces);
    for trace in &turn.traces {
        assert!(trace.ok, "{} 应当成功：{:?}", trace.tool, turn.traces);
    }
}

/// **安全断言**：`read_providers` 只回显掩码——明文密钥不得出现在它的结果里（决策 112）。
///
/// 判据打在**回灌给模型的那份文本**上（第二轮的 messages），不是打在工具返回值上：
/// 只有前者能证明「模型看不到明文」，而后者可能是掩码过、但回灌时又被换回原文的。
#[tokio::test]
async fn read_providers_never_echoes_the_plaintext_key() {
    use agentpipeline_core::types::Provider;

    const SECRET: &str = "sk-live-0000000000000000000000000000";
    let h = Harness::seeded().await;
    h.store
        .upsert_provider(&Provider {
            id: "p-secret".into(),
            vendor: "deepseek".into(),
            model: "deepseek-chat".into(),
            context_window: 64_000,
            base_url: None,
            api_key: Some(SECRET.into()),
            enabled: true,
            created_at: h.clock.now(),
            updated_at: h.clock.now(),
        })
        .await
        .unwrap();

    let mut script = Script::new();
    script
        .for_foreman()
        .tool("read_providers", serde_json::json!({}));
    script.for_foreman().text("provider 配了一个。");
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());
    let turn = runner.say(None, "provider 配了吗？").await.unwrap();
    assert_eq!(turn.traces.len(), 1);
    assert!(turn.traces[0].ok);

    let requests = agent.request_log();
    let fed = serde_json::to_string(&requests[1].messages).unwrap();
    assert!(
        !fed.contains(SECRET),
        "明文密钥经 read_providers 漏给了模型：{fed}"
    );
    assert!(fed.contains("p-secret"), "provider 本身该看得到：{fed}");
    assert!(fed.contains("***"), "密钥的存在性该看得到（掩码）：{fed}");
}

/// 跑一轮**人的**回话，返回模型真正收到的那一份 system prompt（不是常量本身——
/// 取证要取模型看到的那份，否则纪律段算早了还是算晚了都看不出来）。
async fn prompt_for(mode: Option<agentpipeline_core::types::EnvMode>) -> String {
    let h = Harness::seeded().await;
    if let Some(mode) = mode {
        h.store
            .upsert_stage_config(&StageConfig {
                stage: FOREMAN_STAGE_KEY.to_string(),
                env_mode: Some(mode),
                ..Default::default()
            })
            .await
            .unwrap();
    }
    let mut script = Script::new();
    script.for_foreman().text("收到。");
    let agent = FakeAgent::new(script);
    let requests = agent.clone();
    let runner = h.runner(agent);
    runner.say(None, "在吗").await.unwrap();
    requests.request_log()[0].system_prompt.clone()
}

/// 从 system prompt 里抠出工具纪律段列的**两组**名单（`你能直接用的工具是：…。` 与
/// `会改动东西的工具是：…。`），断言打的是模型看到的行文，不是常量表。
fn discipline_groups(prompt: &str) -> (Vec<String>, Vec<String>) {
    fn grab(prompt: &str, marker: &str) -> Vec<String> {
        let line = prompt
            .lines()
            .find(|l| l.contains(marker))
            .unwrap_or_else(|| panic!("prompt 里没有「{marker}」那一行：{prompt}"));
        let body = line
            .split_once(marker)
            .expect("上一行刚按同一个 marker 找到")
            .1
            .trim()
            .trim_end_matches('。');
        body.split(" / ")
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    }
    (
        grab(prompt, "你能直接用的工具是："),
        grab(prompt, "会改动东西的工具是："),
    )
}

/// 人格是**静态文案**，三档、值守与否都逐字相同，且它点着工具名（`read_conversation` /
/// `repair`）——本票明文不动它的文案（决策 247），故「prompt 不含 X」的取证先把人格摘掉：
/// 余下的（须知 + 工具纪律 + 纪律之后的每一段）才是按 `available` 算出来的那部分。
fn without_persona(prompt: &str) -> String {
    prompt.replace(FOREMAN_PERSONA, "")
}

/// 三处措辞（`FOREMAN_PERSONA` / `FOREMAN_BASELINE` / 工具纪律段）按档位说真话，
/// **取证取的是模型真正收到的那一份 system prompt**（不是常量本身）。
///
/// 这一条盯的是一个会让模型开始说谎的失效：上一版纪律段写的是「读不到文件系统，也不能执行
/// 命令」——那在 B / C / E 层落地之后是**假的**，而一段假的能力说明会直接变成一句假话
/// （「我读过那个文件」）。三档各取一轮请求，逐句核。
#[tokio::test]
async fn the_system_prompt_tells_the_truth_about_what_it_can_do_in_each_tier() {
    // 三档共同的下限：不许再出现那两句**已经不成立**的能力说明，且称呼恒定。
    let ask = prompt_for(None).await;
    for forbidden in ["读不到文件系统", "没有动手的权力", "不能执行命令", "值班员"]
    {
        assert!(
            !ask.contains(forbidden),
            "人格／纪律里不得再出现「{forbidden}」（决策 188 / 193 / 206）：{ask}"
        );
    }
    assert!(ask.contains("值班经理"), "{ask}");

    // 归因那段「必填」的规格在**三档里都要在**（决策 227 的必填 + 235 的载体 + 238 的形态）：
    // 它是判据④的载体，而档位只该改「能不能动手」，不该改「怎么收口」。
    for prompt in [&ask] {
        assert!(
            prompt.contains(FOREMAN_ATTRIBUTION_MARK),
            "system prompt 要给出结构块的形状：{prompt}"
        );
        assert!(prompt.contains("四类之外不许收口"), "{prompt}");
        // 判据①的要求与判据④同批写进规格：只给类别不给 run，「这条证据是哪条 run 的」
        // 就只能靠行文猜——2026-09-19 正是这么归错的（决策 230）。
        assert!(
            prompt.contains("run_id") && prompt.contains("报错 run 与没有 run 同判未定位"),
            "规格要一并说清 run_id（判据①的校验面）：{prompt}"
        );
        for kind in AttributionKind::ALL {
            assert!(
                prompt.contains(kind.as_str()) && prompt.contains(kind.label()),
                "四类里的 {} 要写进规格（稳定标识与词都要）：{prompt}",
                kind.as_str()
            );
        }
    }

    // `ask` 档（缺省）：说清「提了但没执行」，且把 C / E 层的工具名摆在纪律段里。
    assert!(ask.contains("待确认的提议"), "{ask}");
    assert!(ask.contains("不会立即发生"), "{ask}");
    assert!(ask.contains("run_command"), "{}", ask);
    assert!(ask.contains("write_file"), "{ask}");
    // D 层那三族也在广告集里（写接口恒为提议）
    for tool in ["task", "config", "skills"] {
        assert!(ask.contains(tool), "缺 {tool}：{ask}");
    }

    // `auto` 档：说清「会立即执行」，但**本服务写接口仍要按键**（决策 206 的硬规矩）。
    let auto = prompt_for(Some(agentpipeline_core::types::EnvMode::Auto)).await;
    assert!(auto.contains("会立即执行"), "{auto}");
    assert!(
        auto.contains("仍然"),
        "auto 档下写接口仍要人按键这句不能少：{auto}"
    );

    // `deny` 档：说清环境层关掉了，并**明确叫它别再提**（提议无处可去）。
    let deny = prompt_for(Some(agentpipeline_core::types::EnvMode::Deny)).await;
    assert!(deny.contains("关掉"), "{deny}");
    assert!(deny.contains("不要提议"), "{deny}");
}

/// 纪律段的两组名单**从档位谓词派生**（决策 247）：并 == 冻结清单、交为空，且每个名字
/// 落在「会改动东西」那一组 ⟺ 它是环境层写工具或本服务写接口——`gate_decision` 与纪律段
/// 从此读同一份档位表。手标的层枚举删掉之后，「分组说的」与「闸门判的」还能不能对上，
/// 由这一条拦。
#[tokio::test]
async fn the_discipline_groups_are_derived_from_the_tier_predicates() {
    let prompt = prompt_for(None).await;
    let (direct, mutating) = discipline_groups(&prompt);

    // 两组的并 == 冻结清单：少一个 = 这一轮拿得到却没人告诉它；多一个 = 念了个不存在的。
    let mut union = direct.clone();
    union.extend(mutating.iter().cloned());
    let mut frozen: Vec<String> = FOREMAN_TOOL_SPECS
        .iter()
        .map(|s| s.name.to_string())
        .collect();
    union.sort();
    frozen.sort();
    assert_eq!(union, frozen, "两组的并必须逐字等于冻结清单：{prompt}");

    // 交为空：同一个名字两组都列，模型收到的就是自相矛盾的纪律。
    for name in &direct {
        assert!(
            !mutating.contains(name),
            "{name} 同时出现在两组里：{prompt}"
        );
    }

    // 分组与档位谓词一致（判据的唯一实现是 `ENV_WRITE_TOOLS` / `SERVICE_WRITE_TOOLS`）。
    for name in &direct {
        assert!(
            !is_env_write_tool(name) && !is_service_write_tool(name),
            "「你能直接用」组里的 {name} 其实会改动东西：{prompt}"
        );
    }
    for name in &mutating {
        assert!(
            is_env_write_tool(name) || is_service_write_tool(name),
            "「会改动东西」组里的 {name} 不在任何写清单里：{prompt}"
        );
    }
}

/// 值守轮：这一轮摘掉的两件贵东西**也不许出现在纪律段里**（决策 247 修的顺序 bug——
/// `deny` 从前晚于 `system_prompt` 才算，纪律段于是广告着 `read_conversation` /
/// `run_command`，而广告集与白名单里没有它们：模型被告知去调一个必被拒的名字）。
#[tokio::test]
async fn the_watch_round_prompt_does_not_advertise_what_it_stripped() {
    let h = Harness::seeded().await;
    note(
        &h,
        "t1",
        agentpipeline_core::storage::AttentionKind::RetryExhausted,
    )
    .await;
    // 去抖窗口过了（默认 60s）
    h.clock.advance_secs(61);

    let mut script = Script::new();
    script.for_foreman().text("t1 重试耗尽了，需要看一眼。");
    let agent = FakeAgent::new(script);
    let requests = agent.clone();
    let runner = h.runner(agent);
    runner.watch().await.unwrap().expect("应当醒一次");

    let prompt = requests.request_log()[0].system_prompt.clone();
    let minus_persona = without_persona(&prompt);
    for gone in ["read_conversation", "run_command"] {
        assert!(
            !minus_persona.contains(gone),
            "值守轮摘掉了 {gone}，纪律段却还广告它：{minus_persona}"
        );
    }
    // 摘的是两件贵的，不是整个清单：便宜的台账读数与诊断包照旧广告（否则这轮变哑巴）。
    let (direct, _) = discipline_groups(&prompt);
    assert!(direct.iter().any(|n| n == "read_task"), "{direct:?}");
    assert!(direct.iter().any(|n| n == "read_diagnosis"), "{direct:?}");
}

/// `deny` 档：纪律段点不出**任何**环境层工具名（决策 247）。「连广告都不给」从前只兑现在
/// 广告集与白名单上，纪律段照旧念着整份清单——这是同一个顺序 bug 的档位那一面。
#[tokio::test]
async fn the_deny_tier_prompt_does_not_advertise_the_environment_layer() {
    let prompt = prompt_for(Some(agentpipeline_core::types::EnvMode::Deny)).await;
    let minus_persona = without_persona(&prompt);
    for gone in ENV_TOOLS {
        assert!(
            !minus_persona.contains(gone),
            "deny 档连广告都不给 {gone}，纪律段却念着它：{minus_persona}"
        );
    }
    // 「什么都不给」同样是假话：台账读数与 D 层照旧在（后者恒为提议，不读档位）。
    let (direct, mutating) = discipline_groups(&prompt);
    assert!(direct.iter().any(|n| n == "read_task"), "{direct:?}");
    for kept in ["task", "config", "skills", "service"] {
        assert!(
            mutating.iter().any(|n| n == kept),
            "D 层 {kept} 不随档位走：{mutating:?}"
        );
    }
}

/// `deny` 档：环境层**连广告都不给**，执行点也拒（决策 206，票 03 的按档断言）。
///
/// 两侧都要取证。「只不广告」是不够的：模型可以无视 tool 定义硬发一个工具名，那正是执行点
/// 白名单存在的理由。而 D 层（本服务写接口）**不受档位影响**——它恒为提议，把值班长配成
/// `deny` 收的是「能碰机器」的手，不是「能提建议」的嘴。
#[tokio::test]
async fn the_deny_tier_removes_the_environment_layer_from_both_sides() {
    let h = Harness::seeded().await;
    h.store
        .upsert_stage_config(&StageConfig {
            stage: FOREMAN_STAGE_KEY.to_string(),
            env_mode: Some(agentpipeline_core::types::EnvMode::Deny),
            ..Default::default()
        })
        .await
        .unwrap();

    let mut script = Script::new();
    script.for_foreman().text("环境层关着。");
    let agent = FakeAgent::new(script);
    let requests = agent.clone();
    let runner = h.runner(agent);
    runner.say(None, "看看家目录").await.unwrap();

    let log = requests.request_log();
    let advertised: Vec<String> = log[0].tools.iter().map(|t| t.name.clone()).collect();
    for gone in [
        "read_file",
        "list_dir",
        "Skill",
        "write_file",
        "edit_file",
        "run_command",
    ] {
        assert!(
            !advertised.iter().any(|n| n == gone),
            "deny 档不该广告 {gone}：{advertised:?}"
        );
    }
    // 台账读数与 D 层照旧在（后者恒为提议，不读档位）。
    for kept in ["read_task", "read_conversation", "task", "config", "skills"] {
        assert!(
            advertised.iter().any(|n| n == kept),
            "deny 档下 {kept} 仍应在广告集里：{advertised:?}"
        );
    }

    // 执行点：硬发一个被摘掉的工具名 → 拒，且**没有任何东西真的跑起来**。
    let mut script = Script::new();
    script
        .for_foreman()
        .tool("run_command", serde_json::json!({"command": "echo pwned"}));
    script.for_foreman().text("跑不了。");
    let runner = h.runner(FakeAgent::new(script));
    let turn = runner.say(None, "帮我跑个命令").await.unwrap();
    assert_eq!(turn.traces.len(), 1);
    assert!(!turn.traces[0].ok, "deny 档下命令必须在执行点被拒");
    assert!(
        h.store
            .list_commands("t1", None, None)
            .await
            .unwrap()
            .is_empty(),
        "被拒的命令不得真的跑起来"
    );
    assert!(
        h.store
            .list_pending_foreman_proposals(&turn.session.id)
            .await
            .unwrap()
            .is_empty(),
        "deny 档不生成提议——它无处可去"
    );
}
// ─────────────────────────── 口径与保留期（票 05）───────────────────────────

#[tokio::test]
async fn conversation_tokens_do_not_move_the_global_metrics() {
    let h = Harness::seeded().await;
    let mut script = Script::new();
    script.for_foreman().text("说完了。");
    let runner = h.runner(FakeAgent::new(script));
    let turn = runner.say(None, "随便聊聊").await.unwrap();

    // 对话产生了 token（自报表里看得到）……
    let (tokens, calls) = h
        .store
        .foreman_session_totals(&turn.session.id)
        .await
        .unwrap();
    assert!(tokens > 0, "对话自身有 token 读数");
    assert_eq!(calls, 1);
    // ……但全局指标（按 run 行算）一分不动：值班长不落 run 行。
    let runs = h.store.all_runs().await.unwrap();
    assert_eq!(metrics::total_tokens(&runs), 0);
    assert_eq!(metrics::total_calls(&runs), 0);
}

#[tokio::test]
async fn session_totals_sum_the_persisted_columns() {
    let h = Harness::empty().await;
    let sid = h.session().await;
    h.store
        .append_foreman_user_message(&sid, "一")
        .await
        .unwrap();
    h.store
        .append_foreman_message(NewForemanMessage {
            session_id: sid.clone(),
            role: "assistant".into(),
            content: "二".into(),
            prompt_tokens: 100,
            completion_tokens: 20,
            briefing_json: None,
            traces_json: None,
            segments_json: None,
            changed_files_json: None,
            thinking: None,
            ask_json: None,
        })
        .await
        .unwrap();
    h.store
        .append_foreman_message(NewForemanMessage {
            session_id: sid.clone(),
            role: "assistant".into(),
            content: "三".into(),
            prompt_tokens: 50,
            completion_tokens: 5,
            briefing_json: None,
            traces_json: None,
            segments_json: None,
            changed_files_json: None,
            thinking: None,
            ask_json: None,
        })
        .await
        .unwrap();

    let (tokens, calls) = h.store.foreman_session_totals(&sid).await.unwrap();
    // 求和而不是另存计数器：计数器会与台账漂移，求和永远等于真实存在的东西。
    assert_eq!(tokens, 175);
    // calls 是**回话次数**（assistant 行），不是工具往返次数。
    assert_eq!(calls, 2);
}

/// 在途行**不计入**会话合计（决策 363③）：页头「本次会话 N tok」不许一轮进行中途跟着涨。
///
/// 四条读数各对一件事实：在途行不计（收口前）、收口写回同一行后计入（权威值）、
/// 没跑起来的那一轮中途刷的读数不残留、丢弃之后也不残留——「口径不动」恢复成
/// 「只算已收口的正常行」。
#[tokio::test]
async fn session_totals_ignore_in_flight_rows() {
    let h = Harness::empty().await;
    let sid = h.session().await;
    h.store
        .append_foreman_user_message(&sid, "一")
        .await
        .unwrap();

    // 一轮开工：在途半截行中途刷写成「已经花掉」的读数——页头不许动。
    let row = h.store.begin_foreman_inflight(&sid, None).await.unwrap();
    h.store
        .update_foreman_inflight(
            row,
            &InFlightPatch {
                content: "正在说".into(),
                thinking: None,
                segments_json: None,
                traces_json: None,
                prompt_tokens: 100,
                completion_tokens: 20,
                seq: 3,
            },
        )
        .await
        .unwrap();
    assert_eq!(h.store.foreman_session_totals(&sid).await.unwrap(), (0, 0));

    // 收口：同一行写成完整行（`status` → NULL），权威读数这时才计入。
    h.store
        .close_foreman_inflight(
            row,
            NewForemanMessage {
                session_id: sid.clone(),
                role: "assistant".into(),
                content: "说完了".into(),
                prompt_tokens: 100,
                completion_tokens: 20,
                briefing_json: None,
                traces_json: None,
                segments_json: None,
                changed_files_json: None,
                thinking: None,
                ask_json: None,
            },
            3,
        )
        .await
        .unwrap();
    assert_eq!(
        h.store.foreman_session_totals(&sid).await.unwrap(),
        (120, 1)
    );

    // 新的一轮开工后没跑起来：它在途时的读数不进合计，丢弃之后也不残留。
    let orphan = h.store.begin_foreman_inflight(&sid, None).await.unwrap();
    h.store
        .update_foreman_inflight(
            orphan,
            &InFlightPatch {
                content: "半句".into(),
                thinking: None,
                segments_json: None,
                traces_json: None,
                prompt_tokens: 500,
                completion_tokens: 500,
                seq: 1,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        h.store.foreman_session_totals(&sid).await.unwrap(),
        (120, 1)
    );
    h.store.discard_foreman_inflight(orphan).await.unwrap();
    assert_eq!(
        h.store.foreman_session_totals(&sid).await.unwrap(),
        (120, 1)
    );
}

/// 保留期**只摘消息表**（票 04，显式修订决策 182④「对讲台与全仓同一把保留期尺」与
/// 决策 204⑦「归档不保护消息」——豁免之后，归档与否、超龄与否都不再删对话消息）。
///
/// 假时钟推过 30 天跑一趟维护：值班长消息**一条不少**（反向断言），运维侧三张表
/// （提议行 / 待办行 / 终态任务的会话行）照旧按**同一个 cutoff** 清，一张表都不豁免。
/// worktree 回收另有专条（`maintenance_recycles_a_repair_worktree_nobody_pressed`）。
///
/// 牙齿：把清理函数又接回消息表（或在 `maintenance` 里补一句 DELETE）→ 本条的反向
/// 断言当场红——正是 spec 牙齿表那颗「清理函数又被接回消息表」。
#[tokio::test]
async fn maintenance_keeps_foreman_messages_forever_and_still_purges_the_rest() {
    let h = Harness::seeded().await;
    let sid = h.session().await;

    // 四类各放一条：对话消息（豁免方）+ 提议 + 待办 + 终态任务的会话行。
    h.store
        .append_foreman_user_message(&sid, "很久以前说的")
        .await
        .unwrap();
    h.store
        .create_foreman_proposal(agentpipeline_core::storage::proposals::NewForemanProposal {
            kind: agentpipeline_core::storage::proposals::ForemanProposalKind::ApiCall,
            payload: None,
            session_id: sid.clone(),
            tool: "write_file".into(),
            args: serde_json::json!({"path": "notes.md"}),
            summary: "写入 notes.md".into(),
            situation: None,
        })
        .await
        .unwrap();
    note(
        &h,
        "t1",
        agentpipeline_core::storage::AttentionKind::TaskPending,
    )
    .await;
    // 会话行要被清，任务得先到终态（清理只对 done / failed / cancelled 生效）——
    // 真 run 行 + 真收尾，不伪造。
    let cursor = h.store.load_live_cursors("t1").await.unwrap()[0].clone();
    let run_id = h
        .store
        .insert_run(&agentpipeline_core::storage::observability::NewRun {
            task_id: "t1".into(),
            cursor_id: cursor.cursor_id.clone(),
            stage: Stage::Develop,
            node: Node::Execute,
            attempt: 1,
            agent_type: "main".into(),
            parent_run_id: None,
            prompt_template_hash: None,
            process_group_id: None,
        })
        .await
        .unwrap();
    h.store
        .finish_run(
            run_id,
            &agentpipeline_core::storage::observability::RunOutcome {
                status: Some(agentpipeline_core::types::NodeStatus::Success),
                prompt_tokens: 0,
                completion_tokens: 0,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    h.store
        .insert_conversation(
            "t1",
            run_id,
            Stage::Develop,
            Node::Execute,
            1,
            "main",
            None,
            &serde_json::json!([{"role": "user", "content": "x"}]),
            None,
            None,
            0,
            0,
            None,
        )
        .await
        .unwrap();
    h.store.mark_terminal("t1", TaskStatus::Done).await.unwrap();

    // 未到期先跑一遍：什么都不动（免得用例退化成「总是清」）。
    let fresh = maintenance(&h).await;
    assert_eq!(fresh.purged_conversations, 0);
    assert_eq!(fresh.purged_foreman_proposals, 0);
    assert_eq!(fresh.purged_attention, 0);

    // 假时钟推进 31 天（等不了真实 30 天）。
    h.clock.advance_secs(31 * 24 * 3600);
    let report = maintenance(&h).await;

    // 反向断言：消息表豁免——31 天后一条不少（牙齿钉在这一句上）。
    let rows = h.store.list_foreman_messages(&sid, 10, None).await.unwrap();
    assert_eq!(
        rows.len(),
        1,
        "消息表退出保留期清理，超龄的对话消息永久在库：{rows:?}"
    );

    // 运维侧照旧：三张表同一个 cutoff，各清各的。
    assert_eq!(report.purged_foreman_proposals, 1, "提议行照旧按年龄删");
    assert!(h
        .store
        .list_foreman_proposals(&sid, 10)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(report.purged_attention, 1, "待办行照旧按年龄删");
    assert_eq!(
        report.purged_conversations, 1,
        "终态任务的会话行照旧按年龄删"
    );
    assert!(h
        .store
        .list_conversations("t1", false)
        .await
        .unwrap()
        .is_empty());
}

/// 维护作业管提议的两件事（决策 207）：**过期清扫改状态、年龄清理删行**。
///
/// 合成一步就会让「过期只让按钮变灰、那一轮留在时间线里」这条规则在维护作业这一侧失效
/// ——而那是这条规则唯一会被自动执行的地方。
#[tokio::test]
async fn scheduler_maintenance_expires_proposals_and_purges_them_by_age() {
    let h = Harness::seeded().await;
    let sid = h.session().await;
    let pid = h
        .store
        .create_foreman_proposal(agentpipeline_core::storage::proposals::NewForemanProposal {
            kind: agentpipeline_core::storage::proposals::ForemanProposalKind::ApiCall,
            payload: None,
            session_id: sid.clone(),
            tool: "write_file".into(),
            args: serde_json::json!({"path": "notes.md"}),
            summary: "写入 notes.md".into(),
            situation: None,
        })
        .await
        .unwrap()
        .id;

    // 没过 TTL：什么都不该动。
    let report = maintenance(&h).await;
    assert_eq!(report.expired_foreman_proposals, 0);
    assert_eq!(report.purged_foreman_proposals, 0);

    // 过了 TTL（10 分钟）但还在保留期内：**只改状态、不删行**。
    h.clock.advance_secs(11 * 60);
    let report = maintenance(&h).await;
    assert_eq!(report.expired_foreman_proposals, 1);
    assert_eq!(report.purged_foreman_proposals, 0);
    assert!(h
        .store
        .list_foreman_proposals(&sid, 10)
        .await
        .unwrap()
        .iter()
        .any(|p| p.id == pid && p.status.as_str() == "expired"));

    // 过了保留期：这才删行（那时时间线里也没有这一轮了）。
    h.clock.advance_secs(31 * 24 * 3600);
    let report = maintenance(&h).await;
    assert_eq!(
        report.expired_foreman_proposals, 0,
        "已经过期的不再重复计数"
    );
    assert_eq!(report.purged_foreman_proposals, 1);
    assert!(h
        .store
        .list_foreman_proposals(&sid, 10)
        .await
        .unwrap()
        .is_empty());
}

/// 修复 worktree 的**第三位回收者**（票 12 的最后一格）：没人按过的修复提议过了保留期，
/// 维护作业要把它建的 worktree 收掉，且**保留分支**。
///
/// 与「拒绝」那条不重复：拒绝是人按的，而这一条走的是「人一直没理它，行被年龄清理删掉」。
/// 顺序要紧——回收排在删行之前，否则没有任何东西知道那个目录属于谁、在哪个仓里。
#[tokio::test]
async fn maintenance_recycles_a_repair_worktree_nobody_pressed() {
    use agentpipeline_core::git::Git;
    use agentpipeline_core::pipeline::repair::{finish_repair_round, new_repair_id, start_repair};

    let h = Harness::empty().await;
    let repo = Repo::clean().unwrap();
    h.git_project(&repo, "true").await;
    let sid = h.session().await;

    // 走**真**那条链把 worktree 与提议造出来（不起 worktree 就没有可回收的东西）。
    let session = start_repair(h._home.home(), repo.path(), "main", &new_repair_id(), &sid)
        .await
        .unwrap();
    std::fs::write(session.worktree.join("fixed.rs"), "pub fn fixed() {}\n").unwrap();
    let project = h.store.get_project("p1").await.unwrap().unwrap();
    finish_repair_round(
        &h.store,
        h._home.home(),
        &h.settings,
        &h.killer,
        &project,
        &session,
        "结论一句话",
        None,
        h.clock.now(),
    )
    .await
    .unwrap();

    // 还在保留期内：目录照旧在（免得这条用例退化成「总是删」）。
    let report = maintenance(&h).await;
    assert_eq!(report.recycled_repair_worktrees, 0);
    assert!(session.worktree.exists());

    // 过了保留期（`conversation_retention_days` = 30 天）：worktree 被收，行也被删。
    h.clock.advance_secs(31 * 24 * 3600);
    let report = maintenance(&h).await;
    assert_eq!(report.recycled_repair_worktrees, 1);
    assert_eq!(report.purged_foreman_proposals, 1);
    assert!(
        !session.worktree.exists(),
        "没人按过的修复 worktree 要在这时候被收掉"
    );
    assert!(
        Git.rev_parse(repo.path(), &session.branch).await.is_ok(),
        "分支必须留着——它是这次修复唯一的证据"
    );
    assert!(
        repo.git(&["worktree", "list"]).trim().lines().count() == 1,
        "台账里也不该再挂着那个 worktree：{}",
        repo.git(&["worktree", "list"])
    );
}

/// 回收失败**不许把整趟维护带走**（票 12 的收口）：项目已经不在磁盘上时那一步必然失败，
/// 而它后面就是保留期清理——让一个 bad row 让所有清理停摆，比留下一个目录坏得多。
#[tokio::test]
async fn a_failed_repair_recycle_does_not_stop_the_maintenance() {
    use agentpipeline_core::pipeline::repair::{finish_repair_round, new_repair_id, start_repair};

    let h = Harness::empty().await;
    let repo = Repo::clean().unwrap();
    h.git_project(&repo, "true").await;
    let sid = h.session().await;

    let session = start_repair(h._home.home(), repo.path(), "main", &new_repair_id(), &sid)
        .await
        .unwrap();
    std::fs::write(session.worktree.join("fixed.rs"), "pub fn fixed() {}\n").unwrap();
    let project = h.store.get_project("p1").await.unwrap().unwrap();
    finish_repair_round(
        &h.store,
        h._home.home(),
        &h.settings,
        &h.killer,
        &project,
        &session,
        "结论一句话",
        None,
        h.clock.now(),
    )
    .await
    .unwrap();
    // 项目**连目录一起**没了（比「行没了」更狠的一种坏数据：判据过得去，动手时才炸）
    std::fs::remove_dir_all(repo.path()).unwrap();

    h.clock.advance_secs(31 * 24 * 3600);
    let report = maintenance(&h).await;
    assert_eq!(report.recycled_repair_worktrees, 0, "回收没成，如实记 0");
    assert_eq!(
        report.purged_foreman_proposals, 1,
        "行**照旧**被年龄清理删掉——维护作业没被那一步带走"
    );
    assert!(
        session.worktree.exists(),
        "那次回收确实失败了（目录还在）——这条用例断的正是「失败了也不许拦着后面的清理」"
    );
}

async fn maintenance(h: &Harness) -> agentpipeline_core::scheduler::MaintenanceReport {
    use agentpipeline_core::scheduler::KanbanScheduler;
    let scheduler = KanbanScheduler::new(
        h.store.clone(),
        Settings::default(),
        Arc::new(h.clock.clone()),
        Arc::new(testkit::RecordingKiller::new()),
        Arc::new(testkit::SseRecorder::new()),
        Arc::new(|_: &str| {}),
    );
    scheduler.maintenance().await.unwrap()
}

// ─────────────────────────── 任务清单不受影响（回归）───────────────────────────

#[tokio::test]
async fn foreman_conversation_never_creates_task_rows() {
    // 值班长不落 run 行、不碰任务表：跑一轮之后看板列表必须一字不变。
    let h = Harness::seeded().await;
    let before = h
        .store
        .list_tasks(&TaskFilter {
            include_archived: true,
            ..Default::default()
        })
        .await
        .unwrap();
    let mut script = Script::new();
    script.for_foreman().text("收到。");
    h.runner(FakeAgent::new(script))
        .say(None, "在吗")
        .await
        .unwrap();
    let after = h
        .store
        .list_tasks(&TaskFilter {
            include_archived: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(before.len(), after.len());
    assert_eq!(after.len(), 1);
    assert!(h.store.all_runs().await.unwrap().is_empty());
    // 会话表里也没有它——那是流水线节点的表。
    assert!(h
        .store
        .list_conversations("t1", true)
        .await
        .unwrap()
        .is_empty());
}

// ───────────────────── 名分：人格与界面同一个词（决策 193）─────────────────────

/// 人格里对**人**的称呼必须与界面上那块名牌是同一个词。
///
/// 这**不是在测 prompt 的效果**（那属 v2 离线 eval，见本文件抬头）：钉住的是同一个名分不能
/// 有两个答案。它漂过一次——「工头」曾同时指玩家与对面那个 agent（决策 174 挂标、176 裁决）；
/// 第二次是人这一侧的名分与权力对不上（`员` < `长`，而拍板权在人手里、对面那个人格还写着
/// 「你没有动手的权力」），使用者当场读出别扭，故改为**值班经理**（决策 193）。界面那一侧
/// 由 e2e `talk.spec.ts` 的同一对字符串钉住（两块名牌都断言）。
#[test]
fn the_persona_calls_the_human_what_the_ui_does() {
    assert!(
        FOREMAN_PERSONA.contains("值班经理"),
        "人格必须称呼人为值班经理（决策 193）：{FOREMAN_PERSONA}"
    );
    assert!(
        !FOREMAN_PERSONA.contains("值班员"),
        "旧名分「值班员」不得回潮（决策 193）：{FOREMAN_PERSONA}"
    );
}

// ─────────────────────── 会话隔离与命令归属（票 01 / 决策 204）───────────────────────

/// 隔离的是**上下文**：两个班次各说两句，消息读不到对方的、合计互不污染。
///
/// 这同时修掉一处假读数（决策 204⑤）：`foreman_session_totals` 此前对整张表求和，
/// 页头那句「本次会话 N tok」其实是「自建库以来的累计值」。
#[tokio::test]
async fn two_sessions_do_not_pollute_each_others_messages_or_totals() {
    let h = Harness::empty().await;
    let a = h.session().await;
    let b = h.session().await;

    h.store
        .append_foreman_user_message(&a, "甲班第一句")
        .await
        .unwrap();
    h.store
        .append_foreman_message(NewForemanMessage {
            session_id: a.clone(),
            role: "assistant".into(),
            content: "甲班回话".into(),
            prompt_tokens: 100,
            completion_tokens: 10,
            briefing_json: None,
            traces_json: None,
            segments_json: None,
            changed_files_json: None,
            thinking: None,
            ask_json: None,
        })
        .await
        .unwrap();
    h.store
        .append_foreman_user_message(&b, "乙班第一句")
        .await
        .unwrap();

    let in_a = h.store.list_foreman_messages(&a, 100, None).await.unwrap();
    let in_b = h.store.list_foreman_messages(&b, 100, None).await.unwrap();
    assert_eq!(in_a.len(), 2);
    assert_eq!(in_b.len(), 1);
    assert!(in_a.iter().all(|m| m.session_id == a));
    assert!(in_b.iter().all(|m| m.session_id == b));
    assert!(!in_a.iter().any(|m| m.content.contains("乙班")));

    assert_eq!(h.store.foreman_session_totals(&a).await.unwrap(), (110, 1));
    assert_eq!(h.store.foreman_session_totals(&b).await.unwrap(), (0, 0));
}

/// 标题取自**首条用户消息**，之后再说多少句都不再改；改名之后也不被下一句冲掉。
#[tokio::test]
async fn the_title_comes_from_the_first_user_message_and_a_rename_sticks() {
    let h = Harness::empty().await;
    let mut script = Script::new();
    script.for_foreman().text("好。");
    script.for_foreman().text("好。");
    let runner = h.runner(FakeAgent::new(script));

    let first = runner.say(None, "帮我看下 t1 卡在哪").await.unwrap();
    assert_eq!(first.session.title, "帮我看下 t1 卡在哪");

    let second = runner
        .say(Some(&first.session.id), "再看一眼")
        .await
        .unwrap();
    assert_eq!(
        second.session.title, "帮我看下 t1 卡在哪",
        "标题来自第一句，后续每一句都不该改写它"
    );

    h.store
        .rename_foreman_session(&first.session.id, "昨晚那一班")
        .await
        .unwrap();
    let third = runner.say(Some(&first.session.id), "还在吗").await.unwrap();
    assert_eq!(third.session.title, "昨晚那一班", "人起的名字不得被冲掉");
}

/// 列表按最近活动倒序——「刚才在说的那个」排在最前，切换才不用找。
#[tokio::test]
async fn sessions_are_listed_by_recent_activity() {
    let h = Harness::empty().await;
    let a = h.session().await;
    h.clock.advance_secs(60);
    let b = h.session().await;
    h.clock.advance_secs(60);

    // 先建的那个因为又说了话，回到最前。
    h.store
        .append_foreman_user_message(&a, "又想起一句")
        .await
        .unwrap();

    let list = h
        .store
        .list_foreman_sessions(Some(FOREMAN_SESSION_KIND_TALK), false)
        .await
        .unwrap();
    assert_eq!(
        list.iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
        vec![a, b]
    );
}

/// 归档 = 从列表里收起来，**不物理删除**：消息仍在库里，按 id 也仍取得到。
#[tokio::test]
async fn archiving_hides_it_from_the_list_but_keeps_its_messages() {
    let h = Harness::empty().await;
    let a = h.session().await;
    let b = h.session().await;
    h.store
        .append_foreman_user_message(&a, "旧班次")
        .await
        .unwrap();

    let archived = h.store.archive_foreman_session(&a).await.unwrap().unwrap();
    assert!(archived.archived_at.is_some());

    let list = h
        .store
        .list_foreman_sessions(Some(FOREMAN_SESSION_KIND_TALK), false)
        .await
        .unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, b, "归档的不在列表里，剩下的照旧");

    assert_eq!(
        h.store
            .list_foreman_messages(&a, 100, None)
            .await
            .unwrap()
            .len(),
        1,
        "归档不删消息——它是收起来，不是永久保存的反面"
    );
    assert!(h.store.get_foreman_session(&a).await.unwrap().is_some());
    // 幂等：重复归档不改第一次的时间戳。
    let again = h.store.archive_foreman_session(&a).await.unwrap().unwrap();
    assert_eq!(again.archived_at, archived.archived_at);
}

/// 往已归档的班次里说话被拒：那样的记录谁也看不见。
#[tokio::test]
async fn sending_to_an_archived_session_is_refused() {
    let h = Harness::empty().await;
    let sid = h.session().await;
    h.store.archive_foreman_session(&sid).await.unwrap();
    let mut script = Script::new();
    script.for_foreman().text("在的。");
    let runner = h.runner(FakeAgent::new(script));

    let err = runner.say(Some(&sid), "喂").await.unwrap_err();
    assert!(
        matches!(err, Error::Validation(_)),
        "归档后不该还能说话：{err}"
    );
    assert!(h
        .store
        .list_foreman_messages(&sid, 10, None)
        .await
        .unwrap()
        .is_empty());
}

/// 不存在的班次 → 报错，不是悄悄落进别的班次。
#[tokio::test]
async fn sending_to_an_unknown_session_reports_it() {
    let h = Harness::empty().await;
    let mut script = Script::new();
    script.for_foreman().text("在的。");
    let runner = h.runner(FakeAgent::new(script));
    let err = runner.say(Some("no-such-session"), "喂").await.unwrap_err();
    assert!(matches!(err, Error::Task(_)), "{err}");
    assert!(h
        .store
        .list_foreman_sessions(Some(FOREMAN_SESSION_KIND_TALK), false)
        .await
        .unwrap()
        .is_empty());
}

/// 值班长的命令挂**会话**，不挂任务（决策 204④）——它是迁移 0012 改 `task_id` 可空的理由。
#[tokio::test]
async fn a_foreman_command_lands_under_the_session_not_a_task() {
    use agentpipeline_core::agent::tools::{CommandFinish, CommandRecorder, CommandStart};
    use agentpipeline_core::types::CommandSource;

    let h = Harness::seeded().await;
    let sid = h.session().await;
    let id = h
        .store
        .record_start(CommandStart {
            // 值班长给的是空串（它没有任务）：存储层把空串归一成 NULL，
            // 否则外键校验会当场拒掉这行（迁移 0004:11 的哨兵值问题）。
            task_id: Some(String::new()),
            session_id: Some(sid.clone()),
            run_id: None,
            stage: Stage::Init,
            node: Node::Execute,
            source: CommandSource::Agent,
            command: "ls tasks".into(),
            cwd: h._home.path().display().to_string(),
            original_command: None,
        })
        .await
        .unwrap();
    h.store
        .record_finish(
            id,
            CommandFinish {
                exit_code: Some(0),
                duration_ms: 3,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let commands = h.store.list_foreman_commands(&sid).await.unwrap();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].task_id, None, "值班长的命令不挂任务");
    assert_eq!(commands[0].session_id.as_deref(), Some(sid.as_str()));
    assert_eq!(commands[0].exit_code, Some(0));
    // 任务口径不变：它不出现在任何任务的命令列表里。
    assert!(h
        .store
        .list_commands("t1", None, None)
        .await
        .unwrap()
        .is_empty());
    // 另一个班次也读不到它。
    let other = h.session().await;
    assert!(h
        .store
        .list_foreman_commands(&other)
        .await
        .unwrap()
        .is_empty());
}

/// 一条命令必须**恰好**属于一个归属：两个都没有时明确报错，而不是落一行无人认领的记录。
#[tokio::test]
async fn a_command_without_an_owner_is_refused() {
    use agentpipeline_core::agent::tools::{CommandRecorder, CommandStart};
    use agentpipeline_core::types::CommandSource;

    let h = Harness::seeded().await;
    let err = h
        .store
        .record_start(CommandStart {
            task_id: Some(String::new()),
            session_id: None,
            run_id: None,
            stage: Stage::Init,
            node: Node::Execute,
            source: CommandSource::System,
            command: "true".into(),
            cwd: "/tmp".into(),
            original_command: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Validation(_)), "{err}");
    assert!(message_of(&err).contains("归属"));
}

fn message_of(err: &Error) -> String {
    err.to_string()
}

// ───────────── 文件域与卸载（决策 206 / 207，票 03）─────────────

/// 值班长读得到家目录里的普通文件与 `logs/` 下的日志，**读不到 `data/`**。
///
/// 这一条是**补偿**不是边界：`run_command` 不受文件策略管（命令自己 `cd` 就出去了），
/// 故它只挡住「用文件工具顺手读走密钥」这一条路。`data/agentpipeline.db` 里
/// 明文存着 provider 密钥（决策 112），而默认那份**模式**名单（`.env*` / `*.pem` / …）
/// 盖不住一个 `.db` 文件——这正是 `foreman_file_policy` 按路径前缀补上的那一条。
///
/// **`logs/` 在读得着的那一侧**（决策 226 撤销了原来那条前缀拒绝）：日志的价值不在秘密
/// 而在体量，而体量由 `read_file` 的字节上限管；把它们一并拒掉，代价是 2026-09-19 那次
/// 僵死的根因（一行 `resume 触发被在跑的 executor 持续挡下`）恰好只写在日志里。
#[tokio::test]
async fn the_foreman_reads_the_home_but_not_the_key_store() {
    let h = Harness::seeded().await;
    let session = h.session().await;
    // 家目录里放一个普通文件，与一个「看起来像配置」的产物
    std::fs::write(h._home.home().root().join("NOTES.md"), "夜班交接：一切正常").unwrap();
    // 密钥库路径上放一个可读文件（真库是 SQLite 二进制，读它本来也没什么可读的）
    std::fs::write(h._home.home().db_path(), "sk-super-secret-value").unwrap();
    std::fs::create_dir_all(h._home.home().logs_dir()).unwrap();
    std::fs::write(h._home.home().logs_dir().join("app.log"), "日志一行").unwrap();

    let mut script = Script::new();
    script
        .for_foreman()
        .tool("read_file", serde_json::json!({"path": "NOTES.md"}));
    script.for_foreman().tool(
        "read_file",
        serde_json::json!({"path": "data/agentpipeline.db"}),
    );
    script
        .for_foreman()
        .tool("read_file", serde_json::json!({"path": "logs/app.log"}));
    script.for_foreman().text("能读的读了，密钥库读不到。");
    let agent = FakeAgent::new(script);
    let requests = agent.clone();
    let runner = h.runner(agent);
    let turn = runner.say(Some(&session), "交接笔记在吗？").await.unwrap();

    assert_eq!(turn.traces.len(), 3);
    assert!(turn.traces[0].ok, "家目录里的普通文件应当读得到");
    assert!(!turn.traces[1].ok, "密钥库必须被拒");
    assert!(turn.traces[2].ok, "日志应当读得到（决策 226）");
    // 拒绝的事实在对话里可见：回灌给模型的是错误原文，模型能转述给人。
    // （判据取**模型收到的消息**，不是库里的 foreman_messages——后者只存人机两边的话，
    //  工具结果不进那一层，这样历史裁的时候也不会被一条工具报错顶掉一句人话。）
    let log = requests.request_log();
    let tail = serde_json::to_string(&log.last().unwrap().messages).unwrap();
    assert!(tail.contains("拒绝名单"), "{tail}");
    assert!(tail.contains("NOTES.md"), "成功的那次读取应当真有内容");
    assert!(
        tail.contains("日志一行"),
        "日志读得到，内容就该真的进对话（否则那条拒绝撤了也没用）：{tail}"
    );
    assert!(
        !tail.contains("sk-super-secret-value"),
        "被拒的读取不得把内容带进对话：{tail}"
    );
}

/// D 层（本服务写接口）**不读档位**：配成 `auto` 也只放开环境层，写接口照旧只是提议。
///
/// 这是决策 206 的硬规矩「本服务的写接口需要人按确认钮」在代码里的落点。三条名字各自
/// 提一条，且**什么都还没发生**（库里没有新任务）。
#[tokio::test]
async fn the_service_write_tools_propose_even_under_the_auto_tier() {
    let h = Harness::seeded().await;
    h.store
        .upsert_stage_config(&StageConfig {
            stage: FOREMAN_STAGE_KEY.to_string(),
            env_mode: Some(agentpipeline_core::types::EnvMode::Auto),
            ..Default::default()
        })
        .await
        .unwrap();
    // 一条既有的阶段覆盖：后面要断言它**还在**（否则「还在」是一句空话）。
    h.store
        .upsert_stage_config(&StageConfig {
            stage: "develop".into(),
            max_tokens: Some(4096),
            ..Default::default()
        })
        .await
        .unwrap();

    let mut script = Script::new();
    script.for_foreman().tool(
        "task",
        serde_json::json!({"action": "cancel", "task_id": "t1"}),
    );
    script.for_foreman().tool(
        "config",
        serde_json::json!({"action": "delete", "stage": "develop"}),
    );
    script.for_foreman().tool(
        "skills",
        serde_json::json!({"action": "delete", "name": "whatever"}),
    );
    script.for_foreman().text("三条都提了，等你按键。");
    let runner = h.runner(FakeAgent::new(script));
    let turn = runner.say(None, "把 t1 停了").await.unwrap();

    assert_eq!(turn.traces.len(), 3);
    for trace in &turn.traces {
        assert!(
            trace.ok,
            "{} 被提成提议应当记为「成功调用」（它不是工具失败）：{:?}",
            trace.tool, trace
        );
    }
    let pending = h
        .store
        .list_pending_foreman_proposals(&turn.session.id)
        .await
        .unwrap();
    let tools: Vec<&str> = pending.iter().map(|p| p.tool.as_str()).collect();
    assert_eq!(tools, ["task", "config", "skills"], "{pending:?}");
    // D 层恒提议的**唯一例外**是「托管中的任务 + resume(continue)」（决策 210② / 票 08）；
    // 这里的三个动作一个都不在例外里，且本例既没开托管、也没注入执行者——**不注入不放行**。
    // 例外的正面与反面断言见本文件「任务级托管与 D 层例外」那一节。
    //
    // 取证：什么状态都没变——任务还在跑、阶段配置还在、技能目录里那个文件也还在。
    let task = h.store.get_task("t1").await.unwrap();
    assert_ne!(task.status, TaskStatus::Cancelled, "提议不是执行");
    assert!(
        h.store.get_stage_config("develop").await.unwrap().is_some(),
        "config delete 只是提议，覆盖行不该消失"
    );
}

/// 任务族的提议**带着态势指纹**（决策 207 的拒执判据），且指纹取自**参数**里的 task_id。
///
/// 这一条盯的是一个会让整条拒执规则静默失效的错法：照**调用上下文**取 task_id 的话，值班长
/// 那侧恒为空串 → 指纹永远是 `None` → 「执行时情况变了就拒执」再也拦不住任何东西，而所有
/// 用例照旧全绿（端点那条路是手工塞 `situation` 进去测的）。
#[tokio::test]
async fn a_task_proposal_carries_a_situation_fingerprint() {
    let h = Harness::seeded().await;
    let session = h.session().await;
    let mut script = Script::new();
    script.for_foreman().tool(
        "task",
        serde_json::json!({"action": "cancel", "task_id": "t1"}),
    );
    script.for_foreman().text("提了，等你按键。");
    let runner = h.runner(FakeAgent::new(script));
    runner.say(Some(&session), "把 t1 停了").await.unwrap();

    let pending = h
        .store
        .list_pending_foreman_proposals(&session)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);
    let situation = pending[0]
        .situation
        .as_ref()
        .expect("任务族的提议必须存下当时那份态势（决策 207 的拒执判据）");
    assert_eq!(situation["status"], "queued", "{situation}");
    // 与当场取的那一份**逐字相同**（同一处实现，不是各算一份）
    assert_eq!(
        situation,
        &situation_fingerprint(&h.store, "t1").await.unwrap()
    );

    // 文件 / 命令族的提议**没有**态势可判——它们的成立与否由端点自己的校验回答
    let mut script = Script::new();
    script.for_foreman().tool(
        "write_file",
        serde_json::json!({"path": "a.md", "content": "x"}),
    );
    script.for_foreman().text("提了。");
    let runner = h.runner(FakeAgent::new(script));
    runner.say(Some(&session), "写个文件").await.unwrap();
    let all = h
        .store
        .list_pending_foreman_proposals(&session)
        .await
        .unwrap();
    let file_proposal = all
        .iter()
        .find(|p| p.tool == "write_file")
        .expect("写文件那条也该在");
    assert!(file_proposal.situation.is_none());
}

/// 操作台记的那几轮（`role = system`）**不得被读回成值班长自己的话**（决策 207）。
///
/// 写成助理轮，模型下一轮读历史时会把「提议已执行：写文件 notes.md」当成自己说过的话——那正是
/// 人格第一条纪律（不得声称自己动了手）要挡的东西。**也不能整段丢掉**：丢掉它，模型不知道人
/// 按了什么键，会以为提议还挂着、于是重提一遍。
#[tokio::test]
async fn the_operation_log_is_not_read_back_as_the_foremans_own_words() {
    let h = Harness::seeded().await;
    let session = h.session().await;
    h.store
        .append_foreman_user_message(&session, "第一句")
        .await
        .unwrap();
    h.store
        .append_foreman_message(NewForemanMessage::assistant(&session, "收到了。"))
        .await
        .unwrap();
    h.store
        .append_foreman_message(NewForemanMessage::system(
            &session,
            "提议已执行：写入文件 notes.md",
        ))
        .await
        .unwrap();

    let mut script = Script::new();
    script.for_foreman().text("知道了。");
    let agent = FakeAgent::new(script);
    let requests = agent.clone();
    let runner = h.runner(agent);
    runner.say(Some(&session), "第二句").await.unwrap();

    let messages = requests.request_log()[0].messages.clone();
    let as_assistant: Vec<&str> = messages
        .iter()
        .filter(|m| m.role == agentpipeline_core::agent::client::Role::Assistant)
        .filter_map(|m| m.content.as_deref())
        .collect();
    assert!(
        !as_assistant.iter().any(|c| c.contains("提议已执行")),
        "操作台记的账不得以值班长的口吻回灌：{as_assistant:?}"
    );
    // 但它**在场**，且带着说明发言者的标记
    let fed = messages
        .iter()
        .filter_map(|m| m.content.as_deref())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(fed.contains("提议已执行"), "{fed}");
    assert!(fed.contains(OPERATION_LOG_MARK), "{fed}");
}

/// 大结果卸载落**会话维度**，且**不写** `{root}/tasks/.context`（票 03 的硬规矩）。
///
/// 空 task_id 若被当成「根目录下的 `.context`」，落点就是所有任务共用的那一层——
/// 下一次运行任意一个真实任务时会把它读成自己的工作区残留。
#[tokio::test]
async fn big_results_offload_into_the_session_dimension() {
    let h = Harness::seeded().await;
    let session = h.session().await;
    // 一个超阈值（4000 token ≈ 16000 字符）的技能正文
    let big = "第 x 行：这是一段很长的技能正文。\n".repeat(1_200);
    testkit::write_skill_dir(&h._home.home().skills_dir(), "big", &big, &[]);

    let mut script = Script::new();
    script
        .for_foreman()
        .tool("Skill", serde_json::json!({"name": "big"}));
    script
        .for_foreman()
        .text("那个技能正文很长，已经在台账里了。");
    let runner = h.runner(FakeAgent::new(script));
    let turn = runner
        .say(Some(&session), "big 技能讲什么？")
        .await
        .unwrap();
    assert_eq!(turn.traces.len(), 1);
    assert!(turn.traces[0].ok);

    // 卸载落在会话维度：`{root}/foreman/context/{session}/`
    let dir = h._home.home().foreman_context_dir(&turn.session.id);
    let offloaded: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("会话卸载目录不存在（{}）：{e}", dir.display()))
        .collect();
    assert_eq!(offloaded.len(), 1, "大结果应当落一份到会话卸载目录");

    // 硬规矩：**不写** `{root}/tasks/.context`
    let shared = h._home.home().tasks_dir().join(".context");
    assert!(
        !shared.exists(),
        "空 task_id 不得落到共享的 tasks/.context：{}",
        shared.display()
    );
}

/// 回灌给模型的那份是**预览 + 路径**，不是全文——卸载的意义就在于把正文移出上下文。
#[tokio::test]
async fn the_offloaded_result_keeps_only_a_preview_in_context() {
    let h = Harness::seeded().await;
    let session = h.session().await;
    let big = "第 x 行：这是一段很长的技能正文。\n".repeat(1_200);
    testkit::write_skill_dir(&h._home.home().skills_dir(), "big", &big, &[]);

    let mut script = Script::new();
    script
        .for_foreman()
        .tool("Skill", serde_json::json!({"name": "big"}));
    script.for_foreman().text("读到了。");
    let agent = FakeAgent::new(script);
    let requests = agent.clone();
    let runner = h.runner(agent);
    runner
        .say(Some(&session), "big 技能讲什么？")
        .await
        .unwrap();

    let log = requests.request_log();
    let last = log.last().unwrap();
    let tail = serde_json::to_string(&last.messages).unwrap();
    assert!(
        tail.contains("foreman/context/"),
        "回灌的应当是卸载路径：{}",
        &tail[tail.len().saturating_sub(600)..]
    );
    assert!(
        tail.len() < big.len(),
        "全文不得进上下文（{} vs {}）",
        tail.len(),
        big.len()
    );
}

// ─────────────── unstick 进托管自动集、重启只提议（决策 210⑧ / 票 09）───────────────

#[tokio::test]
async fn unstick_is_in_the_steward_auto_set_but_restart_never_is() {
    let h = Harness::seeded().await;
    stewarded_task(&h, Some(Stewardship::enabled_now(h.clock.now()))).await;

    // ① unstick 进托管自动集（它与 resume 是两回事，但同一理由：只动一个任务、可逆）
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut script = Script::new();
    script.for_foreman().tool(
        "task",
        serde_json::json!({"action": "unstick", "task_id": "t1"}),
    );
    script.for_foreman().text("解开了。");
    let runner = h.runner_with_steward(script, calls.clone());
    let turn = runner.say(None, "它卡住了").await.unwrap();
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "unstick 在托管自动集里"
    );
    assert!(h
        .store
        .list_pending_foreman_proposals(&turn.session.id)
        .await
        .unwrap()
        .is_empty());

    // ② 「重启服务」**永不**自动——它是全局动作（会打断所有在跑的任务）
    for steward in [true, false] {
        if steward {
            stewarded_task(&h, Some(Stewardship::enabled_now(h.clock.now()))).await;
        }
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut script = Script::new();
        script
            .for_foreman()
            .tool("service", serde_json::json!({"action": "restart"}));
        script.for_foreman().text("提了，等你按键。");
        let runner = h.runner_with_steward(script, calls.clone());
        // 两种托管状态各起一班（同上的理由：提议不能跨轮累加）
        let sid = h.session().await;
        let turn = runner.say(Some(&sid), "重启一下").await.unwrap();
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "重启永远只提议（托管={steward}）"
        );
        let pending = h
            .store
            .list_pending_foreman_proposals(&turn.session.id)
            .await
            .unwrap();
        assert_eq!(pending.len(), 1, "重启是一条提议：{pending:?}");
        assert_eq!(pending[0].tool, "service");
        // 文案要说清影响面：值班经理按下之前唯一能读到的就是这一行
        assert!(
            pending[0].summary.contains("打断"),
            "重启提议的说明必须说清会打断在跑的任务：{}",
            pending[0].summary
        );
    }
}

// ───────────────────── 修复轮的线上入口（决策 210③④ / 票 10–12）─────────────────────

/// 修复这条链**在线上跑得通**：`repair(action=start)` 真拉起一个 worktree 并把它交回，
/// `finish` 真把它收口成一条待按的提议。
///
/// 为什么这条非要走工具（而不是像 `tests/repair.rs` 那样直接调 `pipeline::repair`）：票 10–12
/// 的实现全都写好了，却**没有任何生产调用者**——「通过闸门的测试」与「线上跑得通」之间的
/// 差额恰好就是这个入口。故这里断言的是那条链路本身：工具 → 分派 → worktree 落在值班长的
/// 写域里 → 改动能单独成 commit → 提议出现在待办列表里。
#[tokio::test]
async fn the_repair_tool_opens_a_worktree_and_lands_a_proposal() {
    use agentpipeline_core::storage::proposals::ForemanProposalKind;
    use agentpipeline_core::types::EnvMode;

    let h = Harness::empty().await;
    let repo = Repo::clean().unwrap();
    h.git_project(&repo, "true").await;
    h.task("t1").await;
    // 这条任务得**真的停在 pending 上**——「等修复合入」是加在它的 pending 说明里的，
    // 而一条没停下来的任务不该因为顺手提了个修复就显示成「在等」（下面正面断言那句原文还在）。
    park_task(&h.store, "t1", PendingKind::UserDecision, "需要人定夺").await;
    // `repair` 归**环境层**（决策 206 的 C 层、210③）：默认档位 `ask` 下它会变成一条提议，
    // 而这一条要断言的是「worktree 真的被拉起来了」。档位就是这件事的开关。
    h.foreman_env(EnvMode::Auto).await;

    // —— 第一轮：start ——
    let mut script = Script::new();
    script.for_foreman().tool(
        "repair",
        serde_json::json!({"action": "start", "project_id": "p1"}),
    );
    script.for_foreman().text("去修。");
    let turn = h
        .runner(FakeAgent::new(script))
        .say(None, "p1 有个毛病")
        .await
        .unwrap();
    assert_eq!(turn.traces.len(), 1);
    assert_eq!(turn.traces[0].tool, "repair");
    assert!(turn.traces[0].ok, "start 应当真的拉起 worktree");
    let session_id = turn.session.id.clone();

    // worktree 真的在（而不是只回了一句「已就绪」）：`repair-{id}` 目录下有自己的 `.git`
    let worktree = only_repair_worktree(&h);
    assert!(
        worktree.join(".git").exists(),
        "start 必须真建出 worktree：{}",
        worktree.display()
    );
    assert!(!repo.is_dirty(), "start 不许碰项目工作区");
    let repair_id = worktree
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_prefix("repair-"))
        .expect("worktree 目录名是 `repair-{id}`（命名自洽是重建现场的前提）")
        .to_string();

    // 值班长在这一轮里「改代码」：它写的是 worktree 里那个路径（家目录的写域之内）
    std::fs::write(worktree.join("fixed.rs"), "pub fn fixed() -> i32 { 42 }\n").unwrap();

    // —— 第二轮：finish ——
    let mut script = Script::new();
    script.for_foreman().tool(
        "repair",
        serde_json::json!({
            "action": "finish",
            "project_id": "p1",
            "repair_id": repair_id,
            "conclusion": "补上缺的约束",
            "task_id": "t1",
        }),
    );
    script.for_foreman().text("改完了，等你按。");
    let turn = h
        .runner(FakeAgent::new(script))
        .say(Some(&session_id), "改完了吗")
        .await
        .unwrap();
    assert_eq!(turn.traces.len(), 1);
    assert_eq!(turn.traces[0].tool, "repair");
    assert!(turn.traces[0].ok, "finish 应当收口成功");

    // 收口的三样凭证：待办里一条提议、分支上一个带标记的 commit、改动**没进主干**
    let pending = h
        .store
        .list_pending_foreman_proposals(&session_id)
        .await
        .unwrap();
    assert_eq!(
        pending.len(),
        1,
        "闸门过了就该落一条待按的提议：{pending:?}"
    );
    assert_eq!(pending[0].kind, ForemanProposalKind::Repair);
    assert_eq!(pending[0].tool, "repair");
    assert!(pending[0].payload.is_some(), "载荷里要有 diff 与闸门读数");
    let branch = format!("repair/{repair_id}");
    let message = repo.git(&["log", "--format=%s", "-1", &branch]);
    assert!(
        message.contains("[repair]"),
        "修复 commit 要带可检索的标记：{message}"
    );
    assert!(
        !repo.exists("fixed.rs"),
        "合入由人按：在那之前主干上不该有这个改动"
    );
    // 两处留痕（票 11 的最后一格）：班次里一条，**任务行上也一条**。光有时间线里的提议，
    // 人在看板上盯着那条任务，看不出它卡在哪儿。
    let messages = h
        .store
        .list_foreman_messages(&session_id, 50, None)
        .await
        .unwrap();
    assert!(
        messages
            .iter()
            .any(|m| m.content.contains("【等修复合入】") && m.content.contains("t1")),
        "传了 task_id 就要在班次里留「等修复合入」的字样：{:?}",
        messages.iter().map(|m| &m.content).collect::<Vec<_>>()
    );
    let task = h.store.get_task("t1").await.unwrap();
    let reason = task.pending_reason.expect("任务应当仍停在 pending 上");
    assert!(
        reason.message.contains("等修复合入"),
        "任务本身上要看得出它在等什么：{}",
        reason.message
    );
    assert!(
        reason.message.contains("需要人定夺"),
        "**原来那句不能丢**——修复改变的是「现在等什么」，不是「为什么停」：{}",
        reason.message
    );
    assert_eq!(
        reason.kind,
        PendingKind::UserDecision,
        "pending 的种类照旧：它是投影的来源，还参与 resume 的原因归类"
    );
}

/// `discard`：不修了就回收——**保留分支**（它是唯一的证据），并覆盖「又调了一次」那一支。
///
/// 与「没人按」那条回收路是两回事：那一条是维护作业在行被删之前收的（票 12），这一条是
/// 值班长**自己**放弃这次尝试。两条都收 worktree、都留分支，故收尾规则只有一个答案。
#[tokio::test]
async fn the_repair_tool_discards_an_abandoned_attempt() {
    use agentpipeline_core::git::Git;
    use agentpipeline_core::types::EnvMode;

    let h = Harness::empty().await;
    let repo = Repo::clean().unwrap();
    h.git_project(&repo, "true").await;
    h.foreman_env(EnvMode::Auto).await;

    let mut script = Script::new();
    script.for_foreman().tool(
        "repair",
        serde_json::json!({"action": "start", "project_id": "p1"}),
    );
    script.for_foreman().text("拿到 worktree 了。");
    let turn = h
        .runner(FakeAgent::new(script))
        .say(None, "试试修一下")
        .await
        .unwrap();
    assert!(turn.traces[0].ok);
    let session_id = turn.session.id.clone();
    let worktree = only_repair_worktree(&h);
    let repair_id = worktree
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_prefix("repair-"))
        .unwrap()
        .to_string();

    // ① 放弃：worktree 没了，分支还在
    let mut script = Script::new();
    script.for_foreman().tool(
        "repair",
        serde_json::json!({
            "action": "discard", "project_id": "p1", "repair_id": repair_id,
        }),
    );
    script.for_foreman().text("收掉了。");
    let turn = h
        .runner(FakeAgent::new(script))
        .say(Some(&session_id), "不修了")
        .await
        .unwrap();
    assert!(turn.traces[0].ok, "discard 应当回收成功");
    assert!(!worktree.exists(), "worktree 应当被删掉");
    assert!(
        Git.rev_parse(repo.path(), &format!("repair/{repair_id}"))
            .await
            .is_ok(),
        "分支必须留着——它是这次尝试唯一的证据"
    );

    // ② 又调一次：回一句话而不是让 git 报错（「重复调一次」不该看起来像工具坏了）
    let mut script = Script::new();
    script.for_foreman().tool(
        "repair",
        serde_json::json!({
            "action": "discard", "project_id": "p1", "repair_id": repair_id,
        }),
    );
    script.for_foreman().text("收过了。");
    let turn = h
        .runner(FakeAgent::new(script))
        .say(Some(&session_id), "再收一次")
        .await
        .unwrap();
    assert!(
        turn.traces[0].ok,
        "已经不在的 worktree 上再 discard 一次不是工具故障"
    );
}

/// 家目录下那个唯一的修复 worktree（`gate-output-*.log` 与 `*.diff` 都不算）。
fn only_repair_worktree(h: &Harness) -> std::path::PathBuf {
    let mut found: Vec<std::path::PathBuf> = std::fs::read_dir(h._home.home().worktrees_dir())
        .expect("worktrees 目录应当已被 start 建出来")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("repair-"))
        })
        .collect();
    assert_eq!(found.len(), 1, "应当恰好一个修复 worktree：{found:?}");
    found.pop().unwrap()
}

// ─────────────── 归因结构块（决策 227 / 235 / 238，票 03）───────────────

/// 结构块的解析判据（判决 235 的「四类之外不许收口」+ 238 的「同一个解析点」）。
///
/// 这些是纯函数用例：解析点是这一批裁定的**唯一校验面**，它上面每一条分支都得有名字——
/// 否则「必填」与「不许越界」都只是人格里的一句话（决策 235 的起因正是一次
/// 「格式漂亮但不可校验」的回话）。
mod attribution {
    use agentpipeline_core::pipeline::foreman::{parse_attribution, Attribution, AttributionKind};

    #[test]
    fn a_valid_block_locates_the_category() {
        // 稳定标识与中文词都认（模型写出中文词是正常事，不认只会多一条假未定位）。
        for (written, expected) in [
            ("host", AttributionKind::Host),
            ("pipeline", AttributionKind::Pipeline),
            ("project_code", AttributionKind::ProjectCode),
            ("prompt_config", AttributionKind::PromptConfig),
            ("宿主环境", AttributionKind::Host),
            ("流水线运行", AttributionKind::Pipeline),
            ("目标项目代码", AttributionKind::ProjectCode),
            ("prompt 与配置", AttributionKind::PromptConfig),
        ] {
            let text =
                format!("它卡在 test.execute 上。\n【归因】{{\"attribution\":\"{written}\"}}\n");
            assert_eq!(
                parse_attribution(&text),
                Attribution::Located {
                    kind: expected,
                    run_id: None,
                },
                "写的是 {written}"
            );
        }
    }

    #[test]
    fn a_reply_without_the_block_is_missing_not_a_guess() {
        let text = "我查了台账，看不出问题在哪。";
        assert_eq!(parse_attribution(text), Attribution::Missing);
        // 光有哨兵没有载荷也算没给（「根本没给」与「四类之外」都由这一个点判出）。
        assert_eq!(
            parse_attribution("【归因】\n"),
            Attribution::Invalid {
                payload: String::new(),
                why: "JSON 解析失败",
            }
        );
    }

    #[test]
    fn anything_outside_the_four_does_not_close() {
        // 四类之外：不许收口（决策 235 的核心判据）。
        let outside = "【归因】{\"attribution\":\"unknown_thing\"}\n";
        assert_eq!(
            parse_attribution(outside),
            Attribution::Invalid {
                payload: "{\"attribution\":\"unknown_thing\"}".into(),
                why: "四类之外",
            }
        );
        assert!(!parse_attribution(outside).is_located());
        // 坏 JSON / 缺字段：同样是未定位，但原因分得开（排查要看得出是哪一种）。
        assert!(matches!(
            parse_attribution("【归因】宿主环境\n"),
            Attribution::Invalid {
                why: "JSON 解析失败",
                ..
            }
        ));
        assert!(matches!(
            parse_attribution("【归因】{\"kind\":\"host\"}\n"),
            Attribution::Invalid {
                why: "缺 attribution 字段",
                ..
            }
        ));
    }

    #[test]
    fn repeated_or_contradictory_blocks_do_not_close() {
        // 合规的一次 + 夹带的一次不合规：按**最严的一处**判——不让人靠夹带一个合规字样收口。
        let smuggled =
            "【归因】{\"attribution\":\"pipeline\"}\n【归因】{\"attribution\":\"whatever\"}\n";
        assert!(matches!(
            parse_attribution(smuggled),
            Attribution::Invalid {
                why: "四类之外",
                ..
            }
        ));
        // 结构块**以整行出现**（行首哨兵）才是块；行文里提一句哨兵不算给（那是散文，
        // 不参与判定）。这条规则让「夹带」只有一种形态——再写一行块，而那一行照样按最严判。
        let inline = "上一轮我给的【归因】是 {\"attribution\":\"whatever\"}，但这次是 pipeline。\n\
                      【归因】{\"attribution\":\"pipeline\"}\n";
        assert_eq!(
            parse_attribution(inline),
            Attribution::Located {
                kind: AttributionKind::Pipeline,
                run_id: None,
            }
        );
        // 两处互相矛盾：自相矛盾不是结论。
        let contradictory =
            "【归因】{\"attribution\":\"host\"}\n【归因】{\"attribution\":\"pipeline\"}\n";
        assert!(matches!(
            parse_attribution(contradictory),
            Attribution::Invalid {
                why: "多处自相矛盾",
                ..
            }
        ));
        // 同一类写两遍不算矛盾（复述是正常行文；一次写稳定标识、一次写中文词也对得上）。
        let repeated =
            "【归因】{\"attribution\":\"host\"}\n【归因】{\"attribution\":\"宿主环境\"}\n";
        assert_eq!(
            parse_attribution(repeated),
            Attribution::Located {
                kind: AttributionKind::Host,
                run_id: None,
            }
        );
    }

    /// 判据①的校验面：回话**自己指名的 run**（决策 230 的「证据归错 run 与没有证据同判失败」）。
    ///
    /// 2026-09-19 那次翻车的形状是「行文与类别都合规、只是把 run 27 的活栈记在 run 26 名下」，
    /// 而当时没有任何断言拦得住它——因为结构块里只有一个类别字段，装不下「这次说的是哪条 run」。
    #[test]
    fn a_named_run_travels_with_the_category() {
        assert_eq!(
            parse_attribution("【归因】{\"attribution\":\"pipeline\",\"run_id\":27}\n"),
            Attribution::Located {
                kind: AttributionKind::Pipeline,
                run_id: Some(27),
            }
        );
        // 指不出单条 run 的播报（任务级态势）**不写**就是诚实的：不逼它编一个数。
        assert_eq!(
            parse_attribution("【归因】{\"attribution\":\"pipeline\"}\n").run_id(),
            None
        );
        assert_eq!(
            parse_attribution("【归因】{\"attribution\":\"pipeline\",\"run_id\":null}\n").run_id(),
            None
        );
        // 两处类别一致而 run 不同：**那不是复述，是两处互相矛盾**——正是要拦的形状。
        assert!(matches!(
            parse_attribution(
                "【归因】{\"attribution\":\"host\",\"run_id\":26}\n\
                 【归因】{\"attribution\":\"host\",\"run_id\":27}\n"
            ),
            Attribution::Invalid {
                why: "多处自相矛盾",
                ..
            }
        ));
        // 一字不差地重复同一处（含同一个 run）仍是复述。
        assert_eq!(
            parse_attribution(
                "【归因】{\"attribution\":\"host\",\"run_id\":26}\n\
                 【归因】{\"attribution\":\"host\",\"run_id\":26}\n"
            )
            .run_id(),
            Some(26)
        );
        // `0` / 负数 / 字符串都判非法：`0` 在库里同样是「没有锚点」的值，照单全收会把
        // 「按 run 对账」变成一个假读数（与决策 231 的哨兵归一同一姿态）。
        for bad in ["0", "-3", "\"27\"", "1.5", "true"] {
            let text = format!("【归因】{{\"attribution\":\"pipeline\",\"run_id\":{bad}}}\n");
            assert!(
                matches!(
                    parse_attribution(&text),
                    Attribution::Invalid {
                        why: "run_id 不是正整数",
                        ..
                    }
                ),
                "写的是 {bad}"
            );
        }
    }

    #[test]
    fn the_wire_string_never_invents_a_category() {
        assert_eq!(
            parse_attribution("【归因】{\"attribution\":\"project_code\"}").wire(),
            "project_code"
        );
        assert_eq!(parse_attribution("没有块").wire(), "unlocated");
        assert_eq!(parse_attribution("没有块").reason(), Some("missing"));
        assert_eq!(
            parse_attribution("【归因】{\"attribution\":\"x\"}").reason(),
            Some("四类之外")
        );
    }
}

#[tokio::test]
async fn the_diagnosis_pack_carries_the_last_attribution() {
    // 决策 235③：`read_diagnosis` 顺手带出**最近一次**的归因类别——不然下一轮又要重新问一遍
    // 自己「上次我怎么定的性」，而那一问往往又值一次模型往返。
    let h = Harness::seeded().await;
    park_task(&h.store, "t1", PendingKind::RetryExhausted, "重试耗尽").await;

    let mut script = Script::new();
    script.for_foreman().text(
        "它卡在 open() 上，是宿主环境拦的。\n【归因】{\"attribution\":\"host\",\"run_id\":41}\n",
    );
    script.for_foreman().read_diagnosis("t1");
    script
        .for_foreman()
        .text("结论同上。\n【归因】{\"attribution\":\"host\",\"run_id\":41}\n");
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());

    runner.say(None, "t1 怎么了？").await.unwrap();
    // 第一轮的回话（带结构块）已经落库，于是第二轮调诊断包时它是「最近一次」。
    runner.say(None, "再说一遍").await.unwrap();

    let fed_back = agent.request_log()[2]
        .messages
        .iter()
        .filter_map(|m| m.content.as_deref())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        fed_back.contains("latest_attribution"),
        "诊断包要有上一轮的归因那一节：{fed_back}"
    );
    assert!(
        fed_back.contains("\"attribution\": \"host\""),
        "上一轮的类别要带出来：{fed_back}"
    );
    assert!(
        fed_back.contains("宿主环境"),
        "给人看的那个词也要在（模型读得懂中文词）：{fed_back}"
    );
    // 判据① 的复查面：上一轮指名的 run 也带出来，下一轮才能拿它与台账对账（决策 230）。
    assert!(
        fed_back.contains("\"run_id\": 41"),
        "上一轮指名的 run 要带出来：{fed_back}"
    );
}

#[tokio::test]
async fn a_missing_attribution_reads_as_unlocated_in_the_pack() {
    // 「上一轮我没给出类别」本身是要看见的事实（决策 230 把「没有证据」与「证据归错 run」
    // 同判失败）。故未定位不是**没有这一节**，而是这一节里写着 unlocated + 原因。
    let h = Harness::seeded().await;
    park_task(&h.store, "t1", PendingKind::RetryExhausted, "重试耗尽").await;

    let mut script = Script::new();
    script.for_foreman().text("我还在看。");
    script.for_foreman().read_diagnosis("t1");
    script.for_foreman().text("还是没定下来。");
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());
    runner.say(None, "t1 怎么了？").await.unwrap();
    runner.say(None, "再说一遍").await.unwrap();

    let fed_back = agent.request_log()[2]
        .messages
        .iter()
        .filter_map(|m| m.content.as_deref())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        fed_back.contains("\"attribution\": \"unlocated\""),
        "没有类别就说未定位，不编一个：{fed_back}"
    );
    assert!(
        fed_back.contains("\"reason\": \"missing\""),
        "原因要分得开（missing 与四类之外不是一回事）：{fed_back}"
    );
    assert!(
        fed_back.contains("\"run_id\": null"),
        "没定下来时也没有 run 可指，读数是 null 而不是编一个：{fed_back}"
    );
}

#[tokio::test]
async fn read_stage_configs_echoes_the_node_overrides() {
    // 决策 236：`config set` 是整条替换，而 `read_stage_configs` 此前不回显 `node_overrides`
    // ——「改之前先看」这条既有纪律于是**执行不了**（2026-09-18 值班长因此拒提配置改动，
    // 它明说「这一改我看不全现状」：那不是保守，是没有可看的东西）。
    let h = Harness::seeded().await;
    let overrides = serde_json::json!({
        "validate_input": {"skills": ["grilling"]},
        "execute": {"skills": ["to-spec"]},
    });
    h.store
        .upsert_stage_config(&StageConfig {
            stage: Stage::ArchitectDesign.as_str().to_string(),
            skills_json: Some(serde_json::json!(["domain-modeling"])),
            node_overrides_json: Some(overrides.clone()),
            persona_append: Some("简短。".into()),
            ..Default::default()
        })
        .await
        .unwrap();

    let mut script = Script::new();
    script
        .for_foreman()
        .tool("read_stage_configs", serde_json::json!({}));
    script.for_foreman().text("看清了。");
    let agent = FakeAgent::new(script);
    let turn = h
        .runner(agent.clone())
        .say(None, "现在是什么情况？")
        .await
        .unwrap();
    assert!(turn.traces[0].ok);

    let fed_back = agent.request_log()[1]
        .messages
        .iter()
        .filter_map(|m| m.content.as_deref())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        fed_back.contains("node_overrides_json"),
        "回显里要有 node_overrides：{fed_back}"
    );
    for node in ["validate_input", "execute", "to-spec", "grilling"] {
        assert!(
            fed_back.contains(node),
            "节点级覆盖的 {node} 要看得见（照着带回来才带得全）：{fed_back}"
        );
    }
    // `persona_append` 与 `env_mode` 同批补：它们也是「留空即清成默认」会动的东西。
    assert!(fed_back.contains("persona_append"), "{fed_back}");
    assert!(fed_back.contains("env_mode"), "{fed_back}");
}

/// 同任务 30 分钟冷却**必须对 `run_failed` 真的生效**（决策 234 点名的风险）。
///
/// 为什么单列一条：`run_failed` 是逐 run 的事件，而**重试型故障**会连着出好几条
/// （实测那次三次同形状失败）。冷却不生效的话，一次重试型故障就能把每小时 12 次的唤醒
/// 配额烧光——而那个配额保护的正是「唤醒是花钱的、且是在没人在场的时候花」。
#[tokio::test]
async fn run_failures_of_the_same_task_are_collapsed_by_the_cooldown() {
    let h = Harness::seeded().await;
    note(
        &h,
        "t1",
        agentpipeline_core::storage::AttentionKind::RunFailed,
    )
    .await;
    h.clock.advance_secs(61);

    let mut script = Script::new();
    script.for_foreman().text("第一次播报：t1 的 run 挂了");
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());
    assert!(runner.watch().await.unwrap().is_some(), "第一次要醒");
    assert_eq!(agent.total_calls(), 1);

    // 同一任务又来一条失败的 run（occurred_at 不同，故去重键不会把它折叠掉）
    h.clock.advance_secs(60);
    note(
        &h,
        "t1",
        agentpipeline_core::storage::AttentionKind::RunFailed,
    )
    .await;
    h.clock.advance_secs(61);
    assert!(
        runner.watch().await.unwrap().is_none(),
        "冷却期内不再单独唤醒（事件仍在表里，等冷却到期合并播报）"
    );
    assert_eq!(agent.total_calls(), 1, "没有第二次模型调用");

    // 冷却到期后合并播报：两条事件一起进去
    h.clock.advance_secs(30 * 60);
    let mut second = Script::new();
    second.for_foreman().text("第二次播报：补上后面那条");
    agent.set_script(second);
    assert!(runner.watch().await.unwrap().is_some());
    assert_eq!(agent.total_calls(), 2);
    assert!(h.store.open_attention(100).await.unwrap().is_empty());
}

// ─────── 一轮的收尾语义（决策 233②③ / 239，票 06）───────

/// 当场报错的替身：这一轮跑不起来。
struct FailingLlm;

impl LlmClient for FailingLlm {
    fn complete(
        &self,
        _request: LlmRequest,
    ) -> futures::future::BoxFuture<'static, agentpipeline_core::Result<AgentResponse>> {
        Box::pin(async { Err(Error::Llm("连接被对端关掉".into())) })
    }
}

/// 一边查一边说、从不收口的替身：每次响应都带同一句话与一个 tool_call。
///
/// 为什么要自己写一个：FakeAgent 的脚本步**要么文本、要么工具**（`Step::Text` /
/// `Step::Tool`），而「触顶时还有话说」这件事只出现在**两者同时返回**的那一支上
/// ——那正是决策 233② 要留下东西的那一支。
struct ChattyForever {
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

impl LlmClient for ChattyForever {
    fn complete(
        &self,
        _request: LlmRequest,
    ) -> futures::future::BoxFuture<'static, agentpipeline_core::Result<AgentResponse>> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async move {
            Ok(AgentResponse {
                content: Some("到目前为止：它挂在 test.execute 上，栈落在 parse_chunk。".into()),
                tool_calls: vec![agentpipeline_core::agent::client::ToolCall {
                    id: "c1".into(),
                    name: "read_task".into(),
                    arguments: r#"{"task_id":"t1"}"#.into(),
                }],
                prompt_tokens: 10,
                completion_tokens: 5,
                ..Default::default()
            })
        })
    }
}

/// 触到上限**不再整轮作废**：把已确定的部分落库并标注（决策 233②）。
#[tokio::test]
async fn a_capped_turn_keeps_what_it_already_established() {
    let h = Harness::seeded().await;
    // 上限配成 3：既验「那个设置项真的生效」（决策 233① / 239），也让用例不必跑 300 轮。
    h.store
        .upsert_stage_config(&StageConfig {
            stage: FOREMAN_STAGE_KEY.to_string(),
            max_rounds: Some(3),
            ..Default::default()
        })
        .await
        .unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let runner = ForemanRunner::new(
        h.store.clone(),
        Settings::default(),
        h._home.home().clone(),
        Arc::new(ChattyForever {
            calls: calls.clone(),
        }) as Arc<dyn LlmClient>,
        Arc::new(testkit::SseRecorder::new()),
    );
    let sid = h.session().await;

    let turn = runner.say(Some(&sid), "盯着 t1").await.unwrap();
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        3,
        "轮数上限是设置项说的那个数（不是缺省的 300）"
    );
    assert!(
        turn.reply.contains("它挂在 test.execute 上"),
        "已经查到的东西要留下：{}",
        turn.reply
    );
    assert!(
        turn.reply.contains(FOREMAN_PARTIAL_TURN_MARK),
        "要标出「这不是结论」：{}",
        turn.reply
    );
    assert!(
        turn.reply.contains("3 轮"),
        "标注里要给出实际生效的上限：{}",
        turn.reply
    );
    // 票 02：**轮数上限**那条尾句也接产出注记。
    assert!(
        turn.reply.contains("本轮没有提任何提议，也没有改动文件。"),
        "轮数上限的尾句要按实际产出说话：{}",
        turn.reply
    );

    // 落库的是同一段（「那 30 轮其实查到了东西、却整段扔掉」是实测里最贵的一次浪费）。
    let stored = h.store.list_foreman_messages(&sid, 10, None).await.unwrap();
    assert!(
        stored
            .iter()
            .any(|m| m.role == "assistant" && m.content.contains(FOREMAN_PARTIAL_TURN_MARK)),
        "部分结论要落库：{stored:?}"
    );
    // 而且**不是**一轮失败：不落「没跑起来」的账。
    assert!(
        !stored
            .iter()
            .any(|m| m.role == "system" && m.content.contains(FOREMAN_FAILED_TURN_MARK)),
        "触顶不再是失败：{stored:?}"
    );
}

/// 两个缺省界**钉住**（决策 292 / 票 07）：它们是决策的值，不是随手取的数字。
///
/// 改它要连着改这一行与决策 292 的来历段（`FOREMAN_MAX_ROUNDS` 为什么抬到 1000、
/// 120k 是「实测 85,135 生成 token 的约 1.4 倍」）——照 `agent::loops::constants_are_pinned`
/// 与 `config::default_server_port_is_8788` 的姿态：**决策的取值由测试守着**，
/// 免得它在下一次重构里被静默调小。
#[test]
fn the_default_bounds_are_pinned() {
    assert_eq!(FOREMAN_MAX_ROUNDS, 1000);
    assert_eq!(FOREMAN_WATCH_TOKEN_BUDGET, 120_000);
}

/// 一句话都没说过的触顶**仍旧按失败处置**（没有东西可留，报错才是诚实的）。
///
/// 同 [`a_foreman_that_never_wraps_up_is_capped_and_named`]：脚本每轮查另一个 id，
/// 好让这个用例落在**轮数上限**那条路上（同参重复那条已由票 08 提前拦下）。
#[tokio::test]
async fn a_capped_turn_with_nothing_to_keep_is_still_a_failure() {
    let h = Harness::empty().await;
    let mut script = Script::new();
    for i in 0..FOREMAN_MAX_ROUNDS {
        script
            .for_foreman()
            .tool("read_task", serde_json::json!({"task_id": format!("t{i}")}));
    }
    let runner = h.runner(FakeAgent::new(script));
    let sid = h.session().await;

    let err = runner.say(Some(&sid), "盯着 t1").await.unwrap_err();
    assert_eq!(
        err.llm_classified().map(|(k, _)| k.to_string()).as_deref(),
        Some("model_no_reply"),
        "没有可留的东西时，类别与原来一致：{err}"
    );
}

/// 一边查一边说、到第 `stop_after` 次调用才收口的替身（票 07 的成本门用例）。
///
/// 与 [`ChattyForever`] 同一姿态：票 07 要验的三条收口路径都要求**中途说过话**
/// （`last_text` 有值），而 FakeAgent 的脚本步要么文本要么工具。这里多一格
/// 「第几次收口」，于是「人的那一轮不触顶、会自己说完」这件事也能表达。
struct TalkyThenStop {
    calls: Arc<std::sync::atomic::AtomicUsize>,
    /// 第几次调用换成纯文本收口（`None` = 一直干活）。
    stop_after: Option<usize>,
    /// 第几次调用**起**当场报错（`None` = 不报错；「起」而不是「那一次」——空闲判死那一类
    /// 会被 `complete_with_retry` 重试一次，只错一次的话那一轮会照旧跑下去），类别由
    /// `fail_kind` 给。
    fail_at: Option<usize>,
    fail_kind: &'static str,
    /// 每次调用的生成 token 读数（成本门按它算）。
    completion_tokens: u32,
}

impl LlmClient for TalkyThenStop {
    fn complete(
        &self,
        _request: LlmRequest,
    ) -> futures::future::BoxFuture<'static, agentpipeline_core::Result<AgentResponse>> {
        let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let (stop_after, fail_at, fail_kind, tokens) = (
            self.stop_after,
            self.fail_at,
            self.fail_kind,
            self.completion_tokens,
        );
        Box::pin(async move {
            if fail_at.is_some_and(|from| n >= from) {
                return Err(Error::LlmClassified {
                    kind: fail_kind.to_string(),
                    message: "（用例）这一轮中途断了".into(),
                    raw: "（用例）".into(),
                });
            }
            Ok(AgentResponse {
                content: Some(format!("到目前为止（第 {n} 步）：它挂在 test.execute 上。")),
                tool_calls: if stop_after == Some(n) {
                    Vec::new()
                } else {
                    vec![agentpipeline_core::agent::client::ToolCall {
                        id: format!("c{n}"),
                        name: "read_task".into(),
                        arguments: r#"{"task_id":"t1"}"#.into(),
                    }]
                },
                prompt_tokens: 10,
                completion_tokens: tokens,
                ..Default::default()
            })
        })
    }
}

/// 成本门**分档**（决策 292 / 票 07）：同一条线，值守轮触顶即停（部分结论 + 【未收口】），
/// 人的那一轮不触顶。
#[tokio::test]
async fn the_token_budget_stops_the_watch_round_but_never_the_human_one() {
    // 值守轮：预算配成 20、每次 5 token → 第 5 次调用之前就被拦住。
    let h = Harness::seeded().await;
    h.store
        .upsert_stage_config(&StageConfig {
            stage: FOREMAN_STAGE_KEY.to_string(),
            watch_token_budget: Some(20),
            ..Default::default()
        })
        .await
        .unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let runner = h.runner_with_llm(Arc::new(TalkyThenStop {
        calls: calls.clone(),
        stop_after: None,
        fail_at: None,
        fail_kind: "llm_idle_timeout",
        completion_tokens: 5,
    }) as Arc<dyn LlmClient>);
    note(
        &h,
        "t1",
        agentpipeline_core::storage::AttentionKind::RetryExhausted,
    )
    .await;
    h.clock.advance_secs(61);

    let turn = runner.watch().await.unwrap().expect("应当醒一次");
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        4,
        "到线就停：4 次 × 5 token = 20，第 5 次不该发出去"
    );
    assert!(
        turn.reply.contains(FOREMAN_PARTIAL_TURN_MARK) && turn.reply.contains("token 预算"),
        "触顶要标注、且说清是预算那条线：{}",
        turn.reply
    );
    // 票 02：**预算触顶**那条尾句也接产出注记（此前只有「人按停」那一条有）。
    assert!(
        turn.reply.contains("本轮没有提任何提议，也没有改动文件。"),
        "预算触顶的尾句要按实际产出说话：{}",
        turn.reply
    );
    let stored = h
        .store
        .list_foreman_messages(&turn.session.id, 10, None)
        .await
        .unwrap();
    assert!(
        stored
            .iter()
            .any(|m| m.role == "assistant" && m.content.contains(FOREMAN_PARTIAL_TURN_MARK)),
        "半份结论要落库：{stored:?}"
    );
    assert!(
        !stored
            .iter()
            .any(|m| m.role == "system" && m.content.contains("没跑起来")),
        "触顶不是失败（也不该顺手把待办消费掉）：{stored:?}"
    );

    // 人的那一轮：同一条线，不拦——只落一条软告警（只落账不拦），并**自己说完**。
    let h2 = Harness::seeded().await;
    h2.store
        .upsert_stage_config(&StageConfig {
            stage: FOREMAN_STAGE_KEY.to_string(),
            watch_token_budget: Some(20),
            ..Default::default()
        })
        .await
        .unwrap();
    let calls2 = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let runner2 = h2.runner_with_llm(Arc::new(TalkyThenStop {
        calls: calls2.clone(),
        stop_after: Some(6),
        fail_at: None,
        fail_kind: "llm_idle_timeout",
        completion_tokens: 5,
    }) as Arc<dyn LlmClient>);
    let sid = h2.session().await;
    let turn = runner2.say(Some(&sid), "盯着 t1").await.unwrap();
    assert_eq!(
        calls2.load(std::sync::atomic::Ordering::SeqCst),
        6,
        "人的那一轮没有硬界：过了线也照跑，直到模型自己说完"
    );
    assert!(
        !turn.reply.contains(FOREMAN_PARTIAL_TURN_MARK),
        "没触顶就不标注：{}",
        turn.reply
    );
    let stored = h2
        .store
        .list_foreman_messages(&sid, 10, None)
        .await
        .unwrap();
    let last = stored.last().unwrap();
    assert_eq!(last.role, "assistant");
    assert!(
        last.content.contains("这一轮已烧 30 生成 token") && last.content.contains("没有硬界"),
        "软告警落在该轮台账上（只落账不拦）：{}",
        last.content
    );
    assert!(
        !stored.iter().any(|m| m.role == "system"),
        "软告警不另起系统消息（避免刷屏）：{stored:?}"
    );
}

/// 中途失败**也要留住半份结论**（决策 292 / 票 07）：有话说就先落库再报错，
/// 失败记账照旧（类别 + 那条系统账）。
#[tokio::test]
async fn a_mid_turn_failure_keeps_what_was_already_said() {
    let h = Harness::seeded().await;
    let sid = h.session().await;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let runner = h.runner_with_llm(Arc::new(TalkyThenStop {
        calls: calls.clone(),
        stop_after: None,
        fail_at: Some(3),
        fail_kind: "llm_idle_timeout",
        completion_tokens: 5,
    }) as Arc<dyn LlmClient>);

    // ① 错误照旧报出来（失败记账、通知、悬空提议作废都指望它）
    let err = runner.say(Some(&sid), "盯着 t1").await.unwrap_err();
    assert_eq!(
        err.llm_classified().map(|(k, _)| k.to_string()).as_deref(),
        Some("llm_idle_timeout"),
        "中途失败照旧带自己的类别：{err}"
    );
    // ② 已经说过的那半句**没有跟着错误一起丢掉**
    let stored = h.store.list_foreman_messages(&sid, 10, None).await.unwrap();
    let partial = stored
        .iter()
        .find(|m| m.role == "assistant")
        .unwrap_or_else(|| panic!("半份结论要落库：{stored:?}"));
    assert!(
        partial.content.contains(FOREMAN_PARTIAL_TURN_MARK)
            && partial.content.contains("它挂在 test.execute 上")
            && partial.content.contains("中途断了"),
        "半份结论要带标注与原因：{}",
        partial.content
    );
    // 票 02：**中途断流**那条尾句也接产出注记。
    assert!(
        partial
            .content
            .contains("本轮没有提任何提议，也没有改动文件。"),
        "中途断流的尾句要按实际产出说话：{}",
        partial.content
    );
    // ③ 失败那一行也在（两条记载各说各的，一条也不丢）
    assert!(
        stored
            .iter()
            .any(|m| m.role == "system" && m.content.contains("llm_idle_timeout")),
        "失败账照旧：{stored:?}"
    );
}

/// 一直重复**同一个调用**、从不收口的替身（票 08 的循环检测用例）。
///
/// 与 [`TalkyThenStop`] 的差别是参数固定：每次都发 `read_task {"task_id":"t1"}`——工具结果
/// 逐字相同，于是「同参重复」与「读不到新东西」两条判据都指着它。
/// 它另外把每次请求里那几条**带标记的 user 轮**记下来：提醒必须进模型上下文才算拦得住，
/// 而提醒本身不落库（不往值班经理的时间线上塞一句他没说过的话）——这是它唯一的可观测面。
struct LoopingForever {
    calls: Arc<std::sync::atomic::AtomicUsize>,
    /// 第 n 次调用收到的请求里，带 [`FOREMAN_LOOP_REMINDER_MARK`] 的条目（按调用顺序）。
    reminders: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
}

impl LlmClient for LoopingForever {
    fn complete(
        &self,
        request: LlmRequest,
    ) -> futures::future::BoxFuture<'static, agentpipeline_core::Result<AgentResponse>> {
        let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let seen = request
            .messages
            .iter()
            .filter(|m| m.role == agentpipeline_core::agent::client::Role::User)
            .filter_map(|m| m.content.as_deref())
            .filter(|c| c.contains(FOREMAN_LOOP_REMINDER_MARK))
            .map(str::to_string)
            .collect();
        self.reminders.lock().unwrap().push(seen);
        Box::pin(async move {
            Ok(AgentResponse {
                content: Some(format!("（第 {n} 步）我先看看 t1。")),
                tool_calls: vec![agentpipeline_core::agent::client::ToolCall {
                    id: format!("c{n}"),
                    name: "read_task".into(),
                    arguments: r#"{"task_id":"t1"}"#.into(),
                }],
                prompt_tokens: 10,
                completion_tokens: 5,
                ..Default::default()
            })
        })
    }
}

/// 与 [`LoopingForever`] 同构，但重复的是**同一个 `write_file`**（票 02 的「改过文件的
/// 打转轮」用例）。`write_file` 幂等（`write_file_is_idempotent`），同参重放的结果指纹
/// 逐字相同，故循环判据照样命中；`auto` 档下它每次都真的动了 `notes.md`。
struct LoopingWrite {
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

impl LlmClient for LoopingWrite {
    fn complete(
        &self,
        _request: LlmRequest,
    ) -> futures::future::BoxFuture<'static, agentpipeline_core::Result<AgentResponse>> {
        let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        Box::pin(async move {
            Ok(AgentResponse {
                content: Some(format!("（第 {n} 步）我改一下 notes.md。")),
                tool_calls: vec![agentpipeline_core::agent::client::ToolCall {
                    id: format!("c{n}"),
                    name: "write_file".into(),
                    arguments: r#"{"path":"notes.md","content":"x"}"#.into(),
                }],
                prompt_tokens: 10,
                completion_tokens: 5,
                ..Default::default()
            })
        })
    }
}

/// 循环检测（决策 293 / 票 08）：同一个调用连着做 → **提醒一次**（进模型上下文）→
/// 还在做 → 强制收口，落库那一行挂 `【未收口·在打转】` 且写清为什么。
#[tokio::test]
async fn a_repeating_tool_call_is_reminded_once_then_closed_out() {
    let h = Harness::seeded().await;
    let sid = h.session().await;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let reminders = Arc::new(std::sync::Mutex::new(Vec::new()));
    let runner = h.runner_with_llm(Arc::new(LoopingForever {
        calls: calls.clone(),
        reminders: reminders.clone(),
    }) as Arc<dyn LlmClient>);

    let turn = runner.say(Some(&sid), "盯着 t1").await.unwrap();

    // ① 提醒那一次之后**再犯两次**才收口：3 次触发提醒 → 提醒之后又攒 3 次。
    //    若判据坏掉（比如窗口没在提醒处重置），这里会一路跑到轮数上限。
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        6,
        "第 3 次触发提醒、第 6 次强制收口——提醒之后同样只给两次机会"
    );
    // ② 提醒**进过模型上下文**（不落库，故这是它唯一的证据）；且只进过一次。
    let seen = reminders.lock().unwrap().clone();
    assert!(
        seen[..3].iter().all(|r| r.is_empty()),
        "第 3 次调用之前不该有提醒：{seen:?}"
    );
    assert_eq!(
        seen.iter().filter(|r| !r.is_empty()).count(),
        3,
        "提醒从那之后一直在上下文里（第 4/5/6 次调用各带一份）：{seen:?}"
    );
    assert_eq!(
        seen[5].len(),
        1,
        "同一份提醒不重复注入（转录里只该有一条）：{seen:?}"
    );
    // ③ 收口：部分结论 + 打转的标记 + 为什么（说的是哪件事，不是一个笼统的「出错」）。
    assert!(
        turn.reply.contains(FOREMAN_LOOP_TURN_MARK)
            && turn
                .reply
                .contains("同一个调用（read_task + 同一份参数）连着做了 3 次")
            && turn.reply.contains("（第 6 步）我先看看 t1。"),
        "收口要带标记、原因与已确定的部分：{}",
        turn.reply
    );
    // ③′ **没动过文件**的在打转轮（票 02 的两态之一）：尾句保留「换个线索」——对
    // 「确实查不下去」那一支，再查一轮换个线索是对的；产出注记也按「两个都没有」说话。
    assert!(
        turn.reply.contains("换个线索"),
        "没动过文件时保留「换个线索」（决策 293 的原文）：{}",
        turn.reply
    );
    assert!(
        turn.reply.contains("本轮没有提任何提议，也没有改动文件。"),
        "产出注记按「0 提议 ∪ 0 改动」说话：{}",
        turn.reply
    );
    let stored = h.store.list_foreman_messages(&sid, 10, None).await.unwrap();
    let closed = stored
        .iter()
        .find(|m| m.role == "assistant")
        .unwrap_or_else(|| panic!("收口那一行要落库：{stored:?}"));
    assert!(
        closed.content.contains(FOREMAN_LOOP_TURN_MARK),
        "落库那一行带打转的标记：{}",
        closed.content
    );
    // ④ 收口不是失败：没有失败账（打转是模型行为，被拦下来了就是要它停，不是这一轮坏了）。
    assert!(
        !stored
            .iter()
            .any(|m| m.role == "system" && m.content.contains("没跑起来")),
        "打转收口不该落一条失败账：{stored:?}"
    );
}

/// 票 02 的两态之二：**改过文件**的在打转轮，尾句不再说「换个线索再查」——一轮真改了
/// 东西却被建议「换个线索」，是把修复轮说成研究轮（2026-10-08 实账 394 / 405 的谎）。
#[tokio::test]
async fn a_looping_round_that_changed_files_is_asked_to_fix_not_to_research() {
    let h = Harness::seeded().await;
    // 改动清单只在 `auto` 档读得出来（缺省 `ask` 档写工具只落提议、文件不动，决策 411）。
    h.foreman_env(agentpipeline_core::types::EnvMode::Auto)
        .await;
    let sid = h.session().await;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let runner = h.runner_with_llm(Arc::new(LoopingWrite {
        calls: calls.clone(),
    }) as Arc<dyn LlmClient>);

    let turn = runner.say(Some(&sid), "改那个文件").await.unwrap();

    assert!(
        turn.reply.contains(FOREMAN_LOOP_TURN_MARK),
        "照样是打转收口：{}",
        turn.reply
    );
    assert!(
        !turn.reply.contains("换个线索"),
        "改过文件的轮不许再被建议「换个线索再查」：{}",
        turn.reply
    );
    assert!(
        turn.reply.contains("要接着改说一声"),
        "建议要指向「接着改」：{}",
        turn.reply
    );
    assert!(
        turn.reply.contains("本轮改了 1 个文件"),
        "产出注记要带上改动文件数：{}",
        turn.reply
    );
    // 文末那份清单也在（尾句说的「文末清单」指的就是它）。
    let stored = h.store.list_foreman_messages(&sid, 10, None).await.unwrap();
    let closed = stored
        .iter()
        .find(|m| m.role == "assistant")
        .unwrap_or_else(|| panic!("收口那一行要落库：{stored:?}"));
    assert!(
        closed
            .content
            .contains("【本轮改动】本轮改了 1 个文件：notes.md。"),
        "尾句指的文末清单要真的在：{}",
        closed.content
    );
}

/// 第 N 次调用起**挂住不返回**的替身（票 09 的停钮用例）：只有停钮能把它打断。
///
/// 与 [`ChattyForever`] 的差别只有一个：它在某一次调用上不返回——而「停钮必须能打断
/// **正在跑的那一次调用**」正是本票要证的那件事（只在下一次调用才查的协作式中止，
/// 在人按停这件事上等于没停）。
struct StallingAfter {
    calls: Arc<std::sync::atomic::AtomicUsize>,
    /// 第几次调用**起**挂住。
    stall_at: usize,
    /// 第 1 次调用里由它提一条提议（要验「被停在半路那一轮提的提议留着」，而提议必须
    /// 在**轮内**创建——判据是 `created_at >= 这一轮起点`）。
    propose: Option<(Store, String)>,
    /// 挂住之前先通知一声：用例据此在**调用在飞**时按停。
    stalled: Arc<tokio::sync::Notify>,
}

impl LlmClient for StallingAfter {
    fn complete(
        &self,
        _request: LlmRequest,
    ) -> futures::future::BoxFuture<'static, agentpipeline_core::Result<AgentResponse>> {
        let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let (stall_at, stalled) = (self.stall_at, Arc::clone(&self.stalled));
        let propose = (n == 1).then(|| self.propose.clone()).flatten();
        Box::pin(async move {
            if let Some((store, session_id)) = propose {
                store
                    .create_foreman_proposal(
                        agentpipeline_core::storage::proposals::NewForemanProposal {
                            kind: agentpipeline_core::storage::proposals::ForemanProposalKind::ApiCall,
                            payload: None,
                            session_id,
                            tool: "task".into(),
                            args: serde_json::json!({"task_id": "t1", "action": "resume"}),
                            summary: "（用例）把 t1 接着跑".into(),
                            situation: None,
                        },
                    )
                    .await?;
            }
            if n >= stall_at {
                stalled.notify_one();
                std::future::pending::<()>().await;
            }
            Ok(AgentResponse {
                content: Some(format!("（第 {n} 步）我先看看 t1。")),
                tool_calls: vec![agentpipeline_core::agent::client::ToolCall {
                    id: format!("c{n}"),
                    name: "read_task".into(),
                    arguments: r#"{"task_id":"t1"}"#.into(),
                }],
                prompt_tokens: 10,
                completion_tokens: 5,
                ..Default::default()
            })
        })
    }
}

/// 停钮（决策 294 / 票 09）：按下去 → **正在跑的那一次调用**被打断、部分结论落库并挂
/// `【已停】`、**它提的悬空提议留着**（显式修订决策 233③）且标了来路、失败账为零。
#[tokio::test]
async fn pressing_stop_ends_the_human_turn_and_keeps_its_proposals() {
    let h = Harness::seeded().await;
    let sid = h.session().await;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let stalled = Arc::new(tokio::sync::Notify::new());
    let runner = Arc::new(h.runner_with_llm(Arc::new(StallingAfter {
        calls: calls.clone(),
        // 第 2 次调用上挂住：那时这一轮已经说过一句话（第 1 步），
        // 也提过一条提议——正是「停在半路」的形状。
        stall_at: 2,
        propose: Some((h.store.clone(), sid.clone())),
        stalled: stalled.clone(),
    }) as Arc<dyn LlmClient>));

    let who = Arc::clone(&runner);
    let sid_for_turn = sid.clone();
    let turn_task = tokio::spawn(async move { who.say(Some(&sid_for_turn), "盯着 t1").await });
    // 等第 2 次调用真的发出去（在飞），再按停——这一格是 `select!` 那条路，
    // 只在下一次调用才查的协作式中止在这里会挂着不动。
    stalled.notified().await;
    assert!(
        agentpipeline_core::pipeline::foreman::cancel_foreman_turn(&sid),
        "这一班确实有一轮在飞：停钮必须找得到它的通道"
    );
    let turn = turn_task.await.unwrap().unwrap();

    // ① 收口：部分结论 + 【已停】+ 停在第几轮（读者可能不是按停的那个人）
    assert!(turn.stopped, "这一轮是被按停的");
    assert!(
        turn.reply.contains(FOREMAN_STOPPED_TURN_MARK)
            && turn.reply.contains("（第 1 步）我先看看 t1。")
            && turn.reply.contains("停在第 2 轮"),
        "收口要带标记、部分结论与为什么：{}",
        turn.reply
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "第 2 次调用被放弃：它永远不返回，而这一轮照常收了口"
    );
    // ② 落库那一行：标注在、**失败账不在**（按停不是失败）
    let stored = h.store.list_foreman_messages(&sid, 10, None).await.unwrap();
    let closed = stored
        .iter()
        .find(|m| m.role == "assistant")
        .unwrap_or_else(|| panic!("收口那一行要落库：{stored:?}"));
    assert!(
        closed.content.contains(FOREMAN_STOPPED_TURN_MARK),
        "落库那一行带【已停】：{}",
        closed.content
    );
    assert!(
        !stored.iter().any(|m| m.role == "system"),
        "按停不落失败账（它不是它坏了）：{stored:?}"
    );
    // ③ 提议：**留着**（决策 294 修订 233③），且标了「来自一轮被停在半路的话」
    let proposals = h.store.list_pending_foreman_proposals(&sid).await.unwrap();
    assert_eq!(proposals.len(), 1, "被停在半路那一轮提的提议不许被作废");
    assert!(
        proposals[0].stopped_round,
        "要在提议上标注它来自一轮被停在半路的话"
    );
}

/// 人按停而它**一句话都没说过**（决策 294 / 票 09）：照样落一行（标记 + 停在第几轮），
/// 且**不**走失败处置——把「你要它停」写成「它没跑起来」是归因失真，还会顺手发一条
/// 失败通知（按停的人就在屏幕前，那是纯噪声）。
#[tokio::test]
async fn pressing_stop_before_it_says_anything_still_leaves_a_row() {
    let h = Harness::empty().await;
    let sid = h.session().await;
    let server = TinyHttp::spawn("200", "application/json", b"{}".to_vec(), Duration::ZERO);
    attach_notifier(&h, &server);
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let stalled = Arc::new(tokio::sync::Notify::new());
    let runner = Arc::new(h.runner_with_llm(Arc::new(StallingAfter {
        calls: calls.clone(),
        // 第 1 次调用就挂住：这一轮连一句话都还没说
        stall_at: 1,
        propose: None,
        stalled: stalled.clone(),
    }) as Arc<dyn LlmClient>));

    let who = Arc::clone(&runner);
    let sid_for_turn = sid.clone();
    let turn_task = tokio::spawn(async move { who.say(Some(&sid_for_turn), "盯着 t1").await });
    stalled.notified().await;
    assert!(agentpipeline_core::pipeline::foreman::cancel_foreman_turn(
        &sid
    ));
    let turn = turn_task.await.unwrap().unwrap();

    assert!(turn.stopped);
    assert!(
        turn.reply.contains(FOREMAN_STOPPED_TURN_MARK)
            && turn.reply.contains("停在第 1 轮")
            && turn.reply.contains("没有部分结论可留"),
        "没有部分结论时那行说的是这件事本身：{}",
        turn.reply
    );
    // **收场文案按实际提议数分支**（决策 311，票 foreman-burns 03）：这一轮一条提议都没有，
    // 就不许再说「这一轮提的提议都还在，照样可以按」——2026-09-27 那次它就是这么说的谎，
    // 而那个会话的提议计数是 **0**（唯一相关的那张属于上一个会话，且早在一小时前作废）。
    assert!(
        !turn.reply.contains("提议都还在"),
        "0 条提议时不许再说「提议都还在」：{}",
        turn.reply
    );
    assert!(
        turn.reply.contains("本轮没有提任何提议"),
        "0 条提议要明说 0 条：{}",
        turn.reply
    );
    assert_eq!(
        h.store
            .count_round_foreman_proposals(&sid, h.store.now() - chrono::Duration::hours(1))
            .await
            .unwrap(),
        0,
        "前提：这一轮确实一条提议都没提"
    );
    let stored = h.store.list_foreman_messages(&sid, 10, None).await.unwrap();
    assert!(
        stored
            .iter()
            .any(|m| m.role == "assistant" && m.content.contains(FOREMAN_STOPPED_TURN_MARK)),
        "照样落一行：{stored:?}"
    );
    assert!(
        !stored.iter().any(|m| m.role == "system"),
        "按停不落失败账：{stored:?}"
    );
    assert_eq!(
        server.hits(),
        0,
        "按停不是失败：不发失败通知，也没有回话完成通知"
    );
    // 通道随这一轮结束一起摘掉：没有在飞轮的班次上，停钮**如实**回 false
    assert!(
        !agentpipeline_core::pipeline::foreman::cancel_foreman_turn(&sid),
        "这一轮已经结束了：停钮没有东西可停"
    );
}

/// **有提议时那句提醒一个字不改**（决策 311 的反向）：它是决策 294 / 修订 233③「被停在
/// 半路那轮提的提议保留」的兑现点——丢了它，人就不会去看那批卡片了。
#[tokio::test]
async fn the_closeout_keeps_the_proposal_note_when_the_round_proposed_something() {
    let h = Harness::seeded().await;
    let sid = h.session().await;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let stalled = Arc::new(tokio::sync::Notify::new());
    let runner = Arc::new(h.runner_with_llm(Arc::new(StallingAfter {
        calls: calls.clone(),
        // 第 2 次调用上挂住：第 1 次已经提过一条提议（`StallingAfter` 的 `propose`）
        stall_at: 2,
        propose: Some((h.store.clone(), sid.clone())),
        stalled: stalled.clone(),
    }) as Arc<dyn LlmClient>));

    let who = Arc::clone(&runner);
    let sid_for_turn = sid.clone();
    let turn_task = tokio::spawn(async move { who.say(Some(&sid_for_turn), "盯着 t1").await });
    stalled.notified().await;
    assert!(agentpipeline_core::pipeline::foreman::cancel_foreman_turn(
        &sid
    ));
    let turn = turn_task.await.unwrap().unwrap();

    assert!(turn.stopped);
    assert_eq!(
        h.store
            .count_round_foreman_proposals(&sid, h.store.now() - chrono::Duration::hours(1))
            .await
            .unwrap(),
        1,
        "前提：这一轮提过一条提议"
    );
    assert!(
        turn.reply.contains("这一轮提的提议都还在，照样可以按。"),
        "有提议时那句**一字不改**：{}",
        turn.reply
    );
    assert!(
        !turn.reply.contains("本轮没有提任何提议"),
        "有提议就不许说 0 条：{}",
        turn.reply
    );
}

/// 一轮死了之后，**它那一轮提的提议随之失效**（决策 233③）。
#[tokio::test]
async fn a_failed_turn_invalidates_the_proposals_it_left_behind() {
    let h = Harness::seeded().await;
    let sid = h.session().await;
    // 这一轮提过两条（实测里那两条悬空提议就是这个形状）
    for i in 0..2 {
        h.store
            .create_foreman_proposal(agentpipeline_core::storage::proposals::NewForemanProposal {
                kind: agentpipeline_core::storage::proposals::ForemanProposalKind::ApiCall,
                payload: None,
                session_id: sid.clone(),
                tool: "write_file".into(),
                args: serde_json::json!({"path": format!("notes-{i}.md"), "content": "x"}),
                summary: "（用例）".into(),
                situation: None,
            })
            .await
            .unwrap();
    }
    assert_eq!(
        h.store
            .list_pending_foreman_proposals(&sid)
            .await
            .unwrap()
            .len(),
        2
    );

    // 这一轮跑不起来（模型当场报错）
    let runner = ForemanRunner::new(
        h.store.clone(),
        Settings::default(),
        h._home.home().clone(),
        Arc::new(FailingLlm) as Arc<dyn LlmClient>,
        Arc::new(testkit::SseRecorder::new()),
    );
    assert!(runner.say(Some(&sid), "动手吧").await.is_err());

    assert!(
        h.store
            .list_pending_foreman_proposals(&sid)
            .await
            .unwrap()
            .is_empty(),
        "那一轮已经死了，钮就该随之作废"
    );
    // 行还在（审计：它当时提议过什么必须可追溯），状态是 expired。
    let all = h.store.list_foreman_proposals(&sid, 100).await.unwrap();
    assert_eq!(all.len(), 2);
    assert!(
        all.iter().all(|p| p.status.as_str() == "expired"),
        "留痕但不 pending：{all:?}"
    );
}

/// 一轮死了只作废**它那一轮**提的提议（决策 233③）——上一轮留下的待办不受牵连。
///
/// 为什么这条必须钉：`invalidate` 若只按班次收，一次失败轮会把**上一轮**留下、正等着人按键的
/// 提议一起作废——而它们是合规的待办（实测里那两条悬空提议要的是「**它们那一轮**死了才失效」，
/// 不是「之后任何一轮死了都失效」）。这条用例的牙齿就是把两者放在同一班次里对照。
#[tokio::test]
async fn a_failed_turn_only_invalidates_the_proposals_of_its_own_round() {
    let h = Harness::seeded().await;
    let sid = h.session().await;
    let new_proposal = |tool: &str| agentpipeline_core::storage::proposals::NewForemanProposal {
        kind: agentpipeline_core::storage::proposals::ForemanProposalKind::ApiCall,
        payload: None,
        session_id: sid.clone(),
        tool: tool.into(),
        args: serde_json::json!({"path": "notes.md", "content": "x"}),
        summary: "（用例）".into(),
        situation: None,
    };
    // 上一轮留下的那一条（合规待办，等人按键）
    let old = h
        .store
        .create_foreman_proposal(new_proposal("write_file"))
        .await
        .unwrap()
        .id;

    // 时钟往前推一秒：判据是 `created_at >= 这一轮开始的时刻`，而这一台是**固定时钟**
    // ——不推的话两条提议同刻，那条合规待办会被一起收掉（真实时钟下没有这个问题，
    // 而 `>=` 是故意的：同刻的提议按「这一轮的」收，宁可多收也不漏收）。
    h.clock.advance_secs(1);

    // 这一轮：先说一句 → 提一条 → 然后就跑不起来了。
    let runner = ForemanRunner::new(
        h.store.clone(),
        Settings::default(),
        h._home.home().clone(),
        Arc::new(ProposeThenFail {
            proposed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            store: h.store.clone(),
            session_id: sid.clone(),
        }) as Arc<dyn LlmClient>,
        Arc::new(testkit::SseRecorder::new()),
    );
    assert!(runner.say(Some(&sid), "动手吧").await.is_err());

    let pending: Vec<String> = h
        .store
        .list_pending_foreman_proposals(&sid)
        .await
        .unwrap()
        .iter()
        .map(|p| p.id.clone())
        .collect();
    assert_eq!(
        pending,
        vec![old],
        "上一轮那条要留着（它是合规待办），这一轮提的那条要作废"
    );
    let all = h.store.list_foreman_proposals(&sid, 100).await.unwrap();
    assert_eq!(all.len(), 2, "两条都还在台账里（作废不删行）");
}

/// 先提一条、然后报错的替身（模拟「那一轮提了东西，然后死了」）。
struct ProposeThenFail {
    proposed: Arc<std::sync::atomic::AtomicBool>,
    store: agentpipeline_core::storage::Store,
    session_id: String,
}

impl LlmClient for ProposeThenFail {
    fn complete(
        &self,
        _request: LlmRequest,
    ) -> futures::future::BoxFuture<'static, agentpipeline_core::Result<AgentResponse>> {
        use std::sync::atomic::Ordering;
        if self.proposed.swap(true, Ordering::SeqCst) {
            return Box::pin(async { Err(Error::Llm("连接被对端关掉".into())) });
        }
        let store = self.store.clone();
        let session_id = self.session_id.clone();
        Box::pin(async move {
            // 走真实的提议入口（`ask` 档下的写工具会被拦成提议）——这里直接落一条，
            // 因为这条用例考的**不是**闸门，而是「这一轮提的」这个归属判据。
            store
                .create_foreman_proposal(
                    agentpipeline_core::storage::proposals::NewForemanProposal {
                        kind: agentpipeline_core::storage::proposals::ForemanProposalKind::ApiCall,
                        payload: None,
                        session_id,
                        tool: "write_file".into(),
                        args: serde_json::json!({"path": "during-round.md", "content": "x"}),
                        summary: "（用例）这一轮提的".into(),
                        situation: None,
                    },
                )
                .await?;
            Ok(AgentResponse {
                content: Some("我先提一条。".into()),
                tool_calls: vec![agentpipeline_core::agent::client::ToolCall {
                    id: "c1".into(),
                    name: "read_task".into(),
                    arguments: r#"{"task_id":"t1"}"#.into(),
                }],
                prompt_tokens: 10,
                completion_tokens: 5,
                ..Default::default()
            })
        })
    }
}

/// 存量里的 `max_rounds = 0` **拒绝启动**（决策 233① / 239）——打在**真读路径**上。
///
/// 为什么单列：写入路径（`PUT /stage-configs`、`config set`）已经按正整数拒过，这条守卫管的是
/// **存量**（手工改库、老版本写下的值）。只构造一个 `StageConfig` 去调 `validate_startup` 是
/// 不够的——真正的入口是 `list_stage_configs` → `StageConfigRow::into_config`；那一层若把 `0`
/// 吞成 `None`（当成「没配过」），守卫就永远不可达，而 `0` 被静默当成缺省 300。故这里绕过写入
/// 校验直接改库，再走 `Store::validate_startup`（与 `serve.rs` 启动时同一条路）。
#[tokio::test]
async fn a_stored_zero_max_rounds_fails_startup_on_the_real_read_path() {
    let h = Harness::seeded().await;
    // 先按正常写入放一个合法值，再绕过写入校验把它改成 0（模拟手工改库 / 老版本写下的值）。
    h.store
        .upsert_stage_config(&StageConfig {
            stage: FOREMAN_STAGE_KEY.to_string(),
            max_rounds: Some(3),
            ..Default::default()
        })
        .await
        .unwrap();
    sqlx::query("UPDATE stage_configs SET max_rounds = 0 WHERE stage = 'foreman'")
        .execute(h.store.pool())
        .await
        .unwrap();

    // 读路径必须把 0 原样带出来（不是当成「没配过」）……
    let cfg = h
        .store
        .get_stage_config(FOREMAN_STAGE_KEY)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cfg.max_rounds, Some(0), "0 不许被静默吞成「没配过」");

    // ……启动校验必须当场拒绝，并说清改哪儿、以及「没有无上限这一档」。
    let err = h
        .store
        .validate_startup(&Settings::default())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("max_rounds"), "{err}");
    assert!(err.contains("正整数"), "{err}");
    assert!(
        err.contains(&FOREMAN_MAX_ROUNDS.to_string()),
        "报文要给出缺省值，好让人知道删掉这一格会回到多少：{err}"
    );
}

/// 决策 230 的四项判据**在同一次播报轮里齐备**——这是那一票的验收形状，不是四个分开的读数齐不齐。
///
/// 判据原文：「收到一次失败/停摆后，**一次值守轮内**必须给出 ①哪一个 run（id）②卡在哪一环
/// （阶段 / 节点 / 一次具体调用）③一条**可复核的原始证据**（日志行 / 栈帧 / 命令回执）
/// ④**归因类别**；四项缺一即算未定位，且『证据归错 run』与『没有证据』同判为失败」。
///
/// 为什么必须**打在一轮里**：分开的用例各自绿着、而合起来一轮给不出四项，正是 2026-09-19 的现场
/// ——它采到了正确的方法（`sample` / `lsof` / 栈帧齐全），却把 run 27 的活栈归到**已经 failed 的
/// run 26** 名下。四项里任何一项单独看都不缺，缺的是「四项说的是**同一条 run**」。
/// 故这里断言四个读数落在**同一份回话**上，且 run 的归属是**同一条**。
#[tokio::test]
async fn one_watch_round_closes_all_four_criteria_on_the_same_run() {
    let h = Harness::seeded().await;
    // 现场：一条失败的 run（带 error），命令台账与闸门输出的真文件（可复核的原始证据）。
    let run_id = seed_failed_task(&h, "t1").await;
    // 那条 run 上挂了一次模型请求（票 01 的落点）：它是「这一轮卡在哪一次调用」的读数，
    // 而**它必须归到同一条 run 上**——这正是 2026-09-19 缺的那一环。
    let request_id = h
        .store
        .begin_model_request(&NewModelRequest {
            run_id: Some(run_id),
            session_id: None,
            task_id: Some("t1".into()),
            agent_type: "main".into(),
            stage: "test".into(),
            node: "execute".into(),
            attempt: 1,
        })
        .await
        .unwrap();
    h.store
        .finish_model_request(
            request_id,
            ModelRequestStatus::Error,
            &ModelRequestUsage {
                prompt_tokens: Some(1_234_567),
                ..Default::default()
            },
            Some("对端在流中途关掉了连接"),
        )
        .await
        .unwrap();
    // 任务**没有**因此转 pending（失败后游标自动重试、任务仍在 running）——这正是
    // `run_failed` 那类事件的形状：既有的 `task_pending` / `scheduler_no_effect` 两条都不成立。
    let mut script = Script::new();
    script.for_foreman().read_diagnosis("t1");
    // 回话：① run 身份 ② 卡在哪一环 ③ 原始证据 ④ 归因类别（机器可读的一段）。
    // ① 与 ④ 落在**同一行**上：类别说的是哪一类问题，`run_id` 说这条证据是哪一条 run 的——
    // 判据①的校验面就在这里（决策 230 把「证据归错 run」与「没有证据」同判失败）。
    let reply = format!(
        "run {run_id} 卡在 test.execute 这一次调用上：闸门退出码 1，\
         见 gate-output-test.log（命令台账里的 `cargo test --quiet`）。\n\
         【归因】{{\"attribution\":\"project_code\",\"run_id\":{run_id}}}\n"
    );
    script.for_foreman().text(&reply);
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());

    let attention = agentpipeline_core::storage::AttentionKind::RunFailed;
    note(&h, "t1", attention).await;
    h.clock.advance_secs(61);
    let turn = runner.watch().await.unwrap().expect("这条失败应当唤醒值守");

    // 播报轮的身份没被弄错：落库那一行仍是播报轮（前端靠前缀把两种轮分开——前缀由**后端**
    // 加在入库的那一份上，`turn.reply` 是模型原话，故这里读库），且归因类别已经解析出来。
    let stored = h
        .store
        .list_foreman_messages(&turn.session.id, 10, None)
        .await
        .unwrap();
    let broadcast = stored
        .iter()
        .find(|m| m.role == "assistant")
        .expect("播报要落库");
    assert!(
        broadcast.content.starts_with(FOREMAN_WATCH_MARK),
        "播报那一轮的前缀不能丢：{}",
        broadcast.content
    );
    // 判据①：回话指名的 run 必须与**证据实际挂在的那条 run** 是同一条。这条断言在
    // 2026-09-19 那次是缺的——当时回话把 run 27 的活栈记在 run 26 名下，行文与类别都合规。
    assert_eq!(
        parse_attribution(&broadcast.content),
        Attribution::Located {
            kind: AttributionKind::ProjectCode,
            run_id: Some(run_id),
        },
        "④ 归因类别要在播报那一轮里给出（四类之内），且 ① 它指名的 run 就是证据那条 run：{}",
        broadcast.content
    );

    // ① ② ③ 三项落在**同一次工具回执**上：这是模型据以收口的证据面，也是唯一能被复核的那一份。
    let pack = agent.request_log()[1]
        .messages
        .iter()
        .find(|m| m.name.as_deref() == Some("read_diagnosis"))
        .and_then(|m| m.content.clone())
        .expect("诊断包的回执要在转写里");
    // 按结构断言，不按字符串顺序：回执本身就是一份 JSON 文档（`to_string_pretty`）。
    let doc: serde_json::Value = serde_json::from_str(&pack).expect("诊断包要是一份完整 JSON");
    let evidence = &doc["evidence"];
    assert_eq!(
        evidence[0]["why_stalled"]["latest_failure"]["run_id"].as_i64(),
        Some(run_id),
        "① 哪一个 run：诊断包要指名它（不是「最近失败的那条」这种描述）：{evidence}"
    );
    assert!(
        evidence[0]["why_stalled"]["latest_failure"]["error"]
            .as_str()
            .is_some_and(|e| e.contains("闸门失败：测试命令退出码 1")),
        "② 卡在哪一环（这一次调用的错误原文）：{evidence}"
    );
    assert_eq!(
        evidence[1]["failed_run_context"]["run_id"].as_i64(),
        Some(run_id),
        "② 的现场必须是**那条 run 的**现场——「证据归错 run」与「没有证据」同判失败：{}",
        evidence[1]
    );
    let pack_text = pack.as_str();
    assert!(
        pack_text.contains("gate-output-test.log"),
        "③ 可复核的原始证据（落盘的那份闸门输出）：{pack_text}"
    );
    assert!(
        pack_text.contains("cargo test --quiet"),
        "③ 命令回执（谁跑的、跑的是什么）：{pack_text}"
    );
    // 归位判据也咬在结构上：模型请求台账那一节与失败现场**指向同一条 run**——两张不同的
    // 脸都归到同一个 `run_id` 上，才算「四项说的是同一条 run」（实测里正是这一步错了）。
    let requests = evidence[3]["model_requests"]["recent"]
        .as_array()
        .expect("模型请求那一节要在场（票 01 的归位面）");
    assert!(
        requests
            .iter()
            .any(|r| r["run_id"].as_i64() == Some(run_id)),
        "这一次调用要归到那条 run 上（不是「最近有一次调用」）：{evidence}"
    );
    assert!(
        requests
            .iter()
            .any(|r| r["prompt_tokens"].as_i64() == Some(1_234_567)),
        "量速那一半（这一次调用烧掉多少）也要在场：{evidence}"
    );
}

// ──────────────────── 思考留痕（决策 244）────────────────────

/// 把任意替身包一层：给它每一次响应补上推理原文。
///
/// 为什么包一层而不是改 `FakeAgent`：`Step` 是**共享**的接缝（流水线 / 子代理 / 伪阶段都用
/// 它），而「产推理」只有本特性关心——往那个公共枚举上加一个变体会让每一处 `match step`
/// 都要跟着改，代价与收益不成比例（照 `FailingLlm` / `ChattyForever` 那两个局部替身的先例）。
struct Thinking {
    inner: FakeAgent,
    thought: String,
}

impl LlmClient for Thinking {
    fn complete(
        &self,
        request: LlmRequest,
    ) -> futures::future::BoxFuture<'static, agentpipeline_core::Result<AgentResponse>> {
        let inner = self.inner.clone();
        let thought = self.thought.clone();
        Box::pin(async move {
            let mut response = inner.complete(request).await?;
            response.reasoning = Some(thought);
            Ok(response)
        })
    }
}

/// 值班长想过什么**留在台账那一行**上（决策 244）。
///
/// 为什么落库这一条必须钉住：推理若只活在实时流里，它会在那一轮收口重取台账的那一刻
/// 整段消失（前端以台账为权威，见 `settleForemanStream`）——即「能看到一秒，然后永远
/// 看不到」，那比不展示更坏。
#[tokio::test]
async fn foreman_persists_what_it_thought() {
    let h = Harness::empty().await;
    let mut script = Script::new();
    script.for_foreman().text("看过了，没有待办。");
    let runner = h.runner_with_llm(Arc::new(Thinking {
        inner: FakeAgent::new(script),
        thought: "先看看板，再看有没有卡住的任务。".into(),
    }));

    let turn = runner.say(None, "有活吗").await.unwrap();
    let rows = h
        .store
        .list_foreman_messages(&turn.session.id, 10, None)
        .await
        .unwrap();
    let assistant = rows
        .iter()
        .find(|m| m.role == "assistant")
        .expect("值班长那一行");
    assert_eq!(
        assistant.thinking.as_deref(),
        Some("先看看板，再看有没有卡住的任务。"),
        "推理原文要原样落库（展示留痕）"
    );
    // 用户那一行没有推理（与 `briefing_json` / `traces_json` 同一条口径）。
    let user = rows.iter().find(|m| m.role == "user").unwrap();
    assert!(user.thinking.is_none());
}

/// 不产推理的模型（多数）落 `NULL` 而不是空串——「没有」与「有但是空的」是两件事。
#[tokio::test]
async fn foreman_without_reasoning_leaves_the_column_null() {
    let h = Harness::empty().await;
    let mut script = Script::new();
    script.for_foreman().text("没有待办。");
    let turn = h
        .runner(FakeAgent::new(script))
        .say(None, "有活吗")
        .await
        .unwrap();
    let rows = h
        .store
        .list_foreman_messages(&turn.session.id, 10, None)
        .await
        .unwrap();
    let assistant = rows.iter().find(|m| m.role == "assistant").unwrap();
    assert!(
        assistant.thinking.is_none(),
        "不产推理时该是 NULL，不是空串：{:?}",
        assistant.thinking
    );
}

/// 一轮里模型被叫好几次时，推理**按次累积**（决策 244）。
///
/// 界面上那条折叠块说的是「这一轮它想了什么」，不是「最后一次调用想了什么」——
/// 而值班长的一轮常态就是「先想 → 查台账 → 再想 → 收口」。
#[tokio::test]
async fn foreman_thinking_accumulates_across_the_calls_of_one_turn() {
    let h = Harness::seeded().await;
    let mut script = Script::new();
    script.for_foreman().read_task("t1").text("t1 还在排队。");
    let runner = h.runner_with_llm(Arc::new(Thinking {
        inner: FakeAgent::new(script),
        // 每一次调用都给同一段文本，故累积 N 次就是 N 段拼起来。
        thought: "再核一遍。".into(),
    }));

    let turn = runner.say(None, "t1 怎么样了？").await.unwrap();
    assert_eq!(
        turn.traces.len(),
        1,
        "这一轮确实调了一次工具（两次模型调用）"
    );
    let rows = h
        .store
        .list_foreman_messages(&turn.session.id, 10, None)
        .await
        .unwrap();
    let thinking = rows
        .iter()
        .find(|m| m.role == "assistant")
        .and_then(|m| m.thinking.clone())
        .expect("有推理原文");
    assert_eq!(
        thinking.matches("再核一遍。").count(),
        2,
        "两次模型调用各想了一段，两段都该在：{thinking}"
    );
}

// ──────────────────── 步骤顺序留痕（决策 273）────────────────────

/// 一轮里的**顺序**留在那一行上（决策 273）：推理与工具交错，收口那句不在段序里。
///
/// 为什么三份聚合视图不够：`thinking` 把各次调用的推理拼成一段、`traces_json` 把工具收成
/// 一张表、`content` 是收口那一句——各自都在，但「先想了什么、再查了什么、然后说了什么」
/// 丢了。而值班长的一轮常态正是「先想 → 查台账 → 再想 → 收口」。
///
/// 脚本两步：先 `read_task`（带工具调用），再 `text(..)`（收口）。`Thinking` 给**每一次**
/// 模型调用都补上推理，故段序是「想 → 查 → 想」，而收口那句（`text(..)` 的正文，也就是
/// `content` 列）不进段序——收口的话由 `content` 承载，段序说的是**中途**。
#[tokio::test]
async fn foreman_persists_the_order_of_the_steps_of_one_turn() {
    let h = Harness::seeded().await;
    let mut script = Script::new();
    script.for_foreman().read_task("t1").text("t1 还在排队。");
    let runner = h.runner_with_llm(Arc::new(Thinking {
        inner: FakeAgent::new(script),
        thought: "再核一遍。".into(),
    }));

    let turn = runner.say(None, "t1 怎么样了？").await.unwrap();
    assert_eq!(turn.traces.len(), 1, "这一轮确实调了一次工具");
    let rows = h
        .store
        .list_foreman_messages(&turn.session.id, 10, None)
        .await
        .unwrap();
    let assistant = rows
        .iter()
        .find(|m| m.role == "assistant")
        .expect("值班长那一行");
    let raw = assistant
        .segments_json
        .as_ref()
        .expect("这一轮有推理与工具，段序该在场");
    // 比 `ForemanSegment` 的**值**而不是 JSON 字节：字节比较会把键序 / 字段拼写也钉进
    // 断言，而这里要说的是顺序与种类。
    let segments: Vec<ForemanSegment> =
        serde_json::from_value(raw.clone()).expect("段序要能解回类型");
    assert_eq!(
        segments,
        vec![
            ForemanSegment::Thinking {
                text: "再核一遍。".into()
            },
            ForemanSegment::Tool {
                tool: "read_task".into(),
                // 摘要取聚合那张表里的同一份（同一个事件的两处记录），本用例要说的是
                // 顺序，不是 `summarize_args` 的措辞。详情两字段同理：段序与痕迹是
                // 同一次调用的两处记录（决策 301），必须逐字相同。
                args_summary: turn.traces[0].args_summary.clone(),
                args: turn.traces[0].args.clone(),
                result: turn.traces[0].result.clone(),
                ok: true,
            },
            ForemanSegment::Thinking {
                text: "再核一遍。".into()
            },
        ],
        "推理与工具按发生顺序交错，收口那句不在段序里：{raw}"
    );
    // 决策 301：展开详情真的带了原文与结果——空串意味着界面点开只有摘要那一行。
    assert!(
        !turn.traces[0].args.is_empty(),
        "工具详情要带完整参数：{:?}",
        turn.traces[0]
    );
    assert!(
        !turn.traces[0].result.is_empty(),
        "工具详情要带结果：{:?}",
        turn.traces[0]
    );
}

/// 没有任何一步时落 `NULL` 而不是空数组（决策 273）：与 `traces_json` 同一条口径——
/// 「没有」与「有但是空的」是两件事。
#[tokio::test]
async fn foreman_without_steps_leaves_the_segments_column_null() {
    let h = Harness::empty().await;
    let mut script = Script::new();
    script.for_foreman().text("没有待办。");
    let turn = h
        .runner(FakeAgent::new(script))
        .say(None, "有活吗")
        .await
        .unwrap();
    let rows = h
        .store
        .list_foreman_messages(&turn.session.id, 10, None)
        .await
        .unwrap();
    let assistant = rows.iter().find(|m| m.role == "assistant").unwrap();
    assert!(
        assistant.segments_json.is_none(),
        "无推理、无工具、只有收口一句时该是 NULL，不是空数组：{:?}",
        assistant.segments_json
    );
}

// ─────── 在途轮边流边写（票 01，spec .scratch/talk-replay）───────

/// 第 `1..=gates` 次模型调用**各停在闸上**的替身：把「这一轮正在跑」变成可观测的窗口。
///
/// 为什么要闸而不是轮询：半截行的两个状态（「刚建出来、一个字都没有」与「第一轮查完、
/// 段序已在」）之间隔着一次**真实执行**的工具调用，轮询抓哪一个全凭运气——闸让用例
/// 恰好停在两次调用之间的缝上，两次台账断言因此都是确定的。
struct GatedAt {
    inner: Arc<dyn LlmClient>,
    gates: usize,
    /// 第 `n` 次调用到闸了（用例可以来读库了）。
    arrived: Arc<tokio::sync::Notify>,
    /// 放行这一次调用。
    release: Arc<tokio::sync::Notify>,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

impl LlmClient for GatedAt {
    fn complete(
        &self,
        request: LlmRequest,
    ) -> futures::future::BoxFuture<'static, agentpipeline_core::Result<AgentResponse>> {
        let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        if n > self.gates {
            return self.inner.complete(request);
        }
        let inner = self.inner.clone();
        let arrived = self.arrived.clone();
        let release = self.release.clone();
        Box::pin(async move {
            arrived.notify_one();
            // `Notify` 存许可：用例先放行、这里后到闸也不会错过（1:1 的节奏里两种顺序都对）。
            release.notified().await;
            inner.complete(request).await
        })
    }
}

/// 在途轮的半截行**当场在库里、随轮推进增长、收口后收成完整行**（票 01，spec 决策 1 / 3）。
///
/// 这条钉的是用户报的那条毛病的服务端半边：此前整轮结束才追加一行 assistant，刷新、
/// 重启之后那一轮的前半段**既不在库里、也补不回来**（SSE 无回放，决策 275）。现在一轮
/// 开工即建半截行，四次台账断言各盯一个时态：
///
/// 1. **刚开工**：库里已有 `user` 行 + `status='in_flight'` 的半截行（一个字都还没说）；
/// 2. **第一轮查完**：半截行长出段序与推理（内容随轮推进增长），`status` 照旧在途；
/// 3. **流式途中**：逐字正文随广播推进现场、一次刷写就进库（spec 决策 3 的那条路）；
/// 4. **收口后**：台账恰两行，assistant 行是完整行——`status` 落空、收口那句在 `content`、
///    段序不重复它（决策 273 的既有形状一字不改）。
///
/// 牙齿：把建行挪回「整轮结束才追加」，第一个断言（`expect`）当场红。
#[tokio::test]
async fn an_inflight_turn_leaves_a_growing_half_row_in_the_ledger() {
    let h = Harness::seeded().await;
    let mut script = Script::new();
    script.for_foreman().read_task("t1").text("t1 还在排队。");
    let arrived = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let llm = Arc::new(GatedAt {
        inner: Arc::new(Thinking {
            inner: FakeAgent::new(script),
            thought: "再核一遍。".into(),
        }),
        gates: 2,
        arrived: arrived.clone(),
        release: release.clone(),
        calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    });
    // 带可读事件录制器的构造：位置戳（票 02）是事件上的字段，要断言就得留着事件。
    let recorder = Arc::new(testkit::SseRecorder::new());
    let runner = Arc::new(ForemanRunner::new(
        h.store.clone(),
        Settings::default(),
        h._home.home().clone(),
        llm,
        recorder.clone() as Arc<dyn agentpipeline_core::sse::SseSink>,
    ));
    let sid = h.session().await;

    let speaker = runner.clone();
    let sid_for_turn = sid.clone();
    let turn = tokio::spawn(async move { speaker.say(Some(&sid_for_turn), "t1 怎么样了？").await });

    // ① 刚开工：第一轮还没发出去，半截行已经当场在库里。
    arrived.notified().await;
    let rows = h.store.list_foreman_messages(&sid, 10, None).await.unwrap();
    let half = rows
        .iter()
        .find(|m| m.role == "assistant")
        .expect("在飞时库里已有这条回话的半截行");
    assert_eq!(
        rows.len(),
        2,
        "开工即两行（人一句 + 在途半截行），不是只有一句人话：{rows:?}"
    );
    assert_eq!(
        half.status.as_deref(),
        Some(FOREMAN_MESSAGE_IN_FLIGHT),
        "半截行要标出自己在途：{half:?}"
    );
    assert!(
        half.content.is_empty() && half.thinking.is_none() && half.segments_json.is_none(),
        "一个字都还没说过：「还没说」不是「说了个空」：{half:?}"
    );

    // 放行第一轮（工具真实执行），停在第二次调用上。
    release.notify_one();
    arrived.notified().await;
    // ② 第一轮查完：段序与推理都长出来了，`status` 照旧在途（内容随轮推进增长）。
    let rows = h.store.list_foreman_messages(&sid, 10, None).await.unwrap();
    let half = rows
        .iter()
        .find(|m| m.role == "assistant")
        .expect("半截行还在");
    assert_eq!(
        half.status.as_deref(),
        Some(FOREMAN_MESSAGE_IN_FLIGHT),
        "第二轮调用在飞：仍是半截行，不是提前收口：{half:?}"
    );
    let raw = half
        .segments_json
        .clone()
        .expect("第一轮查完了：段序随轮推进长出来了");
    let segments: Vec<ForemanSegment> = serde_json::from_value(raw.clone()).unwrap();
    // 结果详情取直播那条 end 事件里的同一份（决策 301：事件与留痕不许各记各的）——
    // 这里没有收口后的 `turn` 可引，recorder 就是那份「直播所见」。
    let end_result: String = recorder
        .events()
        .iter()
        .find_map(|e| match e {
            SseEvent::ToolEvent {
                phase: ToolPhase::End,
                result: Some(r),
                ..
            } => Some(r.clone()),
            _ => None,
        })
        .expect("end 事件要带结果详情（决策 301）");
    assert!(!end_result.is_empty(), "结果详情不是空串");
    assert_eq!(
        segments,
        vec![
            ForemanSegment::Thinking {
                text: "再核一遍。".into()
            },
            ForemanSegment::Tool {
                tool: "read_task".into(),
                // 摘要取聚合那张表里的同一份（同一个事件的两处记录），与决策 273 的
                // 顺序用例同一条判据——这里说的是「工具那一步在半截行里」，不是措辞。
                args_summary: "{\"task_id\":\"t1\"}".into(),
                args: "{\"task_id\":\"t1\"}".into(),
                result: end_result,
                ok: true,
            },
        ],
        "thinking 与工具步骤都在半截行里，与直播所见同构：{raw}"
    );
    assert_eq!(
        half.thinking.as_deref(),
        Some("再核一遍。"),
        "推理也随轮推进落库（此刻只跑过第一次调用，故只有一段）"
    );

    // 位置戳（票 02）：工具事件带着**行 id + 行内序号**广播出去；这一批工具收场之后，
    // 行上的 `seq` 必须**盖过**这些号——快照的 `seq0` 于是能挡住「已经在库里」的那些
    // （不重），又不会挡住「还没进库」的（不漏）。号本身只做去重，不做回放（决策 275）。
    let stamps: Vec<(Option<i64>, Option<u64>)> = recorder
        .events()
        .iter()
        .filter_map(|e| match e {
            SseEvent::ToolEvent { ledger_id, seq, .. } => Some((*ledger_id, *seq)),
            _ => None,
        })
        .collect();
    assert_eq!(
        stamps.len(),
        2,
        "一次工具调用 = start + end 两件：{stamps:?}"
    );
    assert!(
        stamps
            .iter()
            .all(|(ledger, seq)| *ledger == Some(half.id) && seq.is_some()),
        "每件都挂在半截行上、都带行内序号：{stamps:?}"
    );
    let covered = stamps.iter().filter_map(|(_, seq)| *seq).max().unwrap();
    assert!(
        covered as i64 <= half.seq,
        "收场刷写后的行 seq（{}）必须盖过已广播的事件（{}）——否则接缝会丢字",
        half.seq,
        covered
    );

    // ③ 逐字正文**随广播落库**（spec 决策 3）：替身不走流式，故由测试按 provider 的
    // 同一条路推增量（`emit_delta` 就是调这两个入口）——正文当场出现在半截行的
    // 回话位上，与直播所见的「正在说的那一句」同构。
    agentpipeline_core::pipeline::foreman::push_foreman_live_delta(
        &sid,
        agentpipeline_core::sse::Channel::Content,
        "t1 还在排队",
    );
    // 节流拍（250ms）已经由上一次调用边界刷写起过头了：等它过去，下一拍才允许再写——
    // 这段等待本身就是「节流批写」的行为（不是测内部计数，是等一个真实的拍）。
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    agentpipeline_core::pipeline::foreman::flush_foreman_live_turn(&sid).await;
    let rows = h.store.list_foreman_messages(&sid, 10, None).await.unwrap();
    let half = rows
        .iter()
        .find(|m| m.role == "assistant")
        .expect("半截行还在");
    assert_eq!(
        half.content, "t1 还在排队",
        "流式正文推进现场后一次刷写就该在库里（节流拍的第一拍不等）：{half:?}"
    );
    assert_eq!(
        half.status.as_deref(),
        Some(FOREMAN_MESSAGE_IN_FLIGHT),
        "刷写只动在途行的内容，不动它的身份：{half:?}"
    );

    // 放行第二轮：收口。
    release.notify_one();
    let turn = turn.await.unwrap().unwrap();
    assert_eq!(turn.reply, "t1 还在排队。");

    // ④ 收口后：恰两行，assistant 行是完整行——行数语义与今天一致（决策 260 的形状）。
    let rows = h
        .store
        .list_foreman_messages(&turn.session.id, 10, None)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2, "收口后台账仍是「一次回话 = 两行」：{rows:?}");
    let assistant = rows
        .iter()
        .find(|m| m.role == "assistant")
        .expect("收口那一行");
    assert!(
        assistant.status.is_none(),
        "收口即不再是半截行：{:?}",
        assistant.status
    );
    assert_eq!(assistant.content, "t1 还在排队。", "收口那句在 content 上");
    let segments: Vec<ForemanSegment> =
        serde_json::from_value(assistant.segments_json.clone().unwrap()).unwrap();
    // 段序与「整轮结束才落」的从前**逐字同形**（决策 273）：两次调用的推理 + 中间那次工具
    // 调用，而收口那句由 `content` 承载、不进段序——边流边写改的只是中途是否可见。
    assert_eq!(
        segments.len(),
        3,
        "段序仍是中途那些步、收口那句不进去（决策 273 一字不改）：{segments:?}"
    );
    assert!(
        segments
            .iter()
            .all(|s| !matches!(s, ForemanSegment::Text { .. })),
        "收口那句在 content 上，不在段序里：{segments:?}"
    );
    assert_eq!(
        assistant
            .thinking
            .as_deref()
            .unwrap()
            .matches("再核一遍。")
            .count(),
        2,
        "两次模型调用的推理都收在这一行里（按次累积，决策 244）"
    );
}

/// 启动恢复把**上一个进程遗留的悬挂行**标成已中断（票 03，显式修订决策 223）。
///
/// 进程被杀时收口那次写入永远不会发生：库里只剩一条 `status='in_flight'` 的行，界面会
/// 永远以为它「正在说」——比没有读数更坏的假读数（与决策 226③ 根除的是同一类失真，
/// 只是方向相反）。本步只在**启动**时跑（此刻进程里没有任何活跃轮，挂着在途的只可能是
/// 上一个实例留下的），与 `orphan_inflight_model_requests` 同姿势。
///
/// 三条判据各钉一颗牙：
/// 1. 悬挂行标成 `interrupted`、`interrupted_at` 取自 **Clock 接缝**（决策 143——
///    假时钟推进多少就是多少，不是建行那一刻）；
/// 2. **内容原样不动**：thinking 与正文是「断在哪一步」的证据，中断是终态、只加状态；
/// 3. 已收口的行（`status IS NULL`）**一个字都不碰**——谓词丢了 `WHERE status=
///    'in_flight'` 会让正常回话平白背一个中断时刻，断言 3 当场红。
///
/// 牙齿：启动恢复那一步被删掉 → 断言 1 红；改成连内容一起重写 → 断言 2 红。
#[tokio::test]
async fn startup_marks_a_hanging_inflight_row_interrupted_without_touching_content() {
    let h = Harness::empty().await;
    let sid = h.session().await;

    // 上一进程留下的形状：开工建了半截行、跑过一次调用，收口永远没来得及发生。
    let row_id = h.store.begin_foreman_inflight(&sid, None).await.unwrap();
    h.store
        .update_foreman_inflight(
            row_id,
            &InFlightPatch {
                content: "断在这半句".into(),
                thinking: Some("先想了一半".into()),
                segments_json: None,
                traces_json: None,
                prompt_tokens: 0,
                completion_tokens: 0,
                seq: 0,
            },
        )
        .await
        .unwrap();
    // 对照组：同一班里一条**已收口**的行——启动恢复不许碰它。
    let closed_id = h
        .store
        .append_foreman_message(NewForemanMessage {
            session_id: sid.clone(),
            role: "assistant".into(),
            content: "正常收口的回话".into(),
            prompt_tokens: 0,
            completion_tokens: 0,
            briefing_json: None,
            traces_json: None,
            segments_json: None,
            changed_files_json: None,
            thinking: None,
            ask_json: None,
        })
        .await
        .unwrap();

    // 中断时刻走 Clock 接缝：推进到「重启那一刻」，标下来的必须是它。
    h.clock.advance_secs(600);
    let marked = h.store.mark_orphan_foreman_inflights().await.unwrap();
    assert_eq!(marked, 1, "只标那一条悬挂的在途行");

    let rows = h.store.list_foreman_messages(&sid, 10, None).await.unwrap();
    let half = rows.iter().find(|m| m.id == row_id).expect("半截行还在");
    assert_eq!(
        half.status.as_deref(),
        Some(FOREMAN_MESSAGE_INTERRUPTED),
        "悬挂行标成已中断"
    );
    assert_eq!(
        half.interrupted_at,
        Some(h.clock.now()),
        "中断时刻来自 Clock 接缝——假时钟推进了 600s，不是建行那一刻"
    );
    assert_eq!(
        half.content, "断在这半句",
        "中断只加状态：正文是断在哪一步的证据"
    );
    assert_eq!(
        half.thinking.as_deref(),
        Some("先想了一半"),
        "推理原样保留（spec 决策 13：只写状态与时刻，不动内容）"
    );

    let closed = rows.iter().find(|m| m.id == closed_id).expect("收口行还在");
    assert!(
        closed.status.is_none() && closed.interrupted_at.is_none(),
        "已收口的行一个字都不碰（谓词必须钉在 status='in_flight' 上）：{closed:?}"
    );

    // 幂等：再跑一次（第二次启动）标不出新的。
    assert_eq!(
        h.store.mark_orphan_foreman_inflights().await.unwrap(),
        0,
        "已中断是终态，不重复标"
    );
}

/// 刷新之后还能接着看这一轮（决策 260）：**「这一班此刻有没有一轮在跑」是一个可读的读数**。
///
/// 起因是一条实测：值班长正在答话时刷新对讲台，那一轮整段看不见——在途轮的现场
/// （乐观轮 / 流式文本）此前只住在界面那侧，刷新即丢，于是增量到达时无从判断「这一段字属于
/// 谁」，闸门一律不接。补的读数只有一处：`say` 与 `watch` 共用的那个漏斗（`respond`）在
/// 开工时登记、返回时摘掉，`foreman_turn_in_flight` 把它读出来。
///
/// 这一条钉三件事：**在跑时为真**、**跑完为假**、**失败也一样为假**（漏摘的话界面会永远
/// 以为它在说话——比没有这个读数更坏：那是假读数）。
#[tokio::test]
async fn a_running_turn_is_readable_and_drops_the_moment_it_ends() {
    /// 停在模型调用里、直到放行标志翻真才回话的模型。
    ///
    /// **用轮询而不是 `Notify`**：`notify_waiters()` 只唤醒**当时已登记**的等待者，
    /// 「放行信号比等待者先到」那一瞬间会丢信号，用例随之挂死（不是变红，是挂住——
    /// 那种失败最难查）。轮询没有这一格：放行标志是**状态**，晚到的观察者照样看得见。
    struct Gated {
        release: Arc<std::sync::atomic::AtomicBool>,
        fail: bool,
    }
    impl LlmClient for Gated {
        fn complete(
            &self,
            _request: LlmRequest,
        ) -> futures::future::BoxFuture<
            'static,
            agentpipeline_core::Result<agentpipeline_core::agent::client::AgentResponse>,
        > {
            let release = self.release.clone();
            let fail = self.fail;
            Box::pin(async move {
                while !release.load(std::sync::atomic::Ordering::SeqCst) {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                if fail {
                    return Err(Error::Llm("模型没配".into()));
                }
                Ok(AgentResponse {
                    content: Some("答完了。".into()),
                    tool_calls: Vec::new(),
                    prompt_tokens: 3,
                    completion_tokens: 5,
                    ..Default::default()
                })
            })
        }
    }

    let h = Harness::empty().await;
    let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let sid = h.session().await;
    assert!(
        !foreman_turn_in_flight(&sid),
        "什么都没发时不该有「在跑」的读数"
    );

    // 开一轮并**停在模型调用里**：此刻它在跑。
    let turn = tokio::spawn({
        let runner = h.runner_with_llm(Arc::new(Gated {
            release: release.clone(),
            fail: false,
        }));
        let sid = sid.clone();
        async move { runner.say(Some(&sid), "盯着 t1").await }
    });
    // 等登记出现：这一格既等到了「在跑」，也顺带确定这一轮已经开跑。
    for _ in 0..400 {
        if foreman_turn_in_flight(&sid) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        foreman_turn_in_flight(&sid),
        "一轮正在跑时这个读数必须为真——界面刷新后正是靠它重新接上"
    );
    assert!(
        !foreman_turn_in_flight("别的班次"),
        "它说的是**那一班**：别的班次不该被连带说成在跑"
    );

    release.store(true, std::sync::atomic::Ordering::SeqCst);
    turn.await.unwrap().unwrap();
    assert!(
        !foreman_turn_in_flight(&sid),
        "跑完即摘：留着的话界面会永远以为它在说话（假读数比没有更坏）"
    );

    // 失败的那一轮同样摘掉——漏摘的代价与上面同一条（`say` 的失败外框那一趟）。
    let sid2 = h.session().await;
    let handle = tokio::spawn({
        let failing = h.runner_with_llm(Arc::new(Gated {
            release: release.clone(),
            fail: true,
        }));
        let sid2 = sid2.clone();
        async move { failing.say(Some(&sid2), "喂").await }
    });
    for _ in 0..400 {
        if foreman_turn_in_flight(&sid2) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(handle.await.unwrap().is_err(), "这个替身这一轮必然失败");
    assert!(
        !foreman_turn_in_flight(&sid2),
        "失败的一轮也要摘掉登记（提前 `?` 退出那条路）"
    );
}

// ─────────────── 出厂技能点名的主缝三断言（决策 261，票 foreman-operate-pipeline 03）───────────────
//
// 取证一律取**模型真正收到的那一份**（system prompt / 回灌的 messages），不是常量本身：
// 点名在不在、拉没拉、参数同不对，都以请求日志为准（照既有纪律段测试的姿态）。

/// 播一次出厂默认（技能文件 + 点名），与 `serve()` 启动时调的是同一个函数。
async fn seed_defaults(h: &Harness) {
    let seeded =
        agentpipeline_core::agent::factory::seed_factory_defaults(h._home.home(), &h.store)
            .await
            .unwrap();
    assert!(seeded.pointer_written, "无存量配置时应当播下点名");
}

/// 断言①（人的回话轮）：点名每轮在场，且位置在人格段之后、工具纪律段之前（决策 261⑤）。
#[tokio::test]
async fn the_human_turn_prompt_carries_the_skill_pointer() {
    let h = Harness::seeded().await;
    seed_defaults(&h).await;

    let mut script = Script::new();
    script.for_foreman().text("在。");
    let agent = FakeAgent::new(script);
    let requests = agent.clone();
    let runner = h.runner(agent);
    runner.say(None, "在吗").await.unwrap();

    let prompt = requests.request_log()[0].system_prompt.clone();
    let ptr = prompt
        .find(agentpipeline_core::agent::factory::FOREMAN_SKILL_POINTER)
        .unwrap_or_else(|| panic!("人对话轮的 system prompt 要带点名：{prompt}"));
    let persona = prompt.find(FOREMAN_PERSONA).expect("人格段在场");
    let discipline = prompt.find("## 工具纪律").expect("工具纪律段在场");
    assert!(
        persona < ptr && ptr < discipline,
        "点名该在人格段之后、工具纪律段之前：{prompt}"
    );
}

/// 断言①（值守轮）：点名同样在场——值守轮自动轮也要按手册来（决策 261⑤ / spec Q10）。
#[tokio::test]
async fn the_watch_round_prompt_carries_the_skill_pointer() {
    let h = Harness::seeded().await;
    seed_defaults(&h).await;
    note(
        &h,
        "t1",
        agentpipeline_core::storage::AttentionKind::RetryExhausted,
    )
    .await;
    h.clock.advance_secs(61); // 去抖窗口（默认 60s）过了

    let mut script = Script::new();
    script.for_foreman().text("t1 重试耗尽，我按手册备好提议。");
    let agent = FakeAgent::new(script);
    let requests = agent.clone();
    let runner = h.runner(agent);
    runner.watch().await.unwrap().expect("应当醒一次");

    let prompt = requests.request_log()[0].system_prompt.clone();
    assert!(
        prompt.contains(agentpipeline_core::agent::factory::FOREMAN_SKILL_POINTER),
        "值守轮的 system prompt 也要带点名：{prompt}"
    );
}

/// 断言②：脚本让模型发 `Skill(name=operate-pipeline)`，工具**真实执行**——回灌进下一轮
/// messages 的是技能根里那份手册正文（脚本从没写过它一个字）。
#[tokio::test]
async fn pulling_the_factory_handbook_returns_the_real_body_from_disk() {
    let h = Harness::seeded().await;
    let seeded =
        agentpipeline_core::agent::factory::seed_factory_defaults(h._home.home(), &h.store)
            .await
            .unwrap();
    assert!(
        seeded
            .skills_written
            .iter()
            .any(|s| s == "operate-pipeline"),
        "技能文件应当本次种入：{seeded:?}"
    );

    let mut script = Script::new();
    script
        .for_foreman()
        .tool("Skill", serde_json::json!({"name": "operate-pipeline"}));
    script.for_foreman().text("手册已读，按它执行。");
    let agent = FakeAgent::new(script);
    let requests = agent.clone();
    let runner = h.runner(agent);
    let turn = runner.say(None, "把 t1 推进一步").await.unwrap();

    assert_eq!(turn.traces.len(), 1, "{:?}", turn.traces);
    assert_eq!(turn.traces[0].tool, "Skill");
    assert!(turn.traces[0].ok, "Skill 工具应执行成功");

    let log = requests.request_log();
    assert_eq!(log.len(), 2, "一次工具往返 = 两次 LLM 请求");
    let fed = serde_json::to_string(&log[1].messages).unwrap();
    // 手册正文里的标志句——只可能来自磁盘上那份 SKILL.md
    assert!(
        fed.contains("确认纪律：一切写操作只产提议，永不绕过按键"),
        "回灌的应是手册正文：{fed}"
    );
    assert!(fed.contains("逐票一卡"), "回灌的应是手册正文：{fed}");
    // frontmatter 已剥（`load_body` 的口径）：回灌正文，不是带头的原文件
    assert!(
        !fed.contains("name: operate-pipeline"),
        "frontmatter 不该回灌进正文：{fed}"
    );
}

/// 断言③：拉完手册后产出的 `task` 提议，args 与配对端点按钮直发的参数**逐字一致**。
///
/// 「逐字」的机制在 `routes/foreman.rs::run_task_tool`：它把 args **逐字段原样**搬进
/// `tasks::ResumeBody`——按键 POST `/tasks/{id}/resume` 直发的就是这份形，没有第二套参数
/// 语言。故 JSON 全等（含字节：两边都按 canonical 形比 `to_string`）。
#[tokio::test]
async fn the_proposal_args_are_the_parameters_the_button_would_send() {
    let h = Harness::seeded().await;
    seed_defaults(&h).await;
    park_task(
        &h.store,
        "t1",
        PendingKind::RetryExhausted,
        "重试耗尽，等你拍板",
    )
    .await;

    let mut script = Script::new();
    script
        .for_foreman()
        .tool("Skill", serde_json::json!({"name": "operate-pipeline"}));
    script.for_foreman().tool(
        "task",
        serde_json::json!({"action": "resume", "task_id": "t1", "resume_action": "continue"}),
    );
    script.for_foreman().text("提议已备好，等你按键。");
    let runner = h.runner(FakeAgent::new(script));
    let turn = runner.say(None, "把 t1 推进一步").await.unwrap();

    assert_eq!(turn.traces.len(), 2, "{:?}", turn.traces);
    assert_eq!(turn.traces[0].tool, "Skill");
    assert_eq!(turn.traces[1].tool, "task");
    assert!(turn.traces[1].ok);

    let pending = h
        .store
        .list_pending_foreman_proposals(&turn.session.id)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1, "应当恰好一张提议：{pending:?}");
    assert_eq!(pending[0].tool, "task");

    let expected = serde_json::json!({
        "action": "resume",
        "task_id": "t1",
        "resume_action": "continue"
    });
    assert_eq!(pending[0].args, expected, "提议参数要与按钮直发的参数同源");
    assert_eq!(
        pending[0].args.to_string(),
        expected.to_string(),
        "逐字一致：按 canonical 字节比，不只比结构"
    );
}

// ──────────────── 值班长回话线 → 离线通知（决策 272②③④）────────────────

/// 轮询等到第 `want` 次命中（投递是 best-effort 后台任务；照 `notify.rs` 的同款）。
async fn wait_hits(server: &TinyHttp, want: usize, ms: u64) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_millis(ms);
    while std::time::Instant::now() < deadline {
        if server.hits() >= want {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    server.hits() >= want
}

/// 挂一个 generic webhook 出口。礼貌用的时钟与 store 的时钟**分开**：免打扰按出口
/// 自己的钟算，这里拨到本地正午——任何时区下都确定不在 `[22, 8)` 免打扰段里。
fn attach_notifier(h: &Harness, server: &TinyHttp) {
    h.store
        .set_notifier(Arc::new(agentpipeline_core::notify::WebhookNotifier::new(
            agentpipeline_core::notify::NotifyTarget::Webhook {
                url: server.url("/hook"),
                format: agentpipeline_core::notify::NotifyFormat::Generic,
            },
            crate::notify::politeness(300, [22, 8]),
            Arc::new(ManualClock::new(crate::notify::at_local_hour(12))),
            h.store.clone(),
        )));
}

#[tokio::test]
async fn a_work_heavy_say_turn_announces_the_reply_line() {
    let h = Harness::empty().await;
    let sid = h.session().await;
    let server = TinyHttp::spawn("200", "application/json", b"{}".to_vec(), Duration::ZERO);
    attach_notifier(&h, &server);

    // 三次工具调用 = 过 `FOREMAN_REPLY_MIN_TOOL_CALLS` 的门（查不存在的任务是正常回答，
    // 故不需要真任务；「查不到」的文本回答恰好也证明正文来自模型的收口轮）。
    let mut script = Script::new();
    script
        .for_foreman()
        .read_task("t-none-1")
        .read_task("t-none-2")
        .read_task("t-none-3")
        .text("三张卡都查完了：台账里没有这三张。");
    let runner = h.runner(FakeAgent::new(script));
    runner.say(Some(&sid), "帮我查三张卡的状态").await.unwrap();

    assert!(
        wait_hits(&server, 1, 8_000).await,
        "过门的回话轮应当出站一条通知"
    );
    let payload: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&server.body())).unwrap();
    assert_eq!(payload["kind"], "foreman_reply", "{payload}");
    assert!(
        payload.get("task_id").is_none(),
        "回话线没有 task_id：省略而不是哨兵（272②）— {payload}"
    );
    // 会话名在 title 里（多会话时否则不知是哪一轮，272⑤）
    let session = h
        .store
        .get_foreman_session(&sid)
        .await
        .unwrap()
        .expect("会话应当存在");
    assert!(
        payload["title"].as_str().unwrap().contains(&session.title),
        "{payload}"
    );
    // 正文带回话原文（268④ 的本人通道豁免只给这一段，272⑤）
    assert!(
        payload["body"].as_str().unwrap().contains("三张卡都查完了"),
        "{payload}"
    );
}

#[tokio::test]
async fn a_short_say_turn_never_touches_the_reply_line() {
    let h = Harness::seeded().await;
    let sid = h.session().await;
    let server = TinyHttp::spawn("200", "application/json", b"{}".to_vec(), Duration::ZERO);
    attach_notifier(&h, &server);

    let mut script = Script::new();
    script.for_foreman().text("在的。");
    let runner = h.runner(FakeAgent::new(script));
    runner.say(Some(&sid), "在吗").await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        server.hits(),
        0,
        "零工具的快轮不通知，且连 cooldown 槽都不碰（门在 notify() 之前，272③）"
    );

    // 金丝雀：attention 线照常出站——出口真的挂着，静默是门干的，不是没接上。
    h.store
        .note_attention(
            "t1",
            agentpipeline_core::storage::AttentionKind::TaskDone,
            h.clock.now(),
            None,
        )
        .await
        .unwrap();
    assert!(
        wait_hits(&server, 1, 8_000).await,
        "金丝雀（TaskDone）应当出站"
    );
}

#[tokio::test]
async fn a_watch_broadcast_announces_without_the_gate_and_a_silent_one_stays_quiet() {
    let h = Harness::seeded().await;
    let server = TinyHttp::spawn("200", "application/json", b"{}".to_vec(), Duration::ZERO);

    // 待办**先记、出口后挂**：否则 attention 线自己的通知（RetryExhausted → failed 恒发）
    // 会先占一次命中，两条线就分不开了。
    note(
        &h,
        "t1",
        agentpipeline_core::storage::AttentionKind::RetryExhausted,
    )
    .await;
    h.clock.advance_secs(61);
    attach_notifier(&h, &server);

    // 静默轮：零工具 + 【无需处理】——恒不通知（272③）。
    let mut silent = Script::new();
    silent
        .for_foreman()
        .text("【无需处理】自行恢复，无需打扰。");
    let runner = h.runner(FakeAgent::new(silent));
    assert!(runner.watch().await.unwrap().is_none(), "静默轮不播报");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(server.hits(), 0, "静默轮恒不通知");

    // 播报轮：零工具也**恒通知**（它本就是「没人在场」的定义，272③——不设 traces 门）。
    // 这一笔 note 的 attention 线通知（TaskStale → pending 类）与回话线通知并存，
    // 故总数是 2；TinyHttp 只留最后一次报文体，用它认出**回话线**那条真的到了。
    // 换 t2 记待办：t1 刚被消费过、在同任务冷却里（决策 209⑤），新事件不单独唤醒。
    h.task("t2").await;
    note(
        &h,
        "t2",
        agentpipeline_core::storage::AttentionKind::TaskStale,
    )
    .await;
    h.clock.advance_secs(61);
    let mut broadcast = Script::new();
    broadcast
        .for_foreman()
        .text("t1 卡住了，需要值班经理看一眼。");
    let runner = h.runner(FakeAgent::new(broadcast));
    assert!(runner.watch().await.unwrap().is_some(), "播报轮照旧");
    assert!(
        wait_hits(&server, 2, 8_000).await,
        "attention 线 + 回话线各一条：hits={}",
        server.hits()
    );
    let payload: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&server.body())).unwrap();
    assert_eq!(
        payload["kind"], "foreman_reply",
        "最后一条是回话线的播报通知：{payload}"
    );
}

#[tokio::test]
async fn a_failed_say_turn_announces_the_failure_line_with_only_the_kind() {
    let h = Harness::seeded().await;
    let sid = h.session().await;
    let server = TinyHttp::spawn("200", "application/json", b"{}".to_vec(), Duration::ZERO);
    attach_notifier(&h, &server);

    // 模型当场报错（照 `FailingLlm` 先例）：失败收口必通知（272③），类 = failed。
    let runner = ForemanRunner::new(
        h.store.clone(),
        Settings::default(),
        h._home.home().clone(),
        Arc::new(FailingLlm) as Arc<dyn LlmClient>,
        Arc::new(testkit::SseRecorder::new()),
    );
    assert!(runner.say(Some(&sid), "动手吧").await.is_err());
    assert!(
        wait_hits(&server, 1, 8_000).await,
        "失败收口应当出站一条通知"
    );
    let payload: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&server.body())).unwrap();
    assert_eq!(payload["kind"], "foreman_reply_failed", "{payload}");
    let body = payload["body"].as_str().unwrap();
    assert!(body.contains("llm_network"), "正文只带类别：{body}");
    // 268④ 的纪律对失败线**不豁免**（272⑤ 的豁免只给回话正文）：错误原文不出网。
    assert!(
        !body.contains("连接被对端关掉"),
        "raw 原文一个字不出网：{body}"
    );
}

// ─────────────────── 真实台账回放（决策 309 / 310 / 311，票 foreman-burns 04）───────────────────

/// **回放 2026-09-27 那本台账**：三道护栏各自该在第几格响，用**真实读数**算给他看。
///
/// **诚实标注（票面要求原样写进测试，不许含糊）**：
/// - **真实的部分**：94 条 `prompt_tokens` 读数（首 6,602 → 末 561,210，逐条抄自
///   `kanban_model_requests`）、95 行里那条 `timeout` 无读数、**0 条提议**、
///   433,628 生成 token、窗口下界 576,210 的推导——都来自那本台账。
/// - **构造的部分**：那次会话的 `kanban_foreman_messages.traces_json` **只存了工具名 +
///   参数摘要 + 成败、没存结果正文**，而这里要回放的「每次调用的字节都不同」这一面需要
///   结果正文。故**按真实长度与真实形状构造**：次数按台账的 95 轮、每次结果都是新字节
///   （正是判据② 看不到的那一面）、落库状态一次没动（0 条提议 / 任务状态与游标都没变）。
/// - **效力因此有边界**：它证的是「**在真实读数与真实形状下，判据会在它该响的那一格响**」，
///   不是「那天系统真的会这么做」（那要接真模型跑一遍，属 opt-in，不进默认门）。
#[tokio::test]
async fn replaying_the_incident_ledger_turns_the_three_guards_on() {
    use agentpipeline_core::agent::loops::{self, CallRecord, Loop, STALLED_STATE_LIMIT};
    use agentpipeline_core::pipeline::window_calibration::{
        trigger_line_in_real_tokens, COMPACT_TRIGGER_RATIO, INCIDENT_PROMPT_TOKENS,
        MEASURED_UNDERESTIMATE_FACTOR, MISCONFIGURED_WINDOW, PROVIDER_WINDOW_LOWER_BOUND,
    };

    // ── 护栏一：窗口界该在第几轮触发 ────────────────────────────────────────
    //
    // 触发线（估算侧）= 窗口的 80%；折算成真实 token 就是 `0.8 × 窗口 × 低估倍数`。
    // 有读数的台账里，**第一次越过这条线的那一格**就是压缩该发生的地方。
    let first_over = |line: f64| {
        INCIDENT_PROMPT_TOKENS
            .iter()
            .position(|r| *r as f64 > line)
            .map(|i| i + 1)
    };

    // 标定之后：线落在真实窗口的 80% → 会在末段那几轮触发（不是 0 次）。
    let calibrated_line = trigger_line_in_real_tokens(PROVIDER_WINDOW_LOWER_BOUND, 1.0);
    let n = first_over(calibrated_line)
        .unwrap_or_else(|| panic!("标定之后必须触得到：线 = {calibrated_line}"));
    assert!(
        n > INCIDENT_PROMPT_TOKENS.len() / 2,
        "它该在**后段**才触发（前面还很宽裕）：第 {n} 格"
    );
    assert!(
        n <= INCIDENT_PROMPT_TOKENS.len(),
        "台账里必须有一格越过它：第 {n} 格"
    );

    // 事故当时那一对（配置 128,000 + 低估 5.5 倍）：线压在真实窗口边缘，
    // 台账里**没有一格**明显越过它——「95 轮一次都没触发」就是这么来的。
    let broken_line =
        trigger_line_in_real_tokens(MISCONFIGURED_WINDOW, MEASURED_UNDERESTIMATE_FACTOR);
    assert!(
        broken_line > COMPACT_TRIGGER_RATIO * PROVIDER_WINDOW_LOWER_BOUND as f64,
        "旧的那条线高过标定后的（所以它才是「太晚」）：{broken_line}"
    );
    assert!(
        first_over(broken_line).is_none()
            || first_over(broken_line) == Some(INCIDENT_PROMPT_TOKENS.len()),
        "旧的那条线要么一次都碰不到、要么只在最后一格：{:?}",
        first_over(broken_line)
    );

    // ── 护栏二：判据③ 该在哪一格提醒，且只提醒一次 ────────────────────────────
    //
    // 回的流按台账形状构造：每轮一次调用（台账 95 行的量级）、**每次结果都是新字节**
    // （判据② 因此 0 次命中——那正是那天的实况），而落库状态一次没动（0 条提议）。
    let mut stream: Vec<CallRecord> = (0..INCIDENT_PROMPT_TOKENS.len())
        .map(|i| CallRecord {
            tool: "read_file".into(),
            arguments: format!(r#"{{"path":"logs/serve.log","offset":{i}}}"#),
            // 每次的字节都不同（偏移 / updated_at / sqlite 行）——别用同一个结果，
            // 那会变成判据② 的假命中，回放就不再是那天的形状了。
            result_digest: loops::result_digest(&format!(
                "第 {i} 段：updated_at=2026-09-27T0{}:00Z prompt={}",
                i % 10,
                INCIDENT_PROMPT_TOKENS[i]
            )),
            state_digest: loops::result_digest("s0"),
        })
        .collect();

    assert_eq!(
        loops::detect(&stream[..STALLED_STATE_LIMIT - 1]),
        None,
        "没攒满一段之前不响"
    );
    let hit = loops::detect(&stream[..STALLED_STATE_LIMIT]).expect("攒满第 12 次就该提醒");
    assert!(matches!(hit, Loop::Stalled { .. }), "{hit:?}");
    assert!(hit.is_remind_only(), "判据③ 只提醒、**不收口**（决策 310）");

    // 「只提醒一次」的机制在调用方：提醒过的那一段被划掉（`loop_reminded_at`），
    // 于是下一次只在**又攒满一整段**时才再提醒——这里按同一把尺回放一次。
    let reminded_at = STALLED_STATE_LIMIT;
    assert_eq!(
        loops::detect(&stream[reminded_at..reminded_at + STALLED_STATE_LIMIT - 1]),
        None,
        "刚提醒过的那一段划掉之后，下一段也要攒满才响"
    );
    assert!(
        loops::detect(&stream[reminded_at..reminded_at + STALLED_STATE_LIMIT]).is_some(),
        "又攒满一整段照样会提醒（提醒级、不做第二次升级）"
    );

    // 一路到最后都只是 `Stalled`：**整本台账里收口一次都不该发生**（收口归 293 原两条判据）。
    assert!(
        matches!(
            loops::detect(&stream[reminded_at..]).unwrap_or(Loop::Stalled { times: 0 }),
            Loop::Stalled { .. }
        ),
        "这条判据在任何时候都不升级成收口"
    );

    // ── 护栏三：收场文案按实际提议数分支 ────────────────────────────────────
    //
    // 那本台账的提议数是 **0**（本用例前面的断言已把「真实」这一半写死），故那天该说的是
    // 「本轮没有提任何提议」而不是「提议都还在，照样可以按」。两个分支各自端到端的断言在
    // `pressing_stop_before_it_says_anything_still_leaves_a_row`（0 条）与
    // `the_closeout_keeps_the_proposal_note_when_the_round_proposed_something`（1 条）里。
    let _ = stream.pop();
}

// ─────────────────── 收口行带机器读出的改动清单（票 01）───────────────────
//
// 决策 311（「收场文案按实际提议数说话」）的下一代：同一件事从「提议数」扩到「改动文件」。
// 2026-10-08 那本账里缺的正是它——394 那一轮改了二十余处，8 分钟后的下一轮**如实**汇报
// 「仍未动一行代码」，因为进下一轮 prompt 的只有上一轮的收口散文，而那段散文一个字都没提
// 它改过什么。

/// 一轮真的改过文件时，**台账行自己说出来**：结构化列 + 正文末尾那一行。
#[tokio::test]
async fn the_closeout_lists_the_files_the_round_changed() {
    let h = Harness::seeded().await;
    // `write_file` / `edit_file` 归环境层：值班长的**缺省档位是 `ask`**，而那一档下它们
    // 只落成一条提议、文件一个字节都不动（`gate_decision`）——故本用例显式配 `auto`。
    h.foreman_env(agentpipeline_core::types::EnvMode::Auto)
        .await;
    let sid = h.session().await;

    let mut script = Script::new();
    script.for_foreman().tool(
        "write_file",
        serde_json::json!({"path": "notes.md", "content": "x"}),
    );
    // 同一个文件改第二次：清单里只该出现一次（去重），且保首次出现序。
    script.for_foreman().tool(
        "edit_file",
        serde_json::json!({"path": "notes.md", "old_text": "x", "new_text": "y"}),
    );
    script.for_foreman().tool(
        "write_file",
        serde_json::json!({"path": "other.md", "content": "z"}),
    );
    script.for_foreman().text("写完了。");
    let runner = h.runner(FakeAgent::new(script));
    runner.say(Some(&sid), "写两个文件").await.unwrap();

    let rows = h.store.list_foreman_messages(&sid, 50, None).await.unwrap();
    let said = rows
        .iter()
        .find(|m| m.role == "assistant")
        .expect("一轮回话应当落一条 assistant 行");
    assert_eq!(
        said.changed_files_json,
        Some(serde_json::json!(["notes.md", "other.md"])),
        "结构化清单应当是这两个文件、且不重复"
    );
    assert!(
        said.content
            .ends_with("【本轮改动】本轮改了 2 个文件：notes.md、other.md。"),
        "收口行应当以改动清单收尾：{}",
        said.content
    );
}

/// 一轮没动过文件：**那一段不出现**，结构化列是 `null` 而不是空数组
/// （「没有」与「有但是空的」是两件事，与 `briefing_json` / `traces_json` 同一条口径）。
#[tokio::test]
async fn a_round_that_changed_nothing_says_nothing_about_changes() {
    let h = Harness::seeded().await;
    h.foreman_env(agentpipeline_core::types::EnvMode::Auto)
        .await;
    let sid = h.session().await;

    let mut script = Script::new();
    script.for_foreman().tool("list_dir", serde_json::json!({}));
    script.for_foreman().text("看完了，什么都没动。");
    let runner = h.runner(FakeAgent::new(script));
    runner.say(Some(&sid), "看看").await.unwrap();

    let rows = h.store.list_foreman_messages(&sid, 50, None).await.unwrap();
    let said = rows
        .iter()
        .find(|m| m.role == "assistant")
        .expect("一轮回话应当落一条 assistant 行");
    assert!(
        said.changed_files_json.is_none(),
        "没动文件就不该有这一列：{:?}",
        said.changed_files_json
    );
    assert!(
        !said.content.contains("【本轮改动】"),
        "没动文件就不该出现那一段：{}",
        said.content
    );
}

/// 正文声称「没改代码」而清单非空：机器**补一句更正**，模型的话**一字不动**
/// （与在打转 / 成本告警 / 归因三处「标注而非改写」同一条姿态）。
#[tokio::test]
async fn a_no_change_claim_is_annotated_not_rewritten() {
    let h = Harness::seeded().await;
    h.foreman_env(agentpipeline_core::types::EnvMode::Auto)
        .await;
    let sid = h.session().await;

    let mut script = Script::new();
    script.for_foreman().tool(
        "write_file",
        serde_json::json!({"path": "notes.md", "content": "x"}),
    );
    script.for_foreman().text("收口。本轮零改动，闸门未跑。");
    let runner = h.runner(FakeAgent::new(script));
    runner.say(Some(&sid), "改吧").await.unwrap();

    let rows = h.store.list_foreman_messages(&sid, 50, None).await.unwrap();
    let said = rows
        .iter()
        .find(|m| m.role == "assistant")
        .expect("一轮回话应当落一条 assistant 行");
    assert!(
        said.content.starts_with("收口。本轮零改动，闸门未跑。"),
        "模型自己的话必须原样留在开头（标注而不是改写）：{}",
        said.content
    );
    assert!(
        said.content.contains(
            "【本轮改动】上面那句说这一轮没改代码，与台账不符——本轮改了 1 个文件：notes.md。"
        ),
        "清单非空却称零改动时，机器该补一句更正：{}",
        said.content
    );
}

/// **本票的全部意义**：上一轮的改动清单进得去**下一轮的 prompt**——值班长认识「我做过什么」
/// 只有历史这一个入口，而历史里此前只有散文。
#[tokio::test]
async fn the_changes_line_reaches_the_next_rounds_prompt() {
    let h = Harness::seeded().await;
    h.foreman_env(agentpipeline_core::types::EnvMode::Auto)
        .await;
    let sid = h.session().await;

    let mut script = Script::new();
    script.for_foreman().tool(
        "write_file",
        serde_json::json!({"path": "notes.md", "content": "x"}),
    );
    script.for_foreman().text("写完了。");
    script.for_foreman().text("第二轮的收口。");
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());
    runner.say(Some(&sid), "写个文件").await.unwrap();
    runner.say(Some(&sid), "再来一轮").await.unwrap();

    let reqs = agent.request_log();
    let last = serde_json::to_string(&reqs.last().expect("第二轮必须发过请求").messages).unwrap();
    assert!(
        last.contains("【本轮改动】本轮改了 1 个文件：notes.md。"),
        "上一轮的改动清单必须进下一轮的历史：{last}"
    );
}
