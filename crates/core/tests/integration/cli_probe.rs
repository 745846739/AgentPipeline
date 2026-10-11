//! L2 集成：CLI provider 探针（决策 423⑤，票 talk-cli-engine 01）。
//!
//! 探测对象是真本机二进制，**不可依赖**（开发机装了 claude、CI 没装），故五条
//! 路径全部经 `probe_program_with_runner` 测试缝指到假脚本。注意缝必须在**程序
//! 路径**上：`with_path_prefix` 只改子进程 env 的 `PATH`，影子不到 Argv 形态的
//! 程序名解析（父进程 PATH）——前置目录方案会跑到本机真 `claude`，绿红全漂。
//! 「二进制缺失」直接指一个不存在的绝对路径（与 PATH 无此名同一 spawn 失败收口）。

use std::path::Path;
use std::sync::Arc;

use agentpipeline_core::agent::providers::cli::{probe_program_with_runner, CLI_BINARY};
use agentpipeline_core::agent::providers::ConnectionTest;
use agentpipeline_core::exec::CommandRunner;
use agentpipeline_core::types::Provider;
use testkit::TestHome;

fn fake_bin(dir: &Path, body: &str) -> std::path::PathBuf {
    let p = dir.join(CLI_BINARY);
    std::fs::write(&p, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    p
}

fn provider() -> Provider {
    Provider {
        id: "p-cli".into(),
        vendor: "cli".into(),
        model: String::new(),
        context_window: 200_000,
        base_url: None,
        api_key: None,
        enabled: true,
        created_at: Default::default(),
        updated_at: Default::default(),
    }
}

async fn probe_program(_home: &TestHome, program: &Path) -> ConnectionTest {
    let program = program.to_str().unwrap();
    let runner = CommandRunner::new(Arc::new(testkit::RecordingKiller::default()));
    probe_program_with_runner(&runner, program, &provider()).await
}

async fn probe_fake(home: &TestHome, body: &str) -> ConnectionTest {
    let dir = home.home().root().join("bin");
    std::fs::create_dir_all(&dir).unwrap();
    let program = fake_bin(&dir, body);
    probe_program(home, &program).await
}

#[tokio::test]
async fn ready_version_reports_ok_with_number() {
    let home = TestHome::new().unwrap();
    let t = probe_fake(&home, "#!/bin/sh\necho \"2.1.6 (Claude Code)\"\n").await;
    assert!(t.ok, "应绿：{t:?}");
    assert_eq!(t.kind, None);
    assert!(t.message.contains("2.1.6"), "读数带版本号：{t:?}");
}

#[tokio::test]
async fn missing_binary_has_determined_readout() {
    let home = TestHome::new().unwrap();
    // 不存在的绝对路径：spawn 失败 → exit_code None，与「PATH 无此名」同一收口路径。
    let t = probe_program(&home, &home.home().root().join("no-such-claude")).await;
    assert!(!t.ok);
    assert_eq!(t.kind.as_deref(), Some("cli_binary_missing"));
    assert!(t.message.contains("claude"), "读数点名二进制：{t:?}");
}

#[tokio::test]
async fn out_of_range_version_is_rejected_not_guessed() {
    let home = TestHome::new().unwrap();
    let t = probe_fake(&home, "#!/bin/sh\necho \"1.5.0 legacy\"\n").await;
    assert!(!t.ok);
    assert_eq!(t.kind.as_deref(), Some("cli_version_unsupported"));
    assert!(t.message.contains("1.5.0"), "读数带实探版本：{t:?}");
    assert!(t.message.contains("2.0.0"), "读数带范围下界：{t:?}");
    assert!(
        t.message.contains("5.0.0"),
        "读数带范围上界（人话形态）：{t:?}"
    );
}

#[tokio::test]
async fn unparsable_output_fails_fast_with_raw() {
    let home = TestHome::new().unwrap();
    let t = probe_fake(&home, "#!/bin/sh\necho \"hello world\"\n").await;
    assert!(!t.ok);
    assert_eq!(t.kind.as_deref(), Some("cli_version_unparsable"));
    assert_eq!(t.raw.as_deref(), Some("hello world"), "raw 留诊断原文");
}

#[tokio::test]
async fn nonzero_exit_carries_stderr_preview() {
    let home = TestHome::new().unwrap();
    let t = probe_fake(&home, "#!/bin/sh\necho \"boom\" >&2\nexit 3\n").await;
    assert!(!t.ok);
    assert_eq!(t.kind.as_deref(), Some("cli_probe_failed"));
    assert_eq!(t.raw.as_deref(), Some("boom"));
}
