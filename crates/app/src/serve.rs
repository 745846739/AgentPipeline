//! 服务启动（决策 153⑤：起服逻辑沉入 lib，供二进制、冒烟测试与桌面壳复用）。
//!
//! 二进制入口只做参数解析后调用 [`serve`]；绑定支持端口 `0`——此时内核分配真实端口，
//! 由 [`ServerHandle::port`] 与启动日志给出，调用方不再需要「先探测空闲端口再释放」
//! 的绕开（决策 153⑤；决策 128 的同源 origin 判定也依赖这个真实端口）。

use std::sync::Arc;

use agentpipeline_core::clock::SystemClock;
use agentpipeline_core::config::Config;
use agentpipeline_core::home::{check_permissions, Home};
use agentpipeline_core::sse::SseBus;
use agentpipeline_core::storage::Store;
use anyhow::Context;

use crate::runtime::Runtime;
use crate::{build_router, AppState};

/// 已就绪的服务句柄：真实绑定地址可读，`shutdown` 置位触发优雅退出。
pub struct ServerHandle {
    /// 内核分配的实际端口（绑定 `:0` 时为真实值，不再是 0）。
    pub port: u16,
    /// 请求停机（决策 54：第一次 SIGINT 走这里）。
    pub shutdown: tokio::sync::watch::Sender<bool>,
    /// 服务任务；正常停机后 resolve。
    pub server: tokio::task::JoinHandle<anyhow::Result<()>>,
}

/// 绑定监听地址并回读真实端口（决策 153⑤）。
///
/// 端口 `0` 时由内核分配真实端口，返回值即真值；调用方（[`AppState`] 的同源 origin
/// 白名单、启动日志、子进程就绪行）都必须用它，不得沿用配置里的 `0`。
pub async fn bind_listener(
    host: &str,
    port: u16,
) -> anyhow::Result<(tokio::net::TcpListener, std::net::SocketAddr)> {
    let addr = tokio::net::lookup_host((host, port))
        .await
        .with_context(|| format!("无法解析绑定地址 {host}:{port}"))?
        .next()
        .context("绑定地址解析为空")?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("端口被占用或无法绑定：{addr}"))?;
    let bound = listener
        .local_addr()
        .context("无法读取内核分配的实际端口")?;
    Ok((listener, bound))
}

/// 以给定配置启动服务，返回可读回真实端口的句柄。
///
/// `port_override` 为 `None` 时回落 `[server] port`（§10.6.5）；`0` 表示由内核分配，
/// 真实端口经 [`ServerHandle::port`] 与启动日志给出（决策 153⑤）。
pub async fn serve(port_override: Option<u16>) -> anyhow::Result<ServerHandle> {
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

    // 先绑定再建 state：端口 0 时把内核分配的真实端口交给 AppState，
    // 决策 128 的本机 origin 白名单必须用真实端口（用 0 会拒掉桌面壳的同源请求）。
    let (listener, bound) = bind_listener(&server.host, port).await?;

    let state = AppState::new(store, home, settings, bound.port())
        .with_sse(sse)
        .with_executor(runtime.executor())
        .with_resume_hook(runtime.resume_hook.clone());
    let router = build_router(state);

    tracing::info!(%bound, port = bound.port(), "AgentPipeline 已启动");
    // 就绪标记（决策 153⑤）：tracing 输出受 `RUST_LOG` 过滤，子进程（冒烟测试 / 桌面壳）
    // 需要一条不受日志级别影响的确定性信号来读取内核分配的真实端口。
    println!("AGENTPIPELINE_READY port={}", bound.port());

    let server_task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.changed().await;
            })
            .await
            .context("axum 服务异常退出")
    });

    Ok(ServerHandle {
        port: bound.port(),
        shutdown: shutdown_tx,
        server: server_task,
    })
}

/// 初始化 tracing（§10.6.5 的 `[logging] level`）。
pub fn init_tracing(config: &Config) {
    use tracing_subscriber::EnvFilter;
    let filter =
        EnvFilter::try_new(&config.logging.level).unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn binding_port_zero_reads_back_kernel_assigned_port() {
        // 决策 153⑤：绑定 0 时返回的必须是内核分配的真实端口，不能是 0。
        let (listener, bound) = bind_listener("127.0.0.1", 0).await.unwrap();
        assert_ne!(bound.port(), 0, "应回读内核分配的真实端口");
        assert_eq!(listener.local_addr().unwrap().port(), bound.port());
    }

    #[tokio::test]
    async fn binding_occupied_port_reports_clear_error() {
        // 端口占用仍须明确报错（不静默换端口）。
        let (listener, bound) = bind_listener("127.0.0.1", 0).await.unwrap();
        let err = bind_listener("127.0.0.1", bound.port())
            .await
            .expect_err("同端口二次绑定应失败");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("端口被占用或无法绑定"),
            "错误信息应明确：{msg}"
        );
        drop(listener);
    }
}
