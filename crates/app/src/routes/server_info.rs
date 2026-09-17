//! 局域网分享端点（决策 167 / 186）：给出手机可访问的地址与对应二维码，并让「绑什么地址」
//! 这件事在界面上可改。
//!
//! 四个端点：
//! - `GET /server-info`：JSON，含绑定 host / 端口 / 候选局域网地址 / 当前是否
//!   仅回环绑定（决定分享页是否该提示「需要绑定 0.0.0.0」）/ **这个 host 是谁定的**
//!   （启动参数 / 界面设置 / 配置文件，决策 186）/ **这个端口是谁给的**（决策 213：
//!   `fallback` = 首选端口被占、退让到了内核随机端口，手机上的旧书签会因此失效）；
//! - `GET /server-info/qr.svg`：把指定 URL 渲染成 SVG 二维码，供分享页 `<img>`
//!   直接引用——前端不必引入 QR 库，也不用把二维码画进 canvas；
//! - `POST /server/lan`（决策 186）：**界面上的那颗钮**——把绑定切成全网卡或只回环，
//!   当场改绑（不必重启）并记住选择；
//! - `DELETE /server/lan`：清掉界面上的选择，回到启动参数 / 配置文件那一级。

use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::lan;
use crate::peer::{is_loopback_bind, peer_is_loopback};
use crate::state::{ApiError, AppState, RebindRequest};

/// `POST /server/lan` 改绑的等待上限。
///
/// 换绑要停掉监听器（最长一个优雅窗口）再绑新的，这个等待**允许超时**：触发改绑的请求
/// 就在被切断的那条连接上，很多情况下应答根本到不了客户端（前端据此重读
/// `/server-info` 拿真值，见 [`switch_bind`]）。
const REBIND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// `GET /server-info` 的响应体。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    /// 实际绑定地址（启动参数 / 界面设置 / `[server] host` 三级，决策 186）。
    pub host: String,
    /// 实际绑定端口（端口 0 时是内核分配的真实端口）。
    pub port: u16,
    /// 仅绑定回环时为 true——此时手机连不上，分享页需给出「怎么开」的动作。
    pub loopback_only: bool,
    /// `host` 是谁定的（`startup` / `settings` / `config`，决策 186）。
    pub bind_source: String,
    /// 端口是谁给的（`startup` / `config` / `fallback`，决策 213）。
    ///
    /// `fallback` 是唯一需要界面出声的一档：首选端口被别的进程占着，这个端口是内核
    /// 临时给的，**重启后会变**——手机上存过的地址这次就是打不开的原因。
    pub port_source: String,
    /// 候选局域网地址，已按推荐度排序（`lan::rank_ipv4`）。
    pub addresses: Vec<AddressEntry>,
}

/// 响应体里的单个地址项。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddressEntry {
    /// 网卡名（诊断用）。
    pub interface: String,
    /// 手机可直接访问的完整 URL（含端口）。
    pub url: String,
    /// 是否被判定为大概率可连（分享页据此把首选放大显示）。
    pub preferred: bool,
}

/// `GET /server-info`。
///
/// 地址按**绑定 host** 分两种语义：绑 `0.0.0.0`（或具体网卡 IP）时枚举的局域网
/// 地址即真实入口；绑回环时枚举结果对手机没有意义，但仍返回（`loopback_only = true`），
/// 让分享页能显示「为什么打不开」而不是空列表。
pub async fn info(State(state): State<AppState>) -> Response {
    Json(server_info(&state)).into_response()
}

/// 组装 `/server-info`。读端点与两个改绑端点的响应共用它——三处各写一份口径必然漂移。
fn server_info(state: &AppState) -> ServerInfo {
    let host = state.bind_host();
    ServerInfo {
        loopback_only: is_loopback_bind(&host),
        bind_source: state.bind_source().as_str().to_string(),
        port_source: state.port_source().as_str().to_string(),
        host,
        port: state.port,
        addresses: build_addresses(&lan::lan_addresses(), state.port),
    }
}

/// `POST /server/lan` 的请求体。
#[derive(Debug, Deserialize)]
pub struct LanBody {
    /// `true` = 绑全网卡（`0.0.0.0`，手机可访问）；`false` = 只绑回环（仅本机）。
    pub enabled: bool,
}

/// 本次请求是否来自回环（票 07 / 决策 186 的同一套判定）。
///
/// 做成提取器而不是在处理器里读 `Request`：`Request` 会把整个请求（含 body）吃掉，
/// 与 `Json<LanBody>` 不能共存（axum 只允许最后一个提取器消费 body）。这个提取器只看
/// `parts.extensions`，**永不拒绝**，且沿用 [`peer_is_loopback`] 的缺省语义
/// （无来源信息 = 隐含本机，那是 L3 契约测试的形态）。
pub struct PeerLoopback(bool);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for PeerLoopback {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        Ok(PeerLoopback(peer_is_loopback(&parts.extensions)))
    }
}

/// `POST /server/lan`（决策 186）：界面上的「绑定全网卡 / 只绑本机」两颗钮。
///
/// **只允许回环来源调用**。这是全站唯一一个能把服务暴露到局域网的入口：局域网里任何一台
/// 设备若能调它，配对令牌（票 07）就白设了——先把它打开，再从自己的机器上来。判定沿用
/// 票 07 的同一套对端地址，不新开一条通路。
///
/// **应答可能到不了**：改绑会切断当前所有连接，包括发出这次请求的那条。故调用方（前端）
/// 看到传输错误时**不得**当作失败——正确的读法是重读 `GET /server-info`：改绑确实完成时，
/// 那个读会给出新状态。
pub async fn set_lan(
    State(state): State<AppState>,
    PeerLoopback(loopback): PeerLoopback,
    Json(body): Json<LanBody>,
) -> Response {
    if !loopback {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": "只有本机可以打开或关闭局域网访问：从局域网里放行它，\
                          等于让同网段的任何设备绕过配对令牌",
            })),
        )
            .into_response();
    }
    // 没有监听器主管 = 这个实例没起监听器（契约测试的 in-process router）。
    // 报 503 而不是假装改了：能力不在这台机器上，责任方要说清。
    let Some(tx) = state.rebind.clone() else {
        return unbound_response();
    };

    let target = if body.enabled { "0.0.0.0" } else { "127.0.0.1" };
    switch_bind(&state, &tx, Some(target.to_string())).await
}

/// `DELETE /server/lan`（决策 186）：清掉界面上的选择，回到启动参数 / 配置文件那一级。
///
/// 没有这个口，只按过一次钮的用户就再也回不到配置文件那条路上——他改 `[server] host`
/// 会发现「改了没用」，而原因不在他改的那个地方。
pub async fn clear_lan(
    State(state): State<AppState>,
    PeerLoopback(loopback): PeerLoopback,
) -> Response {
    if !loopback {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": "只有本机可以修改绑定地址" })),
        )
            .into_response();
    }
    let Some(tx) = state.rebind.clone() else {
        return unbound_response();
    };
    switch_bind(&state, &tx, None).await
}

/// 这个实例没有监听器可改绑时的统一应答（决策 186）。
fn unbound_response() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({
            "error": "改绑监听地址未接线：该实例没有起监听器，\
                      请用 agent-pipeline serve 或桌面应用",
        })),
    )
        .into_response()
}

/// 把一次改绑请求投给监听器主管并等结果，成功时返回**改完之后**的 `/server-info`。
///
/// `target = None` 表示「清除界面设置」。持久化由主管在**成功路径**上做（见 `serve`）：
/// 点了没生效却在下一次启动生效，是这里最不该出现的一种状态。
async fn switch_bind(
    state: &AppState,
    tx: &crate::state::RebindTx,
    target: Option<String>,
) -> Response {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    if tx
        .send(RebindRequest {
            host: target,
            reply: reply_tx,
        })
        .await
        .is_err()
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "监听器主管已停止，改绑未生效" })),
        )
            .into_response();
    }
    match tokio::time::timeout(REBIND_TIMEOUT, reply_rx).await {
        Ok(Ok(Ok(_port))) => Json(server_info(state)).into_response(),
        // 绑不上、且已回滚：这是一条**确定的**失败，400 + 原因（用户该做的是换一个钮，
        // 或者去查端口是不是被别的进程占了）。
        Ok(Ok(Err(message))) => ApiError::bad_request(message).into_response(),
        // 超时或应答通道断了：本次结果对调用方是未知的，但**状态是可读的**——前端重读
        // `/server-info` 即可。故报文只说「去读一眼」，不编造成功也不编造失败。
        _ => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({
                "error": "改绑已提交，但应答没能回来（多半是连接被改绑切断）。\
                          请重新读取服务状态确认。",
                "pending": true,
            })),
        )
            .into_response(),
    }
}

/// `GET /server-info/qr.svg?url=...` 的查询参数。
#[derive(Debug, Deserialize)]
pub struct QrQuery {
    /// 要编码的地址。缺省时用首个推荐地址，仍无则 400。
    #[serde(default)]
    pub url: Option<String>,
}

/// `GET /server-info/qr.svg`：把地址渲染成 SVG。
///
/// **只允许编码本服务自己的地址**（决策 167）：这个端点若接受任意 URL，就等于
/// 给局域网里的任何人一个「用你的服务生成任意二维码」的图床，且分享页的
/// `<img src>` 会把 URL 写进对方日志。
///
/// 校验口径是 **origin 白名单**（票 07 放宽）：请求 URL 的 `scheme://host:port`
/// 必须落在候选集合内，**允许追加 query**（配对 URL 就是 `{base}/?pair={token}`），
/// 也允许追加 path——它们都离不开这个 origin，二维码仍只编码本服务地址。
pub async fn qr_svg(State(state): State<AppState>, Query(query): Query<QrQuery>) -> Response {
    let allowed = allowed_qr_urls(&state);
    let target = match resolve_qr_target(query.url, &allowed) {
        Ok(url) => url,
        Err((status, message)) => return (status, message).into_response(),
    };

    match render_svg(&target) {
        Ok(svg) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "image/svg+xml; charset=utf-8"),
                // 二维码随端口 / 地址变化，禁缓存避免分享页显示旧地址
                (header::CACHE_CONTROL, "no-store"),
            ],
            svg,
        )
            .into_response(),
        Err(e) => {
            tracing::error!(error = %e, "二维码渲染失败");
            (StatusCode::INTERNAL_SERVER_ERROR, "二维码渲染失败").into_response()
        }
    }
}

/// 扫码配对 URL 的形状（票 07）：`{base}/?pair={token}`。
///
/// 唯一事实源：分享页（前端）要把手机引到带令牌的地址，后端要把同一形状的 URL 编成
/// 二维码——参数名与分隔符各写各的就会漂移（前端读 `pair`、后端发 `token` 这类）。
/// 令牌是 Crockford base32（URL 安全字符），故直接拼接无需转义。
pub fn pairing_url(base: &str, token: &str) -> String {
    format!("{base}/?pair={token}")
}

/// URL 的 origin（`scheme://host:port`）。无 scheme / 无 authority 时 `None`。
///
/// 手写而不是引入 `url` crate：本项目不新增依赖，而这里只需要 origin 这一段。
/// authority 取到第一个 `/ ? #` 为止，故 `host:port@evil.example` 这种带 userinfo 的
/// 伪装串整体落不进白名单（白名单里只有纯 `host:port`）。
fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme.is_empty() {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{authority}"))
}

/// 待编码 URL 是否放行：origin 落在白名单内即可（票 07 放宽，允许 query / path）。
fn qr_url_allowed(url: &str, allowed: &[String]) -> bool {
    match origin_of(url) {
        Some(origin) => allowed.contains(&origin),
        None => false,
    }
}

/// 从请求参数选出要编码的地址。`Err` 是 (状态码, 面向用户的报文)。
fn resolve_qr_target(
    requested: Option<String>,
    allowed: &[String],
) -> Result<String, (StatusCode, &'static str)> {
    match requested {
        Some(url) => {
            if qr_url_allowed(&url, allowed) {
                // 原样返回（不只是 origin）：query 里的配对令牌必须进二维码
                Ok(url)
            } else {
                Err((StatusCode::BAD_REQUEST, "二维码只允许编码本服务的访问地址"))
            }
        }
        None => allowed
            .first()
            .cloned()
            .ok_or((StatusCode::BAD_REQUEST, "没有可用的访问地址")),
    }
}

/// 把候选地址转成响应项（含端口拼装）。抽成纯函数便于单测。
fn build_addresses(addresses: &[lan::LanAddress], port: u16) -> Vec<AddressEntry> {
    addresses
        .iter()
        .map(|a| AddressEntry {
            interface: a.interface.clone(),
            url: format!("http://{}:{port}", a.ip),
            preferred: a.preferred,
        })
        .collect()
}

/// 允许被编码成二维码的 origin 集合：局域网候选地址 + 回环地址。
///
/// 返回的字符串本身就是 origin 形态（`scheme://host:port`），既可直接作为
/// [`resolve_qr_target`] 的白名单，也可在「未指定 url」时直接当默认地址渲染。
/// 回环也在集合内，是为了让「本机打开验证」这件事能复用同一端点。
fn allowed_qr_urls(state: &AppState) -> Vec<String> {
    let mut urls: Vec<String> = build_addresses(&lan::lan_addresses(), state.port)
        .into_iter()
        .map(|a| a.url)
        .collect();
    urls.push(format!("http://127.0.0.1:{}", state.port));
    urls.push(format!("http://localhost:{}", state.port));
    urls
}

/// 用 `qrcode` 渲染 SVG（M 级纠错——分享页二维码会被手机摄像头隔着反光扫，
/// 中等纠错是清晰度与容错的经验平衡点）。
fn render_svg(data: &str) -> Result<String, qrcode::types::QrError> {
    use qrcode::render::svg;
    let code = qrcode::QrCode::with_error_correction_level(data, qrcode::EcLevel::M)?;
    Ok(code.render::<svg::Color>().min_dimensions(240, 240).build())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lan::LanAddress;

    fn addr(name: &str, ip: &str, preferred: bool) -> LanAddress {
        LanAddress {
            interface: name.to_string(),
            ip: ip.parse().unwrap(),
            preferred,
        }
    }

    #[test]
    fn addresses_carry_port_in_url() {
        let built = build_addresses(&[addr("en0", "192.168.1.10", true)], 8787);
        assert_eq!(built[0].url, "http://192.168.1.10:8787");
        assert!(built[0].preferred);
    }

    #[test]
    fn loopback_bind_detection_covers_forms() {
        assert!(is_loopback_bind("127.0.0.1"));
        assert!(is_loopback_bind("localhost"));
        assert!(is_loopback_bind("::1"));
        assert!(!is_loopback_bind("0.0.0.0"), "0.0.0.0 是全网卡绑定，非回环");
        assert!(!is_loopback_bind("192.168.1.10"));
    }

    #[test]
    fn qr_renders_for_a_typical_url() {
        let svg = render_svg("http://192.168.1.10:8787").unwrap();
        assert!(svg.starts_with("<?xml"), "应是 SVG：{svg:.60}");
        assert!(svg.contains("<path"), "应含二维码模块路径");
    }

    #[test]
    fn qr_output_is_large_enough_to_scan() {
        // 默认 8px/模块对手机屏偏小，min_dimensions 保证至少 240px
        let svg = render_svg("http://192.168.1.10:8787").unwrap();
        let width: u32 = svg
            .split("width=\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .and_then(|s| s.parse().ok())
            .expect("SVG 应含 width 属性");
        assert!(width >= 240, "二维码宽度应 ≥ 240px，实际 {width}");
    }

    #[test]
    fn origin_of_extracts_scheme_host_port() {
        assert_eq!(
            origin_of("http://192.168.1.10:8787"),
            Some("http://192.168.1.10:8787".to_string())
        );
        assert_eq!(
            origin_of("http://192.168.1.10:8787/"),
            Some("http://192.168.1.10:8787".to_string())
        );
        assert_eq!(
            origin_of("http://192.168.1.10:8787/?pair=abc"),
            Some("http://192.168.1.10:8787".to_string())
        );
        assert_eq!(
            origin_of("http://192.168.1.10:8787?pair=abc"),
            Some("http://192.168.1.10:8787".to_string())
        );
        assert_eq!(origin_of("not-a-url"), None);
        assert_eq!(origin_of("http://"), None);
    }

    #[test]
    fn qr_origin_whitelist_allows_query_but_not_other_origin() {
        let allowed = vec![
            "http://192.168.1.10:8787".to_string(),
            "http://127.0.0.1:8787".to_string(),
        ];
        assert!(qr_url_allowed("http://192.168.1.10:8787", &allowed));
        assert!(
            qr_url_allowed("http://192.168.1.10:8787/?pair=tok", &allowed),
            "白名单 origin 追加 query 必须放行（票 07 放宽）"
        );
        assert!(
            !qr_url_allowed("http://192.168.1.10:9999/?pair=tok", &allowed),
            "端口不同就是不同 origin"
        );
        assert!(!qr_url_allowed("http://evil.example/?pair=tok", &allowed));
        assert!(
            !qr_url_allowed("http://192.168.1.10:8787.evil.example/?pair=tok", &allowed),
            "端口后缀伪装不成立"
        );
        assert!(
            !qr_url_allowed("http://user@192.168.1.10:8787/", &allowed),
            "带 userinfo 的 authority 落不进白名单"
        );
    }

    #[test]
    fn resolve_qr_target_preserves_the_query_string() {
        let allowed = vec!["http://192.168.1.10:8787".to_string()];
        let paired = pairing_url("http://192.168.1.10:8787", "01HZYTOKEN");
        assert_eq!(
            resolve_qr_target(Some(paired), &allowed).unwrap(),
            "http://192.168.1.10:8787/?pair=01HZYTOKEN",
            "返回的是原样 URL，配对令牌必须进二维码"
        );
        assert!(resolve_qr_target(Some("http://evil.example/?pair=x".into()), &allowed).is_err());
        assert_eq!(
            resolve_qr_target(None, &allowed).unwrap(),
            "http://192.168.1.10:8787"
        );
    }

    #[test]
    fn pairing_url_shape_is_fixed() {
        assert_eq!(
            pairing_url("http://192.168.1.10:8787", "ABC123"),
            "http://192.168.1.10:8787/?pair=ABC123"
        );
    }

    #[test]
    fn preferred_flag_survives_serialization() {
        let info = ServerInfo {
            host: "0.0.0.0".into(),
            port: 8787,
            loopback_only: false,
            bind_source: "settings".into(),
            port_source: "fallback".into(),
            addresses: vec![AddressEntry {
                interface: "en0".into(),
                url: "http://192.168.1.10:8787".into(),
                preferred: true,
            }],
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["addresses"][0]["preferred"], true);
        assert_eq!(json["loopback_only"], false);
        assert_eq!(json["port"], 8787);
        assert_eq!(json["port_source"], "fallback");
    }
}
