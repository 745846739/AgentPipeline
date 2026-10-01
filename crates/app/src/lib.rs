//! axum API 层（docs/implementation.md §11.7；契约测试见 tests/）。
//!
//! 设计要点：
//! - 路由构建与二进制启动分离——L3 测试用 tower oneshot 直接打 in-process router，不 spawn 二进制
//!   （决策 144）；
//! - 跨源防护中间件（决策 128）：只拦写请求；SSE 是纯 GET，不受影响；
//! - 对端地址层（决策 182 / 票 06）：把连接信息归一成 [`peer::PeerAddr`]，排在跨源防护
//!   之前，供票 07 判定「仅回环可读」；
//! - 配对令牌层（决策 182㉖㉗㉘ / 票 07；**决策 336 扩到全站**）：非回环形态下未配对的
//!   设备什么页面都拿不到，只有一张自带样式的配对页（[`pairing_page`]）；回环来源豁免；
//! - 前端 dist 由 build.rs 内嵌并同源托管（决策 155），**注册在配对层之前**（决策 336）；
//! - `/server-info` 暴露局域网访问地址与二维码（决策 167），供手机扫码接入；
//! - `api_key` 读接口只回显 `***`（决策 112）；
//! - 响应压缩（决策 361）：`tower-http` 的 br / gzip，按响应 content-type 放过事件流。

pub mod assets;
pub mod io_budget;
pub mod lan;
pub mod pairing_page;
pub mod peer;
pub mod routes;
pub mod runtime;
pub mod serve;
pub mod state;
pub mod stream;

pub use state::{ApiError, ApiResult, AppState, ResumeHook};
pub use stream::cross_origin_guard;

use axum::routing::{get, patch, post};
use axum::Router;
use stream::PUSH_SUBSCRIPTIONS_PREFIX;

/// 构建完整 router。
pub fn build_router(state: AppState) -> Router {
    Router::new()
        // ── 项目 ──
        .route(
            "/projects",
            get(routes::projects::list).post(routes::projects::create),
        )
        .route(
            "/projects/{id}",
            patch(routes::projects::patch).delete(routes::projects::delete),
        )
        .route("/projects/analyze", post(routes::projects::analyze))
        .route("/projects/{id}/analysis", get(routes::projects::analysis))
        // ── 任务 ──
        .route(
            "/tasks",
            get(routes::tasks::list).post(routes::tasks::create),
        )
        .route("/tasks/{id}", get(routes::tasks::detail))
        .route("/tasks/{id}/stream", get(routes::tasks::stream))
        .route("/tasks/{id}/flow", get(routes::tasks::flow))
        .route("/tasks/{id}/metrics", get(routes::tasks::metrics))
        .route("/tasks/{id}/resume", post(routes::tasks::resume))
        // 手动按住 / 重跑本阶段（决策 276）：**非 pending 任务**的那两颗钮。
        // 「续跑」不在这里——它是 `user_paused` 那一行 pending 的 `resume(continue)`，
        // 与其余每一种 pending 同一套机制（`crate::actions` 的动作表）。
        .route("/tasks/{id}/pause", post(routes::tasks::pause))
        .route("/tasks/{id}/rerun", post(routes::tasks::rerun))
        .route("/tasks/{id}/retry", post(routes::tasks::retry))
        // 任务级托管（决策 210① / 票 08）：开关由人拨，授权范围只有一个动作。
        .route(
            "/tasks/{id}/stewardship",
            post(routes::tasks::set_stewardship),
        )
        .route("/tasks/{id}/cancel", post(routes::tasks::cancel))
        .route("/tasks/{id}/archive", post(routes::tasks::archive))
        .route("/tasks/{id}/split", post(routes::tasks::split))
        .route(
            "/tasks/{id}/model-override",
            post(routes::tasks::model_override),
        )
        .route("/tasks/{id}/review", post(routes::tasks::review))
        .route(
            "/tasks/{id}/merge/decision",
            post(routes::tasks::merge_decision),
        )
        .route("/tasks/{id}/files/{*path}", get(routes::tasks::file))
        .route(
            "/tasks/{id}/conversations",
            get(routes::tasks::conversations),
        )
        .route(
            "/tasks/{id}/conversations/{run_id}",
            get(routes::tasks::conversation),
        )
        .route(
            "/tasks/{id}/conversations/{run_id}/messages",
            get(routes::tasks::conversation_messages),
        )
        .route("/tasks/{id}/commands", get(routes::tasks::commands))
        .route("/tasks/{id}/commands/{cmd_id}", get(routes::tasks::command))
        .route(
            "/tasks/{id}/commands/{cmd_id}/output",
            get(routes::tasks::command_output),
        )
        // ── provider ──
        .route(
            "/providers",
            get(routes::providers::list).post(routes::providers::create),
        )
        .route(
            "/providers/{id}",
            patch(routes::providers::patch).delete(routes::providers::delete),
        )
        // 连通性探针（决策 160）：建任务前验证密钥 / 模型 / 地址
        .route("/providers/test", post(routes::providers::test))
        // ── 阶段级 agent 配置（决策 22 / 46 / 66）──
        .route("/stage-configs", get(routes::stage_configs::list))
        .route(
            "/stage-configs/{stage}",
            axum::routing::put(routes::stage_configs::put).delete(routes::stage_configs::delete),
        )
        // ── 全局指标 ──
        .route("/metrics", get(routes::tasks::global_metrics))
        // ── 对讲台（决策 182 / 204，票 01 / 03）──
        // 全部任务无关、项目无关：值班长在首启空 home 上也要答得上话。
        // 这里**没有任何写动作**——它只说话，动手的键仍在任务详情里由后端下发（决策 101）。
        // 「一条长会话」= 一排班次（决策 204）：会话隔离对话上下文与页头读数，
        // 不隔离权限、也不隔离态势快照。
        .route("/foreman/session", get(routes::foreman::session))
        .route(
            "/foreman/sessions",
            get(routes::foreman::sessions).post(routes::foreman::create_session),
        )
        .route(
            "/foreman/sessions/{id}",
            patch(routes::foreman::rename_session),
        )
        .route(
            "/foreman/sessions/{id}/archive",
            post(routes::foreman::archive_session),
        )
        // 停钮（决策 294 / 票 09）：只停**人这一轮**（值守轮归开关，裁决 10）。
        .route(
            "/foreman/sessions/{id}/cancel",
            post(routes::foreman::cancel_session),
        )
        .route("/foreman/messages", post(routes::foreman::send))
        .route("/foreman/stream", get(routes::foreman::stream))
        // 回执标签（决策 247⑤）：全量清单的 `{name, label}`，**不按档位滤**——回执标的是
        // 历史上的工具调用，昨天的回执今天仍要能翻译。
        .route("/foreman/tools", get(routes::foreman::tools))
        // 未消费待办的只读计数（决策 307，票 06）：对讲台页头那枚读数读它。与
        // `turn_in_flight` 无关——值守轮排队时最需要看见它（那正是 crumb 不出现的时候）。
        .route("/foreman/attention", get(routes::foreman::attention))
        // 提议（决策 188 / 207）：写动作的落库形态。**执行不是一条新路**——它按提议里的
        // (工具, 参数) 走既有的那条端点，同一套校验、同一套闸门。这里能做的三件事是
        // 一次一按、过期、态势变化的拒执（决策 207 的三条硬约束）。
        .route("/foreman/commands", get(routes::foreman::commands))
        .route("/foreman/proposals", get(routes::foreman::proposals))
        .route(
            "/foreman/proposals/{id}/execute",
            post(routes::foreman::execute_proposal),
        )
        .route(
            "/foreman/proposals/{id}/reject",
            post(routes::foreman::reject_proposal),
        )
        // ── 值守轮的全局开关（决策 287 / 票 02）──
        // **不落 /foreman/**：那一族未接线时 503，而开关是机器级事实（机器级设置页要看得到）。
        // 保存即活：值守循环每 10s 读一次库里的这一行。
        .route(
            "/foreman-watch",
            get(routes::foreman_watch::settings).put(routes::foreman_watch::set_enabled),
        )
        // ── 命令执行走 rtk 的开关（决策 297 / 票 03、05）──
        // 与 `/foreman-watch` 同族的机器级事实（**不是** `/foreman/*`：那族未接线时 503，
        // 而设置页要能读到「现在是缺省关」）。GET 带一次**活体探测**；PUT 探测失败也 200。
        .route(
            "/rtk",
            get(routes::rtk::settings).put(routes::rtk::set_enabled),
        )
        // ── 技能市场（决策 172⑤，票 09）：本地导入 / 目录扫描 / 卸载。全程离线 ──
        // 子 router 自带 state（import 路由要单独放宽请求体上限），故先 merge 再进防护层。
        .merge(routes::skills::routes(state.clone()))
        // ── 技能来源：GitHub 仓（决策 194，取代票 10 的自定 registry）。仓名单默认空 =
        // 不允许远程安装；访问层由 AppState 注入（生产 `Libgit2Repo` / 测试离线 fixture），
        // 故契约测试离线 ──
        .merge(routes::market::routes(state.clone()))
        // ── 服务自述与局域网分享（决策 167）：读端点纯 GET，无状态变更 ──
        .route("/server-info", get(routes::server_info::info))
        .route("/server-info/qr.svg", get(routes::server_info::qr_svg))
        // 绑定开关（决策 186）：**只允许回环来源**（处理器自己判，见 `PeerLoopback`），
        // 运行时改绑 + 记住选择。它是全站唯一能把服务暴露到局域网的入口，故护栏在两处：
        // 这里的对端判定，以及局域网形态下必然生效的配对令牌层。
        .route(
            "/server/lan",
            post(routes::server_info::set_lan).delete(routes::server_info::clear_lan),
        )
        // ── 配对（决策 182㉖㉗㉘，票 07）──
        // 读取口自己判来源是否回环（局域网来源 403）；重置是写请求，局域网形态下由
        // pairing_guard 护住、回环豁免（丢了令牌必须还能从本机复位）。
        .route("/pairing/token", get(routes::pairing::token))
        .route("/pairing/reset", post(routes::pairing::reset))
        // ── 离线通知设置（决策 272⑥⑦⑧；284② 添礼貌单元）：读数 / 总开关 / 通道单元 /
        // 礼貌单元 / 探针 ──
        // 写请求照全站护栏走（跨源 + 局域网形态下的配对令牌）；读数里秘密一律掩码
        // （`***`），GET 与 /server-info 同一档：机器级事实，不是密钥。
        .route(
            "/notify/settings",
            get(routes::notify::settings).put(routes::notify::set_enabled),
        )
        .route(
            "/notify/channel",
            axum::routing::put(routes::notify::save_channel).delete(routes::notify::clear_channel),
        )
        .route(
            "/notify/politeness",
            axum::routing::put(routes::notify::save_politeness)
                .delete(routes::notify::clear_politeness),
        )
        .route("/notify/test", post(routes::notify::test_channel))
        // ── 浏览器推送的订阅（spec `.scratch/pwa-webpush/` 票 02）──
        // **读也过配对守卫**（决策 336 之后全站都过）：清单里每条都是往那台设备推报文的
        // 能力的一半，而订阅是持续的外泄管道（决策 167 的定点加强）。
        // 地址由那个常量拼出来而不是各写一遍字面量——两处对齐靠手写注释是迟早会漂的东西。
        .route(
            &format!("{PUSH_SUBSCRIPTIONS_PREFIX}subscriptions"),
            get(routes::notify::list_subscriptions)
                .post(routes::notify::subscribe)
                .delete(routes::notify::clear_subscriptions),
        )
        .route(
            &format!("{PUSH_SUBSCRIPTIONS_PREFIX}subscriptions/{{id}}"),
            axum::routing::delete(routes::notify::delete_subscription),
        )
        // 前端静态资源同源托管（决策 155）。**注册在防护层之前**（决策 336 修订了它原来的
        // 位置）：`Router::layer` 只包住**登记在它之前**的路由，而全站闸门必须把入口页与
        // 资产一起罩住——否则未配对的人拿到的是一个能加载、每个数据请求都 403 的空看板
        // （决策 336 就是冲这个来的）。静态路由全是 GET/HEAD，故它过跨源防护时走的是
        // 那条「安全方法直接放行」的早退，不进跨源矩阵。
        .merge(assets::static_routes())
        // 配对令牌层（决策 182㉖㉗㉘，票 07；决策 336 扩到全站）：它要读 peer_address
        // 归一后的来源地址，故必须排在跨源防护**之后**（更内层）。axum 的 `Router::layer`
        // 后挂者在外、先执行，故它登记在 cross_origin_guard 之前、static_routes 之后。
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            stream::pairing_guard,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            stream::cross_origin_guard,
        ))
        // 对端地址层（决策 182 / 票 06）：必须在跨源防护**之前**运行——配对令牌与
        // 「仅回环可读」的读取端点都要按来源地址判定（决策 167）。axum 的 `Router::layer`
        // 逐个包裹已登记的路由，后挂的层在外、先于先挂的执行，故它只能写在
        // cross_origin_guard **之后**（写在之前会被后者包在里面、后于它执行）。
        .layer(axum::middleware::from_fn(peer::peer_address))
        // 响应压缩（决策 361，票 01）：最外层——静态资源、错误报文与全部 JSON 端点
        // 一起受益，且与传输形态（明文 / TLS）无关（两者都走这个 router）。
        .layer(compression_layer())
        .with_state(state)
}

/// 压缩判定用的谓词（决策 361）。
type CompressionPredicate = tower_http::compression::predicate::And<
    tower_http::compression::predicate::DefaultPredicate,
    tower_http::compression::predicate::NotForContentType,
>;

/// 响应压缩层：`Accept-Encoding` 协商，**br 优先、gzip 回退**（决策 361，票 01）。
///
/// 这是本批性价比最高的一刀：一处改动、全局受益。106 实测 `/tasks/{id}/commands` 的
/// 1,330,415 字节此前原样过网（响应头只有 `content-type` + `content-length`），
/// 而它在公网链路上要走 8.5–10.9 秒。
///
/// **事件流必须原样过网**：压缩层在编码器里攒字节，压在缓冲里的 SSE 看起来就是
/// 「直播卡住」。排除**按响应 content-type 判**（`text/event-stream`），不按路由名单：
/// 名单只认当下这两个端点（`/tasks/{id}/stream`、`/foreman/stream`），而「事件流不压缩」
/// 这条不变式属于响应本身——将来任何新加的流会自动落在正确的一侧。axum 的 `Sse`
/// 必然带上这个 content-type（`response/sse.rs`），故这一条在响应头面上可判、可断言
/// （见 `tests/integration/api_contract.rs` 的 `sse_is_never_compressed`）。
///
/// 为什么把 `NotForContentType::SSE` 显式写出来（`DefaultPredicate` 当前也含它）：
/// **承重行为不寄托在第三方默认值上**（与 `skill_import` 不把路径穿越判定外包给 zip
/// 同一条理由）——上游一次 minor 调整不该让直播静默卡住。
fn compression_layer() -> tower_http::compression::CompressionLayer<CompressionPredicate> {
    use tower_http::compression::predicate::{DefaultPredicate, NotForContentType, Predicate as _};
    tower_http::compression::CompressionLayer::new()
        .compress_when(DefaultPredicate::new().and(NotForContentType::SSE))
}
