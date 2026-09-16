//! 技能市场：远程 registry 搜索与安装（决策 172⑤，票 10）。
//!
//! | 端点 | 作用 |
//! |---|---|
//! | `GET /market/search?q=` | 按关键词查 registry，返回候选（未放行的来源不进候选） |
//! | `POST /market/install` | 下载 → `sha256` 校验 → 落盘（复用票 09） |
//! | `GET /market/config` | 当前的来源白名单、它来自哪一级（界面 / 配置文件）、索引地址（决策 187） |
//! | `PUT /market/config` | 保存来源白名单（校验 → 落库 → **当场换客户端**，不必重启） |
//! | `DELETE /market/config` | 清掉界面那份，回到 `config.toml` 的 `[market]` |
//!
//! 索引格式与四类失败的划分见 [`agentpipeline_core::agent::market`] 的模块头注释。
//!
//! ## 与票 09 的关系：市场是**另一个来源**，不是另一条落盘路径
//!
//! 下载下来的 zip 走同一个 `SkillPackage::from_zip` + `install`，因此结构校验、同名冲突、
//! 路径穿越防护一处生效、两处受益。这就是「落盘只实现一次」的价值：远程包不许比本地上传的
//! 包享有更宽的路（`crates/core/tests/market.rs` 有一条用例专门钉这一点）。
//!
//! ## 客户端从 `AppState` 取，不在端点里 new
//!
//! `AppState::market` 是 `Option<Arc<dyn MarketClient>>`：生产在 `serve` 里注入
//! `HttpMarketClient`，L3 契约测试注入 testkit 的 `FakeMarket`。因此这几条端点契约可以在
//! **完全离线**的前提下被钉住——包括「摘要不符」「来源未放行」这些真网络根本没法稳定复现的
//! 路径。`None`（未配置来源）不是 panic 而是明确的错误响应：空白名单 = 不允许远程安装。
//!
//! ## 来源白名单能在运行时改（决策 187）
//!
//! 票 10 只留了 `config.toml` 一条路：改完要重启，且界面上**根本无处可改**——于是
//! 「技能市场」这个能力对不读配置文件的用户等于不存在。现在多了界面上的那一级：
//! 保存走 [`Store::set_market_sources_override`]，并**当场**用新来源重搭客户端
//! （[`AppState::set_market_override`]），所以保存完立刻能搜。优先级「界面 > 配置文件」，
//! 清掉界面那份就回到配置文件（`DELETE` 给的就是这条路）。
//!
//! 两处口径共用同一个校验函数（[`agentpipeline_core::config::validate_market_sources`]）：
//! 放行一个来源等于允许从它下载引导 agent 的正文，这条判定不能有第二个版本。

use agentpipeline_core::agent::market;
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::state::{ApiError, ApiResult, AppState};
use std::sync::Arc;

pub fn routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/market/search", get(search))
        .route("/market/install", post(install))
        .route(
            "/market/config",
            get(config).put(save_config).delete(clear_config),
        )
        .with_state(state)
}

/// 取生效的市场客户端（界面 > 配置文件，决策 187）；未配置时给出可操作的错误。
///
/// 空白名单是**合法配置**（= 不装远程技能），因此这里的报文要说清「怎么开」，
/// 而不是含糊的「服务器内部错误」。
pub(crate) fn client(state: &AppState) -> ApiResult<Arc<dyn market::MarketClient>> {
    state.market_client().ok_or_else(|| {
        ApiError::bad_request(
            "未配置技能市场来源：请在「技能市场」页填入可信来源的 origin\
             （如 https://skills.example.com），或写进配置的 [market] allowed_sources。\
             默认空 = 不允许远程安装技能；本地导入不受影响",
        )
    })
}

// ─────────────────────── GET / PUT / DELETE /market/config ───────────────────────

/// `GET /market/config`：界面上的来源编辑器要的读数。
///
/// `origin` 说清这一份**是谁定的**（界面 / 配置文件）——用户改 `config.toml` 却发现
/// 「改了没用」时，答案必须在这一页上看得见，而不是靠他去猜优先级。
pub async fn config(State(state): State<AppState>) -> ApiResult<impl IntoResponse> {
    Ok(Json(config_json(&state)))
}

fn config_json(state: &AppState) -> serde_json::Value {
    let from_settings = state.market_override().is_some();
    let sources = state.market_sources();
    json!({
        "sources": sources,
        "origin": if from_settings { "settings" } else { "config" },
        // 索引来源 = 列表的第一项（`{source}/index.json`），空表时没有
        "index_source": sources.first(),
        "client_ready": state.market_client().is_some(),
    })
}

#[derive(Debug, Deserialize)]
pub struct SourcesBody {
    /// 归一前的来源 origin 列表（含大小写 / 尾斜杠都照收，校验函数会归一）。
    #[serde(default)]
    pub sources: Vec<String>,
}

/// `PUT /market/config`：保存界面上的来源白名单（决策 187）。
///
/// 三步，顺序是刻意的：**先校验 → 再落库 → 最后换客户端**。
/// - 校验用与 `config.toml` 完全相同的那一个函数（归一 + 重复去重 + 传输安全）；
/// - 落库失败就整条失败，内存里的那一份不动（「界面上显示改了、重启后却不是」最坏）；
/// - 换客户端在最后：它是内存动作，不会失败（空表 → `None`，端点转成可操作报文）。
///
/// 保存是**显式动作**：空数组是合法输入（= 不允许远程安装），与「没保存过」不同。
pub async fn save_config(
    State(state): State<AppState>,
    Json(body): Json<SourcesBody>,
) -> ApiResult<impl IntoResponse> {
    let sources = agentpipeline_core::config::validate_market_sources(&body.sources)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    state
        .store
        .set_market_sources_override(&sources)
        .await
        .map_err(crate::state::map_core_error)?;
    // 当场生效：保存完立刻能搜，不必重启（这正是这一页存在的理由）。
    // 客户端按**这次保存的来源**搭，不读 state——`set_market_override` 还没执行，
    // state 里仍是上一份（那会让「保存了却还是按老来源搜」）。
    let client = build_client(&sources)?;
    state.set_market_override(sources, client);
    Ok(Json(config_json(&state)))
}

/// `DELETE /market/config`：清掉界面那份，回到 `config.toml` 的 `[market]`。
pub async fn clear_config(State(state): State<AppState>) -> ApiResult<impl IntoResponse> {
    state
        .store
        .clear_market_sources_override()
        .await
        .map_err(crate::state::map_core_error)?;
    state.clear_market_override();
    Ok(Json(config_json(&state)))
}

/// 按一组来源现搭客户端（第一个来源做索引地址，与启动路径同一约定）。
///
/// 空表 → `None`（= 不允许远程安装，端点转成可操作报文）。构造失败仍然报 400 而不是
/// `None`：那是「来源畸形」而非「没配来源」，把它糊成后者会让用户永远配不上。
fn build_client(sources: &[String]) -> ApiResult<Option<Arc<dyn market::MarketClient>>> {
    Ok(match sources.first() {
        Some(source) => Some(Arc::new(
            market::HttpMarketClient::new(source)
                .map_err(|e| ApiError::bad_request(e.to_string()))?,
        )),
        None => None,
    })
}

// ─────────────────────────── GET /market/search ───────────────────────────

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    /// 关键词（名字或描述命中；省略 = 列出全部已放行来源的技能）。
    #[serde(default)]
    pub q: Option<String>,
}

/// `GET /market/search?q=`：查 registry 并返回候选清单。
///
/// 未放行来源的条目**不进候选**——让用户看不到装不上的东西，比让他点了再报错好。
pub async fn search(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> ApiResult<impl IntoResponse> {
    let sources = state.market_sources();
    let client = client(&state)?;
    let all = client.index().await.map_err(map_market_error)?;
    let q = query.q.unwrap_or_default();
    let hits = market::search(all, &q, &sources);

    let skills: Vec<serde_json::Value> = hits.iter().map(entry_json).collect();
    Ok(Json(json!({
        "skills": skills,
        "sources": sources,
        "query": q,
    })))
}

fn entry_json(e: &market::IndexEntry) -> serde_json::Value {
    json!({
        "name": e.name,
        "version": e.version,
        "sha256": e.sha256,
        "source": e.source,
        "description": e.description,
        "url": e.url,
    })
}

// ────────────────────────── POST /market/install ──────────────────────────

#[derive(Debug, Deserialize)]
pub struct InstallBody {
    /// 要安装的技能名（须在索引里存在）。
    pub name: String,
    /// 同名覆盖的**显式确认**（与票 09 同口径，默认拒绝）。
    #[serde(default)]
    pub overwrite: bool,
}

/// `POST /market/install`：下载 → 校验摘要 → 落盘。
///
/// 顺序见 [`market::install_from_market`]：每一步都尽量在下载之前失败（条目存在 →
/// 来源放行 → **下载地址的 origin 也放行** → 下载 → 摘要 → 落盘）。
pub async fn install(
    State(state): State<AppState>,
    Json(body): Json<InstallBody>,
) -> ApiResult<impl IntoResponse> {
    let client = client(&state)?;
    let root = state.home.skills_dir();
    let info = market::install_from_market(
        client.as_ref(),
        &root,
        &body.name,
        &state.market_sources(),
        body.overwrite,
    )
    .await
    .map_err(map_market_error)?;

    Ok(Json(json!({
        "skill": {
            "name": info.name,
            "description": info.description,
            "sibling_count": info.sibling_count,
        }
    })))
}

/// 市场错误 → API 错误。
///
/// `Error::Market` 走 [`crate::state::map_core_error`] 会落到 500 兜底，因为它的可操作
/// 提示藏在 `message` 里而 `map_core_error` 只认 `Validation` / `Conflict` 几种。
/// 这里显式映射成**几个互不混淆的状态码**：
///
/// | `kind` | 状态码 | 为什么 |
/// |---|---|---|
/// | `market_network` | 502 | 请求没问题，对面没应答——用户该做的是稍后重试 |
/// | `market_not_found` | 404 | 索引里就没这个技能——用户该做的是换个名字 |
/// | `market_digest_mismatch` / `market_source_not_allowed` / `market_index_malformed` | 400 | 拒绝安装，属请求侧该改的东西 |
///
/// 分开的意义与 `kind` 本身一致：几种失败对应完全不同的动作。`Error::Market` 的 `raw`
/// （期望与实际摘要、HTTP 状态、索引片段）经 `with_detail` 作为响应体的 `detail` 字段下发，
/// 与 `message` 分开——面向用户的话与诊断原始串不该混在一起。
pub(crate) fn map_market_error(err: agentpipeline_core::Error) -> ApiError {
    let detail = err.market_kind().map(|(_, raw)| raw.to_string());
    let mapped = match &err {
        agentpipeline_core::Error::Market { kind, message, .. } if kind == market::KIND_NETWORK => {
            ApiError::bad_gateway(message)
        }
        agentpipeline_core::Error::Market { kind, message, .. }
            if kind == market::KIND_NOT_FOUND =>
        {
            ApiError::not_found(message)
        }
        agentpipeline_core::Error::Market { message, .. } => ApiError::bad_request(message),
        // 同名冲突（票 09 的 Conflict）→ 409，其余走通用映射
        _ => crate::state::map_core_error(err),
    };
    match detail {
        Some(d) => mapped.with_detail(d),
        None => mapped,
    }
}
