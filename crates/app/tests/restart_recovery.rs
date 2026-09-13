//! 真进程重启恢复（主流程票 08）：spawn **真二进制** → 任务运行中 `kill -9` →
//! **同一个 home** 重启 → 启动恢复（决策 127）接管 → 任务续跑到 done。
//!
//! 与 in-process 的 E2E-13（`tests/e2e/tests/crash_recovery.rs`）的分工：
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
