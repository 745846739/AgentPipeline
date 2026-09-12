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
    pub async fn from_script(script: Script) -> Self {
        let script = Arc::new(Mutex::new(script));
        let responder: Responder = Arc::new(move |request: &RecordedRequest| {
            let system = request_system_prompt(&request.body).unwrap_or_default();
            let body = if let Some(agent_type) = pseudo_agent_type(&system) {
                let step = script.lock().unwrap().take_next_pseudo(agent_type);
                render_step(step)
            } else if let Some((stage, node)) = node_for_system(&system) {
                let step = script.lock().unwrap().take_next(stage, node);
                render_step(step)
            } else {
                // 认不出的请求（如未脚本化节点）：当作脚本耗尽，干净收尾。
                sse_text("（脚本已结束）")
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
const PSEUDO_MARKERS: [(&str, &str); 2] = [
    ("你是设计语义冲突比对 agent", "pseudo:conflict_check"),
    ("你是独立复核 agent", "pseudo:validator_cross_check"),
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
