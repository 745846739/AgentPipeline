//! 完全磁盘访问（Full Disk Access）的**启动探测 + 快照**（决策 306，票 executor-never-returns 05）。
//!
//! **为什么需要它**：2026-09-27 那次卡死的真因不是代码，是这台未签名机器上的授权——
//! `codesign` 报 `not signed at all`，TCC 授权按 CDHash 归属，于是**每次重建换身份 →
//! 授权失效 → 重新弹窗 → 没人应答 → `open()` 无限阻塞**（实测挂满 4 小时，
//! tccd 日志里 `AUTHREQ_PROMPTING` 一直没有 `RESULT`）。
//!
//! 本模块做两件事：
//!
//! 1. **每次启动探一次**（[`probe_and_record`]），**有界**：阻塞池 + 超时，超时即判
//!    「未授权」；探测在后台跑（调用方 `tokio::spawn`），**不阻塞启动**——缺授权时应用
//!    仍要开得起来、人仍要能被引导。
//! 2. **把结论记成一份快照**（[`record`] / [`state`]），供组装层**快速失败**用。
//!
//! **为什么不选「安装时探一次」**：这条授权能否跨重建存活**在这台未签名机器上无法离线
//! 验证**（09-25 四次重启全程无弹窗 → 同一二进制授权有效；重建一次就失效）。启动检测
//! 对「稳定」「不稳定」两种世界都正确，安装时检测只在「稳定」那一半正确。
//!
//! **为什么不能在读的那一刻探**：`access()` / `stat()` 对受保护路径**同样会阻塞**——
//! 那样探测本身就复刻了这次 4 小时的形状（挂起发生在「检查授权」里，而人以为在检查文件）。
//! 故只能用启动时记下的快照；代价是**滞后**：运行中改了设置进程不知道，所以错误文案里
//! 必须带「若你刚刚已开启，请重启应用」。
//!
//! **明确不新造 `DiskAccessProbe` trait**（决策 306，照决策 250 删掉那条只有一处 `impl`
//! 的假 seam 的姿态）：授权状态走**值注入**——启动探测写进程级快照，执行体在组装时把它
//! 填进 [`crate::pipeline::model_request::AttemptCtx`]，测试直接置值。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};

use crate::agent::bounded_read::{self, Offloaded};

/// 探测用的那一个文件：macOS 上**没有完全磁盘访问就一定读不动**的经典位置。
///
/// 选它而不是 `read_dir` 目录：目录不存在时的判据含糊（`NotFound` 也可能是别的原因），
/// 而 `TCC.db` 这个文件的存在是系统保证的——读不动它就是要授权，语义干净。
pub const PROBE_FILE: &str = "Library/Application Support/com.apple.TCC/TCC.db";

/// 启动探测的等待上界（秒）。
///
/// 比组装层那 10 秒短：这是**启动路径**上的检查，超时的判据是「连授权检查都挂住了 = 没有
/// 授权」（缺授权时那次 `open()` 会一直挂着直到弹窗被应答），所以不需要等长——
/// 等得越久，人越以为应用卡住了。
pub const PROBE_TIMEOUT_SEC: u64 = 3;

/// 缺授权时给用户的那句话（**含可操作的下一步**）。
///
/// 三点缺一不可：**是什么**（读不动受保护目录）、**去哪里**（系统设置那条路径）、
/// **为什么改了设置还不行**（快照是启动那一刻的，运行中改设置进程不知道）。
/// 最后那句不是免责声明——没有它，人会在设置里开完授权、回来看照样失败，然后以为
/// 「这个开关没用」。
pub const DENIED_HINT: &str = "读不动受保护的目录：本次启动没有检测到「完全磁盘访问权限」。\
请去 系统设置 → 隐私与安全性 → 完全磁盘访问权限 里为 AgentPipeline 打开它；\
若你刚刚已开启，请重启应用（这份判断来自启动那一刻的检查，运行中改设置进程不知道）。";

/// 授权快照的三种取值。**默认是「还没探过」而不是「没授权」**——判不出来时绝不拦人。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskAccessState {
    /// 还没探过（启动探测尚未回来，或探测判不出来）。
    NotProbed,
    /// 探测读得动受保护文件 = 有授权。**这一支永远静默**（零输出、零弹窗）。
    Granted,
    /// 探测读到「权限被拒」，或探测本身撞上等待上界 = 没授权。
    Denied,
}

const NOT_PROBED: u8 = 0;
const GRANTED: u8 = 1;
const DENIED: u8 = 2;

static SNAPSHOT: AtomicU8 = AtomicU8::new(NOT_PROBED);

/// 记下这一次的判断（启动探测的落点；测试直接置值）。
pub fn record(state: DiskAccessState) {
    SNAPSHOT.store(
        match state {
            DiskAccessState::NotProbed => NOT_PROBED,
            DiskAccessState::Granted => GRANTED,
            DiskAccessState::Denied => DENIED,
        },
        Ordering::Relaxed,
    );
}

/// 读回快照。
pub fn state() -> DiskAccessState {
    match SNAPSHOT.load(Ordering::Relaxed) {
        GRANTED => DiskAccessState::Granted,
        DENIED => DiskAccessState::Denied,
        _ => DiskAccessState::NotProbed,
    }
}

/// 缺授权吗——组装层快速失败的那一问。
pub fn is_denied() -> bool {
    state() == DiskAccessState::Denied
}

/// 这一处路径是不是**受 TCC 保护**的地方（macOS 的三处用户文件夹：文稿 / 桌面 / 下载）。
///
/// **为什么需要它**（实现期实测踩到）：只看「探测说缺授权」就拦住组装，会把**每一台**
/// 没开完全磁盘访问的机器都变成跑不动的——而只有**当项目的读真的落在受保护的地方**时，
/// 缺授权才会让人挂住（2026-09-27 那次的项目就在 `~/Documents` 下，正是这一档）。
/// 取舍：判据只有一个（项目根在不在那三处之一），代价是「项目在受保护目录之外、
/// 而读的是别的受保护路径」这种组合不被拦——那就退化成决策 302 的有界等待兜着。
pub fn needs_full_disk_access_in(home: &Path, path: &Path) -> bool {
    const PROTECTED: [&str; 3] = ["Documents", "Desktop", "Downloads"];
    PROTECTED.iter().any(|dir| path.starts_with(home.join(dir)))
}

/// [`needs_full_disk_access_in`] 的默认家目录版本（`$HOME` 取不到 = 判不出来，不拦人）。
pub fn needs_full_disk_access(path: &Path) -> bool {
    std::env::var_os("HOME").is_some_and(|h| needs_full_disk_access_in(Path::new(&h), path))
}

impl DiskAccessState {
    /// 缺授权**且这一轮真会被它伤到**时，给出来的那句可操作错误。
    ///
    /// **返回错误而不是布尔**：调用点要的是「能不能继续」+「不能继续时说哪句话」，
    /// 分两步问总会有人只问第一步，然后回一句没信息量的话。
    ///
    /// **两个条件缺一不可**：快照说缺授权，**且**这一轮的根在受保护的地方
    /// （[`needs_full_disk_access`]）。只判前者会让没开授权的机器整个跑不动——
    /// 而授权只有在读真的落在那些地方时才是必需的。
    /// `root` = 这一轮的**项目根**（受保护与否由 `$HOME` 下那三处判定）。
    pub fn denied_error_for_project(self, root: &Path) -> Option<crate::Error> {
        (self == DiskAccessState::Denied && needs_full_disk_access(root))
            .then(|| crate::Error::Config(DENIED_HINT.to_string()))
    }
}

/// 默认探测路径：`$HOME` 下那个受保护文件。`HOME` 取不到就 `None`（判不出来，不拦人）。
pub fn default_probe_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(PROBE_FILE))
}

/// 探一个路径（**有界**）。判据：
///
/// - 读得动 → [`DiskAccessState::Granted`]；
/// - `PermissionDenied` → [`DiskAccessState::Denied`]；
/// - 撞上 [`PROBE_TIMEOUT_SEC`] → [`DiskAccessState::Denied`]（连检查都挂住了 = 没授权）；
/// - 其余（文件不存在、不是文件、闭包 panic）→ [`DiskAccessState::NotProbed`]——
///   **判不出来就不拦人**，这是这个模块唯一的保守方向。
///
/// 走的是 [`bounded_read::probe`] 而不是 [`bounded_read::run`]：探测**本来就以撞墙为一条
/// 正常结论**，计进「卡住的读」那套读数会让阈值天天告警，把一个专为「环境异常」造的读数
/// 变成噪声。
pub async fn probe_path(path: &Path) -> DiskAccessState {
    let path = path.to_path_buf();
    let label = path.display().to_string();
    let got = bounded_read::probe("disk_access_probe", label, PROBE_TIMEOUT_SEC, move || {
        std::fs::File::open(&path).map(|_| ())
    })
    .await;
    match got {
        Offloaded::Done(Ok(())) => DiskAccessState::Granted,
        Offloaded::Done(Err(e)) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            DiskAccessState::Denied
        }
        Offloaded::Done(Err(e)) => {
            tracing::debug!(error = %e, "完全磁盘访问探测：这个路径给不出结论（判不出来就不拦人）");
            DiskAccessState::NotProbed
        }
        Offloaded::Stuck => DiskAccessState::Denied,
        Offloaded::Panicked(msg) => {
            tracing::debug!(error = %msg, "完全磁盘访问探测：探测任务未能执行（判不出来就不拦人）");
            DiskAccessState::NotProbed
        }
    }
}

/// 启动时那一次探测：探默认路径并把结论记进快照。
///
/// **有授权时零输出**——这是它的正常状态，不是「没消息」。
pub async fn probe_and_record() -> DiskAccessState {
    let Some(path) = default_probe_path() else {
        tracing::debug!("完全磁盘访问探测：取不到 HOME，跳过（判不出来就不拦人）");
        return DiskAccessState::NotProbed;
    };
    let state = probe_path(&path).await;
    record(state);
    state
}

/// 打开系统设置的「完全磁盘访问」面板（macOS）。别的平台什么都不做。
///
/// 失败只记日志：引导是**尽力而为**的——打不开面板不该让启动失败。
pub fn open_system_settings() {
    // **测试家目录（决策 143 的接缝）下不打扰人**：那条接缝下的进程是一次可丢弃的运行，
    // 在开发机上调出系统设置面板既不解决问题、又碍事（实测：跑一遍闸门就弹一次）。
    if std::env::var_os(crate::home::HOME_ENV).is_some() {
        tracing::debug!("测试家目录下不打开系统设置面板");
        return;
    }
    #[cfg(target_os = "macos")]
    {
        // 深链到「隐私与安全性 → 完全磁盘访问权限」那一格。
        let url = "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles";
        match std::process::Command::new("open").arg(url).spawn() {
            // 不等它：设置面板何时起来是系统的事（探测本身也不阻塞启动）。
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "打不开系统设置的完全磁盘访问面板"),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        tracing::debug!("不在 macOS 上：完全磁盘访问这一层不适用");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试之间串味的那一份快照：拿它的人独占，**析构时恢复默认**。
    ///
    /// 快照是进程级的，同二进制里并行跑的用例会互相看见（一条置 Denied、另一条
    /// 正在断言「判不出来就不拦人」）。故凡碰快照的用例都先拿这把锁。
    pub(crate) fn snapshot_guard() -> Snapshotted {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        Snapshotted(guard)
    }

    pub(crate) struct Snapshotted(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);

    impl Drop for Snapshotted {
        fn drop(&mut self) {
            record(DiskAccessState::NotProbed);
        }
    }

    /// 默认是「还没探过」：**判不出来就不拦人**（这条是保守方向，钉住它）。
    #[test]
    fn the_default_snapshot_blocks_nobody() {
        let _g = snapshot_guard();
        record(DiskAccessState::NotProbed);
        assert_eq!(state(), DiskAccessState::NotProbed);
        assert!(!is_denied());
        assert!(
            DiskAccessState::NotProbed
                .denied_error_for_project(Path::new("/x"))
                .is_none(),
            "判不出来时不许拦人"
        );
    }

    /// 有授权时**零输出**：探测自己一个字都不写（没有待办、没有错）。
    #[tokio::test]
    async fn a_readable_path_is_granted_and_says_nothing() {
        let _g = snapshot_guard();
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("plain.txt");
        std::fs::write(&path, "可以读").unwrap();
        assert_eq!(probe_path(&path).await, DiskAccessState::Granted);
        record(DiskAccessState::Granted);
        assert!(!is_denied());
        assert!(DiskAccessState::Granted
            .denied_error_for_project(Path::new("/x"))
            .is_none());
    }

    /// **判据是合取**（决策 306 的收窄）：只有「缺授权 **且** 项目在受保护目录下」才拦人。
    ///
    /// 这条是**实现期实测**逼出来的：只看快照的话，`smoke` / `restart_recovery` 两条真二进制
    /// 用例在没开完全磁盘访问的开发机上当场红——而它们的项目在临时目录里，读那些路径
    /// 根本不需要授权。收窄之后：**缺授权只在会真的挂住的那一格拦人**。
    #[test]
    fn the_fast_fail_needs_both_the_denial_and_a_protected_root() {
        let _g = snapshot_guard();
        let home = Path::new("/home/me");
        let docs = Path::new("/home/me/Documents/proj");
        let elsewhere = Path::new("/tmp/proj");
        assert!(
            needs_full_disk_access_in(home, docs),
            "~/Documents 下是受保护的地方"
        );
        assert!(needs_full_disk_access_in(
            home,
            Path::new("/home/me/Desktop/x")
        ));
        assert!(needs_full_disk_access_in(
            home,
            Path::new("/home/me/Downloads/x")
        ));
        assert!(
            !needs_full_disk_access_in(home, elsewhere),
            "临时目录不受保护"
        );
        assert!(
            !needs_full_disk_access_in(home, Path::new("/home/me/DocumentsX/p")),
            "前缀不能只按字符串比（`DocumentsX` 不是 `Documents`）"
        );
        // 项目根取的是**用户家目录**下的那三处，不是 app 家目录（`~/.agentpipeline`）。
        assert!(!needs_full_disk_access_in(
            Path::new("/home/me/.agentpipeline"),
            docs
        ));
    }

    /// 缺授权那句文案**含可操作的下一步**（票 05 的正反两条：那句话在、并且钉住）。
    #[test]
    fn the_denied_hint_carries_the_next_step_and_the_restart_note() {
        let _g = snapshot_guard();
        record(DiskAccessState::Denied);
        assert!(is_denied());
        let home = std::env::var_os("HOME").expect("用例要拿真实 $HOME 拼受保护路径");
        let under_documents = Path::new(&home).join("Documents").join("proj");
        let err = DiskAccessState::Denied
            .denied_error_for_project(&under_documents)
            .expect("缺授权 + 项目在受保护目录下必须给错误");
        let text = err.to_string();
        assert!(text.contains("完全磁盘访问权限"), "{text}");
        assert!(text.contains("系统设置"), "要给出去哪里：{text}");
        assert!(
            text.contains("若你刚刚已开启，请重启应用"),
            "快照有滞后，必须告诉人为什么改了设置还不行：{text}"
        );
    }

    /// 读不动的路径（不存在）**不是**「缺授权」——判不出来就不拦人。
    #[tokio::test]
    async fn a_missing_file_is_not_a_denial() {
        let _g = snapshot_guard();
        let tmp = tempfile::TempDir::new().unwrap();
        assert_eq!(
            probe_path(&tmp.path().join("nope.db")).await,
            DiskAccessState::NotProbed,
            "文件不存在给不出「有没有授权」的结论"
        );
    }

    /// **探测有界**：真会挂住的读（命名管道）在 [`PROBE_TIMEOUT_SEC`] 之后判「未授权」，
    /// 不带计数器、也不无限期挂着。
    ///
    /// 用暂停时钟把上界一步推过去；管道那头由一个 blocking 线程持有，读完放行，
    /// 免得运行时析构等阻塞任务。
    #[tokio::test]
    // 这条也读「卡住的读」那套进程级计数器，必须和别的读它的用例互斥（见 `test_guard` 的说明）；
    // 拿锁跨 await 是有意的（测试里没有死锁风险）。
    #[allow(clippy::await_holding_lock)]
    async fn a_hanging_probe_is_bounded_and_judged_denied() {
        let _g = snapshot_guard();
        // 归零与断言都在这把锁里做：只归零不互斥的话，本用例会在别条用例断言到一半时把它
        // 的累计数抹掉（这条实测过：`crossing_…` 的终态断言读到 0）。
        let _reads = bounded_read::test_guard();
        let tmp = tempfile::TempDir::new().unwrap();
        let fifo = tmp.path().join("fifo");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo");
        assert!(status.success());
        // 前端"卡住的读"那套读数与本探测无关：这条用例顺带钉住它一个数都不动。
        bounded_read::reset_stats();

        tokio::time::pause();
        let probe_target = fifo.clone();
        let probing = tokio::spawn(async move { probe_path(&probe_target).await });
        for _ in 0..200 {
            tokio::task::yield_now().await;
        }
        tokio::time::advance(std::time::Duration::from_secs(PROBE_TIMEOUT_SEC + 1)).await;
        let verdict = probing.await.unwrap();
        assert_eq!(
            verdict,
            DiskAccessState::Denied,
            "连授权检查都挂住了 = 没授权"
        );
        assert_eq!(
            bounded_read::stats().stuck_total,
            0,
            "探测撞墙是正常结论，不该计进「卡住的读」（否则阈值天天告警）"
        );

        // 收尾：开一个写端，让那个挂住的 `open` 返回（读端的 open 一配上写端就回来了）。
        // 用真线程而不是 shell：`sleep` 那种做法是在赌时间，这里靠一次明确的握手。
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let writer_fifo = fifo.clone();
        std::thread::spawn(move || {
            let _ = release_rx.recv();
            if let Ok(mut w) = std::fs::OpenOptions::new().write(true).open(&writer_fifo) {
                use std::io::Write;
                let _ = w.write_all(b"x");
            }
        });
        let _ = release_tx.send(());
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
