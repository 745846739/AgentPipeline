//! OpenAI 兼容适配器（`openai` / `deepseek` 同族，票 13）。
//!
//! 请求：`POST {base_url}/chat/completions`，`stream: true` +
//! `stream_options.include_usage`（usage 只随最后一个 chunk 下发）。
//! 流：`data: {json}` 行 + 终止标记 `data: [DONE]`；工具调用按 `index`
//! 分片增量下发，需聚合。

use reqwest::RequestBuilder;

use crate::agent::client::{LlmRequest, Message, Role, ToolDef};
use crate::types::Provider;
use crate::{Error, Result};

use super::{Adapter, StreamChunk};

pub struct OpenAiCompatible;

impl Adapter for OpenAiCompatible {
    fn endpoint_path(&self) -> &'static str {
        "/chat/completions"
    }

    fn apply_auth(&self, builder: RequestBuilder, provider: &Provider) -> RequestBuilder {
        match &provider.api_key {
            Some(key) if !key.is_empty() => builder.bearer_auth(key),
            _ => builder,
        }
    }

    fn build_body(&self, provider: &Provider, request: &LlmRequest) -> Result<serde_json::Value> {
        let mut messages = vec![
            serde_json::json!({"role": "system", "content": request.system_prompt}),
            serde_json::json!({"role": "user", "content": request.user_prompt}),
        ];
        for m in &request.messages {
            messages.push(wire_message(m)?);
        }

        let mut body = serde_json::json!({
            "model": provider.model,
            "stream": true,
            "stream_options": {"include_usage": true},
            "messages": messages,
        });
        if let Some(t) = request.temperature {
            body["temperature"] = serde_json::json!(t);
        }
        if let Some(mt) = request.max_tokens {
            body["max_tokens"] = serde_json::json!(mt);
        }
        if !request.tools.is_empty() {
            body["tools"] =
                serde_json::json!(request.tools.iter().map(wire_tool).collect::<Vec<_>>());
        }
        Ok(body)
    }

    fn parse_chunk(&self, payload: &str) -> Result<Vec<StreamChunk>> {
        if payload == "[DONE]" {
            return Ok(vec![StreamChunk::Done]);
        }
        let value: serde_json::Value = serde_json::from_str(payload)
            .map_err(|e| Error::Llm(format!("OpenAI 流 chunk 解析失败：{e}：{payload}")))?;

        let mut chunks = Vec::new();
        // usage 与 choices 可同载荷并存（vLLM 等网关在最后一个 chunk 同时带 usage
        // 和收尾 delta），不得因 early-return 丢掉任何一侧
        if let Some(usage) = value.get("usage").filter(|u| !u.is_null()) {
            let cached = usage
                .get("prompt_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
                .and_then(|v| v.as_u64());
            chunks.push(StreamChunk::Usage {
                prompt_tokens: usage
                    .get("prompt_tokens")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32),
                completion_tokens: usage
                    .get("completion_tokens")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32),
                cache_read: cached.map(|v| v as u32),
                cache_write: None, // OpenAI 协议无缓存写入计数
            });
        }

        let Some(delta) = value
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("delta"))
        else {
            return Ok(chunks);
        };
        // content 与 tool_calls 也可并存于同一 delta，都收
        if let Some(text) = delta.get("content").and_then(|v| v.as_str()) {
            chunks.push(StreamChunk::Text(text.to_string()));
        }
        // 推理增量（决策 244）：`reasoning_content` 是 DeepSeek / 部分 OpenAI 兼容网关
        // 的字段名（vLLM、月之暗面等沿用），`reasoning` 是另一支。两者都收——
        // 认不出就只是没有思考可看，而认错字段会把它当成回话正文念出来。
        for key in ["reasoning_content", "reasoning"] {
            if let Some(thought) = delta.get(key).and_then(|v| v.as_str()) {
                if !thought.is_empty() {
                    chunks.push(StreamChunk::Reasoning(thought.to_string()));
                }
            }
        }
        if let Some(calls) = delta.get("tool_calls").and_then(|v| v.as_array()) {
            // 一个 chunk 可能携带多个 tool_call 分片，全部交给驱动层聚合
            chunks.extend(calls.iter().enumerate().map(|(position, call)| {
                let index = call
                    .get("index")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize)
                    .unwrap_or(position);
                let function = call.get("function");
                StreamChunk::ToolDelta {
                    index,
                    id: call.get("id").and_then(|v| v.as_str()).map(String::from),
                    name: function
                        .and_then(|f| f.get("name"))
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    arguments_delta: function
                        .and_then(|f| f.get("arguments"))
                        .and_then(|v| v.as_str())
                        .map(String::from),
                }
            }));
        }
        Ok(chunks)
    }
}

fn wire_message(message: &Message) -> Result<serde_json::Value> {
    let mut body = match message.role {
        Role::System => serde_json::json!({"role": "system", "content": message.content}),
        Role::User => serde_json::json!({"role": "user", "content": message.content}),
        Role::Assistant => {
            let mut m = serde_json::json!({"role": "assistant"});
            match &message.content {
                Some(c) => m["content"] = serde_json::json!(c),
                None => {
                    m["content"] = serde_json::Value::Null;
                }
            }
            if !message.tool_calls.is_empty() {
                m["tool_calls"] = serde_json::json!(message.tool_calls);
            }
            m
        }
        Role::Tool => {
            let mut m = serde_json::json!({
                "role": "tool",
                "content": message.content.clone().unwrap_or_default(),
            });
            if let Some(id) = &message.tool_call_id {
                m["tool_call_id"] = serde_json::json!(id);
            }
            if let Some(name) = &message.name {
                m["name"] = serde_json::json!(name);
            }
            m
        }
    };
    if body.get("content").is_none() {
        body["content"] = serde_json::Value::Null;
    }
    Ok(body)
}

fn wire_tool(tool: &ToolDef) -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.parameters,
        },
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
            temperature: Some(0.2),
            max_tokens: Some(512),
            provider_id: None,
            run: None,
            idle_timeout_sec: None,
        }
    }

    #[test]
    fn body_maps_system_user_and_tool_roundtrip() {
        let call = ToolCall {
            id: "call_1".into(),
            name: "write_file".into(),
            arguments: r#"{"path":"a.md"}"#.into(),
        };
        let messages = vec![
            Message::assistant(None, vec![call.clone()]),
            Message::tool_result(&call, "{\"success\":true}"),
        ];
        let body = OpenAiCompatible
            .build_body(
                &fixture_provider("openai", "gpt-test", Some("http://127.0.0.1:1")),
                &request(messages),
            )
            .unwrap();
        assert_eq!(body["model"], "gpt-test");
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
        assert_eq!(body["temperature"], 0.2);
        assert_eq!(body["max_tokens"], 512);
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[1]["role"], "user");
        assert_eq!(msgs[2]["role"], "assistant");
        assert_eq!(msgs[2]["tool_calls"][0]["id"], "call_1");
        assert_eq!(msgs[2]["tool_calls"][0]["function"]["name"], "write_file");
        assert_eq!(msgs[3]["role"], "tool");
        assert_eq!(msgs[3]["tool_call_id"], "call_1");
    }

    #[test]
    fn body_omits_unset_sampling_and_empty_tools() {
        let mut req = request(vec![]);
        req.temperature = None;
        req.max_tokens = None;
        let body = OpenAiCompatible
            .build_body(
                &fixture_provider("openai", "gpt-test", Some("http://127.0.0.1:1")),
                &req,
            )
            .unwrap();
        assert!(body.get("temperature").is_none());
        assert!(body.get("max_tokens").is_none());
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn chunk_parses_text_tool_fragments_usage_and_done() {
        // 文本增量
        let text = OpenAiCompatible
            .parse_chunk(r#"{"choices":[{"index":0,"delta":{"role":"assistant","content":"你好"},"finish_reason":null}]}"#)
            .unwrap();
        assert!(matches!(text.as_slice(), [StreamChunk::Text(t)] if t == "你好"));

        // 工具调用分片：id/name 先到，arguments 分两段（片段避开嵌套引号）
        let head = OpenAiCompatible
            .parse_chunk(
                r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_9","type":"function","function":{"name":"write_file","arguments":"{\"path\":"}}]}}]}"#,
            )
            .unwrap();
        assert!(matches!(
            head.as_slice(),
            [StreamChunk::ToolDelta {
                index: 0,
                id: Some(_),
                name: Some(_),
                arguments_delta: Some(_),
                ..
            }]
        ));
        let tail = OpenAiCompatible
            .parse_chunk(
                r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"1}"}}]}}]}"#,
            )
            .unwrap();
        assert!(matches!(
            tail.as_slice(),
            [StreamChunk::ToolDelta { index: 0, id: None, name: None, arguments_delta: Some(a), .. }] if a == "1}"
        ));

        // 一个 chunk 携带两个 tool_call 分片 → 两个块都保留
        let multi = OpenAiCompatible
            .parse_chunk(
                r#"{"choices":[{"index":0,"delta":{"tool_calls":[
                    {"index":0,"id":"call_a","type":"function","function":{"name":"read_file","arguments":"{}"}},
                    {"index":1,"id":"call_b","type":"function","function":{"name":"list_dir","arguments":"{}"}}
                ]}}]}"#,
            )
            .unwrap();
        assert_eq!(multi.len(), 2);
        assert!(matches!(
            multi.as_slice(),
            [
                StreamChunk::ToolDelta { index: 0, .. },
                StreamChunk::ToolDelta { index: 1, .. }
            ]
        ));

        // usage chunk（choices 缺失）
        let usage = OpenAiCompatible
            .parse_chunk(
                r#"{"usage":{"prompt_tokens":20,"completion_tokens":10,"prompt_tokens_details":{"cached_tokens":6}}}"#,
            )
            .unwrap();
        assert!(matches!(
            usage.as_slice(),
            [StreamChunk::Usage {
                prompt_tokens: Some(20),
                completion_tokens: Some(10),
                cache_read: Some(6),
                cache_write: None,
            }]
        ));

        assert!(matches!(
            OpenAiCompatible.parse_chunk("[DONE]").unwrap().as_slice(),
            [StreamChunk::Done]
        ));
    }

    /// 跨语言 golden fixture（票 e2e-mock/01，`tests/fixtures/e2e_mock_sse.json`）。
    ///
    /// 这份 fixture 是**两侧 mock 与 Rust 适配器之间的唯一契约**：Rust testkit 的
    /// `sse_tool` / `sse_text` 与 Node harness 的 `sseTool` / `sseText`（vitest 侧）各自
    /// 断言自己产出的字节与它一致；这里负责另一半——**消费**。把 fixture 的每个载荷逐条
    /// 喂 [`OpenAiCompatible::parse_chunk`]，断言解析出的块序列正是驱动层聚合所依赖的形状
    /// （tool 名 / arguments / 文本 / usage / `[DONE]`）。任一侧漂移，总有一侧变红。
    ///
    /// 键序**不是**契约（`serde_json` 按键序输出、JS 按插入序），故两侧都比对解析后的结构；
    /// 字节层只钉 SSE 帧与字段位置（票据第 4 条）。
    #[test]
    fn shared_golden_fixture_parses_into_the_expected_chunk_sequence() {
        #[derive(serde::Deserialize)]
        struct Fixture {
            usage: FixtureUsage,
            cases: Vec<FixtureCase>,
        }
        #[derive(serde::Deserialize)]
        struct FixtureUsage {
            prompt_tokens: u32,
            completion_tokens: u32,
        }
        #[derive(serde::Deserialize)]
        struct FixtureCase {
            name: String,
            producer: String,
            call: serde_json::Value,
            sse: String,
        }

        let raw = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/e2e_mock_sse.json"
        ));
        let fixture: Fixture = serde_json::from_str(raw).expect("fixture 必须是合法 JSON");
        assert_eq!(
            fixture.cases.len(),
            3,
            "三种步骤形态：tool call / submit / text"
        );

        for case in &fixture.cases {
            // ① 字节契约：`data: ` 前缀、空行分帧、`[DONE]` 终止
            assert!(
                case.sse.starts_with("data: "),
                "{}：缺 data: 前缀",
                case.name
            );
            assert!(
                case.sse.ends_with("data: [DONE]\n\n"),
                "{}：缺 [DONE] 终止",
                case.name
            );
            let events: Vec<&str> = case.sse.split("\n\n").filter(|e| !e.is_empty()).collect();
            assert_eq!(
                events.len(),
                3,
                "{}：三个事件（chunk / usage / [DONE]）",
                case.name
            );
            for event in &events {
                assert!(event.starts_with("data: "), "{}：{event}", case.name);
            }

            // ② 逐条喂适配器——与驱动层 `run_stream` 同一入口与同一取载荷方式
            let mut chunks = Vec::new();
            for event in &events {
                let payload = event.strip_prefix("data:").expect("已断言前缀").trim();
                chunks.extend(
                    OpenAiCompatible
                        .parse_chunk(payload)
                        .unwrap_or_else(|e| panic!("{}：{e}", case.name)),
                );
            }
            assert!(
                matches!(chunks.last(), Some(StreamChunk::Done)),
                "{}：流必须以 [DONE] 收尾：{chunks:?}",
                case.name
            );

            // ③ 结构：工具调用（tool call / submit）或文本（text）
            match case.producer.as_str() {
                "sseTool" => {
                    let (name, args) = chunks
                        .iter()
                        .find_map(|c| match c {
                            StreamChunk::ToolDelta {
                                name: Some(n),
                                arguments_delta: Some(a),
                                ..
                            } => Some((n.clone(), a.clone())),
                            _ => None,
                        })
                        .unwrap_or_else(|| panic!("{}：没有工具调用块：{chunks:?}", case.name));
                    assert_eq!(name, case.call["name"], "{}：工具名", case.name);
                    let got: serde_json::Value = serde_json::from_str(&args).unwrap_or_else(|e| {
                        panic!(
                            "{}：arguments 片段拼起来不是合法 JSON（{e}）：{args}",
                            case.name
                        )
                    });
                    assert_eq!(got, case.call["arguments"], "{}：工具参数", case.name);
                    // id 由现场生成，两侧比对前归一为 fixture 里这个固定值（fixture 头注）
                    assert!(
                        chunks.iter().any(|c| matches!(
                            c,
                            StreamChunk::ToolDelta { id: Some(id), .. } if id == "call_fixture_1"
                        )),
                        "{}：工具调用 id 须归一为 call_fixture_1：{chunks:?}",
                        case.name
                    );
                }
                "sseText" => {
                    let text = chunks
                        .iter()
                        .find_map(|c| match c {
                            StreamChunk::Text(t) => Some(t.clone()),
                            _ => None,
                        })
                        .unwrap_or_else(|| panic!("{}：没有文本块：{chunks:?}", case.name));
                    assert_eq!(text, case.call["text"], "{}：文本", case.name);
                }
                other => panic!("{}：fixture 里的 producer 不认识：{other}", case.name),
            }

            // ④ usage：两个字段都要解析出来（漂移时这里就是「完全静默」的那一处）
            let usage = chunks.iter().find_map(|c| match c {
                StreamChunk::Usage {
                    prompt_tokens,
                    completion_tokens,
                    ..
                } => Some((*prompt_tokens, *completion_tokens)),
                _ => None,
            });
            assert_eq!(
                usage,
                Some((
                    Some(fixture.usage.prompt_tokens),
                    Some(fixture.usage.completion_tokens)
                )),
                "{}：usage 字段",
                case.name
            );
        }
    }

    #[test]
    fn reasoning_deltas_are_their_own_channel() {
        // 决策 244：推理与回话是两条声道。DeepSeek / 部分兼容网关用 `reasoning_content`。
        let deepseek = OpenAiCompatible
            .parse_chunk(r#"{"choices":[{"index":0,"delta":{"reasoning_content":"让我想想"}}]}"#)
            .unwrap();
        assert!(
            matches!(deepseek.as_slice(), [StreamChunk::Reasoning(t)] if t == "让我想想"),
            "{deepseek:?}"
        );

        // 另一支网关用 `reasoning`——两个字段名都收
        let alt = OpenAiCompatible
            .parse_chunk(r#"{"choices":[{"index":0,"delta":{"reasoning":"再想想"}}]}"#)
            .unwrap();
        assert!(
            matches!(alt.as_slice(), [StreamChunk::Reasoning(t)] if t == "再想想"),
            "{alt:?}"
        );

        // 推理与正文同一 delta 并存时**两条都收、且不混**：合成一段的话界面就分不出
        // 哪一句该当回话念、哪一句该收进折叠块了（那正是分开声道的全部理由）。
        // 顺序是正文在前、推理在后（与解析顺序一致，测试照实钉住这个顺序）。
        let both = OpenAiCompatible
            .parse_chunk(
                r#"{"choices":[{"index":0,"delta":{"reasoning_content":"想好了","content":"答案是 42"}}]}"#,
            )
            .unwrap();
        assert_eq!(both.len(), 2, "{both:?}");
        assert!(matches!(&both[0], StreamChunk::Text(t) if t == "答案是 42"));
        assert!(matches!(&both[1], StreamChunk::Reasoning(t) if t == "想好了"));

        // 空串不产块：否则每一轮都会多出一堆空增量事件
        assert!(OpenAiCompatible
            .parse_chunk(r#"{"choices":[{"index":0,"delta":{"reasoning_content":""}}]}"#)
            .unwrap()
            .is_empty());

        // 不产推理的模型（没有这两个字段）照旧只有正文
        let plain = OpenAiCompatible
            .parse_chunk(r#"{"choices":[{"index":0,"delta":{"content":"普通回话"}}]}"#)
            .unwrap();
        assert!(matches!(plain.as_slice(), [StreamChunk::Text(t)] if t == "普通回话"));
    }

    #[test]
    fn usage_and_delta_coexisting_in_one_payload_are_both_kept() {
        // vLLM 式网关：最后一个 chunk 同时带收尾 delta 与 usage
        let mixed = OpenAiCompatible
            .parse_chunk(
                r#"{"choices":[{"index":0,"delta":{"content":"完成"},"finish_reason":"stop"}],"usage":{"prompt_tokens":9,"completion_tokens":3}}"#,
            )
            .unwrap();
        assert_eq!(mixed.len(), 2, "{mixed:?}");
        assert!(matches!(mixed[0], StreamChunk::Usage { .. }));
        assert!(matches!(&mixed[1], StreamChunk::Text(t) if t == "完成"));

        // 同一 delta 里 content 与 tool_calls 并存也都不丢
        let both = OpenAiCompatible
            .parse_chunk(
                r#"{"choices":[{"index":0,"delta":{"content":"调工具","tool_calls":[{"index":0,"id":"c1","type":"function","function":{"name":"read_file","arguments":"{}"}}]}}]}"#,
            )
            .unwrap();
        assert_eq!(both.len(), 2, "{both:?}");
        assert!(matches!(&both[0], StreamChunk::Text(_)));
        assert!(matches!(&both[1], StreamChunk::ToolDelta { index: 0, .. }));
    }

    #[test]
    fn malformed_chunk_is_a_clean_llm_error() {
        let err = OpenAiCompatible.parse_chunk("{not json").unwrap_err();
        assert!(err.to_string().contains("解析失败"), "{err}");
    }
}
