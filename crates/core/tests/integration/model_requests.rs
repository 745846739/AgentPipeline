//! L2 集成：模型请求台账（决策 231）。
//!
//! 这张表是决策 230 的「定位成功」判据（①哪一个 run ②卡在哪一环 ③一条可复核的原始证据
//! ④归因类别）里**前两项的落点**——没有它，一条活栈 / 一个错误串归不了位。故用例打在
//! 三件**外部可观测的行为**上：
//!
//! 1. **归位**：一次请求落在它**真正所属**的 run（或值班长班次）名下，序号在同一归属内自增；
//! 2. **在飞与收场**：`finished_at IS NULL` 就是「此刻在飞」——包括被丢掉的那一支
//!    （墙钟超时 / 中止），它绝不能以「在飞」的样子留下来；
//! 3. **不冒充读数**：没量到的用量 / 字节是 `null`，不是 0（决策 226③ 的同一条纪律）。
//!
//! 替换边界照旧（决策 148）：只替换 LLM 响应流，台账与存储真跑。

use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use tokio::sync::Notify;

use agentpipeline_core::agent::client::{AgentResponse, LlmClient, LlmRequest, RunContext};
use agentpipeline_core::storage::model_requests::{
    ModelRequest, ModelRequestStatus, NewModelRequest,
};
use agentpipeline_core::storage::observability::NewRun;
use agentpipeline_core::storage::Store;
use agentpipeline_core::types::{Node, Stage};
use agentpipeline_core::{Error, Result};
use testkit::{ManualClock, TestHome};

struct Harness {
    _home: TestHome,
    store: Store,
    /// 一条**真** run（外键要求归属指向真实存在的那一行，不能用 7 这种手编数字）。
    run_id: i64,
}

impl Harness {
    async fn new() -> Self {
        let home = TestHome::new().unwrap();
        let clock = ManualClock::fixed();
        let store = Store::open(home.home().clone(), Arc::new(clock.clone()))
            .await
            .unwrap();
        let repo = home.scratch_dir("proj");
        testkit::seed_project(&store, "p1", "示例", &repo, "main")
            .await
            .unwrap();
        testkit::seed_task(&store, "t1", "p1").await.unwrap();
        let cursor_id = store.load_live_cursors("t1").await.unwrap()[0]
            .cursor_id
            .clone();
        let run_id = store
            .insert_run(&NewRun {
                task_id: "t1".into(),
                cursor_id,
                stage: Stage::Test,
                node: Node::Execute,
                attempt: 1,
                agent_type: "main".into(),
                parent_run_id: None,
                prompt_template_hash: None,
                process_group_id: None,
            })
            .await
            .unwrap();
        Harness {
            _home: home,
            store,
            run_id,
        }
    }

    /// 流水线节点那一类请求的上下文。
    fn run_context(&self) -> RunContext {
        RunContext {
            task_id: "t1".into(),
            branch: "main".into(),
            run_id: self.run_id,
            agent_type: "main".into(),
            session_id: String::new(),
        }
    }

    async fn inflight(&self) -> Vec<ModelRequest> {
        self.store.inflight_model_requests(20).await.unwrap()
    }

    /// 等「在飞」的读数长出来（台账写入是异步的，但它发生在模型被叫之前）。
    async fn wait_for_inflight(&self, expected: usize) -> Vec<ModelRequest> {
        for _ in 0..400 {
            let rows = self.inflight().await;
            if rows.len() == expected {
                return rows;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("等不到 {expected} 条在飞请求（超时 2s）");
    }

    /// 等某一行落终态（守卫的收场是 spawned 的，与 Drop 不同步）。
    async fn wait_for_settled(&self, request_id: i64) -> ModelRequest {
        for _ in 0..400 {
            let rows = self
                .store
                .model_requests_for_run(self.run_id, 20)
                .await
                .unwrap();
            if let Some(row) = rows.iter().find(|r| r.id == request_id) {
                if !row.in_flight() {
                    return row.clone();
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("请求 {request_id} 一直没有收场（超时 2s）");
    }
}

fn request(ctx: Option<RunContext>) -> LlmRequest {
    LlmRequest {
        stage: Stage::Test,
        node: Node::Execute,
        attempt: 1,
        system_prompt: "你是测试工位。".into(),
        user_prompt: "跑一遍闸门。".into(),
        messages: Vec::new(),
        tools: Vec::new(),
        temperature: None,
        max_tokens: None,
        provider_id: None,
        run: ctx,
        idle_timeout_sec: None,
    }
}

/// 一个当场返回的替身：可以指定用量与失败方式。
struct StubClient {
    outcome: StubOutcome,
}

enum StubOutcome {
    OkWithReadings,
    Fail,
    Cancelled,
}

impl LlmClient for StubClient {
    fn complete(&self, _request: LlmRequest) -> BoxFuture<'static, Result<AgentResponse>> {
        let outcome = match self.outcome {
            StubOutcome::OkWithReadings => Ok(AgentResponse {
                content: Some("好了".into()),
                prompt_tokens: 1_234,
                completion_tokens: 56,
                cache_read_tokens: 78,
                cache_write_tokens: 9,
                bytes_received: Some(4_096),
                last_byte_at: None,
                ..Default::default()
            }),
            StubOutcome::Fail => Err(Error::Llm("读取流失败：连接被对端关掉".into())),
            StubOutcome::Cancelled => Err(Error::Cancelled("调度器判超时".into())),
        };
        Box::pin(async move { outcome })
    }
}

/// 一个「停在飞」的替身：进到 `enter`、然后等 `release`。
struct BlockingClient {
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

impl LlmClient for BlockingClient {
    fn complete(&self, _request: LlmRequest) -> BoxFuture<'static, Result<AgentResponse>> {
        let entered = self.entered.clone();
        let release = self.release.clone();
        Box::pin(async move {
            entered.notify_one();
            release.notified().await;
            Ok(AgentResponse {
                content: Some("回来了".into()),
                prompt_tokens: 11,
                completion_tokens: 4,
                ..Default::default()
            })
        })
    }
}

#[tokio::test]
async fn a_settled_request_is_recorded_under_its_run_with_its_readings() {
    let h = Harness::new().await;
    let llm = agentpipeline_core::agent::RecordingLlm::new(
        Arc::new(StubClient {
            outcome: StubOutcome::OkWithReadings,
        }),
        h.store.clone(),
    );

    llm.complete(request(Some(h.run_context()))).await.unwrap();

    let rows = h.store.model_requests_for_run(h.run_id, 10).await.unwrap();
    assert_eq!(rows.len(), 1, "一次调用一行");
    let row = &rows[0];
    // 归位：这一行钉在**它真正所属**的 run 上（决策 230 的第一项判据）。
    assert_eq!(row.run_id, Some(h.run_id));
    assert_eq!(row.seq, 1, "序号从 1 起");
    assert_eq!(row.task_id.as_deref(), Some("t1"));
    assert_eq!(row.stage, "test");
    assert_eq!(row.node, "execute");
    assert_eq!(row.agent_type, "main");
    // 收场读数。
    assert_eq!(row.status, ModelRequestStatus::Ok);
    assert!(row.finished_at.is_some(), "收场了就不该再算在飞");
    assert!(!row.in_flight());
    assert_eq!(row.usage.prompt_tokens, Some(1_234));
    assert_eq!(row.usage.completion_tokens, Some(56));
    assert_eq!(row.usage.cache_read_tokens, Some(78));
    assert_eq!(row.usage.cache_write_tokens, Some(9));
    assert_eq!(row.usage.bytes_received, Some(4_096), "量速的分子");
}

#[tokio::test]
async fn requests_are_numbered_within_their_run() {
    let h = Harness::new().await;
    let llm = agentpipeline_core::agent::RecordingLlm::new(
        Arc::new(StubClient {
            outcome: StubOutcome::OkWithReadings,
        }),
        h.store.clone(),
    );

    for _ in 0..3 {
        llm.complete(request(Some(h.run_context()))).await.unwrap();
    }

    let mut seqs: Vec<i64> = h
        .store
        .model_requests_for_run(h.run_id, 10)
        .await
        .unwrap()
        .iter()
        .map(|r| r.seq)
        .collect();
    seqs.sort();
    assert_eq!(seqs, vec![1, 2, 3], "同一 run 内逐次请求按序号排得开");
}

#[tokio::test]
async fn an_in_flight_request_is_readable_before_it_settles() {
    let h = Harness::new().await;
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let llm = Arc::new(agentpipeline_core::agent::RecordingLlm::new(
        Arc::new(BlockingClient {
            entered: entered.clone(),
            release: release.clone(),
        }),
        h.store.clone(),
    ));

    let call = {
        let llm = llm.clone();
        let ctx = h.run_context();
        tokio::spawn(async move { llm.complete(request(Some(ctx))).await })
    };
    entered.notified().await;

    // 「现在在飞什么」——决策 231 定的那个读数。
    let inflight = h.wait_for_inflight(1).await;
    assert_eq!(inflight[0].run_id, Some(h.run_id), "在飞的请求也要归得了位");
    assert_eq!(inflight[0].status, ModelRequestStatus::Running);
    assert!(inflight[0].in_flight());
    assert_eq!(
        inflight[0].usage.prompt_tokens, None,
        "还没收场就没有用量读数"
    );

    release.notify_one();
    call.await.unwrap().unwrap();

    assert!(h.inflight().await.is_empty(), "收场之后不该还在飞");
    let settled = h.store.model_requests_for_run(h.run_id, 10).await.unwrap();
    assert_eq!(settled[0].status, ModelRequestStatus::Ok);
    assert_eq!(settled[0].usage.prompt_tokens, Some(11));
}

#[tokio::test]
async fn a_dropped_request_is_not_left_in_flight() {
    // 这是**最容易被漏掉**的一支：调用方不等它（`respond` 的墙钟超时、执行体在模型调用处的
    // `select!`）时，future 被直接丢掉——如果只有「跟在大 await 后面」的收场代码，那一行会
    // 永远以「在飞」的姿态留在表里。那比「没有读数」更坏：它是个假读数（看起来还在跑）。
    let h = Harness::new().await;
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let llm = Arc::new(agentpipeline_core::agent::RecordingLlm::new(
        Arc::new(BlockingClient {
            entered: entered.clone(),
            release: release.clone(),
        }),
        h.store.clone(),
    ));

    let call = {
        let llm = llm.clone();
        let ctx = h.run_context();
        tokio::spawn(async move { llm.complete(request(Some(ctx))).await })
    };
    entered.notified().await;
    let inflight = h.wait_for_inflight(1).await;
    let request_id = inflight[0].id;

    // 调用方走了（超时 / 中止）：future 被丢掉。
    call.abort();

    let settled = h.wait_for_settled(request_id).await;
    assert_eq!(
        settled.status,
        ModelRequestStatus::Timeout,
        "被丢掉的请求要收成终态，而不是留在飞"
    );
    assert!(!settled.in_flight());
    assert!(
        settled.error.unwrap_or_default().contains("半途"),
        "留痕要说清它为什么没有收场读数"
    );
    assert_eq!(
        settled.usage.bytes_received, None,
        "没量到就是 null，不是 0"
    );
}

#[tokio::test]
async fn a_failed_request_records_no_fake_zero_readings() {
    let h = Harness::new().await;
    let llm = agentpipeline_core::agent::RecordingLlm::new(
        Arc::new(StubClient {
            outcome: StubOutcome::Fail,
        }),
        h.store.clone(),
    );

    let error = llm
        .complete(request(Some(h.run_context())))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("读取流失败"), "原错误照旧往上抛");

    let rows = h.store.model_requests_for_run(h.run_id, 10).await.unwrap();
    assert_eq!(rows[0].status, ModelRequestStatus::Error);
    // 失败时用量**没有读数**——写 0 会把「没量到」说成「一个 token 都没烧」。
    assert_eq!(rows[0].usage.prompt_tokens, None);
    assert_eq!(rows[0].usage.bytes_received, None);
    assert!(rows[0]
        .error
        .as_deref()
        .unwrap_or("")
        .contains("读取流失败"));
    assert!(rows[0].finished_at.is_some());
}

#[tokio::test]
async fn a_cancelled_request_is_distinguishable_from_a_failure() {
    let h = Harness::new().await;
    let llm = agentpipeline_core::agent::RecordingLlm::new(
        Arc::new(StubClient {
            outcome: StubOutcome::Cancelled,
        }),
        h.store.clone(),
    );

    let error = llm
        .complete(request(Some(h.run_context())))
        .await
        .unwrap_err();
    assert!(
        error.is_cancelled(),
        "中止照旧按中止往上抛（决策 226 的分流靠它）"
    );

    let rows = h.store.model_requests_for_run(h.run_id, 10).await.unwrap();
    assert_eq!(
        rows[0].status,
        ModelRequestStatus::Cancelled,
        "中止不是「这次调用失败了」——两者要分得开"
    );
}

#[tokio::test]
async fn the_foreman_request_lands_under_its_session_not_a_placeholder_stage() {
    // 值班长借 `Stage::Init` 当占位只为让 provider 解析链跑通（决策 182②），照抄「init」
    // 会让读的人去 init 阶段找这次调用；它在 `stage_configs` 里**有自己的行**（决策 239），
    // 而且没有 run 行（决策 182⑨）——归属只能靠班次。
    let h = Harness::new().await;
    let session = h.store.create_foreman_session("").await.unwrap().id;
    let llm = agentpipeline_core::agent::RecordingLlm::new(
        Arc::new(StubClient {
            outcome: StubOutcome::OkWithReadings,
        }),
        h.store.clone(),
    );
    let mut req = request(Some(RunContext {
        task_id: String::new(),
        branch: String::new(),
        run_id: 0,
        agent_type: "foreman".into(),
        session_id: session.clone(),
    }));
    req.stage = Stage::Init;

    llm.complete(req).await.unwrap();

    let rows = h
        .store
        .model_requests_for_session(&session, 10)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].run_id, None,
        "哨兵 run_id = 0 要归一成 NULL（0 会撞外键）"
    );
    assert_eq!(rows[0].session_id.as_deref(), Some(session.as_str()));
    assert_eq!(rows[0].stage, "foreman", "落它自己的阶段键，不落占位值");
    assert_eq!(rows[0].agent_type, "foreman");
    assert_eq!(rows[0].task_id, None, "空串不是身份");
}

#[tokio::test]
async fn a_project_analysis_request_without_any_owner_still_lands() {
    // 项目分析那一次调用 `RunContext { run_id: 0, task_id: "" }`、也没有班次——两样归属都
    // 没有，但它仍旧落账：没有归属不等于没有发生。这条也钉住「归一成 NULL 之后外键放行」
    // （真写 0 会被外键拒掉，而拒掉时这条留痕只会打一条 warn 悄悄消失）。
    let h = Harness::new().await;
    let llm = agentpipeline_core::agent::RecordingLlm::new(
        Arc::new(StubClient {
            outcome: StubOutcome::OkWithReadings,
        }),
        h.store.clone(),
    );

    llm.complete(request(Some(RunContext {
        task_id: String::new(),
        branch: String::new(),
        run_id: 0,
        agent_type: "pseudo:project_analysis".into(),
        session_id: String::new(),
    })))
    .await
    .unwrap();

    // 没有归属的请求经 `Store` 的读法读不到（那些读法都以归属为键），而「它到底落没落账」
    // 正是要钉的——故直读原始表（与迁移用例同一手法）。
    let (run_id, session_id, status, agent_type) = raw_request_row(&h._home.home().db_path()).await;
    assert_eq!(run_id, None, "哨兵 0 归一成 NULL");
    assert_eq!(session_id, None, "空串不是身份");
    assert_eq!(status, "ok");
    assert_eq!(agent_type, "pseudo:project_analysis");
}

/// 直读一条模型请求行（绕开 `Store`：没有归属的那些读不到）。
async fn raw_request_row(path: &std::path::Path) -> (Option<i64>, Option<String>, String, String) {
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    let row: (Option<i64>, Option<String>, String, String) =
        sqlx::query_as("SELECT run_id, session_id, status, agent_type FROM kanban_model_requests")
            .fetch_one(&pool)
            .await
            .unwrap();
    pool.close().await;
    row
}

#[tokio::test]
async fn restarting_settles_the_requests_the_previous_process_left_behind() {
    // `finished_at IS NULL` 是这张表唯一的「还在跑」读数：进程被强杀时收场写入不会发生，
    // 不收口的话一个**死掉的**请求会永远以「在飞」的样子出现在诊断包里（决策 226③ 的
    // 同一类失真，方向相反）。启动时那一扫就是这条用例钉的东西。
    let h = Harness::new().await;
    h.store
        .begin_model_request(&NewModelRequest {
            run_id: Some(h.run_id),
            session_id: None,
            task_id: Some("t1".into()),
            agent_type: "main".into(),
            stage: "test".into(),
            node: "execute".into(),
            attempt: 1,
        })
        .await
        .unwrap();
    assert_eq!(h.inflight().await.len(), 1);

    let settled = h
        .store
        .orphan_inflight_model_requests("上一个进程退出时这次请求还没有收场")
        .await
        .unwrap();
    assert_eq!(settled, 1, "遗留的在飞请求要被收成终态");

    let rows = h.store.model_requests_for_run(h.run_id, 10).await.unwrap();
    assert_eq!(rows[0].status, ModelRequestStatus::Timeout);
    assert!(rows[0].finished_at.is_some());
    assert!(rows[0]
        .error
        .as_deref()
        .unwrap_or("")
        .contains("上一个进程退出"));
    // 幂等：再扫一次没有东西可收。
    assert_eq!(
        h.store
            .orphan_inflight_model_requests("再来一次")
            .await
            .unwrap(),
        0
    );
}
