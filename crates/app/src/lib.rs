//! axum API 层（docs/implementation.md §11.7；契约测试见 tests/）。
//!
//! 设计要点：
//! - 路由构建与二进制启动分离——L3 测试用 tower oneshot 直接打 in-process router，不 spawn 二进制
//!   （决策 144）；
//! - 跨源防护中间件（决策 128）：只拦写请求；SSE 是纯 GET，不受影响；
//! - 前端 dist 由 build.rs 内嵌并同源托管（决策 155）；dist 缺失时退化为构建提示页；
//! - `/server-info` 暴露局域网访问地址与二维码（决策 167），供手机扫码接入；
//! - `api_key` 读接口只回显 `***`（决策 112）。

pub mod assets;
pub mod lan;
pub mod routes;
pub mod runtime;
pub mod serve;
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
        .route(
            "/tasks/{id}/conversations/{run_id}/messages",
            get(routes::tasks::conversation_messages),
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
        // 连通性探针（决策 160）：建任务前验证密钥 / 模型 / 地址
        .route("/providers/test", post(routes::providers::test))
        // ── 阶段级 agent 配置（决策 22 / 46 / 66）──
        .route("/stage-configs", get(routes::stage_configs::list))
        .route(
            "/stage-configs/{stage}",
            axum::routing::put(routes::stage_configs::put).delete(routes::stage_configs::delete),
        )
        // ── 全局指标 ──
        .route("/metrics", get(routes::tasks::global_metrics))
        // ── 技能市场（决策 172⑤，票 09）：本地导入 / 目录扫描 / 卸载。全程离线 ──
        // 子 router 自带 state（import 路由要单独放宽请求体上限），故先 merge 再进防护层。
        .merge(routes::skills::routes(state.clone()))
        // ── 技能市场：远程 registry（票 10）。来源白名单默认空 = 不允许远程安装；
        // 客户端由 AppState 注入（生产 HttpMarketClient / 测试 FakeMarket），故契约测试离线 ──
        .merge(routes::market::routes(state.clone()))
        // ── 服务自述与局域网分享（决策 167）：纯 GET，无状态变更 ──
        .route("/server-info", get(routes::server_info::info))
        .route("/server-info/qr.svg", get(routes::server_info::qr_svg))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            stream::cross_origin_guard,
        ))
        // 前端静态资源同源托管（决策 155）：放在防护层之后注册——全 GET/HEAD，
        // 防护只拦写请求，静态路由不进跨源矩阵。
        .merge(assets::static_routes())
        .with_state(state)
}
