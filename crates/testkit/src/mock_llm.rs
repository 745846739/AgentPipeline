//! 极简 mock LLM HTTP server（票 13 / 票 17 的验收基建）。
//!
//! 手写 HTTP/1.1（tokio TcpListener，无新依赖）：按路由前缀匹配脚本化响应，
//! 记录收到的请求（路径 / 头 / 体）供断言。每个连接只处理一个请求后关闭
//! （`connection: close`），足够 reqwest 客户端在测试里反复调用。

use std::sync::{Arc, Mutex};

use agentpipeline_core::agent::templates::system_template;
use agentpipeline_core::types::{Node, Stage};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::script::{Script, Step};

/// 一条脚本化响应。
#[derive(Clone)]
pub struct MockRoute {
    /// 路径前缀（如 `/chat/completions`）。
    pub path: String,
    pub status: u16,
    pub content_type: String,
    pub body: String,
}

impl MockRoute {
    pub fn sse(path: &str, body: impl Into<String>) -> Self {
        MockRoute {
            path: path.to_string(),
            status: 200,
            content_type: "text/event-stream".into(),
            body: body.into(),
        }
    }

    pub fn json(path: &str, status: u16, body: impl Into<String>) -> Self {
        MockRoute {
            path: path.to_string(),
            status,
            content_type: "application/json".into(),
            body: body.into(),
        }
    }
}

/// 记录到的请求。
#[derive(Debug, Clone)]
pub struct RecordedRequest {
    pub method: String,
    pub path: String,
    /// (小写头名, 值)。
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl RecordedRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// 动态响应器：按请求现场决定响应（票 17 的真实二进制冒烟用 `Script` 驱动）。
pub type Responder = Arc<dyn Fn(&RecordedRequest) -> MockRoute + Send + Sync>;

struct Shared {
    routes: Vec<MockRoute>,
    /// 有响应器时优先于静态路由（`from_script` 用）。
    responder: Option<Responder>,
    requests: Mutex<Vec<RecordedRequest>>,
}

/// 运行中的 mock server。`url` 即 provider 的 base_url。
pub struct MockLlm {
    pub url: String,
    shared: Arc<Shared>,
    handle: tokio::task::JoinHandle<()>,
}

impl MockLlm {
    /// 启动：监听 127.0.0.1 随机端口，路由按声明顺序取首个前缀命中，未命中返回 404。
    pub async fn start(routes: Vec<MockRoute>) -> Self {
        let shared = Arc::new(Shared {
            routes,
            responder: None,
            requests: Mutex::new(Vec::new()),
        });
        Self::serve(shared).await
    }

    /// 动态响应器版本：每个请求现场调用 `responder`（优先级高于静态路由）。
    pub async fn start_responder(responder: Responder) -> Self {
        let shared = Arc::new(Shared {
            routes: Vec::new(),
            responder: Some(responder),
            requests: Mutex::new(Vec::new()),
        });
        Self::serve(shared).await
    }

    /// 用 [`Script`] 驱动一个 OpenAI 兼容的流式 mock server（票 17）。
    ///
    /// 用 system prompt 里的节点 persona（内嵌 §10.3 模板 / 伪阶段 persona）
    /// 反查 `(stage, node)` 或 `pseudo:*`，再消费脚本下一步——与 FakeAgent
    /// 同一份 `Script`，因此真实二进制走的是与 L2/L4 相同的场景脚本。
    ///
    /// `Submit` 步之后紧跟一次**无工具调用**的收尾文本（主流程票 08）：真实模型
    /// 提交 `submit_metadata` 后不会再发工具调用，agent loop 随之收束；若 mock
    /// 继续吐队列里的下一步，节点一次 run 会把整个队列吃完，重启重放（票 08）
    /// 将无步可用。同一节点的重放因此消费第二份脚本即可。
    pub async fn from_script(script: Script) -> Self {
        struct ScriptState {
            script: Script,
            /// 刚为该节点/伪阶段吐过 Submit 步——下一请求回收尾文本。
            just_submitted_nodes: std::collections::HashSet<(Stage, Node)>,
            just_submitted_pseudo: std::collections::HashSet<String>,
        }
        let state = Arc::new(Mutex::new(ScriptState {
            script,
            just_submitted_nodes: Default::default(),
            just_submitted_pseudo: Default::default(),
        }));
        let responder: Responder = Arc::new(move |request: &RecordedRequest| {
            let system = request_system_prompt(&request.body).unwrap_or_default();
            let body = {
                let mut state = state.lock().unwrap();
                if let Some(agent_type) = pseudo_agent_type(&system) {
                    if state.just_submitted_pseudo.remove(agent_type) {
                        sse_text("（已提交元数据）")
                    } else {
                        let step = state.script.take_next_pseudo(agent_type);
                        if matches!(step, Some(Step::Submit(_))) {
                            state.just_submitted_pseudo.insert(agent_type.to_string());
                        }
                        render_step(step)
                    }
                } else if let Some((stage, node)) = node_for_system(&system) {
                    if state.just_submitted_nodes.remove(&(stage, node)) {
                        sse_text("（已提交元数据）")
                    } else {
                        let step = state.script.take_next(stage, node);
                        if matches!(step, Some(Step::Submit(_))) {
                            state.just_submitted_nodes.insert((stage, node));
                        }
                        render_step(step)
                    }
                } else {
                    // 认不出的请求（如未脚本化节点）：当作脚本耗尽，干净收尾。
                    sse_text("（脚本已结束）")
                }
            };
            MockRoute::sse("/", body)
        });
        Self::start_responder(responder).await
    }

    async fn serve(shared: Arc<Shared>) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("绑定 mock 端口");
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task_shared = shared.clone();
        let handle = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let shared = task_shared.clone();
                tokio::spawn(async move {
                    let _ = serve_once(stream, &shared).await;
                });
            }
        });
        MockLlm {
            url,
            shared,
            handle,
        }
    }

    pub async fn requests(&self) -> Vec<RecordedRequest> {
        self.shared.requests.lock().unwrap().clone()
    }

    pub async fn shutdown(self) {
        self.handle.abort();
    }
}

async fn serve_once(mut stream: TcpStream, shared: &Shared) -> std::io::Result<()> {
    let request = read_request(&mut stream).await?;
    let Some(request) = request else {
        return Ok(());
    };
    let route = match &shared.responder {
        Some(responder) => Some(responder(&request)),
        None => shared
            .routes
            .iter()
            .find(|r| request.path.starts_with(&r.path))
            .cloned(),
    };
    shared.requests.lock().unwrap().push(request);
    let Some(route) = route else {
        write_response(
            &mut stream,
            404,
            "application/json",
            r#"{"error":"no route"}"#,
        )
        .await?;
        return Ok(());
    };
    write_response(&mut stream, route.status, &route.content_type, &route.body).await
}

/// 12 个 agent 节点（与 §10.3 内嵌模板一一对应）。
const AGENT_NODES: [(Stage, Node); 12] = [
    (Stage::ArchitectDesign, Node::ValidateInput),
    (Stage::ArchitectDesign, Node::Execute),
    (Stage::ArchitectDesign, Node::ValidateOutput),
    (Stage::DevelopDesign, Node::ValidateInput),
    (Stage::DevelopDesign, Node::Execute),
    (Stage::DevelopDesign, Node::ValidateOutput),
    (Stage::TestDesign, Node::ValidateInput),
    (Stage::TestDesign, Node::Execute),
    (Stage::TestDesign, Node::ValidateOutput),
    (Stage::Develop, Node::Execute),
    (Stage::Review, Node::Execute),
    (Stage::Test, Node::Execute),
];

/// 伪阶段 persona 首句 → `agent_type`（票 16 的伪阶段请求据此路由 `Script` 的伪阶段队列）。
///
/// **与 Node 侧 `frontend/e2e/harness.ts::PERSONA_ROUTES` 的伪阶段三条一一对应**
/// （票 e2e-mock/01）：两侧都靠子串匹配 system prompt，一处改了模板而另一处没同步的表现是
/// Node mock 静默回「脚本已结束」文本、任务卡到超时。
///
/// **已知限制（票面要求写明，不得静默假设）**：匹配的是**内嵌 persona**
/// （`PseudoStage::embedded_persona`）。伪阶段配置若写了 `persona_path`，system prompt 里就是
/// 用户那份文件的内容，这个 marker 便不在其中——该伪阶段在脚本化 mock 下**因此无法路由**
/// （`executor.rs` 的 pseudo 分支：`persona_path` 覆盖内嵌 persona，决策 7 / 87）。
/// `persona_append` 不受影响（追加，首句仍在）。真要用 `persona_path` 覆盖
/// `project_analysis`，该用例得走 [`MockLlm::start`] 的静态路由。
const PSEUDO_MARKERS: [(&str, &str); 3] = [
    ("你是设计语义冲突比对 agent", "pseudo:conflict_check"),
    ("你是独立复核 agent", "pseudo:validator_cross_check"),
    ("你是项目分析 agent", "pseudo:project_analysis"),
];

/// 取出 OpenAI 兼容请求体里第一条 system message 的内容。
fn request_system_prompt(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    value
        .get("messages")?
        .get(0)?
        .get("content")?
        .as_str()
        .map(String::from)
}

/// 用 system prompt 里的节点 persona 首句反查 `(stage, node)`。
fn node_for_system(system: &str) -> Option<(Stage, Node)> {
    AGENT_NODES.iter().copied().find(|(stage, node)| {
        let first = system_template(*stage, *node)
            .lines()
            .next()
            .unwrap_or_default();
        !first.is_empty() && system.contains(first)
    })
}

/// 用 system prompt 里的伪阶段 persona 首句反查 `agent_type`。
fn pseudo_agent_type(system: &str) -> Option<&'static str> {
    PSEUDO_MARKERS
        .iter()
        .find(|(marker, _)| system.contains(marker))
        .map(|(_, agent_type)| *agent_type)
}

/// 一步脚本 → 一段 OpenAI 兼容 SSE。
fn render_step(step: Option<Step>) -> String {
    match step {
        Some(Step::Tool { name, arguments }) => sse_tool(&name, &arguments.to_string()),
        Some(Step::Submit(value)) => sse_tool("submit_metadata", &value.to_string()),
        Some(Step::Text(text)) => sse_text(&text),
        // Stall：不写 [DONE]，连接关闭即流结束（与 FakeAgent 的「永不返回」近似）
        Some(Step::Stall) => String::new(),
        None => sse_text("（脚本已结束）"),
    }
}

fn sse_tool(name: &str, arguments: &str) -> String {
    let chunk = serde_json::json!({
        "choices": [{
            "index": 0,
            "delta": {
                "role": "assistant",
                "tool_calls": [{
                    "index": 0,
                    "id": format!("call_{}", ulid::Ulid::new()),
                    "type": "function",
                    "function": {"name": name, "arguments": arguments}
                }]
            },
            "finish_reason": "tool_calls"
        }]
    });
    sse_with_usage(&chunk)
}

fn sse_text(text: &str) -> String {
    let chunk = serde_json::json!({
        "choices": [{
            "index": 0,
            "delta": {"role": "assistant", "content": text},
            "finish_reason": "stop"
        }]
    });
    sse_with_usage(&chunk)
}

fn sse_with_usage(chunk: &serde_json::Value) -> String {
    let usage = serde_json::json!({"usage": {"prompt_tokens": 10, "completion_tokens": 5}});
    format!("data: {chunk}\n\ndata: {usage}\n\ndata: [DONE]\n\n")
}

async fn read_request(stream: &mut TcpStream) -> std::io::Result<Option<RecordedRequest>> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    // 读到头结束
    let header_end = loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find_header_end(&buf) {
            break pos;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let mut headers = Vec::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_lowercase(), v.trim().to_string()));
        }
    }
    let content_length: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body_bytes = buf[header_end + 4..].to_vec();
    while body_bytes.len() < content_length {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        body_bytes.extend_from_slice(&chunk[..n]);
    }
    body_bytes.truncate(content_length);
    Ok(Some(RecordedRequest {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body_bytes).to_string(),
    }))
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

async fn write_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        401 => "Unauthorized",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body.as_bytes()).await?;
    stream.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 把一段 SSE 文本拆成**可解析的字节契约**：逐事件的 JSON 载荷（`[DONE]` 留作字符串）。
    ///
    /// 帧层断言（`data: ` 前缀 / 空行分帧）在拆的过程中一并钉住——这就是 fixture 头注说的
    /// 「键序不是契约，帧与字段位置才是」。两处归一：
    ///
    /// - **工具调用 id** 由两侧现场生成（Ulid / 时间戳随机数），归一为 fixture 里的固定值；
    /// - **`function.arguments`** 是「由谁产出就按谁的键序」的不透明串（本侧与 Node 侧各自
    ///   `JSON.stringify` 调用方给的参数），下游按 JSON 解析它，故比较**解析后的对象**——
    ///   原始字符串的键序不是契约。
    fn frames(sse: &str) -> Vec<serde_json::Value> {
        assert!(sse.starts_with("data: "), "缺 `data: ` 前缀：{sse}");
        assert!(sse.ends_with("\n\n"), "结尾须是空行：{sse}");
        let mut out = Vec::new();
        for event in sse.split("\n\n").filter(|e| !e.is_empty()) {
            let payload = event
                .strip_prefix("data: ")
                .unwrap_or_else(|| panic!("事件缺 `data: ` 前缀：{event}"));
            let mut value: serde_json::Value = if payload == "[DONE]" {
                serde_json::Value::String(payload.to_string())
            } else {
                serde_json::from_str(payload).unwrap_or_else(|e| panic!("{event}：{e}"))
            };
            if let Some(calls) = value
                .pointer_mut("/choices/0/delta/tool_calls")
                .and_then(|c| c.as_array_mut())
            {
                for call in calls {
                    if call.get("id").is_some() {
                        call["id"] = serde_json::json!("call_fixture_1");
                    }
                    if let Some(args) = call
                        .pointer("/function/arguments")
                        .and_then(|a| a.as_str())
                        .and_then(|a| serde_json::from_str::<serde_json::Value>(a).ok())
                    {
                        call["function"]["arguments"] = args;
                    }
                }
            }
            out.push(value);
        }
        out
    }

    fn golden_fixture() -> serde_json::Value {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/e2e_mock_sse.json"
        )))
        .expect("fixture 必须是合法 JSON")
    }

    /// 跨语言 golden fixture 的**生产侧**那一半（票 e2e-mock/01）：
    /// `sse_tool` / `sse_text` 的产出必须与 `tests/fixtures/e2e_mock_sse.json` 一致。
    ///
    /// 另一半在消费侧（`crates/core/src/agent/providers/openai.rs` 的同名用例）与 Node 侧
    /// （`frontend/src/lib/e2e-mock-fixture.test.ts`）。三处任一漂移 → 该处变红。
    #[test]
    fn sse_helpers_match_the_shared_golden_fixture() {
        let fixture = golden_fixture();
        let cases = fixture["cases"].as_array().expect("cases 是数组");
        assert_eq!(cases.len(), 3, "三种步骤形态：tool call / submit / text");

        for case in cases {
            let name = case["name"].as_str().unwrap();
            let actual = match case["producer"].as_str().unwrap() {
                "sseTool" => sse_tool(
                    case["call"]["name"].as_str().unwrap(),
                    &case["call"]["arguments"].to_string(),
                ),
                "sseText" => sse_text(case["call"]["text"].as_str().unwrap()),
                other => panic!("{name}：不认识的 producer {other}"),
            };
            // 逐字段一致 + 帧一致（id 已归一）
            assert_eq!(
                frames(&actual),
                frames(case["sse"].as_str().unwrap()),
                "{name}：testkit mock 的产出与 fixture 不一致"
            );
        }

        // usage 常量同源：fixture 是唯一事实源——helper 里的字面量改了这里就红
        let usage = frames(&sse_text("x"))
            .into_iter()
            .find_map(|v| v.get("usage").cloned())
            .expect("usage 事件");
        assert_eq!(usage, fixture["usage"], "usage 字段须与 fixture 同源");
    }

    /// 三条伪阶段 marker 都能在**组装后的 system prompt** 里命中（票 e2e-mock/01 的
    /// 「实现时验证」项）。第三条（`pseudo:project_analysis`）是本次补的，此前 Rust mock
    /// 覆盖不到该伪阶段。
    ///
    /// 这里走的是内嵌 persona 那条路——`persona_path` 覆盖时路由失效的**已知限制**见
    /// [`PSEUDO_MARKERS`] 的文档。
    #[test]
    fn pseudo_markers_survive_prompt_assembly() {
        use agentpipeline_core::agent::prompts::build_system_prompt;
        use agentpipeline_core::pipeline::pseudo::PseudoStage;

        for stage in [
            PseudoStage::ConflictCheck,
            PseudoStage::ValidatorCrossCheck,
            PseudoStage::ProjectAnalysis,
        ] {
            let system = build_system_prompt("ctx", stage.embedded_persona(), "worktree：/wt", &[]);
            assert_eq!(
                pseudo_agent_type(&system),
                Some(stage.agent_type()),
                "{} 的内嵌 persona 过完组装须仍含 marker",
                stage.stage_key()
            );
        }
    }

    #[tokio::test]
    async fn serves_scripted_routes_and_records_requests() {
        let mock = MockLlm::start(vec![
            MockRoute::sse("/chat/completions", "data: [DONE]\n\n"),
            MockRoute::json("/v1/messages", 401, r#"{"error":"bad key"}"#),
        ])
        .await;

        // 命中 SSE 路由
        let mut stream = tokio::net::TcpStream::connect(mock.url.trim_start_matches("http://"))
            .await
            .unwrap();
        let body = r#"{"model":"m"}"#;
        stream
            .write_all(
                format!(
                    "POST /chat/completions HTTP/1.1\r\nhost: x\r\nauthorization: Bearer sk-x\r\ncontent-length: {}\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        let text = String::from_utf8_lossy(&response);
        assert!(text.starts_with("HTTP/1.1 200 OK"), "{text}");
        assert!(text.contains("data: [DONE]"));

        let requests = mock.requests().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/chat/completions");
        assert_eq!(requests[0].header("authorization"), Some("Bearer sk-x"));
        assert_eq!(requests[0].body, body);
        mock.shutdown().await;
    }
}
