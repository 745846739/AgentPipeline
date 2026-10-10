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
        "用法：agent-pipeline [serve] [--port <PORT>] [--host <IP>] [--public-base-url <ORIGIN>] \
         [--tls-cert <PEM> --tls-key <PEM>] [--allowed-origin <ORIGIN>]"
    );
    println!("  --port <PORT>              覆盖 [server] port（0 = 内核随机分配）");
    println!("  --host <IP>                覆盖 [server] host（局域网访问用 0.0.0.0）");
    println!("  --public-base-url <ORIGIN> 覆盖 [server] public_base_url（决策 334）：");
    println!("                             反向代理 / 公网入口后面部署时，手机访问页的二维码");
    println!("                             指向它，如 --public-base-url https://example.com:3389");
    println!(
        "  --tls-cert / --tls-key     覆盖 [server] tls_cert / tls_key（决策 335）：两个一起给"
    );
    println!(
        "                             即由本进程终止 TLS（没有反向代理的部署形态），都不给 = 明文"
    );
    println!("  --allowed-origin <ORIGIN>  额外放行的跨源写 origin，可重复（决策 157）：");
    println!("                             局域网浏览器要操作写接口，需放行其页面 origin，");
    println!("                             如 --allowed-origin http://192.168.1.10:8788");
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
            "--public-base-url" => {
                let raw = take_value()?;
                let url = normalize_origin(&raw)
                    .map_err(|e| anyhow::anyhow!("--public-base-url 无效：{e}"))?;
                options.public_base_url_override = Some(url);
            }
            "--tls-cert" => options.tls_cert_override = Some(take_value()?),
            "--tls-key" => options.tls_key_override = Some(take_value()?),
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
        assert_eq!(options.public_base_url_override, None);
        assert!(options.extra_allowed_origins.is_empty());
    }

    /// 公网入口（决策 334）：`--public-base-url` 认两种写法、经 `normalize_origin` 归一，
    /// 非法形态**启动期**就失败（不是等到手机上扫出一张打不开的码）。
    #[test]
    fn serve_args_accept_public_base_url_in_both_forms() {
        let split: Vec<String> = ["--public-base-url", "HTTPS://203.0.113.10:3389/"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            parse_serve_args(&split)
                .unwrap()
                .public_base_url_override
                .as_deref(),
            Some("https://203.0.113.10:3389"),
            "归一：小写化 + 剥尾部斜杠（否则二维码会拼出 `//?pair=…`）"
        );
        let inline: Vec<String> = ["--public-base-url=https://ap.example.com"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            parse_serve_args(&inline)
                .unwrap()
                .public_base_url_override
                .as_deref(),
            Some("https://ap.example.com")
        );
        for bad in ["203.0.113.10:3389", "https://ap.example.com/app"] {
            let args: Vec<String> = vec!["--public-base-url".into(), bad.into()];
            assert!(
                parse_serve_args(&args).is_err(),
                "非法入口形态应启动期失败：{bad}"
            );
        }
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

    /// TLS 两个旗标（决策 335）：两种写法都认、缺取值报错。
    ///
    /// **这里只断言「参数搬到了 `ServeOptions` 上」**：路径对不对、PEM 坏没坏由
    /// `serve::Transport::from_pem` 在启动期判（那条链有自己的用例）——CLI 这一层
    /// 不该去碰文件系统，它只负责把两个字符串原样递下去。
    #[test]
    fn serve_args_carry_the_tls_pair() {
        let split: Vec<String> = [
            "--tls-cert",
            "/etc/agentpipeline/tls/c.pem",
            "--tls-key",
            "/etc/agentpipeline/tls/k.pem",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let options = parse_serve_args(&split).unwrap();
        assert_eq!(
            options.tls_cert_override.as_deref(),
            Some("/etc/agentpipeline/tls/c.pem")
        );
        assert_eq!(
            options.tls_key_override.as_deref(),
            Some("/etc/agentpipeline/tls/k.pem")
        );

        let inline: Vec<String> = vec!["--tls-cert=/tmp/only-cert.pem".into()];
        assert_eq!(
            parse_serve_args(&inline)
                .unwrap()
                .tls_cert_override
                .as_deref(),
            Some("/tmp/only-cert.pem"),
            "只给一个旗标在**这一层**是合法的：配不配对由启动期那道门判（决策 335）"
        );

        for flag in ["--tls-cert", "--tls-key"] {
            let missing: Vec<String> = vec![flag.into()];
            assert!(parse_serve_args(&missing).is_err(), "{flag} 缺取值该报错");
        }
    }
}
