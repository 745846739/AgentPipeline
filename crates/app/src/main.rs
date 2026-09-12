//! 二进制入口：CLI 参数解析 + 调用库内 `serve`（决策 11 / 54 / 127 / 153⑤）。
//!
//! 起服逻辑住在库里（`app::serve`），本文件只做参数解析与信号接线，
//! 让冒烟测试与桌面壳能复用同一入口。

use anyhow::Context;
use app::serve::serve;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("serve") => {
            let port = parse_port(&args)?;
            serve_and_wait(port).await
        }
        Some("--version") | Some("version") => {
            println!("agent-pipeline {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("--help") | Some("help") => {
            println!("用法：agent-pipeline [serve] [--port <PORT>]");
            Ok(())
        }
        Some(other) => {
            eprintln!("未知命令：{other}\n用法：agent-pipeline [serve] [--port <PORT>]");
            std::process::exit(2);
        }
    }
}

/// CLI `--port` 覆盖；缺省时回落到 `[server] port`（§10.6.5）。
fn parse_port(args: &[String]) -> anyhow::Result<Option<u16>> {
    let mut port = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--port" {
            port = Some(
                iter.next()
                    .context("--port 缺少取值")?
                    .parse::<u16>()
                    .context("--port 需要数字")?,
            );
        } else if let Some(value) = arg.strip_prefix("--port=") {
            port = Some(value.parse::<u16>().context("--port 需要数字")?);
        }
    }
    Ok(port)
}

/// 启动服务并等待信号驱动的停机（决策 54）：第一次 SIGINT 优雅退出（码 0），
/// 第二次立即退出（码 130）。
async fn serve_and_wait(port_override: Option<u16>) -> anyhow::Result<()> {
    let handle = serve(port_override).await?;
    let shutdown = handle.shutdown.clone();

    tokio::spawn(async move {
        let mut signals = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
            .expect("注册 SIGINT");
        if signals.recv().await.is_some() {
            tracing::info!("收到 SIGINT：停止派发新任务，等当前节点在边界退出");
            let _ = shutdown.send(true);
            if signals.recv().await.is_some() {
                tracing::warn!("收到第二次 SIGINT：立即退出");
                std::process::exit(130);
            }
        }
    });

    handle.server.await.context("服务任务异常")??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_parsing_accepts_both_forms() {
        let args = vec![
            "serve".to_string(),
            "--port".to_string(),
            "9000".to_string(),
        ];
        assert_eq!(parse_port(&args).unwrap(), Some(9000));
        let args = vec!["--port=9100".to_string()];
        assert_eq!(parse_port(&args).unwrap(), Some(9100));
        assert_eq!(parse_port(&[]).unwrap(), None);
        assert!(parse_port(&["--port".to_string(), "abc".to_string()]).is_err());
    }
}
