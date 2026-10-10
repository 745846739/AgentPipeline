//! E2E-00 冒烟（testing.md §8、决策 54 / 56）：spawn **真二进制**。
//!
//! 覆盖：启动 → 服务可达 → 未配置 provider 时创建任务明确报错 → SIGINT 优雅退出。
//!
//! 放在 app 包内是因为 `CARGO_BIN_EXE_agent-pipeline` 只对同包测试可见；
//! 其余 L4 场景（FakeAgent 驱动）在 `tests/e2e`。

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::{Duration, Instant};

use agentpipeline_core::clock::SystemClock;
use agentpipeline_core::types::{
    AcceptanceCriterion, ArchitectExecuteMetadata, CodeChanges, DevelopDesignMetadata, FileAction,
    FileChangeSpec, Node, Project, Provider, ReviewResult, Stage, TestDesignMetadata, TestResult,
    TestScenario, ValidateInputMetadata, ValidateOutputMetadata,
};
use testkit::{MockLlm, Repo, Script, TestHome};
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

/// 启动真二进制并以 `--port 0` 绑定，读回内核分配的真实端口（决策 153⑤）。
///
/// 取代原先「先探测空闲端口再释放」的 `free_port()`：探测-释放-再绑定之间存在
/// 竞争窗口，端口可能被别的进程抢走。`--port 0` 由内核原子分配，就绪行给出真值。
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
        // 用例中途 panic（断言失败 / 超时）时 drop 掉 Child 即回收子进程，
        // 否则会留下仍在监听随机端口的孤儿服务，且其临时 home 已被删除。
        .kill_on_drop(true)
        .spawn()
        .expect("二进制可启动");

    let stdout = child.stdout.take().expect("stdout 已管道化");
    let port = tokio::time::timeout(Duration::from_secs(20), read_ready_port(stdout))
        .await
        .expect("20s 内应打印就绪行（含真实端口）");
    (child, port)
}

/// 从子进程 stdout 读就绪行 `AGENTPIPELINE_READY port=<n>`，返回真实端口。
async fn read_ready_port(stdout: tokio::process::ChildStdout) -> u16 {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut lines = BufReader::new(stdout).lines();
    while let Some(line) = lines.next_line().await.transpose() {
        let line = line.expect("读取子进程 stdout");
        if let Some(rest) = line.trim().strip_prefix("AGENTPIPELINE_READY port=") {
            return rest
                .trim()
                .parse::<u16>()
                .unwrap_or_else(|_| panic!("就绪行端口不可解析：{line}"));
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
    let (mut child, port) = spawn_server(&home).await;
    wait_until_ready(port).await;

    // 决策 155：单二进制同源托管前端——内嵌时回 index.html，未内嵌时回构建提示页
    let (status, body) = http("GET", port, "/", None).unwrap();
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("AgentPipeline"), "{body}");

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
            r#"{{"project_id":"{project_id}","title":"冒烟任务","description":"冒烟：跑通主流程"}}"#
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

/// 写一份启动即用的 pipeline 配置（缩短 tick 周期，让冒烟在秒级看到准入）。
fn write_pipeline_config(home: &TestHome, tick_interval_sec: u64) {
    let cfg = format!("[pipeline]\ntick_interval_sec = {tick_interval_sec}\n");
    std::fs::write(home.home().config_path(), cfg).unwrap();
}

/// 与 E2E-01 同构的最小闭环脚本：真实二进制经 mock LLM 走完
/// init → architect → 并行设计 → sync-check → develop → review → test → merge → done。
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
            changed_files: vec![FileChangeSpec {
                path: "src/lib.rs".into(),
                action: FileAction::Create,
                content_hash: None,
            }],
            unit_test_files: vec![],
            no_changes: false,
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
            ..Default::default()
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

/// 真实二进制端到端（票 17 验收）：创建任务 → tick 循环自动准入 → executor
/// 经 mock LLM 推进到终态 done。mock 只替换 LLM 响应流，工具 / git / 命令全真跑。
#[tokio::test]
async fn real_binary_advances_task_to_terminal_with_mock_llm() {
    let home = TestHome::new().unwrap();
    write_pipeline_config(&home, 1);
    let repo = Repo::clean().unwrap();

    let mut script = Script::new();
    pipeline_script(&mut script, "t1");
    // 伪阶段（票 16）：语义冲突比对给 low，单任务不进入 conflict_wait
    script
        .for_pseudo("pseudo:conflict_check")
        .submit_raw(serde_json::json!({"duplicate_risk": "low", "reason": null}));
    let mock = MockLlm::from_script(script).await;

    // 预置 DB（同库同迁移）：provider 指向 mock，项目指真实 git 仓库
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
            name: "smoke".into(),
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

    let (mut child, port) = spawn_server(&home).await;
    wait_until_ready(port).await;

    // tick（1s）准入 → resume 钩子拉起 executor → mock 驱动到 done。
    // agent 评审模式在 merge 阶段 A 后按设计停在 pending(merge_approval)，
    // 需经端点审批（决策 119）才走阶段 B 合入。
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut last = String::new();
    let mut approved = false;
    loop {
        assert!(
            Instant::now() < deadline,
            "任务未在 120s 内到终态；最后响应：{last}"
        );
        let (status, body) = http("GET", port, "/tasks/t1", None).unwrap();
        last = body.clone();
        if status == 200 && body.contains("\"status\":\"pending\"") && !approved {
            assert!(
                body.contains("merge_approval"),
                "应在 merge_approval 暂停：{body}"
            );
            let (decide_status, decide_body) = http(
                "POST",
                port,
                "/tasks/t1/merge/decision",
                Some(r#"{"decision":"approve"}"#),
            )
            .unwrap();
            assert_eq!(decide_status, 200, "审批应成功：{decide_body}");
            approved = true;
        }
        if status == 200 && body.contains("\"status\":\"done\"") {
            break;
        }
        assert!(
            !body.contains("\"status\":\"failed\"") && !body.contains("\"status\":\"cancelled\""),
            "任务不应失败/取消：{body}"
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    assert!(approved, "应经过 merge_approval 审批");

    // 节点 run 落库（证明 executor 真跑，而非仅准入后停住）
    let runs = store.list_runs("t1").await.unwrap();
    assert!(runs.len() >= 10, "应有完整节点 run 行：{}", runs.len());

    send_sigint(child.id().unwrap());
    let _ = tokio::time::timeout(Duration::from_secs(10), child.wait()).await;
}

#[tokio::test]
async fn second_binary_on_same_port_fails_fast_with_clear_error() {
    let home = TestHome::new().unwrap();
    let (mut first, port) = spawn_server(&home).await;
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
