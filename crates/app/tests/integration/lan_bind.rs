//! L2.5 端到端：界面上的绑定开关（决策 186）——spawn **真二进制**跑一遍。
//!
//! 这一层是必要的，因为「换监听器」这件事在 L3 的 in-process router 上**根本不存在**
//! （那个形态没有监听器，`AppState::rebind` 是 `None`，端点只能报 503）。要验的是：
//! 按下钮之后**真的换了绑定**、重启**还记得**、清掉之后**真的回到配置文件那一级**。
//!
//! 三处刻意保留的粗糙，都是被测行为本身决定的：
//! - 改绑会切断所有连接，**包括发出这次请求的那条**——所以 `POST /server/lan` 的应答
//!   常常读不到。这与前端同一条口径：传输失败不等于改绑失败，真值以重读
//!   `GET /server-info` 为准。测试里也是这样读的。
//! - 换绑期间有**几毫秒没有监听者**（回环与全网卡不能同时绑同一端口，见 `serve::swap`），
//!   故重读要带重试。
//! - 只断言「绑定地址」这一件可观测事实，不断言网卡枚举结果（那取决于跑测试的机器）。

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use testkit::TestHome;
use tokio::process::{Child, Command};

/// 极简 HTTP/1.1 客户端（与 `smoke.rs` 同一手法：只读状态码与 body，不引 reqwest）。
///
/// 返回 `None` = 传输层失败（连接被切断 / 拒绝）。改绑场景里这是一个**合法结果**，
/// 故不能当 panic 处理。
fn try_http(method: &str, port: u16, path: &str, body: Option<&str>) -> Option<(u16, String)> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let body = body.unwrap_or("");
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    let status = response
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())?;
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    Some((status, body))
}

/// 读一次 `/server-info`，重试到服务在新地址上重新可达为止（换绑有几毫秒空窗）。
fn server_info_with_retry(port: u16) -> (u16, String) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Some(hit) = try_http("GET", port, "/server-info", None) {
            return hit;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("改绑后 10s 内服务未恢复可达（端口 {port}）");
}

/// 从 `/server-info` 的 JSON 里抠一个字段（不引 serde_json 的解析器：这里只要两个值）。
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

async fn spawn_server(home: &TestHome) -> (Child, u16) {
    let bin = env!("CARGO_BIN_EXE_agent-pipeline");
    let mut child = Command::new(bin)
        .arg("serve")
        .arg("--port")
        .arg("0")
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

/// 停掉子进程并等它退出（重启场景要复用同一个 home）。
async fn stop(mut child: Child) {
    child.kill().await.expect("可杀掉子进程");
}

#[tokio::test]
async fn lan_toggle_rebinds_persists_and_reverts() {
    let home = TestHome::new().unwrap();
    let (child, port) = spawn_server(&home).await;

    // ① 缺省：只绑回环，来源是配置文件（`[server] host` 缺省 127.0.0.1）
    let (status, body) = try_http("GET", port, "/server-info", None).expect("服务可达");
    assert_eq!(status, 200, "{body}");
    assert_eq!(field(&body, "host"), "127.0.0.1");
    assert_eq!(field(&body, "loopback_only"), "true");
    assert_eq!(field(&body, "bind_source"), "config");

    // ② 按下「绑定全网卡」：应答**可能**读不到（这次请求本身就在被切断的连接上）
    let _ = try_http("POST", port, "/server/lan", Some(r#"{"enabled":true}"#));
    let (status, body) = server_info_with_retry(port);
    assert_eq!(status, 200, "{body}");
    assert_eq!(field(&body, "host"), "0.0.0.0", "应真的换了绑定：{body}");
    assert_eq!(field(&body, "loopback_only"), "false", "{body}");
    assert_eq!(field(&body, "bind_source"), "settings", "{body}");

    // ③ 重启后仍然记得（否则用户每开一次都得重点一遍，那颗钮就白做了）
    stop(child).await;
    let (child, port) = spawn_server(&home).await;
    let (status, body) = try_http("GET", port, "/server-info", None).expect("服务可达");
    assert_eq!(status, 200, "{body}");
    assert_eq!(field(&body, "host"), "0.0.0.0", "重启后应沿用界面上的选择");
    assert_eq!(field(&body, "bind_source"), "settings", "{body}");

    // ④ 清掉界面设置 → 回到配置文件那一级（127.0.0.1），且重启后不再反弹
    let _ = try_http("DELETE", port, "/server/lan", None);
    let (status, body) = server_info_with_retry(port);
    assert_eq!(status, 200, "{body}");
    assert_eq!(field(&body, "host"), "127.0.0.1", "{body}");
    assert_eq!(field(&body, "bind_source"), "config", "{body}");

    stop(child).await;
    let (child, port) = spawn_server(&home).await;
    let (_status, body) = try_http("GET", port, "/server-info", None).expect("服务可达");
    assert_eq!(
        field(&body, "host"),
        "127.0.0.1",
        "清掉之后重启不该回到 0.0.0.0：{body}"
    );
    stop(child).await;
}

#[tokio::test]
async fn startup_override_beats_the_settings_row() {
    // 优先级：启动参数 > 界面设置 > 配置文件（决策 186）。用 `--host` 模拟启动期覆盖。
    let home = TestHome::new().unwrap();
    let (child, port) = spawn_server(&home).await;
    // 先用界面把选择设成全网卡
    let _ = try_http("POST", port, "/server/lan", Some(r#"{"enabled":true}"#));
    let (_status, body) = server_info_with_retry(port);
    assert_eq!(field(&body, "host"), "0.0.0.0");
    stop(child).await;

    // 再用 --host 127.0.0.1 起一次：启动期覆盖说了算，且来源要报 startup
    let bin = env!("CARGO_BIN_EXE_agent-pipeline");
    let mut child = Command::new(bin)
        .arg("serve")
        .arg("--port")
        .arg("0")
        .arg("--host")
        .arg("127.0.0.1")
        .env("AGENTPIPELINE_HOME", home.path())
        .env("RUST_LOG", "warn")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("二进制可启动");
    let stdout = child.stdout.take().unwrap();
    let port = tokio::time::timeout(Duration::from_secs(20), read_ready_port(stdout))
        .await
        .expect("就绪行");
    let (status, body) = try_http("GET", port, "/server-info", None).expect("服务可达");
    assert_eq!(status, 200, "{body}");
    assert_eq!(field(&body, "host"), "127.0.0.1", "{body}");
    assert_eq!(field(&body, "bind_source"), "startup", "{body}");
    stop(child).await;
}
