//! 进程组终止器替身（决策 143 接缝③）：记录调用；默认不真杀，可显式要求真收口。

use std::sync::{Arc, Mutex};

use agentpipeline_core::process::{ProcessKiller, RealProcessKiller};
use agentpipeline_core::Result;

/// 记录 `pgid` 的终止器。默认只记账；[`RecordingKiller::with_real_kill`] 记账后再真收口。
#[derive(Debug, Clone, Default)]
pub struct RecordingKiller {
    killed: Arc<Mutex<Vec<i32>>>,
    /// 记账之后要不要真的收口。
    ///
    /// 默认 `None`：多数用例只关心「超时路径叫没叫到终止器」，而它们的命令自己会退出，
    /// 真杀只是对着一个已消失的进程组做一次无用的 `kill`。
    ///
    /// **但起了一条真会挂住的子进程的用例必须用 [`RecordingKiller::with_real_kill`]**：
    /// 替身只记账不真杀，那条子进程就没人收——测试进程退出后它被 init 收养，成了
    /// PPID=1 的孤儿，长期占着一个 fd 和一个已删除的日志文件。
    /// `readonly.rs` 的超时用例（`tail -f` 一个没人写的文件）就这么漏过：本机一次攒下 87 个。
    real: Option<RealProcessKiller>,
}

impl RecordingKiller {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记账 + 真收口。
    ///
    /// 给「起了一条真会挂住的子进程」的用例用（见 `real` 字段的说明）。这样记录下来的
    /// pgid 是**真被杀掉过**的，断言它 `> 0` 也就不只是形状检查。
    pub fn with_real_kill() -> Self {
        Self {
            killed: Arc::new(Mutex::new(Vec::new())),
            real: Some(RealProcessKiller),
        }
    }

    /// 被终止的进程组列表（按调用顺序）。
    pub fn killed_groups(&self) -> Vec<i32> {
        self.killed.lock().unwrap().clone()
    }

    pub fn was_called(&self) -> bool {
        !self.killed.lock().unwrap().is_empty()
    }

    pub fn call_count(&self) -> usize {
        self.killed.lock().unwrap().len()
    }
}

impl ProcessKiller for RecordingKiller {
    fn kill_process_group(&self, pgid: i32) -> Result<()> {
        self.killed.lock().unwrap().push(pgid);
        match &self.real {
            Some(real) => real.kill_process_group(pgid),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_kills_without_killing() {
        let killer = RecordingKiller::new();
        assert!(!killer.was_called());
        killer.kill_process_group(4242).unwrap();
        assert!(killer.was_called());
        assert_eq!(killer.killed_groups(), vec![4242]);
        assert_eq!(killer.call_count(), 1);
    }

    /// `with_real_kill` 照样记账，并且**不因为真杀而失败**：对一个不存在的进程组，
    /// `kill(2)` 回 ESRCH——那不是错误（进程可能已退出），构造器必须照常返回 `Ok`。
    /// 取一个大到不可能存在的 pgid（macOS 的 pid 上限远小于它），避免误伤真进程组。
    #[test]
    fn with_real_kill_records_and_tolerates_missing_group() {
        let killer = RecordingKiller::with_real_kill();
        killer.kill_process_group(1_073_741_823).unwrap();
        assert_eq!(killer.killed_groups(), vec![1_073_741_823]);
    }
}
