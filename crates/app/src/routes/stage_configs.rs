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
///
/// 那份承诺由 `tests/fixtures/frontend_spec_tables.json` 的 `pseudo_keys` 机器钉住
/// （票 mirror-contract/03，决策 253②）：文件尾部那条测试断言本表与它一致，前端
/// `lib/specTablesFixture.test.ts` 断言同一份表——两侧同一张表、同一断言方向。
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
    /// 环境层档位（决策 206）。**用字符串接**：枚举反序列化对 `"Auto"` 报的是 serde 的
    /// 通用错误，而这里要给一句「只能是 auto / ask / deny」——与 config.toml 那一侧
    /// 同一条口径（同一个值的两种来源，报错也该是同一种说法）。
    #[serde(default)]
    pub env_mode: Option<String>,
    /// 值班长一轮的轮数上限（决策 233① / 239）。**用 `i64` 接**：只收正整数，而用 `u32`
    /// 接的话 `-1` 会退化成 serde 的通用 422 报文，说不出「必须是正整数」这句话。
    #[serde(default)]
    pub max_rounds: Option<i64>,
    /// 值守轮一轮的生成 token 预算（决策 292 / 票 07）。**用 `i64` 接**：与 `max_rounds`
    /// 同一条口径——只收正整数，用 `u32` 接的话 `-1` 会退化成 serde 的通用 422 报文，
    /// 说不出「必须是正整数」这句话。
    #[serde(default)]
    pub watch_token_budget: Option<i64>,
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
    let env_mode = match body.env_mode.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(raw) => {
            let mode = agentpipeline_core::types::EnvMode::parse_or_message(raw)
                .map_err(ApiError::bad_request)?;
            // `ask` 只留给值班长（`run-command-permissions` 规格 §4）：流水线节点无人按那颗钮，
            // 而它又没有提议通道——配成 `ask` 的结果是**静默收掉这个阶段全部的环境写动作**。
            // 要收紧就写 `deny`（拒绝，且连工具都不给），那时意图与行为一致。
            if mode == agentpipeline_core::types::EnvMode::Ask
                && !agentpipeline_core::types::stage_may_use_ask(&stage)
            {
                return Err(ApiError::bad_request(format!(
                    "阶段 {stage} 不能配成 ask：ask 是「等人按键」，而流水线节点无人值守\
                     （它没有提议通道，配成 ask 等于静默收掉这个阶段全部的文件与命令动作）。\
                     要收紧请配 deny"
                )));
            }
            Some(mode)
        }
    };
    // 轮数上限只收正整数（决策 239）：`0` / 负数都不许，也**不提供无上限**——判据写在
    // 决策 239 里（`0` = 无上限会在配置面上造出第二个「留空即特殊」的语义）。
    let max_rounds = match body.max_rounds {
        None => None,
        Some(v) if v > 0 => Some(u32::try_from(v).map_err(|_| {
            ApiError::bad_request(format!(
                "max_rounds 超出口径（收到 {v}）：它管「一轮里能跑几次模型调用」，请给一个正整数"
            ))
        })?),
        Some(v) => {
            return Err(ApiError::bad_request(format!(
                "max_rounds 必须是正整数（收到 {v}）：`0` / 负数都不许，也没有「无上限」这一档。\
                 要恢复缺省就删掉这一格。"
            )))
        }
    };
    // token 预算与轮数上限同一条纪律（决策 292）：`0` / 负数都不许，也没有「无预算」——
    // 「留空即清成默认」已经表达了「用缺省 120k」。
    let watch_token_budget = match body.watch_token_budget {
        None => None,
        Some(v) if v > 0 => Some(u32::try_from(v).map_err(|_| {
            ApiError::bad_request(format!(
                "watch_token_budget 超出口径（收到 {v}）：它管「值守轮一轮能生成多少 token」，\
                 请给一个正整数"
            ))
        })?),
        Some(v) => {
            return Err(ApiError::bad_request(format!(
                "watch_token_budget 必须是正整数（收到 {v}）：`0` / 负数都不许，也没有「无预算」这一档。\
                 要恢复缺省就删掉这一格。"
            )))
        }
    };
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
        env_mode,
        max_rounds,
        watch_token_budget,
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

#[cfg(test)]
mod tests {
    use super::PSEUDO_STAGE_KEYS;

    /// `PSEUDO_STAGE_KEYS` 与共享规格表一致（票 mirror-contract/03，决策 253②）。
    ///
    /// 本表与前端 `stageConfigs.ts::PSEUDO_KEYS` 是**同一份规格的两个副本**，而它们的关系
    /// 不是「一个从另一个生成」——枚举推不出「哪些键不是真实阶段」。故两侧各自断言同一张
    /// 手写表（前端那份在 `lib/specTablesFixture.test.ts`）。
    ///
    /// 这条测试在这里、而不在 core 的 `types.rs`：`PSEUDO_STAGE_KEYS` 住在 app，
    /// 而它是**后端收不收这一行**的唯一判据（`validate_stage_key`）——钉它自己的那份副本。
    #[test]
    fn pseudo_stage_keys_match_the_shared_spec_table() {
        #[derive(serde::Deserialize)]
        struct Fixture {
            pseudo_keys: Vec<String>,
            stage_keys: Vec<String>,
        }

        let raw = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/frontend_spec_tables.json"
        ));
        let fixture: Fixture = serde_json::from_str(raw).expect("fixture 必须是合法 JSON");

        let ours: Vec<&str> = PSEUDO_STAGE_KEYS.to_vec();
        let theirs: Vec<&str> = fixture.pseudo_keys.iter().map(String::as_str).collect();
        assert_eq!(
            ours, theirs,
            "PSEUDO_STAGE_KEYS 与共享规格表不一致——两份副本漂了，\
             症状是「界面上填好、保存被 400 拒掉」"
        );

        // 后 4 项 == 伪键（与 core 侧 `shared_spec_tables_match_the_backend_spec` 同一条
        // 不变量，两处都断一次：core 断的是「表里那 14 项自洽」，这里断的是「app 真的只用
        // 这 4 个当键」）。
        let real: usize = fixture.stage_keys.len() - fixture.pseudo_keys.len();
        assert_eq!(&fixture.stage_keys[real..], theirs.as_slice());

        // 一个真阶段都不许混进来：它会让「真实阶段」与「配置键」两处分组打架。
        for key in &fixture.pseudo_keys {
            assert!(
                !fixture.stage_keys[..real].contains(key),
                "伪键 {key} 同时出现在真实阶段里"
            );
        }
    }
}
