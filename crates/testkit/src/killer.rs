//! 进程组终止器替身（决策 143 接缝③）：记录调用，不真杀。

use std::sync::{Arc, Mutex};

use agentpipeline_core::process::ProcessKiller;
use agentpipeline_core::Result;

/// 只记录 `pgid` 的终止器。
#[derive(Debug, Clone, Default)]
pub struct RecordingKiller {
    killed: Arc<Mutex<Vec<i32>>>,
}

impl RecordingKiller {
    pub fn new() -> Self {
        Self::default()
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
        Ok(())
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
}
