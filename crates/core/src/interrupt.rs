//! 打断策略原语（决策 355）：「一个事件只打扰人一次」的唯一实现。
//!
//! ## 为什么有这个模块
//!
//! 上面那条纪律此前在两个引擎里各长一遍、两套词汇互不相通：
//!
//! - **值守轮的唤醒闸门**（`pipeline::foreman` 的 `watch`）：全局开关 → 人轮排队 →
//!   失败退避 → 去抖 → 任务冷却 → 小时上限；
//! - **出机器那条线的礼貌门**（[`crate::notify`]）：分类 → 免打扰 → 每类节流。
//!
//! 于是决策 350 那次「触发面口径」的调整要在四个文件间推理。上提的是**原语**，
//! 不是词汇——两套词汇各留各的（决策 355 明确不做触发面统一），watch 与 notify
//! 各当一个适配器。
//!
//! ## 三件原语（另加退避）
//!
//! | 原语 | 现场里的名字 | 谁用 |
//! |---|---|---|
//! | [去抖](debounce_elapsed) | 攒批窗口：一阵风吹草动只合成一次 | watch |
//! | [冷却](waiting) | 同任务冷却 / 下一趟允许尝试的时刻 / 每类节流 | watch、notify |
//! | [小时上限](over_hourly_cap) + [上限通知去重](cap_notice_due) | 全局唤醒上限，触顶只留一行 | watch |
//! | [指数退避](backoff_secs) | 失败之后隔多久再问一次 | watch |
//!
//! ## 边界（判据唯一，读数两侧）
//!
//! 冷却 / 去抖 / 退避共用一条判据：[`waiting`]——「[`next_knock`] 算出的那一刻还没到」。
//! 查库的那一档（同任务冷却问「窗口内有没有被消费过」）走 SQL 的 `>= 左沿`，
//! 与 [`waiting`] 同一个窗口、差在**恰好落在左沿上**那一个瞬点：[`window_start`] 那一侧
//! 算窗口内（含左沿），[`waiting`] 那一侧算已到点（不含左沿）。两侧的口径都写在这里，
//! 调用点不再自己减时长。
//!
//! ## 无 I/O
//!
//! 全是纯函数：时刻由调用方给（`Store::now()`，测试里是假时钟）——决策 64 的唯一
//! 时钟源不动，落库与读库留在适配器。阈值一个都不持有：数字来自各自的配置
//! （`watch_debounce_sec` / `watch_task_cooldown_minutes` / `watch_max_wakes_per_hour` /
//! [`crate::notify::NotifyPoliteness`]）。
//!
//! ## 这条线以后怎么改
//!
//! 「一次事件一次打扰」的口径调整（决策 350 那一类）此后只碰本文件：适配器只负责
//! 取值、调这几个函数、落账。
//!
//! ## 这是一次搬家，不是改口径
//!
//! 上提的是**判定**，不是行为：watch 闸门与 notify 礼貌门换成调本模块之后，对外行为
//! 与判定边界逐字不变（那两族的既有用例一个字没改就是证据）。唯一一处刻意的差异——
//! 查库侧含左沿、比时刻侧不含——写在上面「边界」一节，并有单测钉着。

use chrono::{DateTime, Duration, Utc};

/// 下一次可以打扰的时刻 = 参照时刻 + 窗口。
///
/// 参照时刻是「上一回真打扰的那一刻」（冷却）或「窗口里最早那件事的时刻」（去抖）。
/// **不公开**：调用点要么自己握着到期时刻（watch 的退避状态里存的就是它），要么用
/// [`debounce_elapsed`] 这一对现成的判定；把「加一个时长」也摆到台面上，只会给第三个
/// 口径留门。
fn next_knock(after: DateTime<Utc>, window: Duration) -> DateTime<Utc> {
    after + window
}

/// 还在等待里吗——冷却 / 去抖 / 退避共用的**唯一**判据。
///
/// `now < deadline` → 按住（true）。恰好到点即放行（不含右沿）。
pub fn waiting(now: DateTime<Utc>, deadline: DateTime<Utc>) -> bool {
    now < deadline
}

/// 去抖：攒批窗口过了吗。
///
/// `oldest` = 窗口里**最早那件事**的时刻。返回 `false` = 再攒一会儿（不为一件事吵两次）；
/// 返回 `true` = 窗口过后的第一趟，把窗口内所有件一起带上（不许丢事件）。
pub fn debounce_elapsed(now: DateTime<Utc>, oldest: DateTime<Utc>, window: Duration) -> bool {
    !waiting(now, next_knock(oldest, window))
}

/// 冷却窗口的**左沿**（含）：`now - window`。
///
/// 查库那一侧拿它当界（`consumed_at >= 左沿` 即算窗口内，SQL 的 `>=`）——恰好落在左沿上
/// 算「刚发生过」。比时刻那一侧用 [`waiting`]（不含左沿）。同一个窗口的两种读数，
/// 差在相等那一个瞬点，各自的口径以本文件为准。
pub fn window_start(now: DateTime<Utc>, window: Duration) -> DateTime<Utc> {
    now - window
}

/// 每类节流的**间隔形态**（[`crate::notify`] 的 `last_age_sec` 是算好的秒数，不是时刻）。
///
/// `Some(age) if age < window_sec` → 还在冷却里；`None` = 这一类还没发过 → 不拦。
pub fn cooling_by_age(last_age_sec: Option<u64>, window_sec: u64) -> bool {
    matches!(last_age_sec, Some(age) if age < window_sec)
}

/// 指数退避：连续第 `consecutive` 次失败之后等多久（`base × 2^(n-1)`，封顶 `max_secs`）。
///
/// `consecutive` 从 1 起算（第 1 次失败等 `base_secs`）。翻倍到第 7 次为止（`2^6`），
/// 再往后就靠 `max_secs` 封顶——没有第二种诉求之前不扩契约（决策 224 的姿态）。
pub fn backoff_secs(consecutive: u32, base_secs: i64, max_secs: i64) -> i64 {
    let double = 1i64 << (consecutive.saturating_sub(1)).min(6);
    base_secs.saturating_mul(double).min(max_secs)
}

/// 小时上限：窗口内的打扰次数已达上限。
///
/// 触顶**不是**让值守长闭嘴——适配器要留一行「本小时已达上限，N 条待办未播报」，
/// 且待办不消费（下一小时继续，一条不丢）。
pub fn over_hourly_cap(count_in_window: usize, cap: u64) -> bool {
    count_in_window as u64 >= cap
}

/// 触顶通知去重：同一窗口只留一行。
///
/// 没有这一件，触顶本身会变成刷屏源（每一趟都记一行）。
pub fn cap_notice_due(notices_in_window: usize) -> bool {
    notices_in_window == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000 + secs, 0).expect("合法时间戳")
    }

    #[test]
    fn next_knock_is_the_reference_plus_the_window() {
        assert_eq!(next_knock(at(0), Duration::seconds(60)), at(60));
        assert_eq!(next_knock(at(0), Duration::zero()), at(0));
    }

    #[test]
    fn waiting_holds_until_the_deadline_and_releases_exactly_at_it() {
        assert!(waiting(at(0), at(1)), "还没到点：按住");
        assert!(!waiting(at(1), at(1)), "恰好到点：放行（不含右沿）");
        assert!(!waiting(at(2), at(1)), "过了：放行");
    }

    #[test]
    fn debounce_holds_a_burst_and_releases_when_the_window_elapses() {
        let window = Duration::seconds(60);
        assert!(!debounce_elapsed(at(0), at(0), window), "窗口起点：再攒");
        assert!(!debounce_elapsed(at(59), at(0), window), "窗口内：再攒");
        assert!(
            debounce_elapsed(at(60), at(0), window),
            "恰好到窗口右沿：放行"
        );
        assert!(debounce_elapsed(at(61), at(0), window), "过窗口：放行");
        // 参照的是**最早那件**而不是最新那件：窗口内又来了新事件也不重置窗口。
        assert!(
            !debounce_elapsed(at(61), at(30), window),
            "窗口从最早那件算起：晚来的事件不把窗口推后"
        );
    }

    #[test]
    fn window_start_is_the_reference_minus_the_window() {
        assert_eq!(window_start(at(600), Duration::seconds(600)), at(0));
        assert_eq!(window_start(at(0), Duration::zero()), at(0));
    }

    /// 同一个窗口的两种读数**恰好差在相等那一个瞬点**：查库那一侧（SQL 的
    /// `consumed_at >= 左沿`）把落在左沿上的一次算进窗口，比时刻那一侧
    /// （[`waiting`] / [`debounce_elapsed`] 那一族）说窗口已过。钉住它——想统一口径
    /// 的人得先看见这条缝。
    ///
    /// **为什么对照物是 `debounce_elapsed`**：走 SQL 那一档是 watch 的**同任务冷却**，
    /// 它的时钟侧读法与去抖是同一条算术（间隔不足窗口即算窗口内），故这里是同族对照，
    /// 不是随手挑了一个函数。
    #[test]
    fn the_db_side_and_the_clock_side_differ_only_at_the_left_edge() {
        let window = Duration::seconds(60);
        let consumed_at = at(0);
        let now = at(60);
        // 恰在一个窗口之前消费过一次：库里算「窗口内」，时钟那一侧算「窗口已过」。
        assert!(
            consumed_at >= window_start(now, window),
            "查库侧：`>= 左沿` → 仍算窗口内（冷却成立）"
        );
        assert!(
            debounce_elapsed(now, consumed_at, window),
            "时钟侧：恰好一个窗口 → 已过（不含左沿）"
        );
        // 差只有一个瞬点：往里挪一毫秒，两侧就一致了。
        let just_inside = at(0) + Duration::milliseconds(1);
        assert!(
            just_inside >= window_start(now, window),
            "查库侧照旧算窗口内"
        );
        assert!(
            !debounce_elapsed(now, just_inside, window),
            "时钟侧也回到窗口内"
        );
    }

    #[test]
    fn cooling_by_age_pins_both_ends_and_the_never_sent_case() {
        assert!(!cooling_by_age(None, 900), "这一类还没发过：不拦");
        assert!(cooling_by_age(Some(0), 900), "刚发过：拦");
        assert!(cooling_by_age(Some(899), 900), "冷却内：拦");
        assert!(!cooling_by_age(Some(900), 900), "恰好到点：放行");
        assert!(!cooling_by_age(Some(901), 900), "过了：放行");
        assert!(!cooling_by_age(Some(0), 0), "0 秒 = 不节流");
    }

    #[test]
    fn backoff_doubles_from_the_base_and_saturates_at_the_max() {
        assert_eq!(backoff_secs(1, 30, 600), 30, "第 1 次：base");
        assert_eq!(backoff_secs(2, 30, 600), 60);
        assert_eq!(backoff_secs(3, 30, 600), 120);
        assert_eq!(backoff_secs(4, 30, 600), 240);
        assert_eq!(backoff_secs(5, 30, 600), 480);
        assert_eq!(backoff_secs(5, 300, 1800), 1800, "2^4 × 300 已到顶");
        assert_eq!(backoff_secs(6, 30, 600), 600, "翻倍被 max 封顶");
        assert_eq!(backoff_secs(7, 30, 600), 600, "第 7 次起不再翻倍");
        assert_eq!(backoff_secs(99, 30, 600), 600, "计数再大也不溢出");
    }

    #[test]
    fn hourly_cap_and_its_notice_dedup_are_pinned() {
        assert!(!over_hourly_cap(0, 12));
        assert!(!over_hourly_cap(11, 12), "差一次：还没触顶");
        assert!(over_hourly_cap(12, 12), "恰好到上限：触顶");
        assert!(over_hourly_cap(13, 12), "超了：触顶");
        assert!(over_hourly_cap(0, 0), "上限 0 = 一次都不许醒");

        assert!(cap_notice_due(0), "这一小时还没记过：该留一行");
        assert!(!cap_notice_due(1), "记过了：不再重复");
        assert!(!cap_notice_due(2), "记过更多次：不再重复");
    }
}
