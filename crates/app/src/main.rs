//! 二进制入口：CLI + axum HTTP 服务器（决策 11 / 54 / 127）。

use std::sync::Arc;

use agentpipeline_core::clock::SystemClock;
use agentpipeline_core::config::Config;
use agentpipeline_core::home::{check_permissions, Home};
use agentpipeline_core::sse::SseBus;
use agentpipeline_core::storage::Store;
use anyhow::Context;
use app::runtime::Runtime;
use app::{build_router, AppState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("serve") => {
            let port = parse_port(&args)?;
            serve(port).await
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

async fn serve(port_override: Option<u16>) -> anyhow::Result<()> {
    let home = Home::from_env();
    home.ensure_dirs()?;
    let config = Config::load(&home.config_path())?;
    let settings = config.settings();

    // 权限校验：过宽只告警，不阻断启动（§12.14）
    let wide = check_permissions(&home);
    if !wide.is_empty() {
        eprintln!(
            "警告：以下路径权限过宽（建议 0700/0600）：{:?}",
            wide.iter()
                .map(|(p, m)| format!("{} {:o}", p.display(), m))
                .collect::<Vec<_>>()
        );
    }

    init_tracing(&config);

    let store = Store::open(home.clone(), Arc::new(SystemClock)).await?;

    // 恢复流程第一步（决策 127）：清理 kill -9 残留的 executor_owner
    let cleared = store.clear_executor_owners().await?;
    if cleared > 0 {
        tracing::info!(cleared, "已清理残留的 executor 持有者");
    }

    // 配置 fail fast（决策 47 / 103 / 134）
    let report = store.validate_startup(&settings).await?;
    if !report.demoted_providers.is_empty() {
        tracing::warn!(?report.demoted_providers, "不受支持的 vendor 已降级 enabled=0");
    }

    // CLI --port > [server] port（§10.6.5 的配置此前被硬编码架空，决策 128 修订同批对齐）
    let server = config.server.clone();
    let port = port_override.unwrap_or(server.port);

    // 停机信号（决策 54）：一处广播，三处消费——axum、tick 循环、维护循环。
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

    // 生产运行时（票 17）：真实 LLM + executor + scheduler（决策 55）。
    let sse = Arc::new(SseBus::default());
    let runtime = Runtime::new(store.clone(), settings.clone(), sse.clone());
    runtime.spawn_tick_loop(store.clone(), settings.clone(), shutdown_tx.subscribe());
    runtime.spawn_maintenance_loop(store.clone(), settings.clone(), shutdown_tx.subscribe());

    let state = AppState::new(store, home, settings, port)
        .with_sse(sse)
        .with_executor(runtime.executor())
        .with_resume_hook(runtime.resume_hook.clone());
    let router = build_router(state);

    let addr = tokio::net::lookup_host((server.host.as_str(), port))
        .await
        .with_context(|| format!("无法解析绑定地址 {}:{}", server.host, port))?
        .next()
        .context("绑定地址解析为空")?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("端口被占用或无法绑定：{addr}"))?;
    tracing::info!(%addr, "AgentPipeline 已启动");

    // 优雅关闭（决策 54）：第一次 SIGINT 停止派发新任务并在节点边界退出；
    // 第二次立即退出。
    tokio::spawn(async move {
        let mut signals = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
            .expect("注册 SIGINT");
        if signals.recv().await.is_some() {
            tracing::info!("收到 SIGINT：停止派发新任务，等当前节点在边界退出");
            let _ = shutdown_tx.send(true);
            if signals.recv().await.is_some() {
                tracing::warn!("收到第二次 SIGINT：立即退出");
                std::process::exit(130);
            }
        }
    });

    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.changed().await;
        })
        .await?;
    Ok(())
}

fn init_tracing(config: &Config) {
    use tracing_subscriber::EnvFilter;
    let filter =
        EnvFilter::try_new(&config.logging.level).unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
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
