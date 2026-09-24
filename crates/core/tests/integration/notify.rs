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
use agentpipeline_core::notify::{notification_class, NotifyClass, NotifyFormat, WebhookNotifier};
use agentpipeline_core::storage::attention::AttentionKind;
use agentpipeline_core::storage::Store;
use chrono::{DateTime, Local, TimeZone, Utc};
use testkit::{ManualClock, TestHome};

use crate::web_fetch::TinyHttp;

/// 「本地恰为 `hour` 点整」的瞬间（跨零点 / DST 由 chrono 收口；测试只要判据确定）。
fn at_local_hour(hour: u32) -> DateTime<Utc> {
    let today = Local::now().date_naive();
    let naive = today.and_hms_opt(hour, 0, 0).unwrap();
    Local
        .from_local_datetime(&naive)
        .single()
        .or_else(|| Local.from_local_datetime(&naive).earliest())
        .expect("本地整点应当可构造")
        .with_timezone(&Utc)
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
            server.url("/hook"),
            300,
            [22, 8],
            format,
            self.clock.clone(),
        )));
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
    ];
    for (kind, cls) in table {
        assert_eq!(notification_class(kind), cls, "{kind:?}");
    }
}
