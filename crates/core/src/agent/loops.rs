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
/// 连续这么多次调用都没有推动**任何落库状态**（提议数 / 任务状态 / 游标）即判「没在推进」
/// （决策 310，**显式修订决策 293**：判据由两条扩到三条）。
///
/// 标定：2026-09-27 那本台账 **167 次工具调用 / 95 轮 / 0 条提议 / 2.5 小时任务状态一字未动**。
/// 取 12 的理由是「要看得见一段持续的无变化」：实测那轮平均每轮 1.8 次调用，12 次约 7 轮——
/// 连着七轮什么都没推动就不是运气了；而**任何一次落库状态变化立刻归零**（见 [`detect`]），
/// 正当的深查不会被它碰到。
pub const STALLED_STATE_LIMIT: usize = 12;

/// 一次工具调用的检测记录：工具名 + **参数原串** + 结果指纹 + 当时的落库状态指纹。
///
/// 结果只留指纹而不是正文：正文可能上万字，而检测要的只是「是不是同一个东西」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallRecord {
    pub tool: String,
    pub arguments: String,
    pub result_digest: u64,
    /// 这一次调用**之后**的落库状态指纹（判据③ 的「进展」，决策 310）。
    pub state_digest: u64,
}

/// 落库状态的一行读数（决策 310 判据③）：**提议数 / 任务状态 / 游标位置**。
///
/// 为什么是这三样而不是「调用来源指纹」：这次「没进展」的**事实本身**就是「2.5 小时任务
/// 状态一字未动、0 条提议」，这个读数系统已经握着；而「读到了没读过的东西」与「在推进」
/// 根本不是一回事——按来源指纹改，这次照样拦不住（167 次调用每次的字节都不同：日志偏移、
/// `updated_at`、sqlite 行）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StateReadings {
    /// 这一轮提的提议数。
    pub proposals: usize,
    /// 全部未归档任务的 `id:status` 明细（拼成一行短文本）。
    pub tasks: String,
    /// 全部活跃游标的 `cursor_id:status:stage:node` 明细。
    pub cursors: String,
}

impl StateReadings {
    /// 指纹：比的是「变没变」，不是「是什么」——故用与 [`result_digest`] 同一把哈希。
    pub fn digest(&self) -> u64 {
        result_digest(&format!(
            "{}\u{1}{}\u{1}{}",
            self.proposals, self.tasks, self.cursors
        ))
    }
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
    /// 连续若干次调用都**没有推动任何落库状态**（决策 310，判据③）。
    Stalled { times: usize },
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
            Loop::Stalled { times } => {
                format!("连着 {times} 次调用都没推动任何落库状态（提议数 / 任务状态 / 游标都没动）")
            }
        }
    }

    /// 这一条判据的动作是**提醒级、不收口**吗（决策 310）。
    ///
    /// 只有判据③ 是。理由：它比另两条**更容易误伤**——「在推进」对开放型问题（没有目标任务）
    /// 来说是三项读数全无变化，而那种轮本来就可以查很久。提醒级正是为了控住这类误伤：
    /// 提醒一次让人知道「它在原地转」，而**收口的权力仍留在 293 原两条判据手里**。
    pub fn is_remind_only(&self) -> bool {
        matches!(self, Loop::Stalled { .. })
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
    // ③ 落库状态无变化（决策 310，**显式修订 293**：两条判据 → 三条）：从尾部往前数
    //    `state_digest` 与最后一条相同的游程。**判据 ② 接不住这次这件事**——它看的是结果
    //    字节指纹，而 2026-09-27 那 167 次调用每次读数的字节都不同（日志偏移、`updated_at`、
    //    sqlite 行），于是永远「有新东西」，0 次命中。真正「没进展」的事实是「2.5 小时任务
    //    状态一字未动、0 条提议」，那就是这里的 `state_digest`。
    //
    //    放在 ② 之后：同参重复最确定、无进展次之，状态没动最宽——先报最确定的那个，
    //    免得一条更宽的判据把它盖住。
    if let Some(last) = calls.last() {
        let unmoved = calls
            .iter()
            .rev()
            .take_while(|c| c.state_digest == last.state_digest)
            .count();
        if unmoved >= STALLED_STATE_LIMIT {
            return Some(Loop::Stalled { times: unmoved });
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
            // 既有那几条判据（①②）不看落库状态，故这里给一个恒定值——它们要钉的是
            // 「结果/参数」那一面；判据③ 自己的用例用 `call_at_state` 显式喂状态。
            state_digest: 0,
        }
    }

    /// 带落库状态的一次调用（判据③ 的用例用）。
    fn call_at_state(tool: &str, args: &str, result: &str, state: &str) -> CallRecord {
        CallRecord {
            tool: tool.into(),
            arguments: args.into(),
            result_digest: result_digest(result),
            state_digest: result_digest(state),
        }
    }

    #[test]
    fn constants_are_pinned() {
        // 决策 224 / 256 姿态：阈值常量起步、不做配置项。真实台账的标定见模块头
        // （359 次调用最长连续同参游程 = 1）——改这里的值就是有意识地改门槛。
        assert_eq!(REPEAT_LIMIT, 3);
        assert_eq!(NO_PROGRESS_LIMIT, 5);
        // 判据③ 的标定见 STALLED_STATE_LIMIT 的文档（167 次调用 / 0 条提议 / 状态一字未动）。
        assert_eq!(STALLED_STATE_LIMIT, 12);
    }

    /// **判据③（决策 310）**：167 次调用每次读数的字节都不同（判据② 因此 0 次命中），
    /// 但落库状态**一次都没动**——它接住的正是这件事。
    #[test]
    fn an_unmoving_ledger_triggers_even_when_every_byte_is_new() {
        let mut calls = Vec::new();
        for i in 0..STALLED_STATE_LIMIT {
            calls.push(call_at_state(
                "read_file",
                &format!(r#"{{"path":"logs/serve.log","offset":{i}}}"#),
                // 每次都读到**不同的字节**（日志偏移 / updated_at / sqlite 行）——
                // 这正是判据② 永远看不到的那一面。
                &format!("日志第 {i} 段：updated_at=2026-09-27T0{}:00Z", i % 10),
                "s0",
            ));
        }
        assert_eq!(
            detect(&calls),
            Some(Loop::Stalled {
                times: STALLED_STATE_LIMIT
            })
        );
        assert!(
            detect(&calls).unwrap().is_remind_only(),
            "判据③ 只提醒、不收口"
        );
        // 差一次不触发：门槛是「一段持续的无变化」，不是「碰巧连着几次」。
        assert_eq!(detect(&calls[..STALLED_STATE_LIMIT - 1]), None);
    }

    /// **反向断言**：落库状态一动，计数立刻归零——正当的深查不被打扰。
    ///
    /// 尾巴那一串的长度算的是「从最后一次状态变化起有几次调用」——**变化那一次自己也算**
    /// （它就是新状态下的第一次调用）。故变化之后再攒 `LIMIT - 2` 次只到 11。
    #[test]
    fn any_ledger_change_resets_the_stalled_streak() {
        // 对照：22 次调用、状态**一次都没变** → 第 12 次就触发。
        let mut flat = Vec::new();
        for i in 0..STALLED_STATE_LIMIT - 1 {
            flat.push(call_at_state(
                "read_file",
                &format!(r#"{{"path":"f{i}"}}"#),
                &format!("第 {i} 份"),
                "s0",
            ));
        }
        flat.push(call_at_state(
            "propose",
            r#"{"kind":"resume"}"#,
            "提议已落库",
            "s0",
        ));
        for i in 0..STALLED_STATE_LIMIT - 2 {
            flat.push(call_at_state(
                "read_file",
                &format!(r#"{{"path":"g{i}"}}"#),
                &format!("另一份 {i}"),
                "s0",
            ));
        }
        // 报出来的是**整段**游程的长度（22 次都没动，就报 22）——判据要的是「一段持续的
        // 无变化」，长度本身就是给人看的证据。
        assert!(
            matches!(detect(&flat), Some(Loop::Stalled { .. })),
            "对照：一样多的调用、状态不变时它就该响：{:?}",
            detect(&flat)
        );

        // 同一串调用，只把中间那一次的落库状态改成「变过」→ 尾巴只剩 11 条，不响。
        let mut moved = flat.clone();
        moved[STALLED_STATE_LIMIT - 1].state_digest = result_digest("s1");
        assert_eq!(
            detect(&moved),
            None,
            "那次状态变化把连续计数归零了：变化之后只有 {} 条",
            STALLED_STATE_LIMIT - 1
        );
    }

    /// 开放型问题（没有目标任务）也适用：三项读数全无变化即算无进展——**提醒级**正是
    /// 为了控住这类误伤（决策 310 把这条写进裁决，不当意外）。
    #[test]
    fn an_open_ended_question_is_covered_too() {
        let calls: Vec<CallRecord> = (0..STALLED_STATE_LIMIT)
            .map(|i| {
                call_at_state(
                    "run_readonly",
                    &format!(r#"{{"command":"iostat -w {i}"}}"#),
                    &format!("第 {i} 次采样"),
                    "s0",
                )
            })
            .collect();
        let hit = detect(&calls).expect("本机性能这种开放型问题也适用");
        assert!(hit.is_remind_only(), "对开放型问题只提醒、不收口");
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
