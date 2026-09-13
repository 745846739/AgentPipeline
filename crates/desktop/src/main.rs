//! 桌面壳入口（决策 153 / 156 / 167）：Tauri 只当外壳与窗口管理，传输层零改动。
//!
//! 壳只做三件事：
//! 1. 起服——复用 `app::serve(ServeOptions::default())`（决策 153⑤ / 157），
//!    `port_override = Some(0)` 内核分配随机端口；
//! 2. 开窗——导航到 `http://127.0.0.1:{port}` 同源 origin（零 CORS，决策 153⑤
//!    首选路线），不启用 Tauri IPC / asset 协议，前端感知不到部署形态；
//! 3. 停机——窗口关闭 → `RunEvent::Exit` → shutdown watch channel 广播（决策 54），
//!    axum 优雅退出；启动恢复（决策 127）由 `serve()` 内部照跑。
//!
//! 单实例锁（决策 153 非传输件）：tauri-plugin-single-instance，二次启动聚焦既有窗口。
//!
//! **局域网访问（决策 167）**：默认仍只绑回环（缺省姿态不变，决策 128/157）。
//! 设 `AGENTPIPELINE_LAN=1` 时改绑 `0.0.0.0`，窗口仍走回环访问（`0.0.0.0` 包含
//! 回环，故本机体验不变），手机经「手机访问」页扫码接入。

use app::serve::ServeOptions;
use tauri::Manager;

/// 局域网模式的开关环境变量（决策 167）。
const LAN_ENV: &str = "AGENTPIPELINE_LAN";

fn main() {
    // 局域网模式：显式 opt-in，默认关（服务能触发真实 LLM 调用，不默认对外）。
    let lan = matches!(
        std::env::var(LAN_ENV).ok().as_deref(),
        Some("1") | Some("true")
    );
    let host = if lan { "0.0.0.0".to_string() } else { "127.0.0.1".to_string() };

    // 起服在 Tauri run loop 之前：端口就绪后窗口才有地址可去。
    // serve 失败直接退出——桌面壳没有比后端更早成功的道理。
    // 窗口地址恒为本机回环：绑 0.0.0.0 时回环仍是同一服务的入口。
    let handle = tauri::async_runtime::block_on(app::serve::serve(ServeOptions {
        port_override: Some(0),
        host_override: Some(host),
        ..ServeOptions::default()
    }))
    .expect("AgentPipeline 服务启动失败");
    let port = handle.port;
    let shutdown = handle.shutdown;
    let mut server = Some(handle.server);

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // 二次启动：聚焦已有窗口，不给第二个实例。
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.set_focus();
            }
        }))
        .setup(move |app| {
            let url: tauri::Url = format!("http://127.0.0.1:{port}").parse()?;
            tauri::webview::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::External(url),
            )
            .title("AgentPipeline")
            .inner_size(1280.0, 800.0)
            .min_inner_size(960.0, 600.0)
            .build()?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("Tauri 壳构建失败")
        .run(move |_app, event| {
            // 窗口关闭 → run loop 结束 → 这里广播停机并等 axum 优雅退出
            // （shutdown watch channel 三处消费：axum / tick / 维护循环，决策 54）。
            if let tauri::RunEvent::Exit = event {
                let _ = shutdown.send(true);
                if let Some(server) = server.take() {
                    let _ = tauri::async_runtime::block_on(server);
                }
            }
        });
}
