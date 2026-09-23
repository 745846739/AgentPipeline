//! 应用状态与错误映射。

use std::sync::Arc;

use agentpipeline_core::agent::repo::Libgit2Repo;
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

/// 当前端口的**来源**（决策 213）。
///
/// 端口与绑定地址不同：它**运行期不可改**（决策 186 的「改绑不改端口」），故这里是常量而非
/// 共享单元。它存在的理由也不同于 `bind_source` 的「重启还算不算数」——而是**分享地址会不会
/// 随重启变**：`fallback` 意味着手机上存过的 URL 下次启动就作废，界面必须说出来，
/// 否则使用者只会看到一张打不开的书签。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PortSource {
    /// `--port` 显式指定（最高）。
    Startup,
    /// `config.toml` 的 `[server] port`（声明式默认；桌面壳走这一级）。
    Config,
    /// 首选端口被别的进程占着，退让到内核随机端口（决策 213）。
    Fallback,
}

impl PortSource {
    /// 面向用户的说话方式（`/server-info` 的 JSON 字段值，前端据此选文案）。
    pub fn as_str(self) -> &'static str {
        match self {
            PortSource::Startup => "startup",
            PortSource::Config => "config",
            PortSource::Fallback => "fallback",
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

/// 界面那一份来源仓名单的读数（决策 194）。
pub type MarketSnapshot = Vec<String>;

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
    /// 上面那个端口的来源（决策 213）：`fallback` = 首选端口被占用、退让到了内核随机端口，
    /// 此时手机上的旧书签会失效，分享页必须说出来。
    pub port_source: PortSource,
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
    /// 技能来源仓的访问层（决策 194——本 effort 唯一新增接缝，取代票 10 的 `MarketClient`）。
    ///
    /// **不是 `Option`**：GitHub 模式下它没有"没配就用不了"的形态（URL 由 `owner/repo` 拼，
    /// 不需要用户先填一个来源地址），所以总是有一个实现。放行与否由**仓名单**判定
    /// （[`AppState::market_repos`]），与这个实现无关。
    ///
    /// 生产在 `serve` 里注入 [`Libgit2Repo`]；L3 契约测试注入一个指向离线 smart HTTP
    /// fixture 的同一个实现，因此端点契约能在**完全离线**的前提下走**真 libgit2**。
    pub repo: Arc<Libgit2Repo>,
    /// `[market] github_repos` 归一后的仓名单（空名单 = 不从任何仓安装）。
    ///
    /// 启动时那一级；界面保存过之后由 [`AppState::market_override`] 盖过，读经
    /// [`AppState::market_repos`]（决策 187 的两级结构，决策 194 继承）。
    pub configured_repos: Vec<String>,
    /// 界面上的仓名单（决策 194）。`None` = 没保存过 → 用启动时那一级；
    /// `Some(vec![])` = 显式清空（= 不从任何仓安装），与"没保存过"是两回事。
    pub market_override: Arc<std::sync::RwLock<Option<MarketSnapshot>>>,
    /// 列表**钉住**的那一份 commit（决策 194）：仓名 → (commit, 取到它的时刻)。
    ///
    /// 进程内、不过期。它的全部意义是"两次列表之间不漂移，直到用户显式刷新"——
    /// 那是"看到的 = 装到的"的落点。不持久化：重启之后重新 `head()` 是对的
    /// （那时也没有"用户正看着的那一份"了）。
    ///
    /// 类型见 [`crate::routes::market::Listings`]。
    pub listings: crate::routes::market::Listings,
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
            port_source: PortSource::Config,
            bind_host: Arc::new(std::sync::RwLock::new("127.0.0.1".to_string())),
            bind_source: Arc::new(std::sync::RwLock::new(BindSource::Config)),
            rebind: None,
            extra_allowed_origins: Vec::new(),
            repo: Arc::new(Libgit2Repo::default()),
            configured_repos: Vec::new(),
            market_override: Arc::new(std::sync::RwLock::new(None)),
            listings: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            foreman: None,
        }
    }

    /// 界面保存过的那一份仓名单（决策 194）；没保存过时 `None`。
    ///
    /// 与 [`AppState::market_repos`] 的分工：这个读的是"界面那一级存不存在"，
    /// 后者读的是"现在生效的是哪一份"。`Some(vec![])` 与 `None` 必须分得开。
    pub fn market_override(&self) -> Option<MarketSnapshot> {
        self.market_override
            .read()
            .ok()
            .and_then(|guard| guard.clone())
    }

    /// 生效的仓名单（界面 > 配置文件，决策 187 / 194）。
    ///
    /// **放行判定读的是这一份**——它是本系统唯一的安全控制的输入，故只有这一个读法。
    pub fn market_repos(&self) -> Vec<String> {
        match self.market_override() {
            Some(repos) => repos,
            None => self.configured_repos.clone(),
        }
    }

    /// 生效的来源仓访问层（决策 194）。**不是 `Option`**：它没有"没配就用不了"的形态，
    /// 放行与否由仓名单判定，与这个实现无关。
    pub fn repo(&self) -> Arc<Libgit2Repo> {
        Arc::clone(&self.repo)
    }

    /// 装上界面保存的那一份（保存端点调用；决策 187）。
    pub fn set_market_override(&self, repos: Vec<String>) {
        if let Ok(mut guard) = self.market_override.write() {
            *guard = Some(repos);
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

    /// 端口的来源（决策 213）。它是常量，故直接返回。
    pub fn port_source(&self) -> PortSource {
        self.port_source
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

    /// 注入来源仓访问层与 `config.toml` 那一级的仓名单（决策 194）。
    ///
    /// 空名单是**合法状态**（= 不从任何仓安装），端点会给出可操作报文。
    pub fn with_repo(mut self, repo: Arc<Libgit2Repo>, repos: Vec<String>) -> Self {
        self.repo = repo;
        self.configured_repos = repos;
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

    /// 注入端口的来源（决策 213）。缺省 [`PortSource::Config`]；serve 在**退让**那条路上
    /// 传 [`PortSource::Fallback`]，分享页据此说明「手机上的旧地址这次失效了」。
    pub fn with_port_source(mut self, source: PortSource) -> Self {
        self.port_source = source;
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
    /// **机器可读的失败类别**（决策 194）：八类技能来源失败之一，供界面按类分支。
    ///
    /// 为什么必须有这一栏：八类里有几对**状态码相同而用户动作完全不同**
    /// （`repo_not_found` 与 `commit_not_found` 都是 404，前者要改仓名、后者要换 commit；
    /// `repo_not_allowed` 与 `download_too_large` 都是 400，一个去加白名单、一个换小仓）。
    /// 只靠状态码或匹配报文串，界面就只能把它们都渲染成"装不上"——那等于把分类白做了。
    pub kind: Option<String>,
}

impl ApiError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        ApiError {
            status: StatusCode::BAD_REQUEST,
            message: msg.into(),
            detail: None,
            kind: None,
        }
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        ApiError {
            status: StatusCode::NOT_FOUND,
            message: msg.into(),
            detail: None,
            kind: None,
        }
    }

    pub fn conflict(msg: impl Into<String>) -> Self {
        ApiError {
            status: StatusCode::CONFLICT,
            message: msg.into(),
            detail: None,
            kind: None,
        }
    }

    pub fn forbidden(msg: impl Into<String>) -> Self {
        ApiError {
            status: StatusCode::FORBIDDEN,
            message: msg.into(),
            detail: None,
            kind: None,
        }
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: msg.into(),
            detail: None,
            kind: None,
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
            kind: None,
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

    /// 附上机器可读的失败类别（见 [`ApiError::kind`]）。
    pub fn with_kind(mut self, kind: impl Into<String>) -> Self {
        let kind = kind.into();
        if !kind.is_empty() {
            self.kind = Some(kind);
        }
        self
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // `detail` / `kind` 为 `None` 时**不下发该字段**（而不是下发 null）：
        // 既有前端只读 `error`，这两个是加法的诊断栏。
        let mut body = json!({ "error": self.message });
        if let Some(detail) = &self.detail {
            body["detail"] = json!(detail);
        }
        if let Some(kind) = &self.kind {
            body["kind"] = json!(kind);
        }
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
