//! L2 集成：离线通知（决策 268，票 foreman-within-boundary 02）。
//!
//! 判据落在**真的出站了一条 HTTP**与**礼貌门挡没挡**上：
//!
//! 1. `wakes()` 的 attention 落库 → webhook 收到一条通用 JSON（字段按 268④，含 task_id）；
//! 2. 同类 300 秒 cooldown 内第二条被挡，`advance_secs(301)` 之后放行（边界严格小于）；
//! 3. 免打扰时段（本地 22–8）`done` 静音、`pending` 与 `failed` 照发（268③ 镜像前端同一张表）；
//! 4. `SlowRun`（`wakes()` 唯一为 false 的那个）永不通知；
//! 5. 没挂出口（URL 缺席 = 整段关死）时记账照常、无人惊动；
//! 6. kind → class 的映射表钉住（后端独有的一张小表，前端映射的是 SSE 事件）。
//!
//! 时钟用 `ManualClock`（通知的免打扰小时按**本地时区**算，测试先构造出「本地恰为某整点」
//! 的瞬间再把时钟拨过去——任何时区下判据都确定）。HTTP 侧复用 `web_fetch` 的 `TinyHttp`。

use std::sync::Arc;
use std::time::Duration;

use agentpipeline_core::clock::Clock;
use agentpipeline_core::notify::{
    notification_class, NotifyClass, NotifyFormat, NotifyPoliteness, NotifyTarget, WebhookNotifier,
};
use agentpipeline_core::storage::attention::AttentionKind;
use agentpipeline_core::storage::Store;
use chrono::{DateTime, Local, TimeZone, Utc};
use testkit::{ManualClock, TestHome};

use crate::web_fetch::TinyHttp;

/// 「本地恰为 `hour` 点整」的瞬间（跨零点 / DST 由 chrono 收口；测试只要判据确定）。
pub(crate) fn at_local_hour(hour: u32) -> DateTime<Utc> {
    let today = Local::now().date_naive();
    let naive = today.and_hms_opt(hour, 0, 0).unwrap();
    Local
        .from_local_datetime(&naive)
        .single()
        .or_else(|| Local.from_local_datetime(&naive).earliest())
        .expect("本地整点应当可构造")
        .with_timezone(&Utc)
}

/// 一套礼貌（决策 284）：出口的构造参数收成一个值对象后，测试与生产同一个形状。
pub(crate) fn politeness(cooldown_sec: u64, quiet_hours: [u8; 2]) -> NotifyPoliteness {
    NotifyPoliteness {
        cooldown_sec,
        quiet_hours,
    }
}

struct Fixture {
    _home: TestHome,
    store: Store,
    clock: Arc<ManualClock>,
}

async fn fixture(hour: u32) -> Fixture {
    let home = TestHome::new().unwrap();
    let clock: Arc<ManualClock> = Arc::new(ManualClock::new(at_local_hour(hour)));
    let store = Store::open(home.home().clone(), clock.clone())
        .await
        .unwrap();
    // attention 表有外键（决策 234 的记账记在真行上）：先铺真项目 + 真任务。
    store
        .create_project(&agentpipeline_core::types::Project {
            id: "p1".into(),
            name: "通知测试".into(),
            local_path: home.home().root().display().to_string(),
            default_branch: "main".into(),
            language: None,
            test_framework: None,
            lint_command: None,
            agents_md_path: None,
            created_at: store.now(),
        })
        .await
        .unwrap();
    store
        .create_task(&agentpipeline_core::storage::tasks::NewTask::new(
            "t1",
            "通知测试任务",
            "p1",
        ))
        .await
        .unwrap();
    Fixture {
        _home: home,
        store,
        clock,
    }
}

impl Fixture {
    fn attach(&self, server: &TinyHttp) {
        self.attach_format(server, NotifyFormat::Generic);
    }

    fn attach_format(&self, server: &TinyHttp, format: NotifyFormat) {
        self.store.set_notifier(Arc::new(WebhookNotifier::new(
            NotifyTarget::Webhook {
                url: server.url("/hook"),
                format,
            },
            politeness(300, [22, 8]),
            self.clock.clone(),
            self.store.clone(),
        )));
    }

    /// 挂上任意目标并把出口交还调用方——回话线 / 失败线的入口在**出口**上
    /// （`notify_foreman_reply` / `notify_foreman_failure`），测试要拿得到它。
    fn attach_target(&self, target: NotifyTarget) -> Arc<WebhookNotifier> {
        let notifier = Arc::new(WebhookNotifier::new(
            target,
            politeness(300, [22, 8]),
            self.clock.clone(),
            self.store.clone(),
        ));
        self.store.set_notifier(notifier.clone());
        notifier
    }
}

/// 轮询等到第 `want` 次命中（投递是 best-effort 后台任务，测试只能等）。
async fn wait_hits(server: &TinyHttp, want: usize, ms: u64) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_millis(ms);
    while std::time::Instant::now() < deadline {
        if server.hits() >= want {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    server.hits() >= want
}

async fn note(f: &Fixture, kind: AttentionKind, occurred_at: DateTime<Utc>) {
    let detail = serde_json::json!({ "error": "boom" });
    f.store
        .note_attention("t1", kind, occurred_at, Some(&detail))
        .await
        .unwrap();
}

#[tokio::test]
async fn an_attention_that_wakes_posts_generic_json_to_the_webhook() {
    let f = fixture(12).await;
    let server = TinyHttp::spawn("200", "application/json", b"{}".to_vec(), Duration::ZERO);
    f.attach(&server);

    note(&f, AttentionKind::RunFailed, f.clock.now()).await;
    assert!(
        wait_hits(&server, 1, 3_000).await,
        "wakes() 的 attention 应当出站一条 webhook"
    );

    let raw = String::from_utf8_lossy(&server.body()).to_string();
    let payload: serde_json::Value = serde_json::from_str(&raw).expect("payload 是合法 JSON");
    assert_eq!(payload["source"], "agentpipeline", "{payload}");
    assert_eq!(payload["kind"], "run_failed", "{payload}");
    assert_eq!(payload["task_id"], "t1", "{payload}");
    assert!(payload["occurred_at"].is_string(), "{payload}");
    assert!(
        payload["title"].as_str().unwrap().contains("t1"),
        "{payload}"
    );
    // body 只带归因字段（268 明确不做：detail 原文不出网）
    assert!(
        payload["body"].as_str().unwrap().contains("run_failed"),
        "{payload}"
    );
    assert!(
        !payload["body"].as_str().unwrap().contains("boom"),
        "detail 原文一个字都不该出网：{payload}"
    );
    // 方法行是 POST（TinyHttp 的首行记录）。
    assert!(
        server.first_request_line().starts_with("POST"),
        "{}",
        server.first_request_line()
    );
}

#[tokio::test]
async fn feishu_format_posts_a_text_message_instead_of_generic_json() {
    let f = fixture(12).await;
    let server = TinyHttp::spawn("200", "application/json", b"{}".to_vec(), Duration::ZERO);
    f.attach_format(&server, NotifyFormat::Feishu);

    note(&f, AttentionKind::RunFailed, f.clock.now()).await;
    assert!(
        wait_hits(&server, 1, 3_000).await,
        "format = feishu 不改变触发面：wakes() 照样出站"
    );

    let raw = String::from_utf8_lossy(&server.body()).to_string();
    let payload: serde_json::Value = serde_json::from_str(&raw).expect("payload 是合法 JSON");
    assert_eq!(payload["msg_type"], "text", "{payload}");
    let text = payload["content"]["text"].as_str().unwrap();
    assert!(
        text.starts_with("[AgentPipeline] t1 run_failed"),
        "自定义关键词前缀 + kind + task_id：{text}"
    );
    // 归因纪律与格式无关：detail 原文一个字都不出网（270② 分流前共用）。
    assert!(!text.contains("boom"), "detail 原文不出网：{text}");
    assert!(
        server.first_request_line().starts_with("POST"),
        "{}",
        server.first_request_line()
    );
}

#[tokio::test]
async fn cooldown_suppresses_the_same_class_until_the_window_passes() {
    let f = fixture(12).await;
    let server = TinyHttp::spawn("200", "application/json", b"{}".to_vec(), Duration::ZERO);
    f.attach(&server);
    let t0 = f.clock.now();

    note(&f, AttentionKind::TaskDone, t0).await;
    assert!(wait_hits(&server, 1, 3_000).await, "第一条应当放行");

    note(
        &f,
        AttentionKind::TaskDone,
        t0 + chrono::Duration::seconds(5),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        server.hits(),
        1,
        "同类 300 秒 cooldown 内第二条要被挡（前端同一张表）"
    );

    f.clock.advance_secs(301);
    note(
        &f,
        AttentionKind::TaskDone,
        t0 + chrono::Duration::seconds(310),
    )
    .await;
    assert!(
        wait_hits(&server, 2, 3_000).await,
        "出窗口（301 ≥ 300，边界严格小于）后放行"
    );
}

#[tokio::test]
async fn quiet_hours_silence_done_but_pending_and_failed_stay() {
    // 本地 23 点 = 免打扰 [22, 8) 之内。
    let f = fixture(23).await;
    let server = TinyHttp::spawn("200", "application/json", b"{}".to_vec(), Duration::ZERO);
    f.attach(&server);
    let t0 = f.clock.now();

    note(&f, AttentionKind::TaskDone, t0).await; // done：静音
    note(&f, AttentionKind::TaskPending, t0).await; // pending：免打扰豁免
    note(&f, AttentionKind::RunFailed, t0).await; // failed：恒发

    assert!(
        wait_hits(&server, 2, 3_000).await,
        "pending 与 failed 在免打扰时段照发：hits={}",
        server.hits()
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(server.hits(), 2, "done 要被静音，总命中只能是 2");
    let raw = String::from_utf8_lossy(&server.body()).to_string();
    assert!(!raw.contains("\"task_done\""), "done 不该出站：{raw}");
}

/// 决策 284③：出口按**它被造出来时那份**礼貌说话。同一时刻同一类通知，配置级那套
/// （300 / `[22, 8)`）在夜里静音，换成界面单元解析出来的那套（不节流、起止相同 =
/// 全天不静默）就连着放行——「保存即活生效」的判据落在出口自己的行为上。
#[tokio::test]
async fn the_exit_obeys_the_politeness_it_was_built_with() {
    let f = fixture(23).await; // 本地 23 点 = 配置级免打扰 [22, 8) 之内
    let server = TinyHttp::spawn("200", "application/json", b"{}".to_vec(), Duration::ZERO);
    let t0 = f.clock.now();

    // 配置级那一份：夜里 done 静音。
    f.attach(&server);
    note(&f, AttentionKind::TaskDone, t0).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(server.hits(), 0, "配置级免打扰时段里 done 静音");

    // 换成「单元解析出来的那一份」——夜里照发，且同类第二条不被节流挡。
    f.store.set_notifier(Arc::new(WebhookNotifier::new(
        NotifyTarget::Webhook {
            url: server.url("/hook"),
            format: NotifyFormat::Generic,
        },
        politeness(0, [8, 8]),
        f.clock.clone(),
        f.store.clone(),
    )));
    // 三次 `occurred_at` 各不相同：attention 表按 `(task, kind, occurred_at)` 去重
    // （`note_attention` 的 ON CONFLICT DO NOTHING），同一条发生时刻第二次不记账、
    // 也就不出站——这里要的是三件独立的事。
    note(
        &f,
        AttentionKind::TaskDone,
        t0 + chrono::Duration::seconds(1),
    )
    .await;
    assert!(
        wait_hits(&server, 1, 3_000).await,
        "起止相同 = 全天不静默：夜里的 done 应当出站"
    );
    note(
        &f,
        AttentionKind::TaskDone,
        t0 + chrono::Duration::seconds(2),
    )
    .await;
    assert!(
        wait_hits(&server, 2, 3_000).await,
        "节流 0 = 不挡第二条（单元的值真的换上了，不是缺省那份）"
    );
}

#[tokio::test]
async fn slow_run_never_notifies() {
    let f = fixture(12).await;
    let server = TinyHttp::spawn("200", "application/json", b"{}".to_vec(), Duration::ZERO);
    f.attach(&server);
    let t0 = f.clock.now();

    note(&f, AttentionKind::SlowRun, t0).await; // wakes() = false
    note(
        &f,
        AttentionKind::TaskDone,
        t0 + chrono::Duration::seconds(1),
    )
    .await; // 金丝雀
    assert!(
        wait_hits(&server, 1, 3_000).await,
        "金丝雀（TaskDone）应当出站"
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(server.hits(), 1, "SlowRun 不该出站");
    let raw = String::from_utf8_lossy(&server.body()).to_string();
    assert!(raw.contains("\"task_done\""), "{raw}");
}

#[tokio::test]
async fn without_a_notifier_the_accounting_stays_quiet() {
    let f = fixture(12).await;
    // URL 缺席 = 整段关死（268①）：不挂出口，记账照常、无人惊动。
    let inserted = f
        .store
        .note_attention("t1", AttentionKind::RunFailed, f.clock.now(), None)
        .await
        .unwrap();
    assert!(inserted, "没有出口不影响 attention 记账本身");
}

#[test]
fn kind_to_class_mapping_is_pinned() {
    use AttentionKind::*;
    let table: Vec<(AttentionKind, Option<NotifyClass>)> = vec![
        (SlowRun, None), // wakes() 唯一为 false 的那个：类都不给
        (TaskPending, Some(NotifyClass::Pending)),
        (RepeatedPending, Some(NotifyClass::Pending)),
        (OwnerStuck, Some(NotifyClass::Pending)),
        (SchedulerNoEffect, Some(NotifyClass::Pending)),
        (TaskStale, Some(NotifyClass::Pending)),
        (RetryExhausted, Some(NotifyClass::Failed)),
        (ContextOverflow, Some(NotifyClass::Failed)),
        (GateFailure, Some(NotifyClass::Failed)),
        (RunFailed, Some(NotifyClass::Failed)),
        (TaskDone, Some(NotifyClass::Done)),
        (TaskCancelled, Some(NotifyClass::Cancelled)),
        (ResumeBlocked, Some(NotifyClass::Pending)),
        (BlockedRead, Some(NotifyClass::Pending)),
    ];
    for (kind, cls) in table {
        assert_eq!(notification_class(kind), cls, "{kind:?}");
    }
}

// ───────── 决策 272：BlueBubbles 通道与回话线的礼貌语义 ────────

#[tokio::test]
async fn bluebubbles_target_posts_a_send_text_body_with_the_password_in_the_query() {
    let f = fixture(12).await;
    let server = TinyHttp::spawn("200", "application/json", b"{}".to_vec(), Duration::ZERO);
    let notifier = f.attach_target(NotifyTarget::BlueBubbles {
        endpoint: server.url(""), // 端点 = 基地址；路径由代码拼
        password: "s3cret".into(),
        address: "me@icloud.com".into(),
    });

    notifier.notify_foreman_reply("s1", "晚上的重构", "查完了。", f.clock.now());
    assert!(
        wait_hits(&server, 1, 8_000).await,
        "BlueBubbles 通道应当出站"
    );

    // 完整 URL 是代码拼的（272⑦）：路径与 password 在 query 里，TinyHttp 首行可核。
    let line = server.first_request_line();
    assert!(
        line.starts_with("POST /api/v1/message/text?password=s3cret"),
        "{line}"
    );
    let payload: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&server.body())).unwrap();
    assert_eq!(payload["chatGuid"], "iMessage;-;me@icloud.com", "{payload}");
    assert_eq!(payload["method"], "apple-script", "{payload}");
    let message = payload["message"].as_str().unwrap();
    assert!(
        message.starts_with("[AgentPipeline] 晚上的重构 回话"),
        "正文字段是 message（官方文档写 text 是错的）：{message}"
    );
    assert!(message.contains("查完了。"), "{message}");
}

#[tokio::test]
async fn foreman_reply_respects_quiet_hours_while_failure_stays_loud() {
    // 本地 23 点 = 免打扰 [22, 8) 之内。
    let f = fixture(23).await;
    let server = TinyHttp::spawn("200", "application/json", b"{}".to_vec(), Duration::ZERO);
    let notifier = f.attach_target(NotifyTarget::Webhook {
        url: server.url("/hook"),
        format: NotifyFormat::Generic,
    });

    // 回话线：受免打扰、不豁免（272④——回话是摘要不是警报）。
    notifier.notify_foreman_reply("s1", "晚上的重构", "还在查。", f.clock.now());
    // 失败线：类 = failed，恒发（272③）。
    notifier.notify_foreman_failure("s1", "晚上的重构", "llm_network", f.clock.now());
    assert!(
        wait_hits(&server, 1, 8_000).await,
        "免打扰段内只有失败线照发：hits={}",
        server.hits()
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(server.hits(), 1, "回话线被免打扰静音");
    let payload: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&server.body())).unwrap();
    assert_eq!(payload["kind"], "foreman_reply_failed", "{payload}");

    // 拨出免打扰段（本地 8 点）：回话线放行。
    f.clock.advance_secs((9 * 3600) as i64); // 23:00 → 次日 08:00（本地）
    notifier.notify_foreman_reply("s1", "晚上的重构", "查完了。", f.clock.now());
    assert!(wait_hits(&server, 2, 8_000).await, "出免打扰段后回话线放行");
}

#[tokio::test]
async fn foreman_reply_has_its_own_cooldown_slot() {
    let f = fixture(12).await;
    let server = TinyHttp::spawn("200", "application/json", b"{}".to_vec(), Duration::ZERO);
    let notifier = f.attach_target(NotifyTarget::Webhook {
        url: server.url("/hook"),
        format: NotifyFormat::Generic,
    });
    let t0 = f.clock.now();

    notifier.notify_foreman_reply("s1", "晚上的重构", "第一条。", t0);
    assert!(wait_hits(&server, 1, 8_000).await);
    // cooldown 内第二条回话被挡（类内 300s 槽）。
    notifier.notify_foreman_reply(
        "s1",
        "晚上的重构",
        "第二条。",
        t0 + chrono::Duration::seconds(5),
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(server.hits(), 1, "foreman_reply 有自己的 cooldown 槽");

    // **绝不复用 done**（272④）：回话占用槽位后，真正的 task_done 照发。
    note(
        &f,
        AttentionKind::TaskDone,
        t0 + chrono::Duration::seconds(6),
    )
    .await;
    assert!(
        wait_hits(&server, 2, 8_000).await,
        "done 的槽位独立，不能被回话吃掉：hits={}",
        server.hits()
    );
    let payload: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&server.body())).unwrap();
    assert_eq!(payload["kind"], "task_done", "{payload}");
}

// ───────── pwa-webpush 02：浏览器推送（扇出 + 深链 + 礼貌门 + 410）─────────
//
// 判据落在**真的出站了 N 条 HTTP**与**浏览器真的解得开**上：订阅行里的 endpoint 就是
// 远端推送服务的替换点（填 testkit 的 `TinyHttp`，与 webhook URL 指 `TinyHttp` 是同一个
// 姿势——决策 250「URL 是缝不是 trait」的复用，不新增接缝）。
//
// 报文不是黑盒：下面那份解密侧按 RFC 8291 §3 独立写一遍（派生那一步调 core 的
// `derive_content_keys`——它的正确性由 `webpush.rs` 里 RFC 8291 的公开测试向量钉住，
// 这里要验的是「生产那条路上真的用了它、且浏览器解出来就是我推的那份 JSON」）。

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use ring::aead;
use ring::agreement::{agree_ephemeral, EphemeralPrivateKey, UnparsedPublicKey, ECDH_P256};
use ring::rand::SecureRandom;

fn b64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// 造一个「浏览器」：一对 P-256 密钥 + 16 字节鉴权秘密。私钥留在测试里——ring 的
/// `EphemeralPrivateKey` 只能生成、不能从字节导入，这与真浏览器一样（公钥与 auth
/// 出机器，私钥永不出浏览器）。
#[allow(clippy::type_complexity)]
fn fake_user_agent() -> (EphemeralPrivateKey, String, String) {
    let rng = ring::rand::SystemRandom::new();
    let private = EphemeralPrivateKey::generate(&ECDH_P256, &rng).unwrap();
    let public = private.compute_public_key().unwrap();
    let mut auth = [0u8; 16];
    rng.fill(&mut auth).unwrap();
    (private, b64url(public.as_ref()), b64url(&auth))
}

/// 一台「假设备」：它自己的那台推送服务（`TinyHttp`）+ 浏览器侧的三件。
///
/// 私钥是 `Option`：`agree_ephemeral` 吃掉私钥，故**一台设备只解一条报文**——
/// 与真浏览器不同（那边能一直解密），但这正是「私钥不许被复制/导出」的同一个约束。
struct FakeDevice {
    server: TinyHttp,
    private: Option<EphemeralPrivateKey>,
    p256dh: String,
    auth: String,
}

impl FakeDevice {
    /// 造一台并入册：订阅行的 `endpoint` 指向它自己的那台 TinyHttp。
    async fn subscribe(f: &Fixture, status: &str, user_agent: &str) -> FakeDevice {
        let server = TinyHttp::spawn(status, "application/json", b"{}".to_vec(), Duration::ZERO);
        let (private, p256dh, auth) = fake_user_agent();
        f.store
            .upsert_push_subscription(
                &server.url("/push/device"),
                &p256dh,
                &auth,
                Some(user_agent),
            )
            .await
            .unwrap();
        FakeDevice {
            server,
            private: Some(private),
            p256dh,
            auth,
        }
    }

    /// 解出这条设备收到的报文（RFC 8291 §3 的解密侧）。
    fn decrypt(&mut self) -> serde_json::Value {
        let private = self.private.take().expect("一台设备只解一条报文");
        let body = self.server.body();
        assert!(body.len() > 86, "报文体太短：{} 字节", body.len());
        let salt: [u8; 16] = body[0..16].try_into().unwrap();
        assert_eq!(
            u32::from_be_bytes(body[16..20].try_into().unwrap()),
            agentpipeline_core::webpush::RECORD_SIZE,
            "rs 是 4096"
        );
        assert_eq!(body[20] as usize, 65, "idlen 是未压缩点的长度");
        let as_public = &body[21..86];
        let ciphertext = &body[86..];

        let shared = agree_ephemeral(
            private,
            &UnparsedPublicKey::new(&ECDH_P256, as_public),
            |secret| secret.to_vec(),
        )
        .expect("ECDH 应当协商得起来");
        let (cek, nonce) = agentpipeline_core::webpush::derive_content_keys(
            &shared,
            &URL_SAFE_NO_PAD.decode(&self.auth).unwrap(),
            &URL_SAFE_NO_PAD.decode(&self.p256dh).unwrap(),
            as_public,
            &salt,
        )
        .unwrap();
        let key = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_128_GCM, &cek).unwrap());
        let mut record = ciphertext.to_vec();
        let opened = key
            .open_in_place(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::empty(),
                &mut record,
            )
            .expect("浏览器那侧应当解得开");
        assert_eq!(opened[opened.len() - 1], 0x02, "收尾记录的定界符");
        serde_json::from_slice(&opened[..opened.len() - 1]).expect("解出来是那份 JSON")
    }
}

/// 扇出：**每条活订阅各收到一条**（N 条 HTTP，N = 活订阅数），头上带着 VAPID 鉴权与
/// `aes128gcm` 编码，每条都解得开、且解出来的就是「那张卡」的深链。
#[tokio::test]
async fn a_push_notification_fans_out_to_every_live_subscription() {
    let f = fixture(12).await;
    let mut phone = FakeDevice::subscribe(&f, "200 OK", "iPhone Safari").await;
    let mut desktop = FakeDevice::subscribe(&f, "200 OK", "Chrome").await;

    // 通道 = 浏览器推送；VAPID 密钥对由 `ensure` 生成（首次启用自动生成）。
    assert!(
        f.store.push_vapid_keys().await.unwrap().is_none(),
        "还没生成过 VAPID 密钥"
    );
    f.store.ensure_push_vapid_keys().await.unwrap();
    let keys = f
        .store
        .push_vapid_keys()
        .await
        .unwrap()
        .expect("ensure 之后密钥对应当在场");
    f.attach_target(NotifyTarget::WebPush);

    note(&f, AttentionKind::TaskPending, f.clock.now()).await;
    assert!(
        wait_hits(&phone.server, 1, 8_000).await && wait_hits(&desktop.server, 1, 8_000).await,
        "两台设备各该收到一条：phone={} desktop={}",
        phone.server.hits(),
        desktop.server.hits()
    );

    for device in [&mut phone, &mut desktop] {
        // 一条推送请求的形状：POST + VAPID 鉴权头 + aes128gcm 编码 + TTL。
        let head = device.server.request_head().to_lowercase();
        assert!(head.starts_with("post "), "{head}");
        assert!(head.contains("authorization: vapid t="), "{head}");
        assert!(
            head.contains(&keys.public_key.to_lowercase()),
            "k= 是公钥本身：{head}"
        );
        assert!(head.contains("content-encoding: aes128gcm"), "{head}");
        assert!(head.contains("ttl:"), "{head}");
        assert!(
            !head.contains(&keys.private_key.to_lowercase()),
            "私钥不许进头部：{head}"
        );

        let payload = device.decrypt();
        assert!(
            payload["title"].as_str().unwrap().contains("t1"),
            "{payload}"
        );
        assert!(
            payload["body"].as_str().unwrap().contains("task_pending"),
            "正文只带归因白名单（268④）：{payload}"
        );
        assert_eq!(payload["url"], "#/task/t1", "深链落在那张卡上：{payload}");
    }
}

/// 流水线级事件（detail 里有 `run_id`）的深链多带一段 `?run=`：落到**那次运行**。
/// 同一次事件里两台设备解出来的 url 一字不差（深链是服务端拼的，不是各端各拼）。
#[tokio::test]
async fn a_failed_run_pushes_the_run_scoped_deep_link() {
    let f = fixture(12).await;
    let mut phone = FakeDevice::subscribe(&f, "200 OK", "iPhone Safari").await;
    f.store.ensure_push_vapid_keys().await.unwrap();
    f.attach_target(NotifyTarget::WebPush);

    let detail = serde_json::json!({ "run_id": 42, "stage": "develop", "node": "execute" });
    f.store
        .note_attention("t1", AttentionKind::RunFailed, f.clock.now(), Some(&detail))
        .await
        .unwrap();
    assert!(wait_hits(&phone.server, 1, 8_000).await, "应当出站一条");
    let payload = phone.decrypt();
    assert_eq!(payload["url"], "#/task/t1?run=42", "{payload}");
}

/// 值班长那条线的推送落在**那一班**的会话上（`?session=`）。
#[tokio::test]
async fn a_foreman_reply_pushes_the_talk_session_deep_link() {
    let f = fixture(12).await;
    let mut phone = FakeDevice::subscribe(&f, "200 OK", "iPhone Safari").await;
    f.store.ensure_push_vapid_keys().await.unwrap();
    let notifier = f.attach_target(NotifyTarget::WebPush);

    notifier.notify_foreman_reply("s-晚上的重构", "晚上的重构", "查完了。", f.clock.now());
    assert!(wait_hits(&phone.server, 1, 8_000).await, "应当出站一条");
    let payload = phone.decrypt();
    assert_eq!(
        payload["url"], "#/talk?session=s-晚上的重构",
        "点是哪一班就落哪一班：{payload}"
    );
    assert!(
        payload["title"].as_str().unwrap().contains("晚上的重构"),
        "{payload}"
    );
}

/// 礼貌门零新增：**同一道门**。免打扰时段里 `done` 静音而 `pending` / `failed` 照发
/// （268③ 那张表原样），节流窗口内的第二条被挡、出窗口放行——判据与 iMessage 通道
/// 一字不差（票面：将来同批增减事件）。
#[tokio::test]
async fn push_obeys_the_same_politeness_gate_as_the_other_channels() {
    // 本地 23 点 = 免打扰 [22, 8) 之内。
    let f = fixture(23).await;
    let phone = FakeDevice::subscribe(&f, "200 OK", "iPhone Safari").await;
    f.store.ensure_push_vapid_keys().await.unwrap();
    f.attach_target(NotifyTarget::WebPush);
    let t0 = f.clock.now();

    note(&f, AttentionKind::TaskDone, t0).await; // done：静音
    note(&f, AttentionKind::TaskPending, t0).await; // pending：免打扰豁免
    note(&f, AttentionKind::RunFailed, t0).await; // failed：恒发
    assert!(
        wait_hits(&phone.server, 2, 8_000).await,
        "pending 与 failed 在免打扰时段照发：hits={}",
        phone.server.hits()
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(phone.server.hits(), 2, "done 被静音，总命中只能是 2");
    assert!(
        !String::from_utf8_lossy(&phone.server.body()).contains("task_done"),
        "done 不该出站（解不开的密文里也不该出现它的 kind——密文里本来就什么都没有）"
    );

    // 节流：`pending` 的槽已被上面占掉，同一类 300 秒内的第二条不出站；
    // 出窗口（301 ≥ 300，边界严格小于）后放行。
    f.clock.advance_secs(301);
    note(
        &f,
        AttentionKind::TaskPending,
        t0 + chrono::Duration::seconds(310),
    )
    .await;
    assert!(
        wait_hits(&phone.server, 3, 8_000).await,
        "出节流窗口后放行：hits={}",
        phone.server.hits()
    );
}

/// `SlowRun`（`wakes()` 唯一为 false 的那个）在推送这条线上同样一个字节不出站——
/// 金丝雀反证（同一台假设备上，只有金丝雀那条该到）。
#[tokio::test]
async fn slow_run_never_pushes() {
    let f = fixture(12).await;
    let mut phone = FakeDevice::subscribe(&f, "200 OK", "iPhone Safari").await;
    f.store.ensure_push_vapid_keys().await.unwrap();
    f.attach_target(NotifyTarget::WebPush);
    let t0 = f.clock.now();

    note(&f, AttentionKind::SlowRun, t0).await;
    note(
        &f,
        AttentionKind::TaskDone,
        t0 + chrono::Duration::seconds(1),
    )
    .await;
    assert!(wait_hits(&phone.server, 1, 8_000).await, "金丝雀应当到");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(phone.server.hits(), 1, "SlowRun 不该出站");
    assert!(
        phone.decrypt()["title"]
            .as_str()
            .unwrap()
            .contains("task_done"),
        "到的那一条是金丝雀"
    );
}

/// 推送服务说这条订阅死了（404 / 410）→ **删行**（票面：设备清单里永远是活订阅）。
/// 反向一条：5xx（推送服务在抖）**不删行**——一次网络抖动不该清空用户的设备清单。
#[tokio::test]
async fn a_gone_subscription_is_deleted_while_a_transient_failure_is_kept() {
    let f = fixture(12).await;
    let gone = FakeDevice::subscribe(&f, "410 Gone", "换掉的旧手机").await;
    let flaky = FakeDevice::subscribe(&f, "500 Internal Server Error", "在抖的推送服务").await;
    f.store.ensure_push_vapid_keys().await.unwrap();
    f.attach_target(NotifyTarget::WebPush);

    note(&f, AttentionKind::TaskPending, f.clock.now()).await;
    assert!(
        wait_hits(&gone.server, 1, 8_000).await && wait_hits(&flaky.server, 1, 8_000).await,
        "两条订阅都该收到（扇出不看应答）"
    );

    // 删行发生在投递任务的收尾——轮询等它落库。
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let rows = loop {
        let rows = f.store.list_push_subscriptions().await.unwrap();
        if rows.len() <= 1 || std::time::Instant::now() > deadline {
            break rows;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(rows.len(), 1, "410 那条该被删掉：{rows:?}");
    assert!(
        rows[0].user_agent.as_deref() == Some("在抖的推送服务"),
        "留下的是 5xx 那条（transient 不清行）：{rows:?}"
    );

    // 一条订阅都没有时也不炸：出口照常记账，只是没有收件人。
    f.store.clear_push_subscriptions().await.unwrap();
    f.clock.advance_secs(301);
    note(
        &f,
        AttentionKind::TaskPending,
        f.clock.now() + chrono::Duration::seconds(1),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(f.store.list_push_subscriptions().await.unwrap().is_empty());
}

/// 缺 VAPID 密钥对时**一个字节不出站**且不炸（设置页保存通道时会生成；这条守的是
/// 「配置级声明了 webpush 但还没人订阅过」那种状态）。
#[tokio::test]
async fn without_vapid_keys_nothing_is_pushed_and_nothing_panics() {
    let f = fixture(12).await;
    let phone = FakeDevice::subscribe(&f, "200 OK", "iPhone Safari").await;
    f.attach_target(NotifyTarget::WebPush);

    note(&f, AttentionKind::TaskPending, f.clock.now()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(phone.server.hits(), 0, "没有密钥对就不该出站");
    assert_eq!(
        f.store.list_push_subscriptions().await.unwrap().len(),
        1,
        "订阅行照旧留着（它没有错）"
    );
}
