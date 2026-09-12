//! E2E-00 冒烟（testing.md §8、决策 54 / 56）：spawn **真二进制**。
//!
//! 覆盖：启动 → 服务可达 → 未配置 provider 时创建任务明确报错 → SIGINT 优雅退出。
//!
//! 放在 app 包内是因为 `CARGO_BIN_EXE_agent-pipeline` 只对同包测试可见；
//! 其余 L4 场景（FakeAgent 驱动）在 `tests/e2e`。

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use testkit::TestHome;
use tokio::process::{Child, Command};

/// 极简 HTTP/1.1 客户端（冒烟只需读状态码与 body，不引 reqwest）。
fn http(method: &str, port: u16, path: &str, body: Option<&str>) -> std::io::Result<(u16, String)> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let body = body.unwrap_or("");
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes())?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    let status = response
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    Ok((status, body))
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

async fn spawn_server(port: u16, home: &TestHome) -> Child {
    let bin = env!("CARGO_BIN_EXE_agent-pipeline");
    Command::new(bin)
        .arg("serve")
        .arg("--port")
        .arg(port.to_string())
        .env("AGENTPIPELINE_HOME", home.path())
        .env("RUST_LOG", "warn")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("二进制可启动")
}

async fn wait_until_ready(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if let Ok((200, _)) = http("GET", port, "/metrics", None) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("服务未在 20s 内就绪");
}

fn send_sigint(pid: u32) {
    let status = std::process::Command::new("kill")
        .arg("-INT")
        .arg(pid.to_string())
        .status()
        .expect("kill 可执行");
    assert!(status.success(), "发送 SIGINT 失败");
}

#[tokio::test]
async fn binary_starts_serves_and_exits_gracefully_on_sigint() {
    let home = TestHome::new().unwrap();
    let port = free_port();
    let mut child = spawn_server(port, &home).await;
    wait_until_ready(port).await;

    // 家目录骨架按 §12.14 建立，且数据库落在临时 home 内（不碰真实 ~/.agentpipeline）
    assert!(home.home().db_path().exists(), "数据库应建在临时 home");
    assert!(home.home().tasks_dir().exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(home.home().data_dir())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700, "data 目录权限应为 0700（§12.14）");
    }

    // GET /providers 在无 provider 时返回空列表
    let (status, body) = http("GET", port, "/providers", None).unwrap();
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("\"providers\":[]") || body.contains("\"providers\": []"));

    // 建一个真 git 仓库作为项目
    let repo = home.scratch_dir("repo");
    for args in [
        vec!["init", "-b", "main"],
        vec!["config", "user.name", "smoke"],
        vec!["config", "user.email", "smoke@localhost"],
    ] {
        let out = std::process::Command::new("git")
            .args(&args)
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(out.status.success());
    }
    std::fs::write(repo.join("README.md"), "# smoke\n").unwrap();
    for args in [vec!["add", "-A"], vec!["commit", "-m", "init"]] {
        let out = std::process::Command::new("git")
            .args(&args)
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(out.status.success());
    }

    let (status, body) = http(
        "POST",
        port,
        "/projects",
        Some(&format!(
            r#"{{"name":"smoke","local_path":"{}"}}"#,
            repo.display()
        )),
    )
    .unwrap();
    assert_eq!(status, 201, "{body}");
    let project_id = body
        .split("\"id\":\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .expect("返回 project id")
        .to_string();

    // 决策 56：未配置 provider 时创建任务返回明确错误
    let (status, body) = http(
        "POST",
        port,
        "/tasks",
        Some(&format!(
            r#"{{"project_id":"{project_id}","title":"冒烟任务"}}"#
        )),
    )
    .unwrap();
    assert_eq!(status, 400, "应明确报错而不是 500：{body}");
    assert!(body.contains("provider"), "{body}");

    // 决策 54：第一次 SIGINT → 优雅退出（退出码 0）
    let pid = child.id().expect("有 pid");
    send_sigint(pid);
    let exited = tokio::time::timeout(Duration::from_secs(10), child.wait()).await;
    let status = match exited {
        Ok(result) => result.expect("正常回收"),
        Err(_) => {
            let _ = child.kill().await;
            panic!("SIGINT 后未在 10s 内退出");
        }
    };
    assert!(status.success(), "优雅退出应为 0，实际 {:?}", status.code());
}

#[tokio::test]
async fn second_binary_on_same_port_fails_fast_with_clear_error() {
    let home = TestHome::new().unwrap();
    let port = free_port();
    let mut first = spawn_server(port, &home).await;
    wait_until_ready(port).await;

    let bin = env!("CARGO_BIN_EXE_agent-pipeline");
    let second = Command::new(bin)
        .arg("serve")
        .arg("--port")
        .arg(port.to_string())
        .env("AGENTPIPELINE_HOME", home.path())
        .output()
        .await
        .unwrap();
    assert!(!second.status.success(), "端口占用应启动失败");
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        stderr.contains("端口被占用或无法绑定"),
        "错误信息应明确：{stderr}"
    );

    let pid = first.id().unwrap();
    send_sigint(pid);
    let _ = tokio::time::timeout(Duration::from_secs(10), first.wait()).await;
}
