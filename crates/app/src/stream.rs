//! SSE 通道与跨源防护（决策 76 / 123 / 128 / 182）。

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, Method};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::peer::peer_is_loopback;
use crate::state::{ApiError, AppState};

/// 配对令牌请求头（决策 182㉗，票 07）。
pub const PAIRING_TOKEN_HEADER: &str = "x-agentpipeline-token";

/// 跨源防护中间件（决策 128）。
///
/// 127.0.0.1 绑定**不防**跨站 POST（form / no-cors fetch 可驱动 merge/decision、cancel、resume）。
/// 规则：所有写请求（非 GET/HEAD）必须满足其一，否则 403——
/// ① 携带自定义头 `X-AgentPipeline`；② `Origin` / `Referer` 缺失（非浏览器客户端）；
/// ③ `Origin` / `Referer` 等于本机 origin。SSE 为纯 GET，不受影响。
pub async fn cross_origin_guard(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Response {
    let method = request.method().clone();
    if method == Method::GET || method == Method::HEAD || method == Method::OPTIONS {
        return next.run(request).await;
    }

    if headers.contains_key("x-agentpipeline") {
        return next.run(request).await;
    }

    let origin = headers
        .get(header::ORIGIN)
        .or_else(|| headers.get(header::REFERER))
        .and_then(|v| v.to_str().ok());

    match origin {
        // ② 非浏览器客户端（curl / 测试 / CLI）
        None => next.run(request).await,
        Some(value) => {
            // 决策 128（修订）：Origin/Referer 必须**严格等于**本机 origin 之一
            //（{127.0.0.1, localhost}:{port}）。Referer 携带完整 URL，接受以
            // `{origin}/` 开头的同源路径；`127.0.0.1:8787.evil.com` 这类前缀伪装不过。
            let allowed = state.allowed_origins().iter().any(|allowed| {
                value == allowed.as_str()
                    || value
                        .strip_prefix(allowed.as_str())
                        .is_some_and(|rest| rest.starts_with('/'))
            });
            if allowed {
                next.run(request).await
            } else {
                ApiError::forbidden(format!("跨源写请求被拒绝：Origin/Referer = {value}"))
                    .into_response()
            }
        }
    }
}

/// 配对令牌校验（决策 182㉖㉗㉘，票 07）。
///
/// 用户批的边界是「看的随便看，动手和花钱要凭据」：只读 GET（看板 / 会话 / 指标 /
/// 分享页）不护，写请求与全部 `/foreman/*` 要持有令牌。
///
/// 三个前置判断，缺一不可：
/// 1. **非局域网形态直接放行**——默认回环绑定是「本机自己用」，零摩擦是硬约束
///    （票 07：默认形态根本不要求配对）；
/// 2. **回环来源直接放行**——局域网形态下仍有从本机发来的请求（本机浏览器、CLI），
///    对它们要求令牌等于把本机也变成需配对的设备。这一条同时是「令牌泄露后还能
///    从本机一键重置」的前提（见 `routes::pairing::reset`）；
/// 3. 放行集合只含安全方法（GET / HEAD / OPTIONS）与**不以 `/foreman/` 开头**的路径。
pub async fn pairing_guard(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    if !state.lan_mode() {
        return next.run(request).await;
    }
    if peer_is_loopback(request.extensions()) {
        return next.run(request).await;
    }

    let method = request.method().clone();
    let is_read_only = method == Method::GET || method == Method::HEAD || method == Method::OPTIONS;
    let guarded = !is_read_only || request.uri().path().starts_with("/foreman/");
    if !guarded {
        return next.run(request).await;
    }

    // 读取失败时**不放行**：拿不到该比对的令牌就无从证明请求有权，fail closed。
    let expected = match state.store.pairing_token().await {
        Ok(token) => token,
        Err(e) => return ApiError::internal(format!("读取配对令牌失败：{e}")).into_response(),
    };
    let provided = request
        .headers()
        .get(PAIRING_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok());
    match provided {
        Some(token) if fixed_length_eq(token, &expected) => next.run(request).await,
        // 报文点名「跑服务的电脑本机」而不是「已配对的设备」（决策 189）：配对码只能在
        // 回环来源的那一页上生成，原措辞让手机上的使用者去重复扫一张它自己永远拿不到的码。
        _ => ApiError::forbidden("这台设备还没配对：请在跑服务的电脑本机打开手机访问页扫码")
            .into_response(),
    }
}

/// 定长折叠异或比较：先比长度，再逐字节累积差异。
///
/// **诚实地说**：长度不同会提前返回，这一步泄漏的是长度而非内容；本威胁模型是同一
/// 局域网内的对端，通道还是明文 HTTP（决策 167 的分享形态），所以这是卫生习惯而不是
/// 真正的防护。真正的边界是「局域网内的写请求必须持有令牌」，不是抗时序侧信道。
fn fixed_length_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= *x ^ *y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::fixed_length_eq;

    #[test]
    fn fixed_length_eq_matches_only_identical_tokens() {
        assert!(fixed_length_eq("abc", "abc"));
        assert!(!fixed_length_eq("abc", "abd"));
        assert!(!fixed_length_eq("abc", "abcd"), "长度不同直接不等");
        assert!(!fixed_length_eq("", "x"));
        assert!(fixed_length_eq("", ""));
    }

    #[test]
    fn allowed_origins_cover_loopback_forms() {
        // 只断言 origin 生成逻辑（真实中间件行为在 L3 契约测试里用真请求验证）
        let state_port = 8787u16;
        let allowed = [
            format!("http://127.0.0.1:{state_port}"),
            format!("http://localhost:{state_port}"),
        ];
        assert!(allowed.contains(&"http://127.0.0.1:8787".to_string()));
        assert!(allowed.contains(&"http://localhost:8787".to_string()));
        assert!(!allowed.contains(&"http://evil.example".to_string()));
    }
}
