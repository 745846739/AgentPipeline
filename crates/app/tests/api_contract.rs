//! L3 API 契约测试（testing.md §7）：in-process axum router + tower oneshot，不 spawn 二进制
//! （决策 144）。
//!
//! 覆盖：端点契约、跨源防护矩阵（决策 128）、api_key 回显（决策 112）、
//! resume 游标解析（决策 91）与防连点、merge/decision（决策 119）、人工评审（决策 2）。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use agentpipeline_core::agent::tools::CommandRecorder;
use agentpipeline_core::config::Settings;
use agentpipeline_core::sse::{SseEvent, SseEventType};
use agentpipeline_core::types::{Provider, ReviewMode, Stage, TaskStatus};
use app::{build_router, AppState};
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use serde_json::Value;
use testkit::{seed_project, seed_task, seed_task_full, Repo, TestHome};
use tower::ServiceExt;

const PORT: u16 = 8787;

struct Api {
    _home: TestHome,
    _repo: Repo,
    state: AppState,
    router: Router,
    resumes: Arc<AtomicUsize>,
}

async fn api() -> Api {
    let home = TestHome::new().unwrap();
    let (store, _clock) = home.setup().await.unwrap();
    let repo = Repo::clean().unwrap();

    // 默认配一个可用 provider，否则 POST /tasks 会被决策 56 拦下
    store
        .upsert_provider(&Provider {
            id: "p-default".into(),
            vendor: "deepseek".into(),
            model: "deepseek-chat".into(),
            context_window: 64_000,
            base_url: None,
            api_key: Some("sk-super-secret-value-123456".into()),
            enabled: true,
            created_at: store.now(),
            updated_at: store.now(),
        })
        .await
        .unwrap();

    // 端口 0 会让 allowed_origins 变成 http://127.0.0.1:0；测试显式用 PORT
    let settings = Settings {
        pending_resume_cooldown_sec: 5,
        ..Default::default()
    };
    let resumes = Arc::new(AtomicUsize::new(0));
    let hook_resumes = resumes.clone();
    let state = AppState::new(store, home.home().clone(), settings, PORT).with_resume_hook(
        Arc::new(move |_task_id| {
            hook_resumes.fetch_add(1, Ordering::SeqCst);
        }),
    );
    let router = build_router(state.clone());
    Api {
        _home: home,
        _repo: repo,
        state,
        router,
        resumes,
    }
}

fn request(method: &str, uri: &str) -> axum::http::request::Builder {
    Request::builder().method(method).uri(uri)
}

async fn json_body(response: axum::response::Response) -> (StatusCode, Value) {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or(Value::String(String::from_utf8_lossy(&bytes).to_string()))
    };
    (status, value)
}

async fn call(api: &Api, req: Request<Body>) -> (StatusCode, Value) {
    let response = api.router.clone().oneshot(req).await.unwrap();
    json_body(response).await
}

async fn post(api: &Api, uri: &str, body: Value) -> (StatusCode, Value) {
    call(
        api,
        request("POST", uri)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}

async fn get(api: &Api, uri: &str) -> (StatusCode, Value) {
    call(api, request("GET", uri).body(Body::empty()).unwrap()).await
}

async fn seed(api: &Api, task_id: &str) -> String {
    let project_id = format!("proj-{task_id}");
    seed_project(
        &api.state.store,
        &project_id,
        "示例",
        api._repo.path(),
        "main",
    )
    .await
    .unwrap();
    seed_task(&api.state.store, task_id, &project_id)
        .await
        .unwrap();
    project_id
}

// ─────────────────────────── POST /tasks（决策 27 / 56 / 98）───────────────────────────

#[tokio::test]
async fn create_task_lands_queued_and_with_dependency_waiting() {
    let api = api().await;
    let project_id = seed(&api, "t0").await;

    let (status, body) = post(
        &api,
        "/tasks",
        serde_json::json!({"project_id": project_id, "title": "无依赖"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["task"]["status"], "queued");

    let (status, body) = post(
        &api,
        "/tasks",
        serde_json::json!({
            "project_id": project_id,
            "title": "有依赖",
            "depends_on": ["t0"]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        body["task"]["status"], "waiting",
        "有依赖一律 waiting 落库（决策 98）"
    );
}

#[tokio::test]
async fn create_task_rejects_missing_project_and_unknown_dependency() {
    let api = api().await;
    let project_id = seed(&api, "a").await;

    // 依赖链：b 依赖 a
    let (status, body) = post(
        &api,
        "/tasks",
        serde_json::json!({"project_id": project_id, "title": "b", "depends_on": ["a"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // 未知依赖 → 400
    let (status, body) = post(
        &api,
        "/tasks",
        serde_json::json!({"project_id": project_id, "title": "d", "depends_on": ["不存在"]}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("依赖任务不存在"));

    // 未知项目 → 400
    let (status, body) = post(
        &api,
        "/tasks",
        serde_json::json!({"project_id": "不存在", "title": "x"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("项目不存在"));

    // 注：id 由服务端生成，API 层无法构造环——决策 27 的环检测在
    // cursor_lifecycle.rs 的 `dependency_states_and_cycle_detection` 覆盖（store 级）。
}

#[tokio::test]
async fn create_task_fails_fast_without_configured_provider() {
    // 决策 56：未配置 provider 时创建任务返回明确错误
    let api = api().await;
    let project_id = seed(&api, "t0").await;
    api.state.store.delete_provider("p-default").await.unwrap();

    let (status, body) = post(
        &api,
        "/tasks",
        serde_json::json!({"project_id": project_id, "title": "x"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("provider"));
}

// ─────────────────────────── GET /tasks（决策 101）───────────────────────────

#[tokio::test]
async fn list_tasks_supports_filters_and_branch_summary() {
    let api = api().await;
    let project_id = seed(&api, "t1").await;
    // 造一条 archived 任务（默认应被过滤）
    seed_task(&api.state.store, "t2", &project_id)
        .await
        .unwrap();
    api.state.store.archive_task("t2").await.unwrap();

    let (status, body) = get(&api, "/tasks").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["tasks"].as_array().unwrap().len(), 1);

    let (_, body) = get(&api, "/tasks?include_archived=true").await;
    assert_eq!(body["tasks"].as_array().unwrap().len(), 2);

    let (_, body) = get(
        &api,
        &format!("/tasks?project_id={project_id}&status=queued"),
    )
    .await;
    assert_eq!(
        body["tasks"].as_array().unwrap().len(),
        1,
        "archived 任务默认不参与筛选"
    );
    let (_, body) = get(
        &api,
        &format!("/tasks?project_id={project_id}&status=queued&include_archived=true"),
    )
    .await;
    assert_eq!(body["tasks"].as_array().unwrap().len(), 2);

    // 分支级摘要
    let (_, body) = get(&api, "/tasks").await;
    let branches = body["tasks"][0]["branches"].as_array().unwrap();
    assert_eq!(branches.len(), 1);
    assert_eq!(branches[0]["branch"], "main");
    assert_eq!(branches[0]["stage"], "init");
    assert_eq!(branches[0]["node"], "execute");
}

// ─────────────────────────── GET /tasks/{id}（决策 49 / 130）───────────────────────────

#[tokio::test]
async fn task_detail_returns_allowed_actions_for_pending_cursor() {
    let api = api().await;
    seed(&api, "t1").await;
    let cursor = api.state.store.load_live_cursors("t1").await.unwrap()[0].clone();

    // 无 pending → 无动作
    let (_, body) = get(&api, "/tasks/t1").await;
    assert!(body["allowed_actions"].as_array().unwrap().is_empty());

    // 挂 info_insufficient → continue(requires_input) + cancel
    api.state
        .store
        .set_cursor_pending(
            &cursor.cursor_id,
            &agentpipeline_core::types::PendingReason::new(
                agentpipeline_core::types::PendingKind::InfoInsufficient,
                Stage::ArchitectDesign,
                agentpipeline_core::types::Node::ValidateInput,
                "信息不足",
            ),
        )
        .await
        .unwrap();
    api.state.store.sync_task_projection("t1").await.unwrap();

    let (_, body) = get(&api, "/tasks/t1").await;
    let actions = body["allowed_actions"].as_array().unwrap();
    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0]["action"], "continue");
    assert_eq!(actions[0]["requires_input"], true);
    assert_eq!(actions[0]["kind"], "resume");
    assert_eq!(actions[0]["cursor_id"], cursor.cursor_id);
    assert_eq!(actions[1]["kind"], "side_effect");
}

// ─────────────────────────── POST /resume（决策 91 / 49 / 防连点）───────────────────────────

#[tokio::test]
async fn resume_omits_cursor_id_only_when_exactly_one_cursor() {
    let api = api().await;
    seed(&api, "t1").await;
    let cursor = api.state.store.load_live_cursors("t1").await.unwrap()[0].clone();
    api.state
        .store
        .set_cursor_pending(
            &cursor.cursor_id,
            &agentpipeline_core::types::PendingReason::new(
                agentpipeline_core::types::PendingKind::InfoInsufficient,
                Stage::ArchitectDesign,
                agentpipeline_core::types::Node::ValidateInput,
                "补齐信息",
            ),
        )
        .await
        .unwrap();
    api.state.store.sync_task_projection("t1").await.unwrap();

    // 恰好一条 → 可省略
    let (status, body) = post(
        &api,
        "/tasks/t1/resume",
        serde_json::json!({"action": "continue"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["spawned"], true);
    assert_eq!(api.resumes.load(Ordering::SeqCst), 1);
    assert!(api
        .state
        .store
        .get_cursor(&cursor.cursor_id)
        .await
        .unwrap()
        .pending_reason
        .is_none());

    // 分裂成两条后再次 resume 且不给 cursor_id → 409（决策 91）
    api.state.store.split_cursors("t1").await.unwrap();
    let (status, body) = post(
        &api,
        "/tasks/t1/resume",
        serde_json::json!({"action": "continue"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body["error"].as_str().unwrap().contains("cursor_id"));
}

#[tokio::test]
async fn resume_rejects_action_outside_allowed_set() {
    let api = api().await;
    seed(&api, "t1").await;
    let cursor = api.state.store.load_live_cursors("t1").await.unwrap()[0].clone();
    // merge_approval 只允许 approve / return（side_effect），不含 skip
    api.state
        .store
        .set_cursor_pending(
            &cursor.cursor_id,
            &agentpipeline_core::types::PendingReason::new(
                agentpipeline_core::types::PendingKind::MergeApproval,
                Stage::Merge,
                agentpipeline_core::types::Node::Execute,
                "等待审批",
            ),
        )
        .await
        .unwrap();

    let (status, body) = post(
        &api,
        "/tasks/t1/resume",
        serde_json::json!({"action": "skip"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("允许集合"));
}

#[tokio::test]
async fn resume_cooldown_prevents_double_spawn() {
    // §3 pending_resume_cooldown_sec：第二次 resume 不重复启动（固定时钟下必然命中）
    let api = api().await;
    seed(&api, "t1").await;

    for _ in 0..2 {
        let cursor = api.state.store.load_live_cursors("t1").await.unwrap()[0].clone();
        api.state
            .store
            .set_cursor_pending(
                &cursor.cursor_id,
                &agentpipeline_core::types::PendingReason::new(
                    agentpipeline_core::types::PendingKind::InfoInsufficient,
                    Stage::ArchitectDesign,
                    agentpipeline_core::types::Node::ValidateInput,
                    "补齐",
                ),
            )
            .await
            .unwrap();
        let (status, _) = post(
            &api,
            "/tasks/t1/resume",
            serde_json::json!({"action": "continue"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    // 第一次 spawn，第二次因冷却被拦（决策 36 的单执行者守卫）
    assert_eq!(api.resumes.load(Ordering::SeqCst), 1);
}

// ─────────────────────────── merge/decision（决策 119）───────────────────────────

#[tokio::test]
async fn merge_decision_approve_and_return_move_the_cursor() {
    let api = api().await;
    seed(&api, "t1").await;
    // 造出 merge 阶段的 pending(merge_approval)
    api.state
        .store
        .replace_cursors_with_main(
            "t1",
            Stage::Merge,
            agentpipeline_core::types::Node::Execute,
            "main",
        )
        .await
        .unwrap();
    api.state
        .store
        .upsert_stage_output(
            "t1",
            Stage::Merge,
            agentpipeline_core::types::MERGE_OUTPUT_TYPE,
            "merge-proposal.diff",
            // 文档必填字段齐全（缺字段 = 行损坏，反序列化必须报错而不是静默兜底）
            Some(&serde_json::json!({
                "diff_path": "merge-proposal.diff",
                "diff_stats": {"files_changed": 1, "insertions": 0, "deletions": 0, "file_details": []},
                "base_commit": "basesha",
                "gate": "pass",
                "gate_failures": 2,
                "approval": "pending",
                "status": "pending_approval"
            })),
        )
        .await
        .unwrap();
    let cursor = api.state.store.load_live_cursors("t1").await.unwrap()[0].clone();
    api.state
        .store
        .set_cursor_pending(
            &cursor.cursor_id,
            &agentpipeline_core::types::PendingReason::new(
                agentpipeline_core::types::PendingKind::MergeApproval,
                Stage::Merge,
                agentpipeline_core::types::Node::Execute,
                "等待审批",
            ),
        )
        .await
        .unwrap();

    // approve → 重入 merge.execute 走阶段 B
    let (status, body) = post(
        &api,
        "/tasks/t1/merge/decision",
        serde_json::json!({"decision": "approve"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let after = body["cursor"].clone();
    assert_eq!(after["stage"], "merge");
    assert_eq!(after["node"], "execute");

    // approval 落库为 approved
    let meta = api
        .state
        .store
        .stage_output_metadata(
            "t1",
            Stage::Merge,
            agentpipeline_core::types::MERGE_OUTPUT_TYPE,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(meta["approval"], "approved");

    // return → develop.execute，attempts 重置
    api.state
        .store
        .set_cursor_pending(
            &api.state.store.load_live_cursors("t1").await.unwrap()[0]
                .cursor_id
                .clone(),
            &agentpipeline_core::types::PendingReason::new(
                agentpipeline_core::types::PendingKind::MergeApproval,
                Stage::Merge,
                agentpipeline_core::types::Node::Execute,
                "再审批",
            ),
        )
        .await
        .unwrap();
    let (status, body) = post(
        &api,
        "/tasks/t1/merge/decision",
        serde_json::json!({"decision": "return"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["cursor"]["stage"], "develop");
    assert_eq!(body["cursor"]["node"], "execute");
}

// ─────────────────────────── 人工评审（决策 2 / 124）───────────────────────────

#[tokio::test]
async fn human_review_routes_to_test_or_develop() {
    let api = api().await;
    let project_id = seed(&api, "t1").await;
    seed_task_full(
        &api.state.store,
        "t-human",
        &project_id,
        ReviewMode::Human,
        &[],
    )
    .await
    .unwrap();

    // 非 human 模式拒绝
    let (status, _) = post(
        &api,
        "/tasks/t1/review",
        serde_json::json!({"approved": true}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // human approve → test.execute
    api.state
        .store
        .replace_cursors_with_main(
            "t-human",
            Stage::Review,
            agentpipeline_core::types::Node::ValidateOutput,
            "main",
        )
        .await
        .unwrap();
    let (status, body) = post(
        &api,
        "/tasks/t-human/review",
        serde_json::json!({"approved": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["stage"], "test");

    // human reject → develop.execute
    api.state
        .store
        .replace_cursors_with_main(
            "t-human",
            Stage::Review,
            agentpipeline_core::types::Node::ValidateOutput,
            "main",
        )
        .await
        .unwrap();
    let (status, body) = post(
        &api,
        "/tasks/t-human/review",
        serde_json::json!({"approved": false}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["stage"], "develop");
}

// ─────────────────────────── 旁路端点（决策 105 / 117 / 125 / 34）───────────────────────────

#[tokio::test]
async fn retry_requires_terminal_and_requeues() {
    let api = api().await;
    seed(&api, "t1").await;

    // 非终态拒绝
    let (status, _) = post(&api, "/tasks/t1/retry", serde_json::json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    api.state
        .store
        .mark_terminal("t1", TaskStatus::Failed)
        .await
        .unwrap();
    let (status, body) = post(&api, "/tasks/t1/retry", serde_json::json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let task = api.state.store.get_task("t1").await.unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Queued,
        "重试置回 queued 重新准入（决策 117）"
    );
    // 旧游标归档、新 main 指向 init
    let live = api.state.store.load_live_cursors("t1").await.unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].stage, Stage::Init);
}

#[tokio::test]
async fn cancel_marks_terminal_and_notifies_dependents() {
    let api = api().await;
    let project_id = seed(&api, "dep").await;
    seed_task_full(
        &api.state.store,
        "dependent",
        &project_id,
        ReviewMode::Agent,
        &["dep"],
    )
    .await
    .unwrap();

    let (status, body) = post(&api, "/tasks/dep/cancel", serde_json::json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["notified"][0], "dependent");

    let dep = api.state.store.get_task("dep").await.unwrap();
    assert_eq!(dep.status, TaskStatus::Cancelled);
    let dependent = api.state.store.get_task("dependent").await.unwrap();
    assert_eq!(dependent.status, TaskStatus::Pending);
    let reason = dependent.pending_reason.unwrap();
    assert_eq!(
        reason.kind,
        agentpipeline_core::types::PendingKind::DependencyFailed
    );
    assert_eq!(
        reason.context.unwrap().kind.as_deref(),
        Some("dependency_cancelled")
    );
}

#[tokio::test]
async fn archive_requires_terminal_and_splits_into_new_tasks() {
    let api = api().await;
    let project_id = seed(&api, "t1").await;

    let (status, _) = post(&api, "/tasks/t1/archive", serde_json::json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    api.state
        .store
        .mark_terminal("t1", TaskStatus::Cancelled)
        .await
        .unwrap();
    let (status, _) = post(&api, "/tasks/t1/archive", serde_json::json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert!(api
        .state
        .store
        .get_task("t1")
        .await
        .unwrap()
        .archived_at
        .is_some());

    // split：创建 N 个 + 原任务置 cancelled
    seed(&api, "t9").await;
    let (status, body) = post(
        &api,
        "/tasks/t9/split",
        serde_json::json!({"tasks": [{"title": "子 1"}, {"title": "子 2"}]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["created"].as_array().unwrap().len(), 2);
    assert_eq!(
        api.state.store.get_task("t9").await.unwrap().status,
        TaskStatus::Cancelled
    );
    let _ = project_id;
}

#[tokio::test]
async fn model_override_validates_whitelist_and_scope() {
    let api = api().await;
    seed(&api, "t1").await;
    seed(&api, "t2").await;

    let (status, _) = post(
        &api,
        "/tasks/t1/model-override",
        serde_json::json!({"provider_id": "不存在"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = post(
        &api,
        "/tasks/t1/model-override",
        serde_json::json!({"provider_id": "p-default"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        api.state
            .store
            .get_task("t1")
            .await
            .unwrap()
            .model_override
            .as_deref(),
        Some("p-default")
    );
    // 只影响本任务（决策 105）
    assert!(api
        .state
        .store
        .get_task("t2")
        .await
        .unwrap()
        .model_override
        .is_none());
}

// ─────────────────────────── 项目端点（决策 61 / 101）───────────────────────────

#[tokio::test]
async fn project_creation_rejects_non_git_and_delete_refuses_active_tasks() {
    let api = api().await;
    let plain = api._home.scratch_dir("not-a-repo");

    let (status, body) = post(
        &api,
        "/projects",
        serde_json::json!({"name": "非 git", "local_path": plain.display().to_string()}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("不是 git 仓库"));

    // 正常创建
    let (status, body) = post(
        &api,
        "/projects",
        serde_json::json!({"name": "示例", "local_path": api._repo.path().display().to_string()}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let project_id = body["project"]["id"].as_str().unwrap().to_string();
    assert_eq!(body["project"]["default_branch"], "main");
    assert_eq!(body["project"]["language"], "rust");
    assert_eq!(body["project"]["test_framework"], "cargo");

    // 有活跃任务 → 拒绝删除
    seed_task(&api.state.store, "t1", &project_id)
        .await
        .unwrap();
    let (status, body) = call(
        &api,
        request("DELETE", &format!("/projects/{project_id}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    // 任务的写端点（cancel）后仍不活跃 → 可删
    post(&api, "/tasks/t1/cancel", serde_json::json!({})).await;
    let (status, _) = call(
        &api,
        request("DELETE", &format!("/projects/{project_id}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn project_analyze_is_async_and_pollable() {
    let api = api().await;
    let (_, body) = post(
        &api,
        "/projects",
        serde_json::json!({"name": "示例", "local_path": api._repo.path().display().to_string()}),
    )
    .await;
    let project_id = body["project"]["id"].as_str().unwrap().to_string();

    let (status, body) = post(
        &api,
        "/projects/analyze",
        serde_json::json!({"project_id": project_id}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "决策 130 ⑦：202 + 轮询");
    assert!(body["analysis_id"].is_string());

    // 探测在后台任务里完成，轮询直到 done
    for _ in 0..50 {
        let (_, body) = get(&api, &format!("/projects/{project_id}/analysis")).await;
        if body["status"] == "done" {
            assert_eq!(body["result"]["language"], "rust");
            assert_eq!(body["result"]["has_gitignore"], true);
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("项目分析未在预期时间内完成");
}

// ─────────────────────────── provider（决策 112）───────────────────────────

#[tokio::test]
async fn provider_read_api_masks_api_key_and_patch_keeps_secret() {
    let api = api().await;

    let (status, body) = get(&api, "/providers").await;
    assert_eq!(status, StatusCode::OK);
    let providers = body["providers"].as_array().unwrap();
    assert_eq!(providers[0]["api_key"], "***");
    assert!(
        !body.to_string().contains("sk-super-secret-value-123456"),
        "读接口不得返回原值"
    );

    // PATCH 里传回显值 "***" 不得把真密钥覆盖掉
    let (status, _) = call(
        &api,
        request("PATCH", "/providers/p-default")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::json!({"api_key": "***", "enabled": false}).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let raw = api
        .state
        .store
        .get_provider("p-default")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(raw.api_key.as_deref(), Some("sk-super-secret-value-123456"));
    assert!(!raw.enabled);

    // 真传新密钥才覆盖
    call(
        &api,
        request("PATCH", "/providers/p-default")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::json!({"api_key": "sk-new-key-abcdefghijkl"}).to_string(),
            ))
            .unwrap(),
    )
    .await;
    let raw = api
        .state
        .store
        .get_provider("p-default")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(raw.api_key.as_deref(), Some("sk-new-key-abcdefghijkl"));
}

// ─────────────────────────── 跨源防护矩阵（决策 128）───────────────────────────

#[tokio::test]
async fn cross_origin_guard_matrix() {
    let api = api().await;
    seed(&api, "t1").await;

    // ① 带自定义头 → 过
    let (status, _) = call(
        &api,
        request("POST", "/tasks/t1/retry")
            .header("X-AgentPipeline", "1")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await;
    assert_ne!(status, StatusCode::FORBIDDEN, "带自定义头应放行");

    // ② 无 Origin/Referer（非浏览器客户端）→ 过
    let (status, _) = post(&api, "/tasks/t1/retry", serde_json::json!({})).await;
    assert_ne!(status, StatusCode::FORBIDDEN);

    // ③ 本机 Origin → 过
    let (status, _) = call(
        &api,
        request("POST", "/tasks/t1/retry")
            .header(header::ORIGIN, format!("http://127.0.0.1:{PORT}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await;
    assert_ne!(status, StatusCode::FORBIDDEN);

    // ④ 恶意 Origin → 403
    let (status, body) = call(
        &api,
        request("POST", "/tasks/t1/retry")
            .header(header::ORIGIN, "http://evil.example")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body["error"].as_str().unwrap().contains("跨源"));

    // ⑤ 恶意 Referer 同样被拦（form 提交没有 Origin）
    let (status, _) = call(
        &api,
        request("POST", "/tasks/t1/cancel")
            .header(header::REFERER, "https://evil.example/page")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // ⑥ GET / SSE 不受影响
    let (status, _) = call(
        &api,
        request("GET", "/tasks/t1/flow")
            .header(header::ORIGIN, "http://evil.example")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "SSE / GET 不受跨源防护影响（决策 128）"
    );
}

// ─────────────────────────── SSE 通道（决策 76 / 84 / 123）───────────────────────────

#[tokio::test]
async fn stream_is_the_only_event_channel_and_carries_branch() {
    let api = api().await;
    seed(&api, "t1").await;

    let response = api
        .router
        .clone()
        .oneshot(
            request("GET", "/tasks/t1/stream")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        content_type.starts_with("text/event-stream"),
        "{content_type}"
    );

    // 事件体格式：type + branch（用录制器直接验证序列化契约）
    let recorder = testkit::SseRecorder::new();
    use agentpipeline_core::sse::SseSink;
    recorder.emit(SseEvent::NodeStarted {
        task_id: "t1".into(),
        branch: "develop-design".into(),
        stage: Stage::DevelopDesign,
        node: agentpipeline_core::types::Node::Execute,
        attempt: 1,
        run_id: 1,
    });
    assert_eq!(recorder.count_of(SseEventType::NodeStarted), 1);
    assert_eq!(recorder.branches(), vec!["develop-design".to_string()]);
}

// ─────────────────────────── 只读视图（决策 63 / 99 / 114 / 76）───────────────────────────

#[tokio::test]
async fn flow_metrics_and_conversations_endpoints() {
    let api = api().await;
    seed(&api, "t1").await;
    let cursor = api.state.store.load_live_cursors("t1").await.unwrap()[0].clone();

    // system run 有 run 行但无会话行（决策 99 / 114）
    let store = &api.state.store;
    let system_run = store
        .insert_run(&agentpipeline_core::storage::observability::NewRun {
            task_id: "t1".into(),
            cursor_id: cursor.cursor_id.clone(),
            stage: Stage::Init,
            node: agentpipeline_core::types::Node::Execute,
            attempt: 1,
            agent_type: "system".into(),
            parent_run_id: None,
            prompt_template_hash: None,
            process_group_id: None,
        })
        .await
        .unwrap();
    store
        .finish_run(
            system_run,
            &agentpipeline_core::storage::observability::RunOutcome {
                status: Some(agentpipeline_core::types::NodeStatus::Success),
                duration_ms: 42,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let llm_run = store
        .insert_run(&agentpipeline_core::storage::observability::NewRun {
            task_id: "t1".into(),
            cursor_id: cursor.cursor_id.clone(),
            stage: Stage::ArchitectDesign,
            node: agentpipeline_core::types::Node::Execute,
            attempt: 1,
            agent_type: "main".into(),
            parent_run_id: None,
            prompt_template_hash: Some("abcdef0123456789".into()),
            process_group_id: None,
        })
        .await
        .unwrap();
    store
        .finish_run(
            llm_run,
            &agentpipeline_core::storage::observability::RunOutcome {
                status: Some(agentpipeline_core::types::NodeStatus::Success),
                prompt_tokens: 100,
                completion_tokens: 50,
                duration_ms: 7,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    store
        .insert_conversation(
            "t1",
            llm_run,
            Stage::ArchitectDesign,
            agentpipeline_core::types::Node::Execute,
            1,
            "main",
            None,
            &serde_json::json!([{"role": "system", "content": "x"}]),
            Some(&serde_json::json!({"readiness": true})),
            100,
            50,
        )
        .await
        .unwrap();
    store.refresh_task_totals("t1").await.unwrap();

    // metrics：system run 不计入 total_calls（决策 130 ②）
    let (status, body) = get(&api, "/tasks/t1/metrics").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total_calls"], 1);
    assert_eq!(body["total_tokens"], 150);
    assert_eq!(body["stored_total_calls"], 1);

    // conversations：只列 LLM run
    let (_, body) = get(&api, "/tasks/t1/conversations").await;
    let conversations = body["conversations"].as_array().unwrap();
    assert_eq!(conversations.len(), 1);
    assert_eq!(conversations[0]["agent_type"], "main");

    // 单条会话
    let (status, body) = get(&api, &format!("/tasks/t1/conversations/{llm_run}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["conversation"]["prompt_tokens"], 100);

    // flow：流转时间线 + 游标
    let (status, body) = get(&api, "/tasks/t1/flow").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["cursors"].as_array().unwrap().len() == 1);

    // 全局指标
    let (status, body) = get(&api, "/metrics").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["tasks"], 1);
}

#[tokio::test]
async fn command_log_endpoints_expose_卸载_output() {
    let api = api().await;
    seed(&api, "t1").await;
    let store = &api.state.store;

    use agentpipeline_core::agent::tools::{CommandFinish, CommandRecorder, CommandStart};
    let command_id = store
        .record_start(CommandStart {
            task_id: "t1".into(),
            run_id: None,
            stage: Stage::Test,
            node: agentpipeline_core::types::Node::Execute,
            source: agentpipeline_core::types::CommandSource::System,
            command: "cargo test".into(),
            cwd: "/tmp".into(),
        })
        .await
        .unwrap();
    // 卸载文件真实落盘
    let offload = api._home.home().context_dir("t1").join("out.txt");
    std::fs::create_dir_all(offload.parent().unwrap()).unwrap();
    std::fs::write(&offload, "完整输出\n第二行").unwrap();
    store
        .record_finish(
            command_id,
            CommandFinish {
                exit_code: Some(0),
                stdout_path: Some(offload.display().to_string()),
                duration_ms: 12,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let (status, body) = get(&api, "/tasks/t1/commands").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["commands"].as_array().unwrap().len(), 1);
    assert_eq!(body["commands"][0]["command"], "cargo test");

    let (status, body) = get(&api, &format!("/tasks/t1/commands/{command_id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["command"]["source"], "system");

    // 完整 stdout 从卸载文件读取（决策 114）
    let response = api
        .router
        .clone()
        .oneshot(
            request("GET", &format!("/tasks/t1/commands/{command_id}/output"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&bytes), "完整输出\n第二行");
}

// ─────────────────────────── 任务产出文件（G12 / §4.2）───────────────────────────

#[tokio::test]
async fn task_files_endpoint_reads_inside_task_dir_only() {
    let api = api().await;
    seed(&api, "t1").await;
    let design = api._home.home().task_file("t1", "design.md");
    std::fs::create_dir_all(design.parent().unwrap()).unwrap();
    std::fs::write(&design, "# 设计文档").unwrap();

    let response = api
        .router
        .clone()
        .oneshot(
            request("GET", "/tasks/t1/files/design.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&bytes), "# 设计文档");

    // 目录逃逸必须被拒（G11 的 API 侧防线）
    let (status, _) = get(&api, "/tasks/t1/files/../../../etc/hosts").await;
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND,
        "越界路径应被拒绝，实际 {status}"
    );
}

// ─────────────── 偏离修复回归（2026-09-12 文档-实现对齐）───────────────

#[tokio::test]
async fn cross_origin_rejects_prefix_spoofing_but_accepts_same_origin_referer_path() {
    // 决策 128（修订）：严格相等，禁前缀匹配
    let api = api().await;
    seed(&api, "t1").await;

    for spoofed in [
        format!("http://127.0.0.1:{PORT}0.evil.example"),
        format!("http://127.0.0.1:{PORT}.evil.example"),
        "http://localhost:9999".to_string(),
    ] {
        let (status, _) = call(
            &api,
            request("POST", "/tasks/t1/retry")
                .header(header::ORIGIN, spoofed.clone())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{spoofed}");
    }

    // 同源 Referer（带路径）放行
    let (status, _) = call(
        &api,
        request("POST", "/tasks/t1/retry")
            .header(header::REFERER, format!("http://127.0.0.1:{PORT}/tasks/t1"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_ne!(status, StatusCode::FORBIDDEN, "同源 Referer 应放行");
}

#[tokio::test]
async fn human_review_comments_reach_the_transition_reason() {
    // §12.5：`POST /tasks/{id}/review {approved, comments}`，打回时评论带给 develop
    let api = api().await;
    let project_id = seed(&api, "t1").await;
    seed_task_full(
        &api.state.store,
        "t-human",
        &project_id,
        ReviewMode::Human,
        &[],
    )
    .await
    .unwrap();

    let (status, body) = post(
        &api,
        "/tasks/t-human/review",
        serde_json::json!({"approved": false, "comments": "错误处理缺失，请补齐"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let transitions = api.state.store.list_transitions("t-human").await.unwrap();
    assert!(transitions.iter().any(|t| t
        .reason
        .as_deref()
        .is_some_and(|r| r.contains("错误处理缺失，请补齐"))));
}

#[tokio::test]
async fn goto_rejects_targets_outside_the_stage_entry_node() {
    // 决策 69：goto 落点 = entry_node(stage)
    let api = api().await;
    seed(&api, "t1").await;
    let cursor = api.state.store.load_live_cursors("t1").await.unwrap()[0].clone();
    api.state
        .store
        .set_cursor_pending(
            &cursor.cursor_id,
            &agentpipeline_core::types::PendingReason::new(
                agentpipeline_core::types::PendingKind::UserDecision,
                Stage::Develop,
                agentpipeline_core::types::Node::Execute,
                "重复风险",
            )
            .with_context(agentpipeline_core::types::PendingContext::with_kind(
                "duplicate_risk",
            )),
        )
        .await
        .unwrap();

    // 非入口节点 → 400
    let (status, body) = post(
        &api,
        "/tasks/t1/resume",
        serde_json::json!({"action": "goto", "target_stage": "develop", "target_node": "validate_input"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"].as_str().unwrap().contains("入口节点"));

    // entry_node(stage) → 放行
    let (status, body) = post(
        &api,
        "/tasks/t1/resume",
        serde_json::json!({"action": "goto", "target_stage": "develop", "target_node": "execute"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn dependency_failed_continue_requeues_without_spawning() {
    // 决策 130⑤：continue 交还准入，不直接 spawn executor
    let api = api().await;
    seed(&api, "t1").await;
    let cursor = api.state.store.load_live_cursors("t1").await.unwrap()[0].clone();
    api.state
        .store
        .set_cursor_pending(
            &cursor.cursor_id,
            &agentpipeline_core::types::PendingReason::new(
                agentpipeline_core::types::PendingKind::DependencyFailed,
                Stage::Develop,
                agentpipeline_core::types::Node::Execute,
                "依赖任务 dep 已取消",
            )
            .with_context(agentpipeline_core::types::PendingContext::with_kind(
                "dependency_cancelled",
            )),
        )
        .await
        .unwrap();

    let before = api.resumes.load(Ordering::SeqCst);
    let (status, body) = post(
        &api,
        "/tasks/t1/resume",
        serde_json::json!({"action": "continue"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["spawned"], false, "dependency continue不得 spawn");
    assert_eq!(api.resumes.load(Ordering::SeqCst), before);
    assert_eq!(
        api.state.store.get_task("t1").await.unwrap().status,
        TaskStatus::Queued,
        "置回 queued 交还准入（决策 116）"
    );
}

#[tokio::test]
async fn commands_are_scoped_to_their_task() {
    let api = api().await;
    seed(&api, "t1").await;
    seed(&api, "t2").await;

    let cmd_id = api
        .state
        .store
        .record_start(agentpipeline_core::agent::tools::CommandStart {
            task_id: "t1".into(),
            run_id: None,
            stage: Stage::Develop,
            node: agentpipeline_core::types::Node::Execute,
            source: agentpipeline_core::types::CommandSource::System,
            command: "cargo test".into(),
            cwd: ".".into(),
        })
        .await
        .unwrap();

    let (status, _) = get(&api, &format!("/tasks/t2/commands/{cmd_id}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "不得跨任务读命令");
    let (status, _) = get(&api, &format!("/tasks/t2/commands/{cmd_id}/output")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = get(&api, &format!("/tasks/t1/commands/{cmd_id}")).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn retry_resets_worktree_and_records_the_system_command() {
    // 决策 125：retry 显式 `git reset --hard {base_ref}` + `git clean -fdx`（记 system 命令）
    let api = api().await;
    seed(&api, "t1").await;

    let worktree = api._home.home().worktree_path("t1");
    agentpipeline_core::git::Git
        .init_worktree(api._repo.path(), "t1", &worktree, "main")
        .await
        .unwrap();
    std::fs::write(worktree.join("half_done.txt"), "半成品").unwrap();
    api.state
        .store
        .set_task_worktree("t1", worktree.to_string_lossy().as_ref(), "kanban/t1")
        .await
        .unwrap();
    api.state
        .store
        .mark_terminal("t1", TaskStatus::Failed)
        .await
        .unwrap();

    let (status, body) = post(&api, "/tasks/t1/retry", serde_json::json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    assert!(
        !worktree.join("half_done.txt").exists(),
        "clean -fdx 应清掉半成品"
    );
    let cmds = api
        .state
        .store
        .list_commands("t1", None, None)
        .await
        .unwrap();
    assert_eq!(cmds.len(), 1, "{:?}", cmds);
    assert_eq!(
        cmds[0].source,
        agentpipeline_core::types::CommandSource::System
    );
    assert!(cmds[0].command.contains("reset --hard"));
    assert_eq!(cmds[0].exit_code, Some(0));
    assert_eq!(
        api.state.store.get_task("t1").await.unwrap().status,
        TaskStatus::Queued,
        "置回 queued 重新走准入（决策 117）"
    );
}
