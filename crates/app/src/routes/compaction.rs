//! 「管线压缩」设置（long-run-budget 票 02）。
//!
//! | 方法 | 路径 | 说明 |
//! |---|---|---|
//! | GET | `/compaction` | 两个旋钮的有效值 + 逐字段 provenance（`default` / `settings`） |
//! | PUT | `/compaction` | 保存两个旋钮（两格都必须给、必须 > 0），回同一份读数 |
//!
//! 与 `/foreman-watch` / `/offload` 同族的全局设置端点：**GET 与 PUT 回同一份
//! readout**（结构保证），保存即活——流水线每 attempt、值班长每轮懒读 DB 覆盖层
//! （`kanban_compaction`，迁移 0042），下一轮就按新值走，不经重启。
//!
//! provenance 逐字段：列 NULL = 没保存过 = `default`（值来自 config.toml / 缺省）；
//! 非 NULL = `settings`——**保存值等于缺省值也是 `settings`**（诚实口径，决策 257：
//! 这份状态是谁定的，与值是什么都无关）。
//!
//! 注意 `conversation_max_chars`（落库截断，缺省 20 万字符）**不在这张卡上**：
//! 它管「一条会话行留多少痕」，不管模型上下文（票 02 的分家裁决），改它走 config.toml。

use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::state::{map_core_error, ApiResult, AppState};

/// 读数外壳：GET 与 PUT 两端点共用，同形是**结构保证**而不是纪律。
///
/// `effective` 拿的是 config 值（`state.settings`，启动冻结的那份）——DB 覆盖列
/// 非 NULL 时读数里的两格用覆盖值替换。config 值也摆出来（`config_*` 两格），
/// 设置页能显示「现在生效的是谁给的什么」。
async fn readout(state: &AppState) -> ApiResult<serde_json::Value> {
    let overrides = state
        .store
        .compaction_overrides()
        .await
        .map_err(map_core_error)?;
    let effective_tokens = overrides
        .conversation_max_tokens
        .unwrap_or(state.settings.conversation_max_tokens);
    let effective_rounds = overrides
        .keep_recent_rounds
        .unwrap_or(state.settings.keep_recent_rounds);
    Ok(json!({
        "conversation_max_tokens": effective_tokens,
        // 诚实口径（决策 257）：这份状态是谁定的
        "conversation_max_tokens_origin": if overrides.conversation_max_tokens.is_some() { "settings" } else { "default" },
        "keep_recent_rounds": effective_rounds,
        "keep_recent_rounds_origin": if overrides.keep_recent_rounds.is_some() { "settings" } else { "default" },
        // config 层的值（origin=default 时与上一格相同）；摆出来让「谁覆盖了谁」可见
        "config_conversation_max_tokens": state.settings.conversation_max_tokens,
        "config_keep_recent_rounds": state.settings.keep_recent_rounds,
    }))
}

/// `GET /compaction`：设置页的读数。
pub async fn settings(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    Ok(Json(readout(&state).await?))
}

#[derive(Debug, Deserialize)]
pub struct CompactionBody {
    pub conversation_max_tokens: usize,
    pub keep_recent_rounds: usize,
}

/// `PUT /compaction`：保存两个旋钮，回同一份读数。
///
/// 保存即活：流水线每 attempt、值班长每轮懒读覆盖层（票 02），下一轮就按新值走。
pub async fn set(
    State(state): State<AppState>,
    Json(body): Json<CompactionBody>,
) -> ApiResult<Json<serde_json::Value>> {
    if body.conversation_max_tokens == 0 || body.keep_recent_rounds == 0 {
        return Err(crate::state::ApiError::bad_request(
            "conversation_max_tokens 与 keep_recent_rounds 都必须是正整数",
        ));
    }
    state
        .store
        .set_compaction_overrides(body.conversation_max_tokens, body.keep_recent_rounds)
        .await
        .map_err(map_core_error)?;
    Ok(Json(readout(&state).await?))
}
