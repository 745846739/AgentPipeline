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

/// `POST /providers/test` 的请求体：允许**未保存**的表单值直接探测。
///
/// `id` 命中已存 provider 时作为基底——表单回显的 `api_key = "***"`（决策 112）
/// 不覆盖真值；显式传入的新密钥则覆盖。`id` 缺失时必须有显式 `api_key`。
#[derive(Debug, Deserialize)]
pub struct TestProviderBody {
    #[serde(default)]
    pub id: Option<String>,
    pub vendor: String,
    pub model: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
}

/// `POST /providers/test`：连通性探针（决策 160，主流程票 03）。
///
/// 响应体是 [`ConnectionTest`]，**只含结论不含密钥**——决策 112 的掩码语义
/// 不因本端点弱化。探测成功/失败都是 200（它测的是配置，不是本端点）。
pub async fn test(
    State(state): State<AppState>,
    Json(body): Json<TestProviderBody>,
) -> ApiResult<impl IntoResponse> {
    let stored = match &body.id {
        Some(id) => state.store.get_provider(id).await.map_err(map_core_error)?,
        None => None,
    };
    let (vendor, model, base_url, api_key) = match &stored {
        Some(p) => (
            body.vendor.clone(),
            body.model.clone(),
            body.base_url.clone().or_else(|| p.base_url.clone()),
            match body.api_key.as_deref() {
                None | Some("***") => p.api_key.clone(),
                Some(k) => Some(k.to_string()),
            },
        ),
        None => {
            let api_key = body.api_key.clone().filter(|k| k != "***");
            if api_key.is_none() {
                return Err(ApiError::bad_request(
                    "缺少 api_key：未命中已存 provider 时必须显式提供（或传 id 沿用已存密钥）",
                ));
            }
            (
                body.vendor.clone(),
                body.model.clone(),
                body.base_url.clone(),
                api_key,
            )
        }
    };
    let provider = Provider {
        id: body.id.unwrap_or_else(|| "test".into()),
        vendor,
        model,
        // 探针不消费 context_window，给个占位即可
        context_window: stored.as_ref().map_or(8000, |p| p.context_window),
        base_url,
        api_key,
        enabled: true,
        created_at: state.store.now(),
        updated_at: state.store.now(),
    };
    let result = agentpipeline_core::agent::providers::test_provider_connection(&provider).await;
    Ok(Json(json!({ "test": result })))
}
