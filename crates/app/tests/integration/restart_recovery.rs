//! 真进程重启恢复（主流程票 08）：spawn **真二进制** → 任务运行中 `kill -9` →
//! **同一个 home** 重启 → 启动恢复（决策 127）接管 → 任务续跑到 done。
//!
//! 与 in-process 的 E2E-13（`tests/e2e/tests/integration/crash_recovery.rs`）的分工：
//! E2E-13 验**游标检查点语义**（快、决定性，决策 152 允许它不 spawn 真二进制）；
//! 本用例验**进程边界**——真实启动路径上的 `executor_owner` 清理、kill -9 留下的
//! 中间态（WAL / worktree / 分支）、服务能再次起来。两者互补，不互相替代；
//! 本票**不推翻决策 152**（它针对 E2E-13 的实现方式），只补它未覆盖的进程边界。

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::{Duration, Instant};

use agentpipeline_core::clock::SystemClock;
use agentpipeline_core::types::{
    AcceptanceCriterion, ArchitectExecuteMetadata, CodeChanges, DevelopDesignMetadata, Node,
    Project, Provider, ReviewResult, Stage, TestDesignMetadata, TestResult, TestScenario,
    ValidateInputMetadata, ValidateOutputMetadata,
};
use testkit::{MockLlm, Repo, Script, TestHome};
use tokio::process::{Child, Command};

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
            return rest
                .trim()
                .parse()
                .unwrap_or_else(|_| panic!("端口不可解析：{line}"));
        }
    }
    panic!("子进程在退出前未打印就绪行");
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

fn write_pipeline_config(home: &TestHome, tick_interval_sec: u64) {
    let cfg = format!("[pipeline]\ntick_interval_sec = {tick_interval_sec}\n");
    std::fs::write(home.home().config_path(), cfg).unwrap();
}

/// 与 smoke.rs 的 pipeline_script 同构（同测试二进制才能共享，此处独立成册）。
fn pipeline_script(script: &mut Script, task_id: &str) {
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    script
        .for_node(Stage::ArchitectDesign, Node::Execute)
        .write_file("design.md", "# 设计\n## 验收标准\n- AC-1 能登录\n")
        .submit(&ArchitectExecuteMetadata {
            readiness: true,
            affected_files: vec!["src/lib.rs".into()],
            new_symbols: vec![],
            acceptance_criteria: vec![AcceptanceCriterion {
                id: "AC-1".into(),
                description: "能登录".into(),
            }],
            ..Default::default()
        });
    script
        .for_node(Stage::ArchitectDesign, Node::ValidateOutput)
        .submit(&ValidateOutputMetadata {
            passed: true,
            ..Default::default()
        });
    script
        .for_node(Stage::DevelopDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    script
        .for_node(Stage::DevelopDesign, Node::Execute)
        .write_file("dev-plan.md", "# 开发计划\n")
        .submit(&DevelopDesignMetadata {
            readiness: true,
            dev_doc_path: Some("dev-plan.md".into()),
            ..Default::default()
        });
    script
        .for_node(Stage::DevelopDesign, Node::ValidateOutput)
        .submit(&ValidateOutputMetadata {
            passed: true,
            ..Default::default()
        });
    script
        .for_node(Stage::TestDesign, Node::ValidateInput)
        .submit(&ValidateInputMetadata {
            readiness: true,
            blockers: vec![],
        });
    script
        .for_node(Stage::TestDesign, Node::Execute)
        .write_file("test-scenarios.md", "# 测试场景\n")
        .submit(&TestDesignMetadata {
            readiness: true,
            blockers: vec![],
            test_scenarios_path: Some("test-scenarios.md".into()),
            test_scenarios: vec![TestScenario {
                id: "S-1".into(),
                name: "登录成功".into(),
                description: "登录".into(),
                preconditions: vec![],
                steps: vec![],
                expected_result: "成功".into(),
                priority: agentpipeline_core::types::ScenarioPriority::High,
                design_refs: vec!["AC-1".into()],
            }],
        });
    script
        .for_node(Stage::TestDesign, Node::ValidateOutput)
        .submit(&ValidateOutputMetadata {
            passed: true,
            ..Default::default()
        });
    script
        .for_node(Stage::Develop, Node::Execute)
        .write_file(
            "src/lib.rs",
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n#[test]\nfn adds() { assert_eq!(add(1, 2), 3); }\n",
        )
        .run_command(&format!(
            "git add -A && git -c user.name=f -c user.email=f@f commit -m 'feat: task {task_id}'"
        ))
        .submit(&CodeChanges {
            branch_name: format!("kanban/{task_id}"),
            changed_files: vec![],
            unit_test_files: vec![],
        });
    script
        .for_node(Stage::Review, Node::Execute)
        .write_file(
            "review-report.md",
            "# 评审报告\n## 设计符合性\n通过\n## 测试质量\n通过\n",
        )
        .submit(&ReviewResult {
            approved: true,
            review_report_path: Some("review-report.md".into()),
            required_changes: vec![],
        });
    script
        .for_node(Stage::Test, Node::Execute)
        .write_file("test-report.md", "# 测试报告\n全部通过\n")
        .submit(&TestResult {
            passed: true,
            test_report_path: Some("test-report.md".into()),
            failures: vec![],
            gate_recheck: false,
        });
}

/// 任务 JSON 里的整数字段提取（极简，避免引 serde_json 的值遍历样板）。
fn json_i64(body: &str, key: &str) -> Option<i64> {
    let pat = format!("\"{key}\":");
    let rest = body.split(&pat).nth(1)?;
    let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    num.parse().ok()
}

/// 与 testkit `mock_llm::node_for_system` 同逻辑的路由复刻（诊断用）。
fn route_like_mock(system: &str) -> String {
    use agentpipeline_core::agent::templates::system_template;
    const NODES: [(Stage, Node); 12] = [
        (Stage::ArchitectDesign, Node::ValidateInput),
        (Stage::ArchitectDesign, Node::Execute),
        (Stage::ArchitectDesign, Node::ValidateOutput),
        (Stage::DevelopDesign, Node::ValidateInput),
        (Stage::DevelopDesign, Node::Execute),
        (Stage::DevelopDesign, Node::ValidateOutput),
        (Stage::TestDesign, Node::ValidateInput),
        (Stage::TestDesign, Node::Execute),
        (Stage::TestDesign, Node::ValidateOutput),
        (Stage::Develop, Node::Execute),
        (Stage::Review, Node::Execute),
        (Stage::Test, Node::Execute),
    ];
    if system.contains("你是设计语义冲突比对 agent") {
        return "pseudo:conflict_check".into();
    }
    if system.contains("你是独立复核 agent") {
        return "pseudo:validator_cross_check".into();
    }
    NODES
        .iter()
        .copied()
        .find(|(s, n)| {
            let first = system_template(*s, *n).lines().next().unwrap_or_default();
            !first.is_empty() && system.contains(first)
        })
        .map(|(s, n)| format!("{s:?}/{n:?}"))
        .unwrap_or_else(|| "<未识别>".into())
}

#[tokio::test]
async fn kill_9_mid_run_then_restart_recovers_to_done() {
    let home = TestHome::new().unwrap();
    write_pipeline_config(&home, 1);
    let repo = Repo::clean().unwrap();

    let mut script = Script::new();
    // 脚本压两份：一次节点 run 恰消费一份（Submit 后由 mock 收尾文本终止 loop，
    // 见 testkit mock_llm::from_script），重启后中断节点的重放消费第二份。
    // 未被中断的节点只消费首份，第二份留在队列里，不影响断言。
    pipeline_script(&mut script, "t1");
    pipeline_script(&mut script, "t1");
    script
        .for_pseudo("pseudo:conflict_check")
        .submit_raw(serde_json::json!({"duplicate_risk": "low", "reason": null}));
    script
        .for_pseudo("pseudo:conflict_check")
        .submit_raw(serde_json::json!({"duplicate_risk": "low", "reason": null}));
    let mock = MockLlm::from_script(script).await;

    let store = home.store(Arc::new(SystemClock)).await.unwrap();
    let now = store.now();
    store
        .upsert_provider(&Provider {
            id: "prov".into(),
            vendor: "openai".into(),
            model: "mock".into(),
            context_window: 8000,
            base_url: Some(mock.url.clone()),
            api_key: Some("sk-test".into()),
            enabled: true,
            created_at: now,
            updated_at: now,
        })
        .await
        .unwrap();
    store
        .create_project(&Project {
            id: "p1".into(),
            name: "restart".into(),
            local_path: repo.path().display().to_string(),
            default_branch: "main".into(),
            language: None,
            test_framework: Some("true".into()),
            lint_command: None,
            agents_md_path: None,
            created_at: now,
        })
        .await
        .unwrap();
    testkit::seed_task(&store, "t1", "p1").await.unwrap();

    // ── 第一次启动：并行分支推进中（mock 已收到 DevelopDesign.Execute 请求）→ SIGKILL ──
    // 杀点落在 develop-design / test-design 并行窗口，覆盖票 08 的 waiting_join 跨重启：
    // 重启后两分支收齐，sync-check 的 join 必须恰执行一次。
    let (mut child, port) = spawn_server(&home).await;
    wait_until_ready(port).await;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        assert!(
            Instant::now() < deadline,
            "任务未推进到并行设计分支（mock 未收到 DevelopDesign.Execute 请求）"
        );
        let hit = mock.requests().await.iter().any(|r| {
            let system = serde_json::from_str::<serde_json::Value>(&r.body)
                .ok()
                .and_then(|v| v["messages"][0]["content"].as_str().map(String::from))
                .unwrap_or_default();
            route_like_mock(&system) == "DevelopDesign/Execute"
        });
        if hit {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // kill -9：无优雅退出，留下 executor_owner / WAL / worktree 等真实中间态
    child.start_kill().expect("kill -9");
    let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;

    // ── 重启：同一个 home，服务必须能起来（DB 可打开、迁移可跑）──
    let (_child2, port2) = spawn_server(&home).await;
    wait_until_ready(port2).await;

    // 启动恢复（决策 127）：executor_owner 残留被清理，任务被接管续跑。
    // 杀进程点在并行设计分支，故 init / architect 必不重跑；join 恰执行一次。
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut last = String::new();
    let mut approved = false;
    let mut reached_done = false;
    while Instant::now() < deadline {
        let (status, body) = http("GET", port2, "/tasks/t1", None).unwrap();
        last = body.clone();
        assert_ne!(status, 500, "服务端错误：{body}");
        if status == 200 && body.contains("merge_approval") && !approved {
            let (decide_status, decide_body) = http(
                "POST",
                port2,
                "/tasks/t1/merge/decision",
                Some(r#"{"decision":"approve"}"#),
            )
            .unwrap();
            assert_eq!(decide_status, 200, "{decide_body}");
            approved = true;
            continue;
        }
        if status == 200 && body.contains("\"status\":\"done\"") {
            reached_done = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // 诊断开关（flaky 排查留痕）：RESTART_RECOVERY_DEBUG=1 时打印 mock 请求路由分布
    if std::env::var("RESTART_RECOVERY_DEBUG").is_ok() {
        let requests = mock.requests().await;
        let mut by_node: std::collections::BTreeMap<String, usize> = Default::default();
        for r in &requests {
            let system = serde_json::from_str::<serde_json::Value>(&r.body)
                .ok()
                .and_then(|v| v["messages"][0]["content"].as_str().map(String::from))
                .unwrap_or_default();
            *by_node.entry(route_like_mock(&system)).or_default() += 1;
        }
        eprintln!(
            "──── mock 请求路由分布（{total} 个）────",
            total = requests.len()
        );
        for (k, v) in &by_node {
            eprintln!("  {v:3} × {k}");
        }
    }
    assert!(reached_done, "重启后未在 120s 内到终态：{last}");

    // ── 不重复劳动 + 无泄漏 ──
    let runs = store.list_runs("t1").await.unwrap();
    let init_runs = runs.iter().filter(|r| r.stage == Stage::Init).count();
    assert_eq!(init_runs, 1, "kill 前已完成的 init 不得重跑：{runs:?}");
    let sync_runs = runs
        .iter()
        .filter(|r| r.stage == Stage::SyncCheck && r.node == Node::Execute)
        .count();
    assert_eq!(sync_runs, 1, "并行分支重启后 join（sync-check）恰执行一次");
    // done 清理（决策 3）：worktree 与任务分支不残留、不重复创建
    assert!(
        !home.home().worktree_path("t1").exists(),
        "done 后 worktree 应清理"
    );
    assert!(!repo.branch_exists("kanban/t1"), "done 后任务分支应删除");
    assert_eq!(repo.head("main"), repo.head("main")); // 主干存在性冒烟
    let _ = json_i64(&last, "total_tokens").expect("任务计量字段存在");
}

/// 挂住的 provider：收下连接、发一帧正文就**再也不收尾**——不发 `[DONE]`、不关连接。
///
/// 与 `MockLlm` 的分工：那个按脚本回完整一轮，这个的用途是让**一轮永远悬在半空**——
/// 模型调用既不成功也不失败，直到进程被杀。这正是「kill -9 那一刻」的形状：收口那次
/// 写入永远不会发生，库里只剩一条 `status='in_flight'` 的半截行（票 03 的前置）。
///
/// 发一帧正文再挂住（而不是空等）：半截行因此**带着断在哪一步的字**被杀，重启后的
/// 断言才有「内容原样保留、只有状态变了」可做。
fn hang_provider() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("挂住的 provider 能绑定");
    let addr = listener.local_addr().expect("能取到地址");
    std::thread::spawn(move || {
        // 持住已发出的连接：不关 = 永远不到 EOF，调用方永远等不到收尾。
        let mut held = Vec::new();
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { break };
            let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
            let _ = s.set_write_timeout(Some(Duration::from_secs(10)));
            // 读掉请求（内容不看：只需要「有人来问过」这个事实）。
            let mut buf = [0u8; 8192];
            let _ = s.read(&mut buf);
            let chunk = serde_json::json!({
                "choices": [{"index": 0, "delta": {"role": "assistant", "content": "断电前说到这"}}]
            });
            let frame = format!("data: {chunk}\n\n");
            let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n";
            let _ = s.write_all(head.as_bytes());
            let _ = s.write_all(format!("{:x}\r\n", frame.len()).as_bytes());
            let _ = s.write_all(frame.as_bytes());
            let _ = s.write_all(b"\r\n");
            let _ = s.flush();
            held.push(s);
        }
    });
    format!("http://{addr}")
}

/// 票 03：**进程被杀形态**——在途轮悬在半空时 kill -9，重启后半截行标成已中断，
/// 界面不再显示「正在说」（显式修订决策 223：进程退出轮不落账 → 落一条已中断行）。
///
/// 与假时钟用例（core `startup_marks_a_hanging_inflight_row_interrupted…`）的分工：
/// 那条钉恢复步骤的**谓词与时刻来源**（存储层），这条钉**真实进程边界**上的整条链——
/// 真二进制、真 SIGKILL、同 home 重启、经 HTTP 观测（与本文件既有 kill -9 用例同姿势）。
///
/// 三条判据各钉一颗牙：
/// 1. 杀之前：半截行 `status='in_flight'`、`interrupted_at` 为空、`turn_in_flight=true`
///    ——断言 1 当场红，若「在途」在活着的进程里就显示不出来；
/// 2. 重启后：`status='interrupted'` + `interrupted_at` 有值 + `turn_in_flight=false`
///    ——启动恢复那一步被删掉则状态永远停在 `in_flight`，断言 2 当场红；
/// 3. 内容原样不动——改成连内容一起重写（或恢复时清字）则断言 3 红。
#[tokio::test]
async fn kill_9_mid_foreman_turn_marks_the_hanging_row_interrupted_on_restart() {
    let home = TestHome::new().unwrap();
    let hang_url = hang_provider();

    let store = home.store(Arc::new(SystemClock)).await.unwrap();
    let now = store.now();
    store
        .upsert_provider(&Provider {
            id: "prov".into(),
            vendor: "openai".into(),
            model: "mock".into(),
            context_window: 8000,
            base_url: Some(hang_url),
            api_key: Some("sk-test".into()),
            enabled: true,
            created_at: now,
            updated_at: now,
        })
        .await
        .unwrap();

    let (mut child, port) = spawn_server(&home).await;
    wait_until_ready(port).await;

    // ── 开班 ──
    let (status, body) = http(
        "POST",
        port,
        "/foreman/sessions",
        Some(r#"{"title":"中断班"}"#),
    )
    .unwrap();
    assert_eq!(status, 201, "{body}");
    let session_id = serde_json::from_str::<serde_json::Value>(&body)
        .expect("开班回包是 JSON")
        .get("session")
        .and_then(|s| s.get("id"))
        .and_then(|v| v.as_str())
        .expect("session.id 在场")
        .to_string();

    // ── 说一句话：handler 会把这一轮等到天荒地老（provider 挂着），放到独立线程里打 ──
    // 用 std 线程而不是 tokio::spawn：`http` 是阻塞的 std::net，在单线程测试运行时上
    // 会把轮询一起卡住。
    let say_session = session_id.clone();
    std::thread::spawn(move || {
        let payload = serde_json::json!({"text": "喂？", "session_id": say_session});
        let _ = http(
            "POST",
            port,
            "/foreman/messages",
            Some(&payload.to_string()),
        );
    });

    // ── 杀之前：半截行带着断点的字在库，这一轮算「在跑」──
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut last = String::new();
    let half = loop {
        assert!(
            Instant::now() < deadline,
            "半截行没在 15s 内出现（模型调用没发出去？最后回包：{last}）"
        );
        let (status, body) = http(
            "GET",
            port,
            &format!("/foreman/session?session={session_id}"),
            None,
        )
        .unwrap();
        assert_eq!(status, 200, "{body}");
        last = body.clone();
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        if let Some(hit) = v["messages"]
            .as_array()
            .and_then(|msgs| msgs.iter().find(|m| m["status"] == "in_flight"))
            .filter(|m| {
                m["content"]
                    .as_str()
                    .is_some_and(|c| c.contains("断电前说到这"))
            })
        {
            assert_eq!(
                v["turn_in_flight"],
                serde_json::json!(true),
                "活着的进程里这一轮算在跑：{body}"
            );
            assert!(
                hit["interrupted_at"].is_null(),
                "还没断，不该有中断时刻：{hit}"
            );
            break hit.clone();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };

    // ── kill -9：收口那次写入永远不会发生 ──
    child.start_kill().expect("kill -9");
    let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;

    // ── 同一个 home 重启：启动恢复必须在就绪之前标完（serve.rs：恢复 → READY 打印）──
    let (_child2, port2) = spawn_server(&home).await;
    wait_until_ready(port2).await;

    let (status, body) = http(
        "GET",
        port2,
        &format!("/foreman/session?session={session_id}"),
        None,
    )
    .unwrap();
    assert_eq!(status, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).expect("时间线回包是 JSON");
    assert_eq!(
        v["turn_in_flight"],
        serde_json::json!(false),
        "启动后不再显示「正在说」：{body}"
    );
    let msgs = v["messages"].as_array().expect("messages 在场");
    let row = msgs
        .iter()
        .find(|m| m["id"] == half["id"])
        .expect("半截行还在库里（中断是加状态，不是删行）");
    assert_eq!(
        row["status"], "interrupted",
        "悬挂行标成已中断（票 03，修订决策 223）：{row}"
    );
    assert!(
        row["interrupted_at"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "中断时刻记下了：{row}"
    );
    assert_eq!(
        row["content"], half["content"],
        "中断只加状态：正文是断在哪一步的证据，原样保留：{row}"
    );
}

// ─────────────────── 决策 257：界面保存的仓名单跨重启 ───────────────────

/// 界面保存的仓名单**跨真进程重启仍在**，且 `origin` 如实说是「界面」定的。
///
/// **为什么必须在真进程重启上验**：`serve.rs` 的启动路径读 DB 这一步，与 `PUT /market/repos`
/// 写 DB 那一步在**同一个进程里**是串起来的——`AppState` 的 `market_override` 当场就装上了，
/// 故进程内的用例（`market.rs::repos_config_is_a_two_level_override`）看不见「重启后读不读回」
/// 这件事。少了启动读回那一步，症状正是「保存完看着挺好，重启就回到配置文件那一级」。
///
/// **`origin` 也要一起验**：只要名单在、但 `origin` 说成 `config`，用户会以为「我保存的那份
/// 被配置文件盖掉了」——那是与真相相反的困惑，而这一页的存在意义就是回答「现在生效的是哪一份」。
///
/// 与 `server_bind_override` 同构（决策 186 / 213）：那个也是界面写 DB、启动读回，故这条形状
/// 不是为技能市场发明的。
#[tokio::test]
async fn settings_saved_market_repos_survive_a_real_restart() {
    let home = TestHome::new().unwrap();
    // `[market] github_repos` 是**另一级**，用来证明重启后生效的是界面那一份、不是它。
    std::fs::write(
        home.home().config_path(),
        "[market]\ngithub_repos = [\"config/level\"]\n",
    )
    .unwrap();

    // ── 第一次启动：保存一份界面名单 ──
    let (mut first, port) = spawn_server(&home).await;
    wait_until_ready(port).await;

    let (status, body) = http(
        "PUT",
        port,
        "/market/repos",
        Some(r#"{"repos":["Obra/Superpowers"]}"#),
    )
    .unwrap();
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("\"settings\""), "保存后是界面那一级：{body}");

    // ── 真重启：kill 掉、同一个 home 再起 ──
    first.kill().await.unwrap();
    first.wait().await.unwrap();
    let (mut second, port2) = spawn_server(&home).await;
    wait_until_ready(port2).await;

    let (status, body) = http("GET", port2, "/market/repos", None).unwrap();
    assert_eq!(status, 200, "{body}");
    assert!(
        body.contains("Obra/Superpowers"),
        "界面保存的名单必须跨重启存活（回归点：少了启动读回这一步，这里会变回配置文件那一级）：{body}"
    );
    assert!(
        !body.contains("config/level"),
        "生效的必须是界面那一份、不是 config.toml 那一份：{body}"
    );
    assert!(
        body.contains("\"origin\":\"settings\""),
        "origin 必须如实说是界面定的——说成 config 会让用户以为自己的保存被配置文件盖掉了：{body}"
    );

    // ── DELETE 之后重启：回落 `config.toml` 那一级 ──
    let (status, _) = http("DELETE", port2, "/market/repos", None).unwrap();
    assert_eq!(status, 200);
    second.kill().await.unwrap();
    second.wait().await.unwrap();
    let (mut third, port3) = spawn_server(&home).await;
    wait_until_ready(port3).await;

    let (status, body) = http("GET", port3, "/market/repos", None).unwrap();
    assert_eq!(status, 200, "{body}");
    assert!(
        body.contains("config/level"),
        "清掉界面那一级之后应回落到 config.toml：{body}"
    );
    assert!(
        body.contains("\"origin\":\"config\""),
        "回落之后 origin 应说是 config 定的：{body}"
    );
    third.kill().await.unwrap();
    third.wait().await.unwrap();
}
