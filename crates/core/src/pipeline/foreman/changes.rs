//! 收口行带机器读出的改动清单（`.scratch/foreman-work-record` 票 01）。
//!
//! **为什么要有这一格**：这一轮的台账行是值班长**下一轮**认识「我做过什么」的唯一读物——
//! 进下一轮 prompt 的只有 `content` 那段散文，工具痕迹（`traces_json`）从不回灌。于是
//! 「本轮动过哪些文件」这件事此前只存在于 `traces_json` 与工作区里，**每一处消费点都读不到**：
//! 2026-10-08 的班次 `01M4CDY9EC9M9FSSE236GJXYZC` 里，394 那一轮改了二十余处，台账正文只有
//! 104 个字，8 分钟后的下一轮据此**如实**汇报「仍未动一行代码」。
//!
//! 这一格把清单升成机器读数，三个消费者：① 收口正文末尾那一段（[`changes_note`]，它进下一轮
//! 的 prompt）；② 行上的 `changed_files_json` 列（随 `ForemanMessage` 带给下一轮与界面）；
//! ③ 五条收场尾句末尾的**产出注记**（[`output_note`]，票 02）与「在打转」那条的两态措辞
//! （[`loop_tail`]）——收场文案从此按「提议数 ∪ 改动文件数」说话，不再对改过东西的轮
//! 建议「换个线索再查」。
//!
//! **判据是工具名 + 参数里的 `path`**（完整参数原串，决策 301 起就在留痕里），**外加**本轮
//! 若调用过 `repair(finish)` 时那份权威 diff 的文件清单——`run_command` 里也可能改文件
//! （`git apply` / `sed -i`），那一支只有 diff 看得见（并集在收口处做）。

use serde_json::Value;

use super::runner::ForemanTrace;
use crate::types::EnvMode;

/// 收口正文里那一段的标记。与 `【操作台】` / `【值守播报】` 同族、同姿态：**由后端加**，
/// 不由模型自己说（模型说错正是这一格要停掉的东西）。
pub(super) const FOREMAN_CHANGES_MARK: &str = "【本轮改动】";

/// 会**在留痕里带出文件路径**的两个动手工具。值班长的工具清单里只有这两个——
/// `delete_file` 不在它的清单内（`FOREMAN_TOOL_SPECS`），`run_command` 改的文件
/// 归 repair 那份 diff 补（见模块文档）。
const PATH_TOOLS: [&str; 2] = ["write_file", "edit_file"];

/// 正文里声称「这一轮没动代码」的短语。清单非空却命中它 → 换一个更正的开头
/// （[`changes_note`] 的 `contradicts_reply`）。
///
/// 三条都出自实账原文（`kanban_foreman_messages.id = 386 / 388 / 400`），不是猜的词表。
const NO_CHANGE_CLAIMS: [&str; 3] = ["零改动", "一行未改", "仍未动一行代码"];

/// 收口正文里**最多列几个路径**。再多只报计数，明细在行上的 `changed_files_json` 列里——
/// 历史窗口是 24000 个**字符**（`FOREMAN_HISTORY_BUDGET_CHARS`），一段 21 行的清单会把它吃掉。
const NOTE_PATHS: usize = 3;

/// 从这一轮的工具痕迹里读出**改动过的文件**：去重、保首次出现序。
///
/// 四条判据，各对应一种「这不算改动」：
/// - **档位不是 `auto`** → 整轮回空。这是最容易漏的一条：值班长的**缺省档位就是 `ask`**
///   （[`EnvMode::default_for`]），而那一档下 env 写工具**落成提议、文件根本没动**
///   （[`crate::agent::tools::gate_decision`]），回执还是一句 `ok = true` 的
///   「已生成一条待确认的提议……这件事**没有执行**」。把那种痕迹算成改动就是**伪造一份成果**
///   ——正是这一格要停掉的那类谎。`deny` 档同理（连工具都没给，走不到这里）。
/// - **工具名**不在 [`PATH_TOOLS`] 里 → 不看（只读工具也有 `path`，但它没改东西）；
/// - **`ok = false`** → 跳过（失败的编辑什么都没改——本轮实账里正好有一次失败的
///   `edit_file`，它不该进清单）；
/// - 参数不是合法 JSON / 没有 `path` / `path` 空白 → 跳过。
///
/// **读不出来不报错**：痕迹是观测面，一轮收口不该因为一条痕迹解析不出来而失败
/// （与 `traces_json` 其余消费点同一条姿态）。
pub(super) fn changed_paths(traces: &[ForemanTrace], env_mode: EnvMode) -> Vec<String> {
    if env_mode != EnvMode::Auto {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    for trace in traces {
        if !trace.ok || !PATH_TOOLS.contains(&trace.tool.as_str()) {
            continue;
        }
        let Ok(args) = serde_json::from_str::<Value>(&trace.args) else {
            continue;
        };
        let Some(path) = args.get("path").and_then(Value::as_str) else {
            continue;
        };
        let path = path.trim();
        if path.is_empty() || out.iter().any(|p| p == path) {
            continue;
        }
        out.push(path.to_string());
    }
    out
}

/// 两份来源的**并集**：痕迹里读出的在前（那是这一轮实际发生序），diff 里多出来的补在后。
///
/// 为什么并集而不是「用 diff 换掉痕迹」：两者覆盖的东西不同——痕迹只看得到
/// `write_file` / `edit_file`，diff 只看得到 repair worktree 里的那一份提交；谁缺了谁，
/// 并集都补得上。顺序上痕迹在前，因为它更接近「这一轮怎么走过来的」。
pub(super) fn union(primary: Vec<String>, extra: Vec<String>) -> Vec<String> {
    let mut out = primary;
    for path in extra {
        let path = path.trim().to_string();
        if path.is_empty() || out.contains(&path) {
            continue;
        }
        out.push(path);
    }
    out
}

/// 从一份 unified diff 里读出改动过的文件（取 `diff --git a/X b/X` 里 `b/` 那一侧）。
///
/// **为什么看这一行而不是 `diff_stat`**：`--stat` 的路径列会被 git 按宽度截断（`...` 开头），
/// 而 `diff --git` 两行是逐字路径。代价是本函数假定路径里不含 `" b/"`——本仓的路径都是
/// ASCII、无空格；真出现带空格的路径时它至多读错一条，**不报错**（观测面的姿态）。
pub(super) fn diff_paths(diff: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in diff.lines() {
        let Some(rest) = line.strip_prefix("diff --git ") else {
            continue;
        };
        let Some((_, right)) = rest.rsplit_once(" b/") else {
            continue;
        };
        let path = right.trim();
        if path.is_empty() || out.iter().any(|p| p == path) {
            continue;
        }
        out.push(path.to_string());
    }
    out
}

/// 正文里是否声称「这一轮没动代码」（判据是 [`NO_CHANGE_CLAIMS`] 里的短语）。
pub(super) fn claims_no_change(content: &str) -> bool {
    NO_CHANGE_CLAIMS.iter().any(|claim| content.contains(claim))
}

/// 收口正文末尾那一段；清单为空回 `None`。
///
/// **清单为空就不出现**——「没有」与「有但是空的」是两件事，与 `briefing_json` /
/// `traces_json` / `segments_json` 同一条口径。
///
/// `contradicts_reply`（本轮正文声称没改代码）时换一个开头，把「与台账不符」说在前面。
/// 机器**不改写**模型的话，只在它后面把事实摆出来——与在打转 / 成本告警 / 归因三处
/// 「标注而非改写」同一条姿态。
pub(super) fn changes_note(paths: &[String], contradicts_reply: bool) -> Option<String> {
    if paths.is_empty() {
        return None;
    }
    let head = if contradicts_reply {
        format!(
            "{FOREMAN_CHANGES_MARK}上面那句说这一轮没改代码，与台账不符——本轮改了 {} 个文件：",
            paths.len()
        )
    } else {
        format!("{FOREMAN_CHANGES_MARK}本轮改了 {} 个文件：", paths.len())
    };
    let shown = paths
        .iter()
        .take(NOTE_PATHS)
        .cloned()
        .collect::<Vec<_>>()
        .join("、");
    let tail = if paths.len() > NOTE_PATHS {
        format!("（余 {} 个见台账明细）。", paths.len() - NOTE_PATHS)
    } else {
        "。".to_string()
    };
    Some(format!("{head}{shown}{tail}"))
}

/// 「本轮动过文件吗」的判据（票 02）：清单非空。「在打转」那条收场尾句按它换措辞——
/// 改过东西的轮不该被建议「换个线索再查」（把修复轮说成研究轮，正是 394 / 405 两轮的谎）。
pub(super) fn touched_any(paths: &[String]) -> bool {
    !paths.is_empty()
}

/// 「在打转」那条的后半句，两态（票 02）：
/// - **动过文件**：建议从「再查一轮」换成「接着改」——清单在文末 [`changes_note`] 那一段
///   （它恒在这一句之后，故说「文末」不说过期的「见上」）；
/// - **没动过**：原文逐字保留——对「确实查不下去」那一支，「换个线索」是对的。
pub(super) fn loop_tail(touched: bool) -> &'static str {
    if touched {
        "已经改过的文件见文末清单，要接着改说一声。"
    } else {
        "要接着查可以让我再来一轮（换个线索）。"
    }
}

/// 收场尾句末尾的**产出注记**（票 02，显式续写决策 311）：内容 = 提议数 ∪ 改动文件数，
/// 四种组合各有措辞——五条停法的尾句**全部**接上它，不再只有「人按停」那一条。
///
/// 两个计数都只说自己算得出来的事：提议数问库（读不出来按 0，决策 311），改动文件数用
/// 本格的汇总器（同一份判据，两处共用）。措辞上「提议都还在，照样可以按」与「本轮没有提
/// 任何提议」两句是决策 311 的原文——**单来源的两种组合里逐字保留**（既有两条 L2 逐字钉着
/// 它们）；并上改动文件数的两种组合里，311 那句作为前半句嵌入（`；`连接、句号让给整句），
/// 字未改但不再是独立句。
pub(super) fn output_note(proposals: usize, changed: usize) -> String {
    match (proposals > 0, changed > 0) {
        (false, false) => "本轮没有提任何提议，也没有改动文件。".to_string(),
        (true, false) => "这一轮提的提议都还在，照样可以按。".to_string(),
        (false, true) => format!("本轮没有提任何提议；本轮改了 {changed} 个文件。"),
        (true, true) => format!("这一轮提的提议都还在，照样可以按；本轮改了 {changed} 个文件。"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trace(tool: &str, path: &str, ok: bool) -> ForemanTrace {
        ForemanTrace {
            tool: tool.to_string(),
            args_summary: format!("{{\"path\":\"{path}\"}}"),
            args: format!("{{\"path\":\"{path}\",\"content\":\"x\"}}"),
            result: String::new(),
            ok,
        }
    }

    #[test]
    fn only_editing_tools_with_a_path_count() {
        let traces = vec![
            trace("read_file", "a.md", true),
            trace("search_content", "b.md", true),
            trace("run_command", "c.md", true),
            trace("write_file", "d.md", true),
        ];
        assert_eq!(
            changed_paths(&traces, EnvMode::Auto),
            vec!["d.md".to_string()]
        );
    }

    /// **缺省档位下这一格必须是空的**：`ask` 档的写调用只落一条提议（回执还是 ok=true），
    /// 文件一个字节都没动——把它读成改动就是伪造一份成果。
    #[test]
    fn ask_mode_never_reports_a_change() {
        let traces = vec![
            trace("write_file", "a.md", true),
            trace("edit_file", "b.md", true),
        ];
        assert!(changed_paths(&traces, EnvMode::Ask).is_empty());
        assert!(changed_paths(&traces, EnvMode::Deny).is_empty());
        assert_eq!(changed_paths(&traces, EnvMode::Auto).len(), 2);
    }

    #[test]
    fn a_failed_edit_changed_nothing() {
        // 本轮实账里正好有一次失败的 edit_file（`old_text` 没找到）——它不该进清单。
        let traces = vec![
            trace("edit_file", "a.md", false),
            trace("edit_file", "b.md", true),
        ];
        assert_eq!(
            changed_paths(&traces, EnvMode::Auto),
            vec!["b.md".to_string()]
        );
    }

    #[test]
    fn dedupes_and_keeps_first_seen_order() {
        let traces = vec![
            trace("edit_file", "b.md", true),
            trace("write_file", "a.md", true),
            trace("edit_file", "b.md", true),
        ];
        assert_eq!(
            changed_paths(&traces, EnvMode::Auto),
            vec!["b.md".to_string(), "a.md".to_string()]
        );
    }

    #[test]
    fn broken_args_or_missing_path_are_skipped_not_errors() {
        let mut broken = trace("edit_file", "a.md", true);
        broken.args = "{ not json".to_string();
        let mut no_path = trace("edit_file", "a.md", true);
        no_path.args = "{\"old_text\":\"x\"}".to_string();
        let mut blank = trace("write_file", "a.md", true);
        blank.args = "{\"path\":\"   \"}".to_string();
        assert!(changed_paths(&[broken, no_path, blank], EnvMode::Auto).is_empty());
        assert!(changed_paths(&[], EnvMode::Auto).is_empty());
    }

    #[test]
    fn union_dedupes_both_sides() {
        let merged = union(
            vec!["a.md".to_string(), "b.md".to_string()],
            vec!["b.md".to_string(), "c.md".to_string()],
        );
        assert_eq!(merged, vec!["a.md", "b.md", "c.md"]);
    }

    #[test]
    fn diff_paths_read_the_git_headers_not_the_hunks() {
        // 真形状（截自 repair 那份 diff）：头部两行，正文里的 `--- a/x` / `+++ b/x`
        // **不该**被当成第二条路径。
        let diff = "diff --git a/frontend/src/lib/actions.ts b/frontend/src/lib/actions.ts\n\
                    index 807ebc6..79d6e34 100644\n\
                    --- a/frontend/src/lib/actions.ts\n\
                    +++ b/frontend/src/lib/actions.ts\n\
                    @@ -1,3 +1,4 @@\n-x\n+y\n\
                    diff --git a/.scratch/ux-audit-3/IMPLEMENTATION.md b/.scratch/ux-audit-3/IMPLEMENTATION.md\n\
                    new file mode 100644\n";
        assert_eq!(
            diff_paths(diff),
            vec![
                "frontend/src/lib/actions.ts".to_string(),
                ".scratch/ux-audit-3/IMPLEMENTATION.md".to_string(),
            ]
        );
        assert!(diff_paths("").is_empty());
        assert!(diff_paths("no headers here").is_empty());
        // 同一文件两段（改 + 删）只算一次。
        assert_eq!(
            diff_paths("diff --git a/a.md b/a.md\ndiff --git a/a.md b/a.md\n"),
            vec!["a.md"]
        );
    }

    #[test]
    fn the_note_is_absent_when_nothing_changed() {
        assert_eq!(changes_note(&[], false), None);
        assert_eq!(changes_note(&[], true), None);
    }

    #[test]
    fn the_note_lists_up_to_three_paths_then_counts() {
        let two = vec!["a.md".to_string(), "b.md".to_string()];
        assert_eq!(
            changes_note(&two, false).unwrap(),
            "【本轮改动】本轮改了 2 个文件：a.md、b.md。"
        );
        let five: Vec<String> = ["a.md", "b.md", "c.md", "d.md", "e.md"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            changes_note(&five, false).unwrap(),
            "【本轮改动】本轮改了 5 个文件：a.md、b.md、c.md（余 2 个见台账明细）。"
        );
    }

    #[test]
    fn the_note_corrects_a_no_change_claim() {
        let one = vec!["a.md".to_string()];
        let note = changes_note(&one, true).unwrap();
        assert!(note.starts_with("【本轮改动】上面那句说这一轮没改代码，与台账不符——"));
        assert!(note.contains("本轮改了 1 个文件：a.md。"));
    }

    #[test]
    fn no_change_claims_are_the_three_from_the_ledger() {
        assert!(claims_no_change("收口。本轮实况：代码零改动，闸门未跑。"));
        assert!(claims_no_change("本轮进展如下，代码**一行未改**。"));
        assert!(claims_no_change(
            "状态如下——**仍未动一行代码**，但障碍已清零"
        ));
        assert!(!claims_no_change("本轮的改动都在 worktree 里，未进主干。"));
    }

    // ── 票 02：产出注记 + 在打转两态 ──────────────────────────────────────

    #[test]
    fn touched_any_is_exactly_the_non_empty_list() {
        assert!(!touched_any(&[]));
        assert!(touched_any(&["a.md".to_string()]));
    }

    /// 「在打转」那条的两态：动过文件就建议「接着改」，没动过保留决策 293 的原文
    /// （「换个线索」对「确实查不下去」那一支是对的）。
    #[test]
    fn the_loop_tail_has_two_states() {
        assert_eq!(loop_tail(false), "要接着查可以让我再来一轮（换个线索）。");
        assert!(
            !loop_tail(true).contains("换个线索"),
            "改过东西的轮不该被建议换个线索再查：{}",
            loop_tail(true)
        );
        assert!(loop_tail(true).contains("要接着改说一声"));
        assert!(loop_tail(true).contains("文末清单"));
    }

    /// 产出注记的四种组合各一条。**两种单来源的措辞是决策 311 的原文**（既有两条 L2
    /// 逐字钉着它们），新增的只是「并上改动文件数」那一半。
    #[test]
    fn output_note_covers_all_four_combinations() {
        assert_eq!(output_note(0, 0), "本轮没有提任何提议，也没有改动文件。");
        assert_eq!(output_note(3, 0), "这一轮提的提议都还在，照样可以按。");
        assert_eq!(output_note(0, 2), "本轮没有提任何提议；本轮改了 2 个文件。");
        assert_eq!(
            output_note(1, 2),
            "这一轮提的提议都还在，照样可以按；本轮改了 2 个文件。"
        );
    }

    #[test]
    fn the_output_note_never_promises_proposals_that_do_not_exist() {
        // 0 提议时**不许**复述那句不成立的保证（2026-09-27 实测的谎，决策 311）。
        assert!(!output_note(0, 0).contains("照样可以按"));
        assert!(!output_note(0, 2).contains("照样可以按"));
        // 有提议时那句一字不改（决策 294 / 修订 233③ 的兑现点）。
        assert!(output_note(1, 0).contains("这一轮提的提议都还在，照样可以按。"));
        assert!(output_note(1, 2).contains("这一轮提的提议都还在，照样可以按；"));
    }
}
