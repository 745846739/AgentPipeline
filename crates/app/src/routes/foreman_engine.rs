//! 对讲台的默认引擎（决策 424④，票 talk-cli-engine 04）。
//!
//! | 方法 | 路径 | 说明 |
//! |---|---|---|
//! | GET | `/foreman-engine` | 读数：缺省引擎 + provenance |
//! | PUT | `/foreman-engine` | 保存缺省引擎（`{engine}` 一个字段），只影响**新**班次 |
//!
//! 形态照 `/foreman-watch`：**路径故意不落 `/foreman/*`**——那一族在「值班长未接线」
//! 时一律 503，而这个设置是机器级事实，值班长没接线时设置页照样要能读能改。
//!
//! 它只定「`+ 新班次` 不带参数时开什么引擎」；引擎在建会话时选定、**会话内锁定**
//! （决策 424④）——没有改既有班次引擎的写路径，选错了开新班次。

use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use agentpipeline_core::storage::foreman::{FOREMAN_ENGINE_API, FOREMAN_ENGINE_CLI};

use crate::state::{map_core_error, ApiError, ApiResult, AppState};

/// `GET /foreman-engine`：设置页的读数。
pub async fn settings(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let store = state.store.clone();
    let engine = store
        .foreman_default_engine()
        .await
        .map_err(map_core_error)?;
    let overridden = store
        .foreman_default_engine_has_override()
        .await
        .map_err(map_core_error)?;
    Ok(Json(json!({
        "engine": engine,
        // 诚实口径（决策 257）：这一份是谁定的。
        "origin": if overridden { "settings" } else { "default" },
    })))
}

#[derive(Debug, Deserialize)]
pub struct EngineBody {
    pub engine: String,
}

/// `PUT /foreman-engine`：保存缺省引擎。
pub async fn set_engine(
    State(state): State<AppState>,
    Json(body): Json<EngineBody>,
) -> ApiResult<Json<serde_json::Value>> {
    if body.engine != FOREMAN_ENGINE_API && body.engine != FOREMAN_ENGINE_CLI {
        return Err(ApiError::bad_request(format!(
            "未知引擎：{}（可选 api / cli）",
            body.engine
        )));
    }
    state
        .store
        .set_foreman_default_engine(&body.engine)
        .await
        .map_err(map_core_error)?;
    Ok(Json(json!({
        "engine": body.engine,
        "origin": "settings",
    })))
}
