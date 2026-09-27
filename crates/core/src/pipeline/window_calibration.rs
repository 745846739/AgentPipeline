//! 窗口界的标定（决策 309，票 foreman-burns-without-guard 01）。
//!
//! **触发线为什么曾经永远摸不到**：轮内压缩的触发线是「估算是窗口的 80%」（决策 291 的
//! 用意不动），而**两边都在错**、并且**偶然抵消**：
//!
//! - **估算侧偏低 ≥ 5.5 倍**：第 94 轮真实输入 **561,210**，而当轮估算 ≤ 102,400 ⇒ 低估 ≥ 5.5×；
//! - **窗口侧偏低 ≥ 4.5 倍**：provider 行上写着 **128,000**，而 561,210 的输入照样 `ok`；
//! - 于是触发线 ≈ 0.8 × 128,000 = 102,400「估算」≈ **56 万真实 token**——正好压在真实窗口
//!   边缘：95 轮**一次都没触发**（全库 0 条「值班长轮内上下文超线」，而管道侧同款压缩有命中，
//!   说明日志链路是通的，是这条线本身永远摸不到）。
//!
//! **为什么必须同批改**（这一条是「只修一个比不修更糟」的量化论证）：
//!
//! | 改法 | 触发线（折算成真实 token） | 占真实窗口 |
//! |---|---|---|
//! | 都不改 | 0.8 × 128,000 × 5.5 ≈ 563,200 | ≈ 98%（压在边缘，摸不到） |
//! | **只修窗口** | 0.8 × 576,210 × 5.5 ≈ 2,535,324 | ≈ 4.4 倍（**永不触发**） |
//! | **只修估算** | 0.8 × 128,000 × 1.0 = 102,400 | ≈ 18%（**每轮都压、缓存全废**） |
//! | 两个一起改 | 0.8 × 576,210 × 1.0 ≈ 460,968 | **80%**（就是想要的那条线） |
//!
//! 这套算术在 [`tests::both_fixes_together_land_the_line_at_eighty_percent`] 里逐个钉住：
//! **任一侧单独回滚都会让它越出合理带**——那正是「同批」这个词的可执行含义。

/// 本次事故那 **94 条真实读数**（逐条抄自 `kanban_model_requests`，会话
/// `01M3GFQR2JBRXAY61WY8AC9GJ5`，2026-09-27 03:50–06:17 UTC，按 `id` 升序）。
///
/// **这是真实台账，不是构造的**：它同时是「估算偏低」与「真实窗口下界」两条断言的证据。
/// 抄进代码而不是留在库里，是因为库在每台机器上各不相同，而这条标定的效力必须可回归。
///
/// **如实记**：那个会话在库里是 **95 行**，第 95 行（`seq = 95`，`id = 1002`）的
/// `status = timeout`、`prompt_tokens IS NULL`——它**没有读数**，故这里是 94 条数。
/// 这条不是瑕疵而是台账的原样：那一轮本身也是超时收场的。
pub const INCIDENT_PROMPT_TOKENS: [u64; 94] = [
    6_602, 8_199, 9_807, 14_351, 17_688, 20_571, 24_859, 27_070, 28_660, 33_169, 41_732, 51_516,
    59_315, 62_845, 67_258, 69_528, 75_321, 77_999, 90_705, 95_450, 103_308, 106_656, 109_050,
    116_703, 122_622, 132_808, 136_352, 144_629, 152_094, 168_862, 175_333, 194_202, 205_589,
    211_335, 211_743, 211_920, 212_165, 214_245, 222_638, 231_068, 240_732, 242_411, 247_159,
    247_709, 249_381, 249_511, 250_264, 262_772, 271_336, 276_384, 286_752, 293_904, 298_890,
    306_357, 306_838, 307_040, 308_167, 308_930, 310_175, 316_388, 317_625, 319_577, 321_144,
    324_690, 330_508, 339_710, 355_834, 368_920, 369_384, 374_606, 380_826, 381_036, 381_170,
    381_321, 381_534, 382_581, 384_029, 392_631, 401_240, 401_920, 415_832, 420_749, 424_088,
    432_614, 440_162, 462_464, 470_621, 482_796, 493_181, 509_912, 520_501, 533_178, 546_326,
    561_210,
];

/// 真实上下文窗口的**下界**：实测最大输入 + 输出预留。
///
/// 推导：有读数的**末条**是 **561,210** 且那一次**成功返回了**——一个被接受的请求
/// 说明窗口至少容得下它的输入**加**这次生成的输出。输出预留取 [`OUTPUT_RESERVE_TOKENS`]
/// （15,000，与 `context::estimate_context_capacity` 的输出预留同一量级）。
///
/// **如实记**：精确值要向代理核实（spec 的残余风险 1），这条是**可证的下界**，
/// 不是「真实值」。撞墙自校准（[`calibrated_window`]）是兜底而不是替代。
pub const PROVIDER_WINDOW_LOWER_BOUND: usize = 576_210;

/// 输出预留（下界推导里那一项）：561,210 + 15,000 = 576,210。
pub const OUTPUT_RESERVE_TOKENS: usize = 15_000;

/// 事故当时 provider 行上写着的那一个窗口值（实测）。
pub const MISCONFIGURED_WINDOW: usize = 128_000;

/// 实测的**低估倍数下界**：561,210 真实 vs ≤ 102,400 估算。
pub const MEASURED_UNDERESTIMATE_FACTOR: f64 = 5.5;

/// 轮内压缩的触发比例（窗口的 80%，决策 291 的**用意**不动；本模块改的是这条线凭什么算得准）。
pub const COMPACT_TRIGGER_RATIO: f64 = 0.8;

/// 触发线折算成**真实 token**：`0.8 × 配置窗口 × 低估倍数`。
///
/// `underestimate` 的两种取值就是「估算有没有校准」：未校准 = [`MEASURED_UNDERESTIMATE_FACTOR`]，
/// 已校准 = 1.0。把它写成参数是为了让上面那张对照表可以被逐个断言——**这不是运行时口径**，
/// 是标定算术（运行时只有「估算 vs 窗口」这一条线，就在 [`crate::agent::context`] 里）。
pub fn trigger_line_in_real_tokens(window: usize, underestimate: f64) -> f64 {
    COMPACT_TRIGGER_RATIO * window as f64 * underestimate
}

/// 撞墙自校准后的窗口值：**只上调**，且不低于 [`PROVIDER_WINDOW_LOWER_BOUND`]。
///
/// 撞墙（provider 报上下文超长）给出的信息是「这一次请求对真实窗口来说太大了」。
/// 结合已知的实测事实（配置值 128,000 远低于真实值），此时**上调**该行是正确的方向：
/// 配置值偏低会让触发线偏低（每轮都压、缓存全废），而偏高的代价由决策 295 的
/// 「撞墙就地压缩一次再重试」兜住。
///
/// 取的三个数里最大的那个：
/// - `current`：绝不下调（下调会立刻把已经压好的线拉低）；
/// - `observed_estimate`：这一次请求的估算规模（估算已校准 ⇒ ≈ 真实规模）；
/// - [`PROVIDER_WINDOW_LOWER_BOUND`]：这台机器上**已被证明能装下**的规模。
pub fn calibrated_window(current: usize, observed_estimate: usize) -> usize {
    current
        .max(observed_estimate)
        .max(PROVIDER_WINDOW_LOWER_BOUND)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::context::TOKEN_ESTIMATE_TOLERANCE;

    /// 那 95 条读数**原样**钉住：条数、首末、单调不降。
    ///
    /// 它们是本次标定的证据，被谁顺手改一个数都会让下游几条断言失去意义
    /// （例如「末值 = 下界推导里的那一项」）。
    #[test]
    fn the_incident_ledger_is_pinned() {
        assert_eq!(
            INCIDENT_PROMPT_TOKENS.len(),
            94,
            "95 行里有一条是 timeout、没有读数"
        );
        assert_eq!(INCIDENT_PROMPT_TOKENS[0], 6_602, "首轮");
        assert_eq!(*INCIDENT_PROMPT_TOKENS.last().unwrap(), 561_210, "末轮");
        for w in INCIDENT_PROMPT_TOKENS.windows(2) {
            assert!(w[0] <= w[1], "单调不降：{} → {}", w[0], w[1]);
        }
        // 事故当时那一行写着的窗口，比实测输入还小 —— 这就是「窗口侧偏低」的证据。
        assert!(
            MISCONFIGURED_WINDOW < *INCIDENT_PROMPT_TOKENS.last().unwrap() as usize,
            "配置的窗口必须小于实测输入，否则「配置偏低」这个前提不成立"
        );
    }

    /// 下界推导的自洽：末轮输入 + 输出预留 == 下界。
    #[test]
    fn the_lower_bound_is_the_last_reading_plus_the_output_reserve() {
        let last = *INCIDENT_PROMPT_TOKENS.last().unwrap() as usize;
        assert_eq!(
            PROVIDER_WINDOW_LOWER_BOUND,
            last + OUTPUT_RESERVE_TOKENS,
            "下界的推导写在 const 的文档里，这里把它算一遍"
        );
        // 与末条读数比而不是写字面量：这条断言要连着上面那条一起读（下界必须真能装下它）。
        assert!(
            PROVIDER_WINDOW_LOWER_BOUND > last,
            "下界必须大于实测最大输入"
        );
    }

    /// **同批断言**：两处一起改，线正好落在真实窗口的 80%；任一侧单独回滚都越出合理带。
    ///
    /// 合理带取 `[0.6, 0.9]`：低于 60% 是「压得太勤」（缓存全废），高于 90% 是「**太晚**」
    /// ——那正是这次的病（0.977 意味着只有真到窗口边缘才触发，等于没护栏）。
    /// 这条是**标定算术**的回归，不是运行时行为的断言——它把四处数字
    /// （0.8 / 128,000 / 576,210 / 5.5）钉在一起，改任意一处都会红。
    #[test]
    fn both_fixes_together_land_the_line_at_eighty_percent() {
        let real = PROVIDER_WINDOW_LOWER_BOUND as f64;
        // 分母恒为**真实窗口**：这样每一项读出来都是「线落在真实窗口的哪里」。
        let ratio = |window: usize, factor: f64| trigger_line_in_real_tokens(window, factor) / real;

        // 两个一起改：0.8
        let both = ratio(PROVIDER_WINDOW_LOWER_BOUND, 1.0);
        assert!((both - 0.8).abs() < 1e-9, "落在 80%：{both}");

        // 都不改：压在边缘（≈0.98）—— 正是这次「一次都没触发」的位置
        let neither = ratio(MISCONFIGURED_WINDOW, MEASURED_UNDERESTIMATE_FACTOR);
        assert!(
            (0.9..1.1).contains(&neither),
            "旧的一对压在真实窗口边缘：{neither}"
        );

        // 只修窗口：线抬到真实窗口的 4 倍以上 ⇒ 永不触发
        let window_only = ratio(PROVIDER_WINDOW_LOWER_BOUND, MEASURED_UNDERESTIMATE_FACTOR);
        assert!(
            window_only > 1.0,
            "只修窗口 ⇒ 永不触发（越高越糟）：{window_only}"
        );

        // 只修估算：线掉到真实窗口的 18% ⇒ 每轮都压
        let estimate_only = ratio(MISCONFIGURED_WINDOW, 1.0);
        assert!(
            estimate_only < 0.5,
            "只修估算 ⇒ 每轮都压、缓存全废：{estimate_only}"
        );

        // 合理带只容得下「两个一起改」这一种
        let band = 0.6..=0.9;
        assert!(band.contains(&both));
        assert!(!band.contains(&neither));
        assert!(!band.contains(&window_only));
        assert!(!band.contains(&estimate_only));
    }

    /// 估算的**容差**是那条线的最后一道保险：`估算 ÷ 真读数 ∈ [0.7, 1.5]`——
    /// 下界卡得紧，因为偏低会让线摸不到。
    #[test]
    fn the_estimate_tolerance_is_asymmetric_on_purpose() {
        let (lo, hi) = TOKEN_ESTIMATE_TOLERANCE;
        assert!(lo < 1.0 && hi > 1.0);
        assert!(lo < hi);
        assert!(1.0 - lo < hi - 1.0, "偏低更危险，故下界离 1 更近");
        // 在这条容差里，触发线的落点仍在真实窗口的 56%–120% 之间——不会「永不触发」，
        // 也不会「每轮都压」。
        let worst_high = 0.8 * hi;
        let worst_low = 0.8 * lo;
        assert!(worst_high <= 1.25, "{worst_high}");
        assert!(worst_low >= 0.5, "{worst_low}");
    }

    /// 撞墙自校准：**只上调**，且不吃低于实测下界。
    #[test]
    fn the_wall_calibration_only_raises() {
        // 事故当时的行 + 一次撞墙 ⇒ 抬到实测下界（那台机器上已被证明装得下的规模）
        assert_eq!(
            calibrated_window(MISCONFIGURED_WINDOW, 400_000),
            PROVIDER_WINDOW_LOWER_BOUND
        );
        // 观察值更大就取观察值
        assert_eq!(calibrated_window(MISCONFIGURED_WINDOW, 900_000), 900_000);
        // 绝不下调：本来就更宽的行保持原样
        assert_eq!(calibrated_window(2_000_000, 100_000), 2_000_000);
    }
}
