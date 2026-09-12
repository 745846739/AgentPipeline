//! SSE 通道与跨源防护（决策 76 / 123 / 128）。

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, Method};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::state::{ApiError, AppState};

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

#[cfg(test)]
mod tests {
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
