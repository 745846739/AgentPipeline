//! 值守轮的全局开关（决策 287 / 票 02）。
//!
//! | 方法 | 路径 | 说明 |
//! |---|---|---|
//! | GET | `/foreman-watch` | 读数：开关 + provenance + config.toml 的五个节奏数（只读） |
//! | PUT | `/foreman-watch` | 保存开关（`{enabled}` 一个字段），下一趟（≤10s）即活 |
//!
//! **路径故意不落 `/foreman/*`**：那一族在「值班长未接线」时一律 503（同一能力没接上
//! 的统一口径），而开关是**机器级事实**（与 `kanban_server_bind` 同构）——值班长没接线
//! 时设置页照样要能读到「现在是缺省开」，关掉的承诺（今晚没人看）也照样要能落库。
//!
//! **只读展示的五个数**（`[pipeline] watch_*`，决策 209②③⑤ / 票 05–07）：本票**不开写口**
//! ——它们是值守的节奏参数，改它们是深思熟虑的编辑（config.toml 那一级有解析期校验），
//! 不是一次点击。设置页把它们摆出来，是为了让「值守怎么跑」这一页说得出全话。
//!
//! 语义（spec §二 裁决 3）：开关关掉的是「**跑**」——跑都不跑自然不吵；不是「跑着但不吵」
//! （那归离线通知的总开关与礼貌两件，决策 272 / 284）；不做班次级（没人会调的旋钮，
//! 决策 256 的尺子）。

use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::state::{map_core_error, ApiResult, AppState};

/// `GET /foreman-watch`：设置页的读数。
pub async fn settings(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let store = state.store.clone();
    let enabled = store.foreman_watch_enabled().await.map_err(map_core_error)?;
    let overridden = store
        .foreman_watch_has_override()
        .await
        .map_err(map_core_error)?;
    Ok(Json(json!({
        "enabled": enabled,
        // 诚实口径（决策 257）：这一份是谁定的。「default」= 从没碰过设置（缺省开）；
        // 「settings」= 界面保存过（哪怕值与缺省相同——保存过就是保存过）。
        "origin": if overridden { "settings" } else { "default" },
        // 节奏五个数只读展示（[pipeline] watch_*，config.toml 那一级）。
        "config": {
            "watch_event_window_minutes": state.settings.watch_event_window_minutes,
            "watch_owner_stuck_minutes": state.settings.watch_owner_stuck_minutes,
            "watch_debounce_sec": state.settings.watch_debounce_sec,
            "watch_task_cooldown_minutes": state.settings.watch_task_cooldown_minutes,
            "watch_max_wakes_per_hour": state.settings.watch_max_wakes_per_hour,
        },
    })))
}

#[derive(Debug, Deserialize)]
pub struct WatchSwitchBody {
    pub enabled: bool,
}

/// `PUT /foreman-watch`：保存开关。
///
/// 保存即活：值守循环每 10s 读一次库里的这一行，下一趟就按新值走——**不必重启**；
/// 在飞的那一轮不受影响、不被打断（它已经过了这道门）。
pub async fn set_enabled(
    State(state): State<AppState>,
    Json(body): Json<WatchSwitchBody>,
) -> ApiResult<Json<serde_json::Value>> {
    state
        .store
        .set_foreman_watch_enabled(body.enabled)
        .await
        .map_err(map_core_error)?;
    let overridden = state
        .store
        .foreman_watch_has_override()
        .await
        .map_err(map_core_error)?;
    Ok(Json(json!({
        "enabled": body.enabled,
        "origin": if overridden { "settings" } else { "default" },
    })))
}
