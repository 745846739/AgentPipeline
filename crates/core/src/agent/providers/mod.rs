//! 生产 LLM 适配器（票 13）。
//!
//! 两族协议（决策 103：`SUPPORTED_ADAPTERS` 硬编码，改它要发版）：
//! - **OpenAI 兼容**（`openai` / `deepseek`）：`POST {base_url}/chat/completions`；
//! - **Anthropic**：`POST {base_url}/v1/messages`。
//!
//! 行为约定：
//! - 配置沿用 `providers` 表（决策 111）：解析顺序 = 任务覆盖（决策 105）>
//!   阶段配置 `provider_id` > 首个 enabled provider（系统默认）；
//! - 全部走流式（决策 123）：文本增量逐段发 `conversation_delta`，
//!   usage 汇总在流末以一条增量事件发出（`prompt_tokens` / `completion_tokens` 是增量）；
//! - 心跳（决策 64 / 100）：流式 token 活动节流刷新 run 的 `last_activity_at`；
//! - token 计量（决策 46）：`cache_read` / `cache_write` 落 `AgentResponse`，
//!   Anthropic 的 prompt 计量归一为 input + cache_read + cache_creation。

pub mod anthropic;
pub mod openai;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use futures::StreamExt;
use reqwest::Client;

use crate::agent::client::{AgentResponse, LlmClient, LlmRequest, RunContext, ToolCall};
use crate::sse::{SseEvent, SseSink};
use crate::storage::Store;
use crate::types::Provider;
use crate::{Error, Result};

/// Anthropic 必填 max_tokens 的兜底值（阶段配置未给时，§10.6.3）。
pub const DEFAULT_MAX_TOKENS: u32 = 8192;
/// 流式心跳的最小间隔：token 再密也不超过每秒一次 DB 写（决策 64）。
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
/// 错误信息里携带的响应体上限。
const ERROR_BODY_PREVIEW: usize = 500;

/// vendor 是否走 OpenAI 兼容协议（deepseek 与 openai 同族）。
pub fn is_openai_compatible(vendor: &str) -> bool {
    matches!(vendor, "openai" | "deepseek")
}

/// 请求 URL 基座：显式 base_url 优先，否则按 vendor 取官方默认（票 13：base_url 可配）。
pub fn base_url(provider: &Provider) -> String {
    let raw = provider.base_url.clone().unwrap_or_else(|| {
        match provider.vendor.as_str() {
            "openai" => "https://api.openai.com/v1",
            "deepseek" => "https://api.deepseek.com",
            "anthropic" => "https://api.anthropic.com",
            // 调用方已在 vendor 分发处挡掉未知 vendor；这里只是兜底
            _ => "",
        }
        .to_string()
    });
    raw.trim_end_matches('/').to_string()
}

/// 适配器族把厂商 SSE 流翻译成统一的块序列（mod.rs 的流驱动只认这一层）。
#[derive(Debug)]
pub(crate) enum StreamChunk {
    /// 助手文本增量。
    Text(String),
    /// 工具调用增量：按 `index` 聚合 id / name / arguments 片段。
    ToolDelta {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments_delta: Option<String>,
    },
    /// usage 增量：字段只在该事件覆盖时更新（None = 保持不变）。
    Usage {
        prompt_tokens: Option<u32>,
        completion_tokens: Option<u32>,
        cache_read: Option<u32>,
        cache_write: Option<u32>,
    },
    /// 流终止标记（OpenAI `data: [DONE]`；Anthropic `message_stop`）。
    Done,
}

pub(crate) trait Adapter: Send + Sync {
    /// URL 路径（拼在 [`base_url`] 之后）。
    fn endpoint_path(&self) -> &'static str;
    /// 请求体（决策 38 之外的工具 schema 形态由各协议自定）。
    fn build_body(&self, provider: &Provider, request: &LlmRequest) -> Result<serde_json::Value>;
    /// 追加鉴权头（api_key 缺省时本机代理可匿名，不在此报错）。
    fn apply_auth(
        &self,
        builder: reqwest::RequestBuilder,
        provider: &Provider,
    ) -> reqwest::RequestBuilder;
    /// 解析一条 SSE `data:` 载荷；不认识的事件返回空串（保持前向兼容）。
    /// 一条载荷可产生多个块（如 OpenAI 一个 chunk 携带多个 tool_call 分片）。
    fn parse_chunk(&self, payload: &str) -> Result<Vec<StreamChunk>>;
}

/// 生产 LLM 客户端：按 provider 的 vendor 分发到具体适配器族。
#[derive(Clone)]
pub struct ProductionLlm {
    http: Client,
    store: Store,
    sse: Arc<dyn SseSink>,
    heartbeat_interval: Duration,
}

impl ProductionLlm {
    pub fn new(store: Store, sse: Arc<dyn SseSink>) -> Self {
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest 客户端构建失败");
        ProductionLlm {
            http,
            store,
            sse,
            heartbeat_interval: HEARTBEAT_INTERVAL,
        }
    }

    /// 测试用：调小心跳节流间隔，让单测能在毫秒级验证刷新行为。
    pub fn with_heartbeat_interval(mut self, interval: Duration) -> Self {
        self.heartbeat_interval = interval;
        self
    }

    /// provider 解析（决策 129 四级优先级，查找路径唯一）：`node_overrides >
    /// task.model_override（决策 105）> 阶段 provider > 首个 enabled（全局默认）`。
    async fn resolve_provider(&self, request: &LlmRequest) -> Result<Provider> {
        let stage_cfg = self.store.get_stage_config(request.stage.as_str()).await?;
        // 第一级：node_overrides[node].provider_id（§10.6.4 合并表）
        let node_override = stage_cfg
            .as_ref()
            .and_then(|c| c.node_overrides_json.as_ref())
            .and_then(|o| o.get(request.node.as_str()))
            .and_then(|n| n.get("provider_id"))
            .and_then(|v| v.as_str())
            .map(String::from);
        let provider_id = node_override
            // 第二级：任务级覆盖（决策 105）
            .or_else(|| request.provider_id.clone())
            // 第三级：阶段配置
            .or_else(|| stage_cfg.as_ref().and_then(|c| c.provider_id.clone()));
        let provider = match provider_id {
            Some(id) => self
                .store
                .get_provider(&id)
                .await?
                .ok_or_else(|| Error::Llm(format!("provider {id} 不存在")))?,
            None => self
                .store
                .load_providers()
                .await?
                .into_iter()
                .find(|p| p.enabled)
                .ok_or_else(|| {
                    Error::Llm(format!(
                        "阶段 {} 未配置 provider，且没有可用的 enabled provider",
                        request.stage.as_str()
                    ))
                })?,
        };
        if !provider.enabled {
            return Err(Error::Llm(format!("provider {} 已被禁用", provider.id)));
        }
        Ok(provider)
    }

    async fn run_stream(
        &self,
        adapter: &dyn Adapter,
        provider: &Provider,
        request: &LlmRequest,
        run: Option<&RunContext>,
    ) -> Result<AgentResponse> {
        let url = format!("{}{}", base_url(provider), adapter.endpoint_path());
        let body = adapter.build_body(provider, request)?;
        let builder = adapter.apply_auth(self.http.post(&url).json(&body), provider);
        let response = builder
            .send()
            .await
            .map_err(|e| Error::Llm(format!("HTTP 请求失败：{e}")))?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(Error::Llm(format!(
                "HTTP {status}：{}",
                preview(&text, ERROR_BODY_PREVIEW)
            )));
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut content = String::new();
        let mut tools: BTreeMap<usize, ToolAccum> = BTreeMap::new();
        let mut usage = UsageAccum::default();
        let mut done = false;
        let mut last_heartbeat = Instant::now();

        while !done {
            let Some(chunk) = stream.next().await else {
                break;
            };
            let bytes = chunk.map_err(|e| Error::Llm(format!("读取流失败：{e}")))?;
            buffer.push_str(&String::from_utf8_lossy(&bytes));
            while let Some(pos) = buffer.find('\n') {
                let line: String = buffer.drain(..=pos).collect();
                let line = line.trim_end_matches(['\n', '\r']);
                // SSE 语义：注释行与空行忽略，只取 `data:` 载荷
                let Some(payload) = line.strip_prefix("data:") else {
                    continue;
                };
                let payload = payload.trim();
                if payload.is_empty() {
                    continue;
                }
                for chunk in adapter.parse_chunk(payload)? {
                    match chunk {
                        StreamChunk::Text(t) => {
                            content.push_str(&t);
                            if !t.is_empty() {
                                self.emit_delta(run, &t, 0, 0);
                            }
                        }
                        StreamChunk::ToolDelta {
                            index,
                            id,
                            name,
                            arguments_delta,
                        } => {
                            let acc = tools.entry(index).or_default();
                            if let Some(i) = id {
                                acc.id = Some(i);
                            }
                            if let Some(n) = name {
                                acc.name = Some(n);
                            }
                            if let Some(a) = arguments_delta {
                                acc.arguments.push_str(&a);
                            }
                        }
                        StreamChunk::Usage {
                            prompt_tokens,
                            completion_tokens,
                            cache_read,
                            cache_write,
                        } => usage.merge(prompt_tokens, completion_tokens, cache_read, cache_write),
                        StreamChunk::Done => {
                            done = true;
                            break;
                        }
                    }
                }
                // 决策 64 / 100：流式 token 活动刷新 last_activity_at（节流）。
                // 注：节流间隔是真实时间的发射节奏；写入的 last_activity_at 值
                // 仍走 store.now()（Clock 接缝，决策 143），超时判定语义不变。
                if let Some(run) = run {
                    if last_heartbeat.elapsed() >= self.heartbeat_interval {
                        self.store.touch_run_heartbeat(run.run_id).await?;
                        last_heartbeat = Instant::now();
                    }
                }
            }
        }

        if let Some(run) = run {
            // 流结束的确定性心跳（流短于节流间隔时也有终值）
            self.store.touch_run_heartbeat(run.run_id).await?;
        }
        // usage 增量：文本为空、只带 token（决策 123：增量口径）
        if usage.prompt_tokens.is_some() || usage.completion_tokens.is_some() {
            self.emit_delta(
                run,
                "",
                usage.prompt_tokens.unwrap_or(0),
                usage.completion_tokens.unwrap_or(0),
            );
        }

        let tool_calls = tools
            .into_iter()
            .map(|(index, acc)| acc.into_tool_call(index))
            .collect::<Result<Vec<_>>>()?;
        Ok(AgentResponse {
            content: if content.is_empty() {
                None
            } else {
                Some(content)
            },
            tool_calls,
            prompt_tokens: usage.prompt_tokens.unwrap_or(0),
            completion_tokens: usage.completion_tokens.unwrap_or(0),
            cache_read_tokens: usage.cache_read.unwrap_or(0),
            cache_write_tokens: usage.cache_write.unwrap_or(0),
        })
    }

    fn emit_delta(&self, run: Option<&RunContext>, text: &str, prompt: u32, completion: u32) {
        let Some(run) = run else { return };
        self.sse.emit(SseEvent::ConversationDelta {
            task_id: run.task_id.clone(),
            branch: run.branch.clone(),
            run_id: run.run_id,
            agent_type: run.agent_type.clone(),
            role: "assistant".into(),
            text: text.to_string(),
            prompt_tokens: prompt,
            completion_tokens: completion,
        });
    }
}

impl LlmClient for ProductionLlm {
    fn complete(&self, request: LlmRequest) -> BoxFuture<'static, Result<AgentResponse>> {
        let this = self.clone();
        Box::pin(async move {
            let provider = this.resolve_provider(&request).await?;
            let adapter = adapter_family(&provider.vendor)?;
            this.run_stream(adapter, &provider, &request, request.run.as_ref())
                .await
        })
    }
}

/// vendor → 适配器族；白名单以 [`crate::config::SUPPORTED_ADAPTERS`] 为唯一来源
/// （决策 103：代码硬编码，改它要发版——新增 vendor 必须同步适配器族映射，漏了就红）。
pub(crate) fn adapter_family(vendor: &str) -> Result<&'static dyn Adapter> {
    if !crate::config::SUPPORTED_ADAPTERS.contains(&vendor) {
        return Err(Error::Llm(format!(
            "不支持的 vendor：{vendor}（须 ∈ SUPPORTED_ADAPTERS）"
        )));
    }
    if is_openai_compatible(vendor) {
        Ok(&openai::OpenAiCompatible)
    } else if vendor == "anthropic" {
        Ok(&anthropic::Anthropic)
    } else {
        Err(Error::Llm(format!("vendor {vendor} 尚无适配器族实现")))
    }
}

/// 流式工具调用的聚合槽。
#[derive(Default)]
struct ToolAccum {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

impl ToolAccum {
    fn into_tool_call(self, index: usize) -> Result<ToolCall> {
        Ok(ToolCall {
            id: self.id.unwrap_or_else(|| format!("call_{index}")),
            name: self
                .name
                .ok_or_else(|| Error::Llm(format!("第 {index} 个工具调用缺少 name（流不完整）")))?,
            arguments: self.arguments,
        })
    }
}

/// usage 聚合：OpenAI 的 usage 与 Anthropic 的 message_start / message_delta
/// 分属不同事件，按字段覆盖合并。
#[derive(Default)]
struct UsageAccum {
    prompt_tokens: Option<u32>,
    completion_tokens: Option<u32>,
    cache_read: Option<u32>,
    cache_write: Option<u32>,
}

impl UsageAccum {
    fn merge(
        &mut self,
        prompt_tokens: Option<u32>,
        completion_tokens: Option<u32>,
        cache_read: Option<u32>,
        cache_write: Option<u32>,
    ) {
        if prompt_tokens.is_some() {
            self.prompt_tokens = prompt_tokens;
        }
        if completion_tokens.is_some() {
            self.completion_tokens = completion_tokens;
        }
        if cache_read.is_some() {
            self.cache_read = cache_read;
        }
        if cache_write.is_some() {
            self.cache_write = cache_write;
        }
    }
}

fn preview(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        text.to_string()
    } else {
        let cut: String = text.chars().take(limit).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
pub(crate) fn fixture_provider(vendor: &str, model: &str, base_url: Option<&str>) -> Provider {
    Provider {
        id: "p1".into(),
        vendor: vendor.into(),
        model: model.into(),
        context_window: 1000,
        base_url: base_url.map(String::from),
        api_key: Some("sk-test".into()),
        enabled: true,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_strips_trailing_slash_and_falls_back_by_vendor() {
        assert_eq!(
            base_url(&fixture_provider("openai", "m-1", None)),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            base_url(&fixture_provider("deepseek", "m-1", None)),
            "https://api.deepseek.com"
        );
        assert_eq!(
            base_url(&fixture_provider("anthropic", "m-1", None)),
            "https://api.anthropic.com"
        );
        assert_eq!(
            base_url(&fixture_provider(
                "openai",
                "m-1",
                Some("http://127.0.0.1:9/")
            )),
            "http://127.0.0.1:9"
        );
    }

    #[test]
    fn deepseek_shares_the_openai_compatible_family() {
        assert!(is_openai_compatible("openai"));
        assert!(is_openai_compatible("deepseek"));
        assert!(!is_openai_compatible("anthropic"));
    }

    #[test]
    fn every_supported_adapter_maps_to_a_family() {
        // 决策 103：白名单改它要发版——扩白名单而漏适配器族，此测试直接红
        for vendor in crate::config::SUPPORTED_ADAPTERS {
            assert!(
                adapter_family(vendor).is_ok(),
                "vendor {vendor} 在白名单内但没有适配器族"
            );
        }
        assert!(adapter_family("mystery-llm").is_err());
    }
}
