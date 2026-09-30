//! 有界阻塞读（决策 302，票 executor-never-returns 01）。
//!
//! 一次节点请求的组装里有五处**同步文件读**：项目指令文件（AGENTS.md）、persona、
//! 技能目录扫描、任务目录下的注入文件、闸门日志。它们原本直接跑在 tokio 的 worker
//! 线程上——环境一卡（`open()` 挂在系统调用里，如完全磁盘访问弹窗无人应答）就**占死
//! 一个 worker**，4 个 worker 占死 1 个，应用整体降级成「看着活着」，而那个 future
//! 永远不返回。
//!
//! 本模块做两件事：
//!
//! 1. **挪线程**：读全部进阻塞池（[`run`]），worker 不再被占死，其余线程照常服务；
//! 2. **装上界**：等待至多 [`BOUNDED_READ_SEC`]，超界即按各自口径降级并**记账**
//!    （[`stats`]）——**这不是掩盖，是观测**：线程不会回来（`spawn_blocking` 的闭包
//!    被 drop 后继续跑），所以读数如实说「现在有几个读卡着」。
//!
//! **为什么不做成有界专用池**：池满会把「环境卡住」翻译成「节点失败 → 重试 → 再卡」，
//! 把一个可观测的挂起变成一串不可观测的失败重试——正是这次故障的形状。阻塞池上限是
//! 512，按当前复发频率够用，超出之前会先被 [`stats`] 的计数看见。
//!
//! **为什么取消等待不叫掩盖**（决策 226 的边界）：`run_inner` 会不会返回由执行权那一层
//! 独立保证（判 run 终态的同一处清 `executor_owner`），本模块的等待上界只决定**这一处读**
//! 降级得多快。若不取消等待，节点会白等下去，而「线程仍在系统调用里」这个事实照样存在
//! ——它由 [`stats`] 与日志说出来，不靠 timeout 藏起来。

use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 一次读的等待上界（秒）。本地文件读的正常耗时是**微秒级**；到秒级就是环境卡住了。
///
/// 与 `crate::git::IS_DIRTY_TIMEOUT_SEC` 同一个量级（那条是「best-effort 的检查不允许
/// 有能力挂住关键路径」），但两者互不依赖：这里管的是组装层的文件读。
pub const BOUNDED_READ_SEC: u64 = 10;

/// 「卡住的读在**累积**」的告警阈值（决策 308，票 07）。
///
/// **钉死、不做成可配**——照决策 224 / 256 / 293 的姿态（没人会调的旋钮比没有更坏）。
///
/// 取值 3 的理由是**正常值是 0**：一次环境卡住（授权随重建失效）不是「某一处读偶然慢」，
/// 而是同一个组装里的五处读**一起**卡住——`stuck_total` 一眼就到 3 甚至更多。所以 3 是
/// 「这不是偶发，是同一次环境故障的多处表现」；等到阻塞池快满（512）才报，报的是结果
/// 而不是原因，那时新节点已经真的起不来了。
pub const STUCK_READ_ATTENTION_THRESHOLD: u64 = 3;

/// tokio 阻塞池的线程上限（默认 512）——**这条观测存在的理由就在这个数上**（决策 308）。
///
/// `spawn_blocking` 的闭包被放弃等待后**线程仍跑到结束**，泄漏是显式接受的（决策 302）。
/// 也就是说每个卡住的读永久吃掉池子里的一格：一次环境故障卡住五处读、每小时复发的任务
/// 再卡五处……512 格用完那一刻，**新节点会真的起不来**，而症状会退化成「应用整体停摆」，
/// 与这次「看着活着其实降级」是同一类误诊。所以「卡住的读」必须是个**能读的数**，
/// 不是一句口号：它在累积就该有人去看完全磁盘访问的授权，而不是等池子满。
///
/// 如实记：这个 512 是 tokio 的默认值，本仓没有另配；写在这里是为了让「为什么要有这条
/// 观测」有个可核的数，不是又一次配置源。
pub const BLOCKING_POOL_CAP: usize = 512;

/// 阈值刚被跨过、且**还没人认领**。
///
/// 认领用 `swap(false)`：一次发作只让**一个**调用方拿到 `true`，于是落**一条**待办，
/// 而不是每个读各一条。要再拿到 `true`，得等维护作业调 [`reset_stats`] 之后重新累积过
/// 阈值——「累积」本来就是跨读的，不该按读计数。
static ATTENTION_DUE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 有界等待的结局。三支分开是因为调用方要**分得开**「读到没有」与「不知道」：
/// 前两支是确定的，第三支是「读还挂在系统调用里，此刻的真实状态未知」。
#[derive(Debug)]
pub enum Offloaded<T> {
    /// 闭包正常返回（`read_to_string` 那一类里，`io::Result` 的 `Err` 也走这一支）。
    Done(T),
    /// 闭包 panic（join 失败）。与「超界」分开记：前者是确定的坏，后者是不知道。
    Panicked(String),
    /// 等待超界：读仍挂在系统调用里，**线程不会回来**。
    Stuck,
}

impl<T> Offloaded<T> {
    /// 是否超界（调用方降级分支最常问的那一句）。
    pub fn is_stuck(&self) -> bool {
        matches!(self, Offloaded::Stuck)
    }

    /// 取值：正常返回给 `Some`，超界 / panic 给 `None`。
    ///
    /// 给「读不到就用既有降级口径」那几处用（AGENTS.md 缺省上下文、persona 回落内嵌、
    /// 注入文件不渲染）——它们的降级与「文件不存在」本来就是同一条路。
    pub fn ok(self) -> Option<T> {
        match self {
            Offloaded::Done(v) => Some(v),
            _ => None,
        }
    }
}

/// 「卡住的读」的观测读数（票 07 的落点）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockedReadStats {
    /// **现在**有几个读超界、仍挂在系统调用里（线程不会回来）。
    pub stuck_now: usize,
    /// 累计超界次数（含后来返回的）。
    pub stuck_total: u64,
    /// 见过的最长一次等待（毫秒）。
    pub longest_wait_ms: u64,
}

static STUCK_NOW: AtomicUsize = AtomicUsize::new(0);
static STUCK_TOTAL: AtomicU64 = AtomicU64::new(0);
static LONGEST_WAIT_MS: AtomicU64 = AtomicU64::new(0);

/// 当前读数。进程级静态计数器——装配层深在调用链里，拿不到注入句柄，
/// 而「有几个读卡着」本来就是**整机的一个数**，不是某次请求的私有状态。
pub fn stats() -> BlockedReadStats {
    BlockedReadStats {
        stuck_now: STUCK_NOW.load(Ordering::Relaxed),
        stuck_total: STUCK_TOTAL.load(Ordering::Relaxed),
        longest_wait_ms: LONGEST_WAIT_MS.load(Ordering::Relaxed),
    }
}

/// 读数归零。**测试专用**：同一个测试二进制里多条用例共用这份进程级计数器，
/// 不归零就分不清「这次卡住」与「上次遗留」。
pub fn reset_stats() {
    STUCK_NOW.store(0, Ordering::Relaxed);
    STUCK_TOTAL.store(0, Ordering::Relaxed);
    LONGEST_WAIT_MS.store(0, Ordering::Relaxed);
    ATTENTION_DUE.store(false, Ordering::Relaxed);
}

/// 阈值刚被跨过吗——**至多一个调用方**拿到 `true`（票 07 的落账口）。
///
/// 调用方是组装层（`RequestPlan::assemble_seeded`）：它知道这一轮是**哪个任务**在组装，
/// 而待办表是按任务记账的。计数器本身是进程级的（读点深在调用链里拿不到句柄），
/// 两者在这里交汇一次。
pub fn take_attention_due() -> bool {
    ATTENTION_DUE.swap(false, Ordering::Relaxed)
}

/// 碰计数器的那几条用例之间的互斥。**测试专用**：计数器是进程级的，同二进制里
/// 并行跑的用例会互相串味（一条在断言「零」，另一条正把「卡着的」加一）。
#[cfg(test)]
pub(crate) fn test_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// 「这个读是否曾超界」的一次性标记。用状态机而不是两个 bool：超界那一刻与闭包返回
/// 那一刻可能**同时**发生，两个 `AtomicBool` 各判各的会重复计数（涨一个、跌两个）。
struct StuckFlag(AtomicU8);

const RUNNING: u8 = 0;
const STUCK: u8 = 1;
const FINISHED: u8 = 2;

impl StuckFlag {
    fn new() -> Self {
        Self(AtomicU8::new(RUNNING))
    }

    /// 由**等待方**在超界那一刻调用：`RUNNING → STUCK` 的赢家负责记账，返回是否记账。
    fn mark_stuck(&self) -> bool {
        self.0
            .compare_exchange(RUNNING, STUCK, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    }

    /// 由**读自己**在返回时调用：`STUCK → FINISHED` 返回真（这个读曾超界，得把
    /// 「卡着的」减回去）；`RUNNING → FINISHED` 返回假（正常返回，不记账）。
    fn finish(&self) -> bool {
        match self
            .0
            .compare_exchange(STUCK, FINISHED, Ordering::AcqRel, Ordering::Relaxed)
        {
            Ok(_) => true,
            Err(_) => {
                let _ =
                    self.0
                        .compare_exchange(RUNNING, FINISHED, Ordering::AcqRel, Ordering::Relaxed);
                false
            }
        }
    }
}

/// 在阻塞池里执行一段同步读，等待至多 [`BOUNDED_READ_SEC`]。
///
/// `site` / `label` 只进日志（哪一处读、读的哪个路径）——超界时那行日志是**唯一**
/// 能把「应用忽然变慢」和「某个系统调用挂在别处」连起来的东西。
pub async fn run<T, F>(site: &'static str, label: impl Into<String>, f: F) -> Offloaded<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let label = label.into();
    let started = Instant::now();
    let flag = Arc::new(StuckFlag::new());
    let flag_in_read = flag.clone();
    let label_in_read = label.clone();
    let handle = tokio::task::spawn_blocking(move || {
        let out = f();
        if flag_in_read.finish() {
            // 曾超界的读回来了：「卡着的」减一，并把真实等待刷新进读数。
            let waited = started.elapsed();
            let _ = STUCK_NOW.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                Some(v.saturating_sub(1))
            });
            LONGEST_WAIT_MS.fetch_max(waited.as_millis() as u64, Ordering::Relaxed);
            tracing::warn!(
                site,
                label = %label_in_read,
                waited_ms = waited.as_millis() as u64,
                "卡住的读终于返回（线程不回收：阻塞池上限 512）"
            );
        }
        out
    });
    match tokio::time::timeout(Duration::from_secs(BOUNDED_READ_SEC), handle).await {
        Ok(Ok(value)) => Offloaded::Done(value),
        Ok(Err(e)) => Offloaded::Panicked(e.to_string()),
        Err(_) => {
            if flag.mark_stuck() {
                STUCK_NOW.fetch_add(1, Ordering::Relaxed);
                let total = STUCK_TOTAL.fetch_add(1, Ordering::Relaxed) + 1;
                if total >= STUCK_READ_ATTENTION_THRESHOLD {
                    // 跨过阈值 = 「在同一次环境故障里已经卡了不止一处」（票 07）。
                    // 只置位、不落账：落账要一个任务 id，那只有组装层有。
                    ATTENTION_DUE.store(true, Ordering::Relaxed);
                }
                LONGEST_WAIT_MS.fetch_max(BOUNDED_READ_SEC * 1000, Ordering::Relaxed);
                tracing::warn!(
                    site,
                    label = %label,
                    waited_secs = BOUNDED_READ_SEC,
                    stuck_total = total,
                    "读超界，已放弃等待（读仍挂在系统调用里，线程不会回来）"
                );
            }
            Offloaded::Stuck
        }
    }
}

/// 有界读一个文件成字符串（`io::Result` 原样带出：调用方各自的「读不到」口径不同）。
pub async fn read_to_string(site: &'static str, path: &Path) -> Offloaded<std::io::Result<String>> {
    let path = path.to_path_buf();
    run(site, path.display().to_string(), move || {
        std::fs::read_to_string(&path)
    })
    .await
}

/// 有界执行一段同步代码，**不碰 [`stats`] 那套计数**（决策 306 的启动探测用）。
///
/// 与 [`run`] 同一份「挪进阻塞池 + 有界等待」的语义，只差一处：它不计进「卡住的读」。
/// 理由是**探测本来就以撞墙为一条正常结论**——完全磁盘访问缺失时那次 `open()` 必然超界，
/// 计进去会让阈值在正常不过的状态下天天告警（把一个专为「环境异常」造的读数变成噪声）。
///
/// 超界时照旧记一行 warn（「谁、等了多久、线程不会回来」），并返回 [`Offloaded::Stuck`]：
/// 不给计数不等于不吭声。
pub async fn probe<T, F>(
    site: &'static str,
    label: impl Into<String>,
    bound_secs: u64,
    f: F,
) -> Offloaded<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let label = label.into();
    let label_for_log = label.clone();
    let handle = tokio::task::spawn_blocking(f);
    match tokio::time::timeout(Duration::from_secs(bound_secs), handle).await {
        Ok(Ok(value)) => Offloaded::Done(value),
        Ok(Err(e)) => Offloaded::Panicked(e.to_string()),
        Err(_) => {
            tracing::warn!(
                site,
                label = %label_for_log,
                waited_secs = bound_secs,
                "探测超界（不上报计数：探测撞墙是它的正常结论之一）"
            );
            Offloaded::Stuck
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 计数器是进程级的：每条用例自己归零，免得上一条的遗留影响断言。
    fn fresh() -> BlockedReadStats {
        reset_stats();
        stats()
    }

    /// 正常路径：值原样带出，读数**一个都不动**（不产生噪声）。
    #[tokio::test]
    // 计数器是进程级的，这几条**必须**互斥跑；拿锁跨 await 是有意的（测试里没有死锁风险）。
    #[allow(clippy::await_holding_lock)]
    async fn a_normal_read_moves_no_counter() {
        let _guard = test_guard();
        let _ = fresh();
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("f.md");
        std::fs::write(&path, "内容").unwrap();
        let got = read_to_string("test", &path).await;
        assert!(matches!(got, Offloaded::Done(Ok(ref s)) if s == "内容"));
        assert_eq!(
            stats(),
            BlockedReadStats {
                stuck_now: 0,
                stuck_total: 0,
                longest_wait_ms: 0
            }
        );
        // 反向断言（票 07）：没卡住就不该有待落账的告警——不产生噪声。
        assert!(!take_attention_due(), "没有卡住的读时不落待办");
    }

    /// **阈值跨过 → 置位，且只能被认领一次**（票 07：一次发作一条待办，不是每个读各一条）。
    ///
    /// 用暂停时钟把 [`BOUNDED_READ_SEC`] 一步推过去：那三处读**真的**同时挂在
    /// 阻塞池里（等一条被显式放行的消息），走的正是生产里那一条路。
    #[tokio::test]
    // 计数器是进程级的，这几条**必须**互斥跑；拿锁跨 await 是有意的（测试里没有死锁风险）。
    #[allow(clippy::await_holding_lock)]
    async fn crossing_the_threshold_raises_exactly_one_claimable_note() {
        let _guard = test_guard();
        let _ = fresh();
        tokio::time::pause();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let rx = Arc::new(std::sync::Mutex::new(rx));
        let mut reads = Vec::new();
        for _ in 0..STUCK_READ_ATTENTION_THRESHOLD {
            let rx = rx.clone();
            reads.push(tokio::spawn(async move {
                run("test", "hang", move || {
                    // 一直等放行消息：这就是「挂在系统调用里」的形状。
                    let _ = rx.lock().unwrap().recv();
                })
                .await
            }));
        }
        // 让它们的 `timeout` 都挂上，再把时钟推过界。
        for _ in 0..200 {
            tokio::task::yield_now().await;
        }
        assert!(
            stats().stuck_total < STUCK_READ_ATTENTION_THRESHOLD,
            "推时钟之前不该有超界（否则这条用例没在测阈值）：{:?}",
            stats()
        );
        tokio::time::advance(Duration::from_secs(BOUNDED_READ_SEC + 1)).await;
        for read in reads {
            assert!(read.await.unwrap().is_stuck(), "三处读都该超界");
        }
        assert_eq!(stats().stuck_total, STUCK_READ_ATTENTION_THRESHOLD);
        assert_eq!(
            stats().stuck_now as u64,
            STUCK_READ_ATTENTION_THRESHOLD,
            "还没返回：现在卡着几个是能读的数"
        );
        assert!(take_attention_due(), "跨过阈值要置位");
        assert!(!take_attention_due(), "认领过一次就不该再有人拿到");

        // 放行：读返回，「卡着的」减回去（累计不跌）。
        for _ in 0..STUCK_READ_ATTENTION_THRESHOLD {
            tx.send(()).unwrap();
        }
        // **等那个减法落地，而不是数 yield**（2026-09-30，决策 337 的 CI 复跑实测：这条在
        // CI 上偶发红在 `left: 1, right: 0`）。减法在**阻塞池线程**里做（`StuckFlag::finish`
        // 之后那一段），而 `yield_now` 只让出当前运行时的任务队列——两个线程之间没有先后
        // 关系，机器一忙就还没轮到它们收尾。判据一个字不改（返回之后必须归零），只是给它一个
        // 有上限的真实时间窗：到点还不归零，红的就是真东西。
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while stats().stuck_now != 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(stats().stuck_now, 0, "返回之后不再算「卡着」");
        assert_eq!(stats().stuck_total, STUCK_READ_ATTENTION_THRESHOLD);
    }

    /// 读不了（文件不存在）走 `Done(Err(..))`，**不是**超界——两者分得开。
    #[tokio::test]
    // 计数器是进程级的，这几条**必须**互斥跑；拿锁跨 await 是有意的（测试里没有死锁风险）。
    #[allow(clippy::await_holding_lock)]
    async fn an_unreadable_file_is_not_a_stuck_read() {
        let _guard = test_guard();
        let _ = fresh();
        let tmp = tempfile::TempDir::new().unwrap();
        let got = read_to_string("test", &tmp.path().join("nope.md")).await;
        assert!(matches!(got, Offloaded::Done(Err(_))), "读不到是确定的事实");
        assert_eq!(stats().stuck_total, 0);
    }

    /// 闭包 panic 与超界分开记（join 失败不冒充「卡住」）。
    #[tokio::test]
    // 计数器是进程级的，这几条**必须**互斥跑；拿锁跨 await 是有意的（测试里没有死锁风险）。
    #[allow(clippy::await_holding_lock)]
    async fn a_panicking_read_is_reported_as_panicked() {
        let _guard = test_guard();
        let _ = fresh();
        let got = run("test", "panic", || -> u8 { panic!("boom") }).await;
        assert!(matches!(got, Offloaded::Panicked(_)), "{got:?}");
        assert_eq!(stats().stuck_total, 0);
    }

    /// **透明性**：同一路径、同一字节。这是「正常路径逐字不变」的机制那一半——
    /// 组装出的两段 prompt 之所以与挪池之前一字不差，是因为读取层原样带出
    /// `std::fs::read_to_string` 的结果（另一半是各读点既有的回落口径没动）。
    #[tokio::test]
    // 计数器是进程级的，这几条**必须**互斥跑；拿锁跨 await 是有意的（测试里没有死锁风险）。
    #[allow(clippy::await_holding_lock)]
    async fn the_bounded_read_returns_exactly_what_the_sync_read_returns() {
        let _guard = test_guard();
        let _ = fresh();
        let tmp = tempfile::TempDir::new().unwrap();
        let cases: [(&str, &[u8]); 4] = [
            ("content.md", "正文\n第二行\n".as_bytes()),
            ("empty.md", b""),
            ("blank.md", "   \n".as_bytes()),
            // 非 UTF-8：`read_to_string` 会报错——两侧必须报同一种错（都是 Err）
            ("binary.md", &[0x80, 0x81, 0xff]),
        ];
        for (name, bytes) in cases {
            let path = tmp.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            let expected = std::fs::read_to_string(&path);
            let got = read_to_string("test", &path).await;
            match (got, expected) {
                (Offloaded::Done(Ok(a)), Ok(b)) => assert_eq!(a, b, "{name} 字节要一致"),
                (Offloaded::Done(Err(_)), Err(_)) => {}
                (got, expected) => panic!("{name} 两侧结论不一致：{got:?} vs {expected:?}"),
            }
        }
        // 文件不存在同样是同一种 Err（不是超界、不是空串）
        let missing = tmp.path().join("nope.md");
        assert!(std::fs::read_to_string(&missing).is_err());
        assert!(matches!(
            read_to_string("test", &missing).await,
            Offloaded::Done(Err(_))
        ));
    }
}
