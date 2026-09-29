//! 未配对设备的落点：一张**自带全部样式与脚本**的配对页（决策 336）。
//!
//! 为什么需要它：全站闸门之后，未配对设备连 `index.html` 都拿不到，界面自然也没有
//! 机会说「怎么配对」。若维持「静态外壳公开」，未配对的人看到的是一个能加载、但每个
//! 数据请求都 403 的空看板——那既不是「看不了」，也不是「能看」，而是一种最费解的状态。
//! 所以出口是：**页面本身就在闸门后面**，未配对时由守卫直接给出这张页。
//!
//! 这张页因此不能引用任何同源资源（`/assets/*`、`/sw.js`、字体都在闸门后面），
//! 全部内联。[`PAIRING_PAGE_CONSTRAINTS`] 的单测钉住这条约束——它是**约束**不是风格：
//! 一旦有人给这张页加一行 `<link href="/assets/…">`，它在未配对设备上就是白底黑字。

/// 页面正文。改文案直接改 `pairing_page.html`（它不进前端构建，由 `include_str!` 编译期嵌入）。
pub const PAIRING_PAGE: &str = include_str!("pairing_page.html");

/// `401` 而不是 `403`：这次请求**确实没有凭据**，语义上就是未认证；报文里不带
/// `WWW-Authenticate`，故浏览器不会弹自己的密码框（弹了就盖住这张页，反而看不见说明）。
/// `no-store` 是必须的：这张页的内容取决于**这台设备有没有令牌**，一个中间缓存把它发给
/// 刚配对好的设备，就会卡在「还没配对」上直到强刷。
pub fn response() -> axum::response::Response {
    use axum::http::{header, StatusCode};
    use axum::response::IntoResponse;

    (
        StatusCode::UNAUTHORIZED,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        PAIRING_PAGE,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::PAIRING_PAGE;

    /// 页面依赖的两枚约定：地址里的 `pair`（与 `server_info::pairing_url` 同）、本机存令牌的
    /// 键名（与 `frontend/src/api/config.ts::PAIRING_STORAGE_KEY` 同）。抄错任一个，这张页
    /// 都只在「扫码」与「已存过令牌」两条路上悄悄失灵。
    const PAIRING_PAGE_CONSTRAINTS: [&str; 2] = ["pair", "agentpipeline.pairing"];

    #[test]
    fn page_carries_the_two_cross_language_conventions() {
        for needle in PAIRING_PAGE_CONSTRAINTS {
            assert!(PAIRING_PAGE.contains(needle), "配对页少了约定：{needle}");
        }
    }

    /// **零外链**：`/assets/*`、`/sw.js`、字体全在闸门后面，未配对时一个都取不到。
    #[test]
    fn page_pulls_nothing_from_the_guarded_shell() {
        for pattern in ["<script src", "<link ", "<img", "@import", "url("] {
            assert!(
                !PAIRING_PAGE.contains(pattern),
                "配对页引用了外部资源（{pattern}）——那些路径也在闸门后面"
            );
        }
    }

    /// 自动重试的护栏：地址里已经带着令牌时不能再自动跳转，否则这条地址与服务端会来回打转。
    #[test]
    fn page_does_not_auto_retry_a_token_that_came_from_the_url() {
        let auto = PAIRING_PAGE.matches("location.replace").count();
        assert_eq!(auto, 1, "自动跳转只该有一处（本机存过令牌那条路）");
        assert!(
            PAIRING_PAGE.contains("if (fromUrl)"),
            "先判「地址里带没带令牌」，带了的走提示而不是重试"
        );
    }
}
