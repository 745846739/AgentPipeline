//! 存储 I/O 预算的水位读数（决策 321）。
//!
//! 背景：2026-09 的慢 SQL 排查结论——秒级慢语句的主体不是查询形状（全库 28MB、
//! 单行最大 3MB、索引无缺失），而是「写放大 × 机器 I/O 停顿」；且触发条件
//! （磁盘水位、WAL 大小）当时没有任何读数，复发时只能靠猜。本模块把这三样
//! 变成一行日志就能看见的东西：
//!
//! - **WAL 文件大小**：WAL 无界增长是「长读快照挡 checkpoint + 持续写入」的形态
//!   （实测可涨到 GB 级），是慢写的上游信号；
//! - **库文件大小**：基线；
//! - **磁盘剩余空间**：APFS 接近满时写入停顿是秒级到分钟级的经典来源（排查期间
//!   磁盘实锤打满过一次）。
//!
//! 读数的三条出口：维护作业的 checkpoint 行（[`super::Store::checkpoint_wal`]）、
//! app 层慢语句告警的伴随行（`storage::io_budget` target）、以及这里可直接单测的
//! 纯函数。**读数失败一律 `None` 不 panic**——可观测性是 best-effort，不许把
//! 主流程带下水。

/// 一次 checkpoint 的结果与当时的存储水位。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CheckpointOutcome {
    /// TRUNCATE 是否因读者未退让而放弃收缩（`true` = 没收干净，下一趟再试）。
    pub busy: bool,
    /// checkpoint 开始时 WAL 里的总页数。
    pub log_pages: i64,
    /// 真正收缩掉的页数。
    pub checkpointed_pages: i64,
    /// checkpoint 前 / 后的 WAL 文件字节数（`None` = WAL 文件不存在或读不到）。
    pub wal_bytes_before: Option<u64>,
    pub wal_bytes_after: Option<u64>,
    /// 库文件字节数。
    pub db_bytes: Option<u64>,
    /// 数据目录所在卷的剩余字节数。
    pub disk_free_bytes: Option<u64>,
}

/// 任意文件的字节数；不存在或读不到返回 `None`。
pub fn file_bytes(path: &std::path::Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|m| m.len())
}

/// WAL 伴生文件（`<db>-wal`）的字节数；没有在写的 WAL 时文件不存在 → `None`。
pub fn wal_bytes(db_path: &std::path::Path) -> Option<u64> {
    let wal = wal_path(db_path);
    file_bytes(&wal)
}

fn wal_path(db_path: &std::path::Path) -> std::path::PathBuf {
    // 与 `Store::open` 的权限收口同一拼法：`<文件名>-wal` 挂在数据目录下。
    let name = db_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    db_path.with_file_name(format!("{name}-wal"))
}

/// `path` 所在卷的剩余字节数（statvfs）；非 unix 或调用失败返回 `None`。
///
/// `#[allow(clippy::unnecessary_cast)]`：statvfs 字段类型按平台漂移（darwin 的
/// `fsblkcnt_t` 是 u32、`f_frsize` 是 c_ulong，linux 两侧都是 u64）——`as u64`
/// 在有的平台是必要转换、有的平台是同宽转换，两边用同一个写法才不用按平台分叉。
/// 数值上限是「块数 × 块大小」，u64 装不下任何真实磁盘。
#[allow(clippy::unnecessary_cast)]
pub fn disk_free_bytes(path: &std::path::Path) -> Option<u64> {
    #[cfg(unix)]
    {
        let c = std::ffi::CString::new(path.to_string_lossy().as_bytes()).ok()?;
        let mut fs: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY：c 指向合法的 NUL 结尾串，fs 是本线程栈上的合法 statvfs。
        let rc = unsafe { libc::statvfs(c.as_ptr(), &mut fs) };
        if rc != 0 {
            return None;
        }
        Some(fs.f_bavail as u64 * fs.f_frsize as u64)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_bytes_on_missing_file_is_none() {
        assert_eq!(
            file_bytes(std::path::Path::new("/nonexistent-ap-318")),
            None
        );
    }

    #[test]
    fn wal_bytes_follows_the_sibling_naming() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("x.db");
        std::fs::write(dir.path().join("x.db-wal"), b"12345").unwrap();
        assert_eq!(wal_bytes(&db), Some(5));
        // 没有 WAL 在写时就是 None，不是 0。
        std::fs::remove_file(dir.path().join("x.db-wal")).unwrap();
        assert_eq!(wal_bytes(&db), None);
    }

    #[test]
    fn disk_free_is_plausible_on_unix() {
        let free = disk_free_bytes(std::path::Path::new("/tmp"));
        if cfg!(unix) {
            let free = free.expect("unix 上 statvfs 不该失败");
            assert!(free > 0, "剩余空间不该是 0：{free}");
        } else {
            assert_eq!(free, None);
        }
    }
}
