//! 离线通知设置端点（决策 272⑥⑦⑧；284②③⑤ 添礼貌单元；pwa-webpush 02 添第五通道）。
//!
//! 六条路，各办一件事：
//! - `GET /notify/settings`：读数（生效单元 + 两个 `origin` 各说清是谁定的 + 秘密一律
//!   掩码 + 礼貌两件的**生效值** + VAPID 公钥原样 / 私钥掩码 + 解析不了时的报文）。
//! - `PUT /notify/settings`：**总开关**（`{enabled}` 一个字段）——开启时先解析生效
//!   通道、BlueBubbles 先 ping（**够不着不当成功**），都过了才落库 + 重建出口。
//! - `PUT /notify/channel` / `DELETE /notify/channel`：通道单元的保存与交还
//!   （照 `/market/repos` 的先例）——保存是**整体覆盖**（272⑥ 不允许混）；
//!   存 `webpush` 时顺带把 VAPID 密钥对生成出来（pwa-webpush 02，零手工配置）。
//! - `PUT /notify/politeness` / `DELETE /notify/politeness`：**礼貌单元**的保存与交还
//!   （284②：与通道单元**各自成立**，两级关系同构——界面整体覆盖 `config.toml`）。
//! - `POST /notify/test`：连通性探针（照 `POST /providers/test`，决策 160——
//!   对**未保存**的表单值发最小真实请求，成功失败都 200）。
//! - `GET/POST/DELETE /notify/push/subscriptions[/{id}]`：浏览器推送的订阅清单
//!   （pwa-webpush 02）——**读也过配对令牌守卫**（`stream::pairing_guard` 的前缀白名单），
//!   因为清单里的每一条都是「往那台设备推任意报文」的能力的一半（见 `storage::push`）。
//!
//! **秘密面**（272⑦）：`webhook_url` 与 `bluebubbles_password` 读回只给常量掩码 `***`
//! （provider `api_key` 同款，决策 112）；提交掩码或留空 = 不改；BlueBubbles 的完整
//! URL 由代码拼，本文件**不拼发送 URL**（那是 `notify.rs::NotifyTarget::delivery` 的
//! 事），探针走的也是 core 的 `ping_bluebubbles`。VAPID 私钥同一档：**永不回显原值**。

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use agentpipeline_core::notify::{
    ping_bluebubbles, resolve_notify_target, resolve_politeness, validate_politeness,
    NotifyChannelOverride, NotifyFormat, NotifyPoliteness, NotifySettingsState, NotifyTarget,
    WebhookNotifier,
};
use agentpipeline_core::webpush::validate_subscription;

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
    // VAPID 密钥对（pwa-webpush 02）：读接口**不生成**（GET 不该写库），只如实报
    // 「有 / 没有」——没有时读数是两个空串，设置页据此提示「保存一次通道」。
    let vapid = state
        .store
        .push_vapid_keys()
        .await
        .map_err(map_core_error)?;
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
        // 浏览器推送：**没有件可回显**（订阅与密钥对都在库里，不是这一格的字段）——
        // 通道名照给，四件全空（设置页据此只显示「浏览器推送」那一个名字）。
        (None, Some(NotifyTarget::WebPush)) => Some(DeclaredChannel {
            channel: NotifyFormat::WebPush,
            webhook_url: None,
            bluebubbles_url: None,
            bluebubbles_password: None,
            bluebubbles_recipient: None,
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
        // VAPID 两件（pwa-webpush 02）：**公钥原样**（浏览器订阅时就要拿它当
        // applicationServerKey，它不是秘密）、**私钥只给掩码**（对齐 provider api_key，
        // 决策 112）。没生成过时两个都是空串。
        "vapid_public_key": vapid.as_ref().map(|k| k.public_key.as_str()).unwrap_or(""),
        "vapid_private_key": vapid.as_ref().map(|_| SECRET_MASK).unwrap_or(""),
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
    // 浏览器推送：**首次启用自动生成** VAPID 密钥对（pwa-webpush 02，票面「零手工配置」）。
    // 生成放在落库**之后**（先接受这次保存，再补密钥）——两者都在同一次动作里，
    // 而生成失败是真错误（系统随机源不可用），故它照 272⑧ 报出来而不是静默吞掉。
    // 订阅上报那边也兜了一次 `ensure`：手改过库 / 配置级声明的 webpush 都走得到。
    state
        .store
        .set_notify_channel(&unit)
        .await
        .map_err(map_core_error)?;
    if channel == NotifyFormat::WebPush {
        state
            .store
            .ensure_push_vapid_keys()
            .await
            .map_err(map_core_error)?;
    }
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

// ─────────────── 浏览器推送的订阅（spec `.scratch/pwa-webpush/` 票 02）───────────────
//
// 三条路 + 一条单删。**读也过配对令牌守卫**（`stream::pairing_guard` 认
// `PUSH_SUBSCRIPTIONS_PREFIX`）：清单里的每一条都是「往那台设备推任意报文」的能力的
// 一半，而订阅是**持续的**外泄管道（读接口是一次性的偷看，写接口是永久的）——这是对
// 决策 167「v1 无鉴权」的定点加强，回环豁免与报文形状照 182⑦。
//
// 读接口只给**摘要**（`endpoint_hint`），不给完整 endpoint、更不给 `p256dh` / `auth`：
// 清单要能回答「这是哪台设备」，不需要交出「怎么推它」（`storage::push` 的头注）。

/// `POST /notify/push/subscriptions` 的载荷：**浏览器 `PushSubscription.toJSON()` 的形状**
/// 原样收（`{endpoint, keys:{p256dh, auth}}`）——前端不做转换，少一层就少一处漂移。
#[derive(Debug, Deserialize)]
pub struct SubscribeBody {
    pub endpoint: String,
    pub keys: SubscriptionKeys,
}

#[derive(Debug, Deserialize)]
pub struct SubscriptionKeys {
    pub p256dh: String,
    pub auth: String,
}

/// `POST /notify/push/subscriptions`：按 `endpoint` upsert 一条订阅（同设备两次订阅一行）。
///
/// 校验在落库前（形状不对 400 报错不静默，272⑧）：坏行静静躺在清单里、直到某次通知
/// 才发现它永远发不出去，是这一族里最难查的一种「配置错」。
pub async fn subscribe(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SubscribeBody>,
) -> ApiResult<impl IntoResponse> {
    let endpoint = body.endpoint.trim();
    let p256dh = body.keys.p256dh.trim();
    let auth = body.keys.auth.trim();
    validate_subscription(endpoint, p256dh, auth).map_err(ApiError::bad_request)?;
    // 兜一次密钥生成（幂等）：通道来自 `config.toml`、或有人手改过库时，走到这里的
    // 这一刻库里可能还没有那一对——而没有它这条订阅**永远不会被推到**，而用户看到的
    // 是「订阅成功了」。宁可在上报时补上。
    state
        .store
        .ensure_push_vapid_keys()
        .await
        .map_err(map_core_error)?;
    let user_agent = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let id = state
        .store
        .upsert_push_subscription(endpoint, p256dh, auth, user_agent.as_deref())
        .await
        .map_err(map_core_error)?;
    Ok(Json(json!({ "ok": true, "id": id })))
}

/// `GET /notify/push/subscriptions`：设备清单（订阅时间升序）。
pub async fn list_subscriptions(State(state): State<AppState>) -> ApiResult<impl IntoResponse> {
    let subscriptions = state
        .store
        .list_push_subscriptions()
        .await
        .map_err(map_core_error)?;
    let items: Vec<serde_json::Value> = subscriptions
        .iter()
        .map(|sub| {
            json!({
                "id": sub.id,
                // 摘要而非完整 endpoint（掩码先例：清单是给人认设备的，不是能力包）。
                "endpoint_hint": sub.endpoint_hint(),
                "user_agent": sub.user_agent.clone().unwrap_or_default(),
                "created_at": sub.created_at.to_rfc3339(),
            })
        })
        .collect();
    Ok(Json(json!({ "subscriptions": items })))
}

/// `DELETE /notify/push/subscriptions/{id}`：撤销一台设备（单个撤销）。
pub async fn delete_subscription(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<impl IntoResponse> {
    let removed = state
        .store
        .delete_push_subscription(id)
        .await
        .map_err(map_core_error)?;
    Ok(Json(json!({ "ok": true, "removed": removed })))
}

/// `DELETE /notify/push/subscriptions`：一键清空（换手机 / 怀疑被订阅过时的收回动作）。
pub async fn clear_subscriptions(State(state): State<AppState>) -> ApiResult<impl IntoResponse> {
    let removed = state
        .store
        .clear_push_subscriptions()
        .await
        .map_err(map_core_error)?;
    Ok(Json(json!({ "ok": true, "removed": removed })))
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
            // 浏览器推送要读库（订阅行 + VAPID 密钥对）：出口持一个 Store 句柄克隆。
            state.store.clone(),
        ))),
        None => state.store.clear_notifier(),
    }
}
