//! 技能市场：远程 registry 搜索与安装（决策 172⑤，票 10）。
//!
//! | 端点 | 作用 |
//! |---|---|
//! | `GET /market/search?q=` | 按关键词查 registry，返回候选（未放行的来源不进候选） |
//! | `POST /market/install` | 下载 → `sha256` 校验 → 落盘（复用票 09） |
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

use agentpipeline_core::agent::market;
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::state::{ApiError, ApiResult, AppState};

pub fn routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/market/search", get(search))
        .route("/market/install", post(install))
        .with_state(state)
}

/// 取市场客户端；未配置时给出可操作的错误（而不是 panic）。
///
/// 空白名单是**合法配置**（= 不装远程技能），因此这里的报文要说清「怎么开」，
/// 而不是含糊的「服务器内部错误」。
fn client(state: &AppState) -> ApiResult<&std::sync::Arc<dyn market::MarketClient>> {
    state.market.as_ref().ok_or_else(|| {
        ApiError::bad_request(
            "未配置技能市场来源：请在配置的 [market] allowed_sources 里加入可信来源的 origin\
             （如 https://skills.example.com），然后重启。默认空 = 不允许远程安装技能；\
             本地导入不受影响",
        )
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
    let client = client(&state)?;
    let all = client.index().await.map_err(map_market_error)?;
    let q = query.q.unwrap_or_default();
    let hits = market::search(all, &q, &state.market_sources);

    let skills: Vec<serde_json::Value> = hits.iter().map(entry_json).collect();
    Ok(Json(json!({
        "skills": skills,
        "sources": state.market_sources,
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
    let client = client(&state)?.clone();
    let root = state.home.skills_dir();
    let info = market::install_from_market(
        client.as_ref(),
        &root,
        &body.name,
        &state.market_sources,
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
fn map_market_error(err: agentpipeline_core::Error) -> ApiError {
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
