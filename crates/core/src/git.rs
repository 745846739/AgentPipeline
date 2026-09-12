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

/// 合入结果（决策 73 / 97）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeOutcome {
    /// 是否为 fast-forward。
    pub fast_forward: bool,
    /// 合入后的 commit SHA（ff 时为被合入分支的 tip）。
    pub commit: String,
}

fn gerr(e: git2::Error) -> Error {
    Error::Git(e.to_string())
}

/// 在阻塞线程池里执行一段同步 git2 逻辑（决策 12）。
async fn blocking<T, F>(f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(inner) => inner,
        Err(e) => Err(Error::Git(format!("spawn_blocking 任务失败：{e}"))),
    }
}

fn open(path: &Path) -> Result<git2::Repository> {
    git2::Repository::open(path).map_err(gerr)
}

/// 流水线提交者身份（与原 CLI `-c user.name/email` 一致）。
fn committer() -> Result<git2::Signature<'static>> {
    git2::Signature::now("AgentPipeline", "agentpipeline@localhost").map_err(gerr)
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
    pub async fn is_dirty(&self, path: &Path) -> Result<bool> {
        let p = path.to_path_buf();
        blocking(move || {
            let repo = open(&p)?;
            let mut opts = git2::StatusOptions::new();
            opts.include_untracked(true);
            let statuses = repo.statuses(Some(&mut opts)).map_err(gerr)?;
            Ok(!statuses.is_empty())
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
        let project = project_path.to_path_buf();
        let worktree = worktree_path.to_path_buf();
        let default_branch = default_branch.to_string();
        let task_id = task_id.to_string();
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
            if has_origin {
                // fetch 失败不阻断（离线场景），但基准优先用 origin
                if let Ok(mut remote) = repo.find_remote("origin") {
                    let mut opts = git2::FetchOptions::new();
                    opts.prune(git2::FetchPrune::On);
                    let _ = remote.fetch::<&str>(&[], Some(&mut opts), None);
                }
            }
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
            let branch = branch_name(&task_id);
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
                .ok_or_else(|| Error::Git(format!("非法 worktree 路径：{}", worktree.display())))?;
            repo.worktree(name, &worktree, Some(&opts)).map_err(gerr)?;
            Ok(base)
        })
        .await
    }

    /// worktree 分支名（§6）。
    pub fn branch_for(task_id: &str) -> String {
        branch_name(task_id)
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
            Ok(MergeOutcome {
                fast_forward: false,
                commit: commit_id.to_string(),
            })
        })
        .await
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

    #[test]
    fn branch_name_convention() {
        assert_eq!(branch_name("01H"), "kanban/01H");
        assert_eq!(Git::branch_for("t1"), "kanban/t1");
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
