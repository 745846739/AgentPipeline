//! 可测试性接缝③：进程组终止器（决策 143 / 66）。
//!
//! 超时路径必须断言"杀了进程组"，但测试里没有真进程可杀。把终止动作抽成 trait：
//! 生产实现真杀，测试实现只记录调用。

use std::path::{Path, PathBuf};

use crate::Result;

pub trait ProcessKiller: Send + Sync + 'static {
    /// 向进程组发送终止信号（先 TERM，必要时 KILL）。
    fn kill_process_group(&self, pgid: i32) -> Result<()>;
}

/// 子进程的环境调整（决策 297 / 票 04）：目前只有「把私有 shim 目录前置进 `PATH`」一件事。
///
/// **只作用于子进程**：服务进程自己的 `PATH` 一个字不改——在服务进程里前置一个目录会顺手
/// 改掉**别的**命令的解析（`/usr/local/bin` 里还有一堆别的二进制），而 shim 目录里只放
/// `rtk` 一个名字，故它只影响 `rtk`。
///
/// 为什么必须靠 PATH 前置而不是把绝对路径插进命令串：改写器吐出来的是裸 `rtk`，靠 PATH 找；
/// 对每段做字符串手术（把 `rtk ` 换成绝对路径）既脆，还要在有引号的地方做手术。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChildEnv {
    /// 前置进 `PATH` 的目录（`None` = 不动 `PATH`）。
    pub path_prefix: Option<PathBuf>,
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
    env: &ChildEnv,
) -> std::io::Result<tokio::process::Child> {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg(command);
    spawn_with_stdio_and_group(cmd, cwd, env)
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
    env: &ChildEnv,
) -> std::io::Result<tokio::process::Child> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args);
    spawn_with_stdio_and_group(cmd, cwd, env)
}

/// stdio + 独立进程组的共同配置（`spawn()` 不自动接管 stdio，必须显式管道化，
/// 否则读回的是空内容；`process_group(0)` = 以自身 pid 新建进程组）。
#[cfg(unix)]
fn spawn_with_stdio_and_group(
    mut cmd: tokio::process::Command,
    cwd: &Path,
    env: &ChildEnv,
) -> std::io::Result<tokio::process::Child> {
    cmd.current_dir(cwd);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // 0 = 以自身 pid 新建进程组（setsid 的轻量等价物，无需 libc）
    cmd.process_group(0);
    apply_child_env(&mut cmd, env);
    cmd.spawn()
}

/// 把 [`ChildEnv`] 落到 `Command` 上（决策 297 / 票 04）。
///
/// `join_paths` 失败（路径含 `:`）时**不动 `PATH`**：拿不到一个合法的前置，就不要把
/// 一个坏掉的 `PATH` 交给子进程——那会让本来能跑的命令全挂掉，而这一步的收益只是优化。
#[cfg(unix)]
fn apply_child_env(cmd: &mut tokio::process::Command, env: &ChildEnv) {
    let Some(dir) = &env.path_prefix else {
        return;
    };
    let base = std::env::var_os("PATH").unwrap_or_default();
    if let Ok(joined) = child_path(dir, &base) {
        cmd.env("PATH", joined);
    }
}

/// `PATH` 的合成规则：`<shim 目录>:<原有 PATH>`——**纯函数**，故「最小 PATH 也能用」
/// 这件事可以直接喂一个最小 PATH 来断言，不必去动测试进程自己的环境（票 04 的判据）。
///
/// 前置而不是替换：服务进程的 `PATH` 该怎么用还怎么用（`cargo` / `node` 都在里面），
/// 这一条只保证 `rtk` 这个名字**先**解析到钉住的那一份。
#[cfg(unix)]
fn child_path(
    prefix: &Path,
    base: &std::ffi::OsStr,
) -> std::result::Result<std::ffi::OsString, std::env::JoinPathsError> {
    let mut paths = vec![prefix.to_path_buf()];
    paths.extend(std::env::split_paths(base));
    std::env::join_paths(paths)
}

/// 非 Unix 兜底：无进程组语义，原样 spawn（本项目只跑 macOS / Linux）。
#[cfg(not(unix))]
pub fn spawn_in_own_process_group(
    command: &str,
    cwd: &Path,
    _env: &ChildEnv,
) -> std::io::Result<tokio::process::Child> {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg(command).current_dir(cwd);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    cmd.spawn()
}

/// 生产实现：真正的进程组终止（决策 66）。
///
/// **走 `kill(2)` 系统调用，不经外部的 `kill` 命令**（2026-09-30，决策 337⑤）。
/// 旧写法 `Command::new("kill").arg("-TERM").arg("-<pgid>")` 把「负号」交给了一个**外部
/// 程序的命令行解析**，而各家实现并不一致：BSD 的 `kill`（macOS）与 util-linux 的
/// `kill`（Ubuntu 22.04，也就是 106）都按「负 pid = 进程组」办，**procps-ng 的 `kill`
/// （Ubuntu 24.04，也就是 GitHub runner）不办**——它把 `-<pgid>` 当成信号名那一族来解析
/// （见其 `kill.c` 里 `case '?'` 的 "Special case for signal digit negative PIDs" 分支），
/// 结果**退出码 0、进程组一个都没杀**。这台机器上没有任何东西看起来是坏的，直到
/// `command_funnel::a_timed_out_command_takes_its_descendants_with_it` 在 CI 上红：
/// 超时之后子孙进程还活着——正是决策 66 要消掉的那个形状，只是换了个藏身处
/// （开发机与生产机都恰好是「能办」的那两种实现，故本地怎么跑都绿）。
///
/// `libc` 本来就在依赖树里（`statvfs`，决策 321），故这一改**不新增依赖**。
#[derive(Debug, Clone, Copy, Default)]
pub struct RealProcessKiller;

impl ProcessKiller for RealProcessKiller {
    fn kill_process_group(&self, pgid: i32) -> Result<()> {
        if pgid <= 0 {
            return Ok(());
        }
        // 负 pid = 整个进程组（`kill(2)` 的原生语义，不经任何解释层）。
        let rc = unsafe { libc::kill(-pgid, libc::SIGTERM) };
        if rc != 0 {
            // 进程可能已退出——不是错误。
            let err = std::io::Error::last_os_error();
            tracing::debug!(pgid, error = %err, "进程组已不存在或 TERM 失败，尝试 KILL");
            unsafe { libc::kill(-pgid, libc::SIGKILL) };
        }
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// 票 04 的判据：**最小 PATH 下前置 shim 也够用**。
    ///
    /// 桌面壳由 Finder 直接 exec，继承 launchd 的最小 PATH
    /// （`/usr/bin:/bin:/usr/sbin:/sbin`），`/usr/local/bin/rtk` 不在里面。合成规则是
    /// 「前置」而不是「替换」，故最小 PATH 与 shell 里那条长 PATH 得到的是同一个性质：
    /// **`rtk` 这个名字先解析到钉住的那一份**。
    ///
    /// 喂字符串而不是去改测试进程自己的 `PATH`：这是个纯函数，改全局环境会让同进程里
    /// 并行的用例跟着漂。
    #[test]
    fn the_shim_is_prepended_to_any_base_path_including_the_minimal_one() {
        let shim = Path::new("/Users/me/.agentpipeline/rtk-shim");
        let minimal = std::ffi::OsStr::new("/usr/bin:/bin:/usr/sbin:/sbin");
        let joined = child_path(shim, minimal).unwrap();
        let entries: Vec<_> = std::env::split_paths(&joined).collect();
        assert_eq!(
            entries[0], shim,
            "shim 必须排在最前，否则先撞上 PATH 里的别的 rtk"
        );
        assert_eq!(
            &entries[1..],
            &[
                PathBuf::from("/usr/bin"),
                PathBuf::from("/bin"),
                PathBuf::from("/usr/sbin"),
                PathBuf::from("/sbin"),
            ],
            "原有的 PATH 逐项保留（`cargo` / `node` 还在里面），只在其前插一项"
        );

        // 票 04 的存在理由，端到端跑一遍：**最小 PATH 下 shim 里的 rtk 仍是那一个被找到的**。
        // 上面钉的是合成规则，这里把合成结果真的喂给一个子进程——Finder 起的桌面壳继承
        // 的就是这条最小 PATH。**不动本进程的 PATH**：只把结果交给这一个子进程。
        let tmp = tempfile::tempdir().unwrap();
        let fake_shim = tmp.path().join("rtk-shim");
        std::fs::create_dir_all(&fake_shim).unwrap();
        let fake = tmp.path().join("rtk");
        std::fs::write(&fake, "#!/bin/sh\necho SHIM-RTK\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::os::unix::fs::symlink(&fake, fake_shim.join("rtk")).unwrap();

        let joined = child_path(&fake_shim, minimal).unwrap();
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg("command -v rtk")
            .env("PATH", &joined)
            .output()
            .unwrap();
        assert!(out.status.success(), "最小 PATH 下该找得到 rtk");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            fake_shim.join("rtk").display().to_string(),
            "解析到的必须是 shim 里那一份"
        );

        // 空 PATH（环境里没有这一项）也要能前置。`split_paths("")` 会给出一项空串
        // （POSIX 里空项表示当前目录），故这里只断言**次序**：shim 排在最前。
        let joined = child_path(shim, std::ffi::OsStr::new("")).unwrap();
        let entries: Vec<_> = std::env::split_paths(&joined).collect();
        assert_eq!(entries[0], shim, "空 PATH 下 shim 照样排在最前");
    }
}
