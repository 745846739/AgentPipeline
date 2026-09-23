//! 请求来源地址（决策 182 / 票 06）：把 axum 的连接信息归一成处理器可读的扩展。
//!
//! 纯前置模块：不改变任何既有接口与行为，只为票 07 的「仅回环可读」提供判定依据。
//! 中间件由 [`crate::build_router`] 注册，且必须排在跨源防护（决策 128）**之前**——
//! 票 07 的配对令牌要按来源地址决定放行。

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, Request};
use axum::http::Extensions;
use axum::middleware::Next;
use axum::response::Response;

/// 请求来源地址。经 [`peer_address`] 从 [`ConnectInfo`] 归一而来。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerAddr(pub SocketAddr);

impl PeerAddr {
    /// 是否来自本机回环（`127.0.0.0/8` 与 `::1` 都算）。
    pub fn is_loopback(&self) -> bool {
        self.0.ip().is_loopback()
    }
}

/// 中间件：把 axum 的 [`ConnectInfo`] 归一成 [`PeerAddr`] 请求扩展。
///
/// `ConnectInfo` 只在服务经 `into_make_service_with_connect_info` 启动时才在请求扩展里
/// 出现（见 [`crate::serve::into_serving_service`]）；L3 契约测试走 `tower::ServiceExt::oneshot`，
/// 没有连接信息——此时不插入任何扩展，交由 [`peer_is_loopback`] 的缺省语义兜底。
pub async fn peer_address(mut request: Request, next: Next) -> Response {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .copied();
    if let Some(ConnectInfo(addr)) = peer {
        request.extensions_mut().insert(PeerAddr(addr));
    }
    next.run(request).await
}

/// 处理器的唯一读法：**缺省视为回环**。
///
/// 这个缺省是承重的、不是顺手写法：本改动之前服务只绑回环，且 L3 契约测试用 tower
/// oneshot（无 `ConnectInfo`），一切请求都隐含是本机。若缺省判为非回环，全部既有契约
/// 用例会集体变红，「既有接口与行为一律不变」就不成立。故只有**明确观测到的**非回环
/// 地址才返回 `false`，「没有来源信息」与「来源是回环」归为同一类。
pub fn peer_is_loopback(extensions: &Extensions) -> bool {
    match extensions.get::<PeerAddr>() {
        Some(peer) => peer.is_loopback(),
        None => true,
    }
}

/// 绑定 host 是否为回环。
///
/// 两个下游语义都挂在它上面：`GET /server-info` 的 `loopback_only`（决定分享页是否提示
/// 「需重启并绑定 0.0.0.0」，决策 167）与票 07 的 `AppState::lan_mode()`（决定配对令牌
/// 是否生效）。故它必须与对端地址判定住在同一处，否则「绑定在 0.0.0.0」与「请求来自
/// 局域网」两套判断会各自漂移。
///
/// 判据走 `host_policy`（决策 246 的唯一实现）：原先是本文件的一份 `matches!`，
/// 大小写敏感（`LOCALHOST` 与另外几处判得不一样），收敛后 `LOCALHOST` 四处一个答案。
pub fn is_loopback_bind(host: &str) -> bool {
    agentpipeline_core::host_policy::is_loopback(host)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::extract::Extension;
    use axum::http::Request as HttpRequest;
    use axum::http::StatusCode;
    use axum::routing::get;
    use axum::Router;
    use std::net::IpAddr;
    use tower::ServiceExt;

    /// 探针：回显处理器观测到的对端地址（票 06 的验收断言本体）。
    async fn echo_peer(Extension(peer): Extension<PeerAddr>) -> String {
        peer.0.to_string()
    }

    /// 探针：按 [`peer_is_loopback`] 的唯一读法判定本次请求是否回环。
    async fn echo_loopback(request: Request) -> String {
        peer_is_loopback(request.extensions()).to_string()
    }

    /// 与 `build_router` 同一挂法：先登记路由，再挂中间件。
    fn probe_router() -> Router {
        Router::new()
            .route("/probe", get(echo_peer))
            .route("/loopback", get(echo_loopback))
            .layer(axum::middleware::from_fn(peer_address))
    }

    async fn body_text(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        String::from_utf8_lossy(&bytes).to_string()
    }

    fn get_req(uri: &str) -> Request {
        HttpRequest::builder().uri(uri).body(Body::empty()).unwrap()
    }

    fn addr(ip: &str, port: u16) -> SocketAddr {
        SocketAddr::from((ip.parse::<IpAddr>().unwrap(), port))
    }

    #[tokio::test]
    async fn handler_observes_the_peer_address_from_connect_info() {
        // 票 06 的验收断言：处理器拿到的来源地址 == 注入的 ConnectInfo 地址。
        let mut request = get_req("/probe");
        request
            .extensions_mut()
            .insert(ConnectInfo(addr("127.0.0.1", 51234)));

        let response = probe_router().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            body_text(response).await,
            "127.0.0.1:51234",
            "处理器必须能观察到来源地址"
        );
    }

    #[tokio::test]
    async fn absent_connect_info_defaults_to_loopback() {
        // 无 ConnectInfo = L3 契约测试的 tower oneshot 形态；必须仍判回环，
        // 否则全部既有契约用例集体变红（见 peer_is_loopback 的约束注释）。
        let response = probe_router().oneshot(get_req("/loopback")).await.unwrap();
        assert_eq!(body_text(response).await, "true");
    }

    #[tokio::test]
    async fn lan_peer_reads_as_non_loopback() {
        // 模拟局域网客户端（票 07 要拦的正是这一类）。
        let mut request = get_req("/loopback");
        request
            .extensions_mut()
            .insert(ConnectInfo(addr("192.168.1.50", 40000)));

        let response = probe_router().oneshot(request).await.unwrap();
        assert_eq!(body_text(response).await, "false");
    }

    #[test]
    fn peer_addr_loopback_detection() {
        assert!(PeerAddr(addr("127.0.0.1", 51234)).is_loopback());
        assert!(PeerAddr(addr("::1", 51234)).is_loopback());
        assert!(
            PeerAddr(addr("127.0.0.2", 51234)).is_loopback(),
            "127.0.0.0/8 整段都是回环"
        );
        assert!(!PeerAddr(addr("192.168.1.50", 40000)).is_loopback());
        assert!(!PeerAddr(addr("10.0.0.1", 40000)).is_loopback());
    }

    #[test]
    fn is_loopback_bind_covers_forms() {
        assert!(is_loopback_bind("127.0.0.1"));
        assert!(is_loopback_bind("localhost"));
        assert!(is_loopback_bind("::1"));
        assert!(is_loopback_bind("127.0.0.2"));
        // 大小写与尾点与另外两处（出口策略 / 技能来源仓）同一个答案（决策 246）
        assert!(is_loopback_bind("LOCALHOST"));
        assert!(is_loopback_bind("localhost."));
        assert!(!is_loopback_bind("0.0.0.0"), "0.0.0.0 是全网卡绑定，非回环");
        assert!(!is_loopback_bind("192.168.1.10"));
    }

    #[test]
    fn peer_is_loopback_defaults_to_true_without_the_extension() {
        assert!(
            peer_is_loopback(&Extensions::new()),
            "无来源信息 = 隐含本机（见 peer_is_loopback 的约束注释）"
        );
    }

    #[test]
    fn peer_is_loopback_reads_the_extension_when_present() {
        let mut extensions = Extensions::new();
        extensions.insert(PeerAddr(addr("192.168.1.50", 40000)));
        assert!(!peer_is_loopback(&extensions));
    }
}
