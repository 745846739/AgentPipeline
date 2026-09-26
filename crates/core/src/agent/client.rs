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
    /// 这次调用的**推理 / 思考**原文（决策 244）。没有推理的模型给 `None`。
    ///
    /// **它不是 `content` 的一部分**，也**绝不回灌**进后续请求：部分厂商把推理段
    /// 视为模型内部状态，发回去会被拒；进 `messages` 还会让下一轮的 prompt 平白翻倍
    /// （它是这一轮里最长的东西）。它的去处只有两个：实时增量
    /// （`conversation_delta` 的 `reasoning` 声道）与对讲台的展示留痕
    /// （`kanban_foreman_messages.thinking`）——**它不进模型上下文**。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
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
    /// 这次流式调用**收到过多少字节**（决策 231 的「量速」）。
    ///
    /// `None` = 没量到（FakeAgent 之类的脚本实现不产流量），不是「量到 0」——收字节总量
    /// 与最后一次收字节的时刻合起来才分得开「流快但 prompt 本身大」与「流被压到极慢」，
    /// 而 0 会把两者都说成「什么都没收到」。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_received: Option<u64>,
    /// 最后一次从流里收到字节的时刻（决策 231）。
    ///
    /// 与 `finished_at` 一起读才有意义：一个请求在飞、而最后一次收字节是一分钟前，说明它
    /// 卡在流上；两者都往前走，说明它只是慢。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_byte_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// 工具定义（`submit_metadata` 的 parameters 由各阶段 serde 结构体派生，决策 38）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// 8 个内置工具（决策 45 / 172③）。
///
/// `Skill`（决策 172③，票 06）与上游同名是**功能性决定而非命名偏好**：上游技能的正文里
/// 写着 `Call the Skill tool with "grilling"`，工具同名使这些正文**无需改写即可执行**。
///
/// `spawn_sub_agent` **不在**此列——它是需要阶段显式声明的扩展工具（见
/// [`SPAWN_SUB_AGENT_TOOL`]）。
pub const BUILTIN_TOOLS: [&str; 8] = [
    "write_file",
    "edit_file",
    "read_file",
    "delete_file",
    "list_dir",
    "run_command",
    "submit_metadata",
    "Skill",
];

/// 系统最小基线里不可移除的工具（§10.6.2）。
///
/// 就是 [`BUILTIN_TOOLS`] 去掉 `Skill`：`Skill` 是**能力增量**，由阶段声明启用
/// （决策 172③ 明确它不进 `MANDATORY_TOOLS`）——渐进披露下大量技能在池子里，
/// 不该无条件把「按名拉取技能」这个动作塞给每个节点。
pub const MANDATORY_TOOLS: [&str; 7] = [
    "write_file",
    "edit_file",
    "read_file",
    "delete_file",
    "list_dir",
    "run_command",
    "submit_metadata",
];

/// `Skill` 工具名（决策 172③）。与上游同名，使上游技能正文无需改写即可执行。
pub const SKILL_TOOL: &str = "Skill";

/// `spawn_sub_agent` 工具名（决策 172③，票 08）。
///
/// **扩展工具而非内置工具**：它不进 [`BUILTIN_TOOLS`]，也不进 [`MANDATORY_TOOLS`]——
/// 只有阶段显式声明才可用（决策 172③ 明确「不继承阶段声明工具」「不再派子代理」）。
/// 之所以不列进 [`BUILTIN_TOOLS`]：内置集是「每个 agent 都可能拿到」的语义，
/// 而子代理是**要显式授予**的能力。
pub const SPAWN_SUB_AGENT_TOOL: &str = "spawn_sub_agent";

/// v1 已知工具名全集：阶段配置**可以**声明的那些。
///
/// = [`BUILTIN_TOOLS`]（8）+ 扩展工具 [`SPAWN_SUB_AGENT_TOOL`]。判据只有这一处
/// （与 [`crate::agent::tools::denied_by_tier`] 同姿态）：工具定义的生成
/// （`model_request::tool_defs`）与阶段配置的准入校验（`config::validate_startup`，
/// 由 `PUT /stage-configs` 复用）问的是同一个问题——两处各写一份的后果是
/// 「写入时说不知道这个名字、运行时却把它丢掉」这种只能靠现象定位的漂移。
///
/// **不在这里的名字 = v1 不存在的能力**：声明它是配置错误，不是「暂时没实现」。
/// 故它一律 **fail fast**（决策 154 之后立的这条姿态，与 `deny_unknown_fields` /
/// 「引用不存在的 skill → 拒绝启动」同源），不再有「静默丢弃 + 一条 warn」这条路。
pub fn is_known_tool_name(name: &str) -> bool {
    BUILTIN_TOOLS.contains(&name) || name == SPAWN_SUB_AGENT_TOOL
}

/// [`is_known_tool_name`] 承认的全部名字（报错时列出，让用户照着改）。
pub fn known_tool_names() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = BUILTIN_TOOLS.to_vec();
    out.push(SPAWN_SUB_AGENT_TOOL);
    out
}

/// 「声明了 v1 不存在的工具」的**唯一**报文（决策 154 的后续票）。
///
/// 两处会报它：写入 / 启动校验（`config::validate_startup`，含 `PUT /stage-configs`）与执行期
/// 兜底（`model_request::tool_defs`，校验被绕过时才可能走到）。同一个错误只能有一种说法——两处各写
/// 一句的后果是同一个配置问题看起来像两个不同的问题，而对这句话有支配权的（未来加白名单、
/// 改措辞）只有这里。`where_` 是定位串（如「阶段 develop」/「阶段 develop 的 tools_json」）。
pub fn unknown_tools_message(where_: &str, unknown: &[String]) -> String {
    format!(
        "{where_} 声明了 v1 不存在的工具：{}（v1 已知工具集：{}）",
        unknown.join(" / "),
        known_tool_names().join(" / ")
    )
}

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
    /// 归属会话（决策 204⑥）：只有值班长的增量属于某个班次，流水线运行留空。
    ///
    /// 它不是「另一个 task_id」——`task_id` 定位流水线节点，这个定位对讲台里的班次。
    /// 对讲台需要它是因为**同一台机器上多处可以同时说话**（手机 + 电脑），
    /// 而回话的增量走的是同一条广播。
    pub session_id: String,
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
    /// **逐调用空闲判死**（决策 288 / 票 foreman-unbounded 05）：流上 N 秒没有新字节即中止
    /// 本次调用（按 LlmErrorKind::IdleTimeout 归类）。判据是**流上的字节**，从发出请求
    /// 那一刻起算——不是响应完成的时限。`None` = 不启用（现状一字不动）：流水线节点的
    /// 挂死由调度器的心跳判定收口（决策 64/66/88，进程级的另一把尺）；目前只有值班长
    /// 的调用带这个值（foreman 行的 `idle_timeout_sec`，缺省与节点同数 300s）。
    pub idle_timeout_sec: Option<u64>,
}

/// LLM 客户端接缝。
pub trait LlmClient: Send + Sync + 'static {
    fn complete(&self, request: LlmRequest) -> BoxFuture<'static, Result<AgentResponse>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ArchitectExecuteMetadata, ValidateInputMetadata};

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
    fn validate_input_blockers_schema_carries_the_contract() {
        // 决策 277②：blockers 的「问题 + 推荐答案」语义同步进 tool schema——
        // 模板与 schema 同源同话，模型在两处读到的都是同一条契约。
        let tool = submit_metadata_tool::<ValidateInputMetadata>("提交输入充分性判定");
        let desc = tool.parameters["properties"]["blockers"]["description"]
            .as_str()
            .unwrap_or_default();
        assert!(desc.contains("问题 + 推荐答案"), "{desc}");
        assert!(desc.contains("默认值"), "{desc}");
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
    fn builtin_tool_set_matches_decision_45_and_172() {
        assert_eq!(BUILTIN_TOOLS.len(), 8, "决策 172③：7 → 8（+ Skill）");
        assert!(BUILTIN_TOOLS.contains(&"submit_metadata"));
        assert!(BUILTIN_TOOLS.contains(&SKILL_TOOL));
        assert!(!BUILTIN_TOOLS.contains(&"spawn_sub_agent"));
    }

    /// 判据只有一处（决策 154 的后续票）：`is_known_tool_name` 承认的正好是
    /// 「全部内置 + 扩展工具」——多认一个名字会让配置写出运行时根本不存在的工具。
    #[test]
    fn known_tool_names_is_builtins_plus_extended() {
        let known = known_tool_names();
        assert_eq!(known.len(), BUILTIN_TOOLS.len() + 1);
        for b in BUILTIN_TOOLS {
            assert!(known.contains(&b), "{b} 应在已知集合里");
            assert!(is_known_tool_name(b));
        }
        assert!(is_known_tool_name(SPAWN_SUB_AGENT_TOOL));
        assert!(!is_known_tool_name("web_search"));
        assert!(!is_known_tool_name("read_fil"), "拼错的名字不是已知工具");
    }

    /// 决策 172③：`Skill` **不进** `MANDATORY_TOOLS`——它由阶段声明启用，
    /// 不是每个节点都该拿到的能力。
    #[test]
    fn skill_tool_is_not_mandatory() {
        assert_eq!(MANDATORY_TOOLS.len(), 7);
        assert!(
            !MANDATORY_TOOLS.contains(&SKILL_TOOL),
            "Skill 不得进入基线强制工具"
        );
        assert!(MANDATORY_TOOLS.contains(&"submit_metadata"));
    }
}
