//! CLI agent provider（决策 423：provider `kind=cli`，本机整段执行器）。
//!
//! 本文件一期只承载**探测面**（票 talk-cli-engine 01）：`--version` 探测 + 兼容判定。
//! CLI 不走 [`super::Adapter`] 的 HTTP/SSE 通道——它是自带工具循环的整段执行器，
//! [`super::adapter_family`] 对 `cli` 显式报错，防流水线 LLM 阶段静默选中它；
//! 消费面（对讲台会话回路）在票 02 起。
//!
//! 探测走 [`crate::exec`] 收口（决策 297）：argv 直出（决策 232——程序名与参数是
//! 两个独立的数组元素，不过 shell），`--version` 是只读调用，无写面。

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use super::ConnectionTest;
use crate::agent::tools::CommandFinish;
use crate::exec::{CommandOutput, CommandOwner, CommandRequest, CommandRunner, Rewrite, SpawnForm};
use crate::process::RealProcessKiller;
use crate::types::{CommandSource, Node, Provider, Stage};

/// CLI provider 的 vendor 值（决策 423②）。
pub const CLI_VENDOR: &str = "cli";

/// 一期的 CLI 引擎二进制（决策 423⑥：一期只做 claude code）。
pub const CLI_BINARY: &str = "claude";

/// 已知兼容版本范围，**含界**（决策 423⑤ 三件套的第一件：探测版本号，超界拒启）。
///
/// 暂以代码常量承载：provider 级可配（票 01 原文「存 provider 配置」）等第二个
/// CLI 引擎进场时随 `CliAgent` 抽象一起收口——为一档引擎先建列是过早的表结构。
/// 范围刻意取宽（2.x – 4.x）：它的职责是「版本漂移时给确定的红」，不是精确约束。
pub const CLI_VERSION_MIN: (u64, u64, u64) = (2, 0, 0);
pub const CLI_VERSION_MAX: (u64, u64, u64) = (4, u64::MAX, u64::MAX);

/// 探针超时：`--version` 是本地调用，15s 绰绰有余（与 HTTP 探针同档的「防挂死」语义）。
const PROBE_TIMEOUT_SEC: u64 = 15;

/// 从 `claude --version` 的输出解析 semver 三元组。
///
/// 宽容（决策 423⑤：解析宽容）：跳过行首的非数字前缀（`claude 2.1.6` / `v2.1.6`），
/// 补丁位容忍尾巴（`2.1.6 (Claude Code)` / `2.1.6-rc.1`）。解析不出 → `None`，
/// 由调用方给确定读数，**不猜**。
pub fn parse_cli_version(output: &str) -> Option<(u64, u64, u64)> {
    let first = output.lines().next()?.trim();
    let line = first.trim_start_matches(|c: char| !c.is_ascii_digit());
    if line.is_empty() {
        return None;
    }
    let mut it = line.splitn(3, '.');
    let major = leading_num(it.next()?)?;
    let minor = leading_num(it.next()?)?;
    let patch = leading_num(it.next()?)?;
    Some((major, minor, patch))
}

/// 前导数字段：`"6 (Claude Code)"` → `6`；空数字段 → `None`。
fn leading_num(s: &str) -> Option<u64> {
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

/// 兼容判定，范围**含界**：超界即拒，不猜「大概能跑」（决策 423⑤）。
pub fn cli_version_compatible(version: (u64, u64, u64)) -> bool {
    version >= CLI_VERSION_MIN && version <= CLI_VERSION_MAX
}

/// CLI provider 探针（决策 160 的 CLI 分支）：真跑一次 `{claude} --version`。
///
/// 失败四路各有确定读数（票 01 验收）：
/// - 启动不了（PATH 无此二进制）→ `cli_binary_missing`；
/// - 超时 → `cli_probe_timeout`；
/// - 退出码非零 → `cli_probe_failed`（带 stderr 预览）；
/// - 输出解析不出版本 / 超出兼容范围 → `cli_version_unparsable` / `cli_version_unsupported`。
pub async fn probe_cli_provider(provider: &Provider) -> ConnectionTest {
    let runner = CommandRunner::new(Arc::new(RealProcessKiller));
    probe_program_with_runner(&runner, CLI_BINARY, provider).await
}

/// 测试缝：runner 与**程序路径**都由调用点给。
///
/// 为什么缝在程序路径而不是 PATH 前置：`with_path_prefix` 只改**子进程** env 的
/// `PATH`，而 Argv 形态的程序名解析走**父进程** PATH（tokio/std 的 execvp 语义）——
/// 前置目录影子不到 Argv 的名字解析，测试里会跑到本机真二进制。显式传绝对路径
/// 则与解析机制无关，绿红都确定。
pub async fn probe_program_with_runner(
    runner: &CommandRunner,
    program: &str,
    _provider: &Provider,
) -> ConnectionTest {
    let started = Instant::now();
    let latency = || started.elapsed().as_millis() as u64;
    let args: [String; 1] = ["--version".to_string()];
    let req = CommandRequest {
        owner: CommandOwner {
            task_id: None,
            session_id: None,
            run_id: None,
            stage: Stage::Init,
            node: Node::Execute,
            source: CommandSource::System,
        },
        command: "claude --version",
        cwd: Path::new("."),
        timeout_sec: PROBE_TIMEOUT_SEC,
        spawn: SpawnForm::Argv {
            program,
            args: &args,
        },
        rewrite: Rewrite::None,
    };
    let outcome = runner
        .run(req, |out: &CommandOutput| {
            Ok((
                CommandFinish {
                    exit_code: out.exit_code,
                    stdout_path: None,
                    stdout_preview: None,
                    stderr_preview: None,
                    duration_ms: out.duration_ms,
                },
                out.clone(),
            ))
        })
        .await;
    let output = match outcome {
        Ok((_, out)) => out,
        Err(e) => {
            return ConnectionTest {
                ok: false,
                latency_ms: latency(),
                kind: Some("cli_probe_failed".into()),
                message: format!("CLI 探测执行失败：{e}"),
                raw: None,
            }
        }
    };
    if output.timed_out {
        return ConnectionTest {
            ok: false,
            latency_ms: latency(),
            kind: Some("cli_probe_timeout".into()),
            message: format!("`{CLI_BINARY} --version` 超过 {PROBE_TIMEOUT_SEC} 秒未返回"),
            raw: None,
        };
    }
    let Some(code) = output.exit_code else {
        return ConnectionTest {
            ok: false,
            latency_ms: latency(),
            kind: Some("cli_binary_missing".into()),
            message: format!(
                "探测不到本机命令 `{CLI_BINARY}`：PATH 中不存在或无法启动。请先安装并登录 claude code（它用本机登录态，无需在此配密钥）"
            ),
            raw: None,
        };
    };
    if code != 0 {
        return ConnectionTest {
            ok: false,
            latency_ms: latency(),
            kind: Some("cli_probe_failed".into()),
            message: format!("`{CLI_BINARY} --version` 退出码 {code}"),
            raw: Some(
                preview(&output.stderr)
                    .or_else(|| preview(&output.stdout))
                    .unwrap_or_default(),
            ),
        };
    }
    let Some(version) = parse_cli_version(&output.stdout) else {
        return ConnectionTest {
            ok: false,
            latency_ms: latency(),
            kind: Some("cli_version_unparsable".into()),
            message: "版本输出解析不出 semver（CLI 版本漂移？fail-fast 不猜，请核对 claude 版本）"
                .into(),
            raw: Some(preview(&output.stdout).unwrap_or_default()),
        };
    };
    if !cli_version_compatible(version) {
        return ConnectionTest {
            ok: false,
            latency_ms: latency(),
            kind: Some("cli_version_unsupported".into()),
            message: format!(
                "CLI 版本 {} 超出兼容范围（{} 起、{} 前）",
                fmt_version(version),
                fmt_version(CLI_VERSION_MIN),
                fmt_upper_bound(CLI_VERSION_MAX),
            ),
            raw: Some(preview(&output.stdout).unwrap_or_default()),
        };
    }
    ConnectionTest {
        ok: true,
        latency_ms: latency(),
        kind: None,
        message: format!("本机 CLI 就绪：{} {}", CLI_BINARY, fmt_version(version)),
        raw: None,
    }
}

/// 版本的人话形态：`1.5.0`——失败读数是给人看的，不出现元组 Debug。
fn fmt_version(v: (u64, u64, u64)) -> String {
    format!("{}.{}.{}", v.0, v.1, v.2)
}

/// 兼容范围的**上界人话**：哨兵 `(4, u64::MAX, u64::MAX)` 读作「5.0.0 前」；
/// 非 full-max 的补丁位按实值打印（范围定义变了读数跟着变，不写死文案）。
fn fmt_upper_bound(max: (u64, u64, u64)) -> String {
    if max.1 == u64::MAX && max.2 == u64::MAX {
        format!("{}.0.0", max.0 + 1)
    } else {
        fmt_version(max)
    }
}

fn preview(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.chars().take(300).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_semver_line() {
        assert_eq!(parse_cli_version("2.1.6 (Claude Code)\n"), Some((2, 1, 6)));
    }

    #[test]
    fn parses_prefixed_and_suffixed_lines() {
        assert_eq!(parse_cli_version("claude 2.10.3"), Some((2, 10, 3)));
        assert_eq!(parse_cli_version("v2.1.6-rc.1"), Some((2, 1, 6)));
        assert_eq!(
            parse_cli_version("2.1.6 (Claude Code)"),
            Some((2, 1, 6)),
            "补丁位的尾巴不参与"
        );
    }

    #[test]
    fn unparsable_output_is_none_not_a_guess() {
        assert_eq!(parse_cli_version(""), None);
        assert_eq!(parse_cli_version("no digits here"), None);
        assert_eq!(parse_cli_version("2.x"), None);
        assert_eq!(parse_cli_version("2"), None);
    }

    #[test]
    fn compat_boundaries_are_inclusive() {
        // 边界值钉死：2.0.0 含、1.9.9 拒、4.9.9 含、4 后第一个整数拒。
        assert!(cli_version_compatible(CLI_VERSION_MIN), "下界含");
        assert!(cli_version_compatible(CLI_VERSION_MAX), "上界含");
        assert!(!cli_version_compatible((1, 9, 999)), "低于下界拒");
        assert!(!cli_version_compatible((5, 0, 0)), "高于上界拒");
    }
}
