//! 任务端点（§11.7；决策 27 / 35 / 49 / 56 / 91 / 98 / 101 / 105 / 116 / 119 / 125 / 129）。

use agentpipeline_core::actions::allowed_actions;
use agentpipeline_core::agent::tools::{CommandFinish, CommandRecorder, CommandStart};
use agentpipeline_core::git::Git;
use agentpipeline_core::storage::decisions::MergeDecision;
use agentpipeline_core::storage::tasks::{NewTask, TaskFilter};
use agentpipeline_core::types::{
    CommandSource, Node, NodeCursor, ReviewMode, Stage, Stewardship, TaskStatus,
};
use axum::extract::{Path, Query, State};
use axum::http::header::{self, HeaderMap};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::json;

use crate::state::{map_core_error, ApiError, ApiResult, AppState};
use crate::stream::SSE_KEEPALIVE_INTERVAL;

fn cursors_json(cursors: &[NodeCursor]) -> Vec<serde_json::Value> {
    cursors
        .iter()
        .map(|c| {
            json!({
                "cursor_id": c.cursor_id,
                "branch": c.branch,
                "stage": c.stage,
                "node": c.node,
                "status": c.status,
                "validate_attempts": c.validate_attempts,
                "skipped_to_join": c.skipped_to_join,
                "pending_reason": c.pending_reason,
            })
        })
        .collect()
}

// ─────────────────────────────── 创建 / 列表 / 详情 ───────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateTaskBody {
    pub project_id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub review_mode: Option<String>,
    #[serde(default)]
    pub model_override: Option<String>,
}

/// `POST /tasks`：一律以 `queued`（有依赖则 `waiting`）落库（决策 98）。
pub async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateTaskBody>,
) -> ApiResult<impl IntoResponse> {
    // 入口拒绝空白标题 / 描述（票 05①）。描述为空的任务，下游 architect-design 只能靠翻
    // 仓库猜范围——2026-10-01 事故现场：`description = ''` 且 `user-input.md` 只有「同意」，
    // 结果是 60 分钟、89 次只读调用、一个字没写。挡在入口，比在 60 分钟后拦下便宜得多。
    if body.title.trim().is_empty() {
        return Err(ApiError::bad_request("任务标题必填：不能为空或纯空白"));
    }
    if body.description.trim().is_empty() {
        return Err(ApiError::bad_request(
            "任务描述必填：不能为空或纯空白（下游 agent 靠它判断要实现什么）",
        ));
    }

    if state
        .store
        .get_project(&body.project_id)
        .await
        .map_err(map_core_error)?
        .is_none()
    {
        return Err(ApiError::bad_request(format!(
            "项目不存在：{}",
            body.project_id
        )));
    }

    // 未配置 provider 时创建任务返回明确错误（决策 56）
    let providers = state.store.load_providers().await.map_err(map_core_error)?;
    if !providers.iter().any(|p| p.enabled) {
        return Err(ApiError::bad_request(
            "尚未配置任何可用的 provider，请先在设置中添加",
        ));
    }

    // 循环依赖检测（决策 27）
    let task_id = ulid::Ulid::new().to_string();
    if state
        .store
        .would_create_cycle(&task_id, &body.depends_on)
        .await
        .map_err(map_core_error)?
    {
        return Err(ApiError::bad_request("依赖关系构成环路，拒绝创建"));
    }
    for dep in &body.depends_on {
        if state.store.get_task(dep).await.is_err() {
            return Err(ApiError::bad_request(format!("依赖任务不存在：{dep}")));
        }
    }

    let mut new_task = NewTask::new(task_id.clone(), body.title, body.project_id);
    new_task.description = body.description;
    new_task.depends_on = body.depends_on;
    new_task.review_mode = match body.review_mode.as_deref() {
        Some("human") => ReviewMode::Human,
        _ => ReviewMode::Agent,
    };
    new_task.model_override = body.model_override;

    let task = state
        .store
        .create_task(&new_task)
        .await
        .map_err(map_core_error)?;
    Ok((StatusCode::CREATED, Json(json!({ "task": task }))))
}

#[derive(Debug, Deserialize)]
pub struct TaskQuery {
    pub project_id: Option<String>,
    pub status: Option<String>,
    #[serde(default)]
    pub include_archived: bool,
}

/// `GET /tasks`：看板唯一数据源，每条带分支级摘要（决策 101）。
pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<TaskQuery>,
) -> ApiResult<impl IntoResponse> {
    let filter = TaskFilter {
        project_id: query.project_id,
        status: match query.status.as_deref() {
            Some(s) => Some(
                s.parse::<TaskStatus>()
                    .map_err(|e| ApiError::bad_request(e.to_string()))?,
            ),
            None => None,
        },
        include_archived: query.include_archived,
    };
    let tasks = state
        .store
        .list_tasks(&filter)
        .await
        .map_err(map_core_error)?;

    let mut out = Vec::new();
    for task in tasks {
        let cursors = state
            .store
            .load_live_cursors(&task.id)
            .await
            .map_err(map_core_error)?;
        let mut value =
            serde_json::to_value(&task).map_err(|e| ApiError::internal(e.to_string()))?;
        value["branches"] = json!(cursors_json(&cursors));
        value["blocks"] = json!(state
            .store
            .dependents_of(&task.id)
            .await
            .map_err(map_core_error)?);
        out.push(value);
    }
    Ok(Json(json!({ "tasks": out })))
}

/// ETag / 304（决策 361⑦ / 票 07）：回看**已完成**任务时不再重搬——内容没变就回零字节。
///
/// 版本键 = 「端点 + 参数 + 响应体」整体哈希。票面警告过的坑：会话摘要的 `status`
/// 是从台账 `list_runs` 贴进来的，run 收尾时行数与 MAX(id) 都没动——只锚会话行的
/// 版本键会漏掉这次变化，让跑着的任务拿到过期 304。对响应体取哈希则什么都躲不掉：
/// 台账一变，贴进来的字段就变，哈希跟着变。参数（`include_archived` /
/// `include_messages`）显式进键：不同视图各有各的 ETag，一张视图的 ETag 不替另一张背书。
///
/// `Cache-Control: no-cache` 是 304 生效的前提：它让浏览器**每次都来对账**，命中才免搬运。
/// 304 只撤 body，`ETag` 与缓存指令照旧带头。流式响应（SSE）不走这里。
fn etag_response(key: &str, headers: &HeaderMap, body: serde_json::Value) -> Response {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("{key}|{body}").as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    let etag = format!("\"v-{hex}\"");

    let matches = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            // If-None-Match 按 RFC 7232 用弱比较：`*` 匹配任何现存表示，
            // `W/` 前缀（至多一个）照接
            v == "*"
                || v.split(',').any(|c| {
                    let candidate = c.trim();
                    candidate.strip_prefix("W/").unwrap_or(candidate).trim() == etag
                })
        })
        .unwrap_or(false);

    let cache = [
        (header::ETAG, etag),
        (header::CACHE_CONTROL, "no-cache".to_string()),
    ];
    if matches {
        return (StatusCode::NOT_MODIFIED, cache).into_response();
    }
    (cache, Json(body)).into_response()
}

/// `GET /tasks/{id}`：状态 + 游标 + `allowed_actions`（决策 49 / 76）。
pub async fn detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<impl IntoResponse> {
    let task = state.store.get_task(&id).await.map_err(map_core_error)?;
    let cursors = state
        .store
        .load_live_cursors(&id)
        .await
        .map_err(map_core_error)?;
    let actions = state
        .store
        .allowed_actions_for_task(&id)
        .await
        .map_err(map_core_error)?;
    let depends_on = state
        .store
        .dependencies_of(&id)
        .await
        .map_err(map_core_error)?;

    let blocks = state
        .store
        .dependents_of(&id)
        .await
        .map_err(map_core_error)?;

    // 票 review-round-ledger 01 L3：评审轮间台账投影（第 N 轮 · 上轮 M 已改 k · 新增 j）。
    // 无评审产出 / 旧产出 → null，前端不渲染台账行。
    let review_ledger = state
        .store
        .review_ledger(&id)
        .await
        .map_err(map_core_error)?;

    Ok(etag_response(
        &format!("task-detail|{id}"),
        &headers,
        json!({
            "task": task,
            "cursors": cursors_json(&cursors),
            "allowed_actions": actions,
            "depends_on": depends_on,
            "blocks": blocks,
            "review_ledger": review_ledger,
        }),
    ))
}

// ─────────────────────── 任务级托管（决策 210① / 票 08）───────────────────────

#[derive(Debug, Deserialize)]
pub struct StewardshipBody {
    /// 打开 = `true`；关掉 = `false`。
    pub enabled: bool,
}

/// `POST /tasks/{id}/stewardship`：打开 / 关掉**这个任务**的托管。
///
/// 打开之后值班长可以对它免按键 `resume(continue)`——**恰好一个动作**（决策 210②）。
/// 关掉是**清空那一列**，不是写一个 `enabled: false`：留一个「关着的托管」会让
/// 「从来没开过」与「开过又关了」在库里长得一样，而这两件事在复盘时不是一回事。
///
/// 终态任务拒绝：托管天然自限（任务一 done 就再也用不上），给一个终态任务开托管只会
/// 让界面上多一个永远不会生效的开关。
pub async fn set_stewardship(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<StewardshipBody>,
) -> ApiResult<impl IntoResponse> {
    // 值班长没接线时这个开关没有意义（没人会用那份授权）——与对讲台其余端点同一姿态。
    if state.foreman.is_none() {
        return Err(ApiError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: "对讲台未接线：本次运行没有注入值班长，托管无人使用".into(),
            detail: None,
            kind: None,
        });
    }
    let task = state.store.get_task(&id).await.map_err(map_core_error)?;
    if task.status.is_terminal() {
        return Err(ApiError::bad_request(format!(
            "任务 {id} 已是终态（{}）：托管随任务自限，终态任务没有可托管的下一步",
            task.status.as_str()
        )));
    }
    let value = body
        .enabled
        .then(|| Stewardship::enabled_now(state.store.now()));
    state
        .store
        .set_stewardship(&id, value.as_ref())
        .await
        .map_err(map_core_error)?;
    let task = state.store.get_task(&id).await.map_err(map_core_error)?;
    Ok(Json(json!({ "ok": true, "task": task })))
}

// ─────────────────────────────── resume（决策 91 / 49 / §3）───────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ResumeBody {
    pub action: String,
    #[serde(default)]
    pub cursor_id: Option<String>,
    #[serde(default)]
    pub target_stage: Option<String>,
    #[serde(default)]
    pub target_node: Option<String>,
    #[serde(default)]
    pub input: Option<String>,
}

/// `POST /tasks/{id}/resume`
pub async fn resume(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ResumeBody>,
) -> ApiResult<impl IntoResponse> {
    // 状态机在 core 的 `pipeline::resume::apply_resume`（票 08 抽出来的**唯一实现**）：
    // 值班长的托管自动动作走的是同一份，两处逐字同源才不会漂移成
    // 「界面按得动、它按不动」。决策 245 给这条路径补了 SSE——`&Arc<SseBus>` 在实参位
    // 不会自动收窄成 `&Arc<dyn SseSink>`，先在这里立一个绑定。
    let sse: std::sync::Arc<dyn agentpipeline_core::sse::SseSink> = state.sse.clone();
    let applied = agentpipeline_core::pipeline::resume::apply_resume(
        &state.store,
        &state.settings,
        &state.resume_hook,
        &sse,
        &id,
        &agentpipeline_core::pipeline::resume::ResumeRequest {
            action: body.action.clone(),
            cursor_id: body.cursor_id.clone(),
            target_stage: body.target_stage.clone(),
            target_node: body.target_node.clone(),
            input: body.input.clone(),
        },
    )
    .await
    .map_err(map_core_error)?;

    Ok(Json(json!({
        "ok": true,
        "action": applied.action,
        "cursor_id": applied.cursor_id,
        "spawned": applied.spawned,
    })))
}

// ─────────────────────── 手动按住 / 重跑本阶段（决策 276）───────────────────────

/// `POST /tasks/{id}/pause`：把在跑的任务按住（位置保留，等人放行）。
///
/// 与 `resume` 的**分工**：这一颗管「停下来」，那颗管「放行」。暂停之后任务落在
/// `pending`（原因 `user_paused`），续跑/重跑两颗钮由 `allowed_actions` 下发
/// （`crate::actions` 的 `user_paused` 那一行）——与其余每一种 pending 同一套机制。
pub async fn pause(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<impl IntoResponse> {
    // `&Arc<SseBus>` 在实参位不会自动收窄成 `&Arc<dyn SseSink>`，先立一个绑定（同 `resume`）。
    let sse: std::sync::Arc<dyn agentpipeline_core::sse::SseSink> = state.sse.clone();
    let paused = agentpipeline_core::pipeline::pause::pause(&state.store, &sse, &id)
        .await
        .map_err(map_core_error)?;
    let note = if paused.notified {
        "已按暂停：在跑的那一轮收到中止请求，位置保留——按「续跑」从原处接着走"
    } else {
        "已按暂停：位置保留。进程里没有在跑的执行体可通知（它早已退出，或卡在不返回的调用里）\
         ——若它继续动，用 unstick 摘掉它"
    };
    Ok(Json(json!({
        "ok": true,
        "cursor_ids": paused.cursor_ids,
        "notified": paused.notified,
        "message": note,
    })))
}

/// `POST /tasks/{id}/rerun`：**从本阶段入口**重跑一遍（那一轮不算）。
///
/// 与 `retry` 的分工写在这里，因为两颗钮的名字像、范围差一整条任务：
/// 这一颗重跑**当前阶段**，`retry` 把整条任务打回 init。
pub async fn rerun(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let sse: std::sync::Arc<dyn agentpipeline_core::sse::SseSink> = state.sse.clone();
    let rerun =
        agentpipeline_core::pipeline::pause::rerun(&state.store, &state.resume_hook, &sse, &id)
            .await
            .map_err(map_core_error)?;
    let note = if rerun.notified {
        "已重跑本阶段：在跑的那一轮收到中止请求，游标回到本阶段入口，执行体已重派"
    } else {
        "已重跑本阶段：游标回到本阶段入口，执行体已重派（当时进程里没有在跑的执行体可通知）"
    };
    Ok(Json(json!({
        "ok": true,
        "cursors": rerun
            .moved
            .iter()
            .map(|m| json!({ "cursor_id": m.cursor_id, "stage": m.stage }))
            .collect::<Vec<_>>(),
        "notified": rerun.notified,
        "message": note,
    })))
}

// ─────────────────────────────── 旁路动作 ───────────────────────────────

/// `POST /tasks/{id}/retry`（决策 125 / 117）
pub async fn retry(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let task = state.store.get_task(&id).await.map_err(map_core_error)?;
    if !task.status.is_terminal() {
        return Err(ApiError::bad_request("只有终态任务可以重试"));
    }
    // 决策 125：worktree 里留有半成品，而 init.execute 的幂等策略是"已存在则复用"，
    // 不会清场——retry 必须显式 `git reset --hard {base_ref}` + `git clean -fdx`
    //（记 system 命令，run_id 可空，决策 99）。worktree 不存在则跳过（幂等）。
    if let Some(worktree) = task.worktree_path.clone() {
        let project = state
            .store
            .get_project(&task.project_id)
            .await
            .map_err(map_core_error)?
            .ok_or_else(|| ApiError::internal(format!("任务 {} 的项目不存在", task.project_id)))?;
        let wt = std::path::Path::new(&worktree);
        if wt.exists() {
            let base_ref = Git
                .base_ref(
                    std::path::Path::new(&project.local_path),
                    &project.default_branch,
                )
                .await
                .map_err(map_core_error)?;
            let cmd_id = state
                .store
                .record_start(CommandStart {
                    task_id: Some(id.clone()),
                    session_id: None,
                    run_id: None,
                    stage: Stage::Init,
                    node: Node::Execute,
                    source: CommandSource::System,
                    command: format!("git reset --hard {base_ref} && git clean -fdx"),
                    cwd: worktree.clone(),
                    // 这条 `git reset` 不经命令收口（它是 git2 调用的直接记录），故没有原串可言。
                    original_command: None,
                })
                .await
                .map_err(map_core_error)?;
            let outcome = Git.reset_hard_clean(wt, &base_ref).await;
            let finish = match &outcome {
                Ok(()) => CommandFinish {
                    exit_code: Some(0),
                    ..Default::default()
                },
                Err(e) => CommandFinish {
                    exit_code: Some(1),
                    stderr_preview: Some(e.to_string()),
                    ..Default::default()
                },
            };
            state
                .store
                .record_finish(cmd_id, finish)
                .await
                .map_err(map_core_error)?;
            outcome.map_err(|e| ApiError::internal(format!("worktree 重置失败：{e}")))?;
        }
    }
    state
        .store
        .reset_cursors_to_init(&id)
        .await
        .map_err(map_core_error)?;
    // §12.2 / 决策 113 同构：旧 attempt 的会话标记归档（不物理删除，历史仍可查），
    // 新执行落新会话行，查看器默认不再把历次 attempt 混在一起。
    state
        .store
        .archive_conversations(&id)
        .await
        .map_err(map_core_error)?;
    // 置回 queued 重新走准入（决策 117）
    state
        .store
        .set_task_status(&id, TaskStatus::Queued)
        .await
        .map_err(map_core_error)?;
    state
        .store
        .insert_transition(
            &id,
            NodeCursor::BRANCH_MAIN,
            None,
            (Stage::Init, agentpipeline_core::types::Node::Execute),
            agentpipeline_core::types::TransitionTrigger::UserResume,
            Some("用户重试"),
        )
        .await
        .map_err(map_core_error)?;
    Ok(Json(json!({ "ok": true, "status": "queued" })))
}

/// 决策 3 / §12.1：任务终态后回收 worktree 与分支。
///
/// 幂等——§12.1"worktree 已不存在时跳过删除，不报错"；清理失败只告警，
/// 不改变已落定的终态。
async fn cleanup_worktree_and_branch(state: &AppState, task_id: &str) {
    let Ok(task) = state.store.get_task(task_id).await else {
        return;
    };
    let Some(worktree) = task.worktree_path else {
        return;
    };
    let Ok(project) = state.store.get_project(&task.project_id).await else {
        return;
    };
    let Some(project) = project else { return };
    let repo = std::path::Path::new(&project.local_path);
    let wt = std::path::Path::new(&worktree);
    if wt.exists() {
        if let Err(e) = Git.remove_worktree(repo, wt, true).await {
            tracing::warn!(task = task_id, error = %e, "worktree 清理失败");
        }
    }
    // 构建缓存回收（票 runner-offload/03 / B5）。
    agentpipeline_core::prune::prune_build_cache(&state.home).await;
    let branch = agentpipeline_core::git::branch_name(task_id);
    if let Err(e) = Git.delete_branch(repo, &branch).await {
        tracing::warn!(task = task_id, error = %e, "分支清理失败");
    }
}

/// `POST /tasks/{id}/cancel`
pub async fn cancel(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let notified = state.store.cancel_task(&id).await.map_err(map_core_error)?;
    // §12.1 取消流程 ③④：强制清理 worktree 与分支
    cleanup_worktree_and_branch(&state, &id).await;
    // 通知依赖任务（§12.3）
    for dependent in &notified {
        state
            .sse
            .publish(agentpipeline_core::sse::SseEvent::Pending {
                task_id: dependent.clone(),
                branch: NodeCursor::BRANCH_MAIN.to_string(),
                cursor_id: grouped_cursor(&state, dependent).await?,
                reason: agentpipeline_core::types::PendingReason::new(
                    agentpipeline_core::types::PendingKind::DependencyFailed,
                    Stage::Init,
                    agentpipeline_core::types::Node::Execute,
                    format!("依赖任务 {id} 已取消"),
                ),
            });
    }
    state
        .sse
        .publish(agentpipeline_core::sse::SseEvent::TaskCancelled {
            task_id: id.clone(),
            branch: NodeCursor::BRANCH_MAIN.to_string(),
        });
    Ok(Json(json!({ "ok": true, "notified": notified })))
}

async fn grouped_cursor(state: &AppState, task_id: &str) -> ApiResult<String> {
    Ok(state
        .store
        .load_live_cursors(task_id)
        .await
        .map_err(map_core_error)?
        .first()
        .map(|c| c.cursor_id.clone())
        .unwrap_or_default())
}

/// `POST /tasks/{id}/archive`：软删除（决策 34）。
pub async fn archive(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let task = state.store.get_task(&id).await.map_err(map_core_error)?;
    if !task.status.is_terminal() {
        return Err(ApiError::bad_request("只有终态任务可以归档"));
    }
    // git.rs 的回收策略（决策 3）：取消 / 归档同样清理 worktree 与分支
    cleanup_worktree_and_branch(&state, &id).await;
    state
        .store
        .archive_task(&id)
        .await
        .map_err(map_core_error)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Debug, Deserialize)]
pub struct SplitBody {
    pub tasks: Vec<SplitTask>,
}

#[derive(Debug, Deserialize)]
pub struct SplitTask {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
}

/// `POST /tasks/{id}/split`（决策 105）：按用户给定方案创建 N 个新任务 + 原任务置 cancelled。
pub async fn split(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<SplitBody>,
) -> ApiResult<impl IntoResponse> {
    let original = state.store.get_task(&id).await.map_err(map_core_error)?;
    if body.tasks.is_empty() {
        return Err(ApiError::bad_request("split 至少需要一个新任务"));
    }
    let mut created = Vec::new();
    for spec in body.tasks {
        // 重提路径与 `POST /tasks` 同一关（票 05①）：拆分出来的子任务同样是任务，同样不许空描述
        if spec.title.trim().is_empty() || spec.description.trim().is_empty() {
            return Err(ApiError::bad_request(
                "拆分子任务的标题与描述都必填：不能为空或纯空白",
            ));
        }
        let new_id = ulid::Ulid::new().to_string();
        if state
            .store
            .would_create_cycle(&new_id, &spec.depends_on)
            .await
            .map_err(map_core_error)?
        {
            return Err(ApiError::bad_request("拆分子任务的依赖构成环路"));
        }
        let mut new_task = NewTask::new(new_id.clone(), spec.title, original.project_id.clone());
        new_task.description = spec.description;
        new_task.depends_on = spec.depends_on;
        new_task.review_mode = original.review_mode;
        let task = state
            .store
            .create_task(&new_task)
            .await
            .map_err(map_core_error)?;
        created.push(task.id);
    }
    state
        .store
        .mark_terminal(&id, TaskStatus::Cancelled)
        .await
        .map_err(map_core_error)?;
    Ok(Json(json!({ "ok": true, "created": created })))
}

#[derive(Debug, Deserialize)]
pub struct ModelOverrideBody {
    pub provider_id: String,
}

/// `POST /tasks/{id}/model-override`（决策 105 / 129）：只影响本任务，过白名单校验。
pub async fn model_override(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ModelOverrideBody>,
) -> ApiResult<impl IntoResponse> {
    let provider = state
        .store
        .get_provider(&body.provider_id)
        .await
        .map_err(map_core_error)?
        .ok_or_else(|| ApiError::bad_request("provider 不存在"))?;
    if !provider.enabled {
        return Err(ApiError::bad_request("provider 已被禁用"));
    }
    if !agentpipeline_core::config::SUPPORTED_ADAPTERS.contains(&provider.vendor.as_str()) {
        return Err(ApiError::bad_request(format!(
            "不支持的 vendor：{}",
            provider.vendor
        )));
    }
    // 写字段 + 若卡在 `context_overflow` 上则解除 pending（票 04：此前按了没反应）。
    let cleared = state
        .store
        .apply_model_override(&id, &body.provider_id)
        .await
        .map_err(map_core_error)?;
    Ok(Json(json!({
        "ok": true,
        "model_override": body.provider_id,
        // 解除掉的 pending 数（0 = 只是改了模型）。如实回报，界面不必猜。
        "resumed": cleared,
    })))
}

#[derive(Debug, Deserialize)]
pub struct ReviewBody {
    pub approved: bool,
    /// 用户评论：打回时随流转原因带给 develop（§12.5 "rejected → develop.execute（带用户评论）"）。
    #[serde(default)]
    pub comments: Option<String>,
}

/// `POST /tasks/{id}/review`（决策 2 / 124）：human 模式 approve → test / reject → develop。
pub async fn review(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ReviewBody>,
) -> ApiResult<impl IntoResponse> {
    let task = state.store.get_task(&id).await.map_err(map_core_error)?;
    if task.review_mode != ReviewMode::Human {
        return Err(ApiError::bad_request(
            "该任务不是人工评审模式（review_mode = agent）",
        ));
    }
    let cursor = state
        .store
        .apply_human_review(&id, body.approved, body.comments.as_deref())
        .await
        .map_err(map_core_error)?;
    (state.resume_hook)(&id);
    Ok(Json(json!({
        "ok": true,
        "approved": body.approved,
        "stage": cursor.stage,
        "node": cursor.node,
    })))
}

#[derive(Debug, Deserialize)]
pub struct MergeDecisionBody {
    pub decision: String,
    /// 决策 393：approve 时合入后是否 push 到远端（缺省 false；无 remote 自动跳过）。
    pub push: Option<bool>,
}

/// `POST /tasks/{id}/merge/decision`（决策 119）：单事务写 approval + 清 pending + 置游标。
pub async fn merge_decision(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<MergeDecisionBody>,
) -> ApiResult<impl IntoResponse> {
    let decision = MergeDecision::parse(&body.decision).map_err(map_core_error)?;
    let push = body.push.unwrap_or(false);
    let cursor = state
        .store
        .apply_merge_decision(&id, decision, push)
        .await
        .map_err(map_core_error)?;
    (state.resume_hook)(&id);
    Ok(Json(json!({
        "ok": true,
        "decision": body.decision,
        "push": push,
        "cursor": {
            "cursor_id": cursor.cursor_id,
            "stage": cursor.stage,
            "node": cursor.node,
        }
    })))
}

// ─────────────────────────────── 只读视图 ───────────────────────────────

/// `GET /tasks/{id}/stream`：**唯一** SSE 通道（决策 76）。
pub async fn stream(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Sse<impl futures::Stream<Item = Result<Event, std::convert::Infallible>>>> {
    let _task = state.store.get_task(&id).await.map_err(map_core_error)?;
    let receiver = state.sse.subscribe();
    let stream = tokio_stream::wrappers::BroadcastStream::new(receiver).filter_map(move |item| {
        let task_id = id.clone();
        async move {
            match item {
                Ok(event) if event.task_id() == task_id => {
                    Some(Ok(Event::default().event("message").data(event.to_json())))
                }
                _ => None,
            }
        }
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(SSE_KEEPALIVE_INTERVAL)))
}

/// `GET /tasks/{id}/flow`：流转时间线（历史查询，决策 76）。
pub async fn flow(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let transitions = state
        .store
        .list_transitions(&id)
        .await
        .map_err(map_core_error)?;
    let cursors = state
        .store
        .load_live_cursors(&id)
        .await
        .map_err(map_core_error)?;
    Ok(Json(json!({
        "transitions": transitions,
        "cursors": cursors_json(&cursors),
    })))
}

/// `GET /tasks/{id}/metrics`：token 汇总 + 阶段聚合（决策 130 ②）。
pub async fn metrics(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let task = state.store.get_task(&id).await.map_err(map_core_error)?;
    let runs = state.store.list_runs(&id).await.map_err(map_core_error)?;
    Ok(Json(json!({
        "total_tokens": agentpipeline_core::metrics::total_tokens(&runs),
        "total_calls": agentpipeline_core::metrics::total_calls(&runs),
        "stored_total_tokens": task.total_tokens,
        "stored_total_calls": task.total_calls,
        "stages": agentpipeline_core::metrics::stage_metrics(&runs)
            .into_iter()
            .map(|m| json!({
                "stage": m.stage,
                "total_runs": m.total_runs,
                "avg_duration_ms": m.avg_duration_ms,
                "retry_rate": m.retry_rate,
            }))
            .collect::<Vec<_>>(),
        "validate_first_pass_rate": agentpipeline_core::metrics::validate_first_pass_rate(&runs),
    })))
}

/// `GET /metrics`：全局统计。
pub async fn global_metrics(State(state): State<AppState>) -> ApiResult<impl IntoResponse> {
    let tasks = state
        .store
        .list_tasks(&TaskFilter {
            include_archived: true,
            ..Default::default()
        })
        .await
        .map_err(map_core_error)?;
    let statuses: Vec<TaskStatus> = tasks.iter().map(|t| t.status).collect();
    let aggregation = state
        .store
        .stage_aggregation()
        .await
        .map_err(map_core_error)?;
    // 全局 token / 调用数 / validate 首过率：复用 metrics 纯函数口径（决策 130② / 137），
    // 不在端点里另写一套 SQL 聚合（§12.4.1 的口径是契约）。
    let runs = state.store.all_runs().await.map_err(map_core_error)?;
    Ok(Json(json!({
        "tasks": tasks.len(),
        "success_rate": agentpipeline_core::metrics::success_rate(&statuses),
        "validate_first_pass_rate": agentpipeline_core::metrics::validate_first_pass_rate(&runs),
        "total_tokens": agentpipeline_core::metrics::total_tokens(&runs),
        "total_calls": agentpipeline_core::metrics::total_calls(&runs),
        "stage_aggregation": aggregation
            .into_iter()
            .map(|(stage, avg_duration, retry_rate, total)| json!({
                "stage": stage,
                "avg_duration_ms": avg_duration,
                "retry_rate": retry_rate,
                "total_runs": total,
            }))
            .collect::<Vec<_>>(),
        "escape_events": state
            .store
            .escape_events_by_stage()
            .await
            .map_err(map_core_error)?,
    })))
}

/// `GET /tasks/{id}/conversations`
#[derive(Debug, Deserialize)]
pub struct ConversationListQuery {
    /// 取回历史 attempt（含被重试归档的旧会话，§12.2）；默认只返回未归档。
    #[serde(default)]
    pub include_archived: bool,
    /// **批量取正文**（决策 361，票 03）：命中时一次返回该任务全部轮的完整会话
    /// （含 `messages_json`），元素与 `GET /conversations/{run_id}` 的单条读法**同形**
    /// ——同一份序列化，故两种读法内容等价。
    ///
    /// 加性参数：不改会话**列表**的既有分页立场（决策 312 给 messages 定的
    /// 「500 缺省 + `before_id` 向上游标」一个字没动），也不引入通用分页（决策 319 的
    /// 边界）。现场页签此前是 N+1——每轮一跳，浏览器 HTTP/1.1 单源约 6 并发，48 轮要排
    /// 八波；这里让「一个任务的全部轮」一次取回。
    #[serde(default)]
    pub include_messages: bool,
    /// **限定轮**（现场页签增量拉取）：逗号分隔的 run_id 列表，批量读法只取这些轮。
    ///
    /// 只在 `include_messages=true` 分支生效——增量拉取的动机是载荷（messages_json 是
    /// MB 级主体），摘要态本来就没有大列。缺省（不带参数）= 全部轮，老客户端不受影响。
    #[serde(default)]
    pub run_ids: Option<String>,
}

pub async fn conversations(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<ConversationListQuery>,
    headers: HeaderMap,
) -> ApiResult<impl IntoResponse> {
    // 版本键把两个参数都带上：批量与摘要、含归档与不含归档，各是各的 ETag——
    // 限定轮（增量拉取）请求的响应体是全集的真子集，版本键里不带它就会把子集 304 成全集。
    let etag_key = format!(
        "task-conversations|{id}|{}|{}|{}",
        query.include_archived,
        query.include_messages,
        query.run_ids.as_deref().unwrap_or("")
    );
    if query.include_messages {
        // 批量取正文：与单条读法共用 `list_conversations`，故「同一 run_id 两种读法给出
        // 相同会话」是**同一份实现**的直接结果，不是两条路要各自维护的约定。
        let run_ids = match query.run_ids.as_deref() {
            None => Vec::new(),
            Some(raw) => {
                let mut parsed = Vec::new();
                for part in raw.split(',') {
                    let part = part.trim();
                    if part.is_empty() {
                        continue;
                    }
                    parsed.push(part.parse::<i64>().map_err(|_| {
                        ApiError::bad_request(format!("run_ids 里的「{part}」不是 run id"))
                    })?);
                }
                parsed
            }
        };
        let conversations = state
            .store
            .list_conversations_for_runs(&id, query.include_archived, &run_ids)
            .await
            .map_err(map_core_error)?;
        return Ok(etag_response(
            &etag_key,
            &headers,
            json!({ "conversations": conversations }),
        ));
    }

    // run 状态不在会话行里（状态住台账），而药丸过滤要「状态」这一维（票 03）——
    // 按 run_id 从台账取一份贴进摘要。本机单用户量级，整表拉一次即可，不值得新端点。
    // **只在摘要分支取**：批量那条路（现场页签走的正是它）一个字节都用不上它，
    // 而它是一次全表读——别把它挂在本批要修的那条路上。
    let statuses: std::collections::HashMap<i64, String> = state
        .store
        .list_runs(&id)
        .await
        .map_err(map_core_error)?
        .into_iter()
        .map(|r| (r.id, r.status.as_str().to_string()))
        .collect();

    // 缺省（摘要态）：走**只取摘要列**的读法（决策 361，票 03）——此前它读回全文列
    // 并对每行做一次 `serde_json::from_str`，随即在这里丢掉。列表只要轮名与读数。
    let summaries = state
        .store
        .list_conversation_summaries(&id, query.include_archived)
        .await
        .map_err(map_core_error)?;
    // 列表只给摘要（§12.4.3）
    let items: Vec<serde_json::Value> = summaries
        .into_iter()
        .map(|c| {
            json!({
                "run_id": c.run_id,
                "stage": c.stage,
                "node": c.node,
                "attempt": c.attempt,
                "agent_type": c.agent_type,
                "parent_run_id": c.parent_run_id,
                "prompt_tokens": c.prompt_tokens,
                "completion_tokens": c.completion_tokens,
                "archived_at": c.archived_at,
                "status": statuses.get(&c.run_id).map(String::as_str).unwrap_or("unknown"),
            })
        })
        .collect();
    Ok(etag_response(
        &etag_key,
        &headers,
        json!({ "conversations": items }),
    ))
}

/// `GET /tasks/{id}/conversations/{run_id}`
pub async fn conversation(
    State(state): State<AppState>,
    Path((id, run_id)): Path<(String, i64)>,
) -> ApiResult<impl IntoResponse> {
    state
        .store
        .get_conversation(&id, run_id)
        .await
        .map_err(map_core_error)?
        .map(|c| Json(json!({ "conversation": c })))
        .ok_or_else(|| ApiError::not_found(format!("没有 run {run_id} 的会话")))
}

/// `GET /tasks/{id}/conversations/{run_id}/messages`（§12.4.3）：仅返回 messages 数组。
///
/// 按任务隔离：查询以 `task_id + run_id` 为条件，不属于该任务的 run 一律 404，
/// 不泄露其他任务数据（与 `command` 同姿态）。
pub async fn conversation_messages(
    State(state): State<AppState>,
    Path((id, run_id)): Path<(String, i64)>,
) -> ApiResult<impl IntoResponse> {
    let conversation = state
        .store
        .get_conversation(&id, run_id)
        .await
        .map_err(map_core_error)?
        .ok_or_else(|| ApiError::not_found(format!("任务 {id} 没有 run {run_id} 的会话")))?;
    Ok(Json(conversation.messages_json))
}

#[derive(Debug, Deserialize)]
pub struct CommandQuery {
    pub stage: Option<String>,
    pub node: Option<String>,
}

/// `GET /tasks/{id}/commands`
pub async fn commands(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<CommandQuery>,
) -> ApiResult<impl IntoResponse> {
    let stage = match query.stage {
        Some(s) => Some(s.parse::<Stage>().map_err(map_core_error)?),
        None => None,
    };
    let node = match query.node {
        Some(s) => Some(s.parse().map_err(map_core_error)?),
        None => None,
    };
    let commands = state
        .store
        .list_commands(&id, stage, node)
        .await
        .map_err(map_core_error)?;
    Ok(Json(json!({ "commands": commands })))
}

/// `GET /tasks/{id}/commands/{cmd_id}`（命令必须属于该任务——不允许跨任务读取）。
pub async fn command(
    State(state): State<AppState>,
    Path((id, cmd_id)): Path<(String, i64)>,
) -> ApiResult<impl IntoResponse> {
    let command = state
        .store
        .get_command(cmd_id)
        .await
        .map_err(map_core_error)?
        .ok_or_else(|| ApiError::not_found(format!("命令不存在：{cmd_id}")))?;
    // 归属校验按 `Some(id)` 比：值长在表里是 `Option`（迁移 0012 起命令也可以挂会话），
    // 但这条路由只认任务归属——值班长那些 `task_id` 为 NULL 的命令在 `/foreman/` 下读。
    if command.task_id.as_deref() != Some(id.as_str()) {
        return Err(ApiError::not_found(format!(
            "命令 {cmd_id} 不属于任务 {id}"
        )));
    }
    Ok(Json(json!({ "command": command })))
}

/// `GET /tasks/{id}/commands/{cmd_id}/output`：完整 stdout（从卸载文件读取）。
pub async fn command_output(
    State(state): State<AppState>,
    Path((id, cmd_id)): Path<(String, i64)>,
) -> ApiResult<impl IntoResponse> {
    let command = state
        .store
        .get_command(cmd_id)
        .await
        .map_err(map_core_error)?
        .ok_or_else(|| ApiError::not_found(format!("命令不存在：{cmd_id}")))?;
    // 归属校验按 `Some(id)` 比：值长在表里是 `Option`（迁移 0012 起命令也可以挂会话），
    // 但这条路由只认任务归属——值班长那些 `task_id` 为 NULL 的命令在 `/foreman/` 下读。
    if command.task_id.as_deref() != Some(id.as_str()) {
        return Err(ApiError::not_found(format!(
            "命令 {cmd_id} 不属于任务 {id}"
        )));
    }
    match command.stdout_path {
        Some(path) => {
            let content = std::fs::read_to_string(&path)
                .map_err(|e| ApiError::not_found(format!("卸载文件不可读：{e}")))?;
            Ok((StatusCode::OK, content))
        }
        // 未卸载：回落到 preview
        None => Ok((
            StatusCode::OK,
            command
                .stdout_preview
                .unwrap_or_else(|| "（该命令未卸载完整输出，只有 preview）".to_string()),
        )),
    }
}

/// `GET /tasks/{id}/files/{path}`：读取任务产出文件。
pub async fn file(
    State(state): State<AppState>,
    Path((id, rel_path)): Path<(String, String)>,
) -> ApiResult<impl IntoResponse> {
    let _task = state.store.get_task(&id).await.map_err(map_core_error)?;
    let path = state.home.task_dir(&id).join(&rel_path);
    // 只允许读任务目录内的文件（目录逃逸防护）
    let root = state.home.task_dir(&id);
    let resolved = path
        .canonicalize()
        .map_err(|_| ApiError::not_found(format!("文件不存在：{rel_path}")))?;
    let root_resolved = root
        .canonicalize()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    if !resolved.starts_with(&root_resolved) {
        return Err(ApiError::forbidden("路径越出任务目录"));
    }
    let content = std::fs::read_to_string(&resolved)
        .map_err(|e| ApiError::not_found(format!("文件不可读：{e}")))?;
    Ok((
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )],
        content,
    ))
}

/// 供外部断言使用：任务当前的动作集。
pub async fn actions_of(state: &AppState, task_id: &str) -> ApiResult<Vec<serde_json::Value>> {
    let task = state
        .store
        .get_task(task_id)
        .await
        .map_err(map_core_error)?;
    Ok(allowed_actions(
        &agentpipeline_core::types::PendingReason::new(
            agentpipeline_core::types::PendingKind::UserDecision,
            task.current_stage,
            task.current_node,
            "",
        ),
        None,
    )
    .into_iter()
    .map(|a| serde_json::to_value(a).unwrap_or_default())
    .collect())
}
