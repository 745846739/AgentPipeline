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

/// 前端静态路由：入口页 + 构建产物。
///
/// `/assets/{*path}` 走通配（Vite 的 hash 产物）；dist 里的其他文件——根层的
/// favicon 与**嵌套目录**（如主题六自托管的 `fonts/fusion-pixel-12px/*.woff2`，
/// 决策 169）——按各自的字面路径逐个注册。用字面路由而非根层通配，是为了不引入
/// `/{*path}` 兜底：那会改变 API 未命中时的 404 响应形态（决策 155 的合并路由器）。
pub fn static_routes() -> Router<AppState> {
    let mut router = Router::new()
        .route("/", get(index))
        .route("/assets/{*path}", get(asset));
    for (name, _) in EMBEDDED_ASSETS {
        // `assets/` 前缀已由上面的通配路由覆盖，避免重复注册。
        if name.starts_with("assets/") || name.contains('{') || name.contains('}') {
            continue;
        }
        if name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'/'))
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
        Some((bytes, mime)) => serve(bytes, mime, cache_for(name)),
        None => (StatusCode::NOT_FOUND, format!("静态资源不存在：{name}")).into_response(),
    }
}

/// `GET /`：内嵌 index.html；未内嵌时给构建提示页（不 404，避免误读成服务挂了）。
async fn index() -> Response {
    match lookup("index.html") {
        Some((bytes, mime)) => serve(bytes, mime, cache_for("index.html")),
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
        Some((bytes, mime)) => serve(bytes, mime, cache_for(&key)),
        None => (StatusCode::NOT_FOUND, format!("静态资源不存在：{key}")).into_response(),
    }
}

const CACHE_NO_CACHE: &str = "no-cache";
const CACHE_IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// 该文件的 `Cache-Control`：**地址固定、内容会变的必须复验**（决策 285）。
///
/// `no-cache` 不是「不缓存」，是「可缓存、每次用前回源复验」；`immutable` 则承诺
/// 「地址不变内容也不变」，只有 Vite 的 `assets/` 产物配得上——它的文件名里带内容哈希，
/// 内容一改就是另一个地址。根层这些名字（页面 shell 与 manifest）改内容不改地址，长缓存
/// 会把旧副本钉死在客户端上，而这两份都是**行为**：`index.html` 决定加载哪个产物，
/// `manifest.webmanifest` 里的 `start_url` 决定主屏图标的启动地址，配对令牌正是靠那条
/// 地址递进去的（决策 191）。manifest 被这么钉过一次的后果很重——手机拿旧副本建图标，
/// 等于把决策 285 的修复挡在门外。
///
/// `sw.js` 同属这一族（决策 323）：service worker 的地址固定（注册时写死 `/sw.js`，
/// 浏览器按地址判断「是否同一个 worker」）、内容随每次构建而变，且**它就是行为本身**——
/// 旧的 `push`/`notificationclick` 处理器被长缓存钉住，服务端改了通知的跳转语义也推不到
/// 设备上。它比 manifest 更急：manifest 有 24 小时兜底重取，而 SW 脚本的复验时机由
/// 浏览器决定，长缓存会把它推到「最长一年」。
///
/// **图标与字体仍走 `immutable`**（有意）：它们陈旧只是观感问题，而代价是实打实的——
/// 主题六自托管 78 个 woff2 子集（决策 169），改成每次回源就是每次开页重取一遍。
fn cache_for(name: &str) -> &'static str {
    match name {
        "index.html" | "manifest.webmanifest" | "sw.js" => CACHE_NO_CACHE,
        _ => CACHE_IMMUTABLE,
    }
}

/// 页面一律声明**不发送 `Referer`**（决策 191）。
///
/// 配对令牌现在留在地址栏里（主屏图标与书签要靠它每次启动重新递进来，决策 191 修订了
/// 182㉙ 的「从地址栏抹掉」），而 `Referer` 会把**含令牌的完整地址**带给任何第三方资源。
/// 本应用目前不引任何外部资源（字体都自托管），这条是给「以后顺手加了个外链 / 外图」留的
/// 保险——一个响应头换掉当初抹地址栏的三条理由之一，划算。
const REFERRER_POLICY: &str = "no-referrer";

fn serve(bytes: &'static [u8], mime: &'static str, cache: &'static str) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, cache),
            (header::REFERRER_POLICY, REFERRER_POLICY),
        ],
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
    <p>前端热更开发：<code>cd frontend &amp;&amp; npm run dev</code>（vite 把 API 代理到本机 axum，默认 127.0.0.1:8788）。</p>
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

    /// 配对令牌现在留在地址栏里（决策 191 修订了 182㉙ 的「从地址栏抹掉」），
    /// 故页面必须声明**不发 `Referer`**——少了这个头，任何第三方资源都会从 `Referer`
    /// 里拿到含令牌的完整地址。三个静态出口（`index` / `serve_named` / `asset`）都走 `serve`。
    #[test]
    fn static_responses_forbid_referer() {
        let response = serve(b"<html></html>", "text/html; charset=utf-8", CACHE_NO_CACHE);
        assert_eq!(
            response
                .headers()
                .get(header::REFERRER_POLICY)
                .and_then(|value| value.to_str().ok()),
            Some("no-referrer"),
        );
    }

    /// 缓存策略的牙齿（决策 285）：**地址固定、内容会变的那几份必须复验**。
    /// 少了它，manifest 会被当带哈希的产物发一年期 `immutable`——手机拿旧副本建主屏图标，
    /// 图标就丢掉了地址里的配对令牌（决策 191 的契约），而源码里怎么改都推不过去。
    /// `sw.js` 同理（决策 323）：浏览器按地址判断「同一个 worker」，旧副本被钉住就等于
    /// 通知的跳转语义永远停在部署那一刻。
    #[test]
    fn mutable_behavioral_files_revalidate() {
        assert_eq!(cache_for("index.html"), CACHE_NO_CACHE);
        assert_eq!(cache_for("manifest.webmanifest"), CACHE_NO_CACHE);
        assert_eq!(cache_for("sw.js"), CACHE_NO_CACHE);
        // 带内容哈希的产物与纯视觉资产仍长缓存（理由见 cache_for 的注释）。
        assert_eq!(cache_for("assets/index-CnQTDGQQ.js"), CACHE_IMMUTABLE);
        assert_eq!(cache_for("icons/icon-192.png"), CACHE_IMMUTABLE);
    }

    #[test]
    fn mime_covers_self_hosted_font_subsets() {
        // 主题六把像素字体的 78 个 woff2 子集入库（决策 169），且随 dist 内嵌。
        let key = "fonts/fusion-pixel-12px/Fusion-Pixel-12px-Monospaced-Simplified-Chinese.Basic-Latin.woff2";
        assert_eq!(mime_for(key), "font/woff2");
        assert_eq!(
            mime_for("fonts/fusion-pixel-12px/fusion-pixel-12px.css"),
            "text/css; charset=utf-8"
        );
    }

    /// 嵌套字体路径必须被注册成路由（决策 169）：`static_routes` 只看根层字面名时，
    /// `GET /fonts/…/x.woff2` 会 404，自托管字体静默回退成系统 monospace。
    /// 这里对**注册集合**做断言（不启服务），内嵌 dist 存在时逐条核对。
    #[test]
    fn nested_font_assets_are_routed() {
        let fonts: Vec<&str> = EMBEDDED_ASSETS
            .iter()
            .map(|(name, _)| *name)
            .filter(|n| n.starts_with("fonts/"))
            .collect();
        if fonts.is_empty() {
            // dist 缺失（未构建前端）时资产表为空——不是本用例的被测对象。
            assert!(EMBEDDED_ASSETS.is_empty(), "有资产却无字体：主题字体未入库");
            return;
        }
        for name in &fonts {
            assert!(
                name.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'/')),
                "{name} 含不可路由字符，static_routes 会跳过它"
            );
            assert!(lookup(name).is_some(), "{name} 应在资产表内可查");
        }
        // assets/ 前缀仍由通配路由覆盖，不应被逐条注册（避免重复路由 panic）。
        assert!(!fonts.iter().any(|n| n.starts_with("assets/")));
    }
}
