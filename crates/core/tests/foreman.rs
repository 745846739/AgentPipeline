//! L2 集成：值班长（决策 182，票 01 / 02 / 05）。
//!
//! 测的是**外部可观测的行为**：端点无关的那一层——快照装了什么、历史怎么裁、
//! 工具真的执行了没有、越权工具是不是在执行点被拒、对话动没动全局指标、保留期到点
//! 行有没有被清。**不测 prompt 的效果**（答得好不好、判断准不准）：那是外部不确定性，
//! 本仓的口径是 prompt 效果回归属 v2 离线 eval（docs/testing.md §1）。
//!
//! FakeAgent 的替换边界照旧（决策 148）：**只替换 LLM 响应流，工具与存储全部真跑**。
//! 所以「read_task 真读到了台账」这件事由真 SQL 保证，不是脚本演出来的。

use std::sync::Arc;

use agentpipeline_core::agent::client::{LlmClient, LlmRequest};
use agentpipeline_core::clock::Clock;
use agentpipeline_core::config::Settings;
use agentpipeline_core::metrics;
use agentpipeline_core::pipeline::foreman::FOREMAN_TOOLS;
use agentpipeline_core::pipeline::foreman::{
    build_briefing, trim_history, ForemanRunner, FOREMAN_AGENT_TYPE, FOREMAN_PERSONA,
    FOREMAN_STAGE_KEY,
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
    // 三条各 100 字的消息；预算只够容纳最新的两条。
    let body = "字".repeat(100);
    for role in ["user", "assistant", "user"] {
        h.store
            .append_foreman_message(NewForemanMessage {
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
    let all = h.store.list_foreman_messages(100).await.unwrap();
    assert_eq!(all.len(), 3);

    let kept = trim_history(&all, 250);
    assert_eq!(kept.len(), 2, "预算 250 只装得下两条 100 字的");
    assert_eq!(kept[0].id, all[1].id, "保留的应是最新的两条");
    assert_eq!(kept[1].id, all[2].id);

    // 被裁掉的历史**仍在库里**：裁剪只影响这一轮注入了什么。
    let after = h.store.list_foreman_messages(100).await.unwrap();
    assert_eq!(after.len(), 3);
}

#[tokio::test]
async fn history_trimming_always_keeps_the_newest_message_even_over_budget() {
    let h = Harness::empty().await;
    h.store
        .append_foreman_message(NewForemanMessage::user("短"))
        .await
        .unwrap();
    h.store
        .append_foreman_message(NewForemanMessage::user("长".repeat(500)))
        .await
        .unwrap();
    let all = h.store.list_foreman_messages(100).await.unwrap();

    // 预算小到连最新一条都装不下——仍必须保留它，否则值班长会答非所问。
    let kept = trim_history(&all, 10);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].id, all[1].id);
}

#[tokio::test]
async fn session_listing_returns_the_newest_tail_in_chronological_order() {
    let h = Harness::empty().await;
    for i in 0..5 {
        h.store
            .append_foreman_message(NewForemanMessage::user(format!("第{i}句")))
            .await
            .unwrap();
    }
    let tail = h.store.list_foreman_messages(2).await.unwrap();
    // LIMIT 必须作用在最新那一端，再按时间升序交出去。
    assert_eq!(tail.len(), 2);
    assert_eq!(tail[0].content, "第3句");
    assert_eq!(tail[1].content, "第4句");
    assert!(h.store.list_foreman_messages(0).await.unwrap().is_empty());
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

    let turn = runner.say("现在能做什么？").await.unwrap();
    assert!(turn.reply.contains("先建一个项目"));
    assert!(turn.prompt_tokens > 0);
    assert_eq!(turn.traces.len(), 0, "这一轮没有调工具");

    // 两句都落库，且顺序是「人先说、值班长后答」。
    let messages = h.store.list_foreman_messages(100).await.unwrap();
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
    runner2.say("再问一次").await.unwrap();
    let after = h.store.list_foreman_messages(100).await.unwrap();
    assert_eq!(after.len(), 4);
}

#[tokio::test]
async fn the_llm_request_carries_the_foreman_identity_and_a_placeholder_stage() {
    let h = Harness::empty().await;
    let mut script = Script::new();
    script.for_foreman().text("收到。");
    let agent = FakeAgent::new(script);
    let runner = h.runner(agent.clone());
    runner.say("在吗").await.unwrap();

    let requests = agent.request_log();
    assert_eq!(requests.len(), 1);
    let req = &requests[0];
    let run = req.run.as_ref().expect("必须有 run 上下文（流式要用）");
    assert_eq!(run.agent_type, FOREMAN_AGENT_TYPE);
    assert_eq!(run.task_id, "", "空 task id 是既有任务级 SSE 零干扰的前提");
    // 阶段枚举**没有**新增变体（决策 182①）：占位阶段是既有的 Init。
    assert_eq!(req.stage, Stage::Init);
    // 工具集只有两个只读台账工具——没有走 effective_tools（那条路会并入 run_command）。
    //
    // 断言打的是**白名单常量**而不是字面量：`FOREMAN_TOOLS` 是安全边界，而 `tool_defs()`
    // 是「广告给模型的那一份」，两者各写一次名字就会漂移——漂移的表现是模型看得见一个
    // 调用就被拒的工具（或反过来，一个能调但没人告诉它的工具），两种都很难从现象定位。
    let names: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        FOREMAN_TOOLS.to_vec(),
        "广告的工具集必须与白名单逐字一致"
    );
    for forbidden in [
        "read_file",
        "list_dir",
        "run_command",
        "write_file",
        "submit_metadata",
    ] {
        assert!(
            !names.contains(&forbidden),
            "值班长的工具集不得含 {forbidden}：{names:?}"
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
    );
    let err = runner.say("喂").await.unwrap_err();
    assert!(matches!(err, Error::Llm(_)));

    let messages = h.store.list_foreman_messages(100).await.unwrap();
    assert_eq!(messages.len(), 1, "只有用户那一行");
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[0].content, "喂");
}

#[tokio::test]
async fn empty_message_is_rejected_and_not_persisted() {
    let h = Harness::empty().await;
    let runner = h.runner(FakeAgent::new(Script::new()));
    assert!(runner.say("   ").await.is_err());
    assert!(h.store.list_foreman_messages(10).await.unwrap().is_empty());
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

    let turn = runner.say("t1 怎么了？").await.unwrap();
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
    let messages = h.store.list_foreman_messages(100).await.unwrap();
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

    let turn = runner.say("它上次干了啥？").await.unwrap();
    assert_eq!(turn.traces.len(), 1);
    assert!(turn.traces[0].ok);
    assert!(turn.reply.contains("src/auth.rs:42"));

    let messages = h.store.list_foreman_messages(100).await.unwrap();
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

    let turn = runner.say("帮我跑个命令").await.unwrap();
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

    let turn = runner.say("t-nonexistent 呢？").await.unwrap();
    assert_eq!(turn.traces.len(), 1);
    assert!(turn.traces[0].ok, "「查无此任务」是正常回答，不是工具故障");
    assert!(turn.reply.contains("没有这个号"));
}

#[tokio::test]
async fn the_foreman_tool_set_matches_the_frozen_contract() {
    assert_eq!(FOREMAN_TOOLS, ["read_task", "read_conversation"]);
    assert_eq!(FOREMAN_STAGE_KEY, "foreman");
    assert_eq!(FOREMAN_AGENT_TYPE, "foreman");
}

// ─────────────────────────── 口径与保留期（票 05）───────────────────────────

#[tokio::test]
async fn conversation_tokens_do_not_move_the_global_metrics() {
    let h = Harness::seeded().await;
    let mut script = Script::new();
    script.for_foreman().text("说完了。");
    let runner = h.runner(FakeAgent::new(script));
    runner.say("随便聊聊").await.unwrap();

    // 对话产生了 token（自报表里看得到）……
    let (tokens, calls) = h.store.foreman_session_totals().await.unwrap();
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
    h.store
        .append_foreman_message(NewForemanMessage::user("一"))
        .await
        .unwrap();
    h.store
        .append_foreman_message(NewForemanMessage {
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
            role: "assistant".into(),
            content: "三".into(),
            prompt_tokens: 50,
            completion_tokens: 5,
            briefing_json: None,
            traces_json: None,
        })
        .await
        .unwrap();

    let (tokens, calls) = h.store.foreman_session_totals().await.unwrap();
    // 求和而不是另存计数器：计数器会与台账漂移，求和永远等于真实存在的东西。
    assert_eq!(tokens, 175);
    // calls 是**回话次数**（assistant 行），不是工具往返次数。
    assert_eq!(calls, 2);
}

#[tokio::test]
async fn maintenance_purges_foreman_messages_past_the_retention_window() {
    let h = Harness::seeded().await;
    h.store
        .append_foreman_message(NewForemanMessage::user("很久以前说的"))
        .await
        .unwrap();

    // 未到期：一条不少。
    let purged = h
        .store
        .purge_foreman_messages(h.clock.now() - chrono::Duration::days(30))
        .await
        .unwrap();
    assert_eq!(purged, 0);
    assert_eq!(h.store.list_foreman_messages(10).await.unwrap().len(), 1);

    // 假时钟推进 31 天（等不了真实 30 天）；清理按创建时间判年龄，与任务终态无关
    // ——值班长对话不挂任务，没有「任务还没结束所以先留着」这一说。
    h.clock.advance_secs(31 * 24 * 3600);
    let purged = h
        .store
        .purge_foreman_messages(h.clock.now() - chrono::Duration::days(30))
        .await
        .unwrap();
    assert_eq!(purged, 1);
    assert!(h.store.list_foreman_messages(10).await.unwrap().is_empty());
}

#[tokio::test]
async fn scheduler_maintenance_reports_foreman_purges_separately() {
    let h = Harness::seeded().await;
    h.store
        .append_foreman_message(NewForemanMessage::user("昨晚说的"))
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
    h.runner(FakeAgent::new(script)).say("在吗").await.unwrap();
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
