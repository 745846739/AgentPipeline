//! 桌面壳入口（决策 153 / 156 / 167）：Tauri 只当外壳与窗口管理，传输层零改动。
//!
//! 壳只做三件事：
//! 1. 起服——复用 `app::serve(ServeOptions)`（决策 153⑤ / 157），
//!    **端口不指定**（决策 213）：回落 `[server] port`（缺省 8788）。此前这里是
//!    `port_override = Some(0)`，于是每次重启换一个临时端口，手机扫码存下的网址
//!    下次启动就失效——而令牌（决策 182㉖㉗㉘）、界面上的绑定选择（决策 186）都是
//!    跨重启有效的，端口是唯一每开一次就变的东西。真被别的进程占着时**退让**到内核
//!    随机端口（`port_fallback_to_ephemeral`）而不是拒绝开窗，退让这件事会上报到
//!    `/server-info` 的 `port_source`，由分享页讲给使用者听；
//! 2. 开窗——导航到 `http://127.0.0.1:{port}` 同源 origin（零 CORS，决策 153⑤
//!    首选路线），不启用 Tauri IPC / asset 协议，前端感知不到部署形态；
//! 3. 停机——窗口关闭 → `RunEvent::Exit` → shutdown watch channel 广播（决策 54），
//!    axum 优雅退出；启动恢复（决策 127）由 `serve()` 内部照跑。
//!
//! 单实例锁（决策 153 非传输件）：tauri-plugin-single-instance，二次启动聚焦既有窗口。
//!
//! **局域网访问（决策 167 / 186）**：默认仍只绑回环（缺省姿态不变，决策 128/157）。
//! 两条路打开它，优先级是「环境变量 > 界面上的开关 > `[server] host`」：
//! - 设 `AGENTPIPELINE_LAN=1` 启动（启动期覆盖，界面改不动这一次）；
//! - 或者什么都不设，在「手机访问」页按下「绑定全网卡」——`serve` 会读界面上存下的
//!   选择（决策 186），下次启动仍然生效。**故这里只在设了环境变量时才传 `host_override`**：
//!   恒传 `Some(...)` 会让界面上那颗钮永远赢不了。
//!
//! 两种路子里窗口都走回环访问（`0.0.0.0` 包含回环，故本机体验不变），手机经
//! 「手机访问」页扫码接入。

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
    // 只在显式设置时才覆盖：没设就让 `serve` 按「界面设置 > [server] host」解析
    // （决策 186）——恒传 Some 会把界面上的那颗钮彻底架空。
    let host = lan.then(|| "0.0.0.0".to_string());

    // 起服在 Tauri run loop 之前：端口就绪后窗口才有地址可去。
    // serve 失败直接退出——桌面壳没有比后端更早成功的道理。
    // 窗口地址恒为本机回环：绑 0.0.0.0 时回环仍是同一服务的入口。
    let handle = tauri::async_runtime::block_on(app::serve::serve(ServeOptions {
        // 不指定端口（决策 213）：用 `[server] port`（缺省 8788），重启后手机上的书签仍有效。
        port_override: None,
        // 真被占用时退让到随机端口而不是打不开窗；退让会上报，分享页据此说明。
        port_fallback_to_ephemeral: true,
        host_override: host,
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
