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
    build_briefing, foreman_tool_names, trim_history, ForemanRunner, ForemanToolLayer,
    FOREMAN_AGENT_TYPE, FOREMAN_PERSONA, FOREMAN_STAGE_KEY, FOREMAN_TOOL_SPECS,
};
use agentpipeline_core::storage::foreman::NewForemanMessage;
use agentpipeline_core::storage::tasks::TaskFilter;
use agentpipeline_core::storage::Store;
use agentpipeline_core::types::{Node, PendingKind, PendingReason, Stage, TaskStatus};
use agentpipeline_core::Error;
use testkit::{FakeAgent, ManualClock, Script, TestHome};

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

    /// 新开一个班次（会话）。
    ///
    /// 本文件里凡是要「往库里塞几句话」的用例都先开一个——会话是这些行的**必填**
    /// 归属参数，不是可选项：靠缺省值猜「哪句属于哪一班」正是这次要修掉的东西。
    async fn session(&self) -> String {
        self.store.create_foreman_session("").await.unwrap().id
    }

    fn runner(&self, agent: FakeAgent) -> ForemanRunner {
        ForemanRunner::new(
            self.store.clone(),
            Settings::default(),
            self._home.home().clone(),
            Arc::new(agent) as Arc<dyn LlmClient>,
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
    // 反向断言（本票最该保留的一条）：**任何**越权工具名都不得进广告集。
    // 逐个名字列出来而不是只断言数量：数量对得上、名字换了一个的情况，只断言数量看不出来。
    for forbidden in [
        "read_file",
        "list_dir",
        "run_command",
        "write_file",
        "edit_file",
        "delete_file",
        "spawn_sub_agent",
        "submit_metadata",
        "Skill",
    ] {
        assert!(
            !names.contains(&forbidden),
            "值班长的工具集不得含 {forbidden}：{names:?}"
        );
    }
    assert!(
        names.len() >= 8,
        "A 层只读工具（票 01）应当已全部在清单里：{names:?}"
    );
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
    );
    let sid = h.session().await;
    let err = runner.say(Some(&sid), "喂").await.unwrap_err();
    assert!(matches!(err, Error::Llm(_)));

    let messages = h.store.list_foreman_messages(&sid, 100).await.unwrap();
    assert_eq!(messages.len(), 1, "只有用户那一行");
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[0].content, "喂");
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
    // 直接发一个 run_command。票 02 的硬约束，与只读子代理同一处检查。
    let h = Harness::seeded().await;
    let mut script = Script::new();
    script
        .for_foreman()
        .tool("run_command", serde_json::json!({"command": "echo pwned"}));
    script.for_foreman().text("我读不到那个。");
    let runner = h.runner(FakeAgent::new(script));

    let turn = runner.say(None, "帮我跑个命令").await.unwrap();
    assert_eq!(turn.traces.len(), 1);
    assert_eq!(turn.traces[0].tool, "run_command");
    // 工具被拒 → 痕迹记 ok = false；且命令**没有真的被执行**。
    assert!(!turn.traces[0].ok, "越权工具必须在执行点被拒");

    let commands = h.store.list_commands("t1", None, None).await.unwrap();
    assert!(commands.is_empty(), "越权命令不得真的跑起来");
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
        ]
    );
    assert!(
        foreman_tool_names(ForemanToolLayer::Write).is_empty(),
        "本轮不引入任何写工具（写工具由票 04 / 05 / 06 加）"
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

/// 越权工具名不在清单里 → 仍在执行点被拒（白名单是执行点的闸，不是广告集的筛选）。
#[tokio::test]
async fn tools_outside_the_manifest_are_still_rejected_at_the_execution_point() {
    let h = Harness::seeded().await;
    for name in ["read_file", "write_file", "run_command"] {
        let mut script = Script::new();
        script
            .for_foreman()
            .tool(name, serde_json::json!({"path": "src/lib.rs"}));
        script.for_foreman().text("我读不到那个。");
        let runner = h.runner(FakeAgent::new(script));
        let turn = runner.say(None, "帮我看看").await.unwrap();
        assert_eq!(turn.traces.len(), 1);
        assert!(!turn.traces[0].ok, "{name} 必须在执行点被拒");
    }
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
