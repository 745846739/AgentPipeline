//! 配对端点（决策 182㉖㉗㉘，票 07）。
//!
//! | 方法 | 路径 | 说明 |
//! |---|---|---|
//! | GET | `/pairing/token` | **仅本机可读**的令牌读取口 |
//! | POST | `/pairing/reset` | 一键重置，返回新令牌 |
//!
//! 读取口是「本机怎么把令牌递给手机」这条链路的起点：本机（回环）打开分享页，
//! 页面把 `/pairing/token` 的读数拼进配对 URL，二维码把 URL 递到手机上。
//! 故它对局域网来源是 403——局域网客户端若也能读，令牌就不再是「已配对设备的凭据」，
//! 而成了任何人可取的公开值，整个中间件也就没有意义了。

use axum::extract::{Request, State};
use axum::response::IntoResponse;
use axum::Json;
use serde_json::json;

use crate::peer::peer_is_loopback;
use crate::state::{map_core_error, ApiError, ApiResult, AppState};

/// `GET /pairing/token`。
///
/// 判定用的是来源地址（[`peer_is_loopback`]），不是绑定形态：即使服务绑在 `0.0.0.0`，
/// 从本机发出的请求也是回环地址，仍可读到——「用本机把令牌递给手机」正是这个组合。
pub async fn token(
    State(state): State<AppState>,
    request: Request,
) -> ApiResult<impl IntoResponse> {
    if !peer_is_loopback(request.extensions()) {
        return Err(ApiError::forbidden(
            "配对令牌只能在本机读取：请在已配对的设备上打开手机访问页扫码",
        ));
    }
    let token = state.store.pairing_token().await.map_err(map_core_error)?;
    Ok(Json(json!({ "token": token })))
}

/// `POST /pairing/reset`：重生成令牌并返回新值。
///
/// 它在局域网形态下是写请求，已被 `pairing_guard` 拦截（需持有令牌或来自本机）。
/// **回环来源豁免是有意的**：令牌泄露或丢失时必须还能从这台机器上重置，否则唯一的
/// 出路是删掉数据库文件——一个没有复位口的长期令牌等于没有应对泄露的手段（票 07）。
/// 换句话说，这个端点对「本机访问者」的信任，与整个系统对「本机」的既有信任一致。
pub async fn reset(State(state): State<AppState>) -> ApiResult<impl IntoResponse> {
    let token = state
        .store
        .reset_pairing_token()
        .await
        .map_err(map_core_error)?;
    Ok(Json(json!({ "token": token })))
}
