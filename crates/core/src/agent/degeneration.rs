//! 模型输出退化护栏（决策 280）：流式文本的尾部复读循环检测。
//!
//! 现场（任务 01M3BGVCXDWFPT0Q3BZYAGZP8Q 的 run41）：「Playwright 或」无脑复读 150+
//! 次、「路由变化路由变化路由变化」，4,322 completion tokens 大半是垃圾，烧 89 秒后
//! 仍因未交元数据判败——现行机制对退化零感知。护栏在**流式输出层**逐段检查累积文本：
//! 一旦尾部陷入复读循环，立即判废本轮（`Error::Degenerated`），由 agent loop 按决策 278
//! 续接转录＋错误 turn 重试，不再等它烧完。
//!
//! 护栏是机制不是配置（决策 224 姿态）：阈值常量起步，没有第二种诉求之前不扩契约。
//!
//! 检测只看**尾部**：结尾以同一个片段连续重复达 [`MIN_REPEATS`] 次、且重复总长
//! ≥ [`MIN_TOTAL_CHARS`] 字符即判退化。片段长度下限 [`MIN_UNIT_CHARS`] = 3 把
//! Markdown 分隔线（`---` / `═══`——单字符重复）与「……」这类合法排版挡在外面；
//! 「次数 × 总长」双门槛让「哈哈哈哈」这类短重复也够不着。

/// 判退化的片段长度下限（字符）：1–2 字符的重复单元留给排版（分隔线、省略号）。
pub const MIN_UNIT_CHARS: usize = 3;
/// 判退化的片段长度上限（字符）：复读单元实际都很短；上限只影响扫描成本。
pub const MAX_UNIT_CHARS: usize = 48;
/// 连续重复次数下限。
pub const MIN_REPEATS: usize = 5;
/// 重复总长的字符下限：与次数构成双门槛，短单元要叠够长度才算（不误伤感叹）。
pub const MIN_TOTAL_CHARS: usize = 60;

/// 一次退化判定：重复的片段与连续重复次数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Degeneration {
    /// 重复的片段（取自文本结尾）。
    pub unit: String,
    /// 片段在结尾连续出现的次数。
    pub repeats: usize,
}

/// 检测累积文本是否已陷入尾部复读循环。
///
/// 两阶段：① 找尾部的**最小周期** p（最小的 p 使结尾两个 p 字符块相同）——真正复读
/// 循环的最小周期就是它的循环单元；② 周期落在护栏窗口内且「次数 × 总长」双门槛达标
/// 才判退化。最小周期 < [`MIN_UNIT_CHARS`] 的是**单/双字符的重复**（分隔线、省略号、
/// 语气字）——它们的复读是排版不是退化，直接放行；这也堵住了「按 3 字符分组绕过下限」
/// 的洞（`══════` 的最小周期是 1，不会以 p=3 的面目出现）。
///
/// **只扫尾部窗口**（评审发现）：本函数在流式路径逐 chunk 调用，对全量文本
/// `chars().collect()` 是跨 chunk 的 O(n²)。判阈值最多需要「[`MIN_REPEATS`] 次 ×
/// [`MAX_UNIT_CHARS`] 字符」，[`SCAN_WINDOW_CHARS`] 取其 64 倍余量——窗口内的检测
/// 语义与全文扫描完全一致，只是超长复读按窗口内能证明的次数报告。
pub fn detect(content: &str) -> Option<Degeneration> {
    // 字节粗筛（bytes ≥ chars：字节不足下限则字符必不足）
    if content.len() < MIN_TOTAL_CHARS {
        return None;
    }
    let mut start = content.len().saturating_sub(SCAN_WINDOW_CHARS * 4); // 一字符至多 4 字节：窗口保证 ≥ SCAN_WINDOW_CHARS 字符
    while !content.is_char_boundary(start) {
        start -= 1;
    }
    let chars: Vec<char> = content[start..].chars().collect();
    let n = chars.len();
    // ① 最小周期：从头试到窗口上限，第一个「结尾两块相同」的 p 即最小周期
    let max_p = MAX_UNIT_CHARS.min(n / 2);
    let mut p = 1usize;
    while p <= max_p && chars[n - 2 * p..n - p] != chars[n - p..] {
        p += 1;
    }
    if p > max_p {
        return None; // 尾部无周期（窗口内）：正常文本
    }
    if p < MIN_UNIT_CHARS {
        return None; // 单/双字符周期：排版与语气，不是退化
    }
    // 全空白的重复单元（缩进空行等）是排版不是内容复读——「  \n  \n」的最小周期
    // 恰好是 3，光靠周期下限挡不住它。
    let unit: String = chars[n - p..].iter().collect();
    if unit.chars().all(char::is_whitespace) {
        return None;
    }
    // ② 沿最小周期向前数连续重复的块数（含结尾这两块起算的全体）
    let mut repeats = 2usize;
    while (repeats + 1) * p <= n && chars[n - (repeats + 1) * p..n - repeats * p] == chars[n - p..]
    {
        repeats += 1;
    }
    (repeats >= MIN_REPEATS && repeats * p >= MIN_TOTAL_CHARS)
        .then_some(Degeneration { unit, repeats })
}

/// 扫描窗口（字符）：判阈值所需（5 × 48 = 240）的 64 倍余量。
const SCAN_WINDOW_CHARS: usize = MAX_UNIT_CHARS * 64;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_are_pinned() {
        // 决策 224 姿态：阈值常量起步不做配置项。改这里的值 = 有意识地改门槛，
        // 必须连着下面的行为用例一起审。
        assert_eq!(MIN_UNIT_CHARS, 3);
        assert_eq!(MAX_UNIT_CHARS, 48);
        assert_eq!(MIN_REPEATS, 5);
        assert_eq!(MIN_TOTAL_CHARS, 60);
    }

    #[test]
    fn run41_style_repetition_triggers() {
        // 实测现场：英文短语 + 中文收尾字的混合单元，无脑复读 150+ 次
        let content = format!("前文正常的分析。{}", "Playwright 或".repeat(150));
        let d = detect(&content).expect("复读 150 次必须触发");
        assert!(d.repeats >= MIN_REPEATS);
        assert!(content.ends_with(&d.unit), "片段取自文本结尾：{d:?}");
    }

    #[test]
    fn cjk_phrase_repetition_triggers() {
        // 实测现场之二：「路由变化路由变化路由变化」
        let d = detect(&"路由变化".repeat(40)).expect("中文短语复读必须触发");
        assert_eq!(d.unit, "路由变化", "最小周期才是真正的循环单元");
    }

    #[test]
    fn normal_varied_text_does_not_trigger() {
        let content: String = (0..200)
            .map(|i| format!("第 {i} 段：结论随输入而不同，数字 {i} 也不同。\n"))
            .collect();
        assert_eq!(detect(&content), None, "正常文本不得误伤");
    }

    #[test]
    fn markdown_dividers_do_not_trigger() {
        // 排版性重复：单字符 / 双字符单元（分隔线、省略号）不判退化
        for divider in [
            "═".repeat(80),
            "-".repeat(120),
            "…".repeat(60),
            "  \n".repeat(60),
        ] {
            assert_eq!(detect(&divider), None, "分隔线不得误伤：{}", divider.len());
        }
    }

    #[test]
    fn short_or_few_repetitions_do_not_trigger() {
        // 双门槛：次数够但总长不够（「哈哈哈哈」式感叹）、总长够但次数不够
        assert_eq!(detect(&"哈".repeat(120)), None, "单字符重复是排版/语气");
        assert_eq!(detect(&"哈哈".repeat(29)), None, "双字符重复是语气");
        assert_eq!(detect(&"abcdef".repeat(9)), None, "54 字符 < 60：不触发");
        assert_eq!(detect(&"abcdefgh".repeat(4)), None, "只有 4 次 < 5：不触发");
    }

    #[test]
    fn the_double_threshold_bites_at_the_boundary() {
        // 恰好压线（5 次 × 12 字符 = 60 字符）→ 触发：阈值是闭的
        let at = "Playwright 或".repeat(5);
        assert!(detect(&at).is_some(), "5 次 × 12 字符恰达双门槛");
    }

    #[test]
    fn detection_costs_are_bounded_by_the_tail() {
        // 长文本 + 尾部短重复：不因正文长而失控（每次扫描只看尾部窗口）
        let head: String = (0..2_000)
            .map(|i| format!("第 {i} 行正常内容，各不相同。\n"))
            .collect();
        let content = format!("{head}{}", "循环片段X".repeat(13));
        assert!(detect(&content).is_some());
    }
}
