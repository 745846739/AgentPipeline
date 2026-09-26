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
use crate::agent::degeneration;
use crate::sse::{Channel, SseEvent, SseSink};
use crate::storage::Store;
use crate::types::Provider;
use crate::{Error, Result};

/// Anthropic 必填 max_tokens 的兜底值（阶段配置未给时，§10.6.3）。
pub const DEFAULT_MAX_TOKENS: u32 = 8192;
/// 流式心跳的最小间隔：token 再密也不超过每秒一次 DB 写（决策 64）。
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
/// 错误信息里携带的响应体上限。
const ERROR_BODY_PREVIEW: usize = 500;

/// provider 配置类失败的**可归因类别**（主流程票 03）。
///
/// 用户配错模型或密钥是真实使用中最高频的失败；此前它只表现为原始错误串
/// （英文 HTTP 状态 + 供应商返回体片段）配上通用文案「重试耗尽，需要用户介入」，
/// 用户既不知道哪一步失败、也不知道该改什么。分类后由 `pending.message` 给中文
/// 可操作指引，原始错误串作为诊断信息保留在 `pending.context.diagnostic`（不丢）。
///
/// **未知情形一律 `None`**：宁可退回原始错误串，也不把未识别的错误误标成已知类别
/// （误标会给出**错误**的修复指引，比不给指引更糟）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmErrorKind {
    /// 鉴权失败（401 / 403）——密钥错误、过期、或项目无权限。
    Auth,
    /// 模型不存在 / 无该模型权限（404，或错误体指明 model）。
    ModelNotFound,
    /// 连不上（DNS / 连接被拒 / TLS / 超时）——base_url 写错或网络不通。
    Network,
    /// 超出上下文窗口（400 + 长度/上下文相关错误体）。
    ContextWindow,
    /// **额度 / 账单不足**（402，或错误体里的余额 / 配额字样）——决策 271。
    ///
    /// 来历是 2026-09-24 的实测：本地代理回 `HTTP 400 …insufficient credits…`，`from_http`
    /// 认不出这段体 → 落进 `Error::Llm` → 台账类别读作 `llm_network`、指引写「请检查 base_url
    /// 是否正确、网络是否可达」。**一次账单问题被读成配置问题**，而且按网络类的节奏重试。
    /// 等一等不会自己好，动作在 provider 那一侧（续费 / 换 provider），故它单独一档。
    Quota,
    /// **空闲判死**（决策 288 / 票 foreman-unbounded 05）：一次调用的流上 N 秒没有新字节。
    ///
    /// 它不是 HTTP 层的失败（`from_http` **永不**返回它）——是消费侧的 watchdog 在流循环里
    /// 判的：请求已经发出去、连接也在，只是对面一个字节都不再给。与「挂住」同一形状的还有
    /// 连接阶段；判据从发出请求那一刻起算。按瞬时类处置：值班长这一侧重试一次这一次调用。
    IdleTimeout,
}

impl LlmErrorKind {
    /// 面向用户的中文可操作提示（写进 `pending.message`）。
    pub fn advice(self) -> &'static str {
        match self {
            LlmErrorKind::Auth => {
                "provider 鉴权失败：请到「设置 · 模型与密钥」检查 api_key 是否正确、是否过期"
            }
            LlmErrorKind::ModelNotFound => {
                "provider 模型不可用：请到「设置 · 模型与密钥」确认 model 名称与账号权限"
            }
            LlmErrorKind::Network => "连不上 provider：请检查 base_url 是否正确、网络是否可达",
            LlmErrorKind::ContextWindow => {
                "请求超出模型上下文窗口：请换用更大上下文窗口的模型，或调大 context_window 配置"
            }
            LlmErrorKind::Quota => {
                "provider 额度不足（余额 / 配额）：这不是网络问题——续费或换一个 provider 再试"
            }
            LlmErrorKind::IdleTimeout => {
                "模型很久没有给出任何内容（空闲判死）：多半是 provider 临时卡住——稍等片刻重试通常能过"
            }
        }
    }

    /// 稳定标识（写入 `pending.context.kind` 与诊断，便于断言与检索）。
    pub fn as_str(self) -> &'static str {
        match self {
            LlmErrorKind::Auth => "llm_auth",
            LlmErrorKind::ModelNotFound => "llm_model_not_found",
            LlmErrorKind::Network => "llm_network",
            LlmErrorKind::ContextWindow => "llm_context_window",
            LlmErrorKind::Quota => "llm_quota",
            LlmErrorKind::IdleTimeout => "llm_idle_timeout",
        }
    }

    /// 按 HTTP 状态与错误体判定类别；未识别返回 `None`。
    pub fn from_http(status: u16, body: &str) -> Option<Self> {
        let lower = body.to_ascii_lowercase();
        let has = |needles: &[&str]| needles.iter().any(|n| lower.contains(n));
        match status {
            401 | 403 => Some(LlmErrorKind::Auth),
            404 => Some(LlmErrorKind::ModelNotFound),
            // 402 = Payment Required，字面就是额度。与错误体无关（有代理只给状态码）
            402 => Some(LlmErrorKind::Quota),
            // 400 区域：模型名错误与超长是两种不同的用户动作，按错误体区分
            400 => {
                if has(&[
                    "model_not_found",
                    "does not exist",
                    "unknown model",
                    "no such model",
                ]) || (lower.contains("model") && has(&["not found", "invalid"]))
                {
                    Some(LlmErrorKind::ModelNotFound)
                } else if has(&[
                    "context_length",
                    "context window",
                    "too long",
                    "maximum context",
                    "max_tokens",
                    "token limit",
                ]) {
                    Some(LlmErrorKind::ContextWindow)
                } else if has(&[
                    // 额度 / 账单那一族（决策 271）。放在模型名与超长之后：那两支更具体，
                    // 而「insufficient credits」这类体里不会同时出现模型名或长度字样。
                    "insufficient credit",
                    "insufficient_quota",
                    "insufficient quota",
                    "quota exceeded",
                    "exceeded your quota",
                    "payment required",
                    "billing",
                    "insufficient balance",
                ]) {
                    Some(LlmErrorKind::Quota)
                } else {
                    None
                }
            }
            429 => None, // 限流：可重试，不属配置错误
            _ => None,
        }
    }
}

/// 这个错误是「请求超出模型上下文窗口」吗——**两个消费者共用的判据**（决策 291 / 295）。
///
/// 值班长撞墙后的压缩重试（`pipeline/foreman.rs`，票 06(c)）与流水线「压缩一次、且不再
/// 盲目重试」（`pipeline/model_invoke.rs`，票 10）判的是**同一件事**。各写一份 `matches!`
/// 的下场是一边改了另一边不知道，而两边都靠这个字符串和 [`LlmErrorKind::ContextWindow`] 对上。
pub fn is_context_window(error: &crate::Error) -> bool {
    matches!(
        error,
        crate::Error::LlmClassified { kind, .. } if kind == LlmErrorKind::ContextWindow.as_str()
    )
}

/// 把「HTTP 请求失败」的错误归类为网络类：连接 / DNS / TLS / 超时。
const NETWORK_KIND: LlmErrorKind = LlmErrorKind::Network;

/// 时限的人话说法（写进失败账里给人看的那一句）。
///
/// **整分钟才说分钟**：90 秒按整除写成「1 分钟」是在少报现场——
/// 这一句正是人拿去判断「它到底挂了多久」的东西。
fn human_duration(limit: Duration) -> String {
    let secs = limit.as_secs();
    if secs >= 60 && secs % 60 == 0 {
        format!("{} 分钟", secs / 60)
    } else {
        format!("{secs} 秒")
    }
}

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
    /// 推理（思考）增量（决策 244）。
    ///
    /// **与 `Text` 分开是必须的，不是好看**：它是模型的草稿而不是它要说的话——
    /// 两件事混成一个声道之后，界面分不出哪一段是回话、哪一段是过程，而
    /// 「把思考当回话念给人听」与「把回话当思考折叠起来」都是错的。
    ///
    /// 两条去处，都要与 `Text` 分开走：① 实时增量（`conversation_delta` 的
    /// `channel = reasoning`）；② 攒进 [`AgentResponse::reasoning`] 供对讲台展示留痕。
    /// **绝不用它拼 `content`、绝不回灌**——回灌会破坏部分厂商的协议（推理段不是 assistant
    /// 消息的一部分，OpenAI 兼容族把 `reasoning_content` 发回去会被拒），而它通常是一轮里
    /// 最长的一段，进历史窗口会每轮白烧一份 token。
    Reasoning(String),
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
        // 逐调用空闲判死（决策 288 / 票 foreman-unbounded 05）：请求带了空闲界才启用。
        // 判据是**流上的字节**——「距上一个字节超过阈值」从发出请求那一刻起算
        // （连接阶段挂着不吐响应头，与「流中途停了」是同一种挂），不是响应完成的时限。
        // **空块不重置这条界**：能把它往后挪的只有真收到的字节（[`advance_idle_deadline`]）。
        // `None` = 不启用（现状一字不动）：节点路径不走这里，它们的挂死由调度器心跳收口。
        let idle_timeout = request.idle_timeout_sec.map(Duration::from_secs);
        let idle_error = |idle: Duration, received: u64| Error::LlmClassified {
            kind: LlmErrorKind::IdleTimeout.as_str().to_string(),
            message: LlmErrorKind::IdleTimeout.advice().to_string(),
            raw: format!(
                "流上 {} 没有任何新字节（本次已收 {received} 字节）：已中止这一次调用",
                human_duration(idle)
            ),
        };
        let response = match idle_timeout {
            Some(idle) => match tokio::time::timeout(idle, builder.send()).await {
                Ok(result) => result.map_err(|e| Error::LlmClassified {
                    kind: NETWORK_KIND.as_str().to_string(),
                    message: NETWORK_KIND.advice().to_string(),
                    raw: format!("HTTP 请求失败：{e}"),
                })?,
                Err(_) => return Err(idle_error(idle, 0)),
            },
            None => builder.send().await.map_err(|e| Error::LlmClassified {
                kind: NETWORK_KIND.as_str().to_string(),
                message: NETWORK_KIND.advice().to_string(),
                raw: format!("HTTP 请求失败：{e}"),
            })?,
        };
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            let raw = format!("HTTP {status}：{}", preview(&text, ERROR_BODY_PREVIEW));
            return Err(match LlmErrorKind::from_http(status.as_u16(), &text) {
                Some(kind) => Error::LlmClassified {
                    kind: kind.as_str().to_string(),
                    message: kind.advice().to_string(),
                    raw,
                },
                // 未识别：保留原始串（宁可给不出指引，不给出错误指引）
                None => Error::Llm(raw),
            });
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut content = String::new();
        // 推理原文（决策 244）：和 `content` 分开攒，只用于展示留痕，不回灌。
        let mut reasoning = String::new();
        let mut tools: BTreeMap<usize, ToolAccum> = BTreeMap::new();
        let mut usage = UsageAccum::default();
        let mut done = false;
        let mut last_heartbeat = Instant::now();
        // 量速读数（决策 231）：字节总量 + 最后一次收字节的时刻。它们在**这一层**才量得到
        // ——再往外一层只看得到最终响应，分不清「流快但 prompt 大」与「流被压到极慢」。
        let mut bytes_received: u64 = 0;
        let mut last_byte_at: Option<chrono::DateTime<chrono::Utc>> = None;
        // 空闲界的**期限**（不是「距上一次 `next()` 返回」）：每次真收到字节才往后挪，
        // 见 [`advance_idle_deadline`]。
        let mut idle_deadline = idle_timeout.map(|idle| tokio::time::Instant::now() + idle);

        while !done {
            let next = match (idle_deadline, idle_timeout) {
                (Some(deadline), Some(idle)) => {
                    match tokio::time::timeout_at(deadline, stream.next()).await {
                        Ok(next) => next,
                        Err(_) => return Err(idle_error(idle, bytes_received)),
                    }
                }
                _ => stream.next().await,
            };
            let Some(chunk) = next else {
                break;
            };
            let bytes = chunk.map_err(|e| Error::Llm(format!("读取流失败：{e}")))?;
            bytes_received += bytes.len() as u64;
            // 字节 ⇄ 时刻同源：`last_byte_at` 是**读数**（决策 231 的量速），这里同时是
            // 判死那条界的推进点——空块不进这两个读数（判据是字节，见上）。
            idle_deadline = advance_idle_deadline(
                idle_deadline,
                idle_timeout,
                tokio::time::Instant::now(),
                bytes.len(),
            );
            if bytes.is_empty() {
                continue;
            }
            last_byte_at = Some(self.store.now());
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
                            // 决策 280：流式护栏——累积文本尾部陷入复读循环即判废本轮，
                            // 立即返回（不等正常结束、不读完垃圾）；agent loop 按决策 278
                            // 续接转录＋错误 turn 重试。
                            if let Some(d) = degeneration::detect(&content) {
                                return Err(Error::Degenerated(format!(
                                    "片段「{}」连续重复 {} 次",
                                    preview(&d.unit, 24),
                                    d.repeats
                                )));
                            }
                            if !t.is_empty() {
                                self.emit_delta(run, Channel::Content, &t, 0, 0);
                            }
                        }
                        // 推理增量只走实时通道：**不入 `content`**（它不是回话的一部分，
                        // 回灌会破坏部分厂商协议），也不落进模型上下文（决策 244）。
                        StreamChunk::Reasoning(t) => {
                            reasoning.push_str(&t);
                            if !t.is_empty() {
                                self.emit_delta(run, Channel::Reasoning, &t, 0, 0);
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
                Channel::Content,
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
            reasoning: if reasoning.is_empty() {
                None
            } else {
                Some(reasoning)
            },
            tool_calls,
            prompt_tokens: usage.prompt_tokens.unwrap_or(0),
            completion_tokens: usage.completion_tokens.unwrap_or(0),
            cache_read_tokens: usage.cache_read.unwrap_or(0),
            cache_write_tokens: usage.cache_write.unwrap_or(0),
            bytes_received: Some(bytes_received),
            last_byte_at,
        })
    }

    fn emit_delta(
        &self,
        run: Option<&RunContext>,
        channel: Channel,
        text: &str,
        prompt: u32,
        completion: u32,
    ) {
        let Some(run) = run else { return };
        self.sse.emit(SseEvent::ConversationDelta {
            task_id: run.task_id.clone(),
            branch: run.branch.clone(),
            run_id: run.run_id,
            agent_type: run.agent_type.clone(),
            session_id: run.session_id.clone(),
            channel,
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

/// 「测试连接」探针结果（主流程票 03）。
///
/// **不含 api_key**——决策 112 的掩码语义不因本探针弱化；`raw` 是供应商错误体
/// 预览，本就不含密钥。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ConnectionTest {
    pub ok: bool,
    pub latency_ms: u64,
    /// 失败时的可归因类别（`llm_auth` / `llm_model_not_found` / …）；未知情形 None。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// 中文可读结论（成功 / 分类提示 / 配置本身的问题）。
    pub message: String,
    /// 原始诊断（失败时保留，供排查）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}

/// provider「测试连接」探针（决策 160）：对给定配置发一次最小真实流式请求
/// （`max_tokens = 1`），按 [`LlmErrorKind`] 归类失败，让用户在建任务前就能
/// 发现密钥 / 模型 / 地址配错。探针自带 15s 总超时，配置页不被挂死。
pub async fn test_provider_connection(provider: &Provider) -> ConnectionTest {
    let started = Instant::now();
    let latency = |started: Instant| started.elapsed().as_millis() as u64;
    let adapter = match adapter_family(&provider.vendor) {
        Ok(a) => a,
        Err(e) => {
            return ConnectionTest {
                ok: false,
                latency_ms: 0,
                kind: None,
                message: e.to_string(),
                raw: None,
            }
        }
    };
    let http = match Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return ConnectionTest {
                ok: false,
                latency_ms: 0,
                kind: None,
                message: format!("HTTP 客户端构建失败：{e}"),
                raw: None,
            }
        }
    };
    let request = LlmRequest {
        stage: crate::types::Stage::Init,
        node: crate::types::Node::Execute,
        attempt: 0,
        system_prompt: String::new(),
        user_prompt: "ping".into(),
        messages: vec![],
        tools: vec![],
        temperature: None,
        max_tokens: Some(1),
        provider_id: None,
        run: None,
        idle_timeout_sec: None,
    };
    let body = match adapter.build_body(provider, &request) {
        Ok(b) => b,
        Err(e) => {
            return ConnectionTest {
                ok: false,
                latency_ms: latency(started),
                kind: None,
                message: e.to_string(),
                raw: None,
            }
        }
    };
    let url = format!("{}{}", base_url(provider), adapter.endpoint_path());
    let builder = adapter.apply_auth(http.post(&url).json(&body), provider);
    let response = match builder.send().await {
        Ok(r) => r,
        Err(e) => {
            return ConnectionTest {
                ok: false,
                latency_ms: latency(started),
                kind: Some(NETWORK_KIND.as_str().to_string()),
                message: NETWORK_KIND.advice().to_string(),
                raw: Some(format!("HTTP 请求失败：{e}")),
            }
        }
    };
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        let raw = format!("HTTP {status}：{}", preview(&text, ERROR_BODY_PREVIEW));
        let (kind, message) = match LlmErrorKind::from_http(status.as_u16(), &text) {
            Some(k) => (Some(k.as_str().to_string()), k.advice().to_string()),
            None => (None, format!("连接失败：HTTP {status}")),
        };
        return ConnectionTest {
            ok: false,
            latency_ms: latency(started),
            kind,
            message,
            raw: Some(raw),
        };
    }
    // 成功：把（max_tokens=1 的）响应体读完再下结论，半途断流也算失败。
    let bytes = match response.bytes().await {
        Ok(b) => b,
        Err(e) => {
            return ConnectionTest {
                ok: false,
                latency_ms: latency(started),
                kind: Some(NETWORK_KIND.as_str().to_string()),
                message: NETWORK_KIND.advice().to_string(),
                raw: Some(format!("读取响应失败：{e}")),
            }
        }
    };
    if bytes.is_empty() {
        return ConnectionTest {
            ok: false,
            latency_ms: latency(started),
            kind: None,
            message: "连接成功但响应为空，请核对 base_url 是否指向模型服务".into(),
            raw: None,
        };
    }
    ConnectionTest {
        ok: true,
        latency_ms: latency(started),
        kind: None,
        message: "连接成功".into(),
        raw: None,
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

/// 逐调用空闲界的**推进**（决策 288 / 票 05）：**只有真的收到字节**才把界往后挪。
///
/// 为什么单拎出来：判据是「**流上的字节**」，而 `stream.next()` 返回一次不等于流上多了
/// 一个字节——空块（HTTP/2 的空 DATA 帧、被解码器吞掉的保活帧）不带字节却照样返回。
/// 按「返回了就重置」写，一条只发空帧的连接能把这一次调用挂到天荒地老，而那正是这一票
/// 要杀的那件事（「一个字都没有」）。真流的空块造不出来（TCP 上写 0 字节等于没写），
/// 故这条判据落在这里由单测钉住；网络那一层只钉「超时就判死」。
///
/// 空闲界没启用（`idle` 为 `None`）时恒返回原值——`None` 就是「不启用 watchdog」，
/// 这条函数不替调用方做那个决定。
fn advance_idle_deadline(
    deadline: Option<tokio::time::Instant>,
    idle: Option<Duration>,
    now: tokio::time::Instant,
    bytes: usize,
) -> Option<tokio::time::Instant> {
    match (deadline, idle) {
        (Some(_), Some(idle)) if bytes > 0 => Some(now + idle),
        _ => deadline,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 收到字节 → 界往后挪到「此刻 + 空闲界」。
    #[test]
    fn idle_bytes_move_the_deadline_forward() {
        let start = tokio::time::Instant::now();
        let moved = advance_idle_deadline(
            Some(start),
            Some(Duration::from_secs(300)),
            start + Duration::from_secs(10),
            3,
        );
        assert_eq!(moved, Some(start + Duration::from_secs(310)));
    }

    /// **空块不挪界**（票 05：判据是字节，不是「`next()` 返回了一次」）。
    #[test]
    fn an_empty_chunk_does_not_move_the_idle_deadline() {
        let start = tokio::time::Instant::now();
        let kept = advance_idle_deadline(
            Some(start),
            Some(Duration::from_secs(300)),
            start + Duration::from_secs(299),
            0,
        );
        assert_eq!(kept, Some(start), "空块不许重置空闲界");
    }

    /// 没启用空闲界时这条函数一个决定都不做。
    #[test]
    fn no_idle_bound_means_no_deadline() {
        let start = tokio::time::Instant::now();
        assert_eq!(
            advance_idle_deadline(None, None, start + Duration::from_secs(9), 42),
            None
        );
    }

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

    #[test]
    fn llm_error_classification_covers_the_configurable_failures() {
        // 主流程票 03：用户能配错的每一类都要有归因与可操作提示
        use LlmErrorKind as K;

        // 鉴权：401 / 403，与错误体无关
        assert_eq!(K::from_http(401, "{}"), Some(K::Auth));
        assert_eq!(K::from_http(403, "permission denied"), Some(K::Auth));
        // 模型不存在：404，或 400 体里指明 model
        assert_eq!(K::from_http(404, "Not Found"), Some(K::ModelNotFound));
        assert_eq!(
            K::from_http(400, r#"{"error":{"code":"model_not_found"}}"#),
            Some(K::ModelNotFound)
        );
        // 超长：400 + 上下文相关错误体
        assert_eq!(
            K::from_http(400, "This model's maximum context length is 8192 tokens"),
            Some(K::ContextWindow)
        );
        // 额度 / 账单：402 与「余额不足」类错误体（决策 271 的实测来源就是这一句）
        assert_eq!(K::from_http(402, "{}"), Some(K::Quota));
        assert_eq!(
            K::from_http(
                400,
                r#"{"error":{"message":"You have insufficient credits to make this request."}}"#
            ),
            Some(K::Quota)
        );
        assert_eq!(
            K::from_http(400, r#"{"error":{"code":"insufficient_quota"}}"#),
            Some(K::Quota)
        );
        // 新规则不吃掉老规则：只提 model 字样的 400 仍判模型不存在
        assert_eq!(
            K::from_http(400, r#"{"error":{"message":"model invalid"}}"#),
            Some(K::ModelNotFound)
        );
        // 未知情形必须退回原始串——不给错误指引比不给指引更糟
        assert_eq!(K::from_http(400, "weird vendor body"), None);
        assert_eq!(
            K::from_http(429, "rate limited"),
            None,
            "限流可重试，不属配置错误"
        );
        assert_eq!(K::from_http(500, "internal"), None);

        // 每个已知类别都有中文提示，且提示里不泄漏原始返回体
        for kind in [
            K::Auth,
            K::ModelNotFound,
            K::Network,
            K::ContextWindow,
            K::Quota,
        ] {
            let advice = kind.advice();
            assert!(!advice.is_empty(), "{kind:?} 缺少可操作提示");
            assert!(advice.contains('：'), "提示应含指引冒号：{advice}");
        }
        // 稳定标识：进 pending.context.diagnostic 之外的 kind 字段，供断言与检索
        assert_eq!(K::Auth.as_str(), "llm_auth");
        assert_eq!(K::Network.as_str(), "llm_network");
        assert_eq!(K::Quota.as_str(), "llm_quota");
    }

    #[test]
    fn classified_error_keeps_raw_diagnostic_separate_from_message() {
        // 主流程票 03：原始串进 context.diagnostic，message 只留中文可操作提示
        let err = Error::LlmClassified {
            kind: "llm_auth".into(),
            message: "provider 鉴权失败：请到「设置 · 模型与密钥」检查 api_key".into(),
            raw: "HTTP 401：{\"error\":{\"message\":\"Incorrect API key\"}}".into(),
        };
        assert_eq!(
            err.to_string(),
            "provider 鉴权失败：请到「设置 · 模型与密钥」检查 api_key",
            "Display = 用户看到的 message，不含原始串"
        );
        let (kind, raw) = err.llm_classified().expect("应可取回分类与原始诊断");
        assert_eq!(kind, "llm_auth");
        assert!(raw.contains("Incorrect API key"), "raw 应保留原始返回体");
    }
}
