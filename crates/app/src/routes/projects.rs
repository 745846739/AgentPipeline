//! 项目端点（决策 24 / 29 / 61 / 78 / 101 / 130）。

use agentpipeline_core::git::Git;
use agentpipeline_core::pipeline::pseudo::PseudoStage;
use agentpipeline_core::storage::observability::{NewProjectRun, RunOutcome};
use agentpipeline_core::types::{Node, NodeStatus, Project, Stage};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::state::{map_core_error, ApiError, ApiResult, AppState};

#[derive(Debug, Deserialize)]
pub struct CreateProject {
    pub name: String,
    pub local_path: String,
    #[serde(default)]
    pub default_branch: Option<String>,
}

/// `GET /projects`
pub async fn list(State(state): State<AppState>) -> ApiResult<impl IntoResponse> {
    let projects = state.store.list_projects().await.map_err(map_core_error)?;
    Ok(Json(json!({ "projects": projects })))
}

/// `POST /projects`：**立即校验是否为 git 仓库**（决策 61）。
pub async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateProject>,
) -> ApiResult<impl IntoResponse> {
    let path = std::path::PathBuf::from(&body.local_path);
    if !path.is_dir() {
        return Err(ApiError::bad_request(format!(
            "本地路径不存在或不是目录：{}",
            body.local_path
        )));
    }
    if !Git.is_git_repo(&path).await {
        return Err(ApiError::bad_request(format!(
            "不是 git 仓库，拒绝创建项目：{}",
            body.local_path
        )));
    }
    if Git.unborn_head(&path).await.map_err(map_core_error)? {
        return Err(ApiError::bad_request(format!(
            "仓库尚无任何提交（unborn HEAD），无法纳入流水线：{}",
            body.local_path
        )));
    }

    let language = Git::detect_language(&path);
    let project = Project {
        id: ulid::Ulid::new().to_string(),
        name: body.name,
        local_path: path.display().to_string(),
        default_branch: match body.default_branch {
            Some(b) => b,
            None => Git
                .current_branch(&path)
                .await
                .map_err(map_core_error)?
                .unwrap_or_else(|| "main".to_string()),
        },
        // 六～七项探测全部由代码实现（决策 78 / 139）
        test_framework: Git::detect_test_framework(&path, language.as_deref()),
        lint_command: Git::detect_lint_command(&path, language.as_deref()),
        agents_md_path: Git::agents_md_path(&path).map(|p| p.display().to_string()),
        language,
        created_at: state.store.now(),
    };
    state
        .store
        .create_project(&project)
        .await
        .map_err(map_core_error)?;
    Ok((StatusCode::CREATED, Json(json!({ "project": project }))))
}

#[derive(Debug, Deserialize)]
pub struct PatchProject {
    pub name: Option<String>,
    pub default_branch: Option<String>,
    pub test_framework: Option<String>,
    pub lint_command: Option<String>,
}

/// `PATCH /projects/{id}`（决策 101）。
pub async fn patch(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PatchProject>,
) -> ApiResult<impl IntoResponse> {
    if state
        .store
        .get_project(&id)
        .await
        .map_err(map_core_error)?
        .is_none()
    {
        return Err(ApiError::not_found(format!("项目不存在：{id}")));
    }
    state
        .store
        .update_project(
            &id,
            body.name.as_deref(),
            body.default_branch.as_deref(),
            body.test_framework.as_deref(),
            body.lint_command.as_deref(),
        )
        .await
        .map_err(map_core_error)?;
    let project = state.store.get_project(&id).await.map_err(map_core_error)?;
    Ok(Json(json!({ "project": project })))
}

/// `DELETE /projects/{id}`：级联删除项目与其全部历史（决策 328）；
/// 有活跃任务时拒绝（决策 101）——那道闸是级联删除的联锁。
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<impl IntoResponse> {
    // 先读出仓库路径：行删掉之后就查不到了，而 worktree / 分支的回收要用它。
    let project = state.store.get_project(&id).await.map_err(map_core_error)?;
    let task_ids = state
        .store
        .delete_project(&id)
        .await
        .map_err(map_core_error)?;
    // 库已经落定，磁盘产物是**尽力而为**：失败只告警，不回滚已删的项目（§12.1 同一口径）。
    if let Some(project) = project {
        recycle_task_artifacts(&state, &project.local_path, &task_ids).await;
    }
    Ok(Json(json!({ "ok": true })))
}

/// 回收被删任务留在磁盘上的产物：worktree（含 git 登记）、分支、任务目录（§12.1）。
///
/// 幂等：路径不存在就跳过。任务行此刻已经删了，故这里按**约定路径**定位
/// （`Home::worktree_path` / `Home::task_dir`，与执行器建它们时用的是同一条约定），
/// 而不是回头读任务行。
async fn recycle_task_artifacts(state: &AppState, repo_path: &str, task_ids: &[String]) {
    let repo = std::path::Path::new(repo_path);
    for task_id in task_ids {
        let worktree = state.home.worktree_path(task_id);
        if worktree.exists() {
            if let Err(e) = Git.remove_worktree(repo, &worktree, true).await {
                tracing::warn!(task = %task_id, error = %e, "worktree 清理失败");
            }
        }
        let branch = agentpipeline_core::git::branch_name(task_id);
        if let Err(e) = Git.delete_branch(repo, &branch).await {
            tracing::warn!(task = %task_id, error = %e, "分支清理失败");
        }
        let task_dir = state.home.task_dir(task_id);
        if task_dir.exists() {
            if let Err(e) = std::fs::remove_dir_all(&task_dir) {
                tracing::warn!(task = %task_id, error = %e, "任务目录清理失败");
            }
        }
    }
    // 构建缓存回收（票 runner-offload/03 / B5）：项目删除时全部任务的构建活动都已结束。
    agentpipeline_core::prune::prune_build_cache(&state.home).await;
}

#[derive(Debug, Deserialize)]
pub struct AnalyzeRequest {
    pub project_id: String,
}

/// `POST /projects/analyze`（决策 130 ⑦）：**异步** 202 + 轮询。
pub async fn analyze(
    State(state): State<AppState>,
    Json(body): Json<AnalyzeRequest>,
) -> ApiResult<impl IntoResponse> {
    let project = state
        .store
        .get_project(&body.project_id)
        .await
        .map_err(map_core_error)?
        .ok_or_else(|| ApiError::not_found("项目不存在"))?;

    let analysis_id = state
        .store
        .create_analysis(&project.id)
        .await
        .map_err(map_core_error)?;

    // 探测是纯代码（决策 78）；摘要由 project_analysis 伪阶段补（决策 48 / 130⑦）
    let store = state.store.clone();
    let executor = state.executor.clone();
    let id = analysis_id.clone();
    let path = std::path::PathBuf::from(&project.local_path);
    tokio::spawn(async move {
        let language = Git::detect_language(&path);
        let facts = json!({
            "language": language,
            "test_framework": Git::detect_test_framework(&path, language.as_deref()),
            "lint_command": Git::detect_lint_command(&path, language.as_deref()),
            "agents_md_path": Git::agents_md_path(&path).map(|p| p.display().to_string()),
            "has_gitignore": Git::has_gitignore(&path),
            "default_branch": project.default_branch,
            "suspicious": [],
        });
        // 项目级伪阶段的独立观测行（票 10 / 决策 48 / 100）：无任务、无游标，
        // 以 project_id 归属；`agent_type = pseudo:project_analysis` 非 system，
        // 因此计入全局 total_calls（决策 130 ②）。
        // 落库失败**显式告警**（观测面缺失必须可见，不静默吞掉）。
        let run_id = match store
            .insert_project_run(&NewProjectRun {
                project_id: project.id.clone(),
                stage: Stage::Init,
                node: Node::Execute,
                attempt: 1,
                agent_type: PseudoStage::ProjectAnalysis.agent_type().to_string(),
            })
            .await
        {
            Ok(id) => Some(id),
            Err(e) => {
                tracing::warn!(
                    project = %project.id,
                    error = %e,
                    "project_analysis 观测 run 落库失败，本次分析缺少独立观测行"
                );
                None
            }
        };
        let result = match executor {
            Some(ex) => {
                let started = std::time::Instant::now();
                // run id 透下去：这次分析的请求台账要挂在上面（决策 329），否则那几行
                // 三个归属键全空——读不出是哪一次调用，删项目时也带不走。
                match ex.project_analysis(&project, facts.clone(), run_id).await {
                    Ok(merged) => {
                        // 会话行：摘要属于观测面，metadata 存完整合并结果（含 summary）
                        if let Some(run_id) = run_id {
                            let _ = store
                                .finish_run(
                                    run_id,
                                    &RunOutcome {
                                        status: Some(NodeStatus::Success),
                                        duration_ms: started.elapsed().as_millis() as u64,
                                        ..Default::default()
                                    },
                                )
                                .await;
                            let summary = merged
                                .get("summary")
                                .and_then(|s| s.as_str())
                                .unwrap_or_default()
                                .to_string();
                            let messages = json!([{ "role": "assistant", "content": summary }]);
                            let _ = store
                                .insert_project_conversation(
                                    &project.id,
                                    run_id,
                                    Stage::Init,
                                    Node::Execute,
                                    1,
                                    PseudoStage::ProjectAnalysis.agent_type(),
                                    &messages,
                                    None,
                                    Some(&merged),
                                    0,
                                    0,
                                    // 这一行是**拼出来的摘要行**（正文是合并结果里摘的
                                    // summary），不是真转录：没有思考留痕可带，NULL 即实情。
                                    None,
                                )
                                .await;
                        }
                        merged
                    }
                    Err(e) => {
                        // LLM 不可用：既有降级不回退——保留确定事实 + 记摘要错误；
                        // run 行按决策 100 收尾为失败（不落会话，与 task 级伪阶段一致）。
                        if let Some(run_id) = run_id {
                            let _ = store
                                .finish_run(
                                    run_id,
                                    &RunOutcome {
                                        status: Some(NodeStatus::Failed),
                                        duration_ms: started.elapsed().as_millis() as u64,
                                        error: Some(e.to_string()),
                                        ..Default::default()
                                    },
                                )
                                .await;
                        }
                        let mut degraded = facts;
                        degraded["summary_error"] = json!(e.to_string());
                        degraded
                    }
                }
            }
            // 无执行器 = 纯代码探测，没有 LLM 调用，不落 run（total_calls 不计）。
            None => facts,
        };
        let _ = store.finish_analysis(&id, Some(&result), None).await;
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(json!({ "analysis_id": analysis_id })),
    ))
}

/// `GET /projects/{id}/analysis`
pub async fn analysis(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<impl IntoResponse> {
    match state
        .store
        .latest_analysis(&id)
        .await
        .map_err(map_core_error)?
    {
        Some((analysis_id, status, result, error)) => Ok(Json(json!({
            "analysis_id": analysis_id,
            "status": status,
            "result": result,
            "error": error,
        }))),
        None => Err(ApiError::not_found("该项目尚无分析记录")),
    }
}
