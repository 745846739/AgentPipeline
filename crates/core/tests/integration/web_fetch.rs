//! L2 集成：`web_fetch`——受治理的只读网口（决策 266，票 foreman-capability-gaps 02）。
//!
//! 与 `egress.rs` 的分工：那一边钉策略本身与它接到 `run_command` 上的情形；本文件钉
//! **网口工具自己的**几件事，判据落在副作用与台账上（与 `readonly.rs` 同一取证手法）：
//!
//! 1. 回环放行时真的取回正文，且按**会话维度**落一行命令台账（GET 形态 + 预览 + 退出 0）；
//! 2. 未放行的远端主机在**发出请求之前**被策略拒（`PolicyDenied` + 台账留行带约定退出码，
//!    决策 179「被拒的也要留一行」）；
//! 3. 非回环的裸 `http` 被 scheme 判拒（https 才出环），留行但退出码留空
//!    （照 `refuse_readonly`：没跑起来就没有退出码）；
//! 4. 显式超时（`timeout_sec`，与 `run_readonly` 同姿态）打得到点上；
//! 5. 非文本 content-type 拒收——二进制不进对话上下文；
//! 6. 档位不管它（`deny` 也广告，判据沿决策 237），而值守轮的 deny 清单摘掉它与 `ask`
//!    （决策 266⑥ / 265）。
//!
//! 装置：手写 HTTP/1.1 回环服务器（照 `testkit::mock_llm` / `repo_fixture::SmartHttp`
//! 先例，无新依赖），绑 `127.0.0.1:0`、非阻塞 accept 逐连接一线程、Drop 即停。

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agentpipeline_core::agent::client::ToolCall;
use agentpipeline_core::agent::egress::EGRESS_DENIED_EXIT_CODE;
use agentpipeline_core::agent::tools::{ToolCallContext, ToolExecutor};
use agentpipeline_core::config::Settings;
use agentpipeline_core::pipeline::foreman::{
    foreman_available_tools, foreman_available_tools_except, FOREMAN_WATCH_TOOL_DENY,
};
use agentpipeline_core::types::{CommandSource, EnvMode, Node, Stage};
use agentpipeline_core::Error;
use testkit::{ManualClock, RecordingKiller, TestHome};

struct Fixture {
    _home: TestHome,
    store: agentpipeline_core::storage::Store,
    session_id: String,
}

async fn fixture() -> Fixture {
    let home = TestHome::new().unwrap();
    let store = agentpipeline_core::storage::Store::open(
        home.home().clone(),
        Arc::new(ManualClock::fixed()),
    )
    .await
    .unwrap();
    let session_id = store.create_foreman_session("网口").await.unwrap().id;
    Fixture {
        _home: home,
        store,
        session_id,
    }
}

impl Fixture {
    /// 与 `foreman_tooling` 同源的构造（域 = 家目录根 + 记录器接上）。
    fn executor(&self) -> ToolExecutor {
        ToolExecutor::new(
            self._home.home().clone(),
            agentpipeline_core::agent::file_policy::foreman_file_policy(self._home.home().root()),
            Settings::default(),
            Arc::new(RecordingKiller::new()),
        )
        .with_recorder(Arc::new(self.store.clone()))
        .with_env_mode(EnvMode::Auto)
    }

    fn ctx(&self) -> ToolCallContext {
        ToolCallContext {
            task_id: String::new(),
            session_id: Some(self.session_id.clone()),
            stage: Stage::Init,
            node: Node::Execute,
            worktree_path: self._home.home().root().to_path_buf(),
            task_dir: self._home.home().root().to_path_buf(),
            run_id: None,
            command_source: CommandSource::Agent,
            default_cwd: None,
        }
    }
}

fn fetch(url: &str) -> ToolCall {
    ToolCall {
        id: "c".into(),
        name: "web_fetch".into(),
        arguments: serde_json::json!({"url": url}).to_string(),
    }
}

fn fetch_with(url: &str, timeout_sec: u64) -> ToolCall {
    ToolCall {
        id: "c".into(),
        name: "web_fetch".into(),
        arguments: serde_json::json!({"url": url, "timeout_sec": timeout_sec}).to_string(),
    }
}

/// 台账里那一行（被拒的尝试也要有）。
async fn last_command(f: &Fixture) -> agentpipeline_core::types::NodeCommand {
    f.store
        .list_foreman_commands(&f.session_id)
        .await
        .unwrap()
        .pop()
        .expect("应当落了一行命令台账")
}

/// 手写 HTTP/1.1 服务器：按同一份应答回每一个连接，记录命中数与首行（方法行）。
struct TinyHttp {
    addr: std::net::SocketAddr,
    hits: Arc<AtomicUsize>,
    first_line: Arc<Mutex<String>>,
    alive: Arc<AtomicBool>,
}

impl TinyHttp {
    fn spawn(status: &str, content_type: &str, body: Vec<u8>, delay: Duration) -> Self {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let first_line = Arc::new(Mutex::new(String::new()));
        let alive = Arc::new(AtomicBool::new(true));
        let (h2, f2, a2) = (hits.clone(), first_line.clone(), alive.clone());
        let status = status.to_string();
        let ct = content_type.to_string();
        std::thread::spawn(move || {
            while a2.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut sock, _)) => {
                        h2.fetch_add(1, Ordering::SeqCst);
                        let mut buf = [0u8; 8192];
                        let _ = sock.set_read_timeout(Some(Duration::from_secs(2)));
                        let n = sock.read(&mut buf).unwrap_or(0);
                        let head = String::from_utf8_lossy(&buf[..n]).to_string();
                        if let Some(line) = head.lines().next() {
                            *f2.lock().unwrap() = line.to_string();
                        }
                        if !delay.is_zero() {
                            std::thread::sleep(delay);
                        }
                        let resp = format!(
                            "HTTP/1.1 {status}\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = sock.write_all(resp.as_bytes());
                        let _ = sock.write_all(&body);
                        let _ = sock.flush();
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });
        TinyHttp {
            addr,
            hits,
            first_line,
            alive,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    fn first_request_line(&self) -> String {
        self.first_line.lock().unwrap().clone()
    }
}

impl Drop for TinyHttp {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn a_loopback_fetch_returns_the_body_and_lands_in_the_session_ledger() {
    let f = fixture().await;
    let srv = TinyHttp::spawn(
        "200 OK",
        "text/html; charset=utf-8",
        "<html><body>你好，值班长</body></html>".as_bytes().to_vec(),
        Duration::ZERO,
    );
    let executor = f.executor();
    let ctx = f.ctx();

    let out = executor
        .execute(&fetch(&srv.url("/doc")), &ctx)
        .await
        .unwrap();
    assert!(out.content.contains("你好，值班长"), "{}", out.content);
    assert!(
        srv.first_request_line().starts_with("GET /doc"),
        "网口只发 GET：{}",
        srv.first_request_line()
    );

    let row = last_command(&f).await;
    assert!(row.command.starts_with("web_fetch "), "{}", row.command);
    assert!(
        row.command.contains(&srv.addr.ip().to_string()),
        "台账里要有目标：{}",
        row.command
    );
    assert_eq!(row.exit_code, Some(0), "{row:?}");
    assert!(
        row.stdout_preview
            .as_deref()
            .unwrap_or_default()
            .contains("值班长"),
        "预览要带上正文开头：{row:?}"
    );
    assert_eq!(
        row.session_id.as_deref(),
        Some(f.session_id.as_str()),
        "按会话维度归属（决策 204④ 的同一条口径）"
    );
    assert_eq!(row.task_id, None, "值班长的网口不挂任务");
}

#[tokio::test]
async fn an_unlisted_remote_host_is_denied_before_any_request() {
    let f = fixture().await;
    let executor = f.executor();
    let ctx = f.ctx();

    // 没有服务器可达：若请求真发出去了，错误会是 DNS / 连接类而不是策略拒绝。
    let err = executor
        .execute(&fetch("https://example.invalid/secret"), &ctx)
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::PolicyDenied(_)),
        "要在发出请求之前按策略拒：{err}"
    );
    assert!(
        err.to_string().contains("egress_allow_hosts"),
        "报错要说清怎么放行：{err}"
    );

    let row = last_command(&f).await;
    assert_eq!(
        row.exit_code,
        Some(EGRESS_DENIED_EXIT_CODE),
        "决策 179：策略拒绝也留行、带约定退出码：{row:?}"
    );
    assert!(
        row.stderr_preview
            .as_deref()
            .unwrap_or_default()
            .contains("egress_allow_hosts"),
        "审计面要能读到放行方式：{row:?}"
    );
}

#[tokio::test]
async fn plain_http_to_a_remote_host_is_refused() {
    let f = fixture().await;
    let executor = f.executor();
    let ctx = f.ctx();

    let err = executor
        .execute(&fetch("http://example.com/x"), &ctx)
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::PolicyDenied(_)),
        "校验类拒绝与只读取证同族（PolicyDenied）：{err}"
    );
    assert!(
        err.to_string().contains("https"),
        "说清要 https 才出环：{err}"
    );

    let row = last_command(&f).await;
    assert_eq!(
        row.exit_code, None,
        "没跑起来就没有退出码（refuse_readonly 同口径）：{row:?}"
    );
    assert!(
        row.stderr_preview
            .as_deref()
            .unwrap_or_default()
            .contains("https"),
        "台账里也要读得到拒因：{row:?}"
    );
}

#[tokio::test]
async fn an_explicit_timeout_aborts_a_slow_response() {
    let f = fixture().await;
    let srv = TinyHttp::spawn(
        "200 OK",
        "text/plain",
        "迟来的正文".as_bytes().to_vec(),
        Duration::from_secs(4),
    );
    let executor = f.executor();
    let ctx = f.ctx();

    let err = executor
        .execute(&fetch_with(&srv.url("/slow"), 1), &ctx)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("超时"),
        "超时要说清是超时（与连不上分得开）：{err}"
    );

    let row = last_command(&f).await;
    assert_eq!(row.exit_code, Some(1), "取数失败按失败记：{row:?}");
    assert!(
        row.stderr_preview
            .as_deref()
            .unwrap_or_default()
            .contains("超时"),
        "台账里也要读得到超时：{row:?}"
    );
    assert_eq!(
        srv.hits.load(Ordering::SeqCst),
        1,
        "服务器确实被请求过（等的是它不回话）"
    );
}

#[tokio::test]
async fn a_binary_content_type_is_refused() {
    let f = fixture().await;
    let srv = TinyHttp::spawn(
        "200 OK",
        "application/pdf",
        b"%PDF-1.4 not text".to_vec(),
        Duration::ZERO,
    );
    let executor = f.executor();
    let ctx = f.ctx();

    let err = executor
        .execute(&fetch(&srv.url("/doc.pdf")), &ctx)
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::PolicyDenied(_)),
        "非文本与校验拒绝同族：{err}"
    );
    assert!(err.to_string().contains("文本"), "说清只收文本类：{err}");

    let row = last_command(&f).await;
    assert_eq!(row.exit_code, None, "{row:?}");
    assert_eq!(
        srv.hits.load(Ordering::SeqCst),
        1,
        "请求发出去了才看得见 content-type——这正是它与 scheme 拒绝的区别"
    );
}

#[tokio::test]
async fn the_tiers_leave_it_alone_but_the_watch_deny_list_strips_it() {
    // 档位不管网口（决策 266⑤：治理在白名单不在档位，判据沿 237）；
    // 值守轮两样都不给（265 的「不问人」+ 266 的「夜间外发无人盯」）。
    for mode in [EnvMode::Auto, EnvMode::Ask, EnvMode::Deny] {
        let avail = foreman_available_tools(mode);
        assert!(
            avail.contains(&"web_fetch"),
            "{mode:?} 下都要广告 web_fetch：{avail:?}"
        );
        assert!(avail.contains(&"ask"), "{mode:?} 下都要广告 ask：{avail:?}");
    }
    let watch = foreman_available_tools_except(EnvMode::Ask, &FOREMAN_WATCH_TOOL_DENY);
    assert!(
        !watch.contains(&"web_fetch"),
        "值守轮不外发（决策 266⑥）：{watch:?}"
    );
    assert!(
        !watch.contains(&"ask"),
        "值守轮不问人（决策 265）：{watch:?}"
    );
}
