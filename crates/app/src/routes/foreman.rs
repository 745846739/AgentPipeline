//! 对讲台端点（决策 182⑤ / 204，票 01 / 03）。
//!
//! **全部任务无关、项目无关**——这正是本特性的立身之本：值班长在任何时候
//! 都答得上话，包括首启的空 home（用户故事 11）。与任务端点的交集是值班长**自己去查**
//! 的台账读数（`read_task` / `read_conversation` 等，决策 207 的清单），不是路由依赖；
//! 提议执行另走 [`run_proposal_tool`] 见下。
//!
//! | 方法 | 路径 | 说明 |
//! |---|---|---|
//! | GET | `/foreman/sessions` | 未归档的班次列表（按最近活动倒序） |
//! | POST | `/foreman/sessions` | 新开一个班次 |
//! | PATCH | `/foreman/sessions/{id}` | 改名 |
//! | POST | `/foreman/sessions/{id}/archive` | 归档（从列表里收起来，不删行） |
//! | POST | `/foreman/sessions/{id}/cancel` | 停钮：请这一班正在跑的那一轮停下（票 09） |
//! | GET | `/foreman/session?session=<id>` | 某个班次的全部轮次 + 提议 + 合计 token |
//! | POST | `/foreman/messages` | 说一句话，得到一次回话 |
//! | GET | `/foreman/stream` | 订阅回话的逐字增量与提议事件 |
//! | GET | `/foreman/tools` | 全量工具清单的回执标签（`{name, label}`，不按档位滤） |
//! | GET | `/foreman/commands?session=<id>` | 该班次跑过的命令（含被拒的） |
//! | GET | `/foreman/proposals?session=<id>` | 该班次未决的提议 |
//! | POST | `/foreman/proposals/{id}/execute` | 按下确认钮：**走既有端点**执行这条提议 |
//! | POST | `/foreman/proposals/{id}/reject` | 拒绝这条提议（作废，不再可执行） |
//!
//! **改状态的键分两族下发，但执行只有既有端点这一条。** 流水线自身的写动作（resume / retry /
//! 拍板）仍由 `POST /tasks/{id}/resume` 那批端点在详情里下发（决策 101 / 182⑯⑰）；值班长侧
//! （决策 188 起能碰文件与系统接口）靠**提议**（决策 207）：模型提议 → 落库 → 人按下「执行」
//! → [`run_proposal_tool`] 走**既有的那条**端点（同一套校验、同一套闸门）。**这里没有第二条
//! 改状态的路**——绕过校验的捷径一旦存在，「LLM 的判断不直接接进状态机」那条接缝就换个形式
//! 又回来了。
//!
//! **一条长会话改成一排班次**（决策 204）：会话隔离的是对话上下文与页头读数，
//! 不是权限，也不是态势快照——「换会话 ≠ 换看板」。台账从此跨会话，会话只是容器。

use agentpipeline_core::pipeline::foreman::{
    situation_drift, situation_fingerprint, FOREMAN_AGENT_TYPE, FOREMAN_FAILED_TURN_MARK,
    FOREMAN_STAGE_KEY, FOREMAN_TOOL_SPECS, FOREMAN_WATCH_FAILED_TURN_MARK, FOREMAN_WATCH_MARK,
};
use agentpipeline_core::sse::SseEvent;
use agentpipeline_core::storage::foreman::{
    ForemanMessage, ForemanSession, NewForemanMessage, FOREMAN_SESSION_KIND_TALK,
    FOREMAN_SESSION_KIND_WATCH, SESSION_TITLE_MAX_CHARS,
};
use agentpipeline_core::storage::proposals::{ForemanProposal, ForemanProposalStatus};
use agentpipeline_core::storage::Store;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::Json;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::json;

use crate::state::{map_core_error, ApiError, ApiResult, AppState};
use crate::stream::SSE_KEEPALIVE_INTERVAL;

/// 单次读取的会话上限。
///
/// 一条班次里说的话看不到头是不现实的（一个班次就是一晚的台账），故这里给一个够看到
/// 本班次全部对话的上限，而不是分页——分页会把「滚上去看两小时前说的那句」变成一个
/// 要写代码的交互。跨班次翻找是**列表**的职责（`GET /foreman/sessions`）。
const SESSION_PAGE_LIMIT: usize = 500;

/// `GET /foreman/tools`：清单的**回执标签**（决策 247⑤）。
///
/// **全量、按清单顺序、不按档位滤**：回执标的是**历史**上的工具调用——昨天 `auto`
/// 今天改 `deny`，昨天的回执仍要能翻译（按当前档位滤会翻译不了它）。**只出 `{name, label}`**
/// ——description / parameters 前端用不上，interface 能少则少。
///
/// 清单本身是静态的，但**未接线照旧 503**：`/foreman/*` 下没有例外（这条规则由既有
/// 两组 503 用例与 testing.md §7 钉着）——给一条静态端点开「接线外可用」的特例，
/// 等于给「什么算对讲台的能力」造出第二份口径。
pub async fn tools(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    if state.foreman.is_none() {
        return Err(foreman_unwired());
    }
    Ok(Json(json!({
        "tools": FOREMAN_TOOL_SPECS
            .iter()
            .map(|s| json!({ "name": s.name, "label": s.label }))
            .collect::<Vec<_>>(),
    })))
}

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
        // 缺省落点按 `kind` 取各自的「最近」（决策 286 / 票 01）：值守入口不指定
        // id 时落最近活动的值守台账，人的对讲台落最近活动的人的班次——两边不再
        // 抢同一个「最近」。
        None => match params.kind.as_deref() {
            Some(FOREMAN_SESSION_KIND_WATCH) => store
                .latest_foreman_session_of_kind(FOREMAN_SESSION_KIND_WATCH)
                .await
                .map_err(map_core_error)?,
            _ => store
                .latest_foreman_session()
                .await
                .map_err(map_core_error)?,
        },
    };
    Ok(Json(session_payload(&state, &store, session).await?))
}

/// `GET /foreman/sessions`：未归档的班次，按最近活动倒序。
///
/// `?kind=`（决策 286 / 票 01）：`watch` 取值守台账的列表；**缺省只回人的班次**——
/// 对讲台的班次列表与值守的独立入口是两个列表，混在一起会让值守台账被当成一个
/// 可说话的班次（它是只读的一本账）。
pub async fn sessions(
    State(state): State<AppState>,
    Query(params): Query<SessionKindQuery>,
) -> ApiResult<impl IntoResponse> {
    if state.foreman.is_none() {
        return Err(foreman_unwired());
    }
    let kind = params.kind.as_deref().unwrap_or(FOREMAN_SESSION_KIND_TALK);
    if kind != FOREMAN_SESSION_KIND_TALK && kind != FOREMAN_SESSION_KIND_WATCH {
        return Err(ApiError::bad_request(
            "班次类型只有 talk（人的班次）与 watch（值守台账）",
        ));
    }
    let sessions = state
        .store
        .list_foreman_sessions(Some(kind))
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
    /// 缺省落点取哪一类（决策 286 / 票 01）：`watch` 落值守台账，其余落人的班次。
    /// 只在 `session` 未指定时参与解析；指定了 id 就以 id 为准。
    pub kind: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SessionKindQuery {
    /// 要列哪一类班次。缺省 talk（人的班次）。
    pub kind: Option<String>,
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

/// `POST /foreman/sessions/{id}/cancel`：**停钮**——请这一班正在跑的那一轮停下
/// （决策 294 / 票 09）。
///
/// 只管**人这一轮**：值守轮压根不登记停钮通道（裁决 10：它归开关），故这里返回 `false`，
/// 而那不是错误——「没有一轮在跑」本身就是答案，界面据此把那颗钮收回去。
///
/// **这不是硬中止**（与流水线的 `request_cancel` 同一句话，决策 226）：那一轮在可被打断的
/// 观察点收口（正在跑的那次模型调用 / 下一轮开头），已经烧掉的 token 照常落库，
/// **部分结论与它提的提议都留着**（决策 294 显式修订 233③）。收口是**协作**的，故
/// `cancelled: true` 说的是「请求已经送到」，不是「它已经停了」——停了这件事由那一轮自己
/// 落的那一行（`【已停】`）回答。
///
/// 为什么不校验班次是否存在：这个端点的答案与台账无关（在飞现场是**进程内**的登记，
/// 与 `foreman_turn_in_flight` 同一姿态）。查库只会给「班次已被清掉、但那一轮还在跑」
/// 这种真实现场添一条假的 404。
pub async fn cancel_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<impl IntoResponse> {
    if state.foreman.is_none() {
        return Err(foreman_unwired());
    }
    let cancelled = agentpipeline_core::pipeline::foreman::cancel_foreman_turn(&id);
    Ok(Json(json!({ "cancelled": cancelled })))
}

// ───────────────────────── 命令（决策 204④ / 206，票 03）─────────────────────────

/// `GET /foreman/commands?session=<id>`：该班次跑过的命令。
///
/// 为什么另开一条而不是复用 `GET /tasks/{id}/commands`：那个读法的归属列是**任务**
/// （它还带一句 `command.task_id != id` 的归属校验），而值班长的命令 `task_id` 是 NULL
/// ——它在那条路上永远查不到。两条读法是平行的，归属列恰好一个非空，故永不重叠。
///
/// **被拒的命令也在这张表里**（决策 179 的既有口径）：审计面要看得见「有过一次被拒的
/// 尝试」，否则策略在日志里完全不可见，只剩模型侧的一次报错。
pub async fn commands(
    State(state): State<AppState>,
    Query(params): Query<SessionQuery>,
) -> ApiResult<impl IntoResponse> {
    if state.foreman.is_none() {
        return Err(foreman_unwired());
    }
    let store = state.store.clone();
    let session = read_session(&store, params.session.as_deref()).await?;
    let Some(session) = session else {
        return Ok(Json(json!({
            "session": serde_json::Value::Null,
            "commands": [],
        })));
    };
    let commands = store
        .list_foreman_commands(&session.id)
        .await
        .map_err(map_core_error)?;
    Ok(Json(json!({
        "session": session_wire(&session),
        "commands": commands,
    })))
}

// ───────────────────────── 提议（决策 188 / 207，票 02）─────────────────────────

/// `GET /foreman/proposals?session=<id>`：该班次**未决**的提议。
///
/// 与 `GET /foreman/session` 的分工：那里给的是**时间线**（全部提议，含已决与已过期——
/// 过期的那一轮必须还在，决策 207），这里给的是**待办**（只有还要人按键的那些）。
/// 不分页、不排序参数：一个班次里的未决提议是人一只手数得过来的东西。
pub async fn proposals(
    State(state): State<AppState>,
    Query(params): Query<SessionQuery>,
) -> ApiResult<impl IntoResponse> {
    if state.foreman.is_none() {
        return Err(foreman_unwired());
    }
    let store = state.store.clone();
    let session = read_session(&store, params.session.as_deref()).await?;
    let Some(session) = session else {
        return Ok(Json(json!({
            "session": serde_json::Value::Null,
            "proposals": [],
        })));
    };
    let proposals = store
        .list_pending_foreman_proposals(&session.id)
        .await
        .map_err(map_core_error)?;
    Ok(Json(json!({
        "session": session_wire(&session),
        "proposals": proposals.iter().map(proposal_wire).collect::<Vec<_>>(),
    })))
}

/// `POST /foreman/proposals/{id}/execute`：按下确认钮。
///
/// **执行 = 走后端既有的那条路**（同一套校验、同一套闸门：依赖循环、worktree 准入、
/// 写入门、fail fast）。本函数只负责提议层的三件事：一次一按、过期、态势变化的拒执。
pub async fn execute_proposal(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<impl IntoResponse> {
    if state.foreman.is_none() {
        return Err(foreman_unwired());
    }
    let store = state.store.clone();
    let proposal = store
        .get_foreman_proposal(&id)
        .await
        .map_err(map_core_error)?
        .ok_or_else(|| proposal_not_found(&id))?;

    // ① 过期：标成 expired 再拒（决策 207）。**不删行**——那一轮留在时间线里，
    //    按钮变灰并说明原因就是从这里来的。
    if proposal.is_expired(store.now()) {
        let expired = store
            .expire_foreman_proposal(&id)
            .await
            .map_err(map_core_error)?;
        if let Some(expired) = &expired {
            announce_proposal(&state, expired);
        }
        return Err(ApiError::conflict(format!(
            "这条提议已经过期（有效期 {} 分钟）——当时的情况未必还成立，要做得重新提一次",
            agentpipeline_core::storage::proposals::FOREMAN_PROPOSAL_TTL_MINUTES
        )));
    }

    // ② 一次一按：原子占用。拿不到就说明有人先按了 / 正在执行 / 已经被处理过——
    //    这一条是「不可重放」的落点，不靠界面的禁用态（两台设备同时按也算数）。
    let Some(claimed) = store
        .claim_foreman_proposal(&id)
        .await
        .map_err(map_core_error)?
    else {
        return Err(ApiError::conflict("这条提议已经在执行，或已经被处理过了"));
    };

    // ③ 态势变化的拒执（决策 207）：提议成立时存了一份指纹，执行时再取一份。
    //    参数里没有 task_id 的提议没有「态势」可判（文件与命令是这一类）——那种情况下
    //    提议的成立与否由端点自己的校验回答。
    if let Some(before) = &claimed.situation {
        if let Some(task_id) = claimed.args.get("task_id").and_then(|v| v.as_str()) {
            let after = situation_fingerprint(&store, task_id)
                .await
                .map_err(map_core_error)?;
            if let Some(drift) = situation_drift(before, &after) {
                // 释放占用、**保持 pending**：人还可以按「拒绝」把它收掉（那是人的判断），
                // 而把它自动标成 expired 会把「它过期了」与「情况变了」两件事说成一件。
                store
                    .release_foreman_proposal(&id)
                    .await
                    .map_err(map_core_error)?;
                let reason = format!("现在的情况已经不是它当时说的那样：{drift}。要做得重新提一次");
                record_proposal_outcome(&store, &claimed, &format!("提议未执行：{reason}")).await?;
                return Err(ApiError::conflict(reason));
            }
        }
    }

    match run_proposal_tool(&state, &claimed).await {
        Ok(detail) => {
            let resolved = store
                .resolve_foreman_proposal(&id, ForemanProposalStatus::Executed)
                .await
                .map_err(map_core_error)?;
            let text = match &detail {
                Some(d) => format!("提议已执行：{}\n{d}", claimed.summary),
                None => format!("提议已执行：{}", claimed.summary),
            };
            let message = record_proposal_outcome(&store, &claimed, &text).await?;
            if let Some(resolved) = &resolved {
                announce_proposal(&state, resolved);
            }
            Ok(Json(json!({
                "proposal": resolved.as_ref().map(proposal_wire),
                "message": message.as_ref().map(message_wire),
            })))
        }
        Err(err) => {
            // 执行失败**不消耗提议**：参数过不了校验是模型的事，不是提议本身作废
            // （票 02 的验收：那种情况下 status **不**变成 executed）。失败也进时间线。
            store
                .release_foreman_proposal(&id)
                .await
                .map_err(map_core_error)?;
            record_proposal_outcome(&store, &claimed, &format!("提议执行失败：{}", err.message))
                .await?;
            Err(err)
        }
    }
}

/// `POST /foreman/proposals/{id}/reject`：拒绝这条提议。
///
/// 拒绝是**人的动作**，与执行一样要走占用（一次一按），也进时间线。拒绝之后
/// 这条提议不可再执行——它记录的是「值班长提过、值班经理没让做」。
pub async fn reject_proposal(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<impl IntoResponse> {
    if state.foreman.is_none() {
        return Err(foreman_unwired());
    }
    let store = state.store.clone();
    let proposal = store
        .get_foreman_proposal(&id)
        .await
        .map_err(map_core_error)?
        .ok_or_else(|| proposal_not_found(&id))?;
    if !proposal.status.is_open() {
        return Err(ApiError::conflict(format!(
            "这条提议已经{}了",
            proposal_status_label(proposal.status)
        )));
    }
    let Some(claimed) = store
        .claim_foreman_proposal(&id)
        .await
        .map_err(map_core_error)?
    else {
        return Err(ApiError::conflict("这条提议已经在执行，或已经被处理过了"));
    };
    // 修复类的回收（决策 212③ / 票 12）：**保留分支、删 worktree**——分支是唯一的证据，
    // 与决策 207「过期只让按钮变灰、那一轮留在时间线」同一理由。
    if claimed.kind == agentpipeline_core::storage::proposals::ForemanProposalKind::Repair {
        if let Err(e) = discard_repair_worktree(&state, &claimed).await {
            // 回收失败不阻塞「拒绝」这件事本身：提议的状态是人的判断，回收是善后。
            tracing::warn!(proposal = %id, "拒绝修复提议时回收 worktree 失败：{}", e.message);
        }
    }
    let resolved = store
        .resolve_foreman_proposal(&id, ForemanProposalStatus::Rejected)
        .await
        .map_err(map_core_error)?;
    let message = record_proposal_outcome(
        &store,
        &claimed,
        &format!("提议已拒绝：{}（没有执行任何动作）", claimed.summary),
    )
    .await?;
    if let Some(resolved) = &resolved {
        announce_proposal(&state, resolved);
    }
    Ok(Json(json!({
        "proposal": resolved.as_ref().map(proposal_wire),
        "message": message.as_ref().map(message_wire),
    })))
}

/// 提议的**执行接缝**（决策 207）：按提议里的 `(工具, 参数)` 走既有那条路。
///
/// 分派是**按工具名**做的，而每个名字对应的是「走到哪条既有的路」。分法见决策 255——
/// **判据是「这颗动作有没有一颗对应的界面按钮」**：
/// - **有按钮的三族**（`task` / `config` / `skills`）：**直接调那个端点的处理器函数**。
///   不是发一次 in-process HTTP——那会重新过一遍跨源与配对层，而这两层判的是「谁在门外」，
///   这次调用已经在门内（值班经理在本机界面上按下了确认钮）。直接调 handler 也正是
///   「参数与按钮同形」那条要求的执行机制（票 05）。
/// - **没按钮的四件**（env 三件 / `unstick` / `repair` / `service`）：走
///   [`agentpipeline_core::pipeline::foreman_actions`]——与界面无关，故住在 core。
///
/// 两条路都**不新增第二条改状态的实现**：绕过校验的捷径一旦存在，「LLM 的判断不直接接进
/// 状态机」那条接缝就换个形式又回来了。
///
/// `Ok(Some(细节))` 是成功（细节进时间线），`Err` 是失败——失败一律走既有端点的错误，不在这里
/// 翻译成别的东西（`Err` 的报文与界面上直接点那个按钮时看到的是**同一句**）。
///
/// **分派器只此一处**（决策 247 的语汇单点）：四族搬进 core 之后，六条臂仍在这张表里，
/// 只是其中四条各变成一行 core 调用——不设「一个入口内部分派」，那会让工具名语汇出现第二份副本。
async fn run_proposal_tool(
    state: &AppState,
    proposal: &ForemanProposal,
) -> Result<Option<String>, ApiError> {
    use agentpipeline_core::pipeline::foreman_actions;
    let sse: std::sync::Arc<dyn agentpipeline_core::sse::SseSink> = state.sse.clone();
    match proposal.tool.as_str() {
        "write_file" | "edit_file" | "run_command" => {
            foreman_actions::run_env(&state.store, &state.settings, &state.home, sse, proposal)
                .await
                .map_err(map_core_error)
        }
        "task" => run_task_tool(state, proposal).await,
        // 全局动作（决策 210⑧ / 票 09）：**永远只提议**，按下走恢复序列。
        "service" => foreman_actions::run_service(&state.store, proposal)
            .await
            .map_err(map_core_error),
        // 修复提议（决策 212① / 票 12）：执行的不是工具，是「合入一个分支」。
        "repair" => foreman_actions::run_repair(&state.store, proposal)
            .await
            .map_err(map_core_error),
        "config" => run_config_tool(state, proposal).await,
        "skills" => run_skills_tool(state, proposal).await,
        // 工具名对不上的提议是**真实可能**的（升级前落的、或模型报了一个不存在的名字）：
        // 这句话正是要说的事，不是占位。
        other => Err(ApiError::bad_request(format!(
            "这条提议的工具还没有接线：{other}"
        ))),
    }
}

// ──────────────────── 本服务写接口：四个领域各一族（票 05 / 09）────────────────────

/// `task` 族：建任务 / resume / retry / cancel / 拍板 / 合入。
///
/// 参数与界面上那颗按钮点下去时发的**同形**（票 05 的硬要求：不发明第二套参数语言），
/// 故这里只做一件翻译——把 args 搬进端点的 body 结构体。
async fn run_task_tool(
    state: &AppState,
    proposal: &ForemanProposal,
) -> Result<Option<String>, ApiError> {
    use crate::routes::tasks;
    use axum::extract::Path;

    let args = &proposal.args;
    let action = str_arg(args, "action")?;
    let state = state.clone();
    match action.as_str() {
        "create" => {
            let response = tasks::create(
                State(state),
                Json(tasks::CreateTaskBody {
                    project_id: str_arg(args, "project_id")?,
                    title: str_arg(args, "title")?,
                    description: opt_str(args, "description").unwrap_or_default(),
                    depends_on: args
                        .get("depends_on")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default(),
                    review_mode: opt_str(args, "review_mode"),
                    model_override: opt_str(args, "model_override"),
                }),
            )
            .await;
            endpoint_outcome(response).await
        }
        "resume" => {
            let task_id = str_arg(args, "task_id")?;
            let response = tasks::resume(
                State(state),
                Path(task_id),
                Json(tasks::ResumeBody {
                    action: str_arg(args, "resume_action")?,
                    cursor_id: opt_str(args, "cursor_id"),
                    target_stage: opt_str(args, "target_stage"),
                    target_node: opt_str(args, "target_node"),
                    input: opt_str(args, "input"),
                }),
            )
            .await;
            endpoint_outcome(response).await
        }
        "review" => {
            let task_id = str_arg(args, "task_id")?;
            let Some(approved) = args.get("approved").and_then(|v| v.as_bool()) else {
                return Err(ApiError::bad_request(
                    "task review 缺少 approved（true 通过 / false 打回）",
                ));
            };
            let response = tasks::review(
                State(state),
                Path(task_id),
                Json(tasks::ReviewBody {
                    approved,
                    comments: opt_str(args, "comments"),
                }),
            )
            .await;
            endpoint_outcome(response).await
        }
        "merge" => {
            let task_id = str_arg(args, "task_id")?;
            let response = tasks::merge_decision(
                State(state),
                Path(task_id),
                Json(tasks::MergeDecisionBody {
                    decision: str_arg(args, "decision")?,
                }),
            )
            .await;
            endpoint_outcome(response).await
        }
        "unstick" => {
            // `unstick` 不在端点里（它是修补动作面，不是界面上的按钮面）：归 core 的直接
            // 动作面（决策 255②）——判据落在**动作**粒度，不是族粒度。
            agentpipeline_core::pipeline::foreman_actions::run_unstick(
                &state.store,
                &state.settings,
                proposal,
            )
            .await
            .map_err(map_core_error)
        }
        "retry" => {
            let task_id = str_arg(args, "task_id")?;
            let response = tasks::retry(State(state), Path(task_id)).await;
            endpoint_outcome(response).await
        }
        // 手动按住 / 重跑本阶段（决策 276）：**非 pending 任务**的那两颗钮，与界面上
        // 那两颗同形——直接调 handler，参数语言只有一套（票 05 的硬要求）。
        "pause" => {
            let task_id = str_arg(args, "task_id")?;
            let response = tasks::pause(State(state), Path(task_id)).await;
            endpoint_outcome(response).await
        }
        "rerun" => {
            let task_id = str_arg(args, "task_id")?;
            let response = tasks::rerun(State(state), Path(task_id)).await;
            endpoint_outcome(response).await
        }
        "cancel" => {
            let task_id = str_arg(args, "task_id")?;
            let response = tasks::cancel(State(state), Path(task_id)).await;
            endpoint_outcome(response).await
        }
        other => Err(ApiError::bad_request(format!(
            "task 工具没有这个动作：{other}（可用：create / resume / retry / pause / rerun / \
             cancel / review / merge / unstick）"
        ))),
    }
}

/// `config` 族：改阶段配置（整条替换）/ 撤销覆盖。
async fn run_config_tool(
    state: &AppState,
    proposal: &ForemanProposal,
) -> Result<Option<String>, ApiError> {
    use crate::routes::stage_configs;
    use axum::extract::Path;

    let args = &proposal.args;
    let action = str_arg(args, "action")?;
    let stage = str_arg(args, "stage")?;
    let state = state.clone();
    match action.as_str() {
        "set" => {
            // 决策 236：这次 `config set` 会**抹掉旧的 `node_overrides`** 就拒，不静默抹掉。
            //
            // 校验点为什么在工具这一侧而不是 `PUT /stage-configs`：整条替换（「留空即清成
            // 默认」）是那个端点的**既有语义**，界面那份表单也总是把整行（含 node_overrides）
            // 带回来；而这个工具是**唯一会「看不见就改」**的入口——09-18 值班长明确拒提配置
            // 改动，理由就是「整条替换若漏带 node_overrides 会把节点级的 to-spec / grilling
            // 抹掉，而 read_stage_configs 不回显它」。故两处一起补：回显（`read_stage_configs`）
            // + 这条校验。翻掉那个端点的替换语义要另立一条（决策 236 的「明确不做」）。
            //
            // 空对象 `{}` 是**显式清空**（它与「没带」分得开），故不想带旧值的人仍有一条明路。
            // 显式 `null` 与「没带」按同一件事处理：两者在 `PutStageConfig` 里都反序列化成
            // `None`（整条替换下都是「清成默认」），故守卫必须一起罩住——只判 `is_none()`
            // 的话，写一个 `null` 就绕过去了。
            let carries_overrides = args
                .get("node_overrides_json")
                .is_some_and(|v| !v.is_null());
            if !carries_overrides {
                let overridden_nodes = state
                    .store
                    .get_stage_config(&stage)
                    .await
                    .map_err(map_core_error)?
                    .and_then(|c| c.node_overrides_json)
                    .and_then(|v| v.as_object().map(|o| o.len()))
                    .unwrap_or(0);
                if overridden_nodes > 0 {
                    return Err(ApiError::bad_request(format!(
                        "这次 config set 没带 node_overrides_json，而阶段 {stage} 现有配置里有 \
                         {overridden_nodes} 个节点级覆盖——照「留空即清成默认」的语义，改下去会把\
                         它们抹掉。用 read_stage_configs 看清现状，把 node_overrides_json 原样带回来；\
                         确实要清空就显式传 {{}}。"
                    )));
                }
            }
            let response = stage_configs::put(
                State(state),
                Path(stage),
                Json(stage_configs::PutStageConfig {
                    provider_id: opt_str(args, "provider_id"),
                    temperature: args.get("temperature").and_then(|v| v.as_f64()),
                    max_tokens: args
                        .get("max_tokens")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as u32),
                    persona_path: opt_str(args, "persona_path"),
                    persona_append: opt_str(args, "persona_append"),
                    tools_json: args.get("tools_json").cloned(),
                    skills_json: args.get("skills_json").cloned(),
                    idle_timeout_sec: args.get("idle_timeout_sec").and_then(|v| v.as_u64()),
                    max_duration_sec: args.get("max_duration_sec").and_then(|v| v.as_u64()),
                    node_overrides_json: args.get("node_overrides_json").cloned(),
                    env_mode: opt_str(args, "env_mode"),
                    // 轮数上限（决策 233① / 239）：只收正整数，越界由 `put` 那条路拒
                    // （报文与界面上写错时同一句）。
                    max_rounds: args.get("max_rounds").and_then(|v| v.as_i64()),
                    // token 预算（决策 292 / 票 07）：同一条路、同一道门。
                    watch_token_budget: args.get("watch_token_budget").and_then(|v| v.as_i64()),
                }),
            )
            .await;
            endpoint_outcome(response).await
        }
        "delete" => {
            let response = stage_configs::delete(State(state), Path(stage)).await;
            endpoint_outcome(response).await
        }
        other => Err(ApiError::bad_request(format!(
            "config 工具没有这个动作：{other}（可用：set / delete）"
        ))),
    }
}

/// `skills` 族：装（从本地目录）/ 卸。
///
/// **只能从本地目录装**：zip 那条路要的是原始字节，塞不进 args 的 JSON（base64 既涨体积又
/// 要一个新依赖）。值班长本来就能读文件系统，让它指一个目录是更自然的形态。
async fn run_skills_tool(
    state: &AppState,
    proposal: &ForemanProposal,
) -> Result<Option<String>, ApiError> {
    use crate::routes::skills;
    use axum::extract::Path;

    let args = &proposal.args;
    let action = str_arg(args, "action")?;
    let state = state.clone();
    match action.as_str() {
        "install" => {
            let response = skills::import_dir(
                State(state),
                Json(skills::ImportDirBody {
                    paths: vec![std::path::PathBuf::from(str_arg(args, "path")?)],
                    overwrite: args
                        .get("overwrite")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                }),
            )
            .await;
            // 批量端点的形态是「逐项结果」：一项失败时 HTTP 仍是 200，失败在 `results` 里。
            // 提议执行必须把这个区分出来，否则界面会说「已执行」而技能其实没装上——
            // 故这里读 `failed` 这个**字段**，不去匹配报文里的字样（报文一变就静默失效）。
            let value = endpoint_json(response).await?;
            if value.get("failed").and_then(|v| v.as_u64()).unwrap_or(0) > 0 {
                return Err(ApiError::bad_request(format!(
                    "技能没有装上：{}",
                    compact(&value)
                )));
            }
            Ok(Some(compact(&value)))
        }
        "delete" => {
            let name = str_arg(args, "name")?;
            let response = skills::uninstall(State(state), Path(name)).await;
            endpoint_outcome(response).await
        }
        other => Err(ApiError::bad_request(format!(
            "skills 工具没有这个动作：{other}（可用：install / delete）"
        ))),
    }
}

/// 把端点处理器的响应归一成「成功 / 失败 + 一句细节」。
///
/// 端点的错误**逐字带回**（状态码与报文都是端点自己的）：提议执行失败的原因，与在界面上
/// 直接点那颗按钮时看到的是同一个东西。这一条是「执行 = 走既有端点」的可观测形态。
async fn endpoint_outcome<R: IntoResponse>(
    response: ApiResult<R>,
) -> Result<Option<String>, ApiError> {
    Ok(Some(compact(&endpoint_json(response).await?)))
}

/// 同上，但把响应体原样交出来（`skills` 族要读 `failed` 这个字段，不能只看一句话）。
async fn endpoint_json<R: IntoResponse>(
    response: ApiResult<R>,
) -> Result<serde_json::Value, ApiError> {
    let response = match response {
        Ok(r) => r.into_response(),
        Err(e) => return Err(e),
    };
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .map_err(|e| ApiError::internal(format!("读取端点响应失败：{e}")))?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    if !status.is_success() {
        let message = value
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("端点拒绝了这次调用")
            .to_string();
        return Err(ApiError {
            status,
            message,
            detail: None,
            kind: None,
        });
    }
    Ok(value)
}

/// 端点回来的 JSON 压成一行（时间线里那一行要短：它是给人扫一眼的，不是给人读的）。
fn compact(value: &serde_json::Value) -> String {
    let text = value.to_string();
    if text.chars().count() <= 400 {
        return text;
    }
    format!("{}…", text.chars().take(400).collect::<String>())
}

fn str_arg(args: &serde_json::Value, key: &str) -> Result<String, ApiError> {
    args.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| ApiError::bad_request(format!("这条提议缺少参数 {key}")))
}

fn opt_str(args: &serde_json::Value, key: &str) -> Option<String> {
    args.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

/// 提议的结果落进时间线（决策 207：**成功失败都进**，不弹窗）。
///
/// 落成 [`agentpipeline_core::storage::foreman::FOREMAN_ROLE_SYSTEM`] 而不是助理轮：
/// 这是操作台记的账，不是值班长说的话——写成助理轮会让它下一轮读到「我已经做了」。
async fn record_proposal_outcome(
    store: &Store,
    proposal: &ForemanProposal,
    text: &str,
) -> ApiResult<Option<ForemanMessage>> {
    store
        .append_foreman_message(NewForemanMessage::system(&proposal.session_id, text))
        .await
        .map_err(map_core_error)?;
    // 回话行的 id / created_at 由落库产生，重取尾部一行（与 `send` 同一手法）。
    Ok(store
        .list_foreman_messages(&proposal.session_id, 1)
        .await
        .map_err(map_core_error)?
        .pop())
}

/// 提议事件推给 `/foreman/stream`（决策 207：单开一个只读事件，不混进对话增量）。
fn announce_proposal(state: &AppState, proposal: &ForemanProposal) {
    state.sse.publish(SseEvent::ForemanProposal {
        // 值班长的事件不带任务、也没有并行分支（与它的对话增量同一条口径）
        task_id: String::new(),
        branch: String::new(),
        session_id: proposal.session_id.clone(),
        proposal_id: proposal.id.clone(),
        tool: proposal.tool.clone(),
        status: proposal.status.as_str().to_string(),
        summary: proposal.summary.clone(),
        expires_at: proposal.expires_at.to_rfc3339(),
    });
}

/// 读端点用的班次解析：指定 id 就取它（含已归档，读得出来），否则取最近活动的。
///
/// 与 [`ForemanRunner`] 的 `resolve_session` **不同**：那条路要拒绝归档会话（往里说话会
/// 生成一段谁也看不见的记录），而读一条已归档班次的提议列表是完全正当的。
async fn read_session(
    store: &Store,
    session_id: Option<&str>,
) -> ApiResult<Option<ForemanSession>> {
    match session_id {
        Some(id) => store.get_foreman_session(id).await.map_err(map_core_error),
        None => store.latest_foreman_session().await.map_err(map_core_error),
    }
}

pub fn proposal_status_label(status: ForemanProposalStatus) -> &'static str {
    match status {
        ForemanProposalStatus::Pending => "待你按键",
        ForemanProposalStatus::Executed => "执行过",
        ForemanProposalStatus::Rejected => "被拒绝",
        ForemanProposalStatus::Expired => "过期",
    }
}

/// 一条提议 → 线上形态。
///
/// `status` / `expires_at` 原样出去：界面的按钮灰不灰**由前端按 `expires_at` 自己算**
/// （到点即灰，不必等后端把它标成 expired），而后端那份 status 是权威的最终态。
fn proposal_wire(p: &ForemanProposal) -> serde_json::Value {
    json!({
        "id": p.id,
        "session_id": p.session_id,
        "tool": p.tool,
        "args": p.args,
        "summary": p.summary,
        "status": p.status.as_str(),
        // 来路（决策 294 / 票 09）：它来自一轮被人按停的话——那一条的正文里有一句
        // 没说完的结论，按之前多看一眼。界面据此在卡片上多写一行（不另加钮）。
        "stopped_round": p.stopped_round,
        // 形态与载荷（票 12）：前端按 `kind` 决定渲染哪一块（diff + 闸门读数 vs 参数摘要）
        "kind": p.kind.as_str(),
        "payload": p.payload,
        "created_at": p.created_at.to_rfc3339(),
        "expires_at": p.expires_at.to_rfc3339(),
        "resolved_at": p.resolved_at.map(|t| t.to_rfc3339()),
    })
}

/// 拒绝 / 过期时回收修复的 worktree（**保留分支**）。
async fn discard_repair_worktree(
    state: &AppState,
    proposal: &ForemanProposal,
) -> Result<(), ApiError> {
    use agentpipeline_core::pipeline::repair::{finish_repair, RepairOutcome, RepairSession};

    let Some(payload) = &proposal.payload else {
        return Ok(());
    };
    let Ok(outcome) = serde_json::from_value::<RepairOutcome>(payload.clone()) else {
        return Ok(());
    };
    let Some(project_id) = proposal.args.get("project_id").and_then(|v| v.as_str()) else {
        return Ok(());
    };
    let Some(project) = state
        .store
        .get_project(project_id)
        .await
        .map_err(map_core_error)?
    else {
        return Ok(());
    };
    // 重建现场走 `RepairSession::from_outcome`（决策 255）：这条重建此前在三处各写了一遍。
    let session = RepairSession::from_outcome(&outcome, &proposal.session_id);
    finish_repair(std::path::Path::new(&project.local_path), &session, false)
        .await
        .map_err(map_core_error)
}

fn proposal_not_found(id: &str) -> ApiError {
    ApiError::not_found(format!("提议不存在：{id}"))
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
/// **一轮跑在独立任务里**（决策 223）：它与这次 HTTP 请求**不同生共死**。客户端断开
/// （本地超时 / 关页 / 换网 / 手机切走）只丢掉这一次的同步回包，掐不死这一轮——回话
/// 照旧落库，下一次重读台账就能看到它。
///
/// 此前 handler 与请求绑在一起：客户端一放弃，hyper 就把 handler 的 future 丢掉，
/// `say()` 的失败外框（`if let Err(error) = &result`）连执行的机会都没有——库里于是
/// 只剩一条孤立的用户行。2026-09-18 实测的两条消息正是这个形状：`user` 行两条、
/// 回话与失败留痕都是零，连「为什么没回话」都查不到。任务被 detach 之后，这一轮的
/// 成败都归它自己记账（`Err` 走 `record_failed_turn`，panic 走
/// `record_interrupted_turn`）。
///
/// **LLM 失败时值班经理说的那句话已经落库**（`ForemanRunner::say` 的第一步）。
/// 失败返回错误状态，前端据此渲染一条错误轮并**保留输入框内容**，让人改几个字重发
/// 而不是重打一遍（决策 182㉓）。
pub async fn send(
    State(state): State<AppState>,
    Json(body): Json<SendBody>,
) -> ApiResult<impl IntoResponse> {
    let runner = state.foreman.clone().ok_or_else(foreman_unwired)?;
    let who = runner.clone();
    let text = body.text.clone();
    let session_id = body.session_id.clone();
    let turn = match tokio::spawn(async move { who.say(session_id.as_deref(), &text).await }).await
    {
        Ok(inner) => inner.map_err(map_core_error)?,
        // panic 把 `say()` 的失败外框一起带走了，故这一条账只能在这里补——它正是
        // 「库里为什么什么都没有」的那一格（决策 223）。
        Err(join) => {
            tracing::error!(panic = join.is_panic(), "对讲台这一轮没跑完，回话没有落库");
            runner.record_interrupted_turn("内部错误").await;
            return Err(ApiError::internal(
                "这一轮没跑完（内部错误）：台账里已经记下这一次中断，重发一次通常能过去",
            ));
        }
    };
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
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(SSE_KEEPALIVE_INTERVAL)))
}

/// 未接线时的统一拒绝（决策 182：缺省 `None` 返回 503，不是 500）。
///
/// 503 而不是 500 是把责任方说清楚：服务是好的，是这个能力这次没被接上
/// （契约测试里不注入 LLM 替身时就会走到这里）。
///
/// 十一个端点**一样**回 503：它们是同一个能力的几面——一个「能读会话列表、发不出话」
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

/// 一个班次的读数：会话本身 + 该会话的轮次 + 该会话的提议 + 该会话的合计。
///
/// 四个东西一起给，是因为它们在界面上是同一屏的四个位置（chip 的标题、时间线、时间线里的
/// 确认钮、页头读数），分成几个请求只会让「切了班次但页头还显示上一班的花费」这种不一致
/// 有时间窗。
///
/// **提议是全量的**（含已执行 / 已拒绝 / 已过期）：决策 207 要求过期只让按钮变灰、
/// 那一轮留在时间线里（审计——值班长当时提议过什么必须可追溯）。前端把两条列表按时间
/// 并进同一条时间线。
///
/// `turn_in_flight` 是**第五个位置**（决策 260）：这一班此刻有没有一轮在跑。它不是台账里
/// 的一行（回话落库才算数），而是进程内登记的直接读数（`foreman_turn_in_flight`），
/// 界面刷新之后靠它重新接上「正在说话」那一轮——详见该函数的说明。
async fn session_payload(
    state: &AppState,
    store: &Store,
    session: Option<ForemanSession>,
) -> ApiResult<serde_json::Value> {
    let Some(session) = session else {
        return Ok(json!({
            "session": serde_json::Value::Null,
            "messages": [],
            "proposals": [],
            "total_tokens": 0,
            "total_calls": 0,
            "turn_in_flight": false,
            "foreman": foreman_identity(state),
        }));
    };
    let messages = store
        .list_foreman_messages(&session.id, SESSION_PAGE_LIMIT)
        .await
        .map_err(map_core_error)?;
    let proposals = store
        .list_foreman_proposals(
            &session.id,
            agentpipeline_core::storage::proposals::FOREMAN_PROPOSAL_LIST_LIMIT,
        )
        .await
        .map_err(map_core_error)?;
    let (total_tokens, total_calls) = store
        .foreman_session_totals(&session.id)
        .await
        .map_err(map_core_error)?;
    Ok(json!({
        "session": session_wire(&session),
        "messages": messages.iter().map(message_wire).collect::<Vec<_>>(),
        "proposals": proposals.iter().map(proposal_wire).collect::<Vec<_>>(),
        "total_tokens": total_tokens,
        "total_calls": total_calls,
        "turn_in_flight": agentpipeline_core::pipeline::foreman_turn_in_flight(&session.id),
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
///
/// `kind`（决策 286 / 票 01）：班次身份，前端据此把值守台账与人的班次分开渲染——
/// 值守账是只读的一本账（无输入坞、无发送态）。存量的旧行由迁移 0030 落成
/// `talk`（裁决 12：不回填），靠它照旧标对。
fn session_wire(s: &ForemanSession) -> serde_json::Value {
    json!({
        "id": s.id,
        "title": s.title,
        "kind": s.kind,
        "created_at": s.created_at.to_rfc3339(),
        "last_active_at": s.last_active_at.to_rfc3339(),
        "archived_at": s.archived_at.map(|t| t.to_rfc3339()),
    })
}

/// 一行会话 → 线上形态。
///
/// `briefing` / `traces` 原样带出去：审计要的是「它当时看到的是这份读数」「它翻了什么」，
/// 在后端重新摘要一遍只会让界面显示的与落库的不一致。
///
/// `attribution` 是**解析出来**的（决策 235 / 238）：结构块住在回话文本里，而解析只有一处
/// ——界面拿的是后端判定的结果，不自己从正文里抠（那会造出第二个判定点，两边迟早不一致）。
/// 未定位时给 `unlocated` 而不是编一个类别，`attribution_reason` 说清是哪一种未定位。
///
/// `kind` / `proactive` 同理（决策 252）：**「这一行是什么」由后端判定**，前端不拿正文前缀
/// 去猜。两个字段而不是一个枚举，是因为它们**正交**——「值班长自己醒来说的那一轮失败了」
/// 今天算成 `failed` 且 `proactive = false`，压成一个枚举会让这个组合从形状上不可能。
fn message_wire(m: &ForemanMessage) -> serde_json::Value {
    use agentpipeline_core::storage::foreman::{
        FOREMAN_ROLE_ASSISTANT, FOREMAN_ROLE_SYSTEM, FOREMAN_ROLE_USER,
    };
    let attribution = (m.role == FOREMAN_ROLE_ASSISTANT)
        .then(|| agentpipeline_core::pipeline::foreman::parse_attribution(&m.content));
    // 「这一行是谁说的」——四个取值只有这里是判定点（`role` 列是三个值的观测字段）。
    let kind = match m.role.as_str() {
        FOREMAN_ROLE_USER => "mine",
        // 两种失败账同一个 `kind`（都是「没跑起来的那一轮」），差别在 `proactive`：
        // 值守轮那一份由它自己的前缀认（决策 271）。
        FOREMAN_ROLE_SYSTEM
            if m.content.starts_with(FOREMAN_WATCH_FAILED_TURN_MARK)
                || m.content.starts_with(FOREMAN_FAILED_TURN_MARK) =>
        {
            "failed"
        }
        FOREMAN_ROLE_SYSTEM => "console",
        // 结构化选项提问（决策 265）：assistant 且带载荷 → `ask`。判定点与决策 252 的
        // 其余分支同一处——前端拿字段，不从正文里抠选项。
        FOREMAN_ROLE_ASSISTANT if m.ask_json.is_some() => "ask",
        // `assistant` 与**认不出的角色值**：落到值班长一侧。这是原样搬过来的口径
        // （`ForemanMessageRow::into_message` 明写「前端按 `=== "user"` 判定，不认识的值落到
        // 「值班长」一侧」）——本票只把判定点从界面挪到后端，不顺手改这一档的归属。
        _ => "fm",
    };
    // 「是不是它自己醒来的那一轮」——播报（助理轮 + 播报前缀，决策 209④）与**值守轮的失败账**
    // （`system` 行 + 值守失败前缀，决策 271）都算：两个前缀都由后端加，故判定点也在这边。
    //
    // 决策 252③ 说的「两个字段分开写，将来要显示时不必再改线」正是这一格：字段集没动，
    // 只是「自发的轮失败了」这一格终于有了真值（此前它恒为 `false`，因为派生把「自发的」
    // 写死在助理轮上）——名牌据此把「值守 · 没跑起来」与「发送失败」分开。
    //
    // 仍只在**后端写的前缀**上成立：人可能恰好在自己那句里引用那个标记，故用户行不看。
    let proactive = match m.role.as_str() {
        FOREMAN_ROLE_ASSISTANT => m.content.starts_with(FOREMAN_WATCH_MARK),
        FOREMAN_ROLE_SYSTEM => m.content.starts_with(FOREMAN_WATCH_FAILED_TURN_MARK),
        _ => false,
    };
    json!({
        "id": m.id,
        "session_id": m.session_id,
        "role": m.role,
        "content": m.content,
        "prompt_tokens": m.prompt_tokens,
        "completion_tokens": m.completion_tokens,
        "briefing": m.briefing_json,
        "traces": m.traces_json,
        // 这一轮的**步骤顺序**（决策 273）：与 `traces` / `thinking` 一样原样带出去，
        // 不在后端截断——界面按段序渲染「先想 → 再查 → 然后说」，要的就是落库那一份。
        "segments": m.segments_json,
        // 该轮的推理原文（决策 244）：**展示留痕**，界面把它收进一个折叠块。
        // 与 `traces` 一样原样带出去，不在后端截断——界面要显示的就是落库那一份。
        "thinking": m.thinking,
        "created_at": m.created_at.to_rfc3339(),
        // 只对助理轮解析：`system` 行是后端自己写的中断 / 失败账，里面不会有归因块。
        "attribution": attribution.as_ref().map(|a| a.wire()),
        "attribution_label": attribution.as_ref().and_then(|a| a.kind()).map(|k| k.label()),
        "attribution_reason": attribution.as_ref().and_then(|a| a.reason()),
        "kind": kind,
        "proactive": proactive,
        // 结构化选项提问的载荷（决策 265）：恒在场、没有就是 null——加性字段，
        // 老客户端解析不受影响；用户 / system 行恒 null（那一列只有 assistant 会填）。
        "ask": m.ask_json,
    })
}

#[cfg(test)]
mod tests {
    //! 分派器的静态守卫（决策 255⑦，票 foreman-actions 04）。
    //!
    //! 照 `frontend/src/lib/talkLayout.test.ts` 的先例：**读源文本 + 断言**。
    //! 单测只能证明新 module 对，证明不了**路由层没有又长回一份**——这两条守的就是后者。

    use agentpipeline_core::agent::tools::{is_env_write_tool, is_service_write_tool};
    use agentpipeline_core::pipeline::foreman::FOREMAN_TOOL_SPECS;

    fn source() -> String {
        let full = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/routes/foreman.rs"
        ))
        .expect("本文件可读");
        // **只扫生产部分**：本守卫自己带着那些「禁止出现」的字面量，全文扫会扫到自己。
        let cut = full
            .find("#[cfg(test)]")
            .expect("本测试模块之前还有生产代码");
        full[..cut].to_string()
    }

    fn dispatcher_body(src: &str) -> &str {
        let start = src
            .find("async fn run_proposal_tool")
            .expect("分派器还在这个文件里");
        let end = src
            .find("async fn run_task_tool")
            .expect("task 族接在分派器后面（提取锚点）");
        &src[start..end]
    }

    /// **会生成提议的每个工具名在分派器里都有一条臂。**
    ///
    /// 判据与执行点同源：清单（`FOREMAN_TOOL_SPECS`）× 写工具集（`ask` 档转提议 / 恒提议）
    /// 的交集就是「可能落成一条提议」的那些名字；少一条臂，那条提议按下就是
    /// 「还没有接线」，而它本该能执行——这正是决策 247 修的那类漂移的执行面版本。
    #[test]
    fn every_proposal_capable_tool_has_a_dispatcher_arm() {
        let src = source();
        let body = dispatcher_body(&src);
        let mut checked = 0usize;
        for spec in FOREMAN_TOOL_SPECS.iter() {
            if !(is_env_write_tool(spec.name) || is_service_write_tool(spec.name)) {
                continue;
            }
            assert!(
                body.contains(&format!("\"{}\"", spec.name)),
                "工具 {} 会生成提议，但分派器里没有它的臂",
                spec.name
            );
            checked += 1;
        }
        // 读数冻结（照决策 247「21 名顺序冻结」的姿势）：当前交集 = env 写 4
        //（write_file / edit_file / run_command / repair；`delete_file` 不在值班长清单里）
        // + service 写 4（task / config / skills / service）。数目变了先重算再改这里。
        assert_eq!(checked, 8, "交集读数变了：先核对清单与写工具集");
    }

    /// **直接动作面的实现不在路由层**（防它长回来）：git 链、unstick、恢复序列三样
    /// 都只经 `foreman_actions`，且四个函数真的被调到。
    #[test]
    fn the_moved_families_are_not_implemented_here_anymore() {
        let src = source();
        for gone in [
            "rebase_onto_with_auto_resolve",
            "merge_into_default_branch",
            "pipeline::unstick::unstick",
            "clear_executor_owners",
            "agentpipeline_core::git",
        ] {
            assert!(
                !src.contains(gone),
                "{gone} 回到了路由层——它属于 core 的直接动作面（决策 255）"
            );
        }
        for present in [
            "foreman_actions::run_env",
            "foreman_actions::run_service",
            "foreman_actions::run_repair",
            "foreman_actions::run_unstick",
        ] {
            assert!(src.contains(present), "四族要经 {present} 走");
        }
    }
}
