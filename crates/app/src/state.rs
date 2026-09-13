//! 应用状态与错误映射。

use std::sync::Arc;

use agentpipeline_core::config::Settings;
use agentpipeline_core::home::Home;
use agentpipeline_core::pipeline::Executor;
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
    /// 生产执行器：`POST /projects/analyze` 的 `project_analysis` 伪阶段（决策 48 / 130⑦）
    /// 需要它。L3 契约测试不注入（`None`）时退化为纯代码探测。
    pub executor: Option<Arc<Executor>>,
    /// 绑定端口，用于跨源防护的本机 origin 判定（决策 128）。
    pub port: u16,
    /// 实际绑定地址（决策 167）：`/server-info` 据它判断手机能否直连
    /// （仅回环绑定时分享页要给出「如何开启局域网访问」的指引）。
    pub bind_host: String,
    /// 配置 / CLI 注入的额外放行 origin（决策 157），与缺省本机集合合并。
    pub extra_allowed_origins: Vec<String>,
}

impl AppState {
    pub fn new(store: Store, home: Home, settings: Settings, port: u16) -> Self {
        AppState {
            store,
            home,
            settings,
            sse: Arc::new(SseBus::default()),
            resume_hook: Arc::new(|_| {}),
            executor: None,
            port,
            bind_host: "127.0.0.1".to_string(),
            extra_allowed_origins: Vec::new(),
        }
    }

    /// 注入实际绑定地址（决策 167）。serve 路径必须调用，否则 `/server-info`
    /// 会把局域网绑定误报为仅回环。
    pub fn with_bind_host(mut self, host: impl Into<String>) -> Self {
        self.bind_host = host.into();
        self
    }

    pub fn with_resume_hook(mut self, hook: ResumeHook) -> Self {
        self.resume_hook = hook;
        self
    }

    /// 注入生产执行器（`project_analysis` 伪阶段用）。
    pub fn with_executor(mut self, executor: Arc<Executor>) -> Self {
        self.executor = Some(executor);
        self
    }

    /// 复用外部 SSE 总线（票 17：scheduler / executor 的推送必须与端点的
    /// `/tasks/{id}/stream` 订阅同源，否则前端收不到节点事件）。
    pub fn with_sse(mut self, sse: Arc<SseBus>) -> Self {
        self.sse = sse;
        self
    }

    /// 注入额外放行的跨源写 origin（决策 157：局域网 / 桌面壳显式扩权）。
    /// 传入值应已过 `normalize_origin` 归一。
    pub fn with_allowed_origins(mut self, origins: Vec<String>) -> Self {
        self.extra_allowed_origins = origins;
        self
    }

    /// 本机允许的 origin 集合（决策 128）：缺省本机两个 + 配置 / CLI 扩权（决策 157）。
    pub fn allowed_origins(&self) -> Vec<String> {
        let mut origins = vec![
            format!("http://127.0.0.1:{}", self.port),
            format!("http://localhost:{}", self.port),
        ];
        origins.extend(self.extra_allowed_origins.iter().cloned());
        origins
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
