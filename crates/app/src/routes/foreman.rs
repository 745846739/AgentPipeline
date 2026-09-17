//! 对讲台端点（决策 182⑤ / 204，票 01 / 03）。
//!
//! **全部任务无关、项目无关**——这正是本特性的立身之本：值班长在任何时候
//! 都答得上话，包括首启的空 home（用户故事 11）。它们与任务端点的唯一交集是
//! `read_task` / `read_conversation` 两个只读工具，那是值班长自己去查，不是路由依赖。
//!
//! | 方法 | 路径 | 说明 |
//! |---|---|---|
//! | GET | `/foreman/sessions` | 未归档的班次列表（按最近活动倒序） |
//! | POST | `/foreman/sessions` | 新开一个班次 |
//! | PATCH | `/foreman/sessions/{id}` | 改名 |
//! | POST | `/foreman/sessions/{id}/archive` | 归档（从列表里收起来，不删行） |
//! | GET | `/foreman/session?session=<id>` | 某个班次的全部轮次 + 该班次的合计 token |
//! | POST | `/foreman/messages` | 说一句话，得到一次回话 |
//! | GET | `/foreman/stream` | 订阅回话的逐字增量 |
//!
//! **写动作仍然只有任务端点有。** 这里没有任何改变流水线状态的入口——值班长只说话，
//! 动手的键由 `POST /tasks/{id}/resume` 那批端点在详情里下发（决策 101 / 182⑯⑰）。
//!
//! **一条长会话改成一排班次**（决策 204）：会话隔离的是对话上下文与页头读数，
//! 不是权限，也不是态势快照——「换会话 ≠ 换看板」。台账从此跨会话，会话只是容器。

use agentpipeline_core::pipeline::foreman::{FOREMAN_AGENT_TYPE, FOREMAN_STAGE_KEY};
use agentpipeline_core::storage::foreman::{
    ForemanMessage, ForemanSession, SESSION_TITLE_MAX_CHARS,
};
use agentpipeline_core::storage::Store;
use axum::extract::{Path, Query, State};
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
/// 一条班次里说的话看不到头是不现实的（一个班次就是一晚的台账），故这里给一个够看到
/// 本班次全部对话的上限，而不是分页——分页会把「滚上去看两小时前说的那句」变成一个
/// 要写代码的交互。跨班次翻找是**列表**的职责（`GET /foreman/sessions`）。
const SESSION_PAGE_LIMIT: usize = 500;

/// `GET /foreman/session?session=<id>`。
///
/// `total_tokens` 由**该会话落库的 token 列求和**得出（决策 204⑤），不是另存一个计数器：
/// 计数器会与实际行数漂移（崩在写计数器之前就永久偏了），而求和永远等于台账里真实存在的东西。
/// 按会话过滤之前，这个数其实是「自建库以来的累计值」——页头那句「本次会话 N tok」名不副实。
///
/// 不带 `session` 时落到最近活动的未归档班次（老客户端的读法不至于立刻断掉）；
/// 一个班次都没有时返回空列表与 `session: null`，前端据此显示空态并新开一个——
/// 读端点**不**顺手建行，写的东西留给写端点。
pub async fn session(
    State(state): State<AppState>,
    Query(params): Query<SessionQuery>,
) -> ApiResult<impl IntoResponse> {
    if state.foreman.is_none() {
        return Err(foreman_unwired());
    }
    let store = state.store.clone();
    let session = match params.session.as_deref() {
        Some(id) => store
            .get_foreman_session(id)
            .await
            .map_err(map_core_error)?,
        None => store
            .latest_foreman_session()
            .await
            .map_err(map_core_error)?,
    };
    Ok(Json(session_payload(&state, &store, session).await?))
}

/// `GET /foreman/sessions`：未归档的班次，按最近活动倒序。
pub async fn sessions(State(state): State<AppState>) -> ApiResult<impl IntoResponse> {
    if state.foreman.is_none() {
        return Err(foreman_unwired());
    }
    let sessions = state
        .store
        .list_foreman_sessions()
        .await
        .map_err(map_core_error)?;
    Ok(Json(json!({
        "sessions": sessions.iter().map(session_wire).collect::<Vec<_>>(),
    })))
}

#[derive(Debug, Deserialize)]
pub struct SessionQuery {
    /// 要看哪个班次。缺省 = 最近活动的未归档班次。
    pub session: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateSessionBody {
    /// 可选标题。缺省（或全空白）= 中性标题「新班次」，第一句话说出来时按它命名。
    #[serde(default)]
    pub title: Option<String>,
}

/// `POST /foreman/sessions`：新开一个班次。
///
/// 空班次是**合法状态**——它按定义就是这样开始的（「新建后是空会话」）。标题留空即中性标题，
/// 第一句话落进这个班次时自动按它命名（决策 204②）。
pub async fn create_session(
    State(state): State<AppState>,
    Json(body): Json<CreateSessionBody>,
) -> ApiResult<impl IntoResponse> {
    if state.foreman.is_none() {
        return Err(foreman_unwired());
    }
    let title = body.title.unwrap_or_default();
    if title.chars().count() > SESSION_TITLE_MAX_CHARS {
        return Err(ApiError::bad_request(format!(
            "班次标题最多 {SESSION_TITLE_MAX_CHARS} 个字"
        )));
    }
    let session = state
        .store
        .create_foreman_session(&title)
        .await
        .map_err(map_core_error)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "session": session_wire(&session) })),
    ))
}

#[derive(Debug, Deserialize)]
pub struct RenameSessionBody {
    pub title: String,
}

/// `PATCH /foreman/sessions/{id}`：改名。
pub async fn rename_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<RenameSessionBody>,
) -> ApiResult<impl IntoResponse> {
    if state.foreman.is_none() {
        return Err(foreman_unwired());
    }
    let title = body.title.trim();
    if title.is_empty() {
        return Err(ApiError::bad_request("班次标题不能为空"));
    }
    if title.chars().count() > SESSION_TITLE_MAX_CHARS {
        return Err(ApiError::bad_request(format!(
            "班次标题最多 {SESSION_TITLE_MAX_CHARS} 个字"
        )));
    }
    let session = state
        .store
        .rename_foreman_session(&id, title)
        .await
        .map_err(map_core_error)?
        .ok_or_else(|| session_not_found(&id))?;
    Ok(Json(json!({ "session": session_wire(&session) })))
}

/// `POST /foreman/sessions/{id}/archive`：归档（决策 204①：只归档不删除，也不做分叉）。
///
/// **归档不保护消息**：这个班次的话照旧吃 `conversation_retention_days` 的年龄清理
/// （决策 204⑦）。归档是「从列表里收起来」，不是永久保存。
pub async fn archive_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<impl IntoResponse> {
    if state.foreman.is_none() {
        return Err(foreman_unwired());
    }
    let session = state
        .store
        .archive_foreman_session(&id)
        .await
        .map_err(map_core_error)?
        .ok_or_else(|| session_not_found(&id))?;
    Ok(Json(json!({ "session": session_wire(&session) })))
}

#[derive(Debug, Deserialize)]
pub struct SendBody {
    pub text: String,
    /// 这句话落进哪个班次。缺省 = 最近活动的未归档班次（老客户端不至于立刻断掉）。
    #[serde(default)]
    pub session_id: Option<String>,
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
    let turn = runner
        .say(body.session_id.as_deref(), &body.text)
        .await
        .map_err(map_core_error)?;
    let store = state.store.clone();
    let (total_tokens, total_calls) = store
        .foreman_session_totals(&turn.session.id)
        .await
        .map_err(map_core_error)?;
    // 回话行的 id / created_at 由落库产生，重新取一次尾部比让 `say` 多返回两个字段干净
    // ——`ForemanTurn` 是领域层的一次回话，`id` 是存储层的概念，不该混进去。
    // 只取该会话的 1 条：这里要的是刚落库的那一行，不是整段会话。
    let last = store
        .list_foreman_messages(&turn.session.id, 1)
        .await
        .map_err(map_core_error)?
        .pop();
    Ok(Json(json!({
        "message": last.as_ref().map(message_wire),
        "reply": turn.reply,
        "session": session_wire(&turn.session),
        "total_tokens": total_tokens,
        "total_calls": total_calls,
    })))
}

/// `GET /foreman/stream`：订阅值班长的回话增量。
///
/// 复用**同一个** `SseBus`（决策 182⑥）：工头事件与任务事件在同一条广播上，
/// 这里只按 [`SseEvent::is_foreman_event`] 过滤。既有任务级路由不受影响——
/// 它按 task id 精确匹配，而工头事件的 task id 是空串。
///
/// **事件带会话身份**（决策 204⑥）：这条流不按会话分叉（服务端本来就分不出来，
/// 加路径解决不了问题），而是把 `session_id` 放进增量里，由前端丢弃不属于当前会话的那些
/// ——手机与电脑同时连着时，两台设备在两个班次里说话不会串台。
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
///
/// 七个端点**一样**回 503：它们是同一个能力的几面——一个「能读会话列表、发不出话」
/// 的页面比一句「未接线」更难排查。
fn foreman_unwired() -> ApiError {
    ApiError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        message: "对讲台未接线：本次运行没有注入值班长".into(),
        detail: None,
        kind: None,
    }
}

/// 会话不存在 → 404（`Error::Task` 在 `map_core_error` 里的既有映射）。
fn session_not_found(id: &str) -> ApiError {
    ApiError {
        status: StatusCode::NOT_FOUND,
        message: format!("班次不存在：{id}"),
        detail: None,
        kind: None,
    }
}

/// 一个班次的读数：会话本身 + 该会话的轮次 + 该会话的合计。
///
/// 三个东西一起给，是因为它们在界面上是同一屏的三个位置（chip 的标题、时间线、页头读数），
/// 分成三个请求只会让「切了班次但页头还显示上一班的花费」这种不一致有时间窗。
async fn session_payload(
    state: &AppState,
    store: &Store,
    session: Option<ForemanSession>,
) -> ApiResult<serde_json::Value> {
    let Some(session) = session else {
        return Ok(json!({
            "session": serde_json::Value::Null,
            "messages": [],
            "total_tokens": 0,
            "total_calls": 0,
            "foreman": foreman_identity(state),
        }));
    };
    let messages = store
        .list_foreman_messages(&session.id, SESSION_PAGE_LIMIT)
        .await
        .map_err(map_core_error)?;
    let (total_tokens, total_calls) = store
        .foreman_session_totals(&session.id)
        .await
        .map_err(map_core_error)?;
    Ok(json!({
        "session": session_wire(&session),
        "messages": messages.iter().map(message_wire).collect::<Vec<_>>(),
        "total_tokens": total_tokens,
        "total_calls": total_calls,
        "foreman": foreman_identity(state),
    }))
}

/// 身份回执：前端据此确认「对面是谁」，也让人一眼看出这一版接的是哪个 provider。
///
/// 不含任何密钥——`provider_id` 只是个名字（决策 112 的姿态：密钥永不回显）。
fn foreman_identity(state: &AppState) -> serde_json::Value {
    json!({
        "agent_type": FOREMAN_AGENT_TYPE,
        "stage_key": FOREMAN_STAGE_KEY,
        "wired": state.foreman.is_some(),
    })
}

/// 一个班次 → 线上形态。
fn session_wire(s: &ForemanSession) -> serde_json::Value {
    json!({
        "id": s.id,
        "title": s.title,
        "created_at": s.created_at.to_rfc3339(),
        "last_active_at": s.last_active_at.to_rfc3339(),
        "archived_at": s.archived_at.map(|t| t.to_rfc3339()),
    })
}

/// 一行会话 → 线上形态。
///
/// `briefing` / `traces` 原样带出去：审计要的是「它当时看到的是这份读数」「它翻了什么」，
/// 在后端重新摘要一遍只会让界面显示的与落库的不一致。
fn message_wire(m: &ForemanMessage) -> serde_json::Value {
    json!({
        "id": m.id,
        "session_id": m.session_id,
        "role": m.role,
        "content": m.content,
        "prompt_tokens": m.prompt_tokens,
        "completion_tokens": m.completion_tokens,
        "briefing": m.briefing_json,
        "traces": m.traces_json,
        "created_at": m.created_at.to_rfc3339(),
    })
}
