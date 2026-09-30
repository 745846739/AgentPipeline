//! 重试裁定（决策 356 · 票 02）：两条「失败了接着怎么办」的表收成纯函数。
//!
//! ## 为什么有这个模块
//!
//! 同一个问题在流水线里有两张表，此前都长在调用点的 `if` / `else if` 链里，只能靠
//! 「跑一整条真流水线」才验得到：
//!
//! | 表 | 现场 | 判据来源 |
//! |---|---|---|
//! | 一次 attempt 失败 → 进不进下一轮 / 要不要追加错误 turn | `model_invoke::agent_node` 的轮循环 | 决策 278（本体）/ 295（超窗压不动）/ 298（传输类与配置类） |
//! | 节点连续超时 → 自动续接 / 空白重跑 / 挂起 | `scheduler::handle_timeout` 的三岔 | 决策 320（四段梯子）/ 33（干净重试退居降级档） |
//!
//! 「梯子若要单测，走纯函数裁定这条路」是决策 356 的原话：本模块把两张表的**判定**
//! 抽出来，调用点只留下与库 / 进程 / 游标打交道的那一半。
//!
//! ## 与 `foreman/turn_plan.rs` 的分工
//!
//! 值班长那一轮的组装与预算裁定在 [`crate::pipeline::foreman::TurnPlan`]（票 01）；
//! 这里是**流水线节点**的重试语义。两者都是「输入现场 → 下一动作」，但词汇不同
//! （这里的动作是续接 / 空白重跑 / 挂起，那里的是一次调用 / 一条注释），故不合并
//! ——合并会把两套词汇硬拉成一套，而它们服务的是两个不同的状态机。
//!
//! ## 这是一次搬家，不是改口径
//!
//! 两条表的**每一档**都逐字照搬原判据（含边界与顺序），调用点从 `if` / `else if` 链换成
//! 对纯函数的 `match`——没有任何一档的语义被顺手「整理」过。超时/重试族的既有用例
//! （`executor.rs`、`scheduler_tick.rs`、E2E-14）一个字没改就是证据。
//!
//! ## 判据按 `kind` 字段，不按报文字样
//!
//! 分类由构造点带上（[`crate::agent::providers::LlmErrorKind`]），本模块只读那枚字段
//! （决策 259 的同一口径）。谓词本体仍在 `agent/providers`（`is_transport` /
//! `is_wait_useless`）——那是**分类**的家；本模块只做**裁定**。

use crate::agent::providers::{is_transport, is_wait_useless};
use crate::Error;

/// 一次 attempt 失败之后，下一轮怎么走（决策 278 / 295 / 298）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptRetry {
    /// **不进下一轮**：等一等也没用的那一类——超窗（压过一次仍压不动）、鉴权、模型名、
    /// 额度。重试烧的是同一份坏配置的 N 倍 token，而该做的事（换模型、改配置、续费）
    /// 一个字都不会在重试里发生；指引由分类自带的人话走。
    GiveUp,
    /// 续接这一轮的转录重试，**不追加错误 turn**：传输类——请求根本没送到模型，
    /// 转录末尾是上一次成功的完好回合，`retry_prompt` 那句「已判废」是假话。
    ContinueAsIs,
    /// 续接这一轮的转录 + 一条错误 turn 重试：输出契约类，决策 278 的本体
    /// （转录末尾确实是被判废的那轮产出）。
    ContinueWithErrorTurn,
}

/// 这一次失败之后怎么走。
///
/// 顺序是承重的：先判「等一等也没用」（配置类与压不动的超窗），再判传输类。
/// 两支谓词的真相表互不重叠、互不漏（`providers::tests::retry_split_predicates_have_disjoint_truth_tables`）。
pub fn after_attempt_failure(error: &Error) -> AttemptRetry {
    if is_wait_useless(error) {
        return AttemptRetry::GiveUp;
    }
    if is_transport(error) {
        AttemptRetry::ContinueAsIs
    } else {
        AttemptRetry::ContinueWithErrorTurn
    }
}

/// 连续超时到第几次仍然自动续接（决策 320，**写死不配**）。
///
/// 与托管止损「满 2 次即停」（决策 210）同一量级；数字进决策日志，不做配置项。
pub const TIMEOUT_AUTO_CONTINUES_MAX: u32 = 2;

/// 节点连续超时 `streak` 次之后怎么办（决策 320）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeoutRetry {
    /// 1–2 次：**自动续接**上一轮转录（游标记续接原因、run 链落 `continued_from_run_id`）；
    /// 纯代码节点没有转录，这一档退化为空白重跑，但**计数照走**。
    AutoContinue,
    /// 第 3 次：降级**空白重跑**一次——续接救了两轮都没救回来，多半不是「丢了上下文」，
    /// 续第四遍只是继续烧钱。
    BlankRestart,
    /// 第 4 次起：挂起 `pending(timeout)` 交回人工。
    Pending,
}

/// 连续超时第 `streak` 次之后的动作。
///
/// `streak` 由台账算（`trailing_timeout_streak`：该节点尾部连续超时的 run 数，撞墙与
/// 尚未收场的那条都算），本函数只做分档——**在 `finish_run` 之后取**，刚判超时的这一条
/// 已经落库、算进序列里。
pub fn timeout_retry(streak: u32) -> TimeoutRetry {
    if streak <= TIMEOUT_AUTO_CONTINUES_MAX {
        TimeoutRetry::AutoContinue
    } else if streak == TIMEOUT_AUTO_CONTINUES_MAX + 1 {
        TimeoutRetry::BlankRestart
    } else {
        TimeoutRetry::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::providers::LlmErrorKind;

    fn classified(kind: LlmErrorKind) -> Error {
        Error::LlmClassified {
            kind: kind.as_str().to_string(),
            message: "人话".into(),
            raw: "原始诊断".into(),
        }
    }

    #[test]
    fn a_useless_wait_never_gets_another_round() {
        for kind in [
            LlmErrorKind::ContextWindow,
            LlmErrorKind::Auth,
            LlmErrorKind::ModelNotFound,
            LlmErrorKind::Quota,
        ] {
            assert_eq!(
                after_attempt_failure(&classified(kind)),
                AttemptRetry::GiveUp,
                "{kind:?} 重试救不回"
            );
        }
    }

    #[test]
    fn a_transport_failure_retries_without_an_error_turn() {
        assert_eq!(
            after_attempt_failure(&classified(LlmErrorKind::Network)),
            AttemptRetry::ContinueAsIs
        );
        assert_eq!(
            after_attempt_failure(&classified(LlmErrorKind::IdleTimeout)),
            AttemptRetry::ContinueAsIs
        );
        // 适配器层的未分类失败（HTTP 429 / 5xx、流读失败……）归传输类。
        assert_eq!(
            after_attempt_failure(&Error::Llm("connection reset".into())),
            AttemptRetry::ContinueAsIs
        );
    }

    #[test]
    fn an_output_contract_failure_retries_with_the_error_turn() {
        // 决策 278 的本体：转录末尾确实是被判废的那轮产出，那条错误 turn 是属实的
        // （实测里 run42 正是靠它带前文一次自我修正成功）。
        assert_eq!(
            after_attempt_failure(&Error::Validation("未找到结构化元数据".into())),
            AttemptRetry::ContinueWithErrorTurn
        );
        assert_eq!(
            after_attempt_failure(&Error::LlmClassified {
                kind: "model_empty_reply".into(),
                message: "值班长这一轮没有回话".into(),
                raw: "模型返回空内容".into(),
            }),
            AttemptRetry::ContinueWithErrorTurn
        );
        // 认不出类别的内部错误也走这一支：宁可多带一条错误 turn 重试，
        // 也不要凭一个未知类别就整轮放弃。
        assert_eq!(
            after_attempt_failure(&Error::Db(sqlx::Error::RowNotFound)),
            AttemptRetry::ContinueWithErrorTurn
        );
    }

    #[test]
    fn the_timeout_ladder_is_four_steps_and_pins_every_rung() {
        assert_eq!(
            timeout_retry(1),
            TimeoutRetry::AutoContinue,
            "第 1 次：续接"
        );
        assert_eq!(
            timeout_retry(2),
            TimeoutRetry::AutoContinue,
            "第 2 次：仍续接"
        );
        assert_eq!(
            timeout_retry(3),
            TimeoutRetry::BlankRestart,
            "第 3 次：降级空白重跑"
        );
        assert_eq!(
            timeout_retry(4),
            TimeoutRetry::Pending,
            "第 4 次：挂起交回人工"
        );
        assert_eq!(timeout_retry(9), TimeoutRetry::Pending, "再往后照旧挂起");
        assert_eq!(
            timeout_retry(0),
            TimeoutRetry::AutoContinue,
            "0 次不该出现，按续接兜底"
        );
    }
}
