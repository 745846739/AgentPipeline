//! 事件形状的唯一出口（决策 249 立意，决策 352 修订宿主名）。
//!
//! 决策 249 把请求组装与调用切成深模块时，把「SSE 发射留守 executor」一并写下——
//! 意图是**事件形状只有一个主人**：`ToolEvent` / `NodeStarted` / `NodeFinished` 长什么样、
//! 身份怎么填、详情截多少，都只在这一个模块里定，别的模块（模型调用编排片、merge 状态机、
//! 留守核自身）经这里的函数发事件，不自建事件面。此前主人是 executor.rs，model_invoke 与
//! model_request 都得回头 import 编排壳（回边）；决策 352 只换主人不换规矩——事件形状的
//! 唯一出口在 `events`，executor / model_invoke / model_request 平依赖它。
//!
//! 明确不并入 `run_ledger`：run 台账（一笔 run 的开立/收口/读数）与事件形状（一行数据
//! 长什么样、走哪条 wire）是两个概念，合住只会造出新的混居。发射函数里的台账调用
//! （[`finish_run_with_sse`] 经 [`RunLedger`]）是**读数借用**——duration 由台账算一次，
//! 事件与 run 行拿同一个数——不是职责混居。
//!
//! 阶段产出常量 `OUTPUT_*`（§4.2 的产出定名）同住此处：它们是「这一阶段的产出叫什么」
//! 的**同一处**事实源，落库（`stage_output_metadata` / `get_stage_output`）与回读共用，
//! 写两处就会无声漂移。

use crate::sse::{SseEvent, SseSink, ToolPhase};
use crate::types::{NodeCursor, Task};
use crate::Result;

use super::run_ledger::RunLedger;

// ─────────────────────────────── 阶段产出类型定名（§4.2）───────────────────────────────

pub(crate) const OUTPUT_DESIGN_DOC: &str = "design_doc";
pub(crate) const OUTPUT_DEV_DOC: &str = "dev_doc";
pub(crate) const OUTPUT_TEST_SCENARIOS: &str = "test_scenarios";
pub(crate) const OUTPUT_REVIEW_REPORT: &str = "review_report";
pub(crate) const OUTPUT_TEST_REPORT: &str = "test_report";
pub(crate) const OUTPUT_CODE_CHANGES: &str = "code_changes";
pub(crate) const OUTPUT_SYNC_DECISION: &str = "sync_decision";
pub(crate) const OUTPUT_REVIEW_DIFF: &str = "review_diff";

/// 流水线节点的 agent 身份（`RunContext.agent_type` 与 `tool_event.agent_type` 的**同一处**
/// 事实源，决策 244）。
///
/// 两处各写一个字面量的话，「工具事件带身份」这条改动会在其中一处悄悄漂移，而漂移的表现
/// 恰好是**对讲台多出别人的工具调用**（或被判成别人）——一个不会报错的错误。
pub(crate) const PIPELINE_AGENT_TYPE: &str = "main";

/// 决策 123 的 `tool_event` 发射（start / end / error 三态共用一个出口）。
///
/// 身份串与会话**在这一个出口里填死**（决策 244）：执行体发的事件永远是流水线节点的，
/// 由调用点各传一次只会多两处能写错的地方——而写错的后果（对讲台混进流水线的工具调用，
/// 或反之）不会报错，只会显示错。值班长那边有它自己的出口
/// （`ForemanRunner::emit_tool_event`），一样的形状、不同的身份。
///
/// 决策 249 · 票 03：出口单点、**形状只此一处**（宿主随决策 352 迁至 [`super::events`]）——
/// 模型调用编排片经本函数发事件，不自建事件面。
#[allow(clippy::too_many_arguments)] // 事件出口那一族（详情两列 + 身份 + 相位都是这条 wire 的字段，收成结构体等于把 wire 形状抄第二遍）
pub(crate) fn emit_tool_event(
    sse: &dyn SseSink,
    task: &Task,
    cursor: &NodeCursor,
    run_id: i64,
    tool: &str,
    phase: ToolPhase,
    args_summary: &str,
    args: &str,
    result: Option<&str>,
) {
    sse.emit(SseEvent::ToolEvent {
        task_id: task.id.clone(),
        branch: cursor.branch.clone(),
        run_id,
        agent_type: PIPELINE_AGENT_TYPE.to_string(),
        // 流水线节点不挂会话（与 `RunContext.session_id` 同一口径）。
        session_id: String::new(),
        tool: tool.to_string(),
        phase,
        args_summary: args_summary.to_string(),
        // 详情的 12k 上限在这一个出口里压（决策 301）：调用点只管把原文递进来，
        // 截断写两处就会出现「界面一份、别处另一份」的无声缩水。
        args: crate::pipeline::foreman::truncate_tool_result(args),
        result: result.map(crate::pipeline::foreman::truncate_tool_result),
        // 流水线节点不挂在途台账行（`ledger_id` / `seq` 是值班长在途轮的去重基准，票 02）。
        ledger_id: None,
        seq: None,
    });
}

/// `NodeStarted` 的唯一出口（形状只此一处；留守核的 begin 与编排片的带链开立都走它）。
pub(crate) fn emit_node_started(
    sse: &dyn SseSink,
    task: &Task,
    cursor: &NodeCursor,
    attempt: u32,
    run_id: i64,
) {
    sse.emit(SseEvent::NodeStarted {
        task_id: task.id.clone(),
        branch: cursor.branch.clone(),
        stage: cursor.stage,
        node: cursor.node,
        attempt,
        run_id,
    });
}

/// 收口 + `NodeFinished` 的**唯一组合**（观测面跟编排走，决策 245 先例；宿主随决策 352 迁至 [`super::events`]）：
/// duration 由台账算（计时只算一次），事件与 run 行拿到同一个读数。留守核的
/// `finish_run` 包装与编排片的重试环都经本函数。
#[allow(clippy::too_many_arguments)] // 与被它取代的 `finish_run` 包装同宽——参数没变少，只是出口收成单点
pub(crate) async fn finish_run_with_sse(
    sse: &dyn SseSink,
    ledger: &RunLedger<'_>,
    run_id: i64,
    task: &Task,
    cursor: &NodeCursor,
    attempt: u32,
    failed: bool,
    started: chrono::DateTime<chrono::Utc>,
    error: Option<String>,
    tokens: &crate::pipeline::subagent::RunTokens,
) -> Result<()> {
    let duration_ms = ledger
        .finish(run_id, failed, started, error, tokens)
        .await?;
    sse.emit(SseEvent::NodeFinished {
        task_id: task.id.clone(),
        branch: cursor.branch.clone(),
        stage: cursor.stage,
        node: cursor.node,
        attempt,
        run_id,
        status: if failed {
            "failed".into()
        } else {
            "success".into()
        },
        duration_ms,
        prompt_tokens: tokens.prompt,
        completion_tokens: tokens.completion,
    });
    Ok(())
}
