//! 前端静态资源内嵌与同源托管（决策 155）。
//!
//! build.rs 把 `frontend/dist` 拷进 OUT_DIR 并生成 include_bytes! 资产表，本模块经
//! axum 托管——决策 153④ 的「API base 默认同源相对路径」由此直接成立（零 CORS）。
//! dist 缺失时资产表为空：`/` 退化为「前端未构建」提示页，API 不受影响；前端开发
//! 仍走 vite dev server 代理（frontend/vite.config.ts）。

use axum::extract::Path;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use crate::state::AppState;

// build.rs 生成的资产表：`(dist 相对路径, 文件内容)`；dist 缺失时为空表。
// include! 于 item 位置粘贴 OUT_DIR/frontend_assets.rs，其内即 EMBEDDED_ASSETS 的 static 定义。
include!(concat!(env!("OUT_DIR"), "/frontend_assets.rs"));

/// 前端静态路由：入口页 + 构建产物（hash 文件名走 `/assets/{*path}`，dist 根层
/// 其他文件如 favicon 按表逐个注册）。
pub fn static_routes() -> Router<AppState> {
    let mut router = Router::new()
        .route("/", get(index))
        .route("/assets/{*path}", get(asset));
    for (name, _) in EMBEDDED_ASSETS {
        if !name.contains('/')
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
        {
            // 字面路由没有路径参数，不能走 `Path` extractor（会 500），按名分发
            router = router.route(&format!("/{name}"), get(move || serve_named(name)));
        }
    }
    router
}

/// 根层字面路由的按名分发（`/index.html`、`/favicon.ico` 等）。
async fn serve_named(name: &'static str) -> Response {
    match lookup(name) {
        Some((bytes, mime)) => serve(
            bytes,
            mime,
            if name == "index.html" {
                CACHE_NO_CACHE
            } else {
                CACHE_IMMUTABLE
            },
        ),
        None => (StatusCode::NOT_FOUND, format!("静态资源不存在：{name}")).into_response(),
    }
}

/// `GET /`：内嵌 index.html；未内嵌时给构建提示页（不 404，避免误读成服务挂了）。
async fn index() -> Response {
    match lookup("index.html") {
        Some((bytes, mime)) => serve(bytes, mime, CACHE_NO_CACHE),
        None => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            MISSING_FRONTEND_HTML,
        )
            .into_response(),
    }
}

/// `GET /assets/{*path}`：按表精确查找（免路径穿越），未知路径 404。
/// 通配只捕获 `assets/` 之后的部分，查表前补回前缀对齐 dist 相对路径。
async fn asset(Path(path): Path<String>) -> Response {
    let key = format!("assets/{path}");
    match lookup(&key) {
        Some((bytes, mime)) => serve(
            bytes,
            mime,
            if key == "index.html" {
                CACHE_NO_CACHE
            } else {
                CACHE_IMMUTABLE
            },
        ),
        None => (StatusCode::NOT_FOUND, format!("静态资源不存在：{key}")).into_response(),
    }
}

const CACHE_NO_CACHE: &str = "no-cache";
const CACHE_IMMUTABLE: &str = "public, max-age=31536000, immutable";

fn serve(bytes: &'static [u8], mime: &'static str, cache: &'static str) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, cache)],
        bytes,
    )
        .into_response()
}

/// 按精确路径查资产，返回内容与推断的 Content-Type。
fn lookup(path: &str) -> Option<(&'static [u8], &'static str)> {
    EMBEDDED_ASSETS
        .iter()
        .find(|(name, _)| *name == path)
        .map(|(_, bytes)| (*bytes, mime_for(path)))
}

/// 扩展名 → Content-Type（构建产物集合固定，显式枚举即可）。
fn mime_for(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "txt" => "text/plain; charset=utf-8",
        "webmanifest" => "application/manifest+json",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// 未内嵌前端时 `GET /` 的提示页：指路 make build 与 vite dev server。
pub const MISSING_FRONTEND_HTML: &str = r#"<!doctype html>
<html lang="zh-CN">
  <head><meta charset="UTF-8" /><title>AgentPipeline · 前端未构建</title></head>
  <body style="font-family: ui-monospace, SFMono-Regular, Menlo, monospace; max-width: 42rem; margin: 4rem auto; line-height: 1.9;">
    <h1>AgentPipeline API 已启动</h1>
    <p>这个二进制构建时没有内嵌前端（frontend/dist 缺失），当前只有 API 可用。</p>
    <p>打包前后端一体的单二进制：</p>
    <pre><code>make build          # 前端构建 + 内嵌 + release 构建
make run            # 构建并直接启动</code></pre>
    <p>手动等价：<code>cd frontend &amp;&amp; npm ci &amp;&amp; npm run build</code> 后重新 <code>cargo build --release</code>。</p>
    <p>前端热更开发：<code>cd frontend &amp;&amp; npm run dev</code>（vite 把 API 代理到本机 axum，默认 127.0.0.1:8787）。</p>
  </body>
</html>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_covers_vite_build_outputs() {
        assert_eq!(mime_for("index.html"), "text/html; charset=utf-8");
        assert_eq!(
            mime_for("assets/index-CnQTDGQQ.js"),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(
            mime_for("assets/index-CLozDDGR.css"),
            "text/css; charset=utf-8"
        );
        assert_eq!(mime_for("logo.svg"), "image/svg+xml");
        assert_eq!(mime_for("unknown.bin"), "application/octet-stream");
    }

    #[test]
    fn index_lookup_matches_embed_state() {
        match lookup("index.html") {
            Some((bytes, mime)) => {
                assert!(!bytes.is_empty());
                assert_eq!(mime, "text/html; charset=utf-8");
            }
            None => assert!(EMBEDDED_ASSETS.is_empty(), "index 查不到时表应为空"),
        }
    }
}
