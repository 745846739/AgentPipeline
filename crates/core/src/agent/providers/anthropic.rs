//! Anthropic 适配器（票 13）。
//!
//! 请求：`POST {base_url}/v1/messages`，`x-api-key` + `anthropic-version` 鉴权，
//! system prompt 走顶层 `system` 字段，`max_tokens` 必填（阶段配置缺省时用
//! [`super::DEFAULT_MAX_TOKENS`]）。流：`event:`/`data:` 行，按 data 载荷的
//! `type` 分发——usage 分散在 `message_start`（input + cache）与
//! `message_delta`（output）；工具调用是 `tool_use` block + `input_json_delta`
//! 分片；工具结果以 user 角色 `tool_result` block 回传（连续同角色合并）。

use reqwest::RequestBuilder;

use crate::agent::client::{LlmRequest, Message, Role, ToolDef};
use crate::types::Provider;
use crate::{Error, Result};

use super::{Adapter, StreamChunk};

pub const ANTHROPIC_VERSION: &str = "2023-06-01";

pub struct Anthropic;

impl Adapter for Anthropic {
    fn endpoint_path(&self) -> &'static str {
        "/v1/messages"
    }

    fn apply_auth(&self, builder: RequestBuilder, provider: &Provider) -> RequestBuilder {
        let builder = builder.header("anthropic-version", ANTHROPIC_VERSION);
        match &provider.api_key {
            Some(key) if !key.is_empty() => builder.header("x-api-key", key),
            _ => builder,
        }
    }

    fn build_body(&self, provider: &Provider, request: &LlmRequest) -> Result<serde_json::Value> {
        // 出口消毒（票 106-stability/09）：与 openai 适配器同一条纪律——孤儿 tool 消息
        // 的序列到不了 wire。健康转录零改动，不碰前缀缓存。
        let (messages, stats) = crate::agent::context::sanitize_tool_sequence(&request.messages);
        if stats.orphan_results > 0 || stats.unanswered_calls > 0 {
            tracing::warn!(
                orphan_results = stats.orphan_results,
                unanswered_calls = stats.unanswered_calls,
                stage = %request.stage,
                node = %request.node,
                "转录里有配对不完整的工具消息，已在出口摘除（孤儿 tool 消息会被 provider 拒收）"
            );
        }
        // 会话内的 system 消息并入顶层 system（协议不允许 messages 里出现 system）
        let mut system_parts = vec![request.system_prompt.clone()];
        for m in &messages {
            if m.role == Role::System {
                if let Some(c) = &m.content {
                    system_parts.push(c.clone());
                }
            }
        }
        // system 恒为 content block 形式：最后一块打 prompt-cache 断点（ephemeral），
        // 断点之前的 system 前缀可增量命中（spec `.scratch/prompt-cache`——
        // docs/agents.md 的「LLM Cache 策略」至此与实现对上）。
        let mut system: Vec<serde_json::Value> = system_parts
            .iter()
            .map(|p| serde_json::json!({"type": "text", "text": p}))
            .collect();
        if let Some(last) = system.last_mut() {
            last["cache_control"] = serde_json::json!({"type": "ephemeral"});
        }
        let system = serde_json::Value::Array(system);

        let mut wire = wire_messages(&messages)?;
        // 空 user_prompt 不占 wire 头（值班长主轮的快照并进末尾轮之后就是这个形状）：
        // 空 user 消息是无效报文（Anthropic 拒收空文本块），也白吃一段前缀。
        if !request.user_prompt.is_empty() {
            wire.insert(
                0,
                serde_json::json!({
                    "role": "user",
                    "content": [{"type": "text", "text": request.user_prompt}],
                }),
            );
        }
        // 第二个断点打在最后一条消息的最后一个内容块上：断点之前的全部消息前缀可命中，
        // 下一轮在其后追加即可续用（增量缓存）。全请求恰 2 个断点（provider 上限 4，留余量）。
        if let Some(blocks) = wire.last_mut().and_then(|m| m["content"].as_array_mut()) {
            if let Some(last) = blocks.last_mut() {
                last["cache_control"] = serde_json::json!({"type": "ephemeral"});
            }
        }

        let mut body = serde_json::json!({
            "model": provider.model,
            "stream": true,
            "system": system,
            "max_tokens": request.max_tokens.unwrap_or(super::DEFAULT_MAX_TOKENS),
            "messages": wire,
        });
        if let Some(t) = request.temperature {
            body["temperature"] = serde_json::json!(t);
        }
        if !request.tools.is_empty() {
            body["tools"] =
                serde_json::json!(request.tools.iter().map(wire_tool).collect::<Vec<_>>());
        }
        Ok(body)
    }

    fn parse_chunk(&self, payload: &str) -> Result<Vec<StreamChunk>> {
        let value: serde_json::Value = serde_json::from_str(payload)
            .map_err(|e| Error::Llm(format!("Anthropic 流事件解析失败：{e}：{payload}")))?;
        let kind = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
        Ok(match kind {
            "message_start" => {
                let usage = value
                    .pointer("/message/usage")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                // prompt 计量归一：input + cache_read + cache_creation（票 13）
                let input = usage
                    .get("input_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let read = usage
                    .get("cache_read_input_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let write = usage
                    .get("cache_creation_input_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                vec![StreamChunk::Usage {
                    prompt_tokens: Some((input + read + write) as u32),
                    completion_tokens: None,
                    cache_read: Some(read as u32),
                    cache_write: Some(write as u32),
                }]
            }
            "content_block_start" => {
                let block = value.get("content_block").cloned().unwrap_or_default();
                if block.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
                    let index = value
                        .get("index")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as usize)
                        .unwrap_or(0);
                    // input 常规为空对象（参数在后续 input_json_delta 分片里）；
                    // 只有非空 input 才作为 arguments 起点，避免 "{}" 污染聚合
                    let input = block.get("input").cloned().unwrap_or_default();
                    let seeded = input.as_object().map(|o| !o.is_empty()).unwrap_or(false);
                    vec![StreamChunk::ToolDelta {
                        index,
                        id: block.get("id").and_then(|v| v.as_str()).map(String::from),
                        name: block.get("name").and_then(|v| v.as_str()).map(String::from),
                        arguments_delta: if seeded {
                            Some(serde_json::to_string(&input)?)
                        } else {
                            None
                        },
                    }]
                } else {
                    Vec::new()
                }
            }
            "content_block_delta" => {
                let delta = value.get("delta").cloned().unwrap_or_default();
                let index = value
                    .get("index")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize)
                    .unwrap_or(0);
                match delta.get("type").and_then(|v| v.as_str()) {
                    Some("text_delta") => vec![StreamChunk::Text(
                        delta
                            .get("text")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                    )],
                    // 扩展思考的增量（决策 244）：`thinking` 是它的正文。
                    // 本适配器**不开启** `thinking` 请求参数（那是另一件事：开启后必须原样
                    // 回灌思考块，而回灌会改变既有请求形状），故这条分支只为「对端自己发了」
                    // 那种情形准备——收到就展示，而不是静默丢掉。
                    Some("thinking_delta") => {
                        let thought = delta.get("thinking").and_then(|v| v.as_str()).unwrap_or("");
                        if thought.is_empty() {
                            Vec::new()
                        } else {
                            vec![StreamChunk::Reasoning(thought.to_string())]
                        }
                    }
                    Some("input_json_delta") => vec![StreamChunk::ToolDelta {
                        index,
                        id: None,
                        name: None,
                        arguments_delta: delta
                            .get("partial_json")
                            .and_then(|v| v.as_str())
                            .map(String::from),
                    }],
                    _ => Vec::new(),
                }
            }
            "message_delta" => {
                let usage = value.get("usage").cloned().unwrap_or_default();
                let mut chunks = Vec::new();
                // 收尾原因（票 01③）：`max_tokens` 与 OpenAI 的 `length` 同义——输出被上限切断。
                if let Some(reason) = value
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(|v| v.as_str())
                {
                    chunks.push(StreamChunk::FinishReason(reason.to_string()));
                }
                // 新版协议会在 message_delta 补 cache 计量；present 才覆盖
                chunks.push(StreamChunk::Usage {
                    prompt_tokens: None,
                    completion_tokens: usage
                        .get("output_tokens")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as u32),
                    cache_read: usage
                        .get("cache_read_input_tokens")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as u32),
                    cache_write: usage
                        .get("cache_creation_input_tokens")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as u32),
                });
                chunks
            }
            "message_stop" => vec![StreamChunk::Done],
            _ => Vec::new(),
        })
    }
}

/// 内部消息 → Anthropic wire 消息：连续同角色合并（工具结果会连续多条 user）。
fn wire_messages(messages: &[Message]) -> Result<Vec<serde_json::Value>> {
    let mut out: Vec<serde_json::Value> = Vec::new();
    for message in messages.iter().filter(|m| m.role != Role::System) {
        let role = match message.role {
            Role::Assistant => "assistant",
            _ => "user",
        };
        let blocks = content_blocks(message)?;
        if let Some(last) = out.last_mut() {
            if last["role"] == serde_json::json!(role) {
                if let Some(arr) = last["content"].as_array_mut() {
                    arr.extend(blocks);
                    continue;
                }
            }
        }
        out.push(serde_json::json!({"role": role, "content": blocks}));
    }
    Ok(out)
}

fn content_blocks(message: &Message) -> Result<Vec<serde_json::Value>> {
    let mut blocks = Vec::new();
    if message.role == Role::Tool {
        // 工具结果整体进 tool_result.content，不再另发 text block
        blocks.push(serde_json::json!({
            "type": "tool_result",
            "tool_use_id": message.tool_call_id.clone().unwrap_or_default(),
            "content": message.content.clone().unwrap_or_default(),
        }));
        return Ok(blocks);
    }
    if let Some(text) = message.content.as_deref().filter(|t| !t.is_empty()) {
        blocks.push(serde_json::json!({"type": "text", "text": text}));
    }
    if message.role == Role::Assistant {
        for call in &message.tool_calls {
            let input: serde_json::Value = if call.arguments.trim().is_empty() {
                serde_json::json!({})
            } else {
                serde_json::from_str(&call.arguments).map_err(|e| {
                    Error::Llm(format!(
                        "工具 {} 的 arguments 不是合法 JSON：{e}",
                        call.name
                    ))
                })?
            };
            blocks.push(serde_json::json!({
                "type": "tool_use",
                "id": call.id,
                "name": call.name,
                "input": input,
            }));
        }
    }
    if blocks.is_empty() {
        blocks.push(serde_json::json!({"type": "text", "text": ""}));
    }
    Ok(blocks)
}

fn wire_tool(tool: &ToolDef) -> serde_json::Value {
    serde_json::json!({
        "name": tool.name,
        "description": tool.description,
        "input_schema": tool.parameters,
    })
}

#[cfg(test)]
mod tests {
    use super::super::fixture_provider;
    use super::*;
    use crate::agent::client::ToolCall;

    fn request(messages: Vec<Message>) -> LlmRequest {
        LlmRequest {
            stage: crate::types::Stage::ArchitectDesign,
            node: crate::types::Node::Execute,
            attempt: 1,
            system_prompt: "系统提示".into(),
            user_prompt: "用户提示".into(),
            messages,
            tools: vec![],
            temperature: None,
            max_tokens: None,
            provider_id: None,
            run: None,
            idle_timeout_sec: None,
        }
    }

    #[test]
    fn body_maps_tool_roundtrip_with_coalescing_and_defaults() {
        let call = ToolCall {
            id: "toolu_1".into(),
            name: "write_file".into(),
            arguments: r#"{"path":"a.md","content":"x"}"#.into(),
        };
        let messages = vec![
            Message::assistant(None, vec![call.clone()]),
            Message::tool_result(&call, "{\"success\":true}"),
            Message::tool_result(&call, "{\"success\":false}"),
        ];
        let body = Anthropic
            .build_body(
                &fixture_provider("anthropic", "claude-test", Some("http://127.0.0.1:1")),
                &request(messages),
            )
            .unwrap();
        assert_eq!(body["model"], "claude-test");
        assert_eq!(body["stream"], true);
        assert_eq!(body["max_tokens"], super::super::DEFAULT_MAX_TOKENS);
        assert_eq!(body["system"][0]["text"], "系统提示");

        let msgs = body["messages"].as_array().unwrap();
        // [user_prompt, assistant(tool_use), user(两个 tool_result 合并)]
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0]["content"][0]["text"], "用户提示");
        assert_eq!(msgs[1]["role"], "assistant");
        assert_eq!(msgs[1]["content"][0]["type"], "tool_use");
        assert_eq!(msgs[1]["content"][0]["id"], "toolu_1");
        assert_eq!(msgs[1]["content"][0]["input"]["path"], "a.md");
        assert_eq!(msgs[2]["role"], "user");
        assert_eq!(msgs[2]["content"].as_array().unwrap().len(), 2);
        assert_eq!(msgs[2]["content"][0]["type"], "tool_result");
        assert_eq!(msgs[2]["content"][0]["tool_use_id"], "toolu_1");
    }

    #[test]
    fn in_session_system_messages_fold_into_top_level_system() {
        let messages = vec![Message::system("会话内补充约束")];
        let mut req = request(messages);
        req.system_prompt = "基线".into();
        let body = Anthropic
            .build_body(
                &fixture_provider("anthropic", "claude-test", Some("http://127.0.0.1:1")),
                &req,
            )
            .unwrap();
        assert_eq!(body["system"].as_array().unwrap().len(), 2);
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1, "system 消息不得进入 messages");
    }

    #[test]
    fn cache_breakpoints_mark_the_system_tail_and_the_last_message() {
        // prompt-cache spec（.scratch/prompt-cache）：断点打在 system 尾与最后一条消息尾，
        // 断点之前的全部前缀（system + 历史）可增量命中；数量恰为 2（上限度 4 留余量）。
        let messages = vec![
            Message::user("第一句"),
            Message::assistant(Some("答了".into()), Vec::new()),
            Message::user("第二句"),
        ];
        let body = Anthropic
            .build_body(
                &fixture_provider("anthropic", "claude-test", Some("http://127.0.0.1:1")),
                &request(messages),
            )
            .unwrap();

        // system 恒为 content block 形式，最后一块带 ephemeral 断点
        let sys = body["system"].as_array().expect("system 恒为 block 数组");
        assert_eq!(sys[0]["type"], "text");
        assert_eq!(sys[0]["text"], "系统提示");
        assert_eq!(
            sys.last().unwrap()["cache_control"]["type"],
            "ephemeral",
            "system 尾要打缓存断点：{sys:?}"
        );

        // 最后一条消息的最后一个内容块带断点
        let msgs = body["messages"].as_array().unwrap();
        let blocks = msgs.last().unwrap()["content"]
            .as_array()
            .expect("消息内容恒为 block 数组");
        assert_eq!(
            blocks.last().unwrap()["cache_control"]["type"],
            "ephemeral",
            "消息尾要打缓存断点：{blocks:?}"
        );

        let raw = serde_json::to_string(&body).unwrap();
        assert_eq!(
            raw.matches("\"cache_control\"").count(),
            2,
            "全请求断点数恰为 2（system 尾 + 消息尾）：{raw}"
        );
    }

    #[test]
    fn an_empty_user_prompt_leaves_no_empty_wire_head_message() {
        // 值班长主轮的 user_prompt 是空的（快照并进末尾轮之后）——
        // 空槽不得变成一条空 user 消息占住 wire 头（Anthropic 拒收空文本块）。
        let mut req = request(vec![Message::user("第一句")]);
        req.user_prompt = String::new();
        let body = Anthropic
            .build_body(
                &fixture_provider("anthropic", "claude-test", Some("http://127.0.0.1:1")),
                &req,
            )
            .unwrap();
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1, "空 user_prompt 不占 wire 头：{msgs:?}");
        assert_eq!(msgs[0]["content"][0]["text"], "第一句");
    }

    #[test]
    fn events_parse_start_deltas_and_stop() {
        // message_start：input + cache 计量归一
        let start = Anthropic
            .parse_chunk(
                r#"{"type":"message_start","message":{"usage":{"input_tokens":10,"cache_creation_input_tokens":4,"cache_read_input_tokens":6,"output_tokens":1}}}"#,
            )
            .unwrap();
        assert!(matches!(
            start.as_slice(),
            [StreamChunk::Usage {
                prompt_tokens: Some(20),
                completion_tokens: None,
                cache_read: Some(6),
                cache_write: Some(4),
            }]
        ));

        // tool_use block 开始（input 为空对象 → 不播种 arguments）+ input_json_delta 分片
        let block = Anthropic
            .parse_chunk(
                r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_9","name":"run_command","input":{}}}"#,
            )
            .unwrap();
        assert!(matches!(
            block.as_slice(),
            [StreamChunk::ToolDelta {
                index: 1,
                id: Some(_),
                name: Some(_),
                arguments_delta: None,
                ..
            }]
        ));
        let frag = Anthropic
            .parse_chunk(
                r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"command\":"}}"#,
            )
            .unwrap();
        assert!(matches!(
            frag.as_slice(),
            [StreamChunk::ToolDelta {
                index: 1,
                id: None,
                name: None,
                arguments_delta: Some(_),
                ..
            }]
        ));

        // 文本增量
        let text = Anthropic
            .parse_chunk(
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"正在分析"}}"#,
            )
            .unwrap();
        assert!(matches!(text.as_slice(), [StreamChunk::Text(t)] if t == "正在分析"));

        // message_delta（收尾原因 + output tokens）+ message_stop
        let out = Anthropic
            .parse_chunk(r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":42}}"#)
            .unwrap();
        assert!(matches!(
            out.as_slice(),
            [
                StreamChunk::FinishReason(r),
                StreamChunk::Usage {
                    completion_tokens: Some(42),
                    ..
                }
            ] if r == "tool_use"
        ));
        // 新版协议在 message_delta 补 cache 计量
        let out_cached = Anthropic
            .parse_chunk(
                r#"{"type":"message_delta","delta":{},"usage":{"output_tokens":50,"cache_read_input_tokens":8}}"#,
            )
            .unwrap();
        assert!(matches!(
            out_cached.as_slice(),
            [StreamChunk::Usage {
                prompt_tokens: None,
                completion_tokens: Some(50),
                cache_read: Some(8),
                cache_write: None,
            }]
        ));
        assert!(matches!(
            Anthropic
                .parse_chunk(r#"{"type":"message_stop"}"#)
                .unwrap()
                .as_slice(),
            [StreamChunk::Done]
        ));

        // 未知事件（如 ping）忽略
        assert!(Anthropic
            .parse_chunk(r#"{"type":"ping"}"#)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn thinking_delta_is_its_own_channel() {
        // 决策 244：扩展思考的增量走 `thinking_delta` / `thinking`，与正文分开。
        // 本适配器不开启 `thinking` 请求参数（那要原样回灌思考块，是另一件事），
        // 但**对端发了就展示**——静默丢掉会让「它想了什么」永远看不见。
        let thought = Anthropic
            .parse_chunk(
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"先看台账"}}"#,
            )
            .unwrap();
        assert!(
            matches!(thought.as_slice(), [StreamChunk::Reasoning(t)] if t == "先看台账"),
            "{thought:?}"
        );

        // 空串不产块（照 text_delta 的分寸：空增量只会在驱动层被丢掉）
        assert!(Anthropic
            .parse_chunk(
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":""}}"#
            )
            .unwrap()
            .is_empty());
    }

    #[test]
    fn malformed_event_is_a_clean_llm_error() {
        let err = Anthropic.parse_chunk("{{").unwrap_err();
        assert!(err.to_string().contains("解析失败"), "{err}");
    }
}
