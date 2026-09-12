//! Agent 子系统：LLM 接缝、结构化输出解析、文件工具策略、脱敏、上下文压缩、prompt 组装。

pub mod client;
pub mod context;
pub mod file_policy;
pub mod metadata;
pub mod prompts;
pub mod sanitize;
pub mod tools;

pub use client::{
    submit_metadata_tool, AgentResponse, LlmClient, LlmRequest, Message, Role, ToolCall, ToolDef,
    BUILTIN_TOOLS, MANDATORY_TOOLS,
};
pub use file_policy::{FileOp, FileToolPolicy};
pub use metadata::{
    extract_metadata, find_last_balanced_json, parse_metadata, retry_prompt, MetadataExtraction,
    MetadataSource,
};
pub use prompts::{
    build_system_prompt, build_user_prompt, default_agents_context, prompt_template_hash,
    render_template, resolve_persona, PromptSegments, TemplateVars, BASELINE_PREAMBLE,
    FORMAT_RULES,
};
pub use tools::{CommandFinish, CommandRecorder, CommandStart, ToolCallContext, ToolExecutor};
