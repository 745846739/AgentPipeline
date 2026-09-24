//! 离线通知（决策 268，票 foreman-within-boundary 02）：attention 落库且 `wakes()`
//! 时向配置的 webhook POST 一条**通用 JSON**——与前端 toast（决策 130③，仍归前端管
//! 它自己的 cooldown / quiet_hours）并存的、**出机器**的那条线。值守轮的终点是叫醒人，
//! 没人盯浏览器的夜里，叫醒必须出得了这台机器。
//!
//! 两份实现、**一张表**：礼貌语义镜像前端 `lib/notificationPolicy.ts`——`failed` 恒发
//! （免打扰与节流都不拦）、免打扰 `[22, 8)` 跨零点含头不含尾按整点、`pending` 免打扰
//! 豁免（节流照走）、每类 cooldown 严格小于——由 `tests/fixtures/notification_policy.json`
//! 把 Rust 表测试与前端 fixture 测试钉在同一份、同一断言方向（决策 246 回环表的先例）。
//!
//! **显式差异**（决策 268 允许「写清差异」而非强求同一实现）：前端还有 `notifyOn`
//! 每类开关（用户偏好面，缺省 `cancelled: false` = 永不弹）；后端没有偏好面——
//! `cancelled` 按通用类规则走 cooldown + 免打扰。共享表因此只收两边共有的语义子集
//! （`pending` / `done` / `failed` × 免打扰 × 节流），`$comment` 与前端测试各有守卫钉住
//! 这条差异不被悄悄抹平。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Local, Timelike, Utc};

use crate::clock::Clock;
use crate::storage::attention::AttentionKind;

/// 通知分类——与前端 `notificationPolicy.ts::NotificationClass` 同名同义。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotifyClass {
    Pending,
    Done,
    Failed,
    Cancelled,
}

impl NotifyClass {
    pub fn as_str(self) -> &'static str {
        match self {
            NotifyClass::Pending => "pending",
            NotifyClass::Done => "done",
            NotifyClass::Failed => "failed",
            NotifyClass::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(NotifyClass::Pending),
            "done" => Some(NotifyClass::Done),
            "failed" => Some(NotifyClass::Failed),
            "cancelled" => Some(NotifyClass::Cancelled),
            _ => None,
        }
    }
}

/// attention kind → 通知分类。
///
/// 这是**后端独有**的一张小表（前端映射的是 SSE 事件，两边的「类」同名同义、进表的
/// 成员各自钉）：`SlowRun` 不给类——它是 `wakes()` 唯一为 false 的那个，双重挡死；
/// 卡住等人的五个（pending / repeated / owner / scheduler / stale）归 `pending`——
/// 免打扰豁免正是为「等人处理」那一类设的；失败族四个归 `failed`（恒发）。
/// 由 `tests/integration/notify.rs::kind_to_class_mapping_is_pinned` 逐个钉住。
pub fn notification_class(kind: AttentionKind) -> Option<NotifyClass> {
    use AttentionKind::*;
    Some(match kind {
        SlowRun => return None,
        TaskPending | RepeatedPending | OwnerStuck | SchedulerNoEffect | TaskStale => {
            NotifyClass::Pending
        }
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

/// 出站口（决策 268）：一个 URL + 一份礼貌策略 + 一份每类节流状态。
///
/// 与 `Store` 的挂接是 **可选** 的（`set_notifier`）：URL 缺席 = 整段关死，没配
/// webhook 的部署里这条路径一个字节都不出。
pub struct WebhookNotifier {
    url: String,
    client: reqwest::Client,
    cooldown_sec: u64,
    quiet: [u8; 2],
    clock: Arc<dyn Clock>,
    /// 每类最近一次**尝试**时刻（镜像前端 `lastNotifiedAt` 的 per-class 语义）。
    /// 「尝试」而非「成功」：best-effort 不重试，重试循环会把通知变成新的噪音源。
    last_sent: Mutex<HashMap<NotifyClass, DateTime<Utc>>>,
}

impl WebhookNotifier {
    pub fn new(
        url: impl Into<String>,
        cooldown_sec: u64,
        quiet: [u8; 2],
        clock: Arc<dyn Clock>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(WEBHOOK_TIMEOUT)
            // 不跟随重定向：这个 URL 是含 token 的秘密，302 会把它带去别的主机
            // （与决策 266 网口同一条姿态——出站面的跳转就是洞）。
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("reqwest 客户端构造不该失败");
        Self {
            url: url.into(),
            client,
            cooldown_sec,
            quiet,
            clock,
            last_sent: Mutex::new(HashMap::new()),
        }
    }

    /// 一次通知机会（调用点 = `note_attention`，`wakes()` 已由调用方判过）。
    ///
    /// 通过礼貌门就**先占坑再后台投**：cooldown 记的是上次尝试；投递失败只记日志
    /// （reqwest 的错误 Display 会带 URL，走 `without_url()`——秘密不进日志）。
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
        if !should_notify(cls, hour, age, self.cooldown_sec, self.quiet) {
            return;
        }
        guard.insert(cls, now);
        drop(guard);

        // `body` 只带**归因字段**（268 明确不做：不发正文/日志原文）——`detail` 里的
        // `output` / `diagnostic` / `error` / `message` 原文一个都不出网（diagnosis 不出境
        // 的外发面纪律同款），只白名单式地挑短标识键（stage/node/attempt 之类）；
        // 挑不到就退回 kind + task_id，接收端要细节自己回系统查。
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
        let body_text = if attrs.is_empty() {
            format!("{}（{task_id}）", kind.as_str())
        } else {
            format!("{}（{task_id}） {}", kind.as_str(), attrs.join(" "))
        };
        let payload = serde_json::json!({
            "source": "agentpipeline",
            "kind": kind.as_str(),
            "task_id": task_id,
            "occurred_at": occurred_at.to_rfc3339(),
            "title": format!("[AgentPipeline] {task_id} {}", kind.as_str()),
            "body": body_text,
        });
        let url = self.url.clone();
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
}
