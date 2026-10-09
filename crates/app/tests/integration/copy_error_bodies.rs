//! L3 集成（测试场景 3 / 场景 5 · AC-2）：**面向用户的错误报文里不出现内部编号**。
//!
//! 文案纪律任务把 API 错误体里的「（决策 N）（票 N）」括注全部摘掉（决策 199 扩面）。
//! 本文件在**真实 HTTP 面**上把场景 3 列出的错误路径逐条打出来，断言两件事：
//!
//! ① 响应错误体（`error` 连同整个 JSON 体）不含 `决策 N / 票 N`；
//! ② 人话语义保留——说清**发生了什么**与**用户该做什么**（摘编号 ≠ 摘语义）。
//!
//! 覆盖与分工：
//! - 场景 3 的 tasks.rs 三条（无 provider / 非终态重试 / 非终态归档）、resume.rs 四条
//!   （多游标缺 `cursor_id`、goto 缺落点、sync-check 作 goto 目标、非入口落点）、
//!   catalog.rs（删有活跃任务的项目）、skill_import.rs（删出厂技能）——全在本文件。
//! - **环路检测那条在 HTTP 层不可构造**：任务 id 由服务端生成，新 id 永远不在既有依赖
//!   图里（api_contract.rs 的既有注释同此结论）。它的报文面钉两道：store 级行为在
//!   `cursor_lifecycle.rs::dependency_states_and_cycle_detection`（既有用例），报文字面量
//!   的编号归零在本文件 `cycle_error_messages_stay_numberless_in_source`，外加前端门
//!   copy-discipline 规则 4 的窄扫。
//! - 场景 5 的**正面**（豁免面没被整库搜改抹掉）：CLI `--help` 属文档面、编号照旧保留，
//!   钉在 `cli_help_keeps_its_decision_annotations`；日志 / 断言 / prompt 注入块的正面锚点
//!   在 `crates/core/tests/integration/copy_payloads.rs`。
//! - 落库渲染字段（run 终态 `error`、`test_blockers` / `metadata_gaps` 载荷）在 core 侧
//!   `copy_payloads.rs` 与 `executor.rs` 的同步载荷用例。

use agentpipeline_core::config::Settings;
use agentpipeline_core::types::{
    Node, PendingContext, PendingKind, PendingReason, Provider, Stage,
};
use app::{build_router, AppState};
use axum::body::Body;
use axum::http::header;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use axum::Router;
use serde_json::{json, Value};
use testkit::{seed_project, seed_task, Repo, TestHome};
use tower::ServiceExt;

const PORT: u16 = 8787;

// ─────────────────────────────── harness（照 api_contract 的最小形） ───────────────────────────────

struct Api {
    _home: TestHome,
    _repo: Repo,
    state: AppState,
    router: Router,
}

async fn api() -> Api {
    let home = TestHome::new().unwrap();
    let (store, _clock) = home.setup().await.unwrap();
    let repo = Repo::clean().unwrap();
    // 默认配一个可用 provider：本文件多数用例要先穿过「有 provider」的正常路径，
    // 只有无 provider 那条用例显式删掉它。
    store
        .upsert_provider(&Provider {
            id: "p-default".into(),
            vendor: "deepseek".into(),
            model: "deepseek-chat".into(),
            context_window: 64_000,
            base_url: None,
            api_key: Some("sk-test-key-for-copy".into()),
            enabled: true,
            created_at: store.now(),
            updated_at: store.now(),
        })
        .await
        .unwrap();
    let state = AppState::new(store, home.home().clone(), Settings::default(), PORT);
    let router = build_router(state.clone());
    Api {
        _home: home,
        _repo: repo,
        state,
        router,
    }
}

fn request(method: &str, uri: &str) -> axum::http::request::Builder {
    Request::builder().method(method).uri(uri)
}

async fn json_body(response: Response) -> (StatusCode, Value) {
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

async fn call(api: &Api, req: axum::http::request::Request<Body>) -> (StatusCode, Value) {
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

async fn delete(api: &Api, uri: &str) -> (StatusCode, Value) {
    call(api, request("DELETE", uri).body(Body::empty()).unwrap()).await
}

/// 建一个项目（返回 id）。任务不建——多数用例只需要「项目存在」。
async fn seed_project_only(api: &Api, project_id: &str) -> String {
    seed_project(
        &api.state.store,
        project_id,
        "示例",
        api._repo.path(),
        "main",
    )
    .await
    .unwrap();
    project_id.to_string()
}

/// 建一个项目 + 一条 `queued`（非终态）任务。
async fn seed_project_task(api: &Api, project_id: &str, task_id: &str) {
    seed_project_only(api, project_id).await;
    seed_task(&api.state.store, task_id, project_id)
        .await
        .unwrap();
}

// ─────────────────────────────── 判据（与前端门规则 4 同口径） ───────────────────────────────

/// 内部编号的形态：`决策 N` / `票 N`——含全角数字、「决策 130 / 137」连写（首段数字
/// 成立即算命中）与票据号的圈码后缀（`票 02②`）。与 `copy-discipline.test.ts` 规则 4
/// 的 `BACKEND_REF` 同一口径，防止两侧判据漂移。
fn internal_ref(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let digit = |c: char| c.is_ascii_digit() || ('０'..='９').contains(&c);
    let space = |c: char| c.is_whitespace() || c == '\u{3000}';
    for i in 0..chars.len() {
        let mark = match chars[i] {
            '决' if chars.get(i + 1) == Some(&'策') => 2,
            '票' => 1,
            _ => 0,
        };
        if mark == 0 {
            continue;
        }
        let mut j = i + mark;
        while j < chars.len() && space(chars[j]) {
            j += 1;
        }
        if j < chars.len() && digit(chars[j]) {
            let mut k = j;
            while k < chars.len()
                && (digit(chars[k]) || ('\u{2460}'..='\u{2473}').contains(&chars[k]))
            {
                k += 1;
            }
            return Some(chars[i..k].iter().collect());
        }
    }
    None
}

fn assert_numberless(what: &str, text: &str) {
    if let Some(found) = internal_ref(text) {
        panic!("{what}的报文残留内部编号「{found}」：{text}");
    }
}

// ─────────────────────────── 场景 3：tasks.rs —— 无可用 provider ───────────────────────────

#[tokio::test]
async fn create_task_without_a_provider_says_what_to_do_in_plain_words() {
    // 场景 3 步骤 1：无可用 provider 时创建任务 → 400，报文摘编号、保留指路语义。
    let api = api().await;
    let project_id = seed_project_only(&api, "p-copy").await;
    api.state.store.delete_provider("p-default").await.unwrap();

    let (status, body) = post(
        &api,
        "/tasks",
        json!({"project_id": project_id, "title": "x", "description": "没有 provider"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let err = body["error"].as_str().expect("错误体有 error 字段");
    assert!(err.contains("provider"), "要点名缺什么：{err}");
    assert!(err.contains("设置"), "要指路去哪补：{err}");
    // 整个错误体（含 detail / kind 若有）都不许带内部编号
    assert_numberless("无 provider 拒绝", &body.to_string());
}

// ─────────────────────────── 场景 3：tasks.rs —— 非终态重试 / 归档 ───────────────────────────

#[tokio::test]
async fn retry_and_archive_refuse_a_live_task_in_plain_words() {
    let api = api().await;
    seed_project_task(&api, "p-copy", "t-live").await;

    let (status, body) = post(&api, "/tasks/t-live/retry", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        body["error"], "只有终态任务可以重试",
        "人话语义保留：说清谁能重试"
    );
    assert_numberless("非终态重试拒绝", &body.to_string());

    let (status, body) = post(&api, "/tasks/t-live/archive", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        body["error"], "只有终态任务可以归档",
        "人话语义保留：说清谁能归档"
    );
    assert_numberless("非终态归档拒绝", &body.to_string());

    // 拒绝是只读的：任务没有被顺手挪动
    let (_, body) = get(&api, "/tasks/t-live").await;
    assert_eq!(body["task"]["status"], "queued", "{body}");
}

// ─────────────────────────── 场景 3：resume.rs —— 四条拒绝路径 ───────────────────────────

#[tokio::test]
async fn resume_rejections_are_numberless_and_say_what_to_do() {
    let api = api().await;
    seed_project_task(&api, "p-copy", "t-res").await;
    let cursor = api.state.store.load_live_cursors("t-res").await.unwrap()[0].clone();
    // duplicate_risk 的动作集 = [goto, cancel]（actions.rs 权威表），够走出全部 goto 错误
    api.state
        .store
        .set_cursor_pending(
            &cursor.cursor_id,
            &PendingReason::new(
                PendingKind::UserDecision,
                Stage::Develop,
                Node::Execute,
                "重复风险",
            )
            .with_context(PendingContext::with_kind("duplicate_risk")),
        )
        .await
        .unwrap();

    // ① goto 不给落点 → 400，点名两个必填参数
    let (status, body) = post(&api, "/tasks/t-res/resume", json!({"action": "goto"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let err = body["error"].as_str().expect("错误体有 error 字段");
    assert!(
        err.contains("target_stage") && err.contains("target_node"),
        "要说明缺哪个参数：{err}"
    );
    assert_numberless("goto 缺落点", &body.to_string());

    // ② sync-check 作 goto 目标 → 400，说清它为什么不占落点
    let (status, body) = post(
        &api,
        "/tasks/t-res/resume",
        json!({"action": "goto", "target_stage": "sync-check", "target_node": "execute"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let err = body["error"].as_str().expect("错误体有 error 字段");
    assert!(err.contains("sync-check 不占游标行"), "{err}");
    assert_numberless("sync-check 作 goto 目标", &body.to_string());

    // ③ 非入口节点 → 400，说清合法落点长什么样
    let (status, body) = post(
        &api,
        "/tasks/t-res/resume",
        json!({"action": "goto", "target_stage": "develop", "target_node": "validate_input"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let err = body["error"].as_str().expect("错误体有 error 字段");
    assert!(err.contains("入口节点"), "要说清合法落点：{err}");
    assert_numberless("goto 非入口落点", &body.to_string());

    // ④ 多条活跃游标、不带 cursor_id → 409，说清必须显式给哪一项
    //    （游标解析在动作校验之前，故这条用一条干净任务即可，不必先挂 pending；
    //     项目已建过，只补任务行）
    seed_task(&api.state.store, "t-split", "p-copy")
        .await
        .unwrap();
    api.state.store.split_cursors("t-split").await.unwrap();
    let (status, body) = post(&api, "/tasks/t-split/resume", json!({"action": "continue"})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let err = body["error"].as_str().expect("错误体有 error 字段");
    assert!(err.contains("多条活跃游标"), "{err}");
    assert!(err.contains("cursor_id"), "要指明必须显式提供的参数：{err}");
    assert_numberless("缺 cursor_id 冲突", &body.to_string());
}

// ─────────────────────── 场景 3：catalog.rs / skill_import.rs ───────────────────────

#[tokio::test]
async fn project_delete_and_factory_skill_refusals_are_numberless() {
    let api = api().await;
    let project_id = seed_project_only(&api, "p-guard").await;

    // ① 有活跃任务 → 拒绝删除（409），人话 + 联锁不松
    seed_task(&api.state.store, "t-active", &project_id)
        .await
        .unwrap();
    let (status, body) = delete(&api, &format!("/projects/{project_id}")).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(
        body["error"], "项目仍有活跃任务，拒绝删除",
        "人话语义保留：说清拒绝的原因"
    );
    assert_numberless("删除有活跃任务的项目", &body.to_string());
    // 拒绝即联锁：项目一行都没删
    let (_, list) = get(&api, "/projects").await;
    assert!(
        list["projects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == project_id.as_str()),
        "拒绝删除后项目必须还在：{list}"
    );

    // ② 删出厂技能 → 400，说清为什么删不得、想让它不生效该改哪（出路）
    //    （判据在存在性检查之前，故不必先播种）
    let (status, body) = delete(&api, "/skills/operate-pipeline").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let err = body["error"].as_str().expect("错误体有 error 字段");
    assert!(
        err.contains("出厂技能") && err.contains("不可删除"),
        "要说清为什么删不得：{err}"
    );
    assert!(err.contains("persona_append"), "要给出路：{err}");
    assert_numberless("出厂技能删除拒绝", &body.to_string());
}

// ─────────────────────── 场景 3：环路报文的字面量面（HTTP 不可构造） ───────────────────────

#[test]
fn cycle_error_messages_stay_numberless_in_source() {
    // 环路检测在 HTTP 层不可构造（id 服务端生成），报文面直接钉字面量：
    // 两条环路报文必须在场、且都不带内部编号。行为面由 store 级既有用例钉住
    // （cursor_lifecycle.rs::dependency_states_and_cycle_detection）。
    let src = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/routes/tasks.rs"));
    for needle in ["依赖关系构成环路，拒绝创建", "拆分子任务的依赖构成环路"]
    {
        let at = src
            .find(needle)
            .unwrap_or_else(|| panic!("环路报文应还在：{needle}"));
        let start = src[..at].rfind('"').expect("报文应是字符串字面量");
        let end = src[at..].find('"').map(|i| at + i).expect("报文应闭合");
        assert_numberless(&format!("环路报文「{needle}」"), &src[start + 1..end]);
    }
}

// ─────────────────────── 场景 5 正面：CLI --help 的豁免面 ───────────────────────

#[test]
fn cli_help_keeps_its_decision_annotations() {
    // 场景 5（AC-2 正面）：CLI `--help` 属文档 / 帮助面，编号照旧保留——
    // 把它当页面文案整库搜改抹掉 = 违反决策 199「注释 / 文档照旧」的边界，此条即红。
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_agent-pipeline"))
        .arg("--help")
        .output()
        .expect("应能拉起 --help");
    assert!(out.status.success(), "--help 应零退出：{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("（决策 334）"),
        "CLI help 的豁免面被误改了：{stdout}"
    );
}
