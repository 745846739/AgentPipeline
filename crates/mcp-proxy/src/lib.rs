//! stdio MCP 薄代理的翻译核心（决策 427，票 talk-cli-engine 05）。
//!
//! 形状（决策 424② + 427）：每会话一个**无状态**子进程，把 MCP 工具调用翻译成对既有
//! foreman HTTP 面的调用——它自己不解释任何业务语义，校验面全部在端点（工具白名单 /
//! schema / engine=cli 双闸），崩了重启即可。`initialize` / `tools/list` / `tools/call`
//! 三段协议 + stdio 按行 JSON-RPC；未知方法回 -32601，通知不回话。
//!
//! **暴露集是镜像不是清单**（决策 427⑥）：工具名与参数 schema 从
//! [`agentpipeline_core::pipeline::foreman::FOREMAN_TOOL_SPECS`] 一比一生成——
//! 决策 247 的「三处同源」延伸到第四处（广告集 / 执行白名单 / 人格纪律段 / MCP 名单）。
//! 两个例外各有理由：`repair` 不进（它的执行分支要主进程修复链产出的 payload，
//! 代理造不出，票 05 票注）；提议列表是一根手写的工具（它映射的是既有 GET，不是
//! catalog 里的任何工具）。
//!
//! 文案纪律在本 crate 也适用：回给模型的错误串是「给人看的话」，不写决策号。

use std::sync::Arc;

use agentpipeline_core::pipeline::foreman::{
    CLI_PROPOSAL_TOOLS, CLI_READ_TOOLS, FOREMAN_TOOL_SPECS,
};
use futures::future::BoxFuture;
use serde_json::{json, Value};

/// MCP 服务器名：claude 侧看到的工具前缀是 `mcp__agentpipeline__<工具名>`。
pub const MCP_SERVER_NAME: &str = "agentpipeline";

/// 三个环境变量（决策 427⑤）：配对令牌经环境变量进代理（不进 argv——argv 会进
/// 进程列表）；session 绑定同样走环境变量，载荷随行上报。
pub const ENV_BASE_URL: &str = "AGENTPIPELINE_BASE_URL";
pub const ENV_TOKEN: &str = "AGENTPIPELINE_TOKEN";
pub const ENV_SESSION_ID: &str = "AGENTPIPELINE_SESSION_ID";

/// 提议创建类工具的必填参数（决策 427⑥）：「给人看的那句」。**只挂在写工具上**——
/// 读工具没有提议可给，强制它只会白烧 token（票 05 票注：对决策 427⑥ 字面的收窄）。
pub const SUMMARY_PARAM: &str = "summary";

/// 提议列表工具的 MCP 名（第 17 个，手写——它映射的是既有 GET，不是 catalog 工具）。
pub const PROPOSALS_LIST_TOOL: &str = "proposals";

/// 配对令牌的请求头（决策 182 的同一道闸，代理是它的第一个程序化客户端）。
pub const TOKEN_HEADER: &str = "x-agentpipeline-token";

/// 代理的运行上下文：环境变量读出来就不再变（无状态）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyCtx {
    pub base_url: String,
    pub token: String,
    pub session_id: String,
}

/// 一次翻译出来的 HTTP 调用（[`HttpTransport`] 的输入；测试用 Fake 断言它）。
#[derive(Debug, Clone, PartialEq)]
pub struct HttpCall {
    pub method: HttpMethod,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub body: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
}

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Value,
}

pub type TransportResult = std::result::Result<HttpResponse, String>;

/// HTTP 传输接缝：生产实现走 reqwest，测试用 Fake 断言翻译结果。
pub trait HttpTransport: Send + Sync {
    fn call(&self, req: HttpCall) -> BoxFuture<'static, TransportResult>;
}

/// 生产传输：把 [`HttpCall`] 发到 `base_url`。
pub struct ReqwestTransport {
    client: reqwest::Client,
    base_url: String,
}

impl ReqwestTransport {
    pub fn new(base_url: String) -> Self {
        ReqwestTransport {
            client: reqwest::Client::new(),
            base_url,
        }
    }
}

impl HttpTransport for ReqwestTransport {
    fn call(&self, req: HttpCall) -> BoxFuture<'static, TransportResult> {
        let client = self.client.clone();
        let mut url = format!("{}{}", self.base_url.trim_end_matches('/'), req.path);
        if !req.query.is_empty() {
            let qs: Vec<String> = req
                .query
                .iter()
                .map(|(k, v)| format!("{k}={}", urlencode(v)))
                .collect();
            url.push('?');
            url.push_str(&qs.join("&"));
        }
        Box::pin(async move {
            let mut request = match req.method {
                HttpMethod::Get => client.get(&url),
                HttpMethod::Post => client.post(&url),
            };
            for (name, value) in &req.headers {
                request = request.header(name, value);
            }
            if let Some(body) = req.body {
                request = request.json(&body);
            }
            let response = request
                .send()
                .await
                .map_err(|e| format!("连不上服务（{url}）：{e}"))?;
            let status = response.status().as_u16();
            let body: Value = response.json().await.unwrap_or(Value::Null);
            Ok(HttpResponse { status, body })
        })
    }
}

/// 最小百分号编码（查询串用；JSON 正文里只有 ASCII 可打印字符需要转）。
fn urlencode(raw: &str) -> String {
    raw.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// MCP 工具的完整定义（`tools/list` 的每一项）。
fn tool_def(name: &str, description: &str, schema: Value) -> Value {
    json!({ "name": name, "description": description, "inputSchema": schema })
}

/// `tools/list` 的全量定义（镜像 catalog + 手写的提议列表工具）。
///
/// 公开给测试：同源断言（名字 / schema 与 catalog 一致）钉在这里。
pub fn tool_defs() -> Vec<Value> {
    let mut defs = Vec::with_capacity(CLI_PROPOSAL_TOOLS.len() + CLI_READ_TOOLS.len() + 1);
    for spec in FOREMAN_TOOL_SPECS.iter() {
        let is_write = CLI_PROPOSAL_TOOLS.contains(&spec.name);
        let is_read = CLI_READ_TOOLS.contains(&spec.name);
        if !is_write && !is_read {
            continue;
        }
        let mut schema: Value = serde_json::from_str(spec.parameters)
            .expect("catalog 里的 schema 常量必须一直是合法 JSON");
        if is_write {
            let obj = schema.as_object_mut().expect("schema 顶层必须是对象");
            obj.insert(
                SUMMARY_PARAM.to_string(),
                json!({
                    "type": "string",
                    "description": "一句话说明（人按键之前唯一读的那句：要动什么、为什么）。\
                                    必须与参数本身相符——人看到的那行就是将要发生的事。"
                }),
            );
            let required = obj.entry("required").or_insert_with(|| json!([]));
            required
                .as_array_mut()
                .expect("required 必须是数组")
                .push(json!(SUMMARY_PARAM));
        }
        defs.push(tool_def(spec.name, spec.description, schema));
    }
    defs.push(tool_def(
        PROPOSALS_LIST_TOOL,
        "读你本班次**未决**的提议清单（等值班经理按键的那些）。\
         提过什么、按没按过，先查它再决定要不要再提——别把同一件事提两遍。",
        json!({"type": "object", "properties": {}}),
    ));
    defs
}

/// `tools/call` 的翻译：MCP 工具调用 → [`HttpCall`]。
///
/// 返回 `Err(给人看的话)` = 调用没发出去（白名单外 / 缺 summary）；发出去之后的
/// HTTP 错误也收进 `Err`——调用方把它包成 `isError` 结果回给模型。
pub async fn dispatch_tool(
    ctx: &ProxyCtx,
    transport: &dyn HttpTransport,
    name: &str,
    arguments: Value,
) -> std::result::Result<String, String> {
    let arguments = arguments.as_object().cloned().unwrap_or_default();
    let call = if CLI_PROPOSAL_TOOLS.contains(&name) {
        // 提议创建：summary 是必填的人读句，从参数里拆出来随载荷走。
        let summary = arguments
            .get(SUMMARY_PARAM)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                format!(
                    "提议缺「{SUMMARY_PARAM}」——那是人按键之前唯一读的那句，必须写清楚要动什么、为什么"
                )
            })?
            .to_string();
        let mut args = arguments.clone();
        args.remove(SUMMARY_PARAM);
        HttpCall {
            method: HttpMethod::Post,
            path: "/foreman/proposals".into(),
            query: vec![],
            headers: vec![(TOKEN_HEADER.into(), ctx.token.clone())],
            body: Some(json!({
                "session_id": ctx.session_id,
                "tool": name,
                "args": Value::Object(args),
                "summary": summary,
            })),
        }
    } else if name == PROPOSALS_LIST_TOOL {
        HttpCall {
            method: HttpMethod::Get,
            path: "/foreman/proposals".into(),
            query: vec![("session".into(), ctx.session_id.clone())],
            headers: vec![(TOKEN_HEADER.into(), ctx.token.clone())],
            body: None,
        }
    } else if CLI_READ_TOOLS.contains(&name) {
        HttpCall {
            method: HttpMethod::Get,
            path: format!("/foreman/read/{name}"),
            query: vec![
                ("session".into(), ctx.session_id.clone()),
                ("args".into(), Value::Object(arguments).to_string()),
            ],
            headers: vec![(TOKEN_HEADER.into(), ctx.token.clone())],
            body: None,
        }
    } else {
        return Err(format!("未知工具：{name}"));
    };
    let response = transport.call(call).await?;
    if (400..600).contains(&response.status) {
        let message = response
            .body
            .get("error")
            .and_then(|e| e.as_str())
            .unwrap_or("服务端拒绝了这个调用");
        return Err(format!("（HTTP {}）{message}", response.status));
    }
    Ok(response.body.to_string())
}

/// 一条 stdin 消息的处理（[`handle_message`] 的名字参数版，测试直达）。
pub async fn handle_call(
    ctx: &ProxyCtx,
    transport: &dyn HttpTransport,
    method: &str,
    params: &Value,
) -> std::result::Result<Value, (i64, String)> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": params.get("protocolVersion").cloned().unwrap_or(json!("2024-11-05")),
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": MCP_SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_defs() })),
        "tools/call" => {
            let name = params
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or_default();
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
            match dispatch_tool(ctx, transport, name, arguments).await {
                Ok(text) => Ok(json!({
                    "content": [{ "type": "text", "text": text }],
                })),
                Err(message) => Ok(json!({
                    "content": [{ "type": "text", "text": message }],
                    "isError": true,
                })),
            }
        }
        other => Err((-32601, format!("方法不存在：{other}"))),
    }
}

/// 一条 stdin 行 → 一条 stdout 行（`None` = 通知，不执行也不回话）。
pub fn handle_message(
    ctx: &ProxyCtx,
    transport: &Arc<dyn HttpTransport>,
    msg: &Value,
) -> Option<BoxFuture<'static, Option<Value>>> {
    // 通知（没有 id）直接吞掉——**连执行都不执行**：`tools/call` 若以通知到达，跑了也
    // 没处回话，模型看不到结果还以为做了。JSON-RPC 语义本来就只有带 id 的请求才产生
    // 处理；现有客户端通知（notifications/initialized 等）无需服务端动作，代理无状态，
    // 也没有可取消的在途调用需要接 notifications/cancelled。
    let id = msg.get("id").cloned()?;
    let method = msg.get("method")?.as_str()?.to_string();
    let params = msg.get("params").cloned().unwrap_or(json!({}));
    let ctx = ctx.clone();
    let transport = transport.clone();
    Some(Box::pin(async move {
        let response = match handle_call(&ctx, transport.as_ref(), &method, &params).await {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => json!({
                "jsonrpc": "2.0", "id": id,
                "error": { "code": code, "message": message },
            }),
        };
        Some(response)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 记录每次 [`HttpCall`] 的 Fake 传输，按入队顺序回预置响应。
    struct FakeTransport {
        calls: Mutex<Vec<HttpCall>>,
        responses: Vec<HttpResponse>,
    }

    impl FakeTransport {
        fn ok(body: Value) -> Self {
            FakeTransport {
                calls: Mutex::new(vec![]),
                responses: vec![HttpResponse { status: 200, body }],
            }
        }
    }

    impl HttpTransport for FakeTransport {
        fn call(&self, req: HttpCall) -> BoxFuture<'static, TransportResult> {
            self.calls.lock().unwrap().push(req);
            let response = self.responses.first().cloned();
            Box::pin(async move { response.ok_or_else(|| "FakeTransport 响应耗尽".to_string()) })
        }
    }

    fn ctx() -> ProxyCtx {
        ProxyCtx {
            base_url: "http://127.0.0.1:8788".into(),
            token: "tok-123".into(),
            session_id: "s-cli".into(),
        }
    }

    /// 暴露集 = 7 写 + 9 读 + 1 提议列表，名字与 catalog 同源，漏网的工具一个都不许有。
    #[test]
    fn the_mcp_surface_mirrors_the_catalog_exactly() {
        let defs = tool_defs();
        let names: Vec<&str> = defs.iter().map(|d| d["name"].as_str().unwrap()).collect();
        let mut expected: Vec<&str> = CLI_PROPOSAL_TOOLS
            .iter()
            .chain(CLI_READ_TOOLS.iter())
            .copied()
            .collect();
        expected.push(PROPOSALS_LIST_TOOL);
        assert_eq!(names.len(), expected.len(), "{names:?}");
        for name in &expected {
            assert!(names.contains(name), "{name} 不在 MCP 面上：{names:?}");
        }
        // 文件域 / 进程取证 / 网页 / 修复 / 提问 / 修复链的一整套，一个都不许漏进来。
        for leak in [
            "read_file",
            "list_dir",
            "Skill",
            "search_content",
            "run_readonly",
            "web_fetch",
            "repair",
            "ask",
        ] {
            assert!(!names.contains(&leak), "{leak} 漏进了 MCP 面：{names:?}");
        }
    }

    /// 写工具的 schema = catalog schema + 必填 `summary`；读工具不带 summary。
    #[test]
    fn write_tools_carry_a_required_summary_read_tools_do_not() {
        for def in tool_defs() {
            let name = def["name"].as_str().unwrap();
            let schema = &def["inputSchema"];
            let required = schema["required"].as_array().cloned().unwrap_or_default();
            if CLI_PROPOSAL_TOOLS.contains(&name) {
                assert!(
                    required.iter().any(|r| r.as_str() == Some(SUMMARY_PARAM)),
                    "写工具 {name} 的 schema 没带必填 summary：{def}"
                );
            } else {
                assert!(
                    !required.iter().any(|r| r.as_str() == Some(SUMMARY_PARAM)),
                    "读工具 {name} 不该被强加 summary：{def}"
                );
            }
        }
        // 同源的形状钉一条：task 工具的 enum 与 catalog 逐字一致。
        let task = tool_defs()
            .into_iter()
            .find(|d| d["name"] == "task")
            .unwrap();
        assert_eq!(
            task["inputSchema"]["properties"]["action"]["enum"],
            json!([
                "create", "resume", "retry", "pause", "rerun", "cancel", "review", "merge",
                "unstick"
            ])
        );
    }

    /// 写工具 → POST /foreman/proposals，载荷形状与 summary 拆分。
    #[tokio::test]
    async fn a_write_tool_translates_to_a_proposal_creation() {
        let fake = Arc::new(FakeTransport::ok(json!({"id": "p1", "origin": "cli"})));
        let text = dispatch_tool(
            &ctx(),
            fake.as_ref(),
            "task",
            json!({"action": "cancel", "task_id": "t1", "summary": "取消任务 t1"}),
        )
        .await
        .unwrap();
        assert!(text.contains("p1"), "{text}");
        let calls = fake.calls.lock().unwrap();
        let call = &calls[0];
        assert_eq!(call.method, HttpMethod::Post);
        assert_eq!(call.path, "/foreman/proposals");
        assert_eq!(
            call.headers,
            vec![("x-agentpipeline-token".to_string(), "tok-123".to_string())]
        );
        assert_eq!(
            call.body,
            Some(json!({
                "session_id": "s-cli",
                "tool": "task",
                "args": {"action": "cancel", "task_id": "t1"},
                "summary": "取消任务 t1",
            }))
        );
    }

    /// 读工具 → GET /foreman/read/{tool}，args 原文随行；提议列表 → 既有 GET。
    #[tokio::test]
    async fn read_tools_translate_to_the_read_face() {
        let fake = Arc::new(FakeTransport::ok(json!({"tool": "read_task"})));
        dispatch_tool(&ctx(), fake.as_ref(), "read_task", json!({"task_id": "t1"}))
            .await
            .unwrap();
        let call = fake.calls.lock().unwrap()[0].clone();
        assert_eq!(call.method, HttpMethod::Get);
        assert_eq!(call.path, "/foreman/read/read_task");
        assert_eq!(call.query[0], ("session".to_string(), "s-cli".to_string()));
        assert!(call.query[1].0 == "args" && call.query[1].1.contains("t1"));

        let fake = Arc::new(FakeTransport::ok(json!({"proposals": []})));
        dispatch_tool(&ctx(), fake.as_ref(), "proposals", json!({}))
            .await
            .unwrap();
        let call = fake.calls.lock().unwrap()[0].clone();
        assert_eq!(call.path, "/foreman/proposals");
        assert_eq!(call.query[0], ("session".to_string(), "s-cli".to_string()));
    }

    /// 缺 summary 在发请求之前就拒（Fake 上一次调用都不会有）。
    #[tokio::test]
    async fn a_write_tool_without_a_summary_never_leaves_the_proxy() {
        let fake = Arc::new(FakeTransport::ok(json!({})));
        let err = dispatch_tool(
            &ctx(),
            fake.as_ref(),
            "run_command",
            json!({"command": "make check"}),
        )
        .await
        .unwrap_err();
        assert!(err.contains(SUMMARY_PARAM), "{err}");
        assert!(fake.calls.lock().unwrap().is_empty());
        // 空白 summary 同拒。
        let fake = Arc::new(FakeTransport::ok(json!({})));
        let err = dispatch_tool(
            &ctx(),
            fake.as_ref(),
            "run_command",
            json!({"command": "make check", "summary": "   "}),
        )
        .await
        .unwrap_err();
        assert!(err.contains(SUMMARY_PARAM));
    }

    /// 白名单外的工具不发出；HTTP 4xx/5xx 把端点的可读话带回来。
    #[tokio::test]
    async fn unknown_tools_and_http_errors_come_back_as_readable_text() {
        let fake = Arc::new(FakeTransport::ok(json!({})));
        let err = dispatch_tool(
            &ctx(),
            fake.as_ref(),
            "web_fetch",
            json!({"url": "https://x"}),
        )
        .await
        .unwrap_err();
        assert!(err.contains("未知工具"), "{err}");
        assert!(fake.calls.lock().unwrap().is_empty());

        let fake = Arc::new(FakeTransport {
            calls: Mutex::new(vec![]),
            responses: vec![HttpResponse {
                status: 400,
                body: json!({"error": "工具 task 缺必填参数「action」"}),
            }],
        });
        let err = dispatch_tool(
            &ctx(),
            fake.as_ref(),
            "task",
            json!({"summary": "取消", "task_id": "t1"}),
        )
        .await
        .unwrap_err();
        assert!(err.contains("缺必填参数"), "{err}");
    }

    /// initialize 的形状与 tools/call 的结果包装（isError 走结果不走 JSON-RPC 错误）。
    #[tokio::test]
    async fn the_protocol_frames_are_shaped_for_mcp() {
        let fake = Arc::new(FakeTransport::ok(json!({"id": "p1"})));
        let ctx = ctx();
        let init = handle_call(
            &ctx,
            fake.as_ref(),
            "initialize",
            &json!({"protocolVersion": "2025-06-18"}),
        )
        .await
        .unwrap();
        assert_eq!(init["protocolVersion"], "2025-06-18");
        assert_eq!(init["serverInfo"]["name"], MCP_SERVER_NAME);

        let listed = handle_call(&ctx, fake.as_ref(), "tools/list", &json!({}))
            .await
            .unwrap();
        assert_eq!(listed["tools"].as_array().unwrap().len(), 17);

        let failed = handle_call(
            &ctx,
            fake.as_ref(),
            "tools/call",
            &json!({"name": "run_command", "arguments": {"command": "make"}}),
        )
        .await
        .unwrap();
        assert_eq!(failed["isError"], json!(true));
        assert!(failed["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("summary"));

        let err = handle_call(&ctx, fake.as_ref(), "resources/list", &json!({}))
            .await
            .unwrap_err();
        assert_eq!(err.0, -32601);
    }

    /// 通知不执行也不回话；请求的响应带 id 回环。
    #[tokio::test]
    async fn notifications_get_no_response() {
        let fake = Arc::new(FakeTransport::ok(json!({})));
        let ctx = ctx();
        let transport: Arc<dyn HttpTransport> = fake.clone();
        let notification = handle_message(
            &ctx,
            &transport,
            &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        );
        assert!(notification.is_none(), "通知不产生处理，更不产生回话");

        // tools/call 以通知到达也**不执行**——跑了没处回话，模型会以为做了。
        let transport: Arc<dyn HttpTransport> = fake.clone();
        let sneaky = handle_message(
            &ctx,
            &transport,
            &json!({"jsonrpc": "2.0", "method": "tools/call", "params": {"name": "proposals"}}),
        );
        assert!(sneaky.is_none());
        assert!(
            fake.calls.lock().unwrap().is_empty(),
            "通知形态的 tools/call 不许发出 HTTP 调用"
        );

        let response = handle_message(
            &ctx,
            &transport,
            &json!({"jsonrpc": "2.0", "id": 7, "method": "ping"}),
        )
        .unwrap()
        .await
        .unwrap();
        assert_eq!(response["id"], json!(7));
        assert_eq!(response["result"], json!({}));
    }
}

#[cfg(test)]
mod cross_crate_pins {
    use crate::MCP_SERVER_NAME;
    /// MCP 服务器名与 core 侧拼的 `--mcp-config` / 白名单前缀必须同值——一边改名
    /// 一边不知道，claude 看到的工具族就对不上（与决策 247 同源哲学的跨仓版）。
    #[test]
    fn the_server_name_matches_the_core_side() {
        assert_eq!(
            MCP_SERVER_NAME,
            agentpipeline_core::pipeline::foreman::CLI_MCP_SERVER_NAME
        );
    }
}
