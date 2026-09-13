//! 二进制入口：CLI 参数解析 + 调用库内 `serve`（决策 11 / 54 / 127 / 153⑤）。
//!
//! 起服逻辑住在库里（`app::serve`），本文件只做参数解析与信号接线，
//! 让冒烟测试与桌面壳能复用同一入口。

use agentpipeline_core::config::normalize_origin;
use anyhow::Context;
use app::serve::{serve, ServeOptions};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("serve") => {
            let options = parse_serve_args(&args)?;
            serve_and_wait(options).await
        }
        Some("--version") | Some("version") => {
            println!("agent-pipeline {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("--help") | Some("help") => {
            print_help();
            Ok(())
        }
        Some(other) => {
            eprintln!("未知命令：{other}");
            print_help();
            std::process::exit(2);
        }
    }
}

fn print_help() {
    println!(
        "用法：agent-pipeline [serve] [--port <PORT>] [--host <IP>] [--allowed-origin <ORIGIN>]"
    );
    println!("  --port <PORT>              覆盖 [server] port（0 = 内核随机分配）");
    println!("  --host <IP>                覆盖 [server] host（局域网访问用 0.0.0.0）");
    println!("  --allowed-origin <ORIGIN>  额外放行的跨源写 origin，可重复（决策 157）：");
    println!("                             局域网浏览器要操作写接口，需放行其页面 origin，");
    println!("                             如 --allowed-origin http://192.168.1.10:8787");
}

/// serve 参数解析（决策 157）：`--port` / `--host` 覆盖配置文件；`--allowed-origin`
/// 可重复、单值内可逗号分隔，经 `normalize_origin` 校验（非法 fail fast）。
/// 未识别的参数沿用 `parse_port` 时代的宽容姿态，静默忽略。
fn parse_serve_args(args: &[String]) -> anyhow::Result<ServeOptions> {
    let mut options = ServeOptions::default();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v.to_string())),
            _ => (arg.as_str(), None),
        };
        let mut take_value = || -> anyhow::Result<String> {
            match &inline {
                Some(v) => Ok(v.clone()),
                None => iter.next().cloned().context(format!("{flag} 缺少取值")),
            }
        };
        match flag {
            "--port" => {
                options.port_override =
                    Some(take_value()?.parse::<u16>().context("--port 需要数字")?);
            }
            "--host" => options.host_override = Some(take_value()?),
            "--allowed-origin" => {
                let raw = take_value()?;
                for part in raw.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                    let origin = normalize_origin(part)
                        .map_err(|e| anyhow::anyhow!("--allowed-origin 无效：{e}"))?;
                    options.extra_allowed_origins.push(origin);
                }
            }
            _ => {}
        }
    }
    Ok(options)
}

/// 启动服务并等待信号驱动的停机（决策 54）：第一次 SIGINT 优雅退出（码 0），
/// 第二次立即退出（码 130）。
async fn serve_and_wait(options: ServeOptions) -> anyhow::Result<()> {
    let handle = serve(options).await?;
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
    fn serve_args_accept_both_forms_and_repeatable_origins() {
        let args: Vec<String> = [
            "serve",
            "--port",
            "9000",
            "--host",
            "0.0.0.0",
            "--allowed-origin=http://192.168.1.10:8787",
            "--allowed-origin",
            "HTTP://10.0.0.5:8787/,http://10.0.0.6:8787",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let options = parse_serve_args(&args).unwrap();
        assert_eq!(options.port_override, Some(9000));
        assert_eq!(options.host_override.as_deref(), Some("0.0.0.0"));
        assert_eq!(
            options.extra_allowed_origins,
            vec![
                "http://192.168.1.10:8787",
                "http://10.0.0.5:8787",
                "http://10.0.0.6:8787",
            ]
        );
    }

    #[test]
    fn serve_args_defaults_are_empty() {
        let options = parse_serve_args(&[]).unwrap();
        assert_eq!(options.port_override, None);
        assert_eq!(options.host_override, None);
        assert!(options.extra_allowed_origins.is_empty());
    }

    #[test]
    fn serve_args_reject_bad_origin_and_missing_value() {
        let bad_origin: Vec<String> = vec!["--allowed-origin".into(), "ftp://x".into()];
        assert!(parse_serve_args(&bad_origin).is_err());
        let missing: Vec<String> = vec!["--port".into()];
        assert!(parse_serve_args(&missing).is_err());
        let not_a_number: Vec<String> = vec!["--port".into(), "abc".into()];
        assert!(parse_serve_args(&not_a_number).is_err());
    }
}
