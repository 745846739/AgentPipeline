//! 票 13 生产 LLM 适配器集成测试：testkit mock server 驱动 OpenAI 兼容 / Anthropic
//! 两族的真实 HTTP 流式调用，覆盖计量、缓存 token、conversation_delta、心跳与错误路径。

use std::sync::Arc;

use agentpipeline_core::agent::client::{LlmClient, LlmRequest, Message, RunContext, ToolCall};
use agentpipeline_core::agent::providers::ProductionLlm;
use agentpipeline_core::sse::SseEventType;
use agentpipeline_core::storage::observability::NewRun;
use agentpipeline_core::types::{Node, Provider, Stage};
use testkit::{seed_project, seed_task, MockLlm, MockRoute, SseRecorder, TestHome};

// ─────────────────────────── 脚手架 ───────────────────────────

fn provider(vendor: &str, model: &str, base_url: &str, id: &str) -> Provider {
    Provider {
        id: id.into(),
        vendor: vendor.into(),
        model: model.into(),
        context_window: 128_000,
        base_url: Some(base_url.into()),
        api_key: Some("sk-mock-key".into()),
        enabled: true,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}

/// 播种任务 → main 游标 → 一行 run（心跳断言的落点）。
async fn seed_run(store: &agentpipeline_core::storage::Store, task_id: &str) -> i64 {
    seed_project(
        store,
        "p1",
        "项目",
        std::path::Path::new("/tmp/llm-proj"),
        "main",
    )
    .await
    .unwrap();
    seed_task(store, task_id, "p1").await.unwrap();
    let cursor = testkit::live_cursor_for_branch(store, task_id, "main")
        .await
        .unwrap()
        .expect("main 游标");
    store
        .insert_run(&NewRun {
            task_id: task_id.into(),
            cursor_id: cursor.cursor_id,
            stage: Stage::Develop,
            node: Node::Execute,
            attempt: 1,
            agent_type: "main".into(),
            parent_run_id: None,
            prompt_template_hash: None,
            process_group_id: None,
        })
        .await
        .unwrap()
}

fn request(run_id: i64, messages: Vec<Message>) -> LlmRequest {
    LlmRequest {
        stage: Stage::Develop,
        node: Node::Execute,
        attempt: 1,
        system_prompt: "系统提示".into(),
        user_prompt: "用户提示".into(),
        messages,
        tools: vec![],
        temperature: None,
        max_tokens: None,
        provider_id: None,
        run: Some(RunContext {
            task_id: "t1".into(),
            branch: "main".into(),
            run_id,
            agent_type: "main".into(),
            session_id: String::new(),
        }),
        idle_timeout_sec: None,
    }
}

async fn run_row_last_activity(
    store: &agentpipeline_core::storage::Store,
    task_id: &str,
) -> Option<chrono::DateTime<chrono::Utc>> {
    store
        .list_runs(task_id)
        .await
        .unwrap()
        .first()
        .and_then(|r| r.last_activity_at)
}

// ─────────────────────────── OpenAI 兼容族 ───────────────────────────

const OPENAI_STREAM: &str = r##"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":"设计"},"finish_reason":null}]}

data: {"choices":[{"index":0,"delta":{"content":"开始"},"finish_reason":null}]}

data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"write_file","arguments":""}}]}}]}

data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\":\"design.md\",\"content\":\"# 设计\"}"}}]}}]}

data: {"usage":{"prompt_tokens":120,"completion_tokens":40,"prompt_tokens_details":{"cached_tokens":64}}}

data: [DONE]

"##;

#[tokio::test]
async fn openai_stream_aggregates_content_tools_usage_and_cache() {
    let home = TestHome::new().unwrap();
    let (store, clock) = home.setup().await.unwrap();
    let mock = MockLlm::start(vec![MockRoute::sse("/chat/completions", OPENAI_STREAM)]).await;
    store
        .upsert_provider(&provider("openai", "gpt-x", &mock.url, "p1"))
        .await
        .unwrap();
    let run_id = seed_run(&store, "t1").await;
    let before = run_row_last_activity(&store, "t1").await;
    clock.advance_secs(120); // 心跳写入值必须晚于 run 创建时刻

    let sse = SseRecorder::new();
    let client = ProductionLlm::new(store.clone(), Arc::new(sse.clone()));
    let response = client.complete(request(run_id, vec![])).await.unwrap();

    // 聚合：文本、工具调用（分片 arguments 拼接）、计量与缓存
    assert_eq!(response.content.as_deref(), Some("设计开始"));
    assert_eq!(response.tool_calls.len(), 1);
    let call: serde_json::Value = serde_json::from_str(&response.tool_calls[0].arguments).unwrap();
    assert_eq!(call["path"], "design.md");
    assert_eq!(response.prompt_tokens, 120);
    assert_eq!(response.completion_tokens, 40);
    assert_eq!(response.cache_read_tokens, 64);
    assert_eq!(response.cache_write_tokens, 0);

    // 决策 123：文本增量逐段发，usage 收尾一条增量事件
    let deltas: Vec<_> = sse
        .events()
        .into_iter()
        .filter(|e| e.event_type() == SseEventType::ConversationDelta)
        .collect();
    assert_eq!(deltas.len(), 3, "{deltas:?}");
    let text = deltas[..2]
        .iter()
        .map(|e| match e {
            agentpipeline_core::sse::SseEvent::ConversationDelta { text, .. } => text.clone(),
            _ => String::new(),
        })
        .collect::<String>();
    assert_eq!(text, "设计开始");
    let agentpipeline_core::sse::SseEvent::ConversationDelta {
        prompt_tokens,
        completion_tokens,
        branch,
        agent_type,
        ..
    } = &deltas[2]
    else {
        panic!("最后一条应是 usage 增量");
    };
    assert_eq!(*prompt_tokens, 120);
    assert_eq!(*completion_tokens, 40);
    assert_eq!(branch, "main");
    assert_eq!(agent_type, "main");

    // 决策 64：流式活动刷新 last_activity_at（流短于节流间隔 → 收尾心跳兜底）
    let after = run_row_last_activity(&store, "t1").await.unwrap();
    assert!(
        after > before.unwrap(),
        "心跳未刷新：{before:?} → {after:?}"
    );

    // 请求形态：鉴权头 / model / 流式开关
    let requests = mock.requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].header("authorization"),
        Some("Bearer sk-mock-key")
    );
    let body: serde_json::Value = serde_json::from_str(&requests[0].body).unwrap();
    assert_eq!(body["model"], "gpt-x");
    assert_eq!(body["stream"], true);
    assert_eq!(body["stream_options"]["include_usage"], true);
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][1]["role"], "user");

    mock.shutdown().await;
}

#[tokio::test]
async fn deepseek_dispatches_to_openai_compatible_path() {
    let home = TestHome::new().unwrap();
    let (store, _clock) = home.setup().await.unwrap();
    let mock = MockLlm::start(vec![MockRoute::sse(
        "/chat/completions",
        "data: {\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2}}\n\ndata: [DONE]\n\n",
    )])
    .await;
    store
        .upsert_provider(&provider("deepseek", "deepseek-x", &mock.url, "p1"))
        .await
        .unwrap();
    let run_id = seed_run(&store, "t1").await;

    let client = ProductionLlm::new(store.clone(), Arc::new(SseRecorder::new()));
    let response = client.complete(request(run_id, vec![])).await.unwrap();
    assert_eq!(response.prompt_tokens, 3);
    assert!(mock.requests().await[0]
        .path
        .starts_with("/chat/completions"));
    mock.shutdown().await;
}

// ─────────────────────────── Anthropic 族 ───────────────────────────

const ANTHROPIC_STREAM: &str = r#"event: message_start
data: {"type":"message_start","message":{"usage":{"input_tokens":10,"cache_creation_input_tokens":4,"cache_read_input_tokens":6,"output_tokens":1}}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"分析"}}

event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"read_file","input":{}}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"a.md\"}"}}

event: content_block_stop
data: {"type":"content_block_stop","index":1}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":42}}

event: message_stop
data: {"type":"message_stop"}

"#;

#[tokio::test]
async fn anthropic_stream_maps_usage_tool_blocks_and_request_shape() {
    let home = TestHome::new().unwrap();
    let (store, _clock) = home.setup().await.unwrap();
    let mock = MockLlm::start(vec![MockRoute::sse("/v1/messages", ANTHROPIC_STREAM)]).await;
    store
        .upsert_provider(&provider("anthropic", "claude-x", &mock.url, "p1"))
        .await
        .unwrap();
    let run_id = seed_run(&store, "t1").await;

    // 带一轮工具往返，验证 tool_result 回传映射
    let call = ToolCall {
        id: "toolu_0".into(),
        name: "write_file".into(),
        arguments: r#"{"path":"b.md"}"#.into(),
    };
    let messages = vec![
        Message::assistant(None, vec![call.clone()]),
        Message::tool_result(&call, "{\"success\":true}"),
    ];
    let mut req = request(run_id, messages);
    req.max_tokens = Some(2048);

    let sse = SseRecorder::new();
    let client = ProductionLlm::new(store.clone(), Arc::new(sse.clone()));
    let response = client.complete(req).await.unwrap();

    // prompt 计量归一：input + cache_read + cache_creation
    assert_eq!(response.prompt_tokens, 20);
    assert_eq!(response.cache_read_tokens, 6);
    assert_eq!(response.cache_write_tokens, 4);
    assert_eq!(response.completion_tokens, 42);
    assert_eq!(response.content.as_deref(), Some("分析"));
    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_calls[0].id, "toolu_1");
    let args: serde_json::Value = serde_json::from_str(&response.tool_calls[0].arguments).unwrap();
    assert_eq!(args["path"], "a.md");

    // 请求形态：双头鉴权 / system 顶层 / max_tokens / tool_result 合并
    let requests = mock.requests().await;
    assert_eq!(requests[0].header("x-api-key"), Some("sk-mock-key"));
    assert_eq!(
        requests[0].header("anthropic-version"),
        Some(agentpipeline_core::agent::providers::anthropic::ANTHROPIC_VERSION)
    );
    let body: serde_json::Value = serde_json::from_str(&requests[0].body).unwrap();
    assert_eq!(body["model"], "claude-x");
    assert_eq!(body["max_tokens"], 2048);
    // system 恒为 block 形式，末块带 prompt-cache 断点（决策 299）
    assert_eq!(body["system"][0]["text"], "系统提示");
    assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
    let msgs = body["messages"].as_array().unwrap();
    // [user_prompt, assistant(tool_use), user(tool_result)]
    assert_eq!(msgs.len(), 3, "{msgs:?}");
    assert_eq!(msgs[1]["content"][0]["type"], "tool_use");
    assert_eq!(msgs[1]["content"][0]["input"]["path"], "b.md");
    assert_eq!(msgs[2]["content"][0]["type"], "tool_result");
    assert_eq!(msgs[2]["content"][0]["tool_use_id"], "toolu_0");

    // 流式增量也照常发射
    assert!(sse.count_of(SseEventType::ConversationDelta) >= 2);
    mock.shutdown().await;
}

// ─────────────────────────── provider 解析（决策 105 / 111）───────────────────────────

#[tokio::test]
async fn provider_resolution_follows_decision_129_four_tiers() {
    let home = TestHome::new().unwrap();
    let (store, clock) = home.setup().await.unwrap();
    let mock = MockLlm::start(vec![MockRoute::sse(
        "/chat/completions",
        "data: {\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\n\ndata: [DONE]\n\n",
    )])
    .await;
    // 三个 provider：系统默认（首个 enabled，按 created_at 序）/ 阶段配置 / 任务覆盖
    for (id, vendor, model) in [
        ("p-default", "openai", "model-default"),
        ("p-stage", "openai", "model-stage"),
        ("p-task", "openai", "model-task"),
    ] {
        store
            .upsert_provider(&provider(vendor, model, &mock.url, id))
            .await
            .unwrap();
        clock.advance_secs(1); // created_at 递增，"首个" 才是确定的
    }
    store
        .upsert_stage_config(&agentpipeline_core::types::StageConfig {
            stage: "develop".into(),
            provider_id: Some("p-stage".into()),
            node_overrides_json: Some(serde_json::json!({
                "execute": {"provider_id": "p-node"}
            })),
            ..Default::default()
        })
        .await
        .unwrap();
    // 决策 129 第一级的载体 provider
    store
        .upsert_provider(&provider("openai", "model-node", &mock.url, "p-node"))
        .await
        .unwrap();
    let run_id = seed_run(&store, "t1").await;
    let client = ProductionLlm::new(store.clone(), Arc::new(SseRecorder::new()));
    let model_used = || async {
        let requests = mock.requests().await;
        let body: serde_json::Value = serde_json::from_str(&requests.last().unwrap().body).unwrap();
        body["model"].as_str().unwrap().to_string()
    };

    // 决策 129：node_overrides 压过一切——develop.execute 命中节点级覆盖
    client.complete(request(run_id, vec![])).await.unwrap();
    assert_eq!(model_used().await, "model-node");

    // 任务覆盖（决策 105）压过阶段配置，但压不过 node_overrides
    let mut req = request(run_id, vec![]);
    req.provider_id = Some("p-task".into());
    client.complete(req).await.unwrap();
    assert_eq!(model_used().await, "model-node");

    // 非 execute 节点走阶段配置
    let mut req = request(run_id, vec![]);
    req.node = Node::ValidateInput;
    client.complete(req).await.unwrap();
    assert_eq!(model_used().await, "model-stage");

    // 无阶段配置的阶段落到系统默认（首个 enabled）
    let mut req = request(run_id, vec![]);
    req.stage = Stage::Test;
    client.complete(req).await.unwrap();
    assert_eq!(model_used().await, "model-default");

    mock.shutdown().await;
}

// ─────────────────────────── 错误路径 ───────────────────────────

#[tokio::test]
async fn http_error_surfaces_status_and_body_preview() {
    let home = TestHome::new().unwrap();
    let (store, _clock) = home.setup().await.unwrap();
    let mock = MockLlm::start(vec![MockRoute::json(
        "/chat/completions",
        401,
        r#"{"error":"invalid api key"}"#,
    )])
    .await;
    store
        .upsert_provider(&provider("openai", "gpt-x", &mock.url, "p1"))
        .await
        .unwrap();
    let run_id = seed_run(&store, "t1").await;

    let client = ProductionLlm::new(store.clone(), Arc::new(SseRecorder::new()));
    let err = client.complete(request(run_id, vec![])).await.unwrap_err();
    // 主流程票 03：401 归因为鉴权失败。message 只留中文可操作提示；
    // 原始状态与返回体**保留**在 `llm_classified().raw`（经 executor 进
    // pending.context.diagnostic，前端 dossier 渲染）——可诊断性不倒退，只是换了位置。
    let (kind, raw) = err.llm_classified().expect("401 应可归因");
    assert_eq!(kind, "llm_auth");
    assert!(raw.contains("401"), "{raw}");
    assert!(raw.contains("invalid api key"), "{raw}");
    let msg = err.to_string();
    assert!(msg.contains("api_key"), "message 应含可操作提示：{msg}");
    mock.shutdown().await;
}

#[tokio::test]
async fn malformed_stream_is_a_clean_llm_error() {
    let home = TestHome::new().unwrap();
    let (store, _clock) = home.setup().await.unwrap();
    let mock = MockLlm::start(vec![MockRoute::sse(
        "/chat/completions",
        "data: {not-json\n\ndata: [DONE]\n\n",
    )])
    .await;
    store
        .upsert_provider(&provider("openai", "gpt-x", &mock.url, "p1"))
        .await
        .unwrap();
    let run_id = seed_run(&store, "t1").await;

    let client = ProductionLlm::new(store.clone(), Arc::new(SseRecorder::new()));
    let err = client.complete(request(run_id, vec![])).await.unwrap_err();
    assert!(err.to_string().contains("解析失败"), "{err}");
    mock.shutdown().await;
}

// ─────────────────── 决策 280：退化护栏（流式循环检测） ───────────────────

/// 拼一段「正常开头 + 单元复读 N 次」的 OpenAI 兼容 SSE 流。
fn degenerate_stream(unit: &str, times: usize) -> String {
    let mut s = String::from(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"前文分析。\"}}]}\n\n",
    );
    for _ in 0..times {
        let payload = serde_json::json!({"choices":[{"index":0,"delta":{"content": unit}}]});
        s.push_str(&format!("data: {payload}\n\n"));
    }
    s.push_str("data: [DONE]\n\n");
    s
}

#[tokio::test]
async fn a_repetitive_stream_is_voided_as_degraded_mid_flight() {
    // 决策 280：run41 的形态（「Playwright 或」×N）在流中途即被判废——
    // 不等 [DONE]、不读完垃圾，complete 返回 Degenerated 错误。
    let home = TestHome::new().unwrap();
    let (store, _clock) = home.setup().await.unwrap();
    let mock = MockLlm::start(vec![MockRoute::sse(
        "/chat/completions",
        degenerate_stream("Playwright 或", 40),
    )])
    .await;
    store
        .upsert_provider(&provider("openai", "gpt-x", &mock.url, "p1"))
        .await
        .unwrap();
    let run_id = seed_run(&store, "t1").await;

    let client = ProductionLlm::new(store.clone(), Arc::new(SseRecorder::new()));
    let err = client.complete(request(run_id, vec![])).await.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("输出退化（degraded）"), "{msg}");
    assert!(msg.contains("Playwright"), "报文引用重复片段：{msg}");
    assert!(!msg.contains("解析失败"), "不是流解析错误：{msg}");
    mock.shutdown().await;
}

#[tokio::test]
async fn unknown_vendor_and_missing_provider_fail_cleanly() {
    let home = TestHome::new().unwrap();
    let (store, _clock) = home.setup().await.unwrap();
    let mock = MockLlm::start(vec![MockRoute::sse(
        "/chat/completions",
        "data: [DONE]\n\n",
    )])
    .await;
    let run_id = seed_run(&store, "t1").await;
    let client = ProductionLlm::new(store.clone(), Arc::new(SseRecorder::new()));

    // 无任何 provider → 明确报错
    let err = client.complete(request(run_id, vec![])).await.unwrap_err();
    assert!(err.to_string().contains("未配置 provider"), "{err}");

    // vendor 不在适配器集合 → 拒绝调用
    store
        .upsert_provider(&provider("mystery-llm", "m1", &mock.url, "p-x"))
        .await
        .unwrap();
    let err = client.complete(request(run_id, vec![])).await.unwrap_err();
    assert!(err.to_string().contains("不支持的 vendor"), "{err}");

    // 禁用的 provider 不可用（走系统默认时被跳过，显式引用时报错）
    store
        .upsert_provider(&provider("openai", "m2", &mock.url, "p-off"))
        .await
        .unwrap();
    store.set_provider_enabled("p-off", false).await.unwrap();
    let mut req = request(run_id, vec![]);
    req.provider_id = Some("p-off".into());
    let err = client.complete(req).await.unwrap_err();
    assert!(err.to_string().contains("已被禁用"), "{err}");
    mock.shutdown().await;
}

// ─────────────────── 逐调用空闲判死（决策 288 / 票 foreman-unbounded 05）───────────────────

/// 一台「先吐几个字节、然后一声不吭挂住」的 raw server：MockLlm 写完就关连接，
/// 造不出「流停了但连接还在」的真挂——空闲判死要杀的正是这个形状。
async fn spawn_dribble_then_hold() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        // 读掉请求头（读到空行为止）。
        let mut buf = vec![0u8; 8192];
        let mut read_total = 0usize;
        loop {
            let n = sock.read(&mut buf).await.unwrap();
            read_total += n;
            if n == 0 || buf[..read_total].windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        // 回一个 200 的 SSE 头 + 一段真增量，然后**不关流**：对面等 [DONE] 等到地老天荒。
        sock.write_all(
            b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n\
              data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello\"}}]}\n\n",
        )
        .await
        .unwrap();
        sock.flush().await.unwrap();
        // 挂住足够久，让 1 秒的空闲界一定先到。
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
    });
    (format!("http://127.0.0.1:{port}"), handle)
}

use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// 流上超过空闲界没有新字节 → 中止这一次调用，类别是 `llm_idle_timeout`，
/// 报文里带「多久没有字节」与「已收多少字节」——排障要的两样都有。
#[tokio::test]
async fn idle_stream_is_aborted_with_a_classified_error() {
    let home = TestHome::new().unwrap();
    let (store, _clock) = home.setup().await.unwrap();
    let (url, server) = spawn_dribble_then_hold().await;
    store
        .upsert_provider(&provider("openai", "gpt-x", &url, "p-idle"))
        .await
        .unwrap();

    let sse = SseRecorder::new();
    let client = ProductionLlm::new(store.clone(), Arc::new(sse.clone()));
    let mut req = request(1, vec![]);
    req.idle_timeout_sec = Some(1);
    let started = std::time::Instant::now();
    let err = client.complete(req).await.unwrap_err();
    let elapsed = started.elapsed();

    match &err {
        agentpipeline_core::Error::LlmClassified { kind, raw, .. } => {
            assert_eq!(kind, "llm_idle_timeout");
            assert!(raw.contains("没有任何新字节"), "{raw}");
            assert!(raw.contains("1 秒"), "要带出实际的空闲界：{raw}");
            assert!(raw.contains("已收"), "要带出已收字节数：{raw}");
        }
        other => panic!("应当是空闲判死这一类，实际：{other:?}"),
    }
    // 判死在空闲界附近发生，而不是等满挂住时长（30s）——这是「逐调用」的全部意义。
    assert!(elapsed < std::time::Duration::from_secs(10), "{elapsed:?}");
    server.abort();
}

/// `idle_timeout_sec: None` 的请求**不走** watchdog：挂住的流保持现状
/// （节点路径由调度器的心跳判定收口，决策 64/66/88——两把尺互不越界）。
#[tokio::test]
async fn requests_without_an_idle_bound_are_not_watchdogged() {
    let home = TestHome::new().unwrap();
    let (store, _clock) = home.setup().await.unwrap();
    let (url, server) = spawn_dribble_then_hold().await;
    store
        .upsert_provider(&provider("openai", "gpt-x", &url, "p-none"))
        .await
        .unwrap();

    let sse = SseRecorder::new();
    let client = ProductionLlm::new(store.clone(), Arc::new(sse.clone()));
    let req = request(1, vec![]);
    let result =
        tokio::time::timeout(std::time::Duration::from_millis(1500), client.complete(req)).await;
    // 没有 watchdog：1.5s 时它还挂在流上（不是「1 秒就被判死」）。
    assert!(result.is_err(), "不该被判死：{result:?}");
    server.abort();
}
