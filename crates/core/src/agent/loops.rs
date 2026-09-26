//! 值班长一轮的**工具循环检测**（决策 293 / 票 08）：判「在原地打转」——同参重复调用、
//! 或者连着几次调用都读不到新东西。
//!
//! 为什么要有它（拷问 Q8 / Q16）：全仓此前**没有任何**同参重复 / 无进展检测，唯一的护栏是
//! 流式文本的尾部复读（[`crate::agent::degeneration`]），而那条只在「模型把同一句话写两遍」
//! 时命中——「查一个任务 → 再查同一个任务」这条路径一个字都拦不住，全靠 token 预算（决策 292）
//! 兜底：钱烧完了才知道它在打转。
//!
//! **两条判据**（裁决 8，都是纯函数、都只看这一轮的调用流水）：
//! ① 同一工具 + **同一完整参数**连续重复 [`REPEAT_LIMIT`] 次；
//! ② 连续 [`NO_PROGRESS_LIMIT`] 次调用的结果都是这一轮里**见过**的（没有新工具结果、没读到新东西）。
//!
//! 判据 ① 的参数用**原串**（不是 `traces` 里那份截断过的 `args_summary`）：摘要判等会把
//! 「两个不同的任务」错判成同一个调用，而误伤的代价正是它要防的那件事（好轮被收口）。
//! 判据 ② 是 ① 的补充：交替查 A / B / A / B 也没有新信息，但它不是连续同参。
//!
//! 阈值**不做成可配**（决策 224 / 256 的姿态：没人会调的旋钮比没有更坏），取值由真实台账定：
//! 2026-09-26 那本台账里 22 轮共 359 次工具调用，**最长连续同参游程 = 1**——同一个调用连着
//! 出现两次都不曾发生，故 [`REPEAT_LIMIT`] = 3 留了整整一倍的余量，而真打转时第三次就拦住。

use std::collections::HashSet;

/// 同一工具 + 同一完整参数**连续**重复到这个次数即判「同参打转」。
pub const REPEAT_LIMIT: usize = 3;
/// 连续这么多次调用的结果都是见过的（这一轮之内）即判「无进展」。
pub const NO_PROGRESS_LIMIT: usize = 5;

/// 一次工具调用的检测记录：工具名 + **参数原串** + 结果指纹。
///
/// 结果只留指纹而不是正文：正文可能上万字，而检测要的只是「是不是同一个东西」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallRecord {
    pub tool: String,
    pub arguments: String,
    pub result_digest: u64,
}

/// 结果指纹（`DefaultHasher`，进程内稳定即可——比的是同一轮里的两次读数，不跨进程、不落库）。
pub fn result_digest(result: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    result.hash(&mut hasher);
    hasher.finish()
}

/// 一次循环判定：判据 + 证据（证据要能写进收口/提醒的文案里，让人看懂它为什么被拦）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Loop {
    /// 同一工具 + 同一完整参数连续重复。
    SameCall { tool: String, times: usize },
    /// 连续若干次调用都没有新信息。
    NoProgress { times: usize },
}

impl Loop {
    /// 人话一句（提醒与收口共用同一份措辞：同一件事两处说法不同会让人以为是两件事）。
    pub fn reason(&self) -> String {
        match self {
            Loop::SameCall { tool, times } => {
                format!("同一个调用（{tool} + 同一份参数）连着做了 {times} 次")
            }
            Loop::NoProgress { times } => {
                format!("连着 {times} 次调用都读不到新东西")
            }
        }
    }
}

/// 判这一轮的调用流水是不是在打转。`None` = 正常。
///
/// 只吃**已发生的**调用（尾部窗口）：提醒注入之后调用方把窗口清空，于是「提醒过了再犯」
/// 这件事由调用方用同一把尺子判——同一个函数、两份窗口，语义不打折。
pub fn detect(calls: &[CallRecord]) -> Option<Loop> {
    // ① 同参重复：数尾部连续相同的 (tool, arguments)
    if let Some(last) = calls.last() {
        let same = calls
            .iter()
            .rev()
            .take_while(|c| c.tool == last.tool && c.arguments == last.arguments)
            .count();
        if same >= REPEAT_LIMIT {
            return Some(Loop::SameCall {
                tool: last.tool.clone(),
                times: same,
            });
        }
    }
    // ② 无进展：从尾部往前数，连续「结果在这一轮里已经出现过」的次数。**首次出现不算**
    //    ——第一次读到某个东西是进展，它的第二次才不是。
    let mut seen: HashSet<u64> = HashSet::new();
    let mut stale = 0usize;
    for record in calls {
        if seen.insert(record.result_digest) {
            stale = 0; // 这一次读到了新东西：连续计数归零
        } else {
            stale += 1;
            if stale >= NO_PROGRESS_LIMIT {
                return Some(Loop::NoProgress { times: stale });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(tool: &str, args: &str, result: &str) -> CallRecord {
        CallRecord {
            tool: tool.into(),
            arguments: args.into(),
            result_digest: result_digest(result),
        }
    }

    #[test]
    fn constants_are_pinned() {
        // 决策 224 / 256 姿态：阈值常量起步、不做配置项。真实台账的标定见模块头
        // （359 次调用最长连续同参游程 = 1）——改这里的值就是有意识地改门槛。
        assert_eq!(REPEAT_LIMIT, 3);
        assert_eq!(NO_PROGRESS_LIMIT, 5);
    }

    #[test]
    fn a_repeated_identical_call_triggers_at_the_limit() {
        // 假重复流：同一工具 + 同一份参数连着做
        let calls = vec![
            call("read_task", r#"{"task_id":"t1"}"#, "任务 t1：pending"),
            call("read_task", r#"{"task_id":"t1"}"#, "任务 t1：pending"),
            call("read_task", r#"{"task_id":"t1"}"#, "任务 t1：pending"),
        ];
        assert_eq!(
            detect(&calls),
            Some(Loop::SameCall {
                tool: "read_task".into(),
                times: 3
            })
        );
        // 两次不触发：真实台账里连两次都没有发生过，第三次才是「在做同一件事」
        assert_eq!(detect(&calls[..2]), None);
    }

    #[test]
    fn a_real_progress_stream_never_triggers() {
        // 真进展流：查不同的任务、读不同的文件、跑不同的命令——一路都是新信息
        let calls = vec![
            call("read_task", r#"{"task_id":"t1"}"#, "任务 t1：pending"),
            call(
                "read_diagnosis",
                r#"{"task_id":"t1"}"#,
                "诊断包：run 49 超时",
            ),
            call("read_task", r#"{"task_id":"t2"}"#, "任务 t2：running"),
            call("run_readonly", r#"{"command":"ps"}"#, "ps 输出：三个进程"),
            call(
                "read_conversation",
                r#"{"task_id":"t1","run_id":49}"#,
                "会话：栈落在 parse",
            ),
            call(
                "read_file",
                r#"{"path":"logs/serve.log"}"#,
                "日志尾部：ECONNRESET",
            ),
        ];
        assert_eq!(detect(&calls), None);
    }

    #[test]
    fn alternating_between_two_reads_counts_as_no_progress() {
        // 交替查 A / B 也没有新信息——它绕过了同参重复那条判据，故由无进展那条接住。
        // 计数从两次**首见**之后起算：连着 5 次读到已见过的结果才判（第 7 次调用）。
        let mut calls = vec![
            call("read_task", r#"{"task_id":"t1"}"#, "任务 t1：pending"),
            call("read_task", r#"{"task_id":"t2"}"#, "任务 t2：running"),
        ];
        for i in 0..4 {
            calls.push(call(
                "read_task",
                &format!(r#"{{"task_id":"t{}"}}"#, if i % 2 == 0 { 1 } else { 2 }),
                if i % 2 == 0 {
                    "任务 t1：pending"
                } else {
                    "任务 t2：running"
                },
            ));
        }
        assert_eq!(detect(&calls), None, "只攒到 4 次还不判");
        calls.push(call("read_task", r#"{"task_id":"t1"}"#, "任务 t1：pending"));
        assert_eq!(detect(&calls), Some(Loop::NoProgress { times: 5 }));
    }

    #[test]
    fn the_same_tool_with_different_arguments_is_progress() {
        // 同一个工具、不同参数、不同结果：那是**在干活**（拿诊断包的每一节都是这样读的）
        let calls = (0..8)
            .map(|i| {
                call(
                    "read_conversation",
                    &format!(r#"{{"task_id":"t1","run_id":{i}}}"#),
                    &format!("会话 {i}：不同的现场"),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(detect(&calls), None);
    }

    #[test]
    fn a_new_result_resets_the_stale_streak() {
        // 中间读到一个新东西 → 归零；否则「查四次没变 + 一次有变 + 再四次没变」会被误判成
        // 连着八次无进展（参数各不相同，故同参那条判据不会先接住）。
        let mut calls = Vec::new();
        for i in 0..4 {
            calls.push(call(
                "read_task",
                &format!(r#"{{"task_id":"t{}"}}"#, i % 2 + 1),
                if i % 2 == 0 {
                    "任务 t1：pending"
                } else {
                    "任务 t2：running"
                },
            ));
        }
        calls.push(call(
            "run_readonly",
            r#"{"command":"date"}"#,
            "现在是 12:00",
        ));
        for i in 0..4 {
            calls.push(call(
                "read_task",
                &format!(r#"{{"task_id":"t{}"}}"#, i % 2 + 1),
                if i % 2 == 0 {
                    "任务 t1：pending"
                } else {
                    "任务 t2：running"
                },
            ));
        }
        assert_eq!(detect(&calls), None, "新信息之后的连续计数必须从零起算");
    }
}
