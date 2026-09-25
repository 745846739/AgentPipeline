//! axum API 层（docs/implementation.md §11.7；契约测试见 tests/）。
//!
//! 设计要点：
//! - 路由构建与二进制启动分离——L3 测试用 tower oneshot 直接打 in-process router，不 spawn 二进制
//!   （决策 144）；
//! - 跨源防护中间件（决策 128）：只拦写请求；SSE 是纯 GET，不受影响；
//! - 对端地址层（决策 182 / 票 06）：把连接信息归一成 [`peer::PeerAddr`]，排在跨源防护
//!   之前，供票 07 判定「仅回环可读」；
//! - 配对令牌层（决策 182㉖㉗㉘ / 票 07）：只在局域网形态拦写请求与 `/foreman/*`，
//!   排在最后（内层），回环来源豁免；
//! - 前端 dist 由 build.rs 内嵌并同源托管（决策 155）；dist 缺失时退化为构建提示页；
//! - `/server-info` 暴露局域网访问地址与二维码（决策 167），供手机扫码接入；
//! - `api_key` 读接口只回显 `***`（决策 112）。

pub mod assets;
pub mod lan;
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
        .route("/foreman/messages", post(routes::foreman::send))
        .route("/foreman/stream", get(routes::foreman::stream))
        // 回执标签（决策 247⑤）：全量清单的 `{name, label}`，**不按档位滤**——回执标的是
        // 历史上的工具调用，昨天的回执今天仍要能翻译。
        .route("/foreman/tools", get(routes::foreman::tools))
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
        // 配对令牌层（决策 182㉖㉗㉘，票 07）：**必须最后执行**（最内层）——它要读
        // peer_address 归一后的来源地址，且排在跨源防护之后，只处理已过跨源判定的请求。
        // axum 的 `Router::layer` 后挂者在外、先执行，故它登记在 cross_origin_guard 之前。
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            stream::pairing_guard,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            stream::cross_origin_guard,
        ))
        // 对端地址层（决策 182 / 票 06）：必须在跨源防护**之前**运行——票 07 的配对
        // 令牌与「仅回环可读」的读取端点都要按来源地址判定（决策 167）。axum 的
        // `Router::layer` 逐个包裹已登记的路由，后挂的层在外、先于先挂的执行，故它只能
        // 写在 cross_origin_guard **之后**（写在之前会被后者包在里面、后于它执行）。
        // 与防护层同样登记在 `merge(assets::static_routes())` 之前 → 覆盖面一致。
        .layer(axum::middleware::from_fn(peer::peer_address))
        // 前端静态资源同源托管（决策 155）：放在防护层之后注册——全 GET/HEAD，
        // 防护只拦写请求，静态路由不进跨源矩阵。
        .merge(assets::static_routes())
        .with_state(state)
}
