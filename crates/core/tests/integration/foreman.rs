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

use agentpipeline_core::agent::client::{LlmClient, LlmRequest};
use agentpipeline_core::clock::Clock;
use agentpipeline_core::config::Settings;
use agentpipeline_core::metrics;
use agentpipeline_core::pipeline::foreman::{
    build_briefing, foreman_tool_names, situation_fingerprint, trim_history, ForemanRunner,
    ForemanToolLayer, FOREMAN_AGENT_TYPE, FOREMAN_FAILED_TURN_MARK, FOREMAN_MAX_ROUNDS,
    FOREMAN_PERSONA, FOREMAN_STAGE_KEY, FOREMAN_TOOL_SPECS, FOREMAN_WATCH_MARK, OPERATION_LOG_MARK,
};
use agentpipeline_core::storage::foreman::NewForemanMessage;
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

struct Harness {
    _home: TestHome,
    store: Store,
    clock: ManualClock,
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

    /// 最近活动的未归档会话 id（值守轮自己挑的那个）。
    async fn latest_session(&self) -> String {
        self.store
            .latest_foreman_session()
            .await
            .unwrap()
            .expect("应当已有会话")
            .id
    }

    fn runner(&self, agent: FakeAgent) -> ForemanRunner {
        self.runner_with(Settings::default(), agent)
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
            })
            .await
            .unwrap();
    }
    let all = h.store.list_foreman_messages(&sid, 100).await.unwrap();
    assert_eq!(all.len(), 3);

    let kept = trim_history(&all, 250);
    assert_eq!(kept.len(), 2, "预算 250 只装得下两条 100 字的");
    assert_eq!(kept[0].id, all[1].id, "保留的应是最新的两条");
    assert_eq!(kept[1].id, all[2].id);

    // 被裁掉的历史**仍在库里**：裁剪只影响这一轮注入了什么。
    let after = h.store.list_foreman_messages(&sid, 100).await.unwrap();
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
    let all = h.store.list_foreman_messages(&sid, 100).await.unwrap();

    // 预算小到连最新一条都装不下——仍必须保留它，否则值班长会答非所问。
    let kept = trim_history(&all, 10);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].id, all[1].id);
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
    let tail = h.store.list_foreman_messages(&sid, 2).await.unwrap();
    // LIMIT 必须作用在最新那一端，再按时间升序交出去。
    assert_eq!(tail.len(), 2);
    assert_eq!(tail[0].content, "第3句");
    assert_eq!(tail[1].content, "第4句");
    assert!(h
        .store
        .list_foreman_messages(&sid, 0)
        .await
        .unwrap()
        .is_empty());
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
    let messages = h.store.list_foreman_messages(&sid, 100).await.unwrap();
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
    let after = h.store.list_foreman_messages(&sid, 100).await.unwrap();
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
    let messages = h.store.list_foreman_messages(&sid, 100).await.unwrap();
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

/// 挂住的模型流不再把这一轮无限拖住（2026-09-18 实测的形状）。
///
/// 现场那一轮没有 run 行、也就没有任何时限约束：模型流一挂住，`say()` 就一直挂在
/// `stream.next()` 上，`Err` 到不了、失败外框也不执行——库里只剩一条孤立的用户行。
///
/// 牙齿：把 `ForemanRunner::respond` 的 `tokio::time::timeout` 摘掉，这个用例会挂死在
/// 这里（`Step::Stall` 的 future 永不返回）。
#[tokio::test]
async fn a_hung_model_is_bounded_and_recorded() {
    let h = Harness::empty().await;
    let mut script = Script::new();
    script.for_foreman().stall();
    let runner = h
        .runner(FakeAgent::new(script))
        .with_turn_timeout(std::time::Duration::from_millis(1200));
    let sid = h.session().await;

    let err = runner.say(Some(&sid), "盯着 t1").await.unwrap_err();
    match &err {
        // 时限按人话写出来（不足一分钟说秒）：账里那一句是人判断「挂了多久」的凭据。
        Error::Llm(m) => {
            assert!(m.contains("没有结束"), "时限说明要读得懂：{m}");
            assert!(m.contains("1 秒"), "要带出实际的时限：{m}");
        }
        other => panic!("应当是「这一轮没结束」这一类失败，实际：{other:?}"),
    }

    // 用户那一句 + 失败那一句：这一轮**不再**只剩孤立的用户行。
    let messages = h.store.list_foreman_messages(&sid, 100).await.unwrap();
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert_eq!(messages[0].role, "user");
    assert!(
        messages[1].content.starts_with(FOREMAN_FAILED_TURN_MARK)
            && messages[1].content.contains("没有结束"),
        "失败那一行要说清是时限：{}",
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

    let messages = h.store.list_foreman_messages(&sid, 100).await.unwrap();
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

    let messages = h.store.list_foreman_messages(&sid, 100).await.unwrap();
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
/// （决策 182④）。这正是 2026-09-18 那两轮的形状（决策 224）。
///
/// 脚本按常量声明而不是写死轮数：本用例钉的是「耗尽就报这一类别、并且落一条可归因的账、
/// 模型确实被叫了整整数轮」，不是「上限恰好是几」——那个数没有用例值得钉。
#[tokio::test]
async fn a_foreman_that_never_wraps_up_is_capped_and_named() {
    let h = Harness::empty().await;
    let mut script = Script::new();
    for _ in 0..FOREMAN_MAX_ROUNDS {
        script
            .for_foreman()
            .tool("read_task", serde_json::json!({"task_id": "t1"}));
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

    let messages = h.store.list_foreman_messages(&sid, 100).await.unwrap();
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
    assert!(h.store.list_foreman_sessions().await.unwrap().is_empty());

    let sid = h.session().await;
    assert!(runner.say(Some(&sid), "   ").await.is_err());
    assert!(h
        .store
        .list_foreman_messages(&sid, 10)
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
        .list_foreman_messages(&turn.session.id, 100)
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
        .list_foreman_messages(&turn.session.id, 100)
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
            .list_foreman_messages(&turn.session.id, 100)
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
        .list_foreman_messages(&sid, 100)
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
    let sid = h.store.latest_foreman_session().await.unwrap().unwrap().id;
    assert!(
        h.store
            .list_foreman_messages(&sid, 100)
            .await
            .unwrap()
            .is_empty(),
        "静默 = 不落播报行"
    );
    // 但这件事**被处理过了**：不消费的话它会一夜被反复唤醒
    assert!(h.store.open_attention(100).await.unwrap().is_empty());
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
        .list_foreman_messages(&turn.session.id, 100)
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
        .list_foreman_messages(&h.latest_session().await, 100)
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
        .list_foreman_messages(&h.latest_session().await, 100)
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
    for forbidden in ["read_conversation", "run_command"] {
        assert!(
            !watch_tools.contains(&forbidden.to_string()),
            "自动轮不该拿到 {forbidden}：{watch_tools:?}"
        );
    }
    assert!(
        watch_tools.contains(&"read_diagnosis".to_string()),
        "自动轮仍要看得到诊断包（它是台账类）：{watch_tools:?}"
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
    for wanted in ["read_conversation", "run_command"] {
        assert!(
            human_tools.contains(&wanted.to_string()),
            "被追问时该拿得到 {wanted}：{human_tools:?}"
        );
    }
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
    silent.for_foreman().text("【无需处理】");
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
    assert!(
        fed_back.contains("已截断"),
        "超过 12k 要留截断标记，不许静默截短"
    );
    assert!(
        fed_back.contains("闸门失败：测试命令退出码 1"),
        "失败原因是重点证据，必须在截断后的前 12k 里：{}",
        &fed_back[..fed_back.len().min(400)]
    );
    assert!(
        fed_back.contains("重试耗尽"),
        "pending 原因必须在截断后的前 12k 里"
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
        .list_foreman_messages(&turn.session.id, 100)
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

#[tokio::test]
async fn the_foreman_tool_set_matches_the_frozen_contract() {
    // 清单的**名字与层级**逐条钉住：这是安全边界本身（`foreman.rs` 的注释原话），
    // 加一个工具必须先改这里，从而在任何 diff 里显式可见。
    assert_eq!(
        foreman_tool_names(ForemanToolLayer::Read),
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
        ]
    );
    assert_eq!(
        foreman_tool_names(ForemanToolLayer::Write),
        [
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

/// 三处措辞（`FOREMAN_PERSONA` / `FOREMAN_BASELINE` / 工具纪律段）按档位说真话，
/// **取证取的是模型真正收到的那一份 system prompt**（不是常量本身）。
///
/// 这一条盯的是一个会让模型开始说谎的失效：上一版纪律段写的是「读不到文件系统，也不能执行
/// 命令」——那在 B / C / E 层落地之后是**假的**，而一段假的能力说明会直接变成一句假话
/// （「我读过那个文件」）。三档各取一轮请求，逐句核。
#[tokio::test]
async fn the_system_prompt_tells_the_truth_about_what_it_can_do_in_each_tier() {
    /// 跑一轮回话，返回模型收到的 system prompt。
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
        })
        .await
        .unwrap();

    let (tokens, calls) = h.store.foreman_session_totals(&sid).await.unwrap();
    // 求和而不是另存计数器：计数器会与台账漂移，求和永远等于真实存在的东西。
    assert_eq!(tokens, 175);
    // calls 是**回话次数**（assistant 行），不是工具往返次数。
    assert_eq!(calls, 2);
}

#[tokio::test]
async fn maintenance_purges_foreman_messages_past_the_retention_window() {
    let h = Harness::seeded().await;
    let sid = h.session().await;
    h.store
        .append_foreman_user_message(&sid, "很久以前说的")
        .await
        .unwrap();

    // 未到期：一条不少。
    let purged = h
        .store
        .purge_foreman_messages(h.clock.now() - chrono::Duration::days(30))
        .await
        .unwrap();
    assert_eq!(purged, 0);
    assert_eq!(
        h.store.list_foreman_messages(&sid, 10).await.unwrap().len(),
        1
    );

    // 假时钟推进 31 天（等不了真实 30 天）；清理按创建时间判年龄，与任务终态无关
    // ——值班长对话不挂任务，没有「任务还没结束所以先留着」这一说。
    h.clock.advance_secs(31 * 24 * 3600);
    let purged = h
        .store
        .purge_foreman_messages(h.clock.now() - chrono::Duration::days(30))
        .await
        .unwrap();
    assert_eq!(purged, 1);
    assert!(h
        .store
        .list_foreman_messages(&sid, 10)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn scheduler_maintenance_reports_foreman_purges_separately() {
    let h = Harness::seeded().await;
    let sid = h.session().await;
    h.store
        .append_foreman_user_message(&sid, "昨晚说的")
        .await
        .unwrap();
    h.clock.advance_secs(31 * 24 * 3600);

    use agentpipeline_core::scheduler::KanbanScheduler;
    let scheduler = KanbanScheduler::new(
        h.store.clone(),
        Settings::default(),
        Arc::new(h.clock.clone()),
        Arc::new(testkit::RecordingKiller::new()),
        Arc::new(testkit::SseRecorder::new()),
        Arc::new(|_: &str| {}),
    );
    let report = scheduler.maintenance().await.unwrap();
    // 两类分开计数——混成一个数就看不出是哪一类在增长。
    assert_eq!(report.purged_foreman_messages, 1);
    assert_eq!(report.purged_conversations, 0);
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
        })
        .await
        .unwrap();
    h.store
        .append_foreman_user_message(&b, "乙班第一句")
        .await
        .unwrap();

    let in_a = h.store.list_foreman_messages(&a, 100).await.unwrap();
    let in_b = h.store.list_foreman_messages(&b, 100).await.unwrap();
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

    let list = h.store.list_foreman_sessions().await.unwrap();
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

    let list = h.store.list_foreman_sessions().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, b, "归档的不在列表里，剩下的照旧");

    assert_eq!(
        h.store.list_foreman_messages(&a, 100).await.unwrap().len(),
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
        .list_foreman_messages(&sid, 10)
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
    assert!(h.store.list_foreman_sessions().await.unwrap().is_empty());
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
        .list_foreman_messages(&session_id, 50)
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
