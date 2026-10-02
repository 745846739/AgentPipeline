//! 共享构建缓存的**确定性回收**（票 runner-offload/03 / B5 收口）。
//!
//! 背景：任务 worktree 的 cargo 构建经 `CARGO_TARGET_DIR` 收敛到
//! `{home}/shared-target`（[`crate::home::Home::shared_target_path`]），worktree
//! 自身不再各养一份 `target/`。缓存的价值在**复用**——registry 依赖的编译产物
//! 跨任务免重编（106 上全量 ~20 分钟），所以回收不能「任务结束一锅端」：
//!
//! - **增量树**（`{debug,release}/incremental`）按 worktree 路径区分指纹，跨任务
//!   复用率近零、却往往是大头（实测 1.4GB 里 491MB）——任务终态时**必删**，重建便宜；
//! - **总量上限**：剩余部分（deps / build）跨任务复用率高，平时保留；总量越过
//!   [`SHARED_TARGET_CAP_BYTES`] 时整体清空——这是机制保证的兜底，不是「恰好被清」
//!   （01M3X472FJF8NW9082K6BFZPKC 巡检的教训：那次 1.4GB 是被碰巧清掉的，无路径可循）。
//!
//! 调用点：任务进入终态（done / 归档 / 取消）与项目删除——即「这个任务的构建
//! 活动确定结束了」的所有时刻。删除目录是逐项尽力而为：单个条目删除失败不阻断
//! 任务收口，只留 WARN（缓存回收失败不该让已完成的任务报错）。

use crate::home::Home;
use serde::Serialize;
#[cfg(unix)]
use std::os::fd::AsRawFd;

/// 共享缓存的总量上限：10 GiB。40G 盘上给构建缓存留这个量级有历史依据——
/// 单任务全量 target 实测 1.4GB，十个并发深度也到不了这个帽。
pub const SHARED_TARGET_CAP_BYTES: u64 = 10 * 1024 * 1024 * 1024;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct PruneStats {
    /// 本次删除的增量树字节数（0 = 没有增量树可删）。
    pub incremental_bytes: u64,
    /// 是否触发了总量兜底清空（true 时 deps/build 也被清，下次构建全量重编）。
    pub full_reset: bool,
    /// 回收后共享目录剩余字节数（目录不存在时为 0）。
    pub remaining_bytes: u64,
}

/// 任务终态时的构建缓存回收：删增量树 + 总量兜底。**尽力而为**：任何删除失败
/// 只记 WARN，不向上报错——调用点都在任务收口的关键路径上。
pub async fn prune_build_cache(home: &Home) -> PruneStats {
    prune_with_cap(home, SHARED_TARGET_CAP_BYTES).await
}

/// 共享目录的**占锁哨兵**：回收前先试拿非阻塞排它锁。拿不到 = 还有任务在构建
/// （票据的硬要求：清理不得影响在跑任务），本次跳过、留给下一个终态事件。
/// 锁文件常驻不删——删了会出现「持锁 inode 与后来者打开的 inode 不是同一个」的竞态。
#[cfg(unix)]
fn try_lock_guard(root: &std::path::Path) -> Option<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join(".prune-lock"))
        .ok()?;
    // flock(LOCK_EX|LOCK_NB):拿不到立刻走人——这是哨兵,不是闸门。
    // std 的 File::try_lock 要 1.89,仓库 MSRV 声明 1.80,故走 libc
    // (io_budget 的 statvfs 同款先例)。
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        Some(file)
    } else {
        None
    }
}

/// 非 unix 没有这套哨兵:宁可不收,也不冒「删掉在跑构建」的险。
#[cfg(not(unix))]
fn try_lock_guard(_root: &std::path::Path) -> Option<std::fs::File> {
    None
}

/// cap 注入的内部形态：测试用它把「总量兜底」逼出来。
async fn prune_with_cap(home: &Home, cap_bytes: u64) -> PruneStats {
    let root = home.shared_target_path();
    let stats = tokio::task::spawn_blocking(move || {
        // 有别的任务正持锁构建 → 本次不收（WARN 留痕，机会留给下一个终态事件）。
        // 票据的硬要求：清理不得影响在跑任务——拿不到锁就整段让路。
        let Some(_guard) = try_lock_guard(&root) else {
            tracing::warn!("共享构建缓存正被在跑的构建占用，本次回收跳过");
            return PruneStats::default();
        };
        let incremental_bytes = remove_incremental_trees(&root);
        let total_after_incremental = dir_size(&root);
        let mut full_reset = false;
        let mut remaining_bytes = total_after_incremental;
        if total_after_incremental > cap_bytes {
            full_reset = true;
            match std::fs::remove_dir_all(&root) {
                Ok(()) => remaining_bytes = 0,
                Err(e) => {
                    // 清空失败如实记：full_reset 仍为真（意图已发生），读数按实重测——
                    // 不许把「没删掉」报成「已归零」。
                    tracing::warn!(error = %e, "共享构建缓存整体清空失败");
                    remaining_bytes = dir_size(&root);
                }
            }
        }
        if incremental_bytes > 0 || full_reset {
            tracing::info!(
                incremental_mb = incremental_bytes / (1024 * 1024),
                full_reset,
                remaining_mb = remaining_bytes / (1024 * 1024),
                "共享构建缓存已回收"
            );
        }
        PruneStats {
            incremental_bytes,
            full_reset,
            remaining_bytes,
        }
    })
    .await
    .unwrap_or_default();
    stats
}

/// 删除 `{debug,release}/incremental`（尽力而为），返回删掉的字节数。
fn remove_incremental_trees(root: &std::path::Path) -> u64 {
    let mut freed = 0;
    for profile in ["debug", "release"] {
        let incremental = root.join(profile).join("incremental");
        if incremental.exists() {
            freed += dir_size(&incremental);
            if let Err(e) = std::fs::remove_dir_all(&incremental) {
                tracing::warn!(path = %incremental.display(), error = %e, "增量树删除失败（不阻断）");
            }
        }
    }
    freed
}

/// 目录总大小（尽力而为；打不开的条目按 0 计）。
fn dir_size(path: &std::path::Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                total += dir_size(&entry.path());
            } else {
                total += meta.len();
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home_in(tmp: &std::path::Path) -> Home {
        Home::new(tmp.join("home"))
    }

    #[tokio::test]
    async fn prune_removes_incremental_trees_and_keeps_deps() {
        let tmp = tempfile::tempdir().unwrap();
        let home = home_in(tmp.path());
        let root = home.shared_target_path();
        let deps = root.join("debug/deps");
        std::fs::create_dir_all(&deps).unwrap();
        std::fs::write(deps.join("libfoo.rlib"), vec![0u8; 100]).unwrap();
        let incremental = root.join("debug/incremental");
        std::fs::create_dir_all(&incremental).unwrap();
        std::fs::write(incremental.join("chunk.bin"), vec![0u8; 200]).unwrap();

        let stats = prune_build_cache(&home).await;

        assert_eq!(stats.incremental_bytes, 200);
        assert!(!stats.full_reset);
        assert_eq!(stats.remaining_bytes, 100);
        assert!(
            deps.join("libfoo.rlib").exists(),
            "deps 是缓存本体,必须保留"
        );
        assert!(!incremental.exists(), "增量树必须删");
    }

    #[tokio::test]
    async fn prune_full_resets_when_over_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let home = home_in(tmp.path());
        let root = home.shared_target_path();
        let deps = root.join("debug/deps");
        std::fs::create_dir_all(&deps).unwrap();
        std::fs::write(deps.join("libbig.rlib"), vec![0u8; 64]).unwrap();

        // cap 注入:64 字节的缓存、32 字节的帽——兜底必须整体清空。
        let stats = prune_with_cap(&home, 32).await;
        assert!(stats.full_reset);
        assert!(!deps.join("libbig.rlib").exists(), "越过帽必须整体清空");
        assert_eq!(stats.remaining_bytes, 0);

        // 同一缓存、正常帽——不动。
        let stats = prune_build_cache(&home).await;
        assert!(!stats.full_reset, "64 字节不该触发 10GiB 的帽");
    }

    #[tokio::test]
    async fn prune_on_missing_dir_is_a_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let home = home_in(tmp.path());
        let stats = prune_build_cache(&home).await;
        assert_eq!(stats, PruneStats::default());
    }
}
