//! 假时钟（决策 143 接缝①）：超时 / 心跳 / tick 的时间语义靠它确定性验证。

use std::sync::{Arc, Mutex};

use agentpipeline_core::clock::Clock;
use chrono::{DateTime, Duration, Utc};

/// 手动推进的时钟。
#[derive(Debug, Clone)]
pub struct ManualClock {
    now: Arc<Mutex<DateTime<Utc>>>,
}

impl Default for ManualClock {
    fn default() -> Self {
        ManualClock::new(Utc::now())
    }
}

impl ManualClock {
    pub fn new(start: DateTime<Utc>) -> Self {
        ManualClock {
            now: Arc::new(Mutex::new(start)),
        }
    }

    /// 从固定时刻开始（测试更可控）。
    pub fn fixed() -> Self {
        let start = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        ManualClock::new(start)
    }

    pub fn advance_secs(&self, secs: i64) {
        let mut guard = self.now.lock().unwrap();
        *guard += Duration::seconds(secs);
    }

    pub fn set(&self, value: DateTime<Utc>) {
        *self.now.lock().unwrap() = value;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        *self.now.lock().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_advances_manually() {
        let clock = ManualClock::fixed();
        let t0 = clock.now();
        clock.advance_secs(600);
        assert_eq!((clock.now() - t0).num_seconds(), 600);

        let shared: Arc<dyn Clock> = Arc::new(clock.clone());
        clock.advance_secs(1);
        assert_eq!((shared.now() - t0).num_seconds(), 601);
    }
}
