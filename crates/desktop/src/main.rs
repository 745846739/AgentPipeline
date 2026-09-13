//! 桌面壳入口（决策 153 / 156）：Tauri 只当外壳与窗口管理，传输层零改动。
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

use app::serve::ServeOptions;
use tauri::Manager;

fn main() {
    // 起服在 Tauri run loop 之前：端口就绪后窗口才有地址可去。
    // serve 失败直接退出——桌面壳没有比后端更早成功的道理。
    let handle = tauri::async_runtime::block_on(app::serve::serve(ServeOptions {
        port_override: Some(0),
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
