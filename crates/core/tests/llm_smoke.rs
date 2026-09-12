//! 真 LLM 冒烟测试（testing.md §3.2，`#[ignore]`，不进任何自动门）。
//!
//! 手动运行（需要真实 key）：
//!
//! ```text
//! AGENTPIPELINE_SMOKE_VENDOR=openai \
//! AGENTPIPELINE_SMOKE_MODEL=gpt-4o-mini \
//! AGENTPIPELINE_SMOKE_API_KEY=sk-... \
//! AGENTPIPELINE_SMOKE_BASE_URL=https://api.openai.com/v1 \  # 可省略，按 vendor 取官方默认
//! cargo test -p agentpipeline-core --test llm_smoke -- --ignored --nocapture
//! ```
//!
//! 验收点（票 13）：适配器真调用 + 流式增量 + token 计量 + submit_metadata
//! 结构化输出可解析（决策 38 的 schema 链路）。

use std::sync::Arc;

use agentpipeline_core::agent::client::{submit_metadata_tool, LlmClient, LlmRequest, RunContext};
use agentpipeline_core::agent::providers::ProductionLlm;
use agentpipeline_core::sse::SseEventType;
use agentpipeline_core::types::{ArchitectExecuteMetadata, Node, Provider, Stage};
use agentpipeline_core::Error;
use testkit::{SseRecorder, TestHome};

fn smoke_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

#[tokio::test]
#[ignore = "需要真实 LLM key；用法见文件头注释"]
async fn real_llm_completes_architect_execute_with_structured_metadata() {
    let Some(api_key) = smoke_env("AGENTPIPELINE_SMOKE_API_KEY") else {
        eprintln!("未设置 AGENTPIPELINE_SMOKE_API_KEY，跳过真 LLM 冒烟");
        return;
    };
    let vendor = smoke_env("AGENTPIPELINE_SMOKE_VENDOR").unwrap_or_else(|| "openai".into());
    let model = smoke_env("AGENTPIPELINE_SMOKE_MODEL").unwrap_or_else(|| match vendor.as_str() {
        "anthropic" => "claude-sonnet-4-5".into(),
        "deepseek" => "deepseek-chat".into(),
        _ => "gpt-4o-mini".into(),
    });

    let home = TestHome::new().unwrap();
    let (store, _clock) = home.setup().await.unwrap();
    store
        .upsert_provider(&Provider {
            id: "smoke".into(),
            vendor: vendor.clone(),
            model: model.clone(),
            context_window: 128_000,
            base_url: smoke_env("AGENTPIPELINE_SMOKE_BASE_URL"),
            api_key: Some(api_key),
            enabled: true,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        })
        .await
        .unwrap();

    let sse = SseRecorder::new();
    let client = ProductionLlm::new(store.clone(), Arc::new(sse.clone()));

    // architect-design.execute 的一次最小真调用：要求提交结构化元数据
    let request = LlmRequest {
        stage: Stage::ArchitectDesign,
        node: Node::Execute,
        attempt: 1,
        system_prompt: "你是架构师。只提交元数据，不要写文件。".into(),
        user_prompt: "为「任务目录切换为按创建时间排序」提交架构元数据（readiness=true）。".into(),
        messages: vec![],
        tools: vec![submit_metadata_tool::<ArchitectExecuteMetadata>(
            "提交架构设计元数据",
        )],
        temperature: None,
        max_tokens: None,
        provider_id: None,
        run: Some(RunContext {
            task_id: "smoke".into(),
            branch: "main".into(),
            run_id: 1,
            agent_type: "main".into(),
        }),
    };
    let response = client.complete(request).await.unwrap();

    // 计量与流式
    println!(
        "vendor={vendor} model={model} prompt={} completion={}",
        response.prompt_tokens, response.completion_tokens
    );
    assert!(response.prompt_tokens > 0, "prompt tokens 应有计量");
    assert!(
        sse.count_of(SseEventType::ConversationDelta) > 0,
        "流式增量应有事件"
    );

    // 结构化输出：submit_metadata 参数可解析为阶段结构体（决策 38 链路）
    let call = response
        .tool_calls
        .iter()
        .find(|c| c.name == "submit_metadata")
        .ok_or_else(|| {
            Error::Validation(format!(
                "未提交 submit_metadata：tool_calls={:?} content={:?}",
                response.tool_calls, response.content
            ))
        })
        .unwrap();
    let metadata: ArchitectExecuteMetadata = serde_json::from_str(&call.arguments).unwrap();
    println!("metadata readiness={}", metadata.readiness);
}
