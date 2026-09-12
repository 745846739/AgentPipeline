//! FakeAgent（决策 148）：**只替换 LLM 响应流，工具层全部真实执行**。
//!
//! 脚本按 `(stage, node)` 声明 tool_calls 序列；`submit_metadata` 的参数直接由各阶段
//! serde 结构体序列化（决策 38：编译期同源，不会漂移）。

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use agentpipeline_core::agent::client::{AgentResponse, LlmClient, LlmRequest, ToolCall};
use agentpipeline_core::types::{Node, Stage};
use agentpipeline_core::Result;
use futures::future::BoxFuture;
use serde::Serialize;

/// 脚本中的一步。
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// 发出一个 tool_call（工具**真实执行**）。
    Tool {
        name: String,
        arguments: serde_json::Value,
    },
    /// `submit_metadata`（类型化参数，编译期与校验同源）。
    Submit(serde_json::Value),
    /// 纯文本回复（元数据劣化注入：文本 JSON / 缺字段 / 坏 JSON）。
    Text(String),
    /// 不返回（心跳停跳 / 卡死），配合假时钟验证空闲 / 绝对超时。
    Stall,
}

/// 按 `(stage, node)` 组织的脚本。
#[derive(Debug, Default, Clone)]
pub struct Script {
    steps: HashMap<(Stage, Node), VecDeque<Step>>,
}

impl Script {
    pub fn new() -> Self {
        Script::default()
    }

    /// 为某个 `(stage, node)` 追加步骤。
    pub fn push(&mut self, stage: Stage, node: Node, step: Step) -> &mut Self {
        self.steps.entry((stage, node)).or_default().push_back(step);
        self
    }

    /// 链式脚本构建入口。
    pub fn for_node(&mut self, stage: Stage, node: Node) -> NodeScript<'_> {
        NodeScript {
            script: self,
            stage,
            node,
        }
    }

    /// 剩余步骤数。
    pub fn remaining(&self, stage: Stage, node: Node) -> usize {
        self.steps
            .get(&(stage, node))
            .map(VecDeque::len)
            .unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.steps.values().all(VecDeque::is_empty)
    }

    /// 已声明的 `(stage, node)` 列表。
    pub fn declared_nodes(&self) -> Vec<(Stage, Node)> {
        let mut v: Vec<(Stage, Node)> = self.steps.keys().copied().collect();
        v.sort();
        v
    }

    fn pop(&mut self, stage: Stage, node: Node) -> Option<Step> {
        self.steps
            .get_mut(&(stage, node))
            .and_then(|q| q.pop_front())
    }
}

/// 单个节点的脚本构建器。
pub struct NodeScript<'a> {
    script: &'a mut Script,
    stage: Stage,
    node: Node,
}

impl NodeScript<'_> {
    pub fn write_file(self, path: &str, content: &str) -> Self {
        self.push(Step::Tool {
            name: "write_file".into(),
            arguments: serde_json::json!({"path": path, "content": content}),
        })
    }

    pub fn read_file(self, path: &str) -> Self {
        self.push(Step::Tool {
            name: "read_file".into(),
            arguments: serde_json::json!({"path": path}),
        })
    }

    pub fn list_dir(self, path: &str) -> Self {
        self.push(Step::Tool {
            name: "list_dir".into(),
            arguments: serde_json::json!({"path": path}),
        })
    }

    pub fn run_command(self, command: &str) -> Self {
        self.push(Step::Tool {
            name: "run_command".into(),
            arguments: serde_json::json!({"command": command}),
        })
    }

    /// 工具失败注入：发出一个必然失败的调用（未知工具 / 越权路径），
    /// 由 agent loop 按 `tool_retry_max` 重试（G13 分层，决策 33）。
    pub fn failing_tool(self, name: &str, arguments: serde_json::Value) -> Self {
        self.push(Step::Tool {
            name: name.into(),
            arguments,
        })
    }

    /// 类型化 `submit_metadata`（参数由 serde 结构体序列化，决策 38）。
    pub fn submit<T: Serialize>(self, value: &T) -> Self {
        let json = serde_json::to_value(value).expect("元数据可序列化");
        self.push(Step::Submit(json))
    }

    /// 原始 JSON 形态的 `submit_metadata`（绕过类型，用于劣化注入）。
    pub fn submit_raw(self, value: serde_json::Value) -> Self {
        self.push(Step::Submit(value))
    }

    /// 文本回复（文本 JSON 块 / 缺字段 / 坏 JSON 的降级验证）。
    pub fn text(self, text: &str) -> Self {
        self.push(Step::Text(text.to_string()))
    }

    /// 不返回（超时路径）。
    pub fn stall(self) -> Self {
        self.push(Step::Stall)
    }

    pub fn push(self, step: Step) -> Self {
        self.script.push(self.stage, self.node, step);
        self
    }
}

struct Inner {
    script: Script,
    calls: Vec<(Stage, Node)>,
    prompt_tokens: u32,
    completion_tokens: u32,
}

/// 脚本化 LLM 替身。
#[derive(Clone)]
pub struct FakeAgent {
    inner: Arc<Mutex<Inner>>,
}

impl FakeAgent {
    pub fn new(script: Script) -> Self {
        FakeAgent {
            inner: Arc::new(Mutex::new(Inner {
                script,
                calls: Vec::new(),
                prompt_tokens: 0,
                completion_tokens: 0,
            })),
        }
    }

    /// 替换脚本（同一测试内多阶段切换用）。
    pub fn set_script(&self, script: Script) {
        self.inner.lock().unwrap().script = script;
    }

    /// 全部调用记录（按发生顺序）。
    pub fn call_log(&self) -> Vec<(Stage, Node)> {
        self.inner.lock().unwrap().calls.clone()
    }

    pub fn calls_for(&self, stage: Stage, node: Node) -> u32 {
        self.inner
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|(s, n)| *s == stage && *n == node)
            .count() as u32
    }

    pub fn total_calls(&self) -> u32 {
        self.inner.lock().unwrap().calls.len() as u32
    }

    pub fn remaining(&self, stage: Stage, node: Node) -> usize {
        self.inner.lock().unwrap().script.remaining(stage, node)
    }

    /// 尚未消费的脚本（断言"脚本跑完了"用）。
    pub fn script_is_empty(&self) -> bool {
        self.inner.lock().unwrap().script.is_empty()
    }
}

impl LlmClient for FakeAgent {
    fn complete(&self, request: LlmRequest) -> BoxFuture<'static, Result<AgentResponse>> {
        let agent = self.clone();
        Box::pin(async move {
            // 注意：MutexGuard 不跨 await（保持 future 为 Send）
            let step = {
                let mut inner = agent.inner.lock().unwrap();
                inner.calls.push((request.stage, request.node));
                inner.prompt_tokens += 10;
                inner.completion_tokens += 5;
                inner.script.pop(request.stage, request.node)
            };

            match step {
                Some(Step::Tool { name, arguments }) => Ok(AgentResponse {
                    content: None,
                    tool_calls: vec![tool_call(name, arguments)],
                    prompt_tokens: 10,
                    completion_tokens: 5,
                }),
                Some(Step::Submit(value)) => Ok(AgentResponse {
                    content: None,
                    tool_calls: vec![tool_call("submit_metadata".into(), value)],
                    prompt_tokens: 10,
                    completion_tokens: 5,
                }),
                Some(Step::Text(text)) => Ok(AgentResponse {
                    content: Some(text),
                    tool_calls: Vec::new(),
                    prompt_tokens: 10,
                    completion_tokens: 5,
                }),
                // 不返回：由节点级超时包装终止（决策 64 / 148 ④）
                Some(Step::Stall) => {
                    std::future::pending::<agentpipeline_core::Result<AgentResponse>>().await
                }
                // 脚本耗尽：返回无 tool_call 的收尾响应，agent loop 自然结束
                None => Ok(AgentResponse {
                    content: Some("（脚本已结束）".to_string()),
                    tool_calls: Vec::new(),
                    prompt_tokens: 10,
                    completion_tokens: 5,
                }),
            }
        })
    }
}

fn tool_call(name: String, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        id: ulid::Ulid::new().to_string(),
        name,
        arguments: arguments.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentpipeline_core::types::ValidateInputMetadata;

    fn request(stage: Stage, node: Node) -> LlmRequest {
        LlmRequest {
            stage,
            node,
            attempt: 1,
            system_prompt: "sys".into(),
            user_prompt: "user".into(),
            messages: vec![],
            tools: vec![],
        }
    }

    #[tokio::test]
    async fn steps_are_played_in_order_per_node() {
        let mut script = Script::new();
        script
            .for_node(Stage::ArchitectDesign, Node::Execute)
            .write_file("design.md", "# 设计")
            .submit(&ValidateInputMetadata {
                readiness: true,
                blockers: vec![],
            });
        // 另一个节点有独立队列
        script
            .for_node(Stage::DevelopDesign, Node::Execute)
            .text("独立");

        let agent = FakeAgent::new(script);
        let r1 = agent
            .complete(request(Stage::ArchitectDesign, Node::Execute))
            .await
            .unwrap();
        assert_eq!(r1.tool_calls[0].name, "write_file");
        let r2 = agent
            .complete(request(Stage::ArchitectDesign, Node::Execute))
            .await
            .unwrap();
        assert_eq!(r2.tool_calls[0].name, "submit_metadata");
        let r3 = agent
            .complete(request(Stage::DevelopDesign, Node::Execute))
            .await
            .unwrap();
        assert_eq!(r3.content.as_deref(), Some("独立"));

        assert_eq!(agent.calls_for(Stage::ArchitectDesign, Node::Execute), 2);
        assert_eq!(agent.total_calls(), 3);
        assert!(agent.script_is_empty());
    }

    #[tokio::test]
    async fn typed_submit_uses_serde_struct_fields() {
        let mut script = Script::new();
        script
            .for_node(Stage::ArchitectDesign, Node::ValidateInput)
            .submit(&agentpipeline_core::types::ArchitectExecuteMetadata {
                readiness: false,
                blockers: vec!["缺少技术约束".into()],
                ..Default::default()
            });
        let agent = FakeAgent::new(script);
        let resp = agent
            .complete(request(Stage::ArchitectDesign, Node::ValidateInput))
            .await
            .unwrap();
        let args: serde_json::Value = serde_json::from_str(&resp.tool_calls[0].arguments).unwrap();
        assert_eq!(args["readiness"], false);
        assert_eq!(args["blockers"][0], "缺少技术约束");
    }

    #[tokio::test]
    async fn exhausted_script_yields_terminal_response() {
        let agent = FakeAgent::new(Script::new());
        let resp = agent
            .complete(request(Stage::Develop, Node::Execute))
            .await
            .unwrap();
        assert!(resp.tool_calls.is_empty());
        assert!(resp.content.unwrap().contains("脚本已结束"));
    }

    #[tokio::test]
    async fn stall_never_resolves() {
        let mut script = Script::new();
        script.for_node(Stage::Develop, Node::Execute).stall();
        let agent = FakeAgent::new(script);
        let fut = agent.complete(request(Stage::Develop, Node::Execute));
        let timed = tokio::time::timeout(std::time::Duration::from_millis(50), fut).await;
        assert!(timed.is_err(), "Stall 步应当永不返回");
    }

    #[test]
    fn declared_nodes_lists_all_scripts() {
        let mut script = Script::new();
        script.for_node(Stage::Develop, Node::Execute).text("a");
        script.for_node(Stage::Test, Node::Execute).text("b");
        assert_eq!(script.declared_nodes().len(), 2);
        assert_eq!(script.remaining(Stage::Develop, Node::Execute), 1);
    }
}
