//! 模型请求留痕（决策 231）：每一次 `complete` 都落进 `kanban_model_requests`。
//!
//! **为什么是一个装饰器，而不是在五处调用点各写一遍**：模型出口在本仓有五条路径——节点
//! 工具循环 / 伪阶段 / 项目分析 / 子代理 / 值班长回话，各自 `complete` 一次（有的还有循环）。
//! 「广告集与执行点同源」是票 01 的硬要求（`pipeline/foreman.rs`），这里同一条道理：
//! 留痕必须**覆盖所有出口**，而每处各写一份就多一处会漂移、会漏的名单。故它只包在构造处
//! （`Executor::new` / `ForemanRunner::new`），调用点一个字不动。
//!
//! **三行日志**里的第二、三行（请求派发 / 请求收场）也在这里——它们与这张表是同一件事的
//! 两个面（决策 231）：表回答「现在在飞什么」，日志回答「那一刻按时间顺序发生了什么」
//! （09-18 那种「进程早就没了、只剩日志可查」的现场只有日志能覆盖）。第一行（节点开始）
//! 在 `Store::insert_run`——那才是「一个节点开始跑」的唯一漏斗，子代理与伪阶段都过它。
//!
//! **留痕失败不挂关键路径**：与 `Store::set_run_step` / `touch_run_heartbeat` 同一姿态——
//! 一条写不进去的观测行不该让整条流水线失败。故两次写入都是 best-effort + `warn`。

use std::sync::Arc;

use futures::future::BoxFuture;

use crate::agent::client::{AgentResponse, LlmClient, LlmRequest};
use crate::storage::model_requests::{ModelRequestStatus, ModelRequestUsage, NewModelRequest};
use crate::storage::Store;
use crate::{Error, Result};

/// 调用方在半途丢掉这次请求时留下的原因（墙钟超时 / 执行体收口）。
const ABANDONED_NOTE: &str = "这次调用在半途被丢掉（超时或中止），没有收到收场读数";

/// 包住任意 [`LlmClient`]，把每次调用落成一行请求台账。
pub struct RecordingLlm {
    inner: Arc<dyn LlmClient>,
    store: Store,
}

impl RecordingLlm {
    pub fn new(inner: Arc<dyn LlmClient>, store: Store) -> Self {
        Self { inner, store }
    }

    /// 请求 → 一行的归属与身份。
    ///
    /// 三条归一化，各对应一处真实的形状：
    /// * `run_id = 0` 是「没有 run 行」的哨兵，归一成 NULL——0 会撞外键。今天只有值班长
    ///   真没有 run 行（决策 182⑨）；**项目分析有**——决策 100 / 迁移 0004 给了它一行真台账，
    ///   调用方把 id 透进 `RunContext`（决策 329；此前这里填死 0，那几行请求因此永远无归属）。
    /// * 空串不是身份：`task_id` / `session_id` / `agent_type` 为空时一律当「没有」。
    /// * **值班长的阶段名落它自己的键**：它的请求借 `Stage::Init` 当占位（只为让 provider
    ///   解析链跑通，决策 182②），照抄「init」会让读的人去 init 那个阶段找这次调用；而值班长
    ///   在 `stage_configs` 里**有自己的行**（决策 239），落真名才对得上。
    fn describe(request: &LlmRequest) -> NewModelRequest {
        let run = request.run.as_ref();
        let agent_type = run
            .map(|r| r.agent_type.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "main".to_string());
        let stage = if agent_type == crate::pipeline::foreman::FOREMAN_AGENT_TYPE {
            crate::pipeline::foreman::FOREMAN_STAGE_KEY.to_string()
        } else {
            request.stage.as_str().to_string()
        };
        NewModelRequest {
            run_id: run.map(|r| r.run_id).filter(|id| *id != 0),
            session_id: run.map(|r| r.session_id.clone()).filter(|s| !s.is_empty()),
            task_id: run.map(|r| r.task_id.clone()).filter(|s| !s.is_empty()),
            agent_type,
            stage,
            node: request.node.as_str().to_string(),
            attempt: request.attempt,
        }
    }

    fn usage_of(response: &AgentResponse) -> ModelRequestUsage {
        ModelRequestUsage {
            prompt_tokens: Some(response.prompt_tokens),
            completion_tokens: Some(response.completion_tokens),
            cache_read_tokens: Some(response.cache_read_tokens),
            cache_write_tokens: Some(response.cache_write_tokens),
            bytes_received: response.bytes_received,
            last_byte_at: response.last_byte_at,
        }
    }
}

impl LlmClient for RecordingLlm {
    fn complete(&self, request: LlmRequest) -> BoxFuture<'static, Result<AgentResponse>> {
        let inner = self.inner.clone();
        let store = self.store.clone();
        Box::pin(async move {
            let described = Self::describe(&request);
            let request_id = match store.begin_model_request(&described).await {
                Ok(id) => Some(id),
                Err(e) => {
                    tracing::warn!(error = %e, "模型请求台账落行失败（不阻塞这次调用）");
                    None
                }
            };
            tracing::info!(
                request = request_id,
                run = described.run_id,
                session = described.session_id.as_deref(),
                agent_type = %described.agent_type,
                stage = %described.stage,
                node = %described.node,
                attempt = described.attempt,
                "模型请求派发"
            );

            // future 被丢掉（超时 / 中止）时由它兜底收场——见 `SettleInterrupted`。
            let mut settle = Settle {
                store: store.clone(),
                request_id,
                settled: false,
            };
            let outcome = inner.complete(request).await;
            match &outcome {
                Ok(response) => {
                    settle
                        .settle(ModelRequestStatus::Ok, Self::usage_of(response), None)
                        .await;
                }
                Err(error) => {
                    let status = request_status(error);
                    // 失败时用量留 NULL 而不是 0：流半途断掉时 usage 事件根本没到过，
                    // 0 会把「没有读数」说成「一个 token 都没烧」（决策 226③ 的同一件事）。
                    settle
                        .settle(
                            status,
                            ModelRequestUsage::default(),
                            Some(error.to_string()),
                        )
                        .await;
                }
            }
            tracing::info!(
                request = request_id,
                run = described.run_id,
                stage = %described.stage,
                node = %described.node,
                ok = outcome.is_ok(),
                "模型请求收场"
            );
            outcome
        })
    }
}

/// 错误 → 收场状态。中止与失败必须分得开（决策 226：中止不是「这个节点失败了」）。
fn request_status(error: &Error) -> ModelRequestStatus {
    if error.is_cancelled() {
        ModelRequestStatus::Cancelled
    } else {
        ModelRequestStatus::Error
    }
}

/// 收场的兜底者。
///
/// 正常路径显式 [`Settle::settle`]；**future 被丢掉**时（`respond` 的墙钟超时、执行体在
/// 模型调用处的 `select!`）由 `Drop` 补一次收场。为什么必须有：`finished_at IS NULL` 是这张表
/// 唯一的「在飞」读数，而在飞的行没人收场时会永远在飞——那比「没有读数」更坏，因为它是个
/// **假读数**（看起来还在跑，其实调用方早走了）。
///
/// `Drop` 里不能 `.await`，故收场写进一个 spawned task（没有运行时句柄时跳过：那是纯同步
/// 场景，没人 await 过这次调用，也就没有「在飞」这回事）。
struct Settle {
    store: Store,
    request_id: Option<i64>,
    settled: bool,
}

impl Settle {
    async fn settle(
        &mut self,
        status: ModelRequestStatus,
        usage: ModelRequestUsage,
        error: Option<String>,
    ) {
        if let Some(id) = self.request_id {
            if let Err(e) = self
                .store
                .finish_model_request(id, status, &usage, error.as_deref())
                .await
            {
                tracing::warn!(request = id, error = %e, "模型请求台账收场失败");
            }
        }
        self.settled = true;
    }
}

impl Drop for Settle {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        let Some(id) = self.request_id else { return };
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let store = self.store.clone();
        handle.spawn(async move {
            if let Err(e) = store
                .finish_model_request(
                    id,
                    ModelRequestStatus::Timeout,
                    &ModelRequestUsage::default(),
                    Some(ABANDONED_NOTE),
                )
                .await
            {
                tracing::warn!(request = id, error = %e, "被丢掉的模型请求收场失败");
            }
        });
    }
}
