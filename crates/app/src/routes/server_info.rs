//! 局域网分享端点（决策 167）：给出手机可访问的地址与对应二维码。
//!
//! 两个端点都不带状态变更，纯 GET：
//! - `GET /server-info`：JSON，含绑定 host / 端口 / 候选局域网地址 / 当前是否
//!   仅回环绑定（决定分享页是否该提示「需要重启并绑定 0.0.0.0」）；
//! - `GET /server-info/qr.svg`：把指定 URL 渲染成 SVG 二维码，供分享页 `<img>`
//!   直接引用——前端不必引入 QR 库，也不用把二维码画进 canvas。

use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::lan;
use crate::state::AppState;

/// `GET /server-info` 的响应体。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    /// 实际绑定地址（`[server] host` 或 CLI `--host`）。
    pub host: String,
    /// 实际绑定端口（端口 0 时是内核分配的真实端口）。
    pub port: u16,
    /// 仅绑定回环时为 true——此时手机连不上，分享页需给出如何开启的指引。
    pub loopback_only: bool,
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
    let host = state.bind_host.clone();
    let port = state.port;
    let addresses = build_addresses(&lan::lan_addresses(), port);

    Json(ServerInfo {
        loopback_only: is_loopback_bind(&host),
        host,
        port,
        addresses,
    })
    .into_response()
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
/// `<img src>` 会把 URL 写进对方日志。校验方式是比对候选集合 + 回环集合。
pub async fn qr_svg(State(state): State<AppState>, Query(query): Query<QrQuery>) -> Response {
    let allowed = allowed_qr_urls(&state);
    let target = match query.url {
        Some(url) => {
            if !allowed.iter().any(|a| a == &url) {
                return (StatusCode::BAD_REQUEST, "二维码只允许编码本服务的访问地址")
                    .into_response();
            }
            url
        }
        None => match allowed.first() {
            Some(url) => url.clone(),
            None => {
                return (StatusCode::BAD_REQUEST, "没有可用的访问地址").into_response();
            }
        },
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

/// 绑定 host 是否为回环（决定手机能否直连）。
fn is_loopback_bind(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// 允许被编码成二维码的 URL 集合：局域网候选地址 + 回环地址。
///
/// 回环也在集合内，是为了让「本机打开验证」这件事能复用同一端点（蓝牙 / 扫码
/// 场景下用户可能只是在同机另一窗口打开）。
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
    fn preferred_flag_survives_serialization() {
        let info = ServerInfo {
            host: "0.0.0.0".into(),
            port: 8787,
            loopback_only: false,
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
    }
}
