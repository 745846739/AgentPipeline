//! L2.5 端到端：**端口跨重启稳定**（决策 213）。
//!
//! 这条保证的形状很朴素——「下次开机，手机里那本书签还打得开」——却是两条消费者共同的
//! 前提：分享页的二维码（决策 167）、配对令牌（决策 182㉖㉗㉘，令牌本身已经是长期有效的，
//! 端口曾是唯一每开一次就变的东西）。故这里跑真东西，而不是断言一个纯函数：
//!
//! - **重启不变**：写一份 `[server] port = P` 的配置（**桌面壳的形态**：不传 `--port`），起二进制两次，两次的就绪行必须都是 P；
//! - **占用则退让**：占住配置里那个端口，用桌面壳那套 `ServeOptions`（`port_override = None` + `port_fallback_to_ephemeral = true`）起服务，它必须**换一个端口起来**，并把这件事经 `/server-info` 的 `port_source = fallback` 说出来——退让而不出声，使用者只会看到一张打不开的书签而查不到原因。
//!
//! 两条用例的形态差异是**被测行为的差异**，不是随意选的：第一条要的是「跨进程重启」，
//! 故 spawn 真二进制；第二条只存在于 `ServeOptions` 的一条非 CLI 选项上（命令行不暴露，
//! 见 `serve::ServeOptions::port_fallback_to_ephemeral`），故直接调 `app::serve::serve`。
//! **因此本文件只允许一处 in-process `serve`**：`TestHome::install_env` 改的是进程级环境
//! 变量（testkit 的文档里写明了并发使用会互相干扰），两条都改成 in-process 就会互相踩。

use std::time::Duration;

use app::serve::{serve, ServeOptions};
use testkit::TestHome;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};

/// 极简 HTTP/1.1 客户端（与 `lan_bind.rs` / `smoke.rs` 同一手法：只读状态码与 body，不引 reqwest）。
///
/// 这一份是**异步**的：第二条用例的服务端跑在测试自己的运行时里，阻塞式读会把它饿死。
async fn try_http(
    method: &str,
    port: u16,
    path: &str,
    body: Option<&str>,
) -> Option<(u16, String)> {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .ok()?;
    let body = body.unwrap_or("");
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        stream.write_all(request.as_bytes()).await.ok()?;
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.ok()?;
        let text = String::from_utf8_lossy(&response);
        let status = text.split_whitespace().nth(1)?.parse::<u16>().ok()?;
        let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string())?;
        Some((status, body))
    })
    .await
    .ok()?
}

/// 从 `/server-info` 的 JSON 里抠一个字段（不引 serde_json：这里只要两个值）。
fn field(body: &str, key: &str) -> String {
    let needle = format!("\"{key}\":");
    let rest = body
        .split_once(&needle)
        .unwrap_or_else(|| panic!("响应里没有 {key}：{body}"))
        .1;
    match rest.trim_start().split(',').next() {
        Some(v) if v.starts_with('"') => v.trim_matches('"').to_string(),
        Some(v) => v.trim().to_string(),
        None => panic!("{key} 解析失败：{body}"),
    }
}

/// 每个测试独占的家目录，并写入一份 `[server] port` 固定的配置。
///
/// `host` 显式写成回环：这两条用例关心的是端口，绑全网卡会把机器的网卡枚举拉进来。
fn home_with_port(port: u16) -> TestHome {
    let home = TestHome::new().unwrap();
    std::fs::write(
        home.home().config_path(),
        format!("[server]\nhost = \"127.0.0.1\"\nport = {port}\n"),
    )
    .expect("写入 config.toml");
    home
}

/// 拿一个空闲端口当夹具（先绑 `:0` 读出号码再释放）。
///
/// 这是**测试夹具**的手法；生产侧不做这种探测（决策 153⑤ 明写不再需要「先探测空闲端口
/// 再释放」，那中间有一个被别的进程抢走的窗口）。
fn free_port() -> u16 {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    // 必须显式释放：夹具要的是一个**当下空闲**的端口，留着它绑住就等于把头一条用例
    // 变成「端口被占用 → 启动失败」的反例。
    drop(probe);
    port
}

async fn read_ready_port(stdout: tokio::process::ChildStdout) -> u16 {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut lines = BufReader::new(stdout).lines();
    while let Some(line) = lines.next_line().await.transpose() {
        let line = line.expect("读取子进程 stdout");
        if let Some(rest) = line.trim().strip_prefix("AGENTPIPELINE_READY port=") {
            return rest.trim().parse::<u16>().expect("就绪行端口可解析");
        }
    }
    panic!("子进程在退出前未打印就绪行");
}

/// 起一次真二进制，返回 `(子进程, 实际端口)`。**不传 `--port`**——那正是桌面壳与
/// 普通命令行的形态：端口来自配置文件。
async fn spawn_server_using_config(home: &TestHome) -> (Child, u16) {
    let bin = env!("CARGO_BIN_EXE_agent-pipeline");
    let mut child = Command::new(bin)
        .arg("serve")
        .env("AGENTPIPELINE_HOME", home.path())
        .env("RUST_LOG", "warn")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("二进制可启动");
    let stdout = child.stdout.take().expect("stdout 已管道化");
    let port = tokio::time::timeout(Duration::from_secs(20), read_ready_port(stdout))
        .await
        .expect("20s 内应打印就绪行");
    (child, port)
}

#[tokio::test]
async fn configured_port_survives_restart() {
    let port = free_port();
    let home = home_with_port(port);

    let (mut child, first) = spawn_server_using_config(&home).await;
    assert_eq!(
        first, port,
        "不传 --port 时必须用 [server] port（这正是桌面壳的形态）"
    );
    let (status, body) = try_http("GET", first, "/server-info", None)
        .await
        .expect("服务可达");
    assert_eq!(status, 200, "{body}");
    assert_eq!(field(&body, "port"), port.to_string(), "{body}");
    // 端口来源（决策 213）：这一档是 config，界面据此知道「这个端口是配置里的，重启不变」
    assert_eq!(field(&body, "port_source"), "config", "{body}");
    child.kill().await.expect("可杀掉子进程");

    // 重启：手机里那本书签指着的是同一台机器、同一个端口。
    let (mut child, second) = spawn_server_using_config(&home).await;
    assert_eq!(
        second, first,
        "重启后端口必须不变——否则手机上存过的网址就作废了（决策 213）"
    );
    child.kill().await.expect("可杀掉子进程");
}

#[tokio::test]
async fn occupied_config_port_falls_back_and_reports_why() {
    // 占住配置里那个端口：模拟「8788 被别的程序（比如 make run 的开发服务）占着」。
    let held = std::net::TcpListener::bind("127.0.0.1:0").expect("占位监听器可创建");
    let occupied = held.local_addr().unwrap().port();
    let home = home_with_port(occupied);
    let _env = home.install_env();

    let handle = serve(ServeOptions {
        // 桌面壳的形态（决策 213）：端口来自配置，占用时退让而不是拒绝开窗。
        port_override: None,
        port_fallback_to_ephemeral: true,
        ..ServeOptions::default()
    })
    .await
    .expect("首选端口被别的进程占着时，桌面壳这条路必须退让而不是起不来");

    assert_ne!(handle.port, occupied, "应换到别的端口");
    let (status, body) = try_http("GET", handle.port, "/server-info", None)
        .await
        .expect("退让之后服务应可达");
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        field(&body, "port"),
        handle.port.to_string(),
        "AppState 里那个端口必须是真实端口（决策 128 的同源判定指着它）：{body}"
    );
    assert_eq!(
        field(&body, "port_source"),
        "fallback",
        "退让必须被说出来——否则使用者只看到一张打不开的书签，查不到原因：{body}"
    );

    let _ = handle.shutdown.send(true);
    let _ = handle.server.await;
    drop(held);
}
