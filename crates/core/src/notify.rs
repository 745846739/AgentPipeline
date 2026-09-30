//! 离线通知（决策 268，票 foreman-within-boundary 02）：attention 落库且 `wakes()`
//! 时向配置的 webhook POST 一条**通用 JSON**——与前端 toast（决策 130③，仍归前端管
//! 它自己的 cooldown / quiet_hours）并存的、**出机器**的那条线。值守轮的终点是叫醒人，
//! 没人盯浏览器的夜里，叫醒必须出得了这台机器。
//!
//! 决策 272 扩了两件事：
//!
//! 1. **通道**：`format = bluebubbles` 时投给本机 BlueBubbles 服务的
//!    `POST /api/v1/message/text`（iMessage）——完整 URL 是**代码拼**的
//!    （端点 + password 两件组成），除 `without_url()` 外不许进任何日志（272⑦）；
//!    正文字段名是 `message`（官方文档页写 `text` 是**错的**，以服务端源码
//!    `messageRouter.ts:237-241` 为准），`tempGuid` 用 Ulid 现生成——它是服务端发送
//!    队列的去重键，硬编码会让第二条起全灭。
//! 2. **第二触发面**：值班长回话完成（`notify_foreman_reply` / `notify_foreman_failure`）
//!    ——**不**进 attention 表（那张表是「待办」，`task_id NOT NULL` + 外键、由值守轮
//!    消费 `consumed_at`；回话是播报，写进去会污染唤醒判据），由 `foreman.rs` 在
//!    `respond()` 收口之后直接调用。入口有两个，**礼貌只有一个**：回话线的新类
//!    `foreman_reply` 有自己的 cooldown 槽、受免打扰（不豁免）；失败收口走 `failed`
//!    （恒发）。
//!
//! 两份实现、**一张表**：礼貌语义镜像前端 `lib/notificationPolicy.ts`——`failed` 恒发
//! （免打扰与节流都不拦）、免打扰 `[22, 8)` 跨零点含头不含尾按整点、`pending` 免打扰
//! 豁免（节流照走）、每类 cooldown 严格小于——由 `tests/fixtures/notification_policy.json`
//! 把 Rust 表测试与前端 fixture 测试钉在同一份、同一断言方向（决策 246 回环表的先例）。
//!
//! 决策 284 把「同一张表」的**适用范围**写窄了：那张表钉的是**语义**（怎么算静音、
//! 哪些类豁免），**数值**由这一侧的 `NotifyPoliteness` 说了算——它可以来自
//! `config.toml` 的 `[notify]`，也可以来自设置页保存的礼貌单元（`resolve_politeness`，
//! 272⑥ 的「只住 config.toml」由此**显式修订**）。前端那份表只管**浏览器 toast**，
//! 出口这份只管**出机器那条线**：一个语义一处可改。
//!
//! **显式差异**（决策 268 允许「写清差异」而非强求同一实现）：前端还有 `notifyOn`
//! 每类开关（用户偏好面，缺省 `cancelled: false` = 永不弹）；后端没有偏好面——
//! `cancelled` 按通用类规则走 cooldown + 免打扰。**决策 272 又添一条反方向的**：
//! `foreman_reply` 是后端独有的类（前端没有回话完成的 SSE 事件，toast 面根本见不到它），
//! 同样不进共享表。两条都记在 fixture 的 `$comment` 与两侧守卫里。共享表因此只收
//! 两边共有的语义子集（`pending` / `done` / `failed` × 免打扰 × 节流）。
//!
//! **第五支：浏览器推送（spec `.scratch/pwa-webpush/` 票 02）**。它是**第四个互斥通道**
//! （与 generic / feishu / bluebubbles 四选一，沿 272 的单行三选一形状；「多出口并存」
//! 仍归 272 踢出的那一票），触发面与礼貌门**零新增**——同一条 attention 漏斗、同三处
//! 值班长直调、同一道 `dispatch()`。三处与既有通道不同：
//!
//! 1. **通道没有寻址件**：订阅行（每设备一行）与 VAPID 密钥对都在库里
//!    （`storage::push`，0037），出口按 `Store` 现场读——故 `NotifyTarget::WebPush`
//!    是不带字段的一个值，解析层不需要任何必填件。
//! 2. **一条通知扇出到 N 条 HTTP**（N = 活订阅数），每条各做一次 RFC 8291 加密
//!    （`crate::webpush`）；推送服务回 404 / 410 = 那台设备的订阅死了，**删行**。
//! 3. **报文多一个 `url`**（push 独有，其余三种格式一字不改）：深链由服务端拼好，
//!    service worker 只消费。规则见 [`attention_deep_link`] / [`talk_deep_link`]。
//!
//! 触发事件集与 iMessage 通道**完全一致**（票面 29：将来同批增减）——`SlowRun` 照旧
//! 一个字节不出站。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Local, Timelike, Utc};
use serde::{Deserialize, Serialize};

use crate::clock::Clock;
use crate::storage::attention::AttentionKind;
use crate::storage::Store;
use crate::{Error, Result};

/// 报文格式（决策 270，272① 扩到第四支、pwa-webpush 02 扩到第五支）：同一份通用事件、
/// 按目标选序列化——政策语义（cooldown / 免打扰 / `wakes()` 触发面）与格式无关，
/// 只有最后拼 payload 分流。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotifyFormat {
    /// 268④ 的六字段通用 JSON（缺省）。
    #[default]
    Generic,
    /// 飞书机器人文本消息（`{"msg_type":"text","content":{"text":...}}`）；
    /// 安全设置用自定义关键词 `AgentPipeline`——`title` 固定前缀命中，不做签名。
    Feishu,
    /// BlueBubbles → iMessage（决策 272①）：`{"chatGuid","tempGuid","message","method"}`
    /// POST 到 `{端点}/api/v1/message/text?password=`。`method` 恒 `apple-script`
    /// 且**不暴露配置**（`private-api` 要另装私有 API helper，属部署面额外要求）。
    BlueBubbles,
    /// 浏览器推送（spec `.scratch/pwa-webpush/` 票 02）：标准 Web Push——每条**活订阅**
    /// 各发一条 RFC 8291 加密的 `{title, body, url}`（`url` 是这一支独有的字段）。
    /// 它没有单一寻址件（订阅在库里、密钥对也在库里），见 [`NotifyTarget::WebPush`]。
    WebPush,
}

impl NotifyFormat {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "generic" => Some(NotifyFormat::Generic),
            "feishu" => Some(NotifyFormat::Feishu),
            "bluebubbles" => Some(NotifyFormat::BlueBubbles),
            "webpush" => Some(NotifyFormat::WebPush),
            _ => None,
        }
    }

    /// 落库 / 展示用的串形（与 serde 的小写形一致）。
    pub fn as_str(self) -> &'static str {
        match self {
            NotifyFormat::Generic => "generic",
            NotifyFormat::Feishu => "feishu",
            NotifyFormat::BlueBubbles => "bluebubbles",
            NotifyFormat::WebPush => "webpush",
        }
    }
}

/// 通知分类——与前端 `notificationPolicy.ts::NotificationClass` 同名同义。
///
/// `ForemanReply` 是**后端独有**的一类（决策 272③④）：值班长回话完成那条线的类，
/// 有**自己的 cooldown 槽**（绝不复用 `done`——复用会让回话吃掉 `done` 的配额、把真的
/// `task_done` 静默掉）；受免打扰、**不豁免**（叫醒人的是 attention 那条线，回话是
/// 摘要不是警报）。它不进跨语言共享 fixture——`$comment` 与两侧守卫显式记为差异
/// （照 `cancelled` 先例）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotifyClass {
    Pending,
    Done,
    Failed,
    Cancelled,
    ForemanReply,
}

impl NotifyClass {
    pub fn as_str(self) -> &'static str {
        match self {
            NotifyClass::Pending => "pending",
            NotifyClass::Done => "done",
            NotifyClass::Failed => "failed",
            NotifyClass::Cancelled => "cancelled",
            NotifyClass::ForemanReply => "foreman_reply",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(NotifyClass::Pending),
            "done" => Some(NotifyClass::Done),
            "failed" => Some(NotifyClass::Failed),
            "cancelled" => Some(NotifyClass::Cancelled),
            "foreman_reply" => Some(NotifyClass::ForemanReply),
            _ => None,
        }
    }
}

/// attention kind → 通知分类。
///
/// 这张表是**后端独有**的一张小表（前端映射的是 SSE 事件，两边的「类」同名同义、进表的
/// 成员各自钉）：`SlowRun` 不给类——它是 `wakes()` 唯一为 false 的那个，双重挡死；
/// 卡住等人的七个（pending / repeated / owner / scheduler / stale / **resume blocked** /
/// **blocked read**）归 `pending`——免打扰豁免正是为「等人处理」那一类设的
/// （`resume_blocked` 是「系统试过、放弃了，该有人接手」，`blocked_read` 是「这台机器上
/// 有读挂在系统调用里，得有人去看授权」——两件都是要人动手的）；失败族四个归 `failed`（恒发）。
/// 由 `tests/integration/notify.rs::kind_to_class_mapping_is_pinned` 逐个钉住。
pub fn notification_class(kind: AttentionKind) -> Option<NotifyClass> {
    use AttentionKind::*;
    Some(match kind {
        SlowRun => return None,
        TaskPending | RepeatedPending | OwnerStuck | SchedulerNoEffect | TaskStale
        | ResumeBlocked | BlockedRead => NotifyClass::Pending,
        RetryExhausted | ContextOverflow | GateFailure | RunFailed => NotifyClass::Failed,
        TaskDone => NotifyClass::Done,
        TaskCancelled => NotifyClass::Cancelled,
    })
}

/// 免打扰判定（镜像前端 `isQuietHours`）：`start == end` 恒不静音；跨零点
/// （`start > end`）`hour >= start || hour < end`；含头不含尾、按整点。
pub fn is_quiet_hours(hour: u32, quiet: [u8; 2]) -> bool {
    let (start, end) = (quiet[0] as u32, quiet[1] as u32);
    if start == end {
        return false;
    }
    if start < end {
        hour >= start && hour < end
    } else {
        hour >= start || hour < end
    }
}

/// 礼貌门（镜像前端 `shouldNotify` 的语义子集——`notifyOn` 开关不在后端，见头注）。
///
/// 判定顺序与前端逐行同构：`failed` 恒发（免打扰与节流都不拦，`ALWAYS_ANNOUNCED`
/// 先例）→ 免打扰静音（`pending` 豁免）→ **每类** cooldown（严格小于：age == cooldown
/// 放行、age < cooldown 拦下）。`last_age_sec = None` = 这一类还没发过。
/// `ForemanReply` 不在任何豁免列里——它走标准路径（受免打扰、受 cooldown，272④）。
pub fn should_notify(
    cls: NotifyClass,
    hour: u32,
    last_age_sec: Option<u64>,
    cooldown_sec: u64,
    quiet: [u8; 2],
) -> bool {
    if cls == NotifyClass::Failed {
        return true;
    }
    if is_quiet_hours(hour, quiet) && cls != NotifyClass::Pending {
        return false;
    }
    !matches!(last_age_sec, Some(age) if age < cooldown_sec)
}

/// webhook 投递超时：通知是 best-effort，挂住不能拖着 `note_attention` 的调用方
/// （它早已返回，投递在后台），但挂住的连接也该有个头。
const WEBHOOK_TIMEOUT: Duration = Duration::from_secs(10);

/// 归因文本（分流前**共用**，决策 270②）：只带白名单短标识键，`detail` 里的
/// `output` / `diagnostic` / `error` / `message` 原文一个都不出网（268「不发正文/
/// 日志原文」的纪律不因格式松动）。**body 不再复读 kind 与 task_id**（决策 344）：
/// title 已带中文标签与 id，正文从归因属性开始；一个属性都挑不到时就是空串
/// （锁屏上只显示 title，现场都在深链里）。
fn attribution_body(detail: Option<&serde_json::Value>) -> String {
    const ATTRIBUTION_KEYS: [&str; 7] = [
        "stage",
        "node",
        "attempt",
        "pending_kind",
        "gate_failure_kind",
        "run_id",
        "status",
    ];
    let mut attrs: Vec<String> = Vec::new();
    if let Some(d) = detail {
        for k in ATTRIBUTION_KEYS {
            match d.get(k) {
                Some(serde_json::Value::String(s)) => attrs.push(format!("{k}={s}")),
                Some(serde_json::Value::Number(n)) => attrs.push(format!("{k}={n}")),
                _ => {}
            }
        }
    }
    attrs.join(" ")
}

/// 按字符数截断（按 `char` 切，不打断多字节字符），截断时带省略号。
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    format!("{cut}…")
}

/// query 参数的最小百分号编码（零新依赖）：只放行 RFC 3986 的 unreserved 集。
/// BlueBubbles 的 password 实际是服务端生成的 GUID，但拼 URL 的代码不该假设这一点——
/// 用户手填的 password 里出现 `&` / `=` / `%` 时，不编码会把 query 拼坏。
fn encode_query_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 一条通知的**内容**（分流前的最后一层）：`title` 已带 `[AgentPipeline]` 前缀，
/// `body` 在各自入口处就定型——attention 线只带归因白名单（268④；344 起正文不再
/// 复读 kind / task_id），回话线带回话正文（272⑤ 对本人通道的显式豁免，截断
/// 200 字），失败线只带类别不带原文。
///
/// `url`（pwa-webpush 02）是**浏览器推送独有的字段**：深链由服务端拼好（前端那几条
/// hash 路由的形状见 [`attention_deep_link`] / [`talk_deep_link`]），service worker
/// 只消费。其余三种格式一个字都不用它——于是「深链规则」这一件只在有 service worker
/// 的那条线上存在。
struct Notice<'a> {
    kind: &'a str,
    task_id: Option<&'a str>,
    occurred_at: DateTime<Utc>,
    title: String,
    body: String,
    url: String,
}

// 看板首页 `#/` 这个兜底落点**只在前端存在**（`lib/pushPayload.ts` 的 `BOARD_HOME_HASH`：
// payload 里的 `url` 不可用时的降级）。服务端这一侧不发 `#/`：每一条出站通知都挂在一个具体
// 的任务或班次上，拼不出落点的情形不存在（票面「不可达时降级到看板首页而不是白屏」）。
// 原先这里有一个 `BOARD_HOME` 常量，没有任何消费者，落地时删掉（决策 323 的记录口径：
// 不留只为「以后可能用」的钩子）。

/// attention 线的深链落点（票面「深链规则」的后半句）。
///
/// 两类事件两种落点，判据是 detail 里有没有 `run_id`：
/// - **卡片级**（待拍板 / 完成 / 取消 / 停滞 / 等人处理这一族：事件挂在任务上）→
///   任务详情 `#/task/<task_id>`，也就是那张卡的现场。
/// - **流水线级**（失败族：`run_failed` / `retry_exhausted` / `context_overflow`，以及
///   带 `run_id` 的卡住类）→ 同一页再带上 `?run=<run_id>`：任务详情里的**对话页签**
///   按它定位到那一次运行（`?run=` 是决策 326 定的查询串契约，消费者见 `TaskDetail.svelte`）。
///   落点仍是那张卡（用户要的是「哪台机器停了」，而卡是它的身份），多的那一段是
///   「停在哪一次运行」。
pub fn attention_deep_link(task_id: &str, detail: Option<&serde_json::Value>) -> String {
    match detail
        .and_then(|d| d.get("run_id"))
        .and_then(|v| v.as_i64())
    {
        Some(run_id) => format!("#/task/{task_id}?run={run_id}"),
        None => format!("#/task/{task_id}"),
    }
}

/// 值班长那条线的深链落点：对讲台的**那一班**（`?session=` 是决策 217① 起的既有参数，
/// 地址里那一班才是权威）。
pub fn talk_deep_link(session_id: &str) -> String {
    format!("#/talk?session={session_id}")
}

/// **唯一分流点**（决策 270②；272① 扩到 BlueBubbles、pwa-webpush 02 扩到 Web Push）。
/// 归因白名单与「detail 原文不出网」在分流前共用——一个字节的纪律不因格式松动。
///
/// `bb_address` 只在 `format = BlueBubbles` 时被读（拼 `chatGuid`）。
fn render(format: NotifyFormat, notice: &Notice, bb_address: Option<&str>) -> serde_json::Value {
    match format {
        NotifyFormat::Generic => {
            let mut body = serde_json::json!({
                "source": "agentpipeline",
                "kind": notice.kind,
                "occurred_at": notice.occurred_at.to_rfc3339(),
                "title": notice.title,
                "body": notice.body,
            });
            // 回话线没有 task_id（attention 表的语义），**省略字段**而不是填哨兵
            // （272②：哨兵会被 feishu 的关键词前缀与 iMessage 的正文一起露在人眼前）。
            if let Some(task_id) = notice.task_id {
                body["task_id"] = serde_json::json!(task_id);
            }
            body
        }
        NotifyFormat::Feishu => serde_json::json!({
            "msg_type": "text",
            "content": { "text": format!("{}\n{}", notice.title, notice.body) },
        }),
        NotifyFormat::BlueBubbles => {
            let address = bb_address.expect("BlueBubbles 分支必须有收件人地址");
            serde_json::json!({
                "chatGuid": format!("iMessage;-;{address}"),
                // 服务端发送队列的**去重键**（messageValidator.ts:113-116 重复即 400）
                // ——必须每次唯一，否则从第二条起全灭。
                "tempGuid": ulid::Ulid::new().to_string(),
                // 官方文档页写 `text` 是错的，以服务端源码为准（272①）。
                "message": format!("{}\n{}", notice.title, notice.body),
                "method": "apple-script",
            })
        }
        // 浏览器推送的报文体（票面点名三件：title / body / url）——这份 JSON 就是
        // RFC 8291 要加密的明文，浏览器侧 `push` 事件里 `event.data.json()` 读它。
        NotifyFormat::WebPush => serde_json::json!({
            "title": notice.title,
            "body": notice.body,
            "url": notice.url,
        }),
    }
}

/// attention 线的报文（268④ 六字段契约 / 270② feishu / 272① bluebubbles /
/// pwa-webpush 02 webpush）。落点由 [`attention_deep_link`] 按 detail 里的 `run_id` 定。
pub fn payload_for(
    format: NotifyFormat,
    kind: AttentionKind,
    task_id: &str,
    occurred_at: DateTime<Utc>,
    detail: Option<&serde_json::Value>,
    bb_address: Option<&str>,
) -> serde_json::Value {
    let notice = Notice {
        kind: kind.as_str(),
        task_id: Some(task_id),
        occurred_at,
        // 决策 344：标签前置（锁屏截断先掉 id 不掉词），机器词 kind 原样留在
        // generic 报文的 `kind` 字段里；id 在深链里，一个字节不丢。
        title: format!("[AgentPipeline] {} {task_id}", kind.label()),
        body: attribution_body(detail),
        url: attention_deep_link(task_id, detail),
    };
    render(format, &notice, bb_address)
}

/// 回话线的正文上限（决策 272⑤「截断，200 字量级」）。截断的目的地是**本人手机**，
/// 不是存档——完整正文永远在对讲台的台账里。
const FOREMAN_REPLY_BODY_CHARS: usize = 200;

/// 值班长**回话完成**的报文（决策 272②⑤）：正文 = 回话原文（截断）+ 会话名在
/// `title` 里——多会话时否则不知是哪一轮。这是对 268④ 的一次**显式修订**（本人通道）：
/// 出机器、不出账户；豁免只覆盖回话正文，不覆盖日志/命令输出/`detail` 原文。
///
/// `session_id`（pwa-webpush 02 添）只服务深链：点开推送落到**这一班**的对话，
/// 而不是对讲台的第一班。
pub fn foreman_reply_payload_for(
    format: NotifyFormat,
    session_id: &str,
    session_name: &str,
    reply: &str,
    occurred_at: DateTime<Utc>,
    bb_address: Option<&str>,
) -> serde_json::Value {
    let notice = Notice {
        kind: "foreman_reply",
        task_id: None,
        occurred_at,
        title: format!("[AgentPipeline] {session_name} 回话"),
        body: truncate_chars(reply.trim(), FOREMAN_REPLY_BODY_CHARS),
        url: talk_deep_link(session_id),
    };
    render(format, &notice, bb_address)
}

/// 值班长**失败收口**的报文（决策 272③）：正文只带**类别**（`turn_failure_reason`
/// 的 kind 那一层）——不带 `raw` 原文段、不带 detail 原文（268④ 的纪律**不**豁免
/// 这一条线；类别已足够把「续费」与「查网络」分开，现场在对讲台里）。
pub fn foreman_failure_payload_for(
    format: NotifyFormat,
    session_id: &str,
    session_name: &str,
    kind: &str,
    occurred_at: DateTime<Utc>,
    bb_address: Option<&str>,
) -> serde_json::Value {
    let notice = Notice {
        kind: "foreman_reply_failed",
        task_id: None,
        occurred_at,
        title: format!("[AgentPipeline] {session_name} 回话失败"),
        body: format!("这一轮没跑起来（{kind}）——详情见对讲台的失败账。"),
        url: talk_deep_link(session_id),
    };
    render(format, &notice, bb_address)
}

/// 投递目标（决策 272⑥）：通道的寻址面。完整 URL 的拼法**只有这里认识**——
/// 组合出的 URL 是秘密（password 在 query 里），除 `without_url()` 外不许进任何日志。
#[derive(Debug, Clone, PartialEq)]
pub enum NotifyTarget {
    /// 268 的 webhook：URL 原样使用（generic / feishu 报文）。URL 含 token 即秘密
    /// （268①：只进 config.toml，不入台账、不入日志明文）。
    Webhook { url: String, format: NotifyFormat },
    /// BlueBubbles（272①）：端点 + password + 收件人三件。`format` 恒为
    /// [`NotifyFormat::BlueBubbles`]——通道类型就是报文格式的选择，不设第二个旋钮。
    BlueBubbles {
        endpoint: String,
        password: String,
        address: String,
    },
    /// 浏览器推送（pwa-webpush 02）：**没有寻址件**——每条订阅的 endpoint 在
    /// `kanban_push_subscription` 里（每设备一行），VAPID 密钥对在 `kanban_notify_channel`
    /// 那行（首次启用自动生成）。两者都由出口按 `Store` 现场读，故这里是一个不带字段的值
    /// （「通道选它就够」——与 272⑥ 的整体覆盖不冲突：没有任何东西需要用户填）。
    WebPush,
}

impl NotifyTarget {
    /// 这条通道的报文格式。
    fn format(&self) -> NotifyFormat {
        match self {
            NotifyTarget::Webhook { format, .. } => *format,
            NotifyTarget::BlueBubbles { .. } => NotifyFormat::BlueBubbles,
            NotifyTarget::WebPush => NotifyFormat::WebPush,
        }
    }

    /// BlueBubbles 的收件人地址（拼 `chatGuid` 用）；其余通道 `None`。
    fn bb_address(&self) -> Option<&str> {
        match self {
            NotifyTarget::BlueBubbles { address, .. } => Some(address),
            NotifyTarget::Webhook { .. } | NotifyTarget::WebPush => None,
        }
    }

    /// 这份目标怎么投（pwa-webpush 02 添第二支）：一条 POST 到固定地址，还是扇出到
    /// 所有活订阅。**分流与地址拼装都在这一处**——早先是「`delivery()` 判一次通道、
    /// `send_url()` 再对同一个枚举判一次」，于是 Web Push 那一支只能给一个 `unreachable!()`
    /// 的臂；一个运行期会 panic 的形状正是 `Delivery` 想消灭的东西（决策 323）。
    ///
    /// 地址**只在 HTTP 那一支现拼**，且拼出来的字符串不落任何名字带 `log` / `debug` 的
    /// 地方（272⑦：URL 里带 password / token）。
    fn delivery(&self) -> Delivery {
        match self {
            NotifyTarget::Webhook { url, .. } => Delivery::Http(url.clone()),
            NotifyTarget::BlueBubbles {
                endpoint, password, ..
            } => Delivery::Http(format!(
                "{}/api/v1/message/text?password={}",
                endpoint.trim_end_matches('/'),
                encode_query_component(password)
            )),
            // 浏览器推送没有**单一**地址：它扇出到每一条订阅的 endpoint，那些地址在库里
            // （`storage::push`），且是逐条从 DB 读的。
            NotifyTarget::WebPush => Delivery::Push,
        }
    }
}

/// 界面保存的通道单元（决策 272⑥）：**作为一个整体**覆盖 `config.toml`——要么四件
/// 全来自界面、要么全来自配置（不允许混，这正是「界面指向 BlueBubbles 而配置说
/// feishu」不会发生的原因）。password 是明文（决策 112 的存放面：DB 目录 0700 /
/// 文件 0600，`data/` 整目录对 agent 工具关闭）。
#[derive(Debug, Clone, PartialEq)]
pub struct NotifyChannelOverride {
    pub channel: NotifyFormat,
    pub webhook_url: Option<String>,
    pub bluebubbles_url: Option<String>,
    pub bluebubbles_password: Option<String>,
    pub bluebubbles_recipient: Option<String>,
}

/// **礼貌两件**（决策 284，显式修订 272⑥）：节流秒数 + 免打扰起止。
///
/// 它们服务的是**出机器那条线**（webhook / 飞书 / iMessage）——浏览器 toast 有自己
/// 一份固定表（`frontend/src/lib/notificationPolicy.ts`，268③ 的跨语言镜像），
/// 这张设置页不动它。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotifyPoliteness {
    /// 每类通知的合并窗口（秒）；`0` = 不节流（免打扰照走）。
    pub cooldown_sec: u64,
    /// 免打扰 `[开始, 结束)`——本地整点、含头不含尾、跨零点合法、起止相同 = 全天不静默。
    pub quiet_hours: [u8; 2],
}

impl Default for NotifyPoliteness {
    /// 与 `[notify]` 的缺省同源（一处定义，`config.rs` 是那一处）。
    fn default() -> Self {
        let defaults = crate::config::NotifyConfig::default();
        Self {
            cooldown_sec: defaults.cooldown_sec,
            quiet_hours: defaults.quiet_hours,
        }
    }
}

/// 界面那一级可接受的节流上限（秒，一天）。再大就不是「节流」而是「关掉」——
/// 要关掉请用总开关（284⑤：范围校验挡在端点，落库前报错不静默）。
pub const COOLDOWN_SEC_MAX: u64 = 86_400;

/// 礼貌单元的**范围**校验（完整性由类型保证）；`Err` 是面向用户的中文报文
/// （照 `ping_bluebubbles` 的返回形状）。`config.toml` 那一级不过这道闸——它只有
/// 解析期校验（47 / 103 / 134 的 fail fast 姿态）。
pub fn validate_politeness(p: &NotifyPoliteness) -> std::result::Result<(), String> {
    if p.cooldown_sec > COOLDOWN_SEC_MAX {
        return Err(format!(
            "节流要填 0–{COOLDOWN_SEC_MAX} 之间的整数秒（0 = 不节流）"
        ));
    }
    if p.quiet_hours[0] > 23 || p.quiet_hours[1] > 23 {
        return Err("免打扰起止要填 0–23 之间的整点（起止相同 = 全天不静默）".into());
    }
    Ok(())
}

/// 通知设置的两级状态（决策 272⑥；284② 添第二单元）：总开关 + 两个**各自成立**的
/// 界面单元——通道（送到哪）与礼貌（什么时候准吵）。两边可以一个来自界面、一个来自
/// `config.toml`；不混的是**组内**。
#[derive(Debug, Clone, PartialEq)]
pub struct NotifySettingsState {
    /// **一颗总开关**（272⑧）：整条通道开/关，非每类一颗。关死一切——单元与配置
    /// 那一级都不再看。
    pub enabled: bool,
    /// 界面保存的**通道**单元；`None` = 没保存过 → 回落 `config.toml` 那一级。
    pub unit: Option<NotifyChannelOverride>,
    /// 界面保存的**礼貌**单元（284②）；`None` = 没保存过 → 回落 `config.toml`。
    pub politeness: Option<NotifyPoliteness>,
}

/// 两级解析（决策 272⑥）：界面单元 > `config.toml`，总开关关死一切。
///
/// `Err` = 那一级**声明了通道却缺件**。配置级缺件在 `Config::validate` 就该被拦下
/// （fail fast，47 / 103 / 134 姿态）；单元级的缺件由设置端点在**落库前**拦（报错
/// 不静默，272⑧）——本函数的 `Err` 是两道闸之后的兜底。
pub fn resolve_notify_target(
    config: &crate::config::NotifyConfig,
    state: &NotifySettingsState,
) -> Result<Option<NotifyTarget>> {
    if !state.enabled {
        return Ok(None);
    }
    match &state.unit {
        Some(unit) => resolve_unit(
            unit.channel,
            unit.webhook_url.as_deref(),
            unit.bluebubbles_url.as_deref(),
            unit.bluebubbles_password.as_deref(),
            unit.bluebubbles_recipient.as_deref(),
        ),
        None => resolve_config_level(config),
    }
}

/// 礼貌两级解析（决策 284②）：界面单元 > `config.toml`。**总开关不参与**——它管的是
/// 出口在不在（关着时 `resolve_notify_target` 已经给出 `None`），礼貌只在出口存在时
/// 才有意义。两级都缺席时是 `[notify]` 的缺省（268 的零配置姿态）。
pub fn resolve_politeness(
    config: &crate::config::NotifyConfig,
    state: &NotifySettingsState,
) -> NotifyPoliteness {
    state.politeness.unwrap_or(NotifyPoliteness {
        cooldown_sec: config.cooldown_sec,
        quiet_hours: config.quiet_hours,
    })
}

/// 配置级（声明式缺省）：`webhook_url` 缺席 = 整段关死（268①，零配置零行为）；
/// `format = bluebubbles` 声明了就要求三件齐（缺件是配置错误，不是「没配」）；
/// `format = webpush` 没有任何必填件（订阅与密钥对都在库里，见 [`NotifyTarget::WebPush`]）
/// ——声明了就是开了。
fn resolve_config_level(config: &crate::config::NotifyConfig) -> Result<Option<NotifyTarget>> {
    match config.format {
        NotifyFormat::Generic | NotifyFormat::Feishu => match config.webhook_url.as_deref() {
            Some(url) if !url.trim().is_empty() => Ok(Some(NotifyTarget::Webhook {
                url: url.trim().to_string(),
                format: config.format,
            })),
            _ => Ok(None),
        },
        NotifyFormat::BlueBubbles => resolve_unit(
            config.format,
            None,
            config.bluebubbles_url.as_deref(),
            config.bluebubbles_password.as_deref(),
            config.bluebubbles_recipient.as_deref(),
        ),
        NotifyFormat::WebPush => Ok(Some(NotifyTarget::WebPush)),
    }
}

/// 把「一个通道声明」变成投递目标；缺件 = `Err`。端点收尾的 `/` 在这里归一
/// （用户粘贴的 URL 常带），发送侧再兜一次底。
fn resolve_unit(
    channel: NotifyFormat,
    webhook_url: Option<&str>,
    bluebubbles_url: Option<&str>,
    bluebubbles_password: Option<&str>,
    bluebubbles_recipient: Option<&str>,
) -> Result<Option<NotifyTarget>> {
    fn non_empty(v: Option<&str>) -> Option<&str> {
        v.map(str::trim).filter(|s| !s.is_empty())
    }
    match channel {
        NotifyFormat::Generic | NotifyFormat::Feishu => {
            let url = non_empty(webhook_url).ok_or_else(|| {
                Error::Config("离线通知缺 webhook_url（generic / feishu 通道的必填件）".into())
            })?;
            // 与 bluebubbles 分支同一把尺（code-review 2026-09-25 收口）：用户填的
            // 就该是完整 URL，缺 scheme 的半截地址挡在解析层而不是第一次投递时。
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Err(Error::Config(
                    "webhook_url 要写成 http:// 或 https:// 开头的完整地址（含 token）".into(),
                ));
            }
            Ok(Some(NotifyTarget::Webhook {
                url: url.to_string(),
                format: channel,
            }))
        }
        NotifyFormat::BlueBubbles => {
            let endpoint = non_empty(bluebubbles_url)
                .ok_or_else(|| Error::Config("离线通知缺 bluebubbles_url".into()))?;
            let password = non_empty(bluebubbles_password)
                .ok_or_else(|| Error::Config("离线通知缺 bluebubbles_password".into()))?;
            let address = non_empty(bluebubbles_recipient).ok_or_else(|| {
                Error::Config("离线通知缺 bluebubbles_recipient（iMessage 收件地址）".into())
            })?;
            if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
                return Err(Error::Config(
                    "bluebubbles_url 要写成 http:// 或 https:// 开头的完整地址".into(),
                ));
            }
            Ok(Some(NotifyTarget::BlueBubbles {
                endpoint: endpoint.trim_end_matches('/').to_string(),
                password: password.to_string(),
                address: address.to_string(),
            }))
        }
        // 浏览器推送：**没有必填件**（订阅行与 VAPID 密钥对都在库里，票面「零手工配置」）。
        // 声明了就是开了——「通道选它」本身就是完整的声明，故这里不看那几个
        // Option 参数（客户端传来的字段一律忽略）。
        NotifyFormat::WebPush => Ok(Some(NotifyTarget::WebPush)),
    }
}

/// BlueBubbles 连通性探针（决策 272⑧）：`GET /api/v1/ping?password=`。
/// **够不着不当成功**——调用方（开启开关 / 保存单元）必须把 `Err` 当失败处理。
/// 失败文案**不含 URL**（password 在 query 里，URL 整条是秘密）。
pub async fn ping_bluebubbles(endpoint: &str, password: &str) -> std::result::Result<(), String> {
    let url = format!(
        "{}/api/v1/ping?password={}",
        endpoint.trim_end_matches('/'),
        encode_query_component(password)
    );
    let client = reqwest::Client::builder()
        .timeout(WEBHOOK_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?;
    match client.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => Ok(()),
        Ok(resp) => Err(format!(
            "BlueBubbles 回了 {}——检查端点地址与 password 是否正确",
            resp.status()
        )),
        Err(e) => Err(format!("够不着 BlueBubbles 服务：{}", e.without_url())),
    }
}

/// 出站口（决策 268）：一个投递目标 + 一份礼貌策略 + 一份每类节流状态。
///
/// 与 `Store` 的挂接是 **可选** 的（`set_notifier`）：没配通道 = 整段关死，没配
/// webhook 的部署里这条路径一个字节都不出。
///
/// `store`（pwa-webpush 02）：浏览器推送的**订阅行与 VAPID 密钥对**都在库里，出口
/// 按它现场读（其余三条通道不看这个字段）。持的是一个 `Store` 句柄克隆——它本身
/// 就是 Clone 的（内部 `Arc` 的池与出口槽），不构成引用环：`clear_notifier` /
/// `set_notifier` 换掉出口时这个克隆随旧出口一起落地。
pub struct WebhookNotifier {
    target: NotifyTarget,
    client: reqwest::Client,
    /// 生效的礼貌两件（决策 284②：可能是界面单元，也可能是 `config.toml`）——
    /// 由调用方解析好再构造，出口自己不认识两级。
    politeness: NotifyPoliteness,
    clock: Arc<dyn Clock>,
    store: Store,
    /// 每类最近一次**尝试**时刻（镜像前端 `lastNotifiedAt` 的 per-class 语义）。
    /// 「尝试」而非「成功」：best-effort 不重试，重试循环会把通知变成新的噪音源。
    last_sent: Mutex<HashMap<NotifyClass, DateTime<Utc>>>,
}

/// 一次投递的形态（pwa-webpush 02 添第二支）：固定地址一条 POST，还是扇出到所有活订阅。
/// 分流只发生一次（[`NotifyTarget::delivery`]），`dispatch` 的礼貌门与占坑对两支完全相同。
enum Delivery {
    Http(String),
    Push,
}

impl WebhookNotifier {
    pub fn new(
        target: NotifyTarget,
        politeness: NotifyPoliteness,
        clock: Arc<dyn Clock>,
        store: Store,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(WEBHOOK_TIMEOUT)
            // 不跟随重定向：这个 URL 是含 token 的秘密，302 会把它带去别的主机
            // （与决策 266 网口同一条姿态——出站面的跳转就是洞）。浏览器的推送
            // endpoint 同理：它是能力 URL，跟一次 302 就是把它交给别人。
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("reqwest 客户端构造不该失败");
        Self {
            target,
            client,
            politeness,
            clock,
            store,
            last_sent: Mutex::new(HashMap::new()),
        }
    }

    /// **在飞的那一份**礼貌（决策 284）：设置端点与契约测试要能核对「保存即活生效」——
    /// 页面上的读数与出口真正用的那份不许是两回事。
    pub fn politeness(&self) -> NotifyPoliteness {
        self.politeness
    }

    /// 一次通知机会——attention 线（调用点 = `note_attention`，`wakes()` 已由调用方判过）。
    pub fn notify(
        &self,
        kind: AttentionKind,
        task_id: &str,
        occurred_at: DateTime<Utc>,
        detail: Option<&serde_json::Value>,
    ) {
        let Some(cls) = notification_class(kind) else {
            return;
        };
        let payload = payload_for(
            self.target.format(),
            kind,
            task_id,
            occurred_at,
            detail,
            self.target.bb_address(),
        );
        self.dispatch(cls, payload);
    }

    /// 值班长**回话完成**（决策 272②③）——第二触发面的入口，调用点是 `foreman.rs`
    /// 的 `respond()` 收口之后（**不**写 attention 表，见头注）。
    ///
    /// **`traces.len()` 的门由调用方先过**（272③）：门必须发生在本方法之前——短轮
    /// 连 cooldown 槽都不碰，否则一次快问快答会把 `foreman_reply` 的 300 秒槽占掉、
    /// 把真正要追人的那条吞掉（cooldown 倒挂）。
    ///
    /// `session_id`（pwa-webpush 02 添）只进深链：点开推送落到**这一班**（其余三种
    /// 格式看不见它）。
    pub fn notify_foreman_reply(
        &self,
        session_id: &str,
        session_name: &str,
        reply: &str,
        occurred_at: DateTime<Utc>,
    ) {
        let payload = foreman_reply_payload_for(
            self.target.format(),
            session_id,
            session_name,
            reply,
            occurred_at,
            self.target.bb_address(),
        );
        self.dispatch(NotifyClass::ForemanReply, payload);
    }

    /// 值班长**失败收口**（决策 272③）：类 = `failed`（恒发——免打扰与节流都不拦，
    /// 「这一轮没跑起来」与任务失败同一档）。调用点 = `record_failed_turn` /
    /// `record_interrupted_turn`，与台账同拍（同批同类只落一行的那一格才叫人）。
    pub fn notify_foreman_failure(
        &self,
        session_id: &str,
        session_name: &str,
        kind: &str,
        occurred_at: DateTime<Utc>,
    ) {
        let payload = foreman_failure_payload_for(
            self.target.format(),
            session_id,
            session_name,
            kind,
            occurred_at,
            self.target.bb_address(),
        );
        self.dispatch(NotifyClass::Failed, payload);
    }

    /// 礼貌门 + 占坑 + 后台投（三条入口的**唯一**漏斗）。
    ///
    /// 通过礼貌门就**先占坑再后台投**：cooldown 记的是上次尝试；投递失败只记日志
    /// （reqwest 的错误 Display 会带 URL，走 `without_url()`——秘密不进日志）。
    fn dispatch(&self, cls: NotifyClass, payload: serde_json::Value) {
        let now = self.clock.now();
        // 免打扰按**服务器本地**整点（镜像前端「浏览器本地」的语义——各自服务各自的人）。
        let hour = now.with_timezone(&Local).hour();
        let mut guard = match self.last_sent.lock() {
            Ok(g) => g,
            Err(_) => return, // 锁中毒：这一跳不发比 panic 强
        };
        let age = guard
            .get(&cls)
            .map(|t| (now - *t).num_seconds().max(0) as u64);
        if !should_notify(
            cls,
            hour,
            age,
            self.politeness.cooldown_sec,
            self.politeness.quiet_hours,
        ) {
            return;
        }
        guard.insert(cls, now);
        drop(guard);

        match self.target.delivery() {
            Delivery::Http(url) => {
                let client = self.client.clone();
                tokio::spawn(async move {
                    match client.post(&url).json(&payload).send().await {
                        Ok(resp) if resp.status().is_success() => {}
                        Ok(resp) => tracing::warn!(
                            status = %resp.status(),
                            "通知 webhook 返回非 2xx（best-effort，不重试）"
                        ),
                        Err(e) => tracing::warn!(
                            error = %e.without_url(),
                            "通知 webhook 投递失败（best-effort，不重试）"
                        ),
                    }
                });
            }
            Delivery::Push => self.fan_out_push(payload, now),
        }
    }

    /// 浏览器推送的扇出（pwa-webpush 02）：**每条活订阅各发一条**（票面：N 条 HTTP，
    /// N = 活订阅数），每条各做一次 RFC 8291 加密。
    ///
    /// 记账位置与 HTTP 那一支一致（礼貌门在 `dispatch` 里已经过了、坑也占了）——
    /// 「一类事件在窗口内只推一条」，与推到几台设备无关（那是同一件事的多个副本）。
    ///
    /// 逐条的错误处置（票面「410 / 失败即删行」的落点，**写清口径**）：
    /// - **404 / 410** = 推送服务说这条订阅没了（RFC 8030 §5.4）→ **删行**。这是唯一
    ///   一种「行是死」的证据，也是设备清单能保持干净的全部机制。
    /// - 其余非 2xx（5xx / 429 / 401…）与网络错 → 只记日志、**不删行**：那些说的是
    ///   「这一次没送出去」（推送服务在抖、VAPID 配错了），删行等于一次网络抖动就把
    ///   用户的设备清单清空——那比「清单里留着一条暂时收不到的行」坏得多。
    fn fan_out_push(&self, payload: serde_json::Value, now: DateTime<Utc>) {
        let store = self.store.clone();
        let client = self.client.clone();
        tokio::spawn(async move {
            let keys = match store.push_vapid_keys().await {
                Ok(Some(keys)) => keys,
                Ok(None) => {
                    tracing::warn!(
                        "浏览器推送缺 VAPID 密钥对（在设置页保存一次通道即可生成），这一条不出站"
                    );
                    return;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "读 VAPID 密钥对失败，这一条不出站");
                    return;
                }
            };
            let subscriptions = match store.list_push_subscriptions().await {
                Ok(list) => list,
                Err(e) => {
                    tracing::warn!(error = %e, "读推送订阅失败，这一条不出站");
                    return;
                }
            };
            if subscriptions.is_empty() {
                tracing::debug!("浏览器推送没有订阅设备，这一条不出站");
                return;
            }
            let body = match serde_json::to_vec(&payload) {
                Ok(body) => body,
                Err(e) => {
                    tracing::warn!(error = %e, "推送报文序列化失败");
                    return;
                }
            };
            for subscription in subscriptions {
                // 每条订阅各自签一次（aud 是**那条** endpoint 的 origin）、各自加密
                // （临时密钥与 salt 每条都新）。
                let authorization =
                    match crate::webpush::vapid_authorization(&keys, &subscription.endpoint, now) {
                        Ok(value) => value,
                        Err(e) => {
                            tracing::warn!(error = %e, "VAPID 签名失败，跳过这条订阅");
                            continue;
                        }
                    };
                let encrypted = match crate::webpush::encrypt_payload(
                    &subscription.p256dh,
                    &subscription.auth,
                    &body,
                ) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        tracing::warn!(error = %e, "推送报文集不上，跳过这条订阅");
                        continue;
                    }
                };
                let response = client
                    .post(&subscription.endpoint)
                    .header("Authorization", authorization)
                    .header("Content-Encoding", "aes128gcm")
                    .header("Content-Type", "application/octet-stream")
                    .header("TTL", crate::webpush::PUSH_TTL_SECS.to_string())
                    .body(encrypted)
                    .send()
                    .await;
                match response {
                    Ok(resp) if resp.status().is_success() => {}
                    Ok(resp) if matches!(resp.status().as_u16(), 404 | 410) => {
                        // 订阅已失效：删掉它（这就是设备清单永远只有活订阅的那条路）。
                        match store.delete_push_subscription(subscription.id).await {
                            Ok(_) => tracing::info!(
                                device = %subscription.endpoint_hint(),
                                "推送服务说这条订阅已失效（{}），已从清单里删掉",
                                resp.status()
                            ),
                            Err(e) => tracing::warn!(error = %e, "删除失效订阅失败"),
                        }
                    }
                    Ok(resp) => tracing::warn!(
                        status = %resp.status(),
                        device = %subscription.endpoint_hint(),
                        "推送服务回非 2xx（best-effort，不重试；不清行）"
                    ),
                    Err(e) => tracing::warn!(
                        error = %e.without_url(),
                        device = %subscription.endpoint_hint(),
                        "推送投递失败（best-effort，不重试；不清行）"
                    ),
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 跨语言共享表的 **Rust 侧那一半**（决策 268③，照决策 246 回环表先例）：
    /// 与前端 `notificationPolicyFixture.test.ts` 读同一份 fixture、同一断言方向——
    /// 一侧改了规范另一侧没跟时，落后的那一侧变红。
    #[test]
    fn shared_fixture_covers_the_notification_policy_table() {
        #[derive(serde::Deserialize)]
        struct Fixture {
            policy: Policy,
            cases: Vec<Case>,
        }
        #[derive(serde::Deserialize)]
        struct Policy {
            cooldown_sec: u64,
            quiet_hours: [u8; 2],
        }
        #[derive(serde::Deserialize)]
        struct Case {
            id: String,
            cls: String,
            hour: u32,
            age_sec: Option<u64>,
            expected: bool,
        }

        let raw = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/notification_policy.json"
        ));
        let fixture: Fixture = serde_json::from_str(raw).expect("fixture 必须是合法 JSON");
        assert!(
            fixture.cases.len() >= 20,
            "全表不该被悄悄裁短：{}",
            fixture.cases.len()
        );
        // 形状守卫（与 vitest 侧同一把尺）：放行 / 拦截两侧都非空——
        // 退化成全 `true` 时逐行断言照样绿，那等于没测。
        assert!(
            fixture.cases.iter().any(|c| c.expected),
            "放行侧一行都不剩 = 表退化了"
        );
        assert!(
            fixture.cases.iter().any(|c| !c.expected),
            "拦截侧一行都不剩 = 表退化了"
        );
        // 必需行**按名**钉住（与 vitest 同一把尺）。
        for required in [
            "failed-quiet-bypass",
            "pending-quiet-exempt",
            "cooldown-strict-300",
            "cooldown-299-blocks",
            "done-quiet-silenced",
            "done-hour8-not-quiet",
        ] {
            assert!(
                fixture.cases.iter().any(|c| c.id == required),
                "缺必需行 {required}"
            );
        }
        // fixture 的 policy 与 Rust 生产缺省没有漂（漂了表就形同虚设——与 vitest 侧
        // 同一把尺：那边断言 fixture.policy ↔ DEFAULT_NOTIFICATION_POLICY）。
        let defaults = crate::config::NotifyConfig::default();
        assert_eq!(fixture.policy.cooldown_sec, defaults.cooldown_sec);
        assert_eq!(fixture.policy.quiet_hours, defaults.quiet_hours);
        // `cancelled` 不进共享表（前端偏好面差异，头注写明）——与 vitest 守卫同尺。
        assert!(
            fixture.cases.iter().all(|c| c.cls != "cancelled"),
            "cancelled 是前端偏好面差异，不该进共享表"
        );
        // `foreman_reply` 也不进共享表（决策 272④：后端独有的类，前端没有回话完成的
        // SSE 事件）——照 cancelled 先例在两侧守卫显式记为差异。
        assert!(
            fixture.cases.iter().all(|c| c.cls != "foreman_reply"),
            "foreman_reply 是决策 272 的后端独有类，不该进共享表"
        );
        for case in &fixture.cases {
            let cls = NotifyClass::parse(&case.cls)
                .unwrap_or_else(|| panic!("{}：未知 class {}", case.id, case.cls));
            let got = should_notify(
                cls,
                case.hour,
                case.age_sec,
                fixture.policy.cooldown_sec,
                fixture.policy.quiet_hours,
            );
            assert_eq!(got, case.expected, "{}", case.id);
        }
    }

    /// 268④ 六字段契约原样（generic 缺省分支）：归因白名单在、detail 原文不出网。
    #[test]
    fn generic_payload_is_the_six_field_contract() {
        let p = payload_for(
            NotifyFormat::Generic,
            AttentionKind::RunFailed,
            "t1",
            Utc::now(),
            Some(&serde_json::json!({ "stage": "test", "error": "boom" })),
            None,
        );
        assert_eq!(p["source"], "agentpipeline", "{p}");
        assert_eq!(p["kind"], "run_failed", "{p}");
        assert_eq!(p["task_id"], "t1", "{p}");
        assert!(p["occurred_at"].is_string(), "{p}");
        assert!(
            p["title"]
                .as_str()
                .unwrap()
                .contains("[AgentPipeline] 运行失败 t1"),
            "{p}"
        );
        let body = p["body"].as_str().unwrap();
        assert!(body.contains("stage=test"), "{p}");
        assert!(
            !body.contains("run_failed"),
            "body 不再复读 kind（决策 344）：{p}"
        );
        assert!(!body.contains("boom"), "detail 原文不出网：{p}");
    }

    /// 飞书分支（决策 270④）：`msg_type=text`、text 以关键词前缀开头、原文不出网、
    /// 不带通用字段（飞书机器人拒收未知顶层字段之外的形状——以官方文档形状为准）。
    #[test]
    fn feishu_payload_is_a_text_message_with_keyword_prefix() {
        let p = payload_for(
            NotifyFormat::Feishu,
            AttentionKind::TaskPending,
            "t9",
            Utc::now(),
            Some(&serde_json::json!({ "pending_kind": "gate", "error": "boom" })),
            None,
        );
        assert_eq!(p["msg_type"], "text", "{p}");
        let text = p["content"]["text"].as_str().unwrap();
        assert!(
            text.starts_with("[AgentPipeline] 待拍板 t9"),
            "自定义关键词靠这个前缀命中（决策 344：kind 换中文标签、标签前置）：{text}"
        );
        assert!(text.contains("pending_kind=gate"), "{text}");
        assert!(!text.contains("boom"), "detail 原文不出网：{text}");
        assert!(p.get("source").is_none(), "飞书格式不该带通用字段：{p}");
    }

    // ── 决策 272①：BlueBubbles 第四支 ──

    /// BlueBubbles 分支：`{chatGuid, tempGuid, message, method}`，正文在 `message`
    /// （官方文档页写 `text` 是错的）、以关键词前缀开头、chatGuid 按 `iMessage;-;<地址>` 拼。
    #[test]
    fn bluebubbles_payload_is_a_send_text_body_addressed_by_chat_guid() {
        let p = payload_for(
            NotifyFormat::BlueBubbles,
            AttentionKind::TaskPending,
            "t7",
            Utc::now(),
            Some(&serde_json::json!({ "pending_kind": "gate", "error": "boom" })),
            Some("me@icloud.com"),
        );
        assert_eq!(p["chatGuid"], "iMessage;-;me@icloud.com", "{p}");
        assert!(
            p["tempGuid"].as_str().unwrap().len() >= 26,
            "tempGuid 是 Ulid：{p}"
        );
        let message = p["message"].as_str().unwrap();
        assert!(
            message.starts_with("[AgentPipeline] 待拍板 t7"),
            "关键词前缀照旧命中：{message}"
        );
        assert!(message.contains("pending_kind=gate"), "{message}");
        assert!(!message.contains("boom"), "detail 原文不出网：{message}");
        assert_eq!(p["method"], "apple-script", "{p}");
        assert!(p.get("source").is_none(), "BlueBubbles 不带通用字段：{p}");
    }

    /// `tempGuid` 是服务端发送队列的去重键（重复即 400）——两次调用必须不同。
    #[test]
    fn two_bluebubbles_payloads_carry_different_temp_guids() {
        let a = payload_for(
            NotifyFormat::BlueBubbles,
            AttentionKind::TaskPending,
            "t7",
            Utc::now(),
            None,
            Some("me@icloud.com"),
        );
        let b = payload_for(
            NotifyFormat::BlueBubbles,
            AttentionKind::TaskPending,
            "t7",
            Utc::now(),
            None,
            Some("me@icloud.com"),
        );
        assert_ne!(a["tempGuid"], b["tempGuid"], "{a} / {b}");
    }

    // ── 决策 272②⑤：回话线与失败线的报文 ──

    /// 回话线：generic 六字段里**没有 task_id**（省略，不填哨兵）；正文 = 回话原文
    /// （截断）+ 会话名；feishu / bluebubbles 两支的正文同源。
    #[test]
    fn foreman_reply_payload_carries_the_reply_body_and_session_name() {
        let reply = "查完了：闸门挂在 lint。".repeat(30); // > 200 字
        let p = foreman_reply_payload_for(
            NotifyFormat::Generic,
            "s1",
            "晚上的重构",
            &reply,
            Utc::now(),
            None,
        );
        assert_eq!(p["kind"], "foreman_reply", "{p}");
        assert!(
            p.get("task_id").is_none(),
            "回话线没有 task_id，省略而不是哨兵：{p}"
        );
        assert!(p["title"].as_str().unwrap().contains("晚上的重构"), "{p}");
        let body = p["body"].as_str().unwrap();
        assert!(body.starts_with("查完了"), "{p}");
        assert!(body.ends_with('…'), "超长要截断：{p}");
        assert!(body.chars().count() <= FOREMAN_REPLY_BODY_CHARS + 1, "{p}");
    }

    /// 失败线：正文只带**类别**——`raw` 原文段与 detail 原文一个字不出网（268④ 纪律
    /// 不豁免这一线，272⑤ 的豁免只给回话正文）。
    #[test]
    fn foreman_failure_payload_carries_only_the_kind_not_the_raw_text() {
        let p = foreman_failure_payload_for(
            NotifyFormat::Feishu,
            "s1",
            "晚上的重构",
            "llm_quota",
            Utc::now(),
            None,
        );
        let text = p["content"]["text"].as_str().unwrap();
        assert!(
            text.starts_with("[AgentPipeline] 晚上的重构 回话失败"),
            "{text}"
        );
        assert!(text.contains("llm_quota"), "{text}");
        assert!(!text.contains("原文"), "raw 原文段不出网：{text}");
    }

    // ── pwa-webpush 02：第五支（浏览器推送）的报文与深链 ──

    /// 推送报文就是票面点名的三件：`title` / `body` / `url`——**没有**通用格式的
    /// `source` / `occurred_at`（那一份是 webhook 的契约，浏览器侧只消费这三件）。
    #[test]
    fn webpush_payload_carries_title_body_and_url_only() {
        let p = payload_for(
            NotifyFormat::WebPush,
            AttentionKind::TaskPending,
            "t1",
            Utc::now(),
            Some(&serde_json::json!({ "pending_kind": "gate", "error": "boom" })),
            None,
        );
        assert_eq!(p.as_object().unwrap().len(), 3, "{p}");
        assert!(p["title"]
            .as_str()
            .unwrap()
            .contains("[AgentPipeline] 待拍板 t1"));
        assert!(p["body"].as_str().unwrap().contains("pending_kind=gate"));
        assert!(
            !p["body"].as_str().unwrap().contains("task_pending"),
            "body 不再复读 kind（决策 344）：{p}"
        );
        assert!(!p["body"].as_str().unwrap().contains("boom"), "{p}");
        assert_eq!(p["url"], "#/task/t1", "{p}");
    }

    /// **深链规则**（票面两句）：卡片级（无 `run_id`）→ 那张卡；流水线级（带 `run_id`）
    /// → 同一页的**那次运行**（`?run=` 是任务详情对话页签认的参数）。
    #[test]
    fn the_deep_link_tells_the_card_from_the_run() {
        assert_eq!(attention_deep_link("t1", None), "#/task/t1");
        assert_eq!(
            attention_deep_link("t1", Some(&serde_json::json!({ "pending_kind": "gate" }))),
            "#/task/t1",
            "没有 run_id 的事件落在卡片上"
        );
        assert_eq!(
            attention_deep_link("t1", Some(&serde_json::json!({ "run_id": 42 }))),
            "#/task/t1?run=42"
        );
        // 回话 / 失败线没有 task_id，落点是对讲台的那一班。
        assert_eq!(talk_deep_link("s9"), "#/talk?session=s9");
    }

    /// 回话线与失败线的推送报文同样带 url（落点是那一班）——三种格式的正文一字不改，
    /// 只有 webpush 这一支多出 url。
    #[test]
    fn the_foreman_lines_carry_a_talk_deep_link_for_push_only() {
        let reply = foreman_reply_payload_for(
            NotifyFormat::WebPush,
            "s7",
            "晚上的重构",
            "查完了。",
            Utc::now(),
            None,
        );
        assert_eq!(reply["url"], "#/talk?session=s7", "{reply}");
        assert_eq!(reply["title"], "[AgentPipeline] 晚上的重构 回话", "{reply}");

        let failure = foreman_failure_payload_for(
            NotifyFormat::WebPush,
            "s7",
            "晚上的重构",
            "llm_quota",
            Utc::now(),
            None,
        );
        assert_eq!(failure["url"], "#/talk?session=s7", "{failure}");

        // 非 push 格式里**没有** url 这个字段（三个既有报文一字不改）。
        let generic = foreman_reply_payload_for(
            NotifyFormat::Generic,
            "s7",
            "晚上的重构",
            "查完了。",
            Utc::now(),
            None,
        );
        assert!(generic.get("url").is_none(), "{generic}");
        let generic_attention = payload_for(
            NotifyFormat::Generic,
            AttentionKind::TaskDone,
            "t1",
            Utc::now(),
            None,
            None,
        );
        assert!(
            generic_attention.get("url").is_none(),
            "{generic_attention}"
        );
    }

    /// 格式的串形往返（落库 / 回显 / 设置页的三个消费方都认它）。
    #[test]
    fn the_webpush_format_round_trips_through_its_string_form() {
        assert_eq!(NotifyFormat::parse("webpush"), Some(NotifyFormat::WebPush));
        assert_eq!(NotifyFormat::WebPush.as_str(), "webpush");
        assert_eq!(
            serde_json::to_value(NotifyFormat::WebPush).unwrap(),
            serde_json::json!("webpush")
        );
    }

    /// 通道声明 = 目标：webpush 没有任何必填件（订阅与密钥对都在库里），
    /// 声明了就解析得出目标；卡关（总开关）照旧关死一切。
    #[test]
    fn the_webpush_channel_needs_no_fields_and_obeys_the_master_switch() {
        let unit = NotifyChannelOverride {
            channel: NotifyFormat::WebPush,
            webhook_url: None,
            bluebubbles_url: None,
            bluebubbles_password: None,
            bluebubbles_recipient: None,
        };
        let state = NotifySettingsState {
            enabled: true,
            unit: Some(unit.clone()),
            politeness: None,
        };
        assert_eq!(
            resolve_notify_target(&config_with(NotifyFormat::Generic), &state).unwrap(),
            Some(NotifyTarget::WebPush),
            "单元四件全空也解析得出（它本来就不需要那些件）"
        );

        // 配置级同一把尺：`format = webpush` + 没有 webhook_url 也算声明了。
        let config = config_with(NotifyFormat::WebPush);
        assert_eq!(
            resolve_notify_target(
                &config,
                &NotifySettingsState {
                    enabled: true,
                    unit: None,
                    politeness: None,
                }
            )
            .unwrap(),
            Some(NotifyTarget::WebPush)
        );

        // 总开关关死一切（272⑧ 原样）。
        assert_eq!(
            resolve_notify_target(
                &config,
                &NotifySettingsState {
                    enabled: false,
                    unit: Some(unit),
                    politeness: None,
                }
            )
            .unwrap(),
            None
        );
    }

    /// 推送目标**没有单一发送地址**（扇出到每条订阅的 endpoint），故它不走
    /// `Delivery::Http`——`delivery()` 的三支里只有它给出另一个形状。
    #[test]
    fn the_push_target_is_its_own_delivery_shape() {
        assert!(matches!(NotifyTarget::WebPush.delivery(), Delivery::Push));
        assert!(matches!(
            NotifyTarget::Webhook {
                url: "https://x".into(),
                format: NotifyFormat::Generic,
            }
            .delivery(),
            Delivery::Http(url) if url == "https://x"
        ));
    }

    /// query 参数编码：password 里的结构字符不把 query 拼坏。
    #[test]
    fn bluebubbles_send_url_percent_encodes_the_password() {
        let target = NotifyTarget::BlueBubbles {
            endpoint: "http://127.0.0.1:1234/".to_string(),
            password: "a&b=c d%e".to_string(),
            address: "me@icloud.com".to_string(),
        };
        let url = match target.delivery() {
            Delivery::Http(url) => url,
            Delivery::Push => panic!("BlueBubbles 走 HTTP"),
        };
        assert!(
            url.starts_with("http://127.0.0.1:1234/api/v1/message/text?password="),
            "{url}"
        );
        assert_eq!(
            url, "http://127.0.0.1:1234/api/v1/message/text?password=a%26b%3Dc%20d%25e",
            "{url}"
        );
    }

    // ── 决策 272⑥：两级解析 ──

    fn config_with(format: NotifyFormat) -> crate::config::NotifyConfig {
        crate::config::NotifyConfig {
            format,
            ..crate::config::NotifyConfig::default()
        }
    }

    /// 总开关关死一切：单元在、配置齐，`enabled = false` 就是不发。
    #[test]
    fn the_master_switch_kills_everything() {
        let state = NotifySettingsState {
            enabled: false,
            politeness: None,
            unit: Some(NotifyChannelOverride {
                channel: NotifyFormat::Feishu,
                webhook_url: Some("https://x".into()),
                bluebubbles_url: None,
                bluebubbles_password: None,
                bluebubbles_recipient: None,
            }),
        };
        let resolved = resolve_notify_target(&config_with(NotifyFormat::Feishu), &state).unwrap();
        assert!(resolved.is_none(), "{resolved:?}");
    }

    /// 单元整体覆盖配置：界面指向 bluebubbles 时，配置里的 feishu URL 一个字节不参与
    ///（不允许混，272⑥）。
    #[test]
    fn the_unit_overrides_the_config_whole() {
        let state = NotifySettingsState {
            enabled: true,
            politeness: None,
            unit: Some(NotifyChannelOverride {
                channel: NotifyFormat::BlueBubbles,
                webhook_url: None,
                bluebubbles_url: Some("http://127.0.0.1:1234".into()),
                bluebubbles_password: Some("pw".into()),
                bluebubbles_recipient: Some("me@icloud.com".into()),
            }),
        };
        let resolved = resolve_notify_target(&config_with(NotifyFormat::Feishu), &state)
            .unwrap()
            .expect("单元齐件应当解析出目标");
        match resolved {
            NotifyTarget::BlueBubbles { endpoint, .. } => {
                assert_eq!(endpoint, "http://127.0.0.1:1234");
            }
            other => panic!("{other:?}"),
        }
    }

    /// 配置级：`webhook_url` 缺席 = 整段关死（268① 原样保留）。
    #[test]
    fn config_level_without_url_stays_off() {
        let state = NotifySettingsState {
            enabled: true,
            politeness: None,
            unit: None,
        };
        let resolved = resolve_notify_target(&config_with(NotifyFormat::Feishu), &state).unwrap();
        assert!(resolved.is_none(), "{resolved:?}");
    }

    /// 单元缺件 = `Err`（显式动作缺必填件要报错，不是静默关死——272⑧ 的姿态）。
    #[test]
    fn an_incomplete_unit_is_an_error_not_a_silent_off() {
        let state = NotifySettingsState {
            enabled: true,
            politeness: None,
            unit: Some(NotifyChannelOverride {
                channel: NotifyFormat::BlueBubbles,
                webhook_url: None,
                bluebubbles_url: Some("http://127.0.0.1:1234".into()),
                bluebubbles_password: None,
                bluebubbles_recipient: Some("me@icloud.com".into()),
            }),
        };
        let err = resolve_notify_target(&config_with(NotifyFormat::Generic), &state).unwrap_err();
        assert!(matches!(err, Error::Config(_)), "{err:?}");
        assert!(err.to_string().contains("bluebubbles_password"), "{err}");
    }

    /// webhook 通道同一把尺：缺 scheme 的半截地址挡在解析层（两支的信任边界一致）。
    #[test]
    fn a_webhook_url_without_scheme_is_rejected() {
        let state = NotifySettingsState {
            enabled: true,
            politeness: None,
            unit: Some(NotifyChannelOverride {
                channel: NotifyFormat::Feishu,
                webhook_url: Some("open.feishu.cn/hook/x".into()),
                bluebubbles_url: None,
                bluebubbles_password: None,
                bluebubbles_recipient: None,
            }),
        };
        let err = resolve_notify_target(&config_with(NotifyFormat::Generic), &state).unwrap_err();
        assert!(err.to_string().contains("http://"), "{err}");
    }

    /// 端点必须带 scheme：libgit2 那条「用户填的字符串不能有机会变成 URL 形态」的
    /// 反面——这里用户填的**就该是** URL，缺 scheme 的半截地址挡在解析层。
    #[test]
    fn a_bluebubbles_endpoint_without_scheme_is_rejected() {
        let state = NotifySettingsState {
            enabled: true,
            politeness: None,
            unit: Some(NotifyChannelOverride {
                channel: NotifyFormat::BlueBubbles,
                webhook_url: None,
                bluebubbles_url: Some("127.0.0.1:1234".into()),
                bluebubbles_password: Some("pw".into()),
                bluebubbles_recipient: Some("me@icloud.com".into()),
            }),
        };
        let err = resolve_notify_target(&config_with(NotifyFormat::Generic), &state).unwrap_err();
        assert!(err.to_string().contains("http://"), "{err}");
    }

    /// 免打扰边界的纯函数钉子（fixture 没覆盖的角落：start==end、整点含头不含尾）。
    #[test]
    fn quiet_hours_boundaries_are_pinned() {
        assert!(!is_quiet_hours(3, [8, 8]), "start == end 恒不静音");
        assert!(is_quiet_hours(8, [8, 18]), "含头");
        assert!(!is_quiet_hours(18, [8, 18]), "不含尾");
        assert!(is_quiet_hours(22, [22, 8]), "跨零点含头");
        assert!(is_quiet_hours(0, [22, 8]), "跨零点的凌晨段");
        assert!(!is_quiet_hours(8, [22, 8]), "跨零点的尾点不静音");
        assert!(!is_quiet_hours(12, [22, 8]), "白天不静音");
    }

    // ── 决策 284②：礼貌两件的两级解析与范围闸 ──

    /// 礼貌两级各自成立：单元在场就整体覆盖配置（两个字段都不许回落到配置），
    /// 不在场就整体读配置——不存在「节流来自界面、免打扰来自配置」的半份。
    #[test]
    fn politeness_resolves_unit_over_config_as_a_whole() {
        let config = crate::config::NotifyConfig {
            cooldown_sec: 900,
            quiet_hours: [21, 6],
            ..crate::config::NotifyConfig::default()
        };
        let mut state = NotifySettingsState {
            enabled: true,
            unit: None,
            politeness: None,
        };
        assert_eq!(
            resolve_politeness(&config, &state),
            NotifyPoliteness {
                cooldown_sec: 900,
                quiet_hours: [21, 6]
            },
            "没有单元 = 读 config.toml"
        );

        state.politeness = Some(NotifyPoliteness {
            cooldown_sec: 0,
            quiet_hours: [23, 7],
        });
        assert_eq!(
            resolve_politeness(&config, &state),
            NotifyPoliteness {
                cooldown_sec: 0,
                quiet_hours: [23, 7]
            },
            "单元在场 = 两件都按单元的来（0 是「不节流」，不是缺省）"
        );
    }

    /// 总开关不参与礼貌解析：关着的时候出口整个不在（那是 `resolve_notify_target` 的事），
    /// 礼貌照常解析出值——设置页要能在关着的时候显示「开着的话会是哪一份」。
    #[test]
    fn politeness_is_resolved_independently_of_the_master_switch() {
        let config = crate::config::NotifyConfig::default();
        let state = NotifySettingsState {
            enabled: false,
            unit: None,
            politeness: Some(NotifyPoliteness {
                cooldown_sec: 60,
                quiet_hours: [1, 2],
            }),
        };
        assert_eq!(resolve_politeness(&config, &state).cooldown_sec, 60);
    }

    /// 范围闸（284⑤）：越界的礼貌单元在落库前被拒，报文点名是哪一件、合法区间是什么。
    #[test]
    fn politeness_range_gate_names_the_offending_field() {
        assert!(
            validate_politeness(&NotifyPoliteness::default()).is_ok(),
            "缺省必须合法"
        );
        assert!(
            validate_politeness(&NotifyPoliteness {
                cooldown_sec: COOLDOWN_SEC_MAX,
                quiet_hours: [0, 0],
            })
            .is_ok(),
            "上界含、起止相同（全天不静默）合法"
        );
        let too_long = validate_politeness(&NotifyPoliteness {
            cooldown_sec: COOLDOWN_SEC_MAX + 1,
            quiet_hours: [22, 8],
        })
        .unwrap_err();
        assert!(too_long.contains("节流"), "{too_long}");
        assert!(too_long.contains("86400"), "{too_long}");
        let bad_hour = validate_politeness(&NotifyPoliteness {
            cooldown_sec: 300,
            quiet_hours: [24, 8],
        })
        .unwrap_err();
        assert!(bad_hour.contains("免打扰"), "{bad_hour}");
    }
}
