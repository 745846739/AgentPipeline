//! axum API 层（docs/implementation.md §11.7；契约测试见 tests/）。
//!
//! 设计要点：
//! - 路由构建与二进制启动分离——L3 测试用 tower oneshot 直接打 in-process router，不 spawn 二进制
//!   （决策 144）；
//! - 跨源防护中间件（决策 128）：只拦写请求；SSE 是纯 GET，不受影响；
//! - `api_key` 读接口只回显 `***`（决策 112）。

pub mod routes;
pub mod state;
pub mod stream;

pub use state::{ApiError, ApiResult, AppState, ResumeHook};
pub use stream::cross_origin_guard;

use axum::routing::{get, patch, post};
use axum::Router;

/// 构建完整 router。
pub fn build_router(state: AppState) -> Router {
    Router::new()
        // ── 项目 ──
        .route(
            "/projects",
            get(routes::projects::list).post(routes::projects::create),
        )
        .route(
            "/projects/{id}",
            patch(routes::projects::patch).delete(routes::projects::delete),
        )
        .route("/projects/analyze", post(routes::projects::analyze))
        .route("/projects/{id}/analysis", get(routes::projects::analysis))
        // ── 任务 ──
        .route(
            "/tasks",
            get(routes::tasks::list).post(routes::tasks::create),
        )
        .route("/tasks/{id}", get(routes::tasks::detail))
        .route("/tasks/{id}/stream", get(routes::tasks::stream))
        .route("/tasks/{id}/flow", get(routes::tasks::flow))
        .route("/tasks/{id}/metrics", get(routes::tasks::metrics))
        .route("/tasks/{id}/resume", post(routes::tasks::resume))
        .route("/tasks/{id}/retry", post(routes::tasks::retry))
        .route("/tasks/{id}/cancel", post(routes::tasks::cancel))
        .route("/tasks/{id}/archive", post(routes::tasks::archive))
        .route("/tasks/{id}/split", post(routes::tasks::split))
        .route(
            "/tasks/{id}/model-override",
            post(routes::tasks::model_override),
        )
        .route("/tasks/{id}/review", post(routes::tasks::review))
        .route(
            "/tasks/{id}/merge/decision",
            post(routes::tasks::merge_decision),
        )
        .route("/tasks/{id}/files/{*path}", get(routes::tasks::file))
        .route(
            "/tasks/{id}/conversations",
            get(routes::tasks::conversations),
        )
        .route(
            "/tasks/{id}/conversations/{run_id}",
            get(routes::tasks::conversation),
        )
        .route("/tasks/{id}/commands", get(routes::tasks::commands))
        .route("/tasks/{id}/commands/{cmd_id}", get(routes::tasks::command))
        .route(
            "/tasks/{id}/commands/{cmd_id}/output",
            get(routes::tasks::command_output),
        )
        // ── provider ──
        .route(
            "/providers",
            get(routes::providers::list).post(routes::providers::create),
        )
        .route(
            "/providers/{id}",
            patch(routes::providers::patch).delete(routes::providers::delete),
        )
        // ── 全局指标 ──
        .route("/metrics", get(routes::tasks::global_metrics))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            stream::cross_origin_guard,
        ))
        .with_state(state)
}
