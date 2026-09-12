//! 应用状态与错误映射。

use std::sync::Arc;

use agentpipeline_core::config::Settings;
use agentpipeline_core::home::Home;
use agentpipeline_core::sse::SseBus;
use agentpipeline_core::storage::Store;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// resume 之后拉起执行的钩子。
///
/// 生产实现 spawn `run_executor`；L3 契约测试注入记录器，只验证"被触发了一次"
/// （决策 91 / §3 resume 防连点）。
pub type ResumeHook = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub home: Home,
    pub settings: Settings,
    pub sse: Arc<SseBus>,
    pub resume_hook: ResumeHook,
    /// 绑定端口，用于跨源防护的本机 origin 判定（决策 128）。
    pub port: u16,
}

impl AppState {
    pub fn new(store: Store, home: Home, settings: Settings, port: u16) -> Self {
        AppState {
            store,
            home,
            settings,
            sse: Arc::new(SseBus::default()),
            resume_hook: Arc::new(|_| {}),
            port,
        }
    }

    pub fn with_resume_hook(mut self, hook: ResumeHook) -> Self {
        self.resume_hook = hook;
        self
    }

    /// 本机允许的 origin 集合（决策 128）。
    pub fn allowed_origins(&self) -> Vec<String> {
        vec![
            format!("http://127.0.0.1:{}", self.port),
            format!("http://localhost:{}", self.port),
        ]
    }
}

/// API 错误 → HTTP 响应。
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        ApiError {
            status: StatusCode::BAD_REQUEST,
            message: msg.into(),
        }
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        ApiError {
            status: StatusCode::NOT_FOUND,
            message: msg.into(),
        }
    }

    pub fn conflict(msg: impl Into<String>) -> Self {
        ApiError {
            status: StatusCode::CONFLICT,
            message: msg.into(),
        }
    }

    pub fn forbidden(msg: impl Into<String>) -> Self {
        ApiError {
            status: StatusCode::FORBIDDEN,
            message: msg.into(),
        }
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: msg.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}

/// core 错误 → API 错误（`Conflict` 映射 409，决策 91）。
pub fn map_core_error(err: agentpipeline_core::Error) -> ApiError {
    use agentpipeline_core::Error as E;
    match err {
        E::Conflict(msg) => ApiError::conflict(msg),
        E::Task(msg) | E::Cursor(msg) => ApiError::not_found(msg),
        E::Validation(msg) | E::PolicyDenied(msg) => ApiError::bad_request(msg),
        other => ApiError::internal(other.to_string()),
    }
}

pub type ApiResult<T> = std::result::Result<T, ApiError>;
