//! LLM 调用接缝（决策 142 / 143）与消息模型。
//!
//! 生产实现是 [`crate::agent::providers::ProductionLlm`]（OpenAI 兼容 + Anthropic 两族，
//! 票 13）；测试实现是 testkit 的 FakeAgent——**只替换 LLM 响应流，工具层全部真实执行**
//! （决策 148）。因此这里的 trait 是测试边界，不是工具边界。

use futures::future::BoxFuture;
use schemars::JsonSchema;

use crate::types::{Node, Stage};
use crate::Result;

/// 消息角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }
}

/// 一次 LLM 调用里的工具调用请求。
///
/// 序列化形态与 §12.4.3 的 `messages_json` 一致（OpenAI 风格嵌套 `function` 对象），
/// 因为该 JSON 就是落库的会话内容，落库与 context 必须同源。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(from = "ToolCallWire", into = "ToolCallWire")]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// 原始 JSON 字符串（与 provider 契约一致）。
    pub arguments: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct ToolCallWire {
    id: String,
    #[serde(rename = "type", default = "function_type")]
    kind: String,
    function: ToolCallFunction,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct ToolCallFunction {
    name: String,
    arguments: String,
}

fn function_type() -> String {
    "function".to_string()
}

impl From<ToolCallWire> for ToolCall {
    fn from(w: ToolCallWire) -> Self {
        ToolCall {
            id: w.id,
            name: w.function.name,
            arguments: w.function.arguments,
        }
    }
}

impl From<ToolCall> for ToolCallWire {
    fn from(c: ToolCall) -> Self {
        ToolCallWire {
            id: c.id,
            kind: function_type(),
            function: ToolCallFunction {
                name: c.name,
                arguments: c.arguments,
            },
        }
    }
}

/// 会话消息（落库形态见 §12.4.3 的 `messages_json`）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Message {
    pub role: Role,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Message {
            role: Role::System,
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Message {
            role: Role::User,
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant(content: Option<String>, tool_calls: Vec<ToolCall>) -> Self {
        Message {
            role: Role::Assistant,
            content,
            tool_calls,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn tool_result(call: &ToolCall, content: impl Into<String>) -> Self {
        Message {
            role: Role::Tool,
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: Some(call.id.clone()),
            name: Some(call.name.clone()),
        }
    }
}

/// LLM 响应。
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AgentResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default)]
    pub prompt_tokens: u32,
    #[serde(default)]
    pub completion_tokens: u32,
    /// 缓存命中的 prompt token（决策 46；OpenAI `cached_tokens` / Anthropic
    /// `cache_read_input_tokens`）。是 `prompt_tokens` 的子集。
    #[serde(default)]
    pub cache_read_tokens: u32,
    /// 写入缓存的 prompt token（决策 46；Anthropic `cache_creation_input_tokens`，
    /// OpenAI 无对应字段恒为 0）。是 `prompt_tokens` 的子集。
    #[serde(default)]
    pub cache_write_tokens: u32,
}

/// 工具定义（`submit_metadata` 的 parameters 由各阶段 serde 结构体派生，决策 38）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// 7 个内置工具（决策 45）。`spawn_sub_agent` 是默认关闭的扩展工具。
pub const BUILTIN_TOOLS: [&str; 7] = [
    "write_file",
    "edit_file",
    "read_file",
    "delete_file",
    "list_dir",
    "run_command",
    "submit_metadata",
];

/// 系统最小基线里不可移除的工具（决策 10.6.2）。
pub const MANDATORY_TOOLS: [&str; 7] = BUILTIN_TOOLS;

/// 从阶段的 serde 结构体派生 `submit_metadata` 的 tool 定义（决策 38：schema 与校验同源）。
pub fn submit_metadata_tool<T: JsonSchema>(description: impl Into<String>) -> ToolDef {
    ToolDef {
        name: "submit_metadata".to_string(),
        description: description.into(),
        parameters: serde_json::to_value(schemars::schema_for!(T))
            .unwrap_or(serde_json::Value::Null),
    }
}

/// 流式与心跳所需的 run 上下文（决策 64 / 123）：生产适配器据此发射
/// `conversation_delta` 并刷新 `last_activity_at`；FakeAgent 忽略。
#[derive(Debug, Clone, PartialEq)]
pub struct RunContext {
    pub task_id: String,
    pub branch: String,
    pub run_id: i64,
    pub agent_type: String,
}

/// 一次节点调用的请求（节点级独立对话，决策 33）。
#[derive(Debug, Clone)]
pub struct LlmRequest {
    pub stage: Stage,
    pub node: Node,
    pub attempt: u32,
    pub system_prompt: String,
    pub user_prompt: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDef>,
    /// 阶段配置采样参数（§10.6.3 / 决策 46），None 时由适配器取默认值。
    pub temperature: Option<f64>,
    /// 阶段配置最大输出 token（§10.6.3 / 决策 46），None 时由适配器取默认值。
    pub max_tokens: Option<u32>,
    /// 任务级 provider 覆盖（决策 105）优先于阶段配置（生产适配器解析，票 13）。
    pub provider_id: Option<String>,
    /// 流式 run 上下文（决策 123）；None = 无流式（FakeAgent / 纯单元场景）。
    pub run: Option<RunContext>,
}

/// LLM 客户端接缝。
pub trait LlmClient: Send + Sync + 'static {
    fn complete(&self, request: LlmRequest) -> BoxFuture<'static, Result<AgentResponse>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ArchitectExecuteMetadata;

    #[test]
    fn submit_metadata_schema_derives_from_serde_struct() {
        let tool = submit_metadata_tool::<ArchitectExecuteMetadata>("提交架构设计元数据");
        assert_eq!(tool.name, "submit_metadata");
        let props = tool.parameters.get("properties").unwrap();
        // 决策 38：tool parameters 与校验逻辑同源——字段名来自结构体
        assert!(props.get("readiness").is_some());
        assert!(props.get("affected_files").is_some());
        assert!(props.get("acceptance_criteria").is_some());
        assert!(props.get("new_symbols").is_some());
    }

    #[test]
    fn message_shapes_serialize_like_design() {
        let call = ToolCall {
            id: "call_1".into(),
            name: "write_file".into(),
            arguments: r#"{"path":"design.md","content":"x"}"#.into(),
        };
        let msg = Message::tool_result(&call, r#"{"success":true}"#);
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["role"], "tool");
        assert_eq!(json["tool_call_id"], "call_1");
        assert_eq!(json["name"], "write_file");

        let assistant = Message::assistant(None, vec![call]);
        let json = serde_json::to_value(&assistant).unwrap();
        assert_eq!(json["role"], "assistant");
        assert_eq!(json["tool_calls"][0]["type"], "function");
        assert_eq!(json["tool_calls"][0]["function"]["name"], "write_file");
        assert_eq!(
            json["tool_calls"][0]["function"]["arguments"],
            r#"{"path":"design.md","content":"x"}"#
        );

        // 落库形态可反序列化回来
        let back: Message = serde_json::from_value(json).unwrap();
        assert_eq!(back.tool_calls[0].name, "write_file");
        assert_eq!(
            back.tool_calls[0].arguments,
            r#"{"path":"design.md","content":"x"}"#
        );
    }

    #[test]
    fn builtin_tool_set_matches_decision_45() {
        assert_eq!(BUILTIN_TOOLS.len(), 7);
        assert!(BUILTIN_TOOLS.contains(&"submit_metadata"));
        assert!(!BUILTIN_TOOLS.contains(&"spawn_sub_agent"));
    }
}
