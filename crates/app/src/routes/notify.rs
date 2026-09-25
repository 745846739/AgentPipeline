//! 离线通知设置端点（决策 272⑥⑦⑧；284②③⑤ 添礼貌单元）。
//!
//! 六条路，各办一件事：
//! - `GET /notify/settings`：读数（生效单元 + 两个 `origin` 各说清是谁定的 + 秘密一律
//!   掩码 + 礼貌两件的**生效值** + 解析不了时的报文）。
//! - `PUT /notify/settings`：**总开关**（`{enabled}` 一个字段）——开启时先解析生效
//!   通道、BlueBubbles 先 ping（**够不着不当成功**），都过了才落库 + 重建出口。
//! - `PUT /notify/channel` / `DELETE /notify/channel`：通道单元的保存与交还
//!   （照 `/market/repos` 的先例）——保存是**整体覆盖**（272⑥ 不允许混）。
//! - `PUT /notify/politeness` / `DELETE /notify/politeness`：**礼貌单元**的保存与交还
//!   （284②：与通道单元**各自成立**，两级关系同构——界面整体覆盖 `config.toml`）。
//! - `POST /notify/test`：连通性探针（照 `POST /providers/test`，决策 160——
//!   对**未保存**的表单值发最小真实请求，成功失败都 200）。
//!
//! **秘密面**（272⑦）：`webhook_url` 与 `bluebubbles_password` 读回只给常量掩码 `***`
//! （provider `api_key` 同款，决策 112）；提交掩码或留空 = 不改；BlueBubbles 的完整
//! URL 由代码拼，本文件**不拼发送 URL**（那是 `notify.rs::NotifyTarget::send_url` 的
//! 事），探针走的也是 core 的 `ping_bluebubbles`。

use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use agentpipeline_core::notify::{
    ping_bluebubbles, resolve_notify_target, resolve_politeness, validate_politeness,
    NotifyChannelOverride, NotifyFormat, NotifyPoliteness, NotifySettingsState, NotifyTarget,
    WebhookNotifier,
};

use crate::state::{map_core_error, ApiError, ApiResult, AppState};

/// 读接口回显的掩码（`routes/providers.rs` 同一个常量值，决策 112）。
const SECRET_MASK: &str = "***";

/// 生效声明里的件（GET 展示视图）：秘密由调用方掩码，端点与收件人原样。
struct DeclaredChannel<'a> {
    channel: NotifyFormat,
    webhook_url: Option<&'a str>,
    bluebubbles_url: Option<&'a str>,
    bluebubbles_password: Option<&'a str>,
    bluebubbles_recipient: Option<&'a str>,
}

/// `GET /notify/settings`：设置页的读数。
///
/// 展示的是**生效的那一份**（单元 > config.toml，272⑥），`origin` 说清它是谁定的——
/// 用户改 `config.toml` 却发现「改了没用」时，答案必须在这一页上看得见（决策 194
/// 在仓名单上立的规矩）。解析出错不下发伪装的空态：`config_error` 带着原因走
/// （报错不静默，272⑧）。
pub async fn settings(State(state): State<AppState>) -> ApiResult<impl IntoResponse> {
    let stored = state
        .store
        .notify_settings_state()
        .await
        .map_err(map_core_error)?;
    // 展示用的解析**无视总开关**：关着的时候页面仍要能显示配到哪一级了，
    // 否则「关掉再打开」之间页面丢掉所有读数，用户无法核对。
    let display_state = NotifySettingsState {
        enabled: true,
        unit: stored.unit.clone(),
        politeness: stored.politeness,
    };
    let (effective, config_error) =
        match resolve_notify_target(&state.notify_config, &display_state) {
            Ok(target) => (target, None),
            Err(e) => (None, Some(e.to_string())),
        };
    let origin = if stored.unit.is_some() {
        "settings"
    } else {
        "config"
    };
    // 礼貌两件（284②）各自报来源：通道来自界面不代表礼貌也来自界面。
    let politeness = resolve_politeness(&state.notify_config, &display_state);
    let politeness_origin = if stored.politeness.is_some() {
        "settings"
    } else {
        "config"
    };
    // 生效声明里的件（掩码只给 webhook_url 与 password；端点与收件人不是秘密，
    // 原样回显——它们本来就是用户自己填的）。
    let declared = match (&stored.unit, &effective) {
        (Some(unit), _) => Some(DeclaredChannel {
            channel: unit.channel,
            webhook_url: unit.webhook_url.as_deref(),
            bluebubbles_url: unit.bluebubbles_url.as_deref(),
            bluebubbles_password: unit.bluebubbles_password.as_deref(),
            bluebubbles_recipient: unit.bluebubbles_recipient.as_deref(),
        }),
        (None, Some(NotifyTarget::Webhook { url, format })) => Some(DeclaredChannel {
            channel: *format,
            webhook_url: Some(url.as_str()),
            bluebubbles_url: None,
            bluebubbles_password: None,
            bluebubbles_recipient: None,
        }),
        (
            None,
            Some(NotifyTarget::BlueBubbles {
                endpoint,
                password,
                address,
            }),
        ) => Some(DeclaredChannel {
            channel: NotifyFormat::BlueBubbles,
            webhook_url: None,
            bluebubbles_url: Some(endpoint.as_str()),
            bluebubbles_password: Some(password.as_str()),
            bluebubbles_recipient: Some(address.as_str()),
        }),
        (None, None) => None,
    };
    let (channel, webhook_url, bb_url, bb_password, bb_recipient) = declared
        .map(|d| {
            (
                json!(d.channel.as_str()),
                json!(d.webhook_url.map(|_| SECRET_MASK).unwrap_or("")),
                json!(d.bluebubbles_url.unwrap_or("")),
                json!(d.bluebubbles_password.map(|_| SECRET_MASK).unwrap_or("")),
                json!(d.bluebubbles_recipient.unwrap_or("")),
            )
        })
        .unwrap_or((
            serde_json::Value::Null,
            json!(""),
            json!(""),
            json!(""),
            json!(""),
        ));
    let mut body = json!({
        "enabled": stored.enabled,
        "channel": channel,
        "origin": origin,
        "webhook_url": webhook_url,
        "bluebubbles_url": bb_url,
        "bluebubbles_password": bb_password,
        "bluebubbles_recipient": bb_recipient,
        // 礼貌两件（284②）：这里是**生效值**（单元 > config.toml），来源在
        // `politeness_origin` 里说清——界面能改它们（284，修订 272⑥ 的只读姿态）。
        "cooldown_sec": politeness.cooldown_sec,
        "quiet_hours": politeness.quiet_hours,
        "politeness_origin": politeness_origin,
    });
    if let Some(err) = config_error {
        body["config_error"] = json!(err);
    }
    Ok(Json(body))
}

#[derive(Debug, Deserialize)]
pub struct EnabledBody {
    pub enabled: bool,
}

/// `PUT /notify/settings`：**总开关**（决策 272⑧）。
///
/// 开启 = 一条显式动作：先解析生效通道（缺件 → 400 报错不静默），BlueBubbles 先
/// ping（**够不着不当成功**——失败时一个字节都不落库），都过了才落库 + 重建出口。
/// 关闭 = 直接落库 + 摘出口（`clear_notifier`，272⑧ 之前没有的那条反向路径）。
pub async fn set_enabled(
    State(state): State<AppState>,
    Json(body): Json<EnabledBody>,
) -> ApiResult<impl IntoResponse> {
    let stored = state
        .store
        .notify_settings_state()
        .await
        .map_err(map_core_error)?;
    let target_state = NotifySettingsState {
        enabled: body.enabled,
        unit: stored.unit.clone(),
        politeness: stored.politeness,
    };
    let target = resolve_notify_target(&state.notify_config, &target_state)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    if body.enabled {
        // 空配置的开启是**静默无操作**，按「缺必填项报错不静默」（272⑧）拒掉：
        // 没有任何通道声明（界面单元与 config.toml 两级都没有）时，开了也一个字节不出，
        // 与「开了却没配好」从外面分不开——让它显式失败，指去配置动作。
        if target.is_none() {
            return Err(ApiError::bad_request(
                "还没有任何通道声明：先在下方保存通道，或在 config.toml 的 [notify] 段配好。",
            ));
        }
        // 开启时的 ping（272⑧）：对**生效**通道发；失败 = 整条拒绝，什么都没改。
        ping_if_bluebubbles(&target).await?;
        state
            .store
            .set_notify_enabled(true)
            .await
            .map_err(map_core_error)?;
        apply_target(
            &state,
            target,
            resolve_politeness(&state.notify_config, &stored),
        );
    } else {
        state
            .store
            .set_notify_enabled(false)
            .await
            .map_err(map_core_error)?;
        state.store.clear_notifier();
    }
    Ok(Json(json!({ "ok": true, "enabled": body.enabled })))
}

#[derive(Debug, Deserialize)]
pub struct ChannelBody {
    pub channel: String,
    #[serde(default)]
    pub webhook_url: Option<String>,
    #[serde(default)]
    pub bluebubbles_url: Option<String>,
    #[serde(default)]
    pub bluebubbles_password: Option<String>,
    #[serde(default)]
    pub bluebubbles_recipient: Option<String>,
}

/// `PUT /notify/channel`：保存通道单元（**整体覆盖** config.toml，272⑥）。
///
/// 校验顺序：解析通道名 → 掩码合并（`***` / 留空 = 沿用已存值）→ 完整性校验
/// （缺件 400）→ 开着开关时先 ping（BlueBubbles）→ 落库 → 活生效。保存时开关
/// 是关的，单元照存（先配好、后开启是合法顺序），出口维持摘除。
pub async fn save_channel(
    State(state): State<AppState>,
    Json(body): Json<ChannelBody>,
) -> ApiResult<impl IntoResponse> {
    let channel = NotifyFormat::parse(body.channel.trim())
        .ok_or_else(|| ApiError::bad_request(format!("未知的通知通道：{}", body.channel)))?;
    let trim: fn(&Option<String>) -> Option<&str> =
        |v| v.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let stored = state
        .store
        .notify_settings_state()
        .await
        .map_err(map_core_error)?;
    // 掩码合并（决策 112 范式）：掩码或留空 = 不改——「已存值」只认**同一个通道**
    // 的那一份，从 feishu 改成 bluebubbles 时旧单元的 webhook_url 不能冒充新值。
    let stored_same_channel = stored.unit.as_ref().filter(|u| u.channel == channel);
    let merge = |incoming: Option<&str>, kept: Option<&str>| -> Option<String> {
        match incoming.map(str::trim) {
            Some("") | None | Some(SECRET_MASK) => kept.map(str::to_string),
            Some(v) => Some(v.to_string()),
        }
    };
    let unit = NotifyChannelOverride {
        channel,
        webhook_url: merge(
            trim(&body.webhook_url),
            stored_same_channel.and_then(|u| u.webhook_url.as_deref()),
        ),
        bluebubbles_url: merge(
            trim(&body.bluebubbles_url),
            stored_same_channel.and_then(|u| u.bluebubbles_url.as_deref()),
        ),
        bluebubbles_password: merge(
            trim(&body.bluebubbles_password),
            stored_same_channel.and_then(|u| u.bluebubbles_password.as_deref()),
        ),
        bluebubbles_recipient: merge(
            trim(&body.bluebubbles_recipient),
            stored_same_channel.and_then(|u| u.bluebubbles_recipient.as_deref()),
        ),
    };
    // 完整性：单元是显式动作，缺件报错不静默（272⑧）。
    let target = resolve_notify_target(
        &state.notify_config,
        &NotifySettingsState {
            enabled: true,
            unit: Some(unit.clone()),
            politeness: stored.politeness,
        },
    )
    .map_err(|e| ApiError::bad_request(e.to_string()))?
    .ok_or_else(|| ApiError::bad_request("通道单元不完整：缺必填项"))?;
    let enabled = stored.enabled;
    if enabled {
        // 保存即生效（开关开着）：BlueBubbles 先 ping，够不着不当成功。
        ping_if_bluebubbles(&Some(target.clone())).await?;
    }
    state
        .store
        .set_notify_channel(&unit)
        .await
        .map_err(map_core_error)?;
    if enabled {
        // 礼貌取**解析后的**那一份（284③）：保存通道不该把界面上的礼貌换回配置那一份。
        apply_target(
            &state,
            Some(target),
            resolve_politeness(&state.notify_config, &stored),
        );
    }
    Ok(Json(json!({ "ok": true })))
}

/// `DELETE /notify/channel`：交还 `config.toml` 那一级（照 `DELETE /market/repos`）。
/// 开关不动；开着时按配置级通道重建（配置级缺席 = 关死，268①）。
pub async fn clear_channel(State(state): State<AppState>) -> ApiResult<impl IntoResponse> {
    state
        .store
        .clear_notify_channel()
        .await
        .map_err(map_core_error)?;
    let stored = state
        .store
        .notify_settings_state()
        .await
        .map_err(map_core_error)?;
    let target = resolve_notify_target(&state.notify_config, &stored)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    apply_target(
        &state,
        target,
        resolve_politeness(&state.notify_config, &stored),
    );
    Ok(Json(json!({ "ok": true })))
}

#[derive(Debug, Deserialize)]
pub struct PolitenessBody {
    pub cooldown_sec: u64,
    /// `[开始, 结束)` 本地整点（0–23）；起止相同 = 全天不静默。
    pub quiet_hours: [u8; 2],
}

/// `PUT /notify/politeness`：保存**礼貌单元**（决策 284②③⑤，整体覆盖 `config.toml`）。
///
/// 顺序与通道那条同源：范围校验（越界 400，报错不静默）→ 解析当下的生效通道
/// （出口要按它重建；缺件仍是 400）→ 落库 → **活生效**。开关关着时落库照做
/// （先配好、后开启是合法顺序），出口维持摘除。
pub async fn save_politeness(
    State(state): State<AppState>,
    Json(body): Json<PolitenessBody>,
) -> ApiResult<impl IntoResponse> {
    let politeness = NotifyPoliteness {
        cooldown_sec: body.cooldown_sec,
        quiet_hours: body.quiet_hours,
    };
    validate_politeness(&politeness).map_err(ApiError::bad_request)?;
    let stored = state
        .store
        .notify_settings_state()
        .await
        .map_err(map_core_error)?;
    // 这次写不动通道，故目标按**当下**的落库状态解析（与落库后等价）。
    let target = resolve_notify_target(&state.notify_config, &stored)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    state
        .store
        .set_notify_politeness(&politeness)
        .await
        .map_err(map_core_error)?;
    apply_target(&state, target, politeness);
    Ok(Json(json!({ "ok": true })))
}

/// `DELETE /notify/politeness`：交还 `config.toml` 的 `[notify]` 那一份；开关与通道
/// 单元都不动（284⑦：三件事三个钮，各交各的）。
pub async fn clear_politeness(State(state): State<AppState>) -> ApiResult<impl IntoResponse> {
    state
        .store
        .clear_notify_politeness()
        .await
        .map_err(map_core_error)?;
    let stored = state
        .store
        .notify_settings_state()
        .await
        .map_err(map_core_error)?;
    let target = resolve_notify_target(&state.notify_config, &stored)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    apply_target(
        &state,
        target,
        resolve_politeness(&state.notify_config, &stored),
    );
    Ok(Json(json!({ "ok": true })))
}

#[derive(Debug, Deserialize)]
pub struct TestChannelBody {
    #[serde(default)]
    pub bluebubbles_url: Option<String>,
    #[serde(default)]
    pub bluebubbles_password: Option<String>,
}

/// `POST /notify/test`：BlueBubbles 连通性探针（照 `POST /providers/test`，决策 160）。
///
/// 对**未保存**的表单值发最小真实请求；成功失败都 200（它测的是配置，不是本端点）。
/// 掩码语义不弱化：`***` / 留空 = 沿用已存值（单元里没有就沿配置那一份；都没有 = 400）。
pub async fn test_channel(
    State(state): State<AppState>,
    Json(body): Json<TestChannelBody>,
) -> ApiResult<impl IntoResponse> {
    let trim: fn(&Option<String>) -> Option<&str> =
        |v| v.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let endpoint = trim(&body.bluebubbles_url)
        .ok_or_else(|| ApiError::bad_request("请先填写 BlueBubbles 端点地址"))?;
    let stored = state
        .store
        .notify_settings_state()
        .await
        .map_err(map_core_error)?;
    let kept = stored
        .unit
        .as_ref()
        .filter(|u| u.channel == NotifyFormat::BlueBubbles)
        .and_then(|u| u.bluebubbles_password.as_deref())
        .or(state.notify_config.bluebubbles_password.as_deref());
    let password = match trim(&body.bluebubbles_password) {
        Some("") | None | Some(SECRET_MASK) => {
            kept.ok_or_else(|| ApiError::bad_request("请先填写 password（还没有存过密钥）"))?
        }
        Some(p) => p,
    };
    let result = match ping_bluebubbles(endpoint, password).await {
        Ok(()) => json!({ "ok": true, "message": "BlueBubbles 服务可达。" }),
        Err(message) => json!({ "ok": false, "message": message }),
    };
    Ok(Json(json!({ "test": result })))
}

/// 开关开启 / 单元保存路径上的 ping 闸（272⑧：够不着不当成功）。
async fn ping_if_bluebubbles(target: &Option<NotifyTarget>) -> ApiResult<()> {
    if let Some(NotifyTarget::BlueBubbles {
        endpoint, password, ..
    }) = target
    {
        ping_bluebubbles(endpoint, password)
            .await
            .map_err(ApiError::bad_request)?;
    }
    Ok(())
}

/// 活生效（272⑧；284③ 扩到礼貌）：出口是启动时建一次的，任何一次落库都要**重建或
/// 摘除**它。礼貌取**解析后的**那一份（284②，单元 > config.toml）——调用方负责解析，
/// 这里只认值：出口自己不认识两级。
fn apply_target(state: &AppState, target: Option<NotifyTarget>, politeness: NotifyPoliteness) {
    match target {
        Some(target) => state.store.set_notifier(Arc::new(WebhookNotifier::new(
            target,
            politeness,
            state.store.clock().clone(),
        ))),
        None => state.store.clear_notifier(),
    }
}
