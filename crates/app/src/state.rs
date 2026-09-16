//! 应用状态与错误映射。

use std::sync::Arc;

use agentpipeline_core::agent::market::MarketClient;
use agentpipeline_core::config::Settings;
use agentpipeline_core::home::Home;
use agentpipeline_core::pipeline::{Executor, ForemanRunner};
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

/// 当前绑定地址的**来源**（决策 186）。
///
/// 优先级：命令行 / 环境变量 > 界面上的开关（DB）> `config.toml` 的 `[server] host`。
/// 界面要能说清「你按下这颗钮之后，重启还算不算数」，所以来源必须是可读的——
/// 只说「现在绑在 0.0.0.0」而不说「这是谁定的」，用户改配置文件时会撞上「改了没用」
/// 而完全不知道原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BindSource {
    /// `--host` / `AGENTPIPELINE_LAN` 等启动期显式覆盖（最高）。
    Startup,
    /// 「手机访问」页上的开关（住 DB，重启仍生效）。
    Settings,
    /// `config.toml` 的 `[server] host`（声明式默认）。
    Config,
}

impl BindSource {
    /// 面向用户的说话方式（`/server-info` 的 JSON 字段值，前端据此选文案）。
    pub fn as_str(self) -> &'static str {
        match self {
            BindSource::Startup => "startup",
            BindSource::Settings => "settings",
            BindSource::Config => "config",
        }
    }
}

/// 改绑监听地址的请求（决策 186）：端点把请求投给 [`crate::serve`] 里的监听器主管。
///
/// **为什么走消息而不是直接持有监听器**：监听器任务的所有权在 `serve` 手里，端点在
/// `AppState` 里。让状态反过来持有一个「能重建 router」的回调会形成 `AppState` →
/// 回调 → `AppState` 的环；一条通道把方向掰直，且这个方向可测（主管的判定与回滚
/// 不必经过 HTTP 就能钉住）。
pub struct RebindRequest {
    /// 目标绑定地址。`None` = 回到配置文件声明的值（界面上的「恢复配置文件的值」）。
    pub host: Option<String>,
    /// 结果：成功给出真实端口，失败给出面向用户的报文（调用方原样回显）。
    pub reply: tokio::sync::oneshot::Sender<std::result::Result<u16, String>>,
}

/// 改绑通道的发送端（`serve` 之外不存在，故 `AppState` 里是 `Option`）。
pub type RebindTx = tokio::sync::mpsc::Sender<RebindRequest>;

/// 界面那一份市场来源的读数（来源白名单 + 它的客户端，决策 187）。
pub type MarketSnapshot = (Vec<String>, Option<Arc<dyn MarketClient>>);

/// 界面上的技能市场来源（决策 187）：来源白名单 + 由它现搭的客户端。
///
/// 与启动时那份（`[market] allowed_sources`）是**两级**关系：界面保存过就用界面这份，
/// 清掉就回到配置文件。`sources` 为空表是**合法且有意义**的状态（= 不允许远程安装），
/// 与「没保存过」（`None`）必须分得开——见 `Store::market_sources_override`。
pub struct MarketRuntime {
    pub sources: Vec<String>,
    /// 空表时是 `None`（没有来源就没有客户端，端点给可操作报文而不是 panic）。
    pub client: Option<Arc<dyn MarketClient>>,
}

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
    /// 绑定端口，用于跨源防护的本机 origin 判定（决策 128）。**改绑不改端口**（决策 186），
    /// 故它是常量。
    pub port: u16,
    /// 实际绑定地址（决策 167）：`/server-info` 据它判断手机能否直连
    /// （仅回环绑定时分享页要给出「如何开启局域网访问」的指引）。
    ///
    /// **可写**（决策 186）：界面上的开关改绑之后，判定「是不是局域网形态」这件事
    /// （[`AppState::lan_mode`]，也就是配对令牌生不生效）必须跟着变——两处读同一个值，
    /// 故它不再是 `String` 而是一个共享单元。
    pub bind_host: Arc<std::sync::RwLock<String>>,
    /// 上面那个值的来源（决策 186），只影响界面文案与「重启还算不算数」。
    pub bind_source: Arc<std::sync::RwLock<BindSource>>,
    /// 改绑通道（决策 186）：`None` = 这个实例没起监听器（L3 契约测试），
    /// 此时 `POST /server/lan` 返回明确的 503 而不是假装改了。
    pub rebind: Option<RebindTx>,
    /// 配置 / CLI 注入的额外放行 origin（决策 157），与缺省本机集合合并。
    pub extra_allowed_origins: Vec<String>,
    /// 技能市场客户端（决策 172⑤，票 10——本 effort 唯一新增接缝）。
    ///
    /// **缺省 `None`**：没有它时市场端点返回明确的「未配置来源」错误，而不是 panic 或
    /// 静默成功。生产在 `serve` 里按 `[market] allowed_sources` 注入；L3 契约测试注入
    /// testkit 的 `FakeMarket`，因此端点契约能在**完全离线**的前提下被钉住。
    pub market: Option<Arc<dyn MarketClient>>,
    /// `[market] allowed_sources` 归一后的白名单（空白名单 = 不允许远程安装）。
    ///
    /// 启动时那一级；界面保存过之后由 [`AppState::market_override`] 盖过，读经
    /// [`AppState::market_sources`] / [`AppState::market_client`]（决策 187）。
    pub market_sources: Vec<String>,
    /// 界面上的市场来源（决策 187）。`None` = 没保存过 → 用启动时那一级。
    pub market_override: Arc<std::sync::RwLock<Option<MarketRuntime>>>,
    /// 值班长运行器（决策 182，票 01）。
    ///
    /// **缺省 `None`**：没有它时三个对讲台端点返回 503「未接线」，而不是 panic 或
    /// 一个静默的空会话。生产在 `serve` 里按 `ProductionLlm` 注入；契约测试注入
    /// testkit 的 `FakeAgent`，于是「空 home 也能对上话」「越权工具被拒」这些验收锚点
    /// 都能在**不打真网络**的前提下钉住。
    pub foreman: Option<Arc<ForemanRunner>>,
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
            bind_host: Arc::new(std::sync::RwLock::new("127.0.0.1".to_string())),
            bind_source: Arc::new(std::sync::RwLock::new(BindSource::Config)),
            rebind: None,
            extra_allowed_origins: Vec::new(),
            market: None,
            market_sources: Vec::new(),
            market_override: Arc::new(std::sync::RwLock::new(None)),
            foreman: None,
        }
    }

    /// 界面保存过的那一份市场来源（决策 187）；没保存过时 `None`。
    pub fn market_override(&self) -> Option<MarketSnapshot> {
        self.market_override.read().ok().and_then(|guard| {
            guard
                .as_ref()
                .map(|rt| (rt.sources.clone(), rt.client.clone()))
        })
    }

    /// 生效的市场来源（界面 > 配置文件，决策 187）。
    pub fn market_sources(&self) -> Vec<String> {
        match self.market_override() {
            Some((sources, _)) => sources,
            None => self.market_sources.clone(),
        }
    }

    /// 生效的市场客户端。界面保存过空表 → `None`（不允许远程安装）。
    pub fn market_client(&self) -> Option<Arc<dyn MarketClient>> {
        match self.market_override() {
            Some((_, client)) => client,
            None => self.market.clone(),
        }
    }

    /// 装上界面保存的那一份（保存端点调用；决策 187）。
    pub fn set_market_override(&self, sources: Vec<String>, client: Option<Arc<dyn MarketClient>>) {
        if let Ok(mut guard) = self.market_override.write() {
            *guard = Some(MarketRuntime { sources, client });
        }
    }

    /// 清掉界面那一份（回到配置文件，决策 187）。
    pub fn clear_market_override(&self) {
        if let Ok(mut guard) = self.market_override.write() {
            *guard = None;
        }
    }

    /// 注入改绑通道（决策 186，仅 `serve` 调用）。
    pub fn with_rebind(mut self, tx: RebindTx) -> Self {
        self.rebind = Some(tx);
        self
    }

    /// 当前绑定地址（读锁的**唯一**读法：`/server-info` 与 [`AppState::lan_mode`] 都必须
    /// 经它，否则「绑在 0.0.0.0」与「令牌是否生效」两套判断会各自漂移，决策 182㉖）。
    ///
    /// 读锁中毒（持锁线程 panic）时回落到回环：那是**更保守**的一侧（不放开令牌门），
    /// 比 panic 掉一个只读端点或静默当成「非回环」要好。
    pub fn bind_host(&self) -> String {
        self.bind_host
            .read()
            .map(|h| h.clone())
            .unwrap_or_else(|_| "127.0.0.1".to_string())
    }

    /// 绑定地址的来源（决策 186）。
    pub fn bind_source(&self) -> BindSource {
        self.bind_source
            .read()
            .map(|s| *s)
            .unwrap_or(BindSource::Config)
    }

    /// 改绑生效后由监听器主管回写（决策 186）。
    pub fn set_bind_host(&self, host: impl Into<String>, source: BindSource) {
        if let Ok(mut h) = self.bind_host.write() {
            *h = host.into();
        }
        if let Ok(mut s) = self.bind_source.write() {
            *s = source;
        }
    }

    /// 注入值班长运行器（决策 182，票 01）。
    pub fn with_foreman(mut self, runner: Arc<ForemanRunner>) -> Self {
        self.foreman = Some(runner);
        self
    }

    /// 注入技能市场客户端与放行来源（票 10）。
    ///
    /// `client` 为 `None` 是合法状态（白名单为空 = 不装远程技能），端点会给出可操作报文。
    pub fn with_market(
        mut self,
        client: Option<Arc<dyn MarketClient>>,
        sources: Vec<String>,
    ) -> Self {
        self.market = client;
        self.market_sources = sources;
        self
    }

    /// 注入实际绑定地址（决策 167）。serve 路径必须调用，否则 `/server-info`
    /// 会把局域网绑定误报为仅回环。
    ///
    /// 形态是 `Arc<RwLock<String>>`（决策 186）：界面上的开关能在运行时改绑，这个值必须
    /// 跟着变。注入时同时给出**来源**，否则界面无法解释「这颗钮按了算不算数」。
    pub fn with_bind_host(mut self, host: impl Into<String>) -> Self {
        let host = host.into();
        self.bind_host = Arc::new(std::sync::RwLock::new(host));
        self
    }

    /// 注入绑定地址的来源（决策 186）。缺省 [`BindSource::Config`]。
    pub fn with_bind_source(mut self, source: BindSource) -> Self {
        self.bind_source = Arc::new(std::sync::RwLock::new(source));
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

    /// 是否处于**局域网形态**（决策 182㉖㉗，票 07）：绑定的不是回环地址。
    ///
    /// 配对令牌**只在这个形态下生效**——默认回环形态是本机自己使用，零摩擦是它必须
    /// 保持的性质。把判定收在这一个具名谓词里，是因为「绑在哪里」这件事同时决定
    /// `/server-info` 的提示（决策 167）与令牌是否生效（票 07），两处必须同源。
    /// 界面上的开关改绑（决策 186）也会经这里生效——令牌门跟着绑定走。
    pub fn lan_mode(&self) -> bool {
        !crate::peer::is_loopback_bind(&self.bind_host())
    }
}

/// API 错误 → HTTP 响应。
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
    /// 原始诊断（不进 `message`，单独一栏给排查用）。
    ///
    /// 沿用「面向用户的话」与「诊断原始串」分开的既有姿态（决策见 `Error::LlmClassified`）：
    /// `message` 是中文可操作提示，`detail` 是期望/实际摘要、HTTP 状态这类技术细节。
    /// 响应体里作为额外字段下发（前端只读 `error`，故不破坏既有契约），用户截屏报障时
    /// 不用再去翻日志。
    pub detail: Option<String>,
}

impl ApiError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        ApiError {
            status: StatusCode::BAD_REQUEST,
            message: msg.into(),
            detail: None,
        }
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        ApiError {
            status: StatusCode::NOT_FOUND,
            message: msg.into(),
            detail: None,
        }
    }

    pub fn conflict(msg: impl Into<String>) -> Self {
        ApiError {
            status: StatusCode::CONFLICT,
            message: msg.into(),
            detail: None,
        }
    }

    pub fn forbidden(msg: impl Into<String>) -> Self {
        ApiError {
            status: StatusCode::FORBIDDEN,
            message: msg.into(),
            detail: None,
        }
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: msg.into(),
            detail: None,
        }
    }

    /// 下游（远程 registry）不可达（票 10）。
    ///
    /// 与 400 分开是因为**责任方不同**：400 是「你的请求有问题」，502 是「请求没问题，
    /// 但对面没应答」——用户该做的动作也不同（改配置 vs 稍后重试）。票面要求四类市场失败
    /// 互不混淆，这条让网络失败在 HTTP 状态码上也独立出来。
    pub fn bad_gateway(msg: impl Into<String>) -> Self {
        ApiError {
            status: StatusCode::BAD_GATEWAY,
            message: msg.into(),
            detail: None,
        }
    }

    /// 附上原始诊断（见 [`ApiError::detail`]）。
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        if !detail.is_empty() {
            self.detail = Some(detail);
        }
        self
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = match &self.detail {
            Some(detail) => json!({ "error": self.message, "detail": detail }),
            None => json!({ "error": self.message }),
        };
        (self.status, Json(body)).into_response()
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
