//! 可测试性接缝③：进程组终止器（决策 143 / 66）。
//!
//! 超时路径必须断言"杀了进程组"，但测试里没有真进程可杀。把终止动作抽成 trait：
//! 生产实现真杀，测试实现只记录调用。

use std::path::Path;

use crate::Result;

pub trait ProcessKiller: Send + Sync + 'static {
    /// 向进程组发送终止信号（先 TERM，必要时 KILL）。
    fn kill_process_group(&self, pgid: i32) -> Result<()>;
}

/// 在**独立进程组**里启动 `sh -c <command>`（决策 66 / 票 17）。
///
/// Unix 下 `process_group(0)` 让子进程成为新进程组组长，于是
/// `child.id()` 即进程组 id（pgid）——超时时 `kill(-pgid)` 能连子孙进程一起收。
/// 不引 libc：`tokio::process::Command::process_group` 内部走
/// `std::os::unix::process::CommandExt::process_group`。
#[cfg(unix)]
pub fn spawn_in_own_process_group(
    command: &str,
    cwd: &Path,
) -> std::io::Result<tokio::process::Child> {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg(command);
    spawn_with_stdio_and_group(cmd, cwd)
}

/// 在**独立进程组**里按 **argv 直出**启动一个命令（决策 232 / 237）：不经 `sh`。
///
/// 与 [`spawn_in_own_process_group`] 的差别只有一处，而那一处就是安全面的全部：
/// 命令名与参数是两个独立的数组元素，没有一层 shell 去解释分号、管道、`$(...)`。
/// 分层诊断的白名单命令（`date` / `ps` / `pgrep` / `lsof` / `wc` / `tail` / `sample`）
/// 走这条，故「按命令名判定」这句话才有落点——`sh -c "date; rm -rf x"` 的命令名是 `sh`。
#[cfg(unix)]
pub fn spawn_argv_in_own_process_group(
    program: &str,
    args: &[String],
    cwd: &Path,
) -> std::io::Result<tokio::process::Child> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args);
    spawn_with_stdio_and_group(cmd, cwd)
}

/// stdio + 独立进程组的共同配置（`spawn()` 不自动接管 stdio，必须显式管道化，
/// 否则读回的是空内容；`process_group(0)` = 以自身 pid 新建进程组）。
#[cfg(unix)]
fn spawn_with_stdio_and_group(
    mut cmd: tokio::process::Command,
    cwd: &Path,
) -> std::io::Result<tokio::process::Child> {
    cmd.current_dir(cwd);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // 0 = 以自身 pid 新建进程组（setsid 的轻量等价物，无需 libc）
    cmd.process_group(0);
    cmd.spawn()
}

/// 非 Unix 兜底：无进程组语义，原样 spawn（本项目只跑 macOS / Linux）。
#[cfg(not(unix))]
pub fn spawn_in_own_process_group(
    command: &str,
    cwd: &Path,
) -> std::io::Result<tokio::process::Child> {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg(command).current_dir(cwd);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    cmd.spawn()
}

/// 生产实现：真正的进程组终止（决策 66）。
#[derive(Debug, Clone, Copy, Default)]
pub struct RealProcessKiller;

impl ProcessKiller for RealProcessKiller {
    fn kill_process_group(&self, pgid: i32) -> Result<()> {
        if pgid <= 0 {
            return Ok(());
        }
        // 负 pid 表示整个进程组。用 /bin/kill 避免引入 libc 依赖。
        let status = std::process::Command::new("kill")
            .arg("-TERM")
            .arg(format!("-{pgid}"))
            .status()?;
        if !status.success() {
            // 进程可能已退出——不是错误。
            tracing::debug!(pgid, "进程组已不存在或 TERM 失败，尝试 KILL");
            let _ = std::process::Command::new("kill")
                .arg("-KILL")
                .arg(format!("-{pgid}"))
                .status();
        }
        Ok(())
    }
}
