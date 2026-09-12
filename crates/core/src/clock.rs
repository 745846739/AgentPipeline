//! 可测试性接缝①：时钟（决策 143 / 64）。
//!
//! 超时判定、心跳、tick 的唯一时钟源。生产用 [`SystemClock`]；测试用假时钟手动推进
//! （`testkit::ManualClock` 或 `#[tokio::test(start_paused)]`）。

use chrono::{DateTime, Utc};

pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> DateTime<Utc>;
}

/// 生产时钟。
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}
