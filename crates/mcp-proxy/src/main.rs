//! stdio MCP 薄代理的进程入口（决策 427，票 talk-cli-engine 05）。
//!
//! 由 claude 经 `--mcp-config` 起为**它自己的子进程**（主服务不持有它），按行读
//! JSON-RPC、按行回。三个环境变量必须齐（[`ENV_BASE_URL`] / [`ENV_TOKEN`] /
//! [`ENV_SESSION_ID`]）：缺任一只记一行 stderr——协议循环照常跑（`initialize` 与
//! `tools/list` 仍应答），工具调用一律回 `isError` 结果，让模型看到可读的话而不是
//! 连不上就死。

use std::sync::Arc;

use agentpipeline_mcp_proxy::{
    handle_message, ProxyCtx, ReqwestTransport, ENV_BASE_URL, ENV_SESSION_ID, ENV_TOKEN,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::main]
async fn main() {
    let base_url = std::env::var(ENV_BASE_URL).unwrap_or_default();
    let token = std::env::var(ENV_TOKEN).unwrap_or_default();
    let session_id = std::env::var(ENV_SESSION_ID).unwrap_or_default();
    for (name, value) in [
        (ENV_BASE_URL, &base_url),
        (ENV_TOKEN, &token),
        (ENV_SESSION_ID, &session_id),
    ] {
        if value.is_empty() {
            eprintln!(
                "agentpipeline-mcp-proxy: 环境变量 {name} 没给——工具调用会失败（协议照常应答）"
            );
        }
    }
    let ctx = ProxyCtx {
        base_url,
        token,
        session_id,
    };

    let transport: Arc<dyn agentpipeline_mcp_proxy::HttpTransport> =
        Arc::new(ReqwestTransport::new(ctx.base_url.clone()));

    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut lines = BufReader::new(stdin).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(line) else {
            // 单行坏输入不杀进程：回一个协议错误，循环继续（无状态代理的韧性形态）。
            let _ = stdout
                .write_all(
                    format!(
                        "{}\n",
                        serde_json::json!({
                            "jsonrpc": "2.0", "id": serde_json::Value::Null,
                            "error": {"code": -32700, "message": "解析失败：不是合法 JSON"},
                        })
                    )
                    .as_bytes(),
                )
                .await;
            continue;
        };
        if let Some(future) = handle_message(&ctx, &transport, &msg) {
            if let Some(response) = future.await {
                let _ = stdout.write_all(format!("{response}\n").as_bytes()).await;
                let _ = stdout.flush().await;
            }
        }
    }
}
