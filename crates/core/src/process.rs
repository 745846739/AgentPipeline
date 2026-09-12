//! 可测试性接缝③：进程组终止器（决策 143 / 66）。
//!
//! 超时路径必须断言"杀了进程组"，但测试里没有真进程可杀。把终止动作抽成 trait：
//! 生产实现真杀，测试实现只记录调用。

use crate::Result;

pub trait ProcessKiller: Send + Sync + 'static {
    /// 向进程组发送终止信号（先 TERM，必要时 KILL）。
    fn kill_process_group(&self, pgid: i32) -> Result<()>;
}

/// 生产实现：真正的进程组终止（决策 66）。
#[derive(Debug, Clone, Copy, Default)]
pub struct RealProcessKiller;

impl ProcessKiller for RealProcessKiller {
    fn kill_process_group(&self, pgid: i32) -> Result<()> {
        if pgid <= 0 {
            return Ok(());
        }
        // 负 pid 表示整个进程组。用 /bin/kill 避免引入 libc 依赖。
        let status = std::process::Command::new("kill")
            .arg("-TERM")
            .arg(format!("-{pgid}"))
            .status()?;
        if !status.success() {
            // 进程可能已退出——不是错误。
            tracing::debug!(pgid, "进程组已不存在或 TERM 失败，尝试 KILL");
            let _ = std::process::Command::new("kill")
                .arg("-KILL")
                .arg(format!("-{pgid}"))
                .status();
        }
        Ok(())
    }
}
