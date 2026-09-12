//! 极简 mock LLM HTTP server（票 13 / 票 17 的验收基建）。
//!
//! 手写 HTTP/1.1（tokio TcpListener，无新依赖）：按路由前缀匹配脚本化响应，
//! 记录收到的请求（路径 / 头 / 体）供断言。每个连接只处理一个请求后关闭
//! （`connection: close`），足够 reqwest 客户端在测试里反复调用。

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

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

struct Shared {
    routes: Vec<MockRoute>,
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
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("绑定 mock 端口");
        let url = format!("http://{}", listener.local_addr().unwrap());
        let shared = Arc::new(Shared {
            routes,
            requests: Mutex::new(Vec::new()),
        });
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
    let route = shared
        .routes
        .iter()
        .find(|r| request.path.starts_with(&r.path))
        .cloned();
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
