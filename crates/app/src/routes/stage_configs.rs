//! 阶段级 agent 配置端点（决策 22 / 46 / 66 / 111 / 129）。
//!
//! 写入校验复用启动校验本体 [`validate_startup`]：把「现有配置 + 本次改动」当作一份
//! 完整配置做校验，拒绝任何会导致启动 fail fast 的写入（provider 不存在 / 被禁用 /
//! vendor 不受支持、persona_path 不可读或为空、引用了不存在的 skill）。
//! 这样端点的准入语义与启动语义同源，不会漂移（决策 47 / 103 / §10.6.4）。

use agentpipeline_core::config::{discover_available_skills, validate_startup, StartupInputs};
use agentpipeline_core::types::{Stage, StageConfig};
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::str::FromStr;

use crate::state::{map_core_error, ApiError, ApiResult, AppState};

/// 阶段键可以是真实阶段，也可以是伪阶段的 `stage_configs` 键（决策 67 / 87），
/// 外加值班长（决策 182①：它是第 4 个配置 key，但不是 `PseudoStage` 的变体）。
///
/// **必须与 `frontend/src/lib/stageConfigs.ts` 的 `PSEUDO_KEYS` 同步**：那份决定设置页
/// 列不列得出这一行，这份决定后端收不收这一行。不同步的表现是「界面上填好、保存被 400
/// 拒掉」，而 400 的理由写着「未知阶段」——看的人只会当成界面 bug 去查前端。
const PSEUDO_STAGE_KEYS: [&str; 4] = [
    "conflict_check",
    "validator_cross_check",
    "project_analysis",
    agentpipeline_core::pipeline::foreman::FOREMAN_STAGE_KEY,
];

fn validate_stage_key(stage: &str) -> Result<(), ApiError> {
    if Stage::from_str(stage).is_ok() || PSEUDO_STAGE_KEYS.contains(&stage) {
        return Ok(());
    }
    Err(ApiError::bad_request(format!(
        "未知阶段：{stage}（须为真实阶段或伪阶段键之一）"
    )))
}

#[derive(Debug, Deserialize)]
pub struct PutStageConfig {
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub persona_path: Option<String>,
    #[serde(default)]
    pub persona_append: Option<String>,
    #[serde(default)]
    pub tools_json: Option<serde_json::Value>,
    #[serde(default)]
    pub skills_json: Option<serde_json::Value>,
    #[serde(default)]
    pub idle_timeout_sec: Option<u64>,
    #[serde(default)]
    pub max_duration_sec: Option<u64>,
    #[serde(default)]
    pub node_overrides_json: Option<serde_json::Value>,
}

/// 用「现有配置 + 待改动」跑一遍启动校验；`removed` 是本次要从集合里去掉的阶段键。
///
/// `pub(crate)`：`routes::skills` 的信任转换（票 11）复用同一道门——它改写的也是
/// `skills_json` 的形态，若各写一份校验，两处准入语义必然漂移。
pub(crate) async fn validate_prospective(
    state: &AppState,
    configs: Vec<StageConfig>,
    removed: Option<&str>,
) -> Result<(), ApiError> {
    let providers = state.store.load_providers().await.map_err(map_core_error)?;
    let inputs = StartupInputs {
        settings: state.settings.clone(),
        providers,
        stage_configs: configs
            .into_iter()
            .filter(|c| removed != Some(c.stage.as_str()))
            .collect(),
        available_skills: discover_available_skills(&state.home.skills_dir()),
        home_root: Some(state.home.root().to_path_buf()),
        // 决策 170 / 172：知识型技能的正文必须存在且非空，frontmatter name 须与目录名一致
        skills_root: Some(state.home.skills_dir()),
    };
    validate_startup(&inputs).map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(())
}

/// `GET /stage-configs`：全部阶段覆盖（未配置的阶段不出现在列表里）。
pub async fn list(State(state): State<AppState>) -> ApiResult<impl IntoResponse> {
    let configs = state
        .store
        .list_stage_configs()
        .await
        .map_err(map_core_error)?;
    Ok(Json(json!({ "stage_configs": configs })))
}

/// `PUT /stage-configs/{stage}`：整条替换（缺省字段清空为默认）。
pub async fn put(
    State(state): State<AppState>,
    Path(stage): Path<String>,
    Json(body): Json<PutStageConfig>,
) -> ApiResult<impl IntoResponse> {
    validate_stage_key(&stage)?;
    let candidate = StageConfig {
        stage: stage.clone(),
        provider_id: body.provider_id,
        temperature: body.temperature,
        max_tokens: body.max_tokens,
        persona_path: body.persona_path,
        persona_append: body.persona_append,
        tools_json: body.tools_json,
        skills_json: body.skills_json,
        idle_timeout_sec: body.idle_timeout_sec,
        max_duration_sec: body.max_duration_sec,
        node_overrides_json: body.node_overrides_json,
        updated_at: state.store.now(),
    };

    // 现有配置 + 待写入的一条 → 整体校验
    let mut prospective = state
        .store
        .list_stage_configs()
        .await
        .map_err(map_core_error)?;
    prospective.retain(|c| c.stage != stage);
    prospective.push(candidate.clone());
    validate_prospective(&state, prospective, None).await?;

    state
        .store
        .upsert_stage_config(&candidate)
        .await
        .map_err(map_core_error)?;
    Ok(Json(json!({ "stage_config": candidate })))
}

/// `DELETE /stage-configs/{stage}`：撤销覆盖、回到系统默认（不存在则 404）。
pub async fn delete(
    State(state): State<AppState>,
    Path(stage): Path<String>,
) -> ApiResult<impl IntoResponse> {
    validate_stage_key(&stage)?;
    if state
        .store
        .get_stage_config(&stage)
        .await
        .map_err(map_core_error)?
        .is_none()
    {
        return Err(ApiError::not_found(format!("阶段配置不存在：{stage}")));
    }
    // 删除同样要过启动校验：删掉被 cross_family_judge 依赖的伪阶段配置会被拒绝
    let prospective = state
        .store
        .list_stage_configs()
        .await
        .map_err(map_core_error)?;
    validate_prospective(&state, prospective, Some(&stage)).await?;

    state
        .store
        .delete_stage_config(&stage)
        .await
        .map_err(map_core_error)?;
    Ok(Json(json!({ "ok": true })))
}
