//! Git 操作封装（决策 12 / 41 / 61 / 73 / 96 / 97 / 125）。
//!
//! 统一走 **git2（libgit2 绑定）**（决策 12）：git2 的类型不是 `Send`/`Sync`，
//! 所有操作都在 `tokio::task::spawn_blocking` 里执行，不阻塞 tokio 运行时。
//!
//! 与系统 git CLI 的差异已按 git2 语义重写（2026-09-12 用户裁决）：
//! - rebase 用 libgit2 的 rebase 状态机，冲突时保持 rebase 中间态返回
//!   `RebaseOutcome::Conflict`，由调用方 `rebase_abort`（决策 74 不变）；
//! - merge 阶段 B 改为 **内存合并**（`merge_commits` → 写树 → 双亲 commit），
//!   直接写回 `refs/heads/{default_branch}`，不再建临时 detached worktree；
//!   「合入结果必须显式写回分支」的不变量不变（决策 73 / 97）；
//! - `clean -fdx` 用 statuses 枚举未跟踪（含 ignored）文件后删除。
//!
//! testkit 的场景仓库搭建仍走系统 git CLI（决策 146 修订：那只是测试脚手架）。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::clock::{Clock, SystemClock};
use crate::{Error, Result};

/// 供集成测试直接操作底层仓库（例如检查 rebase abort 后的 tracked 状态）。
pub use git2;

/// git 操作入口。
#[derive(Debug, Clone, Copy, Default)]
pub struct Git;

/// rebase 结果（决策 74 / 96）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RebaseOutcome {
    /// rebase 成功，返回 rebase 后 HEAD 的 SHA。
    Clean { head: String },
    /// 有冲突且无法自动解决——rebase 保持中断态，调用方必须先 `rebase_abort`。
    Conflict { files: Vec<String> },
}

/// rebase + 自动解决冲突的结果（pipeline-spec §6，票 15）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoRebaseOutcome {
    /// 无冲突，rebase 完成。
    Clean { head: String },
    /// 冲突全部被机械自动解决，rebase 完成并记录了被解决的文件。
    AutoResolved { head: String, files: Vec<String> },
    /// 存在无法机械判定的冲突：已 `rebase --abort` 恢复干净状态，调用方按决策 74 打回 develop。
    Conflict { files: Vec<String> },
}

/// 合入结果（决策 73 / 97）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeOutcome {
    /// 是否为 fast-forward。
    pub fast_forward: bool,
    /// 合入后的 commit SHA（ff 时为被合入分支的 tip）。
    pub commit: String,
}

/// push 结果（决策 393）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushOutcome {
    /// 已推送到该 remote。
    Pushed {
        /// 实际使用的 remote 名（origin 优先，否则配置里的第一个）。
        remote: String,
    },
    /// 仓没有配置任何 remote：跳过，不算失败（纯本地仓兼容）。
    NoRemote,
}

pub(crate) fn gerr(e: git2::Error) -> Error {
    Error::Git(e.to_string())
}

/// 单次 git 操作的兜底上限（秒）。
///
/// **为什么必须有**（决策 209）：libgit2 的调用是没有上限的，而节点超时**杀不掉**它们——
/// 系统节点没有进程组（`kanban_node_runs.process_group_id` 为空），`handle_timeout` 按进程组
/// 杀，于是「超时」只往台账上写了一个字，那份阻塞的工作还在跑、还占着锁，任务从此永久
/// 卡在 `running` 且不会转 `pending`（零信号）。
///
/// 2026-09-17 的实例：`do_init` 的脏工作区检查里，`git2::Repository::open` 读 `.git/config`
/// 时被 macOS 拦在 `open()` 里（未签名的 app 没有 `~/Documents` 的访问授权），进程 0% CPU、
/// state=S、永不返回。任务卡了 4 小时，直到外部 `sample` 附进程才拿到栈。
///
/// 取值压在 `node_idle_timeout_sec`（默认 300）**之下**：让挂死以「一个普通的节点错误」
/// 的形式浮出来——可归因、可重试、可播报——而不是变成一个杀不掉的超时。
/// **超时不会终止那个阻塞线程**（`JoinHandle` 被丢弃、任务 detach 后继续跑）；这是拿
/// 一个后台线程换关键路径能继续，不是真取消。**这一点在持锁的调用上更重**：
/// `init_worktree` 的整个闭包在 `with_worktree_lock` 里跑，若它超时，那个线程仍持有该仓库的
/// 分桶锁，此**同一仓库的每一次建 worktree 都会阻塞**。所以这条上限是「让关键路径能继续」
/// 的权宜，不是「让 git 调用可取消」的答案；真取消需要另一套机制（见
/// `.scratch/foreman-watch/issues/10-repair-worktree.md`）。
const GIT_OP_TIMEOUT_SEC: u64 = 180;

/// 脏工作区检查的上限（秒）。比 [`GIT_OP_TIMEOUT_SEC`] 短一个数量级。
///
/// 因为它换来的只是一条 `warn!`。同一个 2026-09-17 的实例里，正是这条只值一条警告的
/// 检查把 `init.execute` 挂死了——**best-effort 的检查不允许有能力挂住关键路径**。
const IS_DIRTY_TIMEOUT_SEC: u64 = 10;

/// push 的上限（秒）。网络往返 + 大对象上传的量级；凭据提示已被
/// `GIT_TERMINAL_PROMPT=0` 关掉，真挂住只可能是远端无响应。
const GIT_PUSH_TIMEOUT_SEC: u64 = 120;

/// 未提交改动清单的条数上限（票 04 的简报用）：只给「有哪些文件」，不做全量导出。
const DIRTY_FILES_MAX: usize = 50;

/// 在阻塞线程池里执行一段同步 git2 逻辑（决策 12），带 [`GIT_OP_TIMEOUT_SEC`] 兜底。
pub(crate) async fn blocking<T, F>(f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    blocking_within(GIT_OP_TIMEOUT_SEC, f).await
}

/// [`blocking`] 的实际实现，上限可指定（内联测试需要注入一个短上限）。
async fn blocking_within<T, F>(timeout_sec: u64, f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    let limit = std::time::Duration::from_secs(timeout_sec);
    match tokio::time::timeout(limit, tokio::task::spawn_blocking(f)).await {
        Ok(Ok(inner)) => inner,
        Ok(Err(e)) => Err(Error::Git(format!("spawn_blocking 任务失败：{e}"))),
        Err(_) => Err(Error::Git(format!("git 操作超时（{timeout_sec}s）"))),
    }
}

fn open(path: &Path) -> Result<git2::Repository> {
    git2::Repository::open(path).map_err(gerr)
}

/// fetch `origin`（--prune）——全仓唯一的一份 fetch（决策 415 形状 1 的预构）。
///
/// 两处调用，**失败语义由调用方决定**，这正是这份实现必须只有一份、而失败处置
/// 必须留在调用方的原因：
/// - 建 worktree（[`Git::init_worktree_named`]）：失败**不阻断**（离线场景），照旧用
///   本地引用当基准——旧基准只让修复的起点旧一点，改动本身照常成立；
/// - 修复合入（[`crate::pipeline::repair::fetch_origin_tip`]）：失败**即拒执**——这条
///   路上「基准有没有前进」的判定直接决定合入落在哪条线上，取不到就不敢断言
///   （决策 415，2026-10-08 事故）。
///
/// 三种读数：`Ok(true)` 取到了；`Ok(false)` 这个仓没有 `origin` 远端（没有可取的）；
/// `Err` 有远端但取不到——只有「远端不存在」是合法的 false，别的错误照实上传，
/// 不许把一次真失败折叠成「没有 origin」。
pub(crate) fn fetch_origin(repo: &git2::Repository) -> Result<bool> {
    let mut remote = match repo.find_remote("origin") {
        Ok(r) => r,
        Err(e) if e.code() == git2::ErrorCode::NotFound => return Ok(false),
        Err(e) => return Err(gerr(e)),
    };
    let mut opts = git2::FetchOptions::new();
    opts.prune(git2::FetchPrune::On);
    remote
        .fetch::<&str>(&[], Some(&mut opts), None)
        .map_err(gerr)?;
    Ok(true)
}

/// 索引态那一字符（`git status --short` 的 X 位）；无索引态改动时是空格。
/// 未跟踪文件两列都是 `?`（与 porcelain 一致）——git2 只置 `WT_NEW`，这里补上前一列。
fn index_state(s: git2::Status) -> char {
    if s.contains(git2::Status::WT_NEW) {
        '?'
    } else if s.contains(git2::Status::INDEX_NEW) {
        'A'
    } else if s.contains(git2::Status::INDEX_MODIFIED) {
        'M'
    } else if s.contains(git2::Status::INDEX_DELETED) {
        'D'
    } else if s.contains(git2::Status::INDEX_RENAMED) {
        'R'
    } else {
        ' '
    }
}

/// 工作区态那一字符（`git status --short` 的 Y 位）；未跟踪按 `?`。
fn worktree_state(s: git2::Status) -> char {
    if s.contains(git2::Status::WT_NEW) {
        '?'
    } else if s.contains(git2::Status::WT_MODIFIED) {
        'M'
    } else if s.contains(git2::Status::WT_DELETED) {
        'D'
    } else if s.contains(git2::Status::WT_RENAMED) {
        'R'
    } else {
        ' '
    }
}

/// 建 worktree 的跨任务互斥锁（按仓库路径分桶）。
///
/// **为什么必须有：**libgit2 建 worktree 时对**共享的** `{repo}/.git/worktrees`
/// 目录先 `path_exists` 再 `mkdir(GIT_MKDIR_EXCL)`（worktree.c 的 TOCTOU），两个任务
/// 同时启动（决策 98 准入允许的常态用法）会双双看到"不存在"、各建一次，后者拿到
/// `EEXIST`：init 阶段直接报 `failed to make directory '.../.git/worktrees': directory
/// exists`，任务挂在 init 重试耗尽——用户看到的是任务一创建就失败。主流程票 09 的
/// 浏览器并发用例实测挂在这里。
///
/// 锁粒度按 `{repo}/.git` 路径取，与 `.git/worktrees` 的共享范围一致：不同仓库互不
/// 阻塞，同仓库串行。libgit2 侧无法加锁（无开关），故在调用侧串行化这个窗口。
fn worktree_creation_lock(repo: &Path) -> &'static Mutex<()> {
    static LOCKS: OnceLock<Mutex<std::collections::HashMap<PathBuf, &'static Mutex<()>>>> =
        OnceLock::new();
    let locks = LOCKS.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let mut map = locks.lock().unwrap_or_else(|e| e.into_inner());
    map.entry(repo.to_path_buf())
        .or_insert_with(|| Box::leak(Box::new(Mutex::new(()))))
}

/// 在[`worktree_creation_lock`]保护下执行 `f`（同步上下文；调用方在 spawn_blocking 里）。
fn with_worktree_lock<T>(repo: &Path, f: impl FnOnce() -> Result<T>) -> Result<T> {
    with_worktree_lock_within(&SystemClock, WORKTREE_LOCK_WAIT_SEC, repo, f)
}

/// 等这把锁的上限（秒）。
///
/// **为什么连等锁也要有上限**（2026-09-18 实测）：`GIT_OP_TIMEOUT_SEC` 的超时**不会终止**
/// 那个仍在 `open()` 里阻塞的线程，而它手里正握着这把锁。当天给桌面进程采的栈里就有三条
/// 这样的线程（两条 `Git::is_dirty`、一条 `Git::init_worktree_named`），于是同一仓库之后
/// 每一次建 worktree 都只能等满 180s 的通用上限，报出来的却是「git 操作超时」——
/// 一句话指向 libgit2，而真凶是「这把锁被一个挂死的线程占着」。
///
/// 取值对**正常**竞争是极宽裕的量级：这把锁只罩着 `branch` + `worktree add` 那几毫秒，
/// fetch 在锁外（见 `init_worktree_named`）。真等满这一档，只可能是上一次调用挂住了。
const WORKTREE_LOCK_WAIT_SEC: u64 = 60;

/// 同上，等待上限与**时钟**都可指定（照 `blocking_within` 的注入形状；生产传
/// [`SystemClock`]——工具上下文（`start_repair` 链）手里没有 clock 可传，不为此动
/// `ToolExecutor`，票 02 只改取时点：`Instant` 换成 [`Clock`] 接缝的读法）。
fn with_worktree_lock_within<T>(
    clock: &dyn Clock,
    wait_sec: u64,
    repo: &Path,
    f: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let lock = worktree_creation_lock(repo);
    let deadline = clock.now() + chrono::Duration::seconds(wait_sec as i64);
    let _guard = loop {
        match lock.try_lock() {
            Ok(guard) => break guard,
            // 毒化不当作错误：这把锁护的是一段没有共享状态的窗口，前一个持有者 panic
            // 不该让之后每一次建 worktree 都失败（与 `lock()` 的既有姿态一致）。
            Err(std::sync::TryLockError::Poisoned(poisoned)) => break poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                if clock.now() >= deadline {
                    return Err(Error::Git(format!(
                        "建 worktree 的互斥锁被占用超过 {wait_sec}s 没放（同一仓库上一次建 \
                         worktree 的调用可能还挂在 libgit2 里）：这个仓库的建 worktree 会继续\
                         被挡住，别的仓库不受影响"
                    )));
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    };
    f()
}

/// merge 阶段 A 自愈提交的标记（决策 416 B）：进 commit message 首行，供 `git log --grep` 认账。
pub const MERGE_AUTOSAVE_MARK: &str = "[autosave]";

/// 决策 416 C：这一条 git 错误是不是**确定性的前置条件失败**（重试必失败，要人先去修）？
///
/// 判的是「来自哪个 git 操作 + 字面可核的那句话」，**刻意不看 `ErrorClass`**——
/// `agent/repo.rs` 有实测教训（`:1112`：协议层被拒报出来的 class 也是 `Net`，class 会把
/// 不同因归一类）。宁可漏判（落回普通失败，至少 message 是原话），也不要把一条会自己
/// 恢复的失败误判成「你得先去修环境」——那比现在的误标更贵。
///
/// 每一条都指向一句**人能修、且修完重试就会过**的事实：
/// - `unstaged changes exist in workdir` / `uncommitted changes exist in index`：libgit2
///   `rebase_ensure_not_dirty`（`src/libgit2/rebase.c`）的两处检查，即本次事故的原话；
/// - `HEAD 无指向` / `尚无任何提交`：本仓自写的 unborn HEAD 判定（`rebase` 起点与
///   `init` 建 worktree 那两处）；
/// - `could not find repository`：仓 / worktree 不在了（`git2::Repository::open`）。
pub fn is_environment_precondition(message: &str) -> bool {
    const FINGERPRINTS: [&str; 5] = [
        "unstaged changes exist in workdir",
        "uncommitted changes exist in index",
        "HEAD 无指向",
        "尚无任何提交",
        "could not find repository",
    ];
    FINGERPRINTS.iter().any(|f| message.contains(f))
}

/// 流水线提交者身份（与原 CLI `-c user.name/email` 一致）。
fn committer() -> Result<git2::Signature<'static>> {
    git2::Signature::now("AgentPipeline", "agentpipeline@localhost").map_err(gerr)
}

/// 合入把引用前移后，把**检出了该分支的工作区**同步到新 tip。
///
/// 为什么必须做：内存合入（决策 73 / 97）只写 `refs/heads/{default_branch}`，
/// 不碰索引与工作区。若该分支在某个工作区被检出（用户的**项目主仓库**就是这种情形），
/// 引用前移后索引/工作区仍停在旧提交 → `git status` 出现**已暂存**的改动（用户一
/// `git commit` 就把合入回滚掉），磁盘上仍是旧代码，且决策 61 的「目标分支工作区
/// 不干净」检查会让**下一个任务在 merge 处被误判为脏**并挂起——合入成功反而破坏了
/// 下一次合入。
///
/// 实现取「硬同步」姿态：`checkout_head(force)` + 索引读回 HEAD。合入前调用方已按
/// 决策 61 校验过工作区干净（`allow_dirty_worktree_merge = false` 为默认），因此
/// 这里不会丢弃用户未提交的改动；force 只用于覆盖「引用已前移、索引尚未更新」这一
/// 由本次合入自身造成的差异。
fn sync_checked_out_worktree(repo: &git2::Repository, default_ref: &str) -> Result<()> {
    let branch_name = default_ref
        .strip_prefix("refs/heads/")
        .unwrap_or(default_ref);
    // 只有「HEAD 正指向该分支」的工作区才需要同步；否则（纯裸仓库 / 分支未被检出，
    // 例如另一任务正在自己的 worktree 里工作）不得触碰任何工作区。
    let head_is_default = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().map(|s| s == branch_name))
        .unwrap_or(false);
    if !head_is_default {
        return Ok(());
    }
    let mut opts = git2::build::CheckoutBuilder::new();
    opts.force();
    repo.checkout_head(Some(&mut opts)).map_err(gerr)?;
    Ok(())
}

/// 解析 diff range（`a..b` / `a...b` / `..b` / `a..`，空侧为 HEAD）。
fn split_range(range: &str) -> (String, String) {
    let (a, b) = range.split_once("..").unwrap_or((range, ""));
    let a = a.trim_end_matches('.').trim();
    let b = b.trim_start_matches('.').trim();
    let head = "HEAD".to_string();
    (
        if a.is_empty() {
            head.clone()
        } else {
            a.to_string()
        },
        if b.is_empty() { head } else { b.to_string() },
    )
}

/// 决策 74：打回 develop 前的系统清理——读取 rebase 冲突状态时复用。
fn conflicted_paths(repo: &git2::Repository) -> Result<Vec<String>> {
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(false).include_ignored(false);
    let statuses = repo.statuses(Some(&mut opts)).map_err(gerr)?;
    Ok(statuses
        .iter()
        .filter(|e| e.status() == git2::Status::CONFLICTED)
        .filter_map(|e| e.path().map(str::to_string))
        .collect())
}

/// `clean -fdx` 等价物：枚举未跟踪 + ignored 文件并删除，随后剪掉空目录。
fn clean_untracked(repo: &git2::Repository, root: &Path) -> Result<()> {
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true)
        .include_ignored(true)
        .recurse_untracked_dirs(true);
    let statuses = repo.statuses(Some(&mut opts)).map_err(gerr)?;
    let target = git2::Status::WT_NEW.union(git2::Status::IGNORED);
    let mut removed = Vec::new();
    for entry in statuses.iter() {
        if !entry.status().intersects(target) {
            continue;
        }
        let Some(rel) = entry.path() else { continue };
        let abs = root.join(rel);
        if abs.is_dir() {
            let _ = std::fs::remove_dir_all(&abs);
        } else if abs.exists() {
            let _ = std::fs::remove_file(&abs);
        }
        removed.push(rel.to_string());
    }
    // 剪掉因删除而变空的父目录（不越过仓库根）
    let mut dirs: Vec<PathBuf> = removed
        .iter()
        .filter_map(|rel| root.join(rel).parent().map(Path::to_path_buf))
        .collect();
    dirs.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    dirs.dedup();
    for dir in dirs {
        if dir.starts_with(root) && dir != root {
            let _ = std::fs::remove_dir(&dir);
        }
    }
    Ok(())
}

impl Git {
    /// 是否为 git 仓库（`POST /projects` 的立即校验，决策 61）。
    pub async fn is_git_repo(&self, path: &Path) -> bool {
        let p = path.to_path_buf();
        blocking(move || Ok(git2::Repository::open(&p).is_ok()))
            .await
            .unwrap_or(false)
    }

    /// unborn HEAD（零提交）：init 必须**明确报错**，不抛底层 git 错误（决策 61）。
    ///
    /// 只有 unborn HEAD 才返回 true；其他失败（损坏仓库、权限）如实上抛，
    /// 不冒充"unborn"让调用方误报"仓库无提交"。
    pub async fn unborn_head(&self, path: &Path) -> Result<bool> {
        let p = path.to_path_buf();
        blocking(move || {
            let repo = open(&p)?;
            let unborn = match repo.head() {
                Ok(_) => false,
                Err(e) if e.code() == git2::ErrorCode::UnbornBranch => true,
                Err(e) => return Err(gerr(e)),
            };
            Ok(unborn)
        })
        .await
    }

    /// 某个 ref 的 commit SHA。
    pub async fn rev_parse(&self, path: &Path, refname: &str) -> Result<String> {
        let p = path.to_path_buf();
        let refname = refname.to_string();
        blocking(move || {
            let repo = open(&p)?;
            let commit = repo
                .revparse_single(&refname)
                .and_then(|o| o.peel_to_commit())
                .map_err(gerr)?;
            Ok(commit.id().to_string())
        })
        .await
    }

    /// HEAD 所在分支名（detached 时为 None）。
    pub async fn current_branch(&self, path: &Path) -> Result<Option<String>> {
        let p = path.to_path_buf();
        blocking(move || {
            let repo = open(&p)?;
            let branch = match repo.head() {
                Ok(r) if r.is_branch() => r.shorthand().map(str::to_string),
                Ok(_) => None,
                Err(e) if e.code() == git2::ErrorCode::UnbornBranch => None,
                Err(e) => return Err(gerr(e)),
            };
            Ok(branch)
        })
        .await
    }

    pub async fn has_remote(&self, path: &Path) -> Result<bool> {
        let p = path.to_path_buf();
        blocking(move || {
            let repo = open(&p)?;
            let remotes = repo.remotes().map_err(gerr)?;
            Ok(!remotes.is_empty())
        })
        .await
    }

    /// 工作区是否有未提交改动（决策 61，含未跟踪文件）。
    ///
    /// 上限取 [`IS_DIRTY_TIMEOUT_SEC`]（比一般 git 操作短一个数量级）：这个读数的唯一用途
    /// 是决定要不要打一条警告，而它在 `init.execute` 的**第一句**——超时了还让 init 干等，
    /// 就是 2026-09-17 那次「任务一创建就永久卡住」。
    ///
    /// 失败会在调用点区分对待，这里**不**吞错：`do_init` 那条路按 best-effort 处理，
    /// 而 merge 阶段那条（`executor.rs` 的合入前脏检查）必须失败即停——把它变成
    /// `Ok(false)` 等于「查不出来就当干净」，那是往脏工作区里合入。
    pub async fn is_dirty(&self, path: &Path) -> Result<bool> {
        let p = path.to_path_buf();
        blocking_within(IS_DIRTY_TIMEOUT_SEC, move || {
            let repo = open(&p)?;
            let mut opts = git2::StatusOptions::new();
            opts.include_untracked(true);
            let statuses = repo.statuses(Some(&mut opts)).map_err(gerr)?;
            Ok(!statuses.is_empty())
        })
        .await
    }

    /// 脏态的一句话读数（决策 358②：清存量前先记「清掉了什么」）。
    ///
    /// 只数不改：`N 处未提交（其中 M 个未跟踪）`——落进清理命令的台账里，
    /// 让「先清后落」在审计面上看得到代价。空工作区返回 `Ok(None)`。
    pub async fn dirty_summary(&self, path: &Path) -> Result<Option<String>> {
        let p = path.to_path_buf();
        blocking(move || {
            let repo = open(&p)?;
            let mut opts = git2::StatusOptions::new();
            opts.include_untracked(true);
            let statuses = repo.statuses(Some(&mut opts)).map_err(gerr)?;
            if statuses.is_empty() {
                return Ok(None);
            }
            let untracked = statuses
                .iter()
                .filter(|e| e.status() == git2::Status::WT_NEW)
                .count();
            Ok(Some(format!(
                "{} 处未提交（其中 {untracked} 个未跟踪）",
                statuses.len()
            )))
        })
        .await
    }

    /// 未提交改动的**逐条清单**（`XY path`，上限 [`DIRTY_FILES_MAX`] 条），空工作区/非仓库
    /// 返回空表。与 [`Self::dirty_summary`] 同源（同一种 `StatusOptions`），只是要的是
    /// 「哪几个文件」而不是「几处」——票 04 的续接简报要把它交到**新起的一段对话**手里
    /// （事故的直接教训：改过的 `ux-audit.spec.ts` 只活在 worktree 里，差点随重跑被无视）。
    ///
    /// 与 [`Self::is_dirty`] 同一个短上限（[`IS_DIRTY_TIMEOUT_SEC`]）：它换来的也只是
    /// 简报里的一段文本，**不允许有能力挂住关键路径**（决策 209 的教训）。读不出来时
    /// 调用方按「读不到」降级，不因这一条拖垮整轮组装。
    ///
    /// **先排序再截断**：截断取的是「名字靠前的那 [`DIRTY_FILES_MAX`] 条」，不是 git2
    /// 枚举顺序里的前若干条——否则超过上限时被丢掉的恰好可能是那条最该被保护的文件。
    ///
    /// `XY` 两字符与 `git status --short` 同序（X = 索引态，Y = 工作区态）：
    /// `A`（新增）/`M`（已跟踪改动）/`D`（删除）/`R`（改名）/`?`（未跟踪）。忽略文件
    /// （`!`）不在表内——`include_untracked` 不含 ignored，那些行不该进简报。
    pub async fn dirty_files(&self, path: &Path) -> Result<Vec<String>> {
        let p = path.to_path_buf();
        blocking_within(IS_DIRTY_TIMEOUT_SEC, move || {
            let repo = open(&p)?;
            let mut opts = git2::StatusOptions::new();
            opts.include_untracked(true);
            let statuses = repo.statuses(Some(&mut opts)).map_err(gerr)?;
            let mut out: Vec<String> = statuses
                .iter()
                .map(|entry| {
                    let code = entry.status();
                    format!(
                        "{}{} {}",
                        index_state(code),
                        worktree_state(code),
                        entry.path().unwrap_or("(非 UTF-8 路径)")
                    )
                })
                .collect();
            out.sort();
            out.truncate(DIRTY_FILES_MAX);
            Ok(out)
        })
        .await
    }

    /// 把一段 patch 文本应用进工作区（票 13「落补丁」：修复 commit 的 diff 落到任务
    /// worktree，**不提交**——提交与否由闸门之后的 `[repair]` commit 决定）。
    ///
    /// 用 libgit2 的 `apply`（WorkDir 位）而不是 shell `git apply`：与本文件其余原语
    /// 同一姿态。补丁与目标树对不上（任务分支碰过同一批行）时返回 `Err`——调用方
    /// 据此走「等合入」回落，绝不留半份补丁在工作区。
    pub async fn apply_patch(&self, worktree: &Path, patch: &str) -> Result<()> {
        let wt = worktree.to_path_buf();
        let patch = patch.to_string();
        blocking(move || {
            let repo = open(&wt)?;
            let diff = git2::Diff::from_buffer(patch.as_bytes()).map_err(gerr)?;
            repo.apply(&diff, git2::ApplyLocation::WorkDir, None)
                .map_err(|e| Error::Git(format!("补丁落不进任务工作区（多半与任务改动冲突）：{e}")))
        })
        .await
    }

    /// 分支相对基准的**自有提交数**（决策 391：develop 零提交硬检查的读数）。
    ///
    /// libgit2 revwalk：从分支 tip 走、藏掉基准点——分支与基准同点时恰为 0。
    /// 短超时（与 is_dirty 同档）：这是闸门里的一条读数，读不出时调用方按
    /// 「读不到」降级，不允许它挂住 validate_output。
    pub async fn ahead_count(
        &self,
        repo_path: &Path,
        base_ref: &str,
        branch: &str,
    ) -> Result<usize> {
        let p = repo_path.to_path_buf();
        let base = base_ref.to_string();
        let branch = branch.to_string();
        blocking_within(IS_DIRTY_TIMEOUT_SEC, move || {
            let repo = open(&p)?;
            let base_commit = repo
                .revparse_single(&base)
                .map_err(gerr)?
                .peel_to_commit()
                .map_err(gerr)?;
            let branch_commit = repo
                .revparse_single(&branch)
                .map_err(gerr)?
                .peel_to_commit()
                .map_err(gerr)?;
            if branch_commit.id() == base_commit.id() {
                return Ok(0);
            }
            let mut walk = repo.revwalk().map_err(gerr)?;
            walk.push(branch_commit.id()).map_err(gerr)?;
            walk.hide(base_commit.id()).map_err(gerr)?;
            Ok(walk.count())
        })
        .await
    }

    /// 分支相对基准的**改动文件名**（决策 397：develop 申报比对的 committed 面）。
    ///
    /// merge-base 三方口径（`git diff --name-only <base>...<branch>` 的 libgit2 等价）：
    /// 从 merge base 到分支 tip 的树差异——基准自身的漂移不计入。改名取新路径
    /// （申报语义是「我改了哪些文件」，新名才是 review 要读的那个）。
    /// 短超时与 [`Self::ahead_count`] 同档：闸门读数，出错由调用方按「读不到」降级。
    pub async fn changed_files_vs_base(
        &self,
        repo_path: &Path,
        base_ref: &str,
        branch: &str,
    ) -> Result<Vec<String>> {
        let p = repo_path.to_path_buf();
        let base = base_ref.to_string();
        let branch = branch.to_string();
        blocking_within(IS_DIRTY_TIMEOUT_SEC, move || {
            let repo = open(&p)?;
            let base_commit = repo
                .revparse_single(&base)
                .map_err(gerr)?
                .peel_to_commit()
                .map_err(gerr)?;
            let branch_commit = repo
                .revparse_single(&branch)
                .map_err(gerr)?
                .peel_to_commit()
                .map_err(gerr)?;
            let merge_base = repo
                .merge_base(base_commit.id(), branch_commit.id())
                .map_err(gerr)?;
            let base_tree = repo
                .find_commit(merge_base)
                .map_err(gerr)?
                .tree()
                .map_err(gerr)?;
            let branch_tree = branch_commit.tree().map_err(gerr)?;
            let diff = repo
                .diff_tree_to_tree(Some(&base_tree), Some(&branch_tree), None)
                .map_err(gerr)?;
            let mut out: Vec<String> = diff
                .deltas()
                .filter_map(|d| {
                    d.new_file()
                        .path()
                        .or_else(|| d.old_file().path())
                        .map(|p| p.display().to_string())
                })
                .collect();
            out.sort();
            out.dedup();
            Ok(out)
        })
        .await
    }

    /// worktree 未提交改动的**裸路径**（决策 397：申报比对的 dirty 面）。
    ///
    /// 与 [`Self::dirty_files`] 的差别：不带 `XY` 状态前缀；**不截断**——比对漏报时
    /// 截断会把「没报的那条」藏掉，检查语义下宁可贵一点也不许静默丢行；**不含
    /// untracked**（理由见函数体内注释）。
    pub async fn changed_worktree_paths(&self, path: &Path) -> Result<Vec<String>> {
        let p = path.to_path_buf();
        blocking_within(IS_DIRTY_TIMEOUT_SEC, move || {
            let repo = open(&p)?;
            // git2 缺省即**不含** untracked / ignored——这里刻意不开 `include_untracked`：
            // 未跟踪文件不进任务分支 diff、也就不进 review 评审面与 merge 合并面，把它算
            // 漏报只会制造无法通过补申报消除的假红（决策 397 §1）。
            let statuses = repo.statuses(None).map_err(gerr)?;
            let mut out: Vec<String> = statuses
                .iter()
                .filter_map(|entry| entry.path().map(|p| p.to_string()))
                .collect();
            out.sort();
            out.dedup();
            Ok(out)
        })
        .await
    }

    /// 基准 ref（决策 41）：有 **origin** remote 用 `origin/{default_branch}`，否则本地分支。
    ///
    /// 只认 `origin`：仓库只配了别的 remote 名时，`origin/{branch}` 并不存在，
    /// 用它作基准会在解析时才炸。
    pub async fn base_ref(&self, path: &Path, default_branch: &str) -> Result<String> {
        let p = path.to_path_buf();
        let default_branch = default_branch.to_string();
        blocking(move || {
            let repo = open(&p)?;
            let has_origin = repo.find_remote("origin").is_ok();
            Ok(if has_origin {
                format!("origin/{default_branch}")
            } else {
                default_branch
            })
        })
        .await
    }

    /// init.execute：创建 worktree 隔离工作区（§6 Worktree 约定）。
    ///
    /// - 有 remote 先 fetch（--prune），以 `origin/{default_branch}` 为基准；
    /// - 已存在则复用（幂等，§8）；
    /// - unborn HEAD 明确报错。
    pub async fn init_worktree(
        &self,
        project_path: &Path,
        task_id: &str,
        worktree_path: &Path,
        default_branch: &str,
    ) -> Result<String> {
        self.init_worktree_named(
            project_path,
            &branch_name(task_id),
            worktree_path,
            default_branch,
        )
        .await
    }

    /// 同 [`Git::init_worktree`]，但**分支名由调用方给**（票 10 的修复 worktree 用）。
    ///
    /// 两者共用这一份实现：base 的取法（有 `origin` 用 `origin/{default}`）、幂等复用、
    /// `with_worktree_lock` 的串行化、unborn HEAD 的明确报错——这些都不该有第二份。
    pub async fn init_worktree_named(
        &self,
        project_path: &Path,
        branch: &str,
        worktree_path: &Path,
        default_branch: &str,
    ) -> Result<String> {
        let project = project_path.to_path_buf();
        let worktree = worktree_path.to_path_buf();
        let default_branch = default_branch.to_string();
        let branch = branch.to_string();
        blocking(move || {
            let repo = open(&project)?;
            match repo.head() {
                Err(e) if e.code() == git2::ErrorCode::UnbornBranch => {
                    return Err(Error::Git(format!(
                        "仓库 {} 尚无任何提交（unborn HEAD），无法创建 worktree",
                        project.display()
                    )));
                }
                Err(e) => return Err(gerr(e)),
                Ok(_) => {}
            }
            let has_origin = repo.find_remote("origin").is_ok();
            // fetch 失败不阻断（离线场景），但基准优先用 origin；fetch 本体是全仓
            // 唯一一份（决策 415），失败怎么处置由这里（忽略）与修复合入（拒执）各自决定。
            let _ = fetch_origin(&repo);
            let base = if has_origin {
                format!("origin/{default_branch}")
            } else {
                default_branch.clone()
            };

            if worktree.exists() {
                return Ok(base);
            }
            if let Some(parent) = worktree.parent() {
                std::fs::create_dir_all(parent)?;
            }
            // 建 worktree 的窗口必须按仓库串行（见 worktree_creation_lock）：
            // libgit2 对共享的 `.git/worktrees` 先查后建，跨任务并发会撞 EEXIST。
            with_worktree_lock(&project, || {
                let base_commit = repo
                    .revparse_single(&base)
                    .and_then(|o| o.peel_to_commit())
                    .map_err(gerr)?;
                // 分支已存在（重试场景）→ 复用已有分支
                if repo.find_branch(&branch, git2::BranchType::Local).is_err() {
                    repo.branch(&branch, &base_commit, false).map_err(gerr)?;
                }
                let branch_ref = repo
                    .find_reference(&format!("refs/heads/{branch}"))
                    .map_err(gerr)?;
                let mut opts = git2::WorktreeAddOptions::new();
                opts.reference(Some(&branch_ref));
                // worktree 名与 CLI 一致：取路径末段
                let name = worktree
                    .file_name()
                    .and_then(|n| n.to_str())
                    .ok_or_else(|| {
                        Error::Git(format!("非法 worktree 路径：{}", worktree.display()))
                    })?
                    .to_string();
                repo.worktree(&name, &worktree, Some(&opts)).map_err(gerr)?;
                Ok(())
            })?;
            Ok(base)
        })
        .await
    }

    /// worktree 分支名（§6）。
    pub fn branch_for(task_id: &str) -> String {
        branch_name(task_id)
    }

    /// **修复分支**名（决策 210③ / 票 10）：`repair/{id}`。
    ///
    /// 与 `kanban/{task_id}` 分开的理由是**可检索**：`git branch --list` 里要一眼看得出
    /// 哪些分支是修复产物。混用同一个前缀，三个月后没人分得出哪条是谁留的。
    pub fn repair_branch_for(repair_id: &str) -> String {
        format!("repair/{repair_id}")
    }

    /// merge 阶段 A 自愈的提交标记（决策 416 B）：与 `[repair]` 标记同一个目的——三个月后
    /// `git log --grep` 出来时，一眼分出这一笔不是 agent 手写的。
    ///
    /// **为什么不能是 `add -A`**：把未跟踪文件一起提交会把垃圾送进任务分支、进而合入主干，
    /// 而决策 397 恰恰认定「未跟踪不进任务 diff、agent 没有义务为垃圾文件补申报」。
    pub async fn commit_unstaged_tracked_changes(
        &self,
        worktree: &Path,
    ) -> Result<Option<(String, Vec<String>)>> {
        let wt = worktree.to_path_buf();
        blocking(move || {
            let repo = open(&wt)?;
            let mut opts = git2::StatusOptions::new();
            opts.include_untracked(true);
            let statuses = repo.statuses(Some(&mut opts)).map_err(gerr)?;

            // 只挑**已跟踪**的脏条目：`WT_NEW` 是未跟踪（rebase 不看它，见
            // `rebase_ensure_not_dirty` 走的 `git_diff_index_to_workdir` 默认不含未跟踪），
            // `IGNORED` 更不进 diff。其余状态（已暂存 / 已改 / 已删）都要落成提交才算干净。
            let mut paths: Vec<String> = Vec::new();
            for e in statuses.iter() {
                let s = e.status();
                if s.contains(git2::Status::WT_NEW) || s.contains(git2::Status::IGNORED) {
                    continue;
                }
                if let Some(p) = e.path() {
                    let rel = p.to_string();
                    if !paths.contains(&rel) {
                        paths.push(rel);
                    }
                }
            }
            if paths.is_empty() {
                return Ok(None);
            }

            let mut index = repo.index().map_err(gerr)?;
            for rel in &paths {
                // 工作区已删 → `add_path` 会报「文件不存在」，得从索引移除才算删除；
                // 其余一律 `add_path` 把当前内容暂存（对已暂存的条目幂等）。
                if wt.join(rel).exists() {
                    index.add_path(Path::new(rel)).map_err(gerr)?;
                } else {
                    let _ = index.remove_path(Path::new(rel));
                }
            }
            index.write().map_err(gerr)?;
            let tree_id = index.write_tree().map_err(gerr)?;
            let tree = repo.find_tree(tree_id).map_err(gerr)?;
            let parent = repo
                .head()
                .and_then(|h| h.peel_to_commit())
                .map_err(gerr)?;
            let sig = committer()?;
            let message = format!(
                "{MERGE_AUTOSAVE_MARK} 合入前自动落提交：工作区未提交的已跟踪改动\n\n\
                 来源：merge 阶段 A 的自愈（决策 416 B）。内容是本任务此前留在工作区没提交的改动，\n\
                 不是新做的修改；提交只为让 rebase 有一个干净工作区。\n\n\
                 与决策 61 / 132 的「目标分支工作区不自动 stash」分界：stash 把改动藏起来（评审与\n\
                 审批都看不见），commit 留痕（diff、审批面板与 base_commit 都认它）。\n"
            );
            let oid = repo
                .commit(Some("HEAD"), &sig, &sig, &message, &tree, &[&parent])
                .map_err(gerr)?;
            Ok(Some((oid.to_string(), paths)))
        })
        .await
    }

    /// merge 阶段 A：在 worktree 内 rebase 到基准（决策 74 / 96）。
    pub async fn rebase_onto(&self, worktree: &Path, base_ref: &str) -> Result<RebaseOutcome> {
        let wt = worktree.to_path_buf();
        let base = base_ref.to_string();
        blocking(move || {
            let repo = open(&wt)?;
            let sig = committer()?;
            let base_commit = repo
                .revparse_single(&base)
                .and_then(|o| o.peel_to_commit())
                .map_err(gerr)?;
            // 已包含基准时与 `git rebase` 的 no-op 语义一致：不重放提交、保留原 SHA
            let head_id = repo
                .head()
                .map_err(gerr)?
                .target()
                .ok_or_else(|| Error::Git("rebase 起点 HEAD 无指向".into()))?;
            if head_id == base_commit.id()
                || repo
                    .graph_descendant_of(head_id, base_commit.id())
                    .map_err(gerr)?
            {
                return Ok(RebaseOutcome::Clean {
                    head: head_id.to_string(),
                });
            }
            let base_annotated = repo.find_annotated_commit(base_commit.id()).map_err(gerr)?;
            let mut rebase = repo
                .rebase(
                    None,
                    Some(&base_annotated),
                    Some(&base_annotated),
                    Some(&mut git2::RebaseOptions::new()),
                )
                .map_err(gerr)?;
            loop {
                match rebase.next() {
                    Some(Ok(_op)) => {
                        // libgit2 在冲突时不一定从 next() 报错，而是把冲突留在
                        // 索引里、到 commit 时才炸——先查索引再提交
                        let index = repo.index().map_err(gerr)?;
                        if index.has_conflicts() {
                            // 保持 rebase 中间态返回，调用方按决策 74 先 rebase_abort
                            let files = conflicted_paths(&repo)?;
                            return Ok(RebaseOutcome::Conflict { files });
                        }
                        // 以原作者身份提交（committer 为流水线身份）
                        rebase
                            .commit(None, &sig, None)
                            .map_err(|e| Error::Git(format!("rebase 提交失败：{e}")))?;
                    }
                    Some(Err(e)) if e.code() == git2::ErrorCode::Conflict => {
                        // 保持 rebase 中间态返回，调用方按决策 74 先 rebase_abort
                        let files = conflicted_paths(&repo)?;
                        return Ok(RebaseOutcome::Conflict { files });
                    }
                    Some(Err(e)) => return Err(gerr(e)),
                    None => break,
                }
            }
            rebase.finish(Some(&sig)).map_err(gerr)?;
            let head = repo
                .head()
                .map_err(gerr)?
                .target()
                .ok_or_else(|| Error::Git("rebase 完成后 HEAD 无指向".into()))?
                .to_string();
            Ok(RebaseOutcome::Clean { head })
        })
        .await
    }

    /// 打回 develop **之前**由系统执行的清理（决策 74：不再要求 agent 自己 abort）。
    pub async fn rebase_abort(&self, worktree: &Path) -> Result<()> {
        let wt = worktree.to_path_buf();
        blocking(move || {
            let repo = open(&wt)?;
            if let Ok(mut rebase) = repo.open_rebase(None) {
                rebase
                    .abort()
                    .map_err(|e| Error::Git(format!("rebase --abort 失败：{e}")))?;
            }
            Ok(())
        })
        .await
    }

    /// 在 worktree 内 rebase 到 `base_ref`，并尝试自动解决冲突（pipeline-spec §6，票 15）。
    ///
    /// 仅自动解决**无歧义**的冲突（三方合并可机械判定）：某侧与 merge-base 相同
    /// （只有另一侧改动）或两侧内容相同（含同名同内容的新增文件）。其余返回
    /// [`AutoRebaseOutcome::Conflict`]（内部已 abort，worktree 干净）。
    ///
    /// 该逻辑住在 git 层（票 03）：执行器只消费结果，不再内嵌 git 细节。
    pub async fn rebase_onto_with_auto_resolve(
        &self,
        worktree: &Path,
        base_ref: &str,
    ) -> Result<AutoRebaseOutcome> {
        let wt = worktree.to_path_buf();
        let base_ref = base_ref.to_string();
        blocking(move || {
            let repo = open(&wt)?;
            let sig = committer()?;
            let base_commit = repo
                .revparse_single(&base_ref)
                .and_then(|o| o.peel_to_commit())
                .map_err(gerr)?;
            let head_id = repo
                .head()
                .map_err(gerr)?
                .target()
                .ok_or_else(|| Error::Git("rebase 起点 HEAD 无指向".into()))?;
            // 已包含基准：与 `git rebase` no-op 语义一致
            if head_id == base_commit.id()
                || repo
                    .graph_descendant_of(head_id, base_commit.id())
                    .map_err(gerr)?
            {
                return Ok(AutoRebaseOutcome::Clean {
                    head: head_id.to_string(),
                });
            }
            let base_annotated = repo.find_annotated_commit(base_commit.id()).map_err(gerr)?;
            let mut rebase = repo
                .rebase(
                    None,
                    Some(&base_annotated),
                    Some(&base_annotated),
                    Some(&mut git2::RebaseOptions::new()),
                )
                .map_err(gerr)?;
            let mut resolved: Vec<String> = Vec::new();

            // 处理当前索引冲突：可机械解决则写入并返回 Ok(())；否则返回 Err(files) 触发 abort。
            let handle_conflicts = |repo: &git2::Repository,
                                    index: &mut git2::Index,
                                    resolved: &mut Vec<String>|
             -> Result<std::result::Result<(), Vec<String>>> {
                let conflicts: Vec<git2::IndexConflict> = index
                    .conflicts()
                    .map_err(gerr)?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(gerr)?;
                let mut hard: Vec<String> = Vec::new();
                for c in conflicts {
                    let path = c
                        .our
                        .as_ref()
                        .or(c.their.as_ref())
                        .or(c.ancestor.as_ref())
                        .map(|e| e.path.clone())
                        .ok_or_else(|| Error::Git("rebase 冲突条目缺少路径".into()))?;
                    let ancestor = c.ancestor.as_ref().map(|e| e.id);
                    let ours = c.our.as_ref().map(|e| e.id);
                    let theirs = c.their.as_ref().map(|e| e.id);
                    let chosen: Option<git2::Oid> = if ours == theirs {
                        ours
                    } else if ancestor == ours {
                        theirs
                    } else if ancestor == theirs {
                        ours
                    } else {
                        hard.push(String::from_utf8_lossy(&path).to_string());
                        continue;
                    };
                    let rel = std::str::from_utf8(&path)
                        .map_err(|e| Error::Git(format!("冲突路径非 UTF-8：{e}")))?
                        .to_string();
                    match chosen {
                        Some(id) => {
                            let blob = repo.find_blob(id).map_err(gerr)?;
                            std::fs::write(wt.join(&rel), blob.content())?;
                            index.add_path(Path::new(&rel)).map_err(gerr)?;
                        }
                        None => {
                            let abs = wt.join(&rel);
                            if abs.exists() {
                                std::fs::remove_file(&abs)?;
                            }
                            index.remove_path(Path::new(&rel)).map_err(gerr)?;
                        }
                    }
                    resolved.push(rel);
                }
                if hard.is_empty() {
                    Ok(Ok(()))
                } else {
                    Ok(Err(hard))
                }
            };

            macro_rules! step_or_abort {
                ($commit:expr) => {{
                    let mut index = repo.index().map_err(gerr)?;
                    if index.has_conflicts() {
                        match handle_conflicts(&repo, &mut index, &mut resolved)? {
                            Ok(()) => {
                                index.write().map_err(gerr)?;
                            }
                            Err(files) => {
                                let _ = rebase.abort();
                                return Ok(AutoRebaseOutcome::Conflict { files });
                            }
                        }
                    }
                    if $commit {
                        match rebase.commit(None, &sig, None) {
                            Ok(_) => {}
                            // 补丁已包含于基准（add/add 同内容等）→ libgit2 报 Applied，跳过即可
                            Err(e) if e.code() == git2::ErrorCode::Applied => {}
                            Err(e) => return Err(Error::Git(format!("rebase 提交失败：{e}"))),
                        }
                    }
                }};
            }

            loop {
                match rebase.next() {
                    Some(Ok(_op)) => step_or_abort!(true),
                    Some(Err(e)) if e.code() == git2::ErrorCode::Conflict => step_or_abort!(true),
                    Some(Err(e)) => return Err(gerr(e)),
                    None => break,
                }
            }
            rebase
                .finish(Some(&sig))
                .map_err(|e| Error::Git(format!("rebase 收尾失败：{e}")))?;
            let head = repo
                .head()
                .map_err(gerr)?
                .target()
                .ok_or_else(|| Error::Git("rebase 完成后 HEAD 无指向".into()))?
                .to_string();
            if resolved.is_empty() {
                Ok(AutoRebaseOutcome::Clean { head })
            } else {
                Ok(AutoRebaseOutcome::AutoResolved {
                    head,
                    files: resolved,
                })
            }
        })
        .await
    }

    /// 未合并（冲突）文件列表。
    pub async fn conflict_files(&self, repo_path: &Path) -> Result<Vec<String>> {
        let p = repo_path.to_path_buf();
        blocking(move || {
            let repo = open(&p)?;
            conflicted_paths(&repo)
        })
        .await
    }

    /// merge 阶段 B：把任务分支合入 default_branch 并写回（决策 73 / 97）。
    ///
    /// git2 实现（2026-09-12）：**内存合并**——`merge_commits` 生成三方合并索引，
    /// 写树后以双亲 commit 直接写回 `refs/heads/{default_branch}`，不建临时
    /// detached worktree。ff 优先，不可 ff 则生成 merge commit，**绝不 force**。
    pub async fn merge_into_default_branch(
        &self,
        project_path: &Path,
        default_branch: &str,
        source_branch: &str,
    ) -> Result<MergeOutcome> {
        let project = project_path.to_path_buf();
        let default_branch = default_branch.to_string();
        let source_branch = source_branch.to_string();
        blocking(move || {
            let repo = open(&project)?;
            let sig = committer()?;
            let default_ref = format!("refs/heads/{default_branch}");
            let source_ref = format!("refs/heads/{source_branch}");
            let default_commit = repo
                .find_reference(&default_ref)
                .and_then(|r| r.peel_to_commit())
                .map_err(gerr)?;
            let source_commit = repo
                .find_reference(&source_ref)
                .and_then(|r| r.peel_to_commit())
                .map_err(gerr)?;
            let default_id = default_commit.id();
            let source_id = source_commit.id();

            // ff：default 是 source 的祖先 → 直接移动引用
            if repo
                .graph_descendant_of(source_id, default_id)
                .map_err(gerr)?
            {
                repo.reference(&default_ref, source_id, true, "AgentPipeline merge (ff)")
                    .map_err(gerr)?;
                sync_checked_out_worktree(&repo, &default_ref)?;
                return Ok(MergeOutcome {
                    fast_forward: true,
                    commit: source_id.to_string(),
                });
            }
            // source 已包含于主干 → Already up to date（与 CLI 语义一致，不建 commit）
            if repo
                .graph_descendant_of(default_id, source_id)
                .map_err(gerr)?
            {
                return Ok(MergeOutcome {
                    fast_forward: false,
                    commit: default_id.to_string(),
                });
            }

            // no-ff：内存三方合并 → 写树 → 双亲 commit 写回引用
            let mut index = repo
                .merge_commits(&default_commit, &source_commit, None)
                .map_err(|e| Error::Git(format!("合入失败：{e}")))?;
            if index.has_conflicts() {
                return Err(Error::Git(format!(
                    "合入失败：{source_branch} 与 {default_branch} 存在冲突，无法自动合入"
                )));
            }
            let tree_oid = index.write_tree_to(&repo).map_err(gerr)?;
            let tree = repo.find_tree(tree_oid).map_err(gerr)?;
            let msg = format!("Merge {source_branch}");
            let commit_id = repo
                .commit(
                    Some(&default_ref),
                    &sig,
                    &sig,
                    &msg,
                    &tree,
                    &[&default_commit, &source_commit],
                )
                .map_err(|e| Error::Git(format!("合入失败：{e}")))?;
            sync_checked_out_worktree(&repo, &default_ref)?;
            Ok(MergeOutcome {
                fast_forward: false,
                commit: commit_id.to_string(),
            })
        })
        .await
    }

    /// merge 阶段 B 的可选收尾：把默认分支 push 到远端（决策 393）。
    ///
    /// remote 清单读 git2（同仓同一读法）；**推送本体走系统 git CLI**——这是决策 12
    /// （统一 git2）的唯一例外：push 要复用用户仓自己的凭据链（credential.helper /
    /// ssh-agent / ssh config），libgit2 不代跑助手，自拼凭据回调两头不讨好。
    /// 仓没有配置任何 remote → 返回 [`PushOutcome::NoRemote`]，调用方照常收尾，
    /// **不算失败**（纯本地仓是常态，决策 393）。
    pub async fn push_default_branch(
        &self,
        project_path: &Path,
        default_branch: &str,
    ) -> Result<PushOutcome> {
        let project = project_path.to_path_buf();
        let remote = blocking(move || {
            let repo = open(&project)?;
            let remotes = repo.remotes().map_err(gerr)?;
            if remotes.is_empty() {
                return Ok(None);
            }
            // 优先 origin；没有就用配置里的第一个。
            let origin = (0..remotes.len())
                .find_map(|i| remotes.get(i))
                .filter(|name| *name == "origin");
            let name = match origin {
                Some(n) => n.to_string(),
                None => remotes
                    .get(0)
                    .expect("remotes 非空时 get(0) 必有值")
                    .to_string(),
            };
            Ok(Some(name))
        })
        .await?;
        let Some(remote) = remote else {
            return Ok(PushOutcome::NoRemote);
        };
        let mut cmd = tokio::process::Command::new("git");
        cmd.current_dir(project_path)
            .args(["push", &remote, default_branch])
            // 凭据问询改为直接失败：无人值守的服务里对着 TTY 等输入 = 挂死。
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        let child = cmd
            .spawn()
            .map_err(|e| Error::Git(format!("git push 启动失败：{e}")))?;
        let output = match tokio::time::timeout(
            std::time::Duration::from_secs(GIT_PUSH_TIMEOUT_SEC),
            child.wait_with_output(),
        )
        .await
        {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => return Err(Error::Git(format!("git push 执行失败：{e}"))),
            Err(_) => {
                return Err(Error::Git(format!(
                    "git push 超时（{GIT_PUSH_TIMEOUT_SEC}s）：远端 {remote} 无响应"
                )))
            }
        };
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Git(format!(
                "git push 失败（{remote} {}）：{}",
                default_branch,
                stderr.trim()
            )));
        }
        Ok(PushOutcome::Pushed { remote })
    }

    pub async fn update_ref(&self, repo_path: &Path, refname: &str, commit: &str) -> Result<()> {
        let p = repo_path.to_path_buf();
        let refname = refname.to_string();
        let commit = commit.to_string();
        blocking(move || {
            let repo = open(&p)?;
            let oid = repo.revparse_single(&commit).map_err(gerr)?.id();
            repo.reference(&refname, oid, true, "AgentPipeline update-ref")
                .map_err(gerr)?;
            Ok(())
        })
        .await
    }

    /// 合入前的工作区干净检查（决策 61）。
    pub async fn ensure_clean(&self, path: &Path) -> Result<()> {
        if self.is_dirty(path).await? {
            return Err(Error::Git(format!("工作区不干净：{}", path.display())));
        }
        Ok(())
    }

    /// 重试重置（决策 125）：reset --hard {base} + clean -fdx 等价操作。
    pub async fn reset_hard_clean(&self, worktree: &Path, base_ref: &str) -> Result<()> {
        let wt = worktree.to_path_buf();
        let base = base_ref.to_string();
        blocking(move || {
            let repo = open(&wt)?;
            let commit = repo
                .revparse_single(&base)
                .and_then(|o| o.peel_to_commit())
                .map_err(gerr)?;
            repo.reset(commit.as_object(), git2::ResetType::Hard, None)
                .map_err(|e| Error::Git(format!("reset --hard 失败：{e}")))?;
            clean_untracked(&repo, &wt)?;
            Ok(())
        })
        .await
    }

    /// 清理 worktree（决策 3：合入后立即删除；取消 / 归档同样清理）。
    ///
    /// `force = false` 时与 CLI `worktree remove` 的差异：脏工作区不报错，
    /// 只在目录为空时剪掉登记——当前所有调用方都传 `force = true`。
    pub async fn remove_worktree(
        &self,
        repo_path: &Path,
        worktree: &Path,
        force: bool,
    ) -> Result<()> {
        let repo_dir = repo_path.to_path_buf();
        let wt = worktree.to_path_buf();
        let _ = force;
        blocking(move || {
            let repo = open(&repo_dir)?;
            // 兜底删除目录本身（幂等）——仅当它确实是本仓库登记的 worktree
            //（worktree 的 .git 是文件而非目录），避免误删恰好占用该路径的无关目录
            if wt.exists() && wt.join(".git").is_file() {
                let _ = std::fs::remove_dir_all(&wt);
            }
            // 剪掉登记（目录已删时普通 prune 即可生效）
            let name = wt
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| Error::Git(format!("非法 worktree 路径：{}", wt.display())))?;
            if let Ok(registered) = repo.find_worktree(name) {
                let _ = registered.prune(None);
            }
            Ok(())
        })
        .await
    }

    pub async fn delete_branch(&self, repo_path: &Path, branch: &str) -> Result<()> {
        let p = repo_path.to_path_buf();
        let branch = branch.to_string();
        blocking(move || {
            let repo = open(&p)?;
            // 幂等：分支不存在不报错
            let _ = repo
                .find_branch(&branch, git2::BranchType::Local)
                .and_then(|mut b| b.delete());
            Ok(())
        })
        .await
    }

    /// diff（merge proposal / review-diff 用）。
    pub async fn diff_range(&self, repo_path: &Path, range: &str) -> Result<String> {
        let p = repo_path.to_path_buf();
        let (from, to) = split_range(range);
        blocking(move || {
            let repo = open(&p)?;
            let from_tree = tree_of(&repo, &from)?;
            let to_tree = tree_of(&repo, &to)?;
            let diff = repo
                .diff_tree_to_tree(from_tree.as_ref(), to_tree.as_ref(), None)
                .map_err(gerr)?;
            let mut out = Vec::new();
            diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
                // git2 的行内容不含 +/-/空格 前缀，手动补上
                match line.origin() {
                    '+' | '-' | ' ' => out.push(line.origin() as u8),
                    _ => {}
                }
                out.extend_from_slice(line.content());
                true
            })
            .map_err(gerr)?;
            Ok(String::from_utf8_lossy(&out).to_string())
        })
        .await
    }

    /// diff 统计。
    pub async fn diff_stat(&self, repo_path: &Path, range: &str) -> Result<String> {
        let p = repo_path.to_path_buf();
        let (from, to) = split_range(range);
        blocking(move || {
            let repo = open(&p)?;
            let from_tree = tree_of(&repo, &from)?;
            let to_tree = tree_of(&repo, &to)?;
            let diff = repo
                .diff_tree_to_tree(from_tree.as_ref(), to_tree.as_ref(), None)
                .map_err(gerr)?;
            let stats = diff.stats().map_err(gerr)?;
            let buf = stats
                .to_buf(git2::DiffStatsFormat::FULL, 80)
                .map_err(gerr)?;
            Ok(String::from_utf8_lossy(&buf).to_string())
        })
        .await
    }

    /// 当前分支是否包含某个 commit。
    pub async fn contains(&self, repo_path: &Path, branch: &str, commit: &str) -> Result<bool> {
        let p = repo_path.to_path_buf();
        let branch = branch.to_string();
        let commit = commit.to_string();
        blocking(move || {
            let repo = open(&p)?;
            let branch_tip = resolve_commit(&repo, &branch)?;
            let target = resolve_commit(&repo, &commit)?;
            if branch_tip == target {
                return Ok(true);
            }
            repo.graph_descendant_of(branch_tip, target).map_err(gerr)
        })
        .await
    }

    // ─────────────────── 项目静态探测（决策 78 / 139，纯代码）───────────────────

    /// 语言探测（按标记文件，结果稳定可单测）。
    pub fn detect_language(path: &Path) -> Option<String> {
        let has = |f: &str| path.join(f).exists();
        if has("Cargo.toml") {
            Some("rust".into())
        } else if has("package.json") {
            Some("node".into())
        } else if has("pyproject.toml") || has("requirements.txt") || has("setup.py") {
            Some("python".into())
        } else {
            None
        }
    }

    /// 测试框架探测。
    pub fn detect_test_framework(_path: &Path, language: Option<&str>) -> Option<String> {
        match language {
            Some("rust") => Some("cargo".into()),
            Some("node") => Some("npm".into()),
            Some("python") => Some("pytest".into()),
            _ => None,
        }
    }

    /// lint 工具探测（决策 139：探测到的命令作为候选，由用户确认后预填）。
    pub fn detect_lint_command(path: &Path, language: Option<&str>) -> Option<String> {
        match language {
            Some("rust") => Some("cargo clippy --all-targets -- -D warnings".into()),
            Some("node") => {
                let pkg = std::fs::read_to_string(path.join("package.json")).unwrap_or_default();
                pkg.contains("eslint").then(|| "npx eslint .".to_string())
            }
            Some("python") => {
                if std::fs::metadata(path.join("ruff.toml")).is_ok()
                    || std::fs::read_to_string(path.join("pyproject.toml"))
                        .map(|s| s.contains("ruff"))
                        .unwrap_or(false)
                {
                    Some("ruff check .".into())
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// `.gitignore` 是否存在（决策 78 探测清单第六项）。
    pub fn has_gitignore(path: &Path) -> bool {
        path.join(".gitignore").exists()
    }

    /// `AGENTS.md` 路径（决策 78）。
    pub fn agents_md_path(path: &Path) -> Option<PathBuf> {
        let candidate = path.join("AGENTS.md");
        candidate.exists().then_some(candidate)
    }
}

fn resolve_commit(repo: &git2::Repository, refname: &str) -> Result<git2::Oid> {
    repo.revparse_single(refname)
        .and_then(|o| o.peel_to_commit())
        .map(|c| c.id())
        .map_err(gerr)
}

fn tree_of<'repo>(
    repo: &'repo git2::Repository,
    refname: &str,
) -> Result<Option<git2::Tree<'repo>>> {
    let commit = repo
        .revparse_single(refname)
        .and_then(|o| o.peel_to_commit())
        .map_err(gerr)?;
    Ok(Some(commit.tree().map_err(gerr)?))
}

/// 分支名约定（§6 Worktree 约定）。
pub fn branch_name(task_id: &str) -> String {
    format!("kanban/{task_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 阻塞的 git 工作必须有上限（决策 209）。
    ///
    /// 牙齿：把 `blocking_within` 的 `tokio::time::timeout` 摘掉，这个用例会挂死到
    /// 5 秒后返回 `Ok`，`matches!` 随即变红。
    #[tokio::test]
    async fn blocking_within_times_out_instead_of_hanging() {
        let r = blocking_within(1, || {
            std::thread::sleep(std::time::Duration::from_secs(2));
            Ok(())
        })
        .await;
        match r {
            Err(Error::Git(m)) => assert!(m.contains("超时"), "超时文案不对：{m}"),
            other => panic!("应当超时，实际：{other:?}"),
        }
    }

    /// 等锁也有上限，且报文说清是「锁被占着」而不是「git 慢」（2026-09-18 实测）。
    ///
    /// 牙齿：把 `with_worktree_lock_within` 的 `try_lock` 换回 `lock()`，这个用例会挂死在
    /// 等锁上——而「挂死在等锁上、报出来的却是 git 超时」正是当时那条误导性报文的来源。
    #[test]
    fn worktree_lock_wait_is_bounded_and_says_so() {
        let repo = Path::new("/tmp/agentpipeline-lock-probe");
        // 同一条线程自己占住它：`std::sync::Mutex` 不可重入，第二次拿就是 WouldBlock。
        let held = worktree_creation_lock(repo);
        let _guard = held.lock().unwrap_or_else(|e| e.into_inner());
        let r: Result<()> = with_worktree_lock_within(&SystemClock, 0, repo, || Ok(()));
        match r {
            Err(Error::Git(m)) => assert!(m.contains("互斥锁被占用"), "报文不对：{m}"),
            other => panic!("应当报「锁被占用」，实际：{other:?}"),
        }
    }

    /// 没有超时的活儿照常走通，且错误仍然是错误（不吞错）。
    #[tokio::test]
    async fn blocking_within_passes_through_and_propagates_errors() {
        assert_eq!(blocking_within(5, || Ok(7)).await.unwrap(), 7);
        let r: Result<()> = blocking_within(5, || Err(Error::Git("底层报错".into()))).await;
        match r {
            Err(Error::Git(m)) => assert_eq!(m, "底层报错"),
            other => panic!("应当原样透出底层错误，实际：{other:?}"),
        }
    }

    /// `is_dirty` 的上限必须显著短于一般 git 操作——它只换来一条警告（决策 209）。
    ///
    /// 两条断言搬进 `const` 块：两边都是常量，本就不该在运行期比——这正是 clippy 1.98 的
    /// `assertions_on_constants` 指出的。进 const 块之后，违反这条大小关系会**编译失败**，
    /// 比跑测试更早拦下。代价是消息里不能插值（const 上下文只收字面量，E0015），
    /// 故消息点常量名而不点数值——数值就在上面那两行常量定义里。
    #[test]
    fn is_dirty_timeout_is_shorter_than_the_general_bound() {
        const {
            assert!(
                IS_DIRTY_TIMEOUT_SEC < GIT_OP_TIMEOUT_SEC,
                "脏检查上限（IS_DIRTY_TIMEOUT_SEC）必须短于通用上限（GIT_OP_TIMEOUT_SEC）"
            );
            assert!(
                GIT_OP_TIMEOUT_SEC < 300,
                "通用上限（GIT_OP_TIMEOUT_SEC）必须压在 node_idle_timeout_sec 默认值 300 之下，\
                 否则挂死会退化成杀不掉的节点超时"
            );
        }
    }

    #[test]
    fn branch_name_convention() {
        assert_eq!(branch_name("01H"), "kanban/01H");
        assert_eq!(Git::branch_for("t1"), "kanban/t1");
    }

    /// 决策 416 C 的判据要**窄**：命中的是「人修一下、修完重试就会过」的那几句原话，
    /// 普通命令失败（哪怕同样来自 git、同样带 `; class=` 尾巴）一个都不许命中——
    /// 宁可漏判落回 retry_exhausted，也不要把会自己恢复的失败说成「你得先去修环境」。
    #[test]
    fn environment_precondition_fingerprint_is_narrow() {
        // 命中：五条指纹各自的原话（含 libgit2 报文后缀的形态）
        for hit in [
            "rebase failed: unstaged changes exist in workdir; class=Reference (4)",
            "cannot commit: uncommitted changes exist in index",
            "rebase 起点 HEAD 无指向",
            "尚无任何提交",
            "could not find repository from '/tmp/missing/.git'",
        ] {
            assert!(is_environment_precondition(hit), "应命中：{hit}");
        }
        // 不命中：普通失败——exit code、冲突、鉴权、超时、空串
        for miss in [
            "",
            "command `cargo test` exited with status 101",
            "rebase 提交失败：conflict in src/lib.rs",
            "authentication required for github.com",
            "operation timed out after 30000ms",
        ] {
            assert!(!is_environment_precondition(miss), "不应命中：{miss}");
        }
    }

    /// 决策 416 B 的原语：只把**已跟踪**的残留落成 `[autosave]` 提交，未跟踪的原样留着
    /// （`add -A` 会把垃圾送进任务分支，决策 397 恰恰认定未跟踪不进任务 diff）。
    /// 干净工作区 → `None`（不空转造提交）。
    #[tokio::test]
    async fn autosave_commits_tracked_residue_but_leaves_untracked_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(tmp.path()).unwrap();
        std::fs::write(tmp.path().join("src.txt"), "v1").unwrap();
        {
            let mut index = repo.index().unwrap();
            index.add_path(Path::new("src.txt")).unwrap();
            index.write().unwrap();
            let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
            let sig = git2::Signature::now("t", "t@example.com").unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
                .unwrap();
        }
        drop(repo);

        // 干净 → None
        assert!(
            Git.commit_unstaged_tracked_changes(tmp.path())
                .await
                .unwrap()
                .is_none(),
            "干净工作区不该造提交"
        );

        // 已跟踪改动 + 未跟踪文件：只提交前者
        std::fs::write(tmp.path().join("src.txt"), "v2").unwrap();
        std::fs::write(tmp.path().join("junk.txt"), "junk").unwrap();
        let (commit, files) = Git
            .commit_unstaged_tracked_changes(tmp.path())
            .await
            .unwrap()
            .expect("已跟踪残留应落成提交");
        assert_eq!(files, vec!["src.txt".to_string()]);
        assert_eq!(commit.len(), 40, "返回的应是提交 SHA");

        // 标记进 message（`git log --grep '[autosave]'` 可认账）
        let reopened = git2::Repository::open(tmp.path()).unwrap();
        let head_id = reopened.head().unwrap().peel_to_commit().unwrap().id();
        let log = reopened.find_commit(head_id).unwrap();
        assert!(
            log.message().unwrap().contains(MERGE_AUTOSAVE_MARK),
            "自愈提交必须带 [autosave] 标记：{}",
            log.message().unwrap()
        );
        // 未跟踪文件没被卷进去：HEAD 树里没有它，工作区里它还在
        let head_tree = log.tree().unwrap();
        assert!(
            head_tree.get_name("junk.txt").is_none(),
            "未跟踪文件不得进自愈提交"
        );
        assert!(tmp.path().join("junk.txt").exists());
        // 已跟踪改动已提交 → 工作区对已跟踪面干净（再次调用不重复造提交）
        assert!(
            Git.commit_unstaged_tracked_changes(tmp.path())
                .await
                .unwrap()
                .is_none(),
            "自愈后再次调用应为 None"
        );
    }

    /// 票 04：未提交改动要**点得出文件名**（简报据此提醒「改好了，别重做」），且非仓库
    /// 时降级为 `Err`（调用方按「读不到」显示），不是一个 panic。
    #[tokio::test]
    async fn dirty_files_names_each_change_and_degrades_on_a_non_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(tmp.path()).unwrap();
        std::fs::write(tmp.path().join("tracked.txt"), "v1").unwrap();
        {
            let mut index = repo.index().unwrap();
            index.add_path(Path::new("tracked.txt")).unwrap();
            index.write().unwrap();
            let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
            let sig = git2::Signature::now("t", "t@example.com").unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
                .unwrap();
        }
        // 一处已跟踪文件的改动 + 一处未跟踪文件
        std::fs::write(tmp.path().join("tracked.txt"), "v2").unwrap();
        std::fs::write(tmp.path().join("new.txt"), "n").unwrap();

        let files = Git.dirty_files(tmp.path()).await.unwrap();
        assert!(
            files
                .iter()
                .any(|f| f.starts_with(" M") && f.ends_with("tracked.txt")),
            "已跟踪改动按 ` M` 记：{files:?}"
        );
        assert!(
            files
                .iter()
                .any(|f| f.starts_with("??") && f.ends_with("new.txt")),
            "未跟踪按 `??` 记：{files:?}"
        );

        // 非仓库 → Err（上层降级成一句「读不到」，不拖垮组装）
        let outside = tempfile::tempdir().unwrap();
        assert!(Git.dirty_files(outside.path()).await.is_err());
    }

    #[test]
    fn language_detection_by_marker_files() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(Git::detect_language(tmp.path()), None);

        std::fs::write(tmp.path().join("Cargo.toml"), "[package]").unwrap();
        assert_eq!(Git::detect_language(tmp.path()).as_deref(), Some("rust"));
        assert_eq!(
            Git::detect_test_framework(tmp.path(), Some("rust")).as_deref(),
            Some("cargo")
        );
        assert!(Git::detect_lint_command(tmp.path(), Some("rust"))
            .unwrap()
            .contains("clippy"));
    }

    #[test]
    fn test_framework_detection_per_language() {
        assert_eq!(
            Git::detect_test_framework(Path::new("/x"), Some("python")).as_deref(),
            Some("pytest")
        );
        assert_eq!(
            Git::detect_test_framework(Path::new("/x"), Some("node")).as_deref(),
            Some("npm")
        );
        assert_eq!(Git::detect_test_framework(Path::new("/x"), None), None);
    }

    #[test]
    fn lint_detection_respects_project_config() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("package.json"), r#"{"devDependencies":{}}"#).unwrap();
        assert_eq!(Git::detect_lint_command(tmp.path(), Some("node")), None);
        std::fs::write(
            tmp.path().join("package.json"),
            r#"{"devDependencies":{"eslint":"^9"}}"#,
        )
        .unwrap();
        assert_eq!(
            Git::detect_lint_command(tmp.path(), Some("node")).as_deref(),
            Some("npx eslint .")
        );
    }

    #[test]
    fn gitignore_and_agents_md_probing() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!Git::has_gitignore(tmp.path()));
        assert!(Git::agents_md_path(tmp.path()).is_none());
        std::fs::write(tmp.path().join(".gitignore"), "/target").unwrap();
        std::fs::write(tmp.path().join("AGENTS.md"), "# 指令").unwrap();
        assert!(Git::has_gitignore(tmp.path()));
        assert!(Git::agents_md_path(tmp.path())
            .unwrap()
            .ends_with("AGENTS.md"));
    }

    #[test]
    fn range_parsing_covers_two_and_three_dot_forms() {
        assert_eq!(
            split_range("main..kanban/t1"),
            ("main".into(), "kanban/t1".into())
        );
        assert_eq!(
            split_range("main...kanban/t1"),
            ("main".into(), "kanban/t1".into())
        );
        assert_eq!(split_range("..main"), ("HEAD".into(), "main".into()));
        assert_eq!(split_range("main.."), ("main".into(), "HEAD".into()));
    }
}
