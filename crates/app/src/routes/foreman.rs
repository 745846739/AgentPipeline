//! 对讲台端点（决策 182⑤，票 01 / 03）。
//!
//! 三个端点，**全部任务无关、项目无关**——这正是本特性的立身之本：值班长在任何时候
//! 都答得上话，包括首启的空 home（用户故事 11）。它们与任务端点的唯一交集是
//! `read_task` / `read_conversation` 两个只读工具，那是值班长自己去查，不是路由依赖。
//!
//! | 方法 | 路径 | 说明 |
//! |---|---|---|
//! | GET | `/foreman/session` | 本会话的全部轮次 + 合计 token |
//! | POST | `/foreman/messages` | 说一句话，得到一次回话 |
//! | GET | `/foreman/stream` | 订阅回话的逐字增量 |
//!
//! **写动作仍然只有任务端点有。** 这里没有任何改变流水线状态的入口——值班长只说话，
//! 动手的键由 `POST /tasks/{id}/resume` 那批端点在详情里下发（决策 101 / 182⑯⑰）。

use agentpipeline_core::storage::foreman::ForemanMessage;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, Sse};
use axum::response::IntoResponse;
use axum::Json;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::json;

use crate::state::{map_core_error, ApiError, ApiResult, AppState};

/// 单次读取的会话上限。
///
/// 一条长会话就是整晚的值班台账（out of scope 里明确不做多会话 / 会话列表），
/// 故这里给一个够看到当晚全部对话的上限，而不是分页——分页会把「滚上去看两小时前
/// 说的那句」变成一个要写代码的交互。
const SESSION_PAGE_LIMIT: usize = 500;

/// `GET /foreman/session`。
///
/// `total_tokens` 由**落库的 token 列求和**得出，不是另存一个计数器：计数器会与实际
/// 行数漂移（崩在写计数器之前就永久偏了），而求和永远等于台账里真实存在的东西。
///
/// 未接线时与另外两个端点**一样**回 503：读会话本身只需要库，但这个能力的三个入口
/// 是同一件事的三面——一个「能读历史、发不出话」的页面比一句「未接线」更难排查。
pub async fn session(State(state): State<AppState>) -> ApiResult<impl IntoResponse> {
    if state.foreman.is_none() {
        return Err(foreman_unwired());
    }
    let store = state.store.clone();
    let messages = store
        .list_foreman_messages(SESSION_PAGE_LIMIT)
        .await
        .map_err(map_core_error)?;
    let (total_tokens, total_calls) = store
        .foreman_session_totals()
        .await
        .map_err(map_core_error)?;
    Ok(Json(json!({
        "messages": messages.iter().map(message_wire).collect::<Vec<_>>(),
        "total_tokens": total_tokens,
        "total_calls": total_calls,
        "foreman": foreman_identity(&state),
    })))
}

#[derive(Debug, Deserialize)]
pub struct SendBody {
    pub text: String,
}

/// `POST /foreman/messages`。
///
/// **LLM 失败时值班经理说的那句话已经落库**（`ForemanRunner::say` 的第一步）。
/// 失败返回错误状态，前端据此渲染一条错误轮并**保留输入框内容**，让人改几个字重发
/// 而不是重打一遍（决策 182㉓）。
pub async fn send(
    State(state): State<AppState>,
    Json(body): Json<SendBody>,
) -> ApiResult<impl IntoResponse> {
    let runner = state.foreman.clone().ok_or_else(foreman_unwired)?;
    let turn = runner.say(&body.text).await.map_err(map_core_error)?;
    let store = state.store.clone();
    let (total_tokens, total_calls) = store
        .foreman_session_totals()
        .await
        .map_err(map_core_error)?;
    // 回话行的 id / created_at 由落库产生，重新取一次尾部比让 `say` 多返回两个字段干净
    // ——`ForemanTurn` 是领域层的一次回话，`id` 是存储层的概念，不该混进去。
    // 只取 1 条：这里要的是刚落库的那一行，不是整段会话。
    let last = store
        .list_foreman_messages(1)
        .await
        .map_err(map_core_error)?
        .pop();
    Ok(Json(json!({
        "message": last.as_ref().map(message_wire),
        "reply": turn.reply,
        "total_tokens": total_tokens,
        "total_calls": total_calls,
    })))
}

/// `GET /foreman/stream`：订阅值班长的回话增量。
///
/// 复用**同一个** `SseBus`（决策 182⑥）：工头事件与任务事件在同一条广播上，
/// 这里只按 [`SseEvent::is_foreman_event`] 过滤。既有任务级路由不受影响——
/// 它按 task id 精确匹配，而工头事件的 task id 是空串。
pub async fn stream(
    State(state): State<AppState>,
) -> ApiResult<Sse<impl futures::Stream<Item = Result<Event, std::convert::Infallible>>>> {
    if state.foreman.is_none() {
        return Err(foreman_unwired());
    }
    let receiver = state.sse.subscribe();
    let stream =
        tokio_stream::wrappers::BroadcastStream::new(receiver).filter_map(|item| async move {
            match item {
                Ok(event) if event.is_foreman_event() => {
                    Some(Ok(Event::default().event("message").data(event.to_json())))
                }
                _ => None,
            }
        });
    Ok(Sse::new(stream))
}

/// 未接线时的统一拒绝（决策 182：缺省 `None` 返回 503，不是 500）。
///
/// 503 而不是 500 是把责任方说清楚：服务是好的，是这个能力这次没被接上
/// （契约测试里不注入 LLM 替身时就会走到这里）。
fn foreman_unwired() -> ApiError {
    ApiError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        message: "对讲台未接线：本次运行没有注入值班长".into(),
        detail: None,
    }
}

/// 身份回执：前端据此确认「对面是谁」，也让人一眼看出这一版接的是哪个 provider。
///
/// 不含任何密钥——`provider_id` 只是个名字（决策 112 的姿态：密钥永不回显）。
fn foreman_identity(state: &AppState) -> serde_json::Value {
    json!({
        "agent_type": agentpipeline_core::pipeline::foreman::FOREMAN_AGENT_TYPE,
        "stage_key": agentpipeline_core::pipeline::foreman::FOREMAN_STAGE_KEY,
        "wired": state.foreman.is_some(),
    })
}

/// 一行会话 → 线上形态。
///
/// `briefing` / `traces` 原样带出去：审计要的是「它当时看到的是这份读数」「它翻了什么」，
/// 在后端重新摘要一遍只会让界面显示的与落库的不一致。
fn message_wire(m: &ForemanMessage) -> serde_json::Value {
    json!({
        "id": m.id,
        "role": m.role,
        "content": m.content,
        "prompt_tokens": m.prompt_tokens,
        "completion_tokens": m.completion_tokens,
        "briefing": m.briefing_json,
        "traces": m.traces_json,
        "created_at": m.created_at.to_rfc3339(),
    })
}
