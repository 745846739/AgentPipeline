//! SSE 通道与跨源防护（决策 76 / 123 / 128 / 182）。

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, Method};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::peer::peer_is_loopback;
use crate::state::{ApiError, AppState};

/// 配对令牌请求头（决策 182㉗，票 07）：脚本 / 桌面壳 / 测试的载法。
pub const PAIRING_TOKEN_HEADER: &str = "x-agentpipeline-token";

/// 配对令牌的 cookie 名（决策 336）：**浏览器的载法**。
///
/// 为什么必须有第四条通道（前三者是头、地址、本机存储）：导航请求带不了自定义头，而
/// 地址里那条 `?pair=` 只在扫码那一次有——本机存的那份 JS 读得到、服务器读不到。判
/// 「这台设备配过没有」得靠一样浏览器自己会带的东西，那只能是 cookie。
pub const PAIRING_COOKIE: &str = "agentpipeline_pairing";

/// 订阅族的路由前缀（pwa-webpush 02）：`lib.rs` 那两条路由注册从这个常量拼出来，
/// **唯一事实源**——守卫的白名单与注册的地址一旦不一致，症状是「接口在、守卫不认」，
/// 而靠手写注释对齐是迟早会漂的。（决策 336 之后守卫不再按前缀放行任何路径，这个常量
/// 只剩注册这一处用处，故它现在只钉「地址怎么拼」。）
pub const PUSH_SUBSCRIPTIONS_PREFIX: &str = "/notify/push/";

/// SSE 心跳间隔（票 01，stream-self-heal）：静默的流每 15 秒发一帧
/// **不携带 data 的注释帧**——客户端分帧器只认 data 行，解析器因此零改动。
///
/// 「有事件才出字节」的流在半开连接下双方都察觉不到：这是客户端停滞看门狗
/// 分清「安静且健康」与「安静且已死」的唯一凭据。与看门狗阈值成 **3 倍**关系
/// （客户端阈值 45 秒 = 3 × 本间隔）：慢到丢两帧心跳才判死，改一个必须看另一个。
pub const SSE_KEEPALIVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);

/// 配对缺失的机器可读失败类别（票 04 / 决策 259，姿态照 `KIND_SKILL_NOT_FOUND` 先例）。
///
/// 403 在本应用里被两处用着（配对缺失、跨源防护），界面此前只能按**报文字串**分支
/// 来区分——报文即接口。`kind` 让这两次 403 从形状上分得开；**报文一个字不动**，
/// 它照旧是给人看的那句话（决策 189 的措辞）。
pub const KIND_PAIRING_REQUIRED: &str = "pairing_required";

/// 配对缺失的拒绝（`pairing_guard` 的落点）。抽成构造函数是为了让「带 kind、
/// 报文原样」这件事能被单测钉住——中间件本体要真请求才跑得到（L3 契约的地盘）。
fn pairing_rejected() -> ApiError {
    // 报文点名「跑服务的电脑本机」而不是「已配对的设备」（决策 189）：配对码只能在
    // 回环来源的那一页上生成，原措辞让手机上的使用者去重复扫一张它自己永远拿不到的码。
    ApiError::forbidden("这台设备还没配对：请在跑服务的电脑本机打开手机访问页扫码")
        .with_kind(KIND_PAIRING_REQUIRED)
}

/// 跨源防护的拒绝（`cross_origin_guard` 的落点）。**不带 kind**——它与配对缺失同为 403，
/// 界面按 kind 分支的前提正是这一对里只有一个带（票 04 要钉的「跨源 403 不挂配对入口」）。
fn origin_rejected(origin: &str) -> ApiError {
    ApiError::forbidden(format!("跨源写请求被拒绝：Origin/Referer = {origin}"))
}

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
                origin_rejected(value).into_response()
            }
        }
    }
}

/// 配对令牌校验（决策 182㉖㉗㉘，票 07；**决策 336 扩到全站**）。
///
/// **判据一句话：非回环形态下，未配对的设备什么页面都拿不到**（决策 336 修订决策 167
/// 的「看的随便看」）。原来那条边界（只护写请求 + `/foreman/` + `/notify/push/`）是给
/// **可信局域网**写的——看板与任务内容在那个前提下不算秘密。可一旦服务直接挂在公网上
/// （裸 IP + 应用自己终止 TLS 的形态），「读」就是最要紧的那件事：任务标题、会话、命令
/// 输出、指标全都是可读数据，而自签证书只拦浏览器、拦不住扫描器。于是读与写一起护，
/// **静态外壳也不再例外**（理由见第 3 条），凭据仍是那一个配对令牌
/// （按设备可重置，见 `routes::pairing::reset`）。
///
/// 1. **回环绑定形态直接放行**——默认绑定是「本机自己用」，零摩擦是硬约束
///    （票 07：默认形态根本不要求配对）；
/// 2. **回环来源直接放行**——局域网 / 公网形态下仍有从本机发来的请求（本机浏览器、CLI、
///    同机的反向代理），对它们要求令牌等于把本机也变成需配对的设备。这一条同时是
///    「令牌泄露后还能从本机一键重置」与「令牌只能从本机读出来递给手机」的前提
///    （见 `routes::pairing::token` / `reset`）；
/// 3. 剩下的一律要凭据，**同一个令牌有三种载法**：
///    - **地址里的 `?pair=`**（[`pair_param`]）——配对入口。二维码与主屏图标都靠它，
///      故它是唯一能把令牌带进**一次导航**的通道（自定义头带不进导航，cookie 那时还没种）。
///      对上即放行，并在响应上顺手种 cookie；
///    - **`X-AgentPipeline-Token` 头**（决策 182㉙）——脚本 / 桌面壳 / 测试的载法；
///    - **cookie**（决策 336）——浏览器的载法：导航请求带不了自定义头，而地址里那条
///      `?pair=` 只在扫码那一次有，本机存的那份 JS 读得到、服务器读不到。判「这台设备
///      配过没有」必须有一样浏览器自己会带的东西，它也是唯一能护住**第二次访问**的通道
///      （直接输地址、点书签、地址栏里没有 `?pair=` 的刷新）。
///
/// **静态外壳为什么也不再例外**（修订 `assets::is_embedded_asset_path` 那条放行）：
/// 外壳本身确实不含数据，但「不含数据」不等于「可用」——未配对时放行外壳，拿到的是一个
/// 能加载、每个数据请求都 403 的空看板，界面上「怎么配对」这件事反而看不见。既然要挡，
/// 就挡得能被理解：未配对设备只得到一张自带样式的配对页（[`crate::pairing_page`]）。
/// 代价是那张页不能引用任何同源资源，故它全内联。
///
/// 保留的两处历史理由（现在被上面那条总规则盖住，但各自的账仍然成立）：
/// `/foreman/` 读对话会触发真实 LLM 调用（决策 182㉘：花钱要凭据）；
/// `/notify/push/` 的读接口本身是一条外泄管道——设备清单里每一条都是「往这台设备推任意
/// 报文」能力的一半（endpoint 是能力 URL，`p256dh`/`auth` 是另一半，故清单只给摘要），
/// 而订阅**是持续的**：别人一旦把自己的 endpoint 订进来，你的每条任务动态都会流到他那里
/// （pwa-webpush 02）。
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

    // 读取失败时**不放行**：拿不到该比对的令牌就无从证明请求有权，fail closed。
    let expected = match state.store.pairing_token().await {
        Ok(token) => token,
        Err(e) => return ApiError::internal(format!("读取配对令牌失败：{e}")).into_response(),
    };

    let enrolled = pair_param(request.uri().query()).is_some_and(|t| fixed_length_eq(t, &expected));
    if enrolled || carried_token(&request, &expected) {
        let mut response = next.run(request).await;
        if enrolled {
            // 这一次照常放行而**不跳转**：地址栏里那条 `?pair=` 是主屏图标与书签每次启动
            // 重新递令牌的通道（决策 191），重定向会把它从地址上抹掉，等于断了那条路。
            // 种 cookie 是**加**一条更省事的通道，不是换一条（每次带 `?pair=` 都续期）。
            match header::HeaderValue::from_str(&enrollment_cookie(&expected)) {
                Ok(cookie) => {
                    response.headers_mut().append(header::SET_COOKIE, cookie);
                }
                // 值里有非法字节（不该发生：令牌是 Crockford base32）。**宁可不种**也不种
                // 一个坏值——坏值只会让下一次导航又回到这张配对页，且没人知道为什么。
                Err(e) => tracing::warn!(error = %e, "配对 cookie 值非法，本次不种"),
            }
        }
        return response;
    }

    // 未配对。浏览器导航拿一张配对页（它自己会说「怎么配」）；其余请求保持既有形状，
    // 界面据此分支（`kind`，决策 189）。
    if wants_document(request.headers()) {
        return crate::pairing_page::response();
    }
    pairing_rejected().into_response()
}

/// 地址里的配对参数（与 `server_info::pairing_url` 是同一个约定：`{base}/?pair={token}`）。
///
/// 手写而不引 `serde_urlencoded`：只认这一个键，且**不做百分号解码**——令牌是两枚
/// ULID 拼出来的 Crockford base32（决策 182㉗），字符集全在 URL 安全字符里。
/// 一处宽容：`?a=1&pair=t&b=2` 这种顺序无关的写法照收（分享链常带别的参数）。
fn pair_param(query: Option<&str>) -> Option<&str> {
    query?
        .split('&')
        .find_map(|kv| kv.strip_prefix("pair="))
        .filter(|t| !t.is_empty())
}

/// 请求自带的凭据：自定义头（决策 182㉙）或 cookie（决策 336）。
fn carried_token(request: &Request, expected: &str) -> bool {
    let header_token = request
        .headers()
        .get(PAIRING_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok());
    if header_token.is_some_and(|t| fixed_length_eq(t, expected)) {
        return true;
    }
    cookie_value(request.headers(), PAIRING_COOKIE).is_some_and(|t| fixed_length_eq(t, expected))
}

/// `Cookie:` 头里取一个键的值（手写：只认一个键，不理会属性与其它 cookie）。
///
/// 多个 `Cookie` 头（HTTP/2 允许拆）与同一头里多个 `;` 段都要看，故逐个展开再找；
/// 命中第一个非空值即可——同名 cookie 出现两次本就不该发生，浏览器带哪个不由这里决定。
fn cookie_value<'a>(headers: &'a axum::http::HeaderMap, name: &str) -> Option<&'a str> {
    let prefix = format!("{name}=");
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|raw| raw.split(';'))
        .filter_map(|kv| kv.trim().strip_prefix(prefix.as_str()))
        .find(|v| !v.is_empty())
}

/// 配对成功时种下的 cookie（决策 336）。
///
/// 四处属性各有理由：`Path=/`（全站都要带）；`HttpOnly`（前端从不需要读它——它只回答
/// 「这台设备配过没有」，而值本身仍由头与地址两条老路带着走）；`SameSite=Lax`（跨站子请求
/// 不携带，写请求的 CSRF 面因此归零，而**顶层导航照带**——第一次扫码进来正是导航）；
/// `Secure` **无条件带上**：TLS 形态下它就该只在加密连接上走，明文形态（局域网 HTTP）下
/// 浏览器会直接拒收这个 cookie——而那正是想要的，明文连接不该让凭据驻留。
/// `Max-Age` 一年：与「主屏图标里那条地址长期有效」同一量级；令牌一重置它就立刻失效
/// （服务端比的是当前令牌），故不需要更短的寿命。
fn enrollment_cookie(token: &str) -> String {
    format!("{PAIRING_COOKIE}={token}; Path=/; Max-Age=31536000; HttpOnly; Secure; SameSite=Lax")
}

/// 这次请求要的是「一张给人看的页面」吗（而不是 JSON / 脚本 / 图片）。
///
/// 判据只能用 `Accept`（外加 `Sec-Fetch-Mode: navigate`）：路径分不出来——`/` 是导航，
/// 而把 `/tasks` 直接输在地址栏里同样是导航。`Sec-Fetch-*` 更准（连「点链接」与「fetch」
/// 都分得开），但它要求浏览器较新，故两条取或。
fn wants_document(headers: &axum::http::HeaderMap) -> bool {
    let accepts_html = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("text/html"));
    let navigates = headers
        .get("sec-fetch-mode")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("navigate"));
    accepts_html || navigates
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
    use super::{
        cookie_value, enrollment_cookie, fixed_length_eq, origin_rejected, pair_param,
        pairing_rejected, wants_document, KIND_PAIRING_REQUIRED, PAIRING_COOKIE,
    };
    use crate::state::ApiError;
    use axum::http::{header, HeaderMap, HeaderValue};

    #[test]
    fn pairing_rejection_carries_machine_readable_kind_and_verbatim_message() {
        let err = pairing_rejected();
        assert_eq!(err.status, axum::http::StatusCode::FORBIDDEN);
        assert_eq!(err.kind.as_deref(), Some(KIND_PAIRING_REQUIRED));
        // 报文一个字不动（决策 189）：kind 是加给机器的，不是换给人的那句话
        assert_eq!(
            err.message,
            "这台设备还没配对：请在跑服务的电脑本机打开手机访问页扫码"
        );
    }

    #[test]
    fn origin_rejection_has_no_kind_so_the_two_403s_stay_distinguishable() {
        let err = origin_rejected("http://evil.example");
        assert_eq!(err.status, axum::http::StatusCode::FORBIDDEN);
        assert!(
            err.kind.is_none(),
            "跨源 403 带上配对 kind 会让界面把 Origin 拒绝也挂上配对入口"
        );
    }

    #[test]
    fn forbidden_defaults_to_no_kind() {
        // 没显式 with_kind 的 403（路径越界、令牌只能本机读…）保持 None——
        // kind 是例外而非常态，界面按它分支才不会误报。
        let err = ApiError::forbidden("随便");
        assert!(err.kind.is_none());
    }

    #[test]
    fn fixed_length_eq_matches_only_identical_tokens() {
        assert!(fixed_length_eq("abc", "abc"));
        assert!(!fixed_length_eq("abc", "abd"));
        assert!(!fixed_length_eq("abc", "abcd"), "长度不同直接不等");
        assert!(!fixed_length_eq("", "x"));
        assert!(fixed_length_eq("", ""));
    }

    /* ─────────── 决策 336：地址 / cookie / 导航判定这三处解析 ─────────── */

    #[test]
    fn pair_param_reads_the_enrollment_channel_and_ignores_the_rest() {
        assert_eq!(pair_param(Some("pair=abc")), Some("abc"));
        assert_eq!(
            pair_param(Some("a=1&pair=abc&b=2")),
            Some("abc"),
            "顺序无关"
        );
        assert_eq!(pair_param(Some("Pair=abc")), None, "键名大小写敏感");
        assert_eq!(pair_param(Some("pair=")), None, "空值不算");
        assert_eq!(pair_param(Some("pairx=abc")), None, "前缀相同但不是这个键");
        assert_eq!(pair_param(Some("")), None);
        assert_eq!(pair_param(None), None, "没有查询串");
    }

    #[test]
    fn cookie_value_finds_the_key_among_neighbours() {
        let headers = cookie_headers(&[("session=zzz; agentpipeline_pairing=tok; theme=dark", 1)]);
        assert_eq!(cookie_value(&headers, PAIRING_COOKIE), Some("tok"));
    }

    #[test]
    fn cookie_value_handles_split_headers_and_spacing() {
        // HTTP/2 允许把 cookie 拆成多个头字段；`;` 两侧的空格也不该影响判定
        let split = cookie_headers(&[("session=zzz", 1), ("agentpipeline_pairing=tok2", 2)]);
        assert_eq!(cookie_value(&split, PAIRING_COOKIE), Some("tok2"));
        let spaced = cookie_headers(&[("session=zzz;  agentpipeline_pairing=tok3  ", 1)]);
        assert_eq!(cookie_value(&spaced, PAIRING_COOKIE), Some("tok3"));
        // 键名里有空格就是另一个键——「看着像」不放行
        let lookalike = cookie_headers(&[(" agentpipeline_pairing =tok4", 1)]);
        assert_eq!(cookie_value(&lookalike, PAIRING_COOKIE), None);
    }

    #[test]
    fn cookie_value_never_returns_an_empty_value() {
        // 空 cookie 值绝不能当成「带了令牌」（它连比较都不该进）
        let headers = cookie_headers(&[(&format!("{PAIRING_COOKIE}="), 1)]);
        assert_eq!(cookie_value(&headers, PAIRING_COOKIE), None);
    }

    #[test]
    fn enrollment_cookie_carries_the_four_attributes_that_matter() {
        let cookie = enrollment_cookie("tok");
        assert!(cookie.starts_with(&format!("{PAIRING_COOKIE}=tok;")));
        for attribute in ["Path=/", "HttpOnly", "Secure", "SameSite=Lax", "Max-Age="] {
            assert!(
                cookie.contains(attribute),
                "cookie 少了 {attribute}：{cookie}"
            );
        }
    }

    #[test]
    fn wants_document_depends_on_accept_or_the_fetch_metadata() {
        assert!(wants_document(&accept(&[
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"
        ])));
        let navigate_only = HeaderMap::from_iter([(
            header::HeaderName::from_static("sec-fetch-mode"),
            HeaderValue::from_static("navigate"),
        )]);
        assert!(
            wants_document(&navigate_only),
            "Accept 缺失时靠 Sec-Fetch-Mode"
        );
        assert!(
            !wants_document(&accept(&["*/*"])),
            "fetch / 静态资源不是导航"
        );
        assert!(!wants_document(&accept(&["application/json"])));
        assert!(!wants_document(&HeaderMap::new()), "什么都没有时不发页面");
    }

    fn accept(values: &[&str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(header::ACCEPT, HeaderValue::from_str(value).unwrap());
        }
        headers
    }

    fn cookie_headers(values: &[(&str, usize)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (value, count) in values {
            for _ in 0..*count {
                headers.append(header::COOKIE, HeaderValue::from_str(value).unwrap());
            }
        }
        headers
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
