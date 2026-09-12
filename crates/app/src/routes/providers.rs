//! provider 端点（决策 22 / 111 / 112）：`api_key` 明文存储，读接口只回显 `***`。

use agentpipeline_core::types::Provider;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::state::{map_core_error, ApiError, ApiResult, AppState};

#[derive(Debug, Deserialize)]
pub struct ProviderBody {
    #[serde(default)]
    pub id: Option<String>,
    pub vendor: String,
    pub model: String,
    pub context_window: u32,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

/// `GET /providers`：**只回显 `***`，不返回原值**（决策 112）。
pub async fn list(State(state): State<AppState>) -> ApiResult<impl IntoResponse> {
    let providers = state
        .store
        .list_providers_masked()
        .await
        .map_err(map_core_error)?;
    Ok(Json(json!({
        "providers": providers
            .into_iter()
            .map(|p| json!({
                "id": p.id,
                "vendor": p.vendor,
                "model": p.model,
                "context_window": p.context_window,
                "base_url": p.base_url,
                "api_key": p.api_key,   // 已是 "***"
                "enabled": p.enabled,
            }))
            .collect::<Vec<_>>()
    })))
}

/// `POST /providers`
pub async fn create(
    State(state): State<AppState>,
    Json(body): Json<ProviderBody>,
) -> ApiResult<impl IntoResponse> {
    let provider = Provider {
        id: body.id.unwrap_or_else(|| ulid::Ulid::new().to_string()),
        vendor: body.vendor,
        model: body.model,
        context_window: body.context_window,
        base_url: body.base_url,
        api_key: body.api_key,
        enabled: body.enabled,
        created_at: state.store.now(),
        updated_at: state.store.now(),
    };
    state
        .store
        .upsert_provider(&provider)
        .await
        .map_err(map_core_error)?;
    // 响应同样不回原值
    let mut masked = provider.clone();
    masked.api_key = provider.masked_api_key();
    Ok((StatusCode::CREATED, Json(json!({ "provider": masked }))))
}

#[derive(Debug, Deserialize)]
pub struct PatchProvider {
    pub vendor: Option<String>,
    pub model: Option<String>,
    pub context_window: Option<u32>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub enabled: Option<bool>,
}

/// `PATCH /providers/{id}`：未提供的字段保持原值（`api_key` 传 `***` 视为不修改）。
pub async fn patch(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PatchProvider>,
) -> ApiResult<impl IntoResponse> {
    let mut provider = state
        .store
        .get_provider(&id)
        .await
        .map_err(map_core_error)?
        .ok_or_else(|| ApiError::not_found(format!("provider 不存在：{id}")))?;

    if let Some(v) = body.vendor {
        provider.vendor = v;
    }
    if let Some(v) = body.model {
        provider.model = v;
    }
    if let Some(v) = body.context_window {
        provider.context_window = v;
    }
    if let Some(v) = body.base_url {
        provider.base_url = Some(v);
    }
    if let Some(v) = body.api_key {
        // 回显值不写回，避免把 "***" 存成真密钥
        if v != "***" {
            provider.api_key = Some(v);
        }
    }
    if let Some(v) = body.enabled {
        provider.enabled = v;
    }
    provider.updated_at = state.store.now();
    state
        .store
        .upsert_provider(&provider)
        .await
        .map_err(map_core_error)?;
    let mut masked = provider.clone();
    masked.api_key = provider.masked_api_key();
    Ok(Json(json!({ "provider": masked })))
}

/// `DELETE /providers/{id}`
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<impl IntoResponse> {
    state
        .store
        .delete_provider(&id)
        .await
        .map_err(map_core_error)?;
    Ok(Json(json!({ "ok": true })))
}
