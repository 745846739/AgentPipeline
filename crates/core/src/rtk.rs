//! rtk（Rust Token Killer）的**改写**与**可用性**（决策 297 / 票 03–04）。
//!
//! 两件事住在一个模块里，因为它们共用同一个事实：**这台机器上的 rtk 能不能改写**。
//!
//! ## 改写为什么复用 rtk 自己的改写器，而不自写前缀器
//!
//! 两种失败的形状不同。协议变了 → 解析不出 → 不改写 → 命令照常跑（只丢优化）；
//! 而「哪些命令 rtk 认得」这份名单写错 → **命令坏掉**：`test -f x` 撞 rtk 自己的
//! `test` 子命令（实测 `rtk test 1 = 1` 走到 rtk 的测试运行器上、exit 127）、
//! `env FOO=1 cmd` 撞 `rtk env`。而那份名单住在 rtk 里，抄一份出来就是制造一份会漂的
//! 清单（决策 258 的教训）。
//!
//! 它不只是加前缀（这是台账必须记两份的原因）：`cat X` → `rtk read X`；
//! `python3 -m pytest -q` → `rtk pytest -q`；`npx eslint .` → `rtk lint .`。
//! 执行的是**另一条命令**，不是同一条命令的包装。
//!
//! ## 可用性判据三条（第三条是关键）
//!
//! ① 解析到一个绝对路径；② 它 `--version` 能跑；③ `rtk hook claude` 喂一条 `ls`，
//! 能回一段**可解析的**改写。第三条把本设计里唯一一处「依赖没文档的协议」变成
//! **开箱可见的事实**：rtk 版本太老、没有 `hook` 子命令、改写被关掉——都在设置页就说
//! 「这台机器上的 rtk 不能改写」，而不是等第一条命令悄悄没被优化。静默失效是这类耦合
//! 最坏的形状。
//!
//! ## 失败一律放行
//!
//! 非零退出 / 空输出 / JSON 解析失败 / 超时一律 [`None`]：**原样执行，不阻断命令**。
//! 优化器不可用不该升级成整条命令失败。

use std::path::{Path, PathBuf};

use crate::home::Home;

/// shim 目录里那唯一一个名字（票 04）。
pub const SHIM_NAME: &str = "rtk";

/// 改写器的超时（决策 297 / 票 03）。实测单次约 45ms，2s 是「远够用且不拖住命令」的量。
pub const REWRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// 探测的超时：`--version` 与「喂一条 `ls` 看它回不回改写」**各自**的预算（最坏 ~2×）。
///
/// 比 [`REWRITE_TIMEOUT`] 宽一档是**故意的**：探测是人手动触发的一次性动作，多等两秒换个
/// 「真结论」永远比拿一个假的「不能改写」去糊设置页划算——而改写跑在命令热路径上，
/// 那边到点就放行（fail-open）才是对的。两条路的取舍方向本来相反，故预算也不同。
pub const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// 已知目录（决策 297 / 票 04）：服务进程的 `PATH` 之后按这个顺序找。
///
/// **不问用户的登录 shell**：`$SHELL -lc 'command -v rtk'` 会在服务进程里**静默执行用户的
/// rc 文件**（副作用 + 数百毫秒），而决策 185 之后本仓对「偷偷扫 PATH」一贯的处置是删掉。
/// 失败可见、可修就够了。
const KNOWN_DIRS: [&str; 5] = [
    "/usr/local/bin",
    "/opt/homebrew/bin",
    "/opt/local/bin",
    ".local/bin",
    ".cargo/bin",
];

/// 后两条是**相对家目录**的（`~/.local/bin` / `~/.cargo/bin`）。
const HOME_RELATIVE_FROM: usize = 3;

/// 运行期的 rtk：**钉住的那份路径** + shim 目录（票 04）。
///
/// 「本机装了 rtk」与「这个服务能用 rtk」是两个答案，后者才是唯一有意义的那个。
/// 桌面壳由 Finder 直接 exec（`crates/desktop/src/main.rs`），继承 launchd 的最小 PATH
/// （本机 `launchctl getenv PATH` 为空 → `/usr/bin:/bin:/usr/sbin:/sbin`），
/// `/usr/local/bin/rtk` 不在里面；而命令行起 `agent-pipeline serve` 则继承 shell 的 PATH。
/// 钉住它，「启用时说可用」与「真跑起来可用」才是同一件事。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtkRuntime {
    /// 探测解析到的**绝对路径**：改写器调它。
    pub binary: PathBuf,
    /// 私有 shim 目录：由 [`crate::exec::CommandRunner`] 前置进**子进程**的 `PATH`，
    /// 于是改写器吐出来的裸 `rtk` 找得到。
    pub shim_dir: PathBuf,
}

/// 解析来源（诚实口径，决策 257 的同一条纪律：这一份是谁定的）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// 设置页手填的路径（**覆盖**自动解析）。
    Manual,
    /// 服务进程的 `PATH`。
    Path,
    /// 五个已知目录之一。
    KnownDir,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Manual => "manual",
            Source::Path => "path",
            Source::KnownDir => "known-dir",
        }
    }
}

/// 一次可用性探测的结果（`GET /rtk` 与 `PUT /rtk` 的响应体就是它）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Availability {
    /// 三条判据全过才算可用。
    pub available: bool,
    /// 解析到的绝对路径（判据①）。
    pub path: Option<PathBuf>,
    /// 这一份路径是谁定的。
    pub source: Option<Source>,
    /// `rtk --version` 的读数（判据②）。
    pub version: Option<String>,
    /// 不可用时**可归因**的原因——三种失败各有各的说法，界面原样摆出来。
    pub reason: Option<String>,
}

impl Availability {
    fn failed(reason: impl Into<String>) -> Self {
        Availability {
            available: false,
            path: None,
            source: None,
            version: None,
            reason: Some(reason.into()),
        }
    }
}
/// 私有 shim 目录（票 04）：`{home}/rtk-shim/`，**只放一个名字**。
pub fn shim_dir(home: &Home) -> PathBuf {
    home.root().join("rtk-shim")
}

/// 把解析到的绝对路径钉成一个 shim 符号链接，返回 shim 目录（票 04）。
///
/// 目录里**只含 `rtk` 一个名字**：每次重建都先清空目录——「只含一个名字」是这条设计的
/// 性质（避免顺手改掉 `python3` 之类别的命令的解析），不是当前的巧合。
pub fn pin(home: &Home, binary: &Path) -> std::io::Result<PathBuf> {
    let dir = shim_dir(home);
    // 相对路径的符号链接在 shim 目录里解析不到原处（那是链接**所在**目录的相对路径），
    // 故钉之前先把目标绝对化。
    let target = std::fs::canonicalize(binary).unwrap_or_else(|_| binary.to_path_buf());
    // 已经是这一份就**什么都不做**。这条捷径不只是省一次写：重建要先清空目录，于是
    // 并发命令下会开一个「shim 里暂时没有 `rtk`」的窗口——那一刻起跳的子进程拿到的是
    // PATH 里的另一份，或者干脆 127。常态（同一目标）下把这个窗口收成零。
    if shim_is_current(&dir, &target) {
        return Ok(dir);
    }
    std::fs::create_dir_all(&dir)?;
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        // 目录里出现别的东西（旧链接、坏链接、误放的目录）一并清掉。
        if path.is_dir() {
            let _ = std::fs::remove_dir_all(&path);
        } else {
            let _ = std::fs::remove_file(&path);
        }
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, dir.join(SHIM_NAME))?;
    Ok(dir)
}

/// shim 已经就位吗：目录里**恰好**一个 `rtk`，且它指向 `target`。
///
/// 「只含一个名字」是这条设计的性质而不是当前的巧合，故捷径也要守着它——目录里多出
/// 别的东西（有人手放进去的、旧版本留下的）时照旧走重建那一支清干净。
fn shim_is_current(dir: &Path, target: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    let mut names: Vec<String> = Vec::new();
    for entry in entries {
        match entry {
            Ok(e) => names.push(e.file_name().to_string_lossy().into_owned()),
            Err(_) => return false,
        }
    }
    if names.len() != 1 || names[0] != SHIM_NAME {
        return false;
    }
    std::fs::read_link(dir.join(SHIM_NAME))
        .map(|p| p == target)
        .unwrap_or(false)
}

/// 拆掉 shim（关掉开关 / 路径解析不到时）——**不留残迹**。
pub fn unpin(home: &Home) -> std::io::Result<()> {
    let dir = shim_dir(home);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

/// 「本服务用不上 rtk」的留痕：**同一个原因只报一次**（票 03 的判据——每个班次最多一条）。
///
/// 判据是**原因串**：三种失败各有各的说法，故它们各自报一次；同一原因第二次起静默。
/// 这是进程级的集合，条目是几个固定的短串（找不到 / 建不起来 / 读开关失败），不会长。
pub fn warn_unavailable(reason: &str) {
    if first_time(reason) {
        tracing::warn!(reason, "rtk 已启用但本服务用不上它，命令按原样执行");
    }
}

/// 这个原因是不是第一次见（[`warn_unavailable`] 的判据本身，单测直接钉它）。
pub fn first_time(reason: &str) -> bool {
    static SEEN: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    let seen = SEEN.get_or_init(Default::default);
    match seen.lock() {
        Ok(mut set) => set.insert(reason.to_string()),
        // 中毒的锁不该让命令失败：报一次总比不报好。
        Err(_) => true,
    }
}

/// 运行期读数：开关开着时从设置里取出生效的 rtk（票 04）。
///
/// 开关开着而二进制**不在场**（被卸载、shim 失效）时返回 `None`：命令**原样执行**，
/// 每个班次最多一条 `tracing::warn`。优化器不可用不该升级成整条命令失败。
pub fn runtime(home: &Home, enabled: bool, manual: Option<&Path>) -> Option<RtkRuntime> {
    if !enabled {
        return None;
    }
    let resolved = match resolve(manual) {
        Ok(r) => r,
        Err(reason) => {
            // 留痕：静默失效是这类耦合最坏的形状——探测时说得清，运行期也得留一句。
            // **每个原因最多一条**（票 03 的判据）：这是热路径（每条命令都问一次），
            // 而这类失败是持续性的，按条报只会把日志刷成噪声。
            warn_unavailable(&format!("找不到：{reason}"));
            return None;
        }
    };
    let dir = match pin(home, &resolved.path) {
        Ok(dir) => dir,
        Err(e) => {
            warn_unavailable(&format!("shim 建不起来：{e}"));
            return None;
        }
    };
    Some(RtkRuntime {
        binary: resolved.path,
        shim_dir: dir,
    })
}

/// 解析到的路径 + 它来自哪一级。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub path: PathBuf,
    pub source: Source,
}

/// 按顺序解析 rtk 的绝对路径（决策 297 / 票 04）。
///
/// **手填路径是权威**，不是「最后一根稻草」：spec §4 的「服务进程 PATH → 已知目录 →
/// 手填路径（兜底）」说的是用户**什么时候会去填**（自动都失败时），而票 04 的判据
/// 「手填路径能覆盖自动解析」要求填了就生效——否则装了新旧两个 rtk 的人没法指定用哪个。
/// 两个说法合起来是：填了就用它，**填错时如实报错，不静默回落到自动解析**。
pub fn resolve(manual: Option<&Path>) -> Result<Resolved, String> {
    if let Some(raw) = manual {
        let raw = raw.to_string_lossy();
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("填的路径是空的——留空表示「自动找」，要么填一个绝对路径".into());
        }
        let path = Path::new(trimmed);
        if !path.is_absolute() {
            return Err(format!(
                "填的路径不是绝对路径：{trimmed}（要形如 /usr/local/bin/rtk）"
            ));
        }
        if !is_executable_file(path) {
            return Err(format!(
                "填的路径上没有可执行的 rtk：{trimmed}（检查这个文件在不在、是不是可执行）"
            ));
        }
        return Ok(Resolved {
            path: path.to_path_buf(),
            source: Source::Manual,
        });
    }
    if let Some(path) = find_on_path() {
        return Ok(Resolved {
            path,
            source: Source::Path,
        });
    }
    if let Some(path) = find_in_known_dirs() {
        return Ok(Resolved {
            path,
            source: Source::KnownDir,
        });
    }
    Err(format!(
        "没找到 rtk：服务进程的 PATH 与五个已知目录（{}）里都没有。\
         装上它（`brew install rtk` 之类），或者在这一页手填它的绝对路径。",
        KNOWN_DIRS.join(" / ")
    ))
}

/// 服务进程的 `PATH` 里找（**只读环境变量，不跑用户的登录 shell**）。
fn find_on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(SHIM_NAME))
        // `PATH` 里的相对条目要绝对化：解析结果的契约就是「一个绝对路径」。
        .map(|candidate| std::fs::canonicalize(&candidate).unwrap_or(candidate))
        .find(|candidate| is_executable_file(candidate))
}

fn find_in_known_dirs() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    KNOWN_DIRS
        .iter()
        .enumerate()
        .filter_map(|(i, dir)| {
            let base = if i >= HOME_RELATIVE_FROM {
                home.as_ref()?.join(dir)
            } else {
                PathBuf::from(dir)
            };
            let candidate = base.join(SHIM_NAME);
            is_executable_file(&candidate).then_some(candidate)
        })
        .next()
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

/// 三条判据一起跑（`GET /rtk` 的活体探测，决策 297 / 票 03）。
///
/// **不缓存上一次的结果**——`lanToggle` 那条纪律：重读目标态才算数（决策 257 是「漏读」
/// 的学费）。每次打开页面都重新问一遍这台机器。
pub async fn probe(manual: Option<&Path>) -> Availability {
    probe_within(manual, PROBE_TIMEOUT).await
}

/// 同 [`probe`]，但每一格的预算由调用点给——**这一格存在的理由是测试**（同 [`rewrite_within`]）。
///
/// 生产的预算就是 [`PROBE_TIMEOUT`]；测「失败形态认不认得出来」的那些用例要给宽松预算。
/// 不这么做的话，在几百个用例并排跑的全量套件里，`/bin/sh` 起一个进程这句会偶发超过 5s，
/// 于是**每一格**都变成「它卡住了？」——那是设计本身造出来的红，不是被测的那件事。
pub async fn probe_within(manual: Option<&Path>, budget: std::time::Duration) -> Availability {
    let resolved = match resolve(manual) {
        Ok(r) => r,
        Err(reason) => return Availability::failed(reason),
    };
    let version = match read_version(&resolved.path, budget).await {
        Ok(v) => v,
        Err(reason) => return Availability::failed(reason),
    };
    if let Err(reason) = can_rewrite(&resolved.path, budget).await {
        return Availability {
            path: Some(resolved.path),
            source: Some(resolved.source),
            version: Some(version),
            ..Availability::failed(reason)
        };
    }
    Availability {
        available: true,
        path: Some(resolved.path),
        source: Some(resolved.source),
        version: Some(version),
        reason: None,
    }
}

async fn read_version(binary: &Path, budget: std::time::Duration) -> Result<String, String> {
    let out = tokio::time::timeout(budget, async {
        tokio::process::Command::new(binary)
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .output()
            .await
    })
    .await;
    let out = match out {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => return Err(format!("找到了 {}，但它跑不起来：{e}", binary.display())),
        Err(_) => {
            return Err(format!(
                "找到了 {}，但 `--version` 在 {}s 内没有回来（它卡住了？）",
                binary.display(),
                budget.as_secs()
            ))
        }
    };
    if !out.status.success() {
        return Err(format!(
            "找到了 {}，但 `--version` 以非零退出（{}）——这个二进制不能当 rtk 用",
            binary.display(),
            out.status.code().unwrap_or(-1)
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if text.is_empty() {
        return Err(format!(
            "找到了 {}，但 `--version` 什么都没打出来——认不出它是不是 rtk",
            binary.display()
        ));
    }
    // 只留第一行：版本串是这一行的全部用途，而有些工具会附一长串构建信息。
    Ok(text.lines().next().unwrap_or_default().to_string())
}

/// 判据③：喂一条 `ls`，看它能不能回一段**可解析的**改写。
async fn can_rewrite(binary: &Path, budget: std::time::Duration) -> Result<(), String> {
    // 生产那一档是探测预算（5s）而不是改写预算（2s）：见 `PROBE_TIMEOUT` 的说明——这一句
    // 的结论会**显示在设置页上**，拿一个到点就放行的短预算去判它，等于把「机器忙」说成
    // 「这台机器的 rtk 不能改写」。
    match rewrite_within(binary, "ls", budget).await {
        Some(_) => Ok(()),
        None => Err(format!(
            "找到了 {}，但它不能改写：`rtk hook claude` 没有回出可解析的改写\
             （rtk 版本太老、没有 `hook` 子命令，或改写被关掉了）",
            binary.display()
        )),
    }
}

/// 改写一条命令（决策 297 / 票 03）：`None` = **不改写，原样执行**。
///
/// 协议（rtk 0.42.4 的 Claude Code PreToolUse 钩子）：
///
/// ```text
/// stdin : {"tool_name":"Bash","tool_input":{"command":"<原命令>"}}
/// stdout: {"hookSpecificOutput":{…,"updatedInput":{"command":"<改写后>"}}}
/// 空输出 = 不改写
/// ```
///
/// 它自带三件我们不想自己维护的东西：**链式逐段加前缀**（`cd x; ls; git status` →
/// `cd x; rtk ls; rtk git status`）、一份**允许名单**、以及**幂等**（已是 `rtk …` 就回空
/// 输出，故模型自己加了前缀也不会变成 `rtk rtk …`）。
pub async fn rewrite(binary: &Path, command: &str) -> Option<String> {
    rewrite_within(binary, command, REWRITE_TIMEOUT).await
}

/// 同 [`rewrite`]，但预算由调用点给。
///
/// **这一格存在的理由是测试**：2s 是给生产的（本地二进制跑一次解析），而「起进程 + 读一行」
/// 在几百个用例并排跑的全量套件里会偶发超过 2s——那条路按设计回 `None`（fail-open），于是
/// 「读得到改写器的回话」这条断言会偶发变红，红的却是设计本身而不是缺陷。把预算变成参数，
/// 单测就能用一个宽松到不该到点的预算去测**映射**，另用一个短的预算去测**超时那条分支**。
/// 生产侧只有 [`rewrite`]，这里换不换预算都改不到产品行为。
async fn rewrite_within(
    binary: &Path,
    command: &str,
    budget: std::time::Duration,
) -> Option<String> {
    let payload = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": command },
    })
    .to_string();
    let stdout = run_rewriter(binary, &payload, budget).await?;
    parse_rewrite(&stdout, command)
}

/// 把改写器的输出解析成「改写后的串」——**收串吐串的纯映射**，单测直接喂（票 03 的判据）。
///
/// 空输出 / 解析不出 / 与原文相同一律 `None`（最后一条：改写器回了原文等于没改，
/// 记一条 `original_command` 只会让台账多一列噪声）。
pub fn parse_rewrite(stdout: &str, original: &str) -> Option<String> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    let parsed: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    let updated = parsed
        .get("hookSpecificOutput")?
        .get("updatedInput")?
        .get("command")?
        .as_str()?
        .trim();
    if updated.is_empty() || updated == original {
        return None;
    }
    Some(updated.to_string())
}

/// 跑一次改写器，收它的 stdout（非零退出 / 超时 / 起不来一律 `None`）。
///
/// `kill_on_drop` 在这里是**对的**：超时时那个 future 被丢掉，进程跟着被杀——
/// 不留一个还在等 stdin 的 rtk 挂在那里。（全仓别处没有 `kill_on_drop`，见 `exec.rs`
/// 的 `ChildGuard`：那里的进程是**被追踪**的，这里的一次性探针不是。）
async fn run_rewriter(binary: &Path, payload: &str, budget: std::time::Duration) -> Option<String> {
    use tokio::io::AsyncWriteExt;

    let work = async {
        let mut child = tokio::process::Command::new(binary)
            .arg("hook")
            .arg("claude")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .ok()?;
        {
            let mut stdin = child.stdin.take()?;
            stdin.write_all(payload.as_bytes()).await.ok()?;
            // 关掉 stdin：改写器读完整串才会回话（不关就是它等我们、我们等它）。
        }
        let out = child.wait_with_output().await.ok()?;
        if !out.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&out.stdout).to_string())
    };

    tokio::time::timeout(budget, work).await.ok()?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一个假 rtk：按 `hook claude` 的协议回话，内容由环境变量决定。
    fn fake_rtk(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    /// 下面这几条「起真进程」的断言用的宽松预算，**不是**生产里那两档
    /// （[`REWRITE_TIMEOUT`] 2s / [`PROBE_TIMEOUT`] 5s）。
    ///
    /// 它们说的是「映射对不对」与「失败形态认不认得出来」，不是「2s / 5s 够不够」——而在几百个
    /// 用例并排跑的全量套件里，「`/bin/sh` 起一个进程读一行」偶发超过那一档，那条路按设计回
    /// `None` / 报「它卡住了？」，于是断言会**因为设计本身**变红。要用生产预算去测超时，就去看
    /// [`a_rewriter_that_hangs_is_not_waited_for`]（它给的是短预算，故那条分支既测得到也不慢）。
    const GENEROUS: std::time::Duration = std::time::Duration::from_secs(60);

    /// 改写是**纯映射**：链式逐段、已带前缀回空、形状不认识回空（票 03 的判据）。
    #[test]
    fn parse_rewrite_is_a_pure_mapping() {
        // 链式逐段加前缀：改写器自己做的，我们只把结果原样取出来
        let chunked = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse",
            "updatedInput":{"command":"cd /tmp; rtk ls; rtk git status"}}}"#;
        assert_eq!(
            parse_rewrite(chunked, "cd /tmp; ls; git status").as_deref(),
            Some("cd /tmp; rtk ls; rtk git status")
        );

        // 空输出 = 不改写
        assert_eq!(parse_rewrite("", "ls"), None);
        assert_eq!(parse_rewrite("   \n", "ls"), None);

        // 已是 `rtk …` → 改写器回空输出（幂等），故模型自己加了前缀也不会变成 `rtk rtk …`
        assert_eq!(parse_rewrite("", "rtk ls"), None);

        // 回的原文 = 没改：不算改写（记一条 original_command 只会是噪声）
        let echo = r#"{"hookSpecificOutput":{"updatedInput":{"command":"ls"}}}"#;
        assert_eq!(parse_rewrite(echo, "ls"), None);

        // 形状不认识：不是 JSON / 缺字段 / 字段类型不对
        assert_eq!(parse_rewrite("not json", "ls"), None);
        assert_eq!(parse_rewrite("{}", "ls"), None);
        assert_eq!(parse_rewrite(r#"{"hookSpecificOutput":{}}"#, "ls"), None);
        assert_eq!(
            parse_rewrite(r#"{"hookSpecificOutput":{"updatedInput":{}}}"#, "ls"),
            None
        );
        assert_eq!(
            parse_rewrite(
                r#"{"hookSpecificOutput":{"updatedInput":{"command":42}}}"#,
                "ls"
            ),
            None
        );
    }

    #[tokio::test]
    async fn rewrite_reads_the_updated_command_from_the_rewriter() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = fake_rtk(
            tmp.path(),
            "rtk",
            "#!/bin/sh\n\
             read -r payload\n\
             case \"$payload\" in\n\
               *'\"ls\"'*) printf '%s' '{\"hookSpecificOutput\":{\"updatedInput\":{\"command\":\"rtk ls\"}}}' ;;\n\
               *) : ;;\n\
             esac\n",
        );
        assert_eq!(
            rewrite_within(&bin, "ls", GENEROUS).await.as_deref(),
            Some("rtk ls")
        );
        // 已经带了前缀：改写器回空输出 → 不改写
        assert_eq!(rewrite_within(&bin, "cat x", GENEROUS).await, None);
    }

    /// 非零退出 / 起不来 / 回不出可解析的串一律 `None`——**失败放行**，不阻断命令。
    /// （超时那条路见 [`a_rewriter_that_hangs_is_not_waited_for`]。）
    #[tokio::test]
    async fn every_failure_mode_yields_none() {
        let tmp = tempfile::tempdir().unwrap();
        // 用宽松预算：这三条要测的是「非零退出」与「解析不出」，若它们因为机器忙先撞上
        // 2s 那条路，断言照样 `None`——**过得去但过得不对**（红线是别的缺陷时它已经绿了）。
        let crash = fake_rtk(tmp.path(), "rtk-crash", "#!/bin/sh\nexit 3\n");
        assert_eq!(rewrite_within(&crash, "ls", GENEROUS).await, None);

        let garbage = fake_rtk(tmp.path(), "rtk-garbage", "#!/bin/sh\nread x\necho hi\n");
        assert_eq!(rewrite_within(&garbage, "ls", GENEROUS).await, None);

        let missing = tmp.path().join("nope");
        assert_eq!(rewrite_within(&missing, "ls", GENEROUS).await, None);
    }

    /// 超时那条分支：预算到点就放行，且**不真的等那么久**。
    ///
    /// 这条是补的——此前整套用例没有一条走到这条路（失败那三条走的是非零退出与解析失败），
    /// 而「2s 到点放行」正是设计里最该有断言的一句话。预算可注入，故这里喂 200ms 配一个
    /// `sleep 30` 的假 rtk：既快又确定。
    #[tokio::test]
    async fn a_rewriter_that_hangs_is_not_waited_for() {
        let tmp = tempfile::tempdir().unwrap();
        let hang = fake_rtk(tmp.path(), "rtk-hang", "#!/bin/sh\nsleep 30\n");
        let started = std::time::Instant::now();
        assert_eq!(
            rewrite_within(&hang, "ls", std::time::Duration::from_millis(200)).await,
            None
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "不该真的等那个 30s 的子进程：{:?}",
            started.elapsed()
        );
    }

    /// 判据②③：`--version` 跑不起来 / 跑得起来但不能改写，各自的原因**可归因**。
    #[tokio::test]
    async fn probe_attributes_each_failure() {
        let tmp = tempfile::tempdir().unwrap();

        // 判据①：解析不到
        let empty = tempfile::tempdir().unwrap();
        let manual_missing = empty.path().join("rtk");
        let missing = probe_within(Some(&manual_missing), GENEROUS).await;
        assert!(!missing.available);
        assert!(missing.reason.unwrap().contains("没有可执行的 rtk"));

        // 判据②：`--version` 非零
        let broken = fake_rtk(tmp.path(), "rtk-broken", "#!/bin/sh\nexit 2\n");
        let broken_run = probe_within(Some(&broken), GENEROUS).await;
        assert!(!broken_run.available);
        let broken_reason = broken_run.reason.clone().unwrap_or_default();
        assert!(
            broken_reason.contains("非零退出"),
            "判据②要可归因，实际原因串：{broken_reason}"
        );

        // 判据③：版本能跑，但改写回不出可解析的东西
        let mute = fake_rtk(
            tmp.path(),
            "rtk-mute",
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'rtk 0.1.0'; else read x; fi\n",
        );
        let unrewritable = probe_within(Some(&mute), GENEROUS).await;
        assert!(!unrewritable.available);
        assert_eq!(
            unrewritable.version.as_deref(),
            Some("rtk 0.1.0"),
            "版本这一格要读出来，实际原因串：{:?}",
            unrewritable.reason
        );
        let unrewritable_reason = unrewritable.reason.clone().unwrap_or_default();
        assert!(
            unrewritable_reason.contains("不能改写"),
            "判据③要可归因，实际原因串：{unrewritable_reason}"
        );

        // 三条全过
        let good = fake_rtk(
            tmp.path(),
            "rtk-good",
            "#!/bin/sh\n\
             if [ \"$1\" = \"--version\" ]; then echo 'rtk 0.42.4'; exit 0; fi\n\
             read -r payload\n\
             printf '%s' '{\"hookSpecificOutput\":{\"updatedInput\":{\"command\":\"rtk ls\"}}}'\n",
        );
        let ok = probe_within(Some(&good), GENEROUS).await;
        assert!(ok.available);
        assert_eq!(ok.version.as_deref(), Some("rtk 0.42.4"));
        assert_eq!(ok.source, Some(Source::Manual));
    }

    /// 手填路径是**权威**：填错时如实报错，不静默回落到自动解析（票 04 的判据）。
    #[test]
    fn manual_path_overrides_and_never_falls_back() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("rtk");
        let err = resolve(Some(&missing)).unwrap_err();
        assert!(err.contains("没有可执行的 rtk"), "{err}");

        // 相对路径不是「一个绝对路径」，按判据①拒掉
        let err = resolve(Some(Path::new("rtk"))).unwrap_err();
        assert!(err.contains("不是绝对路径"), "{err}");

        // 空串 = 没说；不是「填了一个空路径」
        let err = resolve(Some(Path::new("   "))).unwrap_err();
        assert!(err.contains("空的"), "{err}");
    }

    /// 留痕**每个原因只报一次**（票 03 的判据：每个班次最多一条）。
    ///
    /// 命令是热路径，而这类失败是持续性的——按条报会把日志刷成噪声，而它要传的信息
    /// 一条就够。判据是原因串，故三种失败各自报一次。
    #[test]
    fn each_unavailable_reason_is_reported_once() {
        let reason = "测试专用：同一个原因只报一次";
        assert!(first_time(reason), "第一次该报");
        assert!(!first_time(reason), "同一个原因第二次起静默");
        assert!(
            first_time("测试专用：另一种原因"),
            "另一种原因另算一次（三种失败各有各的说法）"
        );
    }

    /// 票 04 的纪律：**不跑用户的登录 shell**。
    ///
    /// 判据不是读代码而是**放一个会写标记的 `$SHELL`**：真跑过它，标记就会出现。
    /// 改这个进程级环境变量在这里是安全的——全仓只有本文件的一处注释提到 `SHELL`，
    /// 没有别的用例会观察它。断言只看标记：这台机器装没装 rtk 都不影响这条。
    #[tokio::test]
    async fn the_users_login_shell_is_never_run() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("shell-ran");
        let fake_shell = fake_rtk(
            tmp.path(),
            "shell",
            &format!("#!/bin/sh\ntouch {}\nexit 1\n", marker.display()),
        );
        let previous = std::env::var_os("SHELL");
        std::env::set_var("SHELL", &fake_shell);

        let resolved = resolve(None);
        let probed = probe(None).await;

        match previous {
            Some(v) => std::env::set_var("SHELL", v),
            None => std::env::remove_var("SHELL"),
        }
        assert!(
            !marker.exists(),
            "解析跑了用户的登录 shell——那会在服务进程里静默执行用户的 rc 文件"
        );
        // 结果本身与这条纪律无关：找得到 / 找不到都行，读到的路径也不能是那个假 shell
        assert!(
            resolved.map(|r| r.path) != Ok(fake_shell.clone()),
            "不该把 $SHELL 当成 rtk 候选"
        );
        assert_ne!(probed.path.as_deref(), Some(fake_shell.as_path()));
    }

    /// 同一目标上 `pin` **幂等**：不重建目录（票 04 的并发面）。
    ///
    /// 重建 = 清空 + 重链，于是并发命令下会开一个「shim 里暂时没有 `rtk`」的窗口——
    /// 那一刻起跳的子进程拿到的是 PATH 里的另一份，或者干脆 127。判据是链接自己的 inode：
    /// 不重建则不变；目标变了才重建。顺手钉住「目录里多出别的东西时照旧重建」。
    /// `pin` 钉的是**解析后**的路径（`canonicalize`，失败则退回原路径）。断言同一口径，
    /// 免得又把 `/bin/sh` 这种符号链接型路径写成期望值——那在 Linux 上必红。
    fn resolved_path(path: &std::path::Path) -> std::path::PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }

    #[test]
    fn pinning_keeps_the_same_target_without_rebuilding() {
        let tmp = tempfile::tempdir().unwrap();
        let home = Home::new(tmp.path().join("home"));
        home.ensure_dirs().unwrap();
        let first = PathBuf::from("/bin/sh");
        let dir = pin(&home, &first).unwrap();
        assert!(
            shim_is_current(&dir, &resolved_path(&first)),
            "同一目标应当就位（这条捷径就是「不重建」本身）"
        );
        for _ in 0..3 {
            pin(&home, &first).unwrap();
        }
        // 三次重复钉之后仍是同一份（没被清掉重建）
        assert!(
            shim_is_current(&dir, &resolved_path(&first)),
            "同一目标不该重建（重建会开一个空窗）"
        );

        // 目标变了：该重建
        let second = PathBuf::from("/bin/ls");
        if second.exists() {
            assert!(
                !shim_is_current(&dir, &resolved_path(&second)),
                "换了目标就不再是「已就位」——这正是重建的触发条件"
            );
            pin(&home, &second).unwrap();
            // 断链接指向（**不拿 inode 当代理**）：Linux 上「删掉旧链接再建一个」常复用
            // 刚释放的 inode 号，`assert_ne!(inode)` 于是假红（2026-09-30 CI 实测），
            // 而 `assert_eq!(inode)` 又会把「其实重建了」放过去——两边都不可信。
            // 比的是**解析后**的目标（`pin` 先 `canonicalize`）：`/bin/sh` 在 Linux 上是
            // 指向 `/usr/bin/dash` 的符号链接、`/bin/ls` 同理，写死原路径的断言在 macOS
            // 上过、在 CI 的 Linux 上必红。
            assert_eq!(
                std::fs::read_link(dir.join(SHIM_NAME)).unwrap(),
                resolved_path(&second),
                "换了目标就得重建：链接必须指向解析后的新目标"
            );
        }

        // 「只含一个名字」是性质不是巧合：目录里多出别的东西时，捷径不生效、照旧清干净
        std::fs::write(dir.join("python3"), b"stale").unwrap();
        pin(&home, &first).unwrap();
        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![SHIM_NAME.to_string()]);
    }

    /// shim 目录只含 `rtk` 一个名字，重建时不留残迹（票 04 的判据）。
    #[test]
    fn shim_holds_exactly_one_name_and_leaves_no_residue() {
        let tmp = tempfile::tempdir().unwrap();
        let home = Home::new(tmp.path().join("home"));
        home.ensure_dirs().unwrap();

        // 先在目录里放点「残迹」：一个旧名字、一个坏链接、一个目录
        let dir = shim_dir(&home);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("python3"), b"stale").unwrap();
        std::fs::create_dir_all(dir.join("nested")).unwrap();

        // 拿一个真实存在的可执行文件当目标（sh 一定在）
        let target = PathBuf::from("/bin/sh");
        assert!(is_executable_file(&target), "测试前提：/bin/sh 可执行");
        let dir = pin(&home, &target).unwrap();

        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(
            names,
            vec![SHIM_NAME.to_string()],
            "shim 目录只该有一个名字"
        );
        // 指向的是原处（绝对化过，故不随 shim 目录解析）
        let link = std::fs::read_link(dir.join(SHIM_NAME)).unwrap();
        assert_eq!(link, resolved_path(&target));

        unpin(&home).unwrap();
        assert!(!shim_dir(&home).exists(), "关掉之后不留残迹");
    }
}
