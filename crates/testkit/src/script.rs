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
    /// LLM 调用当场失败（决策 211① / 票 01）：适配器层的可归因失败——401 / 429 /
    /// 网络不可达 / 窗口超限。与 [`Step::Stall`] 并列成一对照：一个「永不返回」，
    /// 一个「立刻报错」；两者都是**失败现场**，都得留下证据。
    Fail {
        kind: String,
        message: String,
        raw: String,
    },
    /// 输出退化注入（决策 280）：LLM 调用当场返回 `Degenerated` 错误——
    /// 流式护栏判废本轮。生产里护栏在 `ProductionLlm` 的流中途触发；脚本替身
    /// 在同一接缝（`complete` 的返回值）给出同型错误，供 agent loop 整环验证。
    Degenerate { message: String },
}

/// 按 `(stage, node)` 组织的脚本；伪阶段按 `agent_type`（`pseudo:*`）单独排队（testing.md §3.2 ⑥）。
#[derive(Debug, Default, Clone)]
pub struct Script {
    steps: HashMap<(Stage, Node), VecDeque<Step>>,
    pseudo_steps: HashMap<String, VecDeque<Step>>,
    /// 子代理的脚步（票 08）：子代理复用父节点的 `(stage, node)`，若与父节点共用队列，
    /// 父节点的一步会被子代理悄悄吃掉，测试就无法表达「父派子代理 → 子代理干活 →
    /// 摘要回灌父」这个序列。故单独排队。
    subagent_steps: VecDeque<Step>,
    /// 值班长的脚步（决策 182，票 01）：它既不是阶段、也不是既有伪阶段之一，
    /// 却同样走 `(Stage::Init, Node::Execute)` 这个占位坐标——共用队列会被
    /// Init/Execute 的脚本悄悄吃掉。单独排队，理由与子代理那一路完全相同。
    foreman_steps: VecDeque<Step>,
    /// 工具失败注入规则（testing.md §3.2 ② / G13）：`(stage, node, tool)` 的
    /// 第 `n` 次调用（1-based）换成必然失败的参数，其余同名调用真实执行。
    fail_rules: HashMap<(Stage, Node, String), u32>,
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

    /// 为某个伪阶段（`agent_type`，如 `pseudo:conflict_check`）追加步骤。
    pub fn push_pseudo(&mut self, agent_type: &str, step: Step) -> &mut Self {
        self.pseudo_steps
            .entry(agent_type.to_string())
            .or_default()
            .push_back(step);
        self
    }

    /// 为子代理追加步骤（票 08）。
    ///
    /// 子代理与父节点共用 `(stage, node)` 坐标，所以不能靠坐标区分——单独排队。
    /// 这也让「父派子代理 → 子代理跑 N 步 → 摘要回灌父」可以被写成线性脚本。
    pub fn push_subagent(&mut self, step: Step) -> &mut Self {
        self.subagent_steps.push_back(step);
        self
    }

    /// 为值班长追加步骤（决策 182，票 01）。
    pub fn push_foreman(&mut self, step: Step) -> &mut Self {
        self.foreman_steps.push_back(step);
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

    /// 伪阶段的链式脚本构建入口。
    pub fn for_pseudo<'a>(&'a mut self, agent_type: &'a str) -> PseudoScript<'a> {
        PseudoScript {
            script: self,
            agent_type,
        }
    }

    /// 值班长的链式脚本构建入口（决策 182，票 01）。
    pub fn for_foreman(&mut self) -> ForemanScript<'_> {
        ForemanScript { script: self }
    }

    /// 工具失败注入（testing.md §3.2 ② / G13 / 决策 33）：`(stage, node)` 内某个工具的
    /// **第 `n` 次**调用（1-based）被替换为必然失败的参数，其余调用照常真实执行。
    ///
    /// 失败形态：把参数换成 JSON 字符串，工具层解析参数时缺必填字段 → `Err`，
    /// 由 agent loop 按 `tool_retry_max` 分层计数（单次失败不触发节点重试）。
    pub fn fail_tool_n(&mut self, stage: Stage, node: Node, tool: &str, n: u32) -> &mut Self {
        assert!(n >= 1, "fail_tool_n 的 n 从 1 开始");
        self.fail_rules.insert((stage, node, tool.to_string()), n);
        self
    }

    /// 命中规则的调用序号（`None` = 不注入失败）。
    pub fn fail_tool_at(&self, stage: Stage, node: Node, tool: &str) -> Option<u32> {
        self.fail_rules
            .get(&(stage, node, tool.to_string()))
            .copied()
    }

    /// 剩余步骤数。
    pub fn remaining(&self, stage: Stage, node: Node) -> usize {
        self.steps
            .get(&(stage, node))
            .map(VecDeque::len)
            .unwrap_or(0)
    }

    /// 伪阶段剩余步骤数。
    pub fn remaining_pseudo(&self, agent_type: &str) -> usize {
        self.pseudo_steps
            .get(agent_type)
            .map(VecDeque::len)
            .unwrap_or(0)
    }

    /// 值班长剩余步骤数（决策 182）。
    pub fn remaining_foreman(&self) -> usize {
        self.foreman_steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.values().all(VecDeque::is_empty)
            && self.pseudo_steps.values().all(VecDeque::is_empty)
            && self.subagent_steps.is_empty()
            && self.foreman_steps.is_empty()
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

    fn pop_pseudo(&mut self, agent_type: &str) -> Option<Step> {
        self.pseudo_steps
            .get_mut(agent_type)
            .and_then(|q| q.pop_front())
    }

    fn pop_subagent(&mut self) -> Option<Step> {
        self.subagent_steps.pop_front()
    }

    fn pop_foreman(&mut self) -> Option<Step> {
        self.foreman_steps.pop_front()
    }

    /// 取出并消费某个 `(stage, node)` 的下一步——供 mock HTTP 脚本服务器复用
    /// （票 17：真实二进制冒烟用 `Script` 驱动 `mock_llm`）。
    pub fn take_next(&mut self, stage: Stage, node: Node) -> Option<Step> {
        self.pop(stage, node)
    }

    /// 取出并消费某个伪阶段（`agent_type` 形如 `pseudo:*`）的下一步。
    pub fn take_next_pseudo(&mut self, agent_type: &str) -> Option<Step> {
        self.pop_pseudo(agent_type)
    }

    /// 取出并消费值班长的下一步（决策 182）。
    pub fn take_next_foreman(&mut self) -> Option<Step> {
        self.pop_foreman()
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

    /// 声明式工具失败注入（§3.2 ② / G13）：本节点该工具第 `n` 次调用失败。
    ///
    /// 与 [`NodeScript::failing_tool`] 不同，这里不改变脚本队列——真实调用照常声明，
    /// 由 [`FakeAgent`] 在运行时把第 `n` 次换成必然失败的参数。
    pub fn fail_tool_n(self, tool: &str, n: u32) -> Self {
        self.script.fail_tool_n(self.stage, self.node, tool, n);
        self
    }

    /// 超长工具结果注入（§3.2 ⑤）：追加一个真实执行的 `run_command`——`seq 1 <lines>`。
    ///
    /// 行数 > 150 触发 L1 裁剪（`trim_run_command` 前 50 + 后 100 行）；
    /// 字符数超过 `offload_threshold_tokens × 4`（默认 4000 token ≈ 16000 字符）
    /// 触发 L2 卸载（写 `context_dir` 并替换为预览，决策 110）。
    pub fn long_tool_result(self, lines: u64) -> Self {
        self.run_command(&format!("seq 1 {lines}"))
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

    /// LLM 调用当场失败（票 01）：`kind` 用生产的稳定类别标识
    /// （`llm_auth` / `llm_network` / `llm_context_window` …）。
    pub fn fail_llm(self, kind: &str, message: &str, raw: &str) -> Self {
        self.push(Step::Fail {
            kind: kind.to_string(),
            message: message.to_string(),
            raw: raw.to_string(),
        })
    }

    /// 输出退化注入（决策 280）：本轮被护栏判废，agent loop 应按决策 278 续接重试。
    pub fn degenerate(self, message: &str) -> Self {
        self.push(Step::Degenerate {
            message: message.to_string(),
        })
    }

    pub fn push(self, step: Step) -> Self {
        self.script.push(self.stage, self.node, step);
        self
    }
}

/// 单个伪阶段的脚本构建器（`agent_type` 形如 `pseudo:conflict_check`）。
pub struct PseudoScript<'a> {
    script: &'a mut Script,
    agent_type: &'a str,
}

impl PseudoScript<'_> {
    /// 类型化 `submit_metadata`（result 结构体由 serde 序列化）。
    pub fn submit<T: Serialize>(self, value: &T) -> Self {
        let json = serde_json::to_value(value).expect("元数据可序列化");
        self.push(Step::Submit(json))
    }

    pub fn submit_raw(self, value: serde_json::Value) -> Self {
        self.push(Step::Submit(value))
    }

    /// 纯文本回复（伪阶段也可走文本 JSON 解析）。
    pub fn text(self, text: &str) -> Self {
        self.push(Step::Text(text.to_string()))
    }

    pub fn stall(self) -> Self {
        self.push(Step::Stall)
    }

    pub fn push(self, step: Step) -> Self {
        self.script.push_pseudo(self.agent_type, step);
        self
    }
}

/// 值班长的脚本构建器（决策 182，票 01）。
///
/// 与 [`PseudoScript`] 分开而不是复用 `for_pseudo`：值班长的请求 `run.agent_type` 是
/// `"foreman"`，**不带 `pseudo:` 前缀**——它是配置键，不是伪阶段。让它继续按
/// `pseudo:` 路由会把「值班长」与「流水线内的辅助调用」在测试基建里混成一类，
/// 而这恰恰是决策 182 明确分开的一件事。
pub struct ForemanScript<'a> {
    script: &'a mut Script,
}

impl ForemanScript<'_> {
    /// 纯文本回话。值班长**不用** `submit_metadata` 收口（决策 182④），
    /// 所以这里只需要文本与工具两种步。
    pub fn text(self, text: &str) -> Self {
        self.push(Step::Text(text.to_string()))
    }

    /// 发起一次只读工具调用（工具**真实执行**）。
    pub fn tool(self, name: &str, arguments: serde_json::Value) -> Self {
        self.push(Step::Tool {
            name: name.to_string(),
            arguments,
        })
    }

    /// 查某个任务台账的便捷写法。
    pub fn read_task(self, task_id: &str) -> Self {
        self.tool("read_task", serde_json::json!({ "task_id": task_id }))
    }

    /// 查某个任务诊断包的便捷写法（决策 211③，票 03）。
    pub fn read_diagnosis(self, task_id: &str) -> Self {
        self.tool("read_diagnosis", serde_json::json!({ "task_id": task_id }))
    }

    /// 查某次运行回执的便捷写法（`run_id` 省略即「最近一次」）。
    pub fn read_conversation(self, task_id: &str, run_id: Option<i64>) -> Self {
        let mut args = serde_json::json!({ "task_id": task_id });
        if let Some(id) = run_id {
            args["run_id"] = serde_json::json!(id);
        }
        self.tool("read_conversation", args)
    }

    pub fn stall(self) -> Self {
        self.push(Step::Stall)
    }

    pub fn push(self, step: Step) -> Self {
        self.script.push_foreman(step);
        self
    }
}

struct Inner {
    script: Script,
    calls: Vec<(Stage, Node)>,
    /// 每次调用的请求快照（按发生顺序；断言 prompt 组装 / 配置透传用）。
    requests: Vec<LlmRequest>,
    /// 各 `(stage, node, tool)` 已发出的工具调用计数（`fail_tool_n` 判定用）。
    tool_calls: HashMap<(Stage, Node, String), u32>,
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
                requests: Vec::new(),
                tool_calls: HashMap::new(),
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

    /// 全部请求快照（按发生顺序）。
    pub fn request_log(&self) -> Vec<LlmRequest> {
        self.inner.lock().unwrap().requests.clone()
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
            let (stage, node) = (request.stage, request.node);
            // 伪阶段请求的 `run.agent_type` 形如 `pseudo:*`，按它路由独立脚本队列
            let pseudo_type = request
                .run
                .as_ref()
                .map(|r| r.agent_type.clone())
                .filter(|a| a.starts_with("pseudo:"));
            // 子代理请求（票 08）复用父节点的 `(stage, node)`，同样必须走独立队列，
            // 否则它会吃掉父节点的一步。
            let is_subagent = request
                .run
                .as_ref()
                .is_some_and(|r| r.agent_type == "subagent");
            // 值班长请求（决策 182）借 `(Stage::Init, Node::Execute)` 作占位坐标，
            // 与 Init 的脚本撞坐标——同样必须走独立队列。
            let is_foreman = request.run.as_ref().is_some_and(|r| {
                r.agent_type == agentpipeline_core::pipeline::foreman::FOREMAN_AGENT_TYPE
            });
            let step = {
                let mut inner = agent.inner.lock().unwrap();
                inner.calls.push((stage, node));
                inner.requests.push(request);
                inner.prompt_tokens += 10;
                inner.completion_tokens += 5;
                let popped = match (&pseudo_type, is_subagent, is_foreman) {
                    (Some(agent_type), _, _) => inner.script.pop_pseudo(agent_type),
                    (None, true, _) => inner.script.pop_subagent(),
                    (None, false, true) => inner.script.pop_foreman(),
                    (None, false, false) => inner.script.pop(stage, node),
                };
                match popped {
                    // fail_tool_n：把该工具第 n 次调用换成必然失败的参数（其余真实执行）
                    Some(Step::Tool { name, arguments })
                        if pseudo_type.is_none() && !is_subagent && !is_foreman =>
                    {
                        let key = (stage, node, name.clone());
                        let next = inner.tool_calls.get(&key).copied().unwrap_or(0) + 1;
                        inner.tool_calls.insert(key, next);
                        let fail = inner.script.fail_tool_at(stage, node, &name) == Some(next);
                        let arguments = if fail {
                            failing_arguments(arguments)
                        } else {
                            arguments
                        };
                        Some(Step::Tool { name, arguments })
                    }
                    other => other,
                }
            };

            match step {
                Some(Step::Tool { name, arguments }) => Ok(AgentResponse {
                    content: None,
                    tool_calls: vec![tool_call(name, arguments)],
                    prompt_tokens: 10,
                    completion_tokens: 5,
                    ..Default::default()
                }),
                Some(Step::Submit(value)) => Ok(AgentResponse {
                    content: None,
                    tool_calls: vec![tool_call("submit_metadata".into(), value)],
                    prompt_tokens: 10,
                    completion_tokens: 5,
                    ..Default::default()
                }),
                Some(Step::Text(text)) => Ok(AgentResponse {
                    content: Some(text),
                    tool_calls: Vec::new(),
                    prompt_tokens: 10,
                    completion_tokens: 5,
                    ..Default::default()
                }),
                // 不返回：由节点级超时包装终止（决策 64 / 148 ④）
                Some(Step::Stall) => {
                    std::future::pending::<agentpipeline_core::Result<AgentResponse>>().await
                }
                // 当场报错：适配器层的可归因失败（票 01 的失败落库路径）
                Some(Step::Fail { kind, message, raw }) => {
                    Err(agentpipeline_core::Error::LlmClassified { kind, message, raw })
                }
                // 输出退化（决策 280）：护栏判废本轮，agent loop 按决策 278 续接重试
                Some(Step::Degenerate { message }) => {
                    Err(agentpipeline_core::Error::Degenerated(message))
                }
                // 脚本耗尽：返回无 tool_call 的收尾响应，agent loop 自然结束
                None => Ok(AgentResponse {
                    content: Some("（脚本已结束）".to_string()),
                    tool_calls: Vec::new(),
                    prompt_tokens: 10,
                    completion_tokens: 5,
                    ..Default::default()
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

/// 必然失败的参数（§3.2 ②）：换成 JSON 字符串，工具层解析后缺一切必填字段 → `Err`。
fn failing_arguments(_original: serde_json::Value) -> serde_json::Value {
    serde_json::json!("__agentpipeline__fail_tool_n__")
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
            temperature: None,
            max_tokens: None,
            provider_id: None,
            run: None,
            idle_timeout_sec: None,
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

    #[tokio::test]
    async fn fail_tool_n_fails_only_the_nth_call_of_that_tool() {
        // §3.2 ② / G13：第 2 次 run_command 调用被换成必然失败形态，1 / 3 次真实执行
        let mut script = Script::new();
        script
            .for_node(Stage::Develop, Node::Execute)
            .run_command("echo one")
            .run_command("echo two")
            .run_command("echo three")
            .fail_tool_n("run_command", 2);
        let agent = FakeAgent::new(script);

        let mut calls = Vec::new();
        for _ in 0..3 {
            let resp = agent
                .complete(request(Stage::Develop, Node::Execute))
                .await
                .unwrap();
            assert_eq!(resp.tool_calls[0].name, "run_command");
            calls.push(
                serde_json::from_str::<serde_json::Value>(&resp.tool_calls[0].arguments).unwrap(),
            );
        }
        assert_eq!(calls[0]["command"], "echo one");
        assert_eq!(calls[2]["command"], "echo three");
        assert!(
            calls[1].is_string(),
            "第 2 次应是失败注入形态（JSON 字符串缺 command）：{:?}",
            calls[1]
        );

        // 规则只在命中的那次生效，后续调用不再注入
        let fourth = agent
            .complete(request(Stage::Develop, Node::Execute))
            .await
            .unwrap();
        assert!(fourth.tool_calls.is_empty(), "队列已耗尽 → 收尾响应");
    }

    #[tokio::test]
    async fn fail_tool_n_is_scoped_to_stage_node_and_tool() {
        let mut script = Script::new();
        // 规则只挂在 Develop.Execute.run_command 上
        script.fail_tool_n(Stage::Develop, Node::Execute, "run_command", 1);
        script
            .for_node(Stage::Develop, Node::Execute)
            .run_command("echo a");
        script
            .for_node(Stage::Test, Node::Execute)
            .run_command("echo b");
        let agent = FakeAgent::new(script);

        // 不同节点：不注入
        let test = agent
            .complete(request(Stage::Test, Node::Execute))
            .await
            .unwrap();
        let test_args: serde_json::Value =
            serde_json::from_str(&test.tool_calls[0].arguments).unwrap();
        assert_eq!(test_args["command"], "echo b");

        // 命中节点：注入
        let dev = agent
            .complete(request(Stage::Develop, Node::Execute))
            .await
            .unwrap();
        let dev_args: serde_json::Value =
            serde_json::from_str(&dev.tool_calls[0].arguments).unwrap();
        assert!(dev_args.is_string());
    }

    #[tokio::test]
    async fn long_tool_result_emits_real_seq_command() {
        // §3.2 ⑤：超长工具结果注入 = 真实执行的 seq 命令
        let mut script = Script::new();
        script
            .for_node(Stage::Develop, Node::Execute)
            .long_tool_result(300);
        let agent = FakeAgent::new(script);
        let resp = agent
            .complete(request(Stage::Develop, Node::Execute))
            .await
            .unwrap();
        assert_eq!(resp.tool_calls[0].name, "run_command");
        let args: serde_json::Value = serde_json::from_str(&resp.tool_calls[0].arguments).unwrap();
        assert_eq!(args["command"], "seq 1 300");
    }

    #[test]
    fn declared_nodes_lists_all_scripts() {
        let mut script = Script::new();
        script.for_node(Stage::Develop, Node::Execute).text("a");
        script.for_node(Stage::Test, Node::Execute).text("b");
        assert_eq!(script.declared_nodes().len(), 2);
        assert_eq!(script.remaining(Stage::Develop, Node::Execute), 1);
    }

    #[tokio::test]
    async fn pseudo_steps_route_by_agent_type() {
        // testing.md §3.2 ⑥：伪阶段脚本按 `run.agent_type = pseudo:*` 独立路由
        let mut script = Script::new();
        script
            .for_pseudo("pseudo:conflict_check")
            .submit_raw(serde_json::json!({"duplicate_risk": "high"}));
        script
            .for_node(Stage::ArchitectDesign, Node::Execute)
            .text("主节点");
        let agent = FakeAgent::new(script);

        let mut pseudo = request(Stage::ArchitectDesign, Node::Execute);
        pseudo.run = Some(agentpipeline_core::agent::client::RunContext {
            task_id: "t".into(),
            branch: "main".into(),
            run_id: 1,
            agent_type: "pseudo:conflict_check".into(),
            session_id: String::new(),
        });
        let resp = agent.complete(pseudo).await.unwrap();
        assert_eq!(resp.tool_calls[0].name, "submit_metadata");
        let args: serde_json::Value = serde_json::from_str(&resp.tool_calls[0].arguments).unwrap();
        assert_eq!(args["duplicate_risk"], "high");

        // 主节点请求仍走自己的队列，未被伪阶段脚本吃掉
        let main = agent
            .complete(request(Stage::ArchitectDesign, Node::Execute))
            .await
            .unwrap();
        assert_eq!(main.content.as_deref(), Some("主节点"));
        assert!(agent.script_is_empty());
    }
}
