//! `unstick`：把卡死的任务解开（决策 210⑧ / 票 09）。
//!
//! **`unstick` 与 `resume` 是两回事。** `resume` 是「人拍过板了，继续走」；`unstick` 是
//! 「这条已经死了但占着位置」——清 `executor_owner`、把僵死的 run 标终态、游标转 `pending`。
//!
//! 为什么必须有它（2026-09-17 实证）：`支持rtk` 的 run 被标 `timeout` 后，超时处理也写了
//! 「干净对话重试」的 transition，但**重试从未发生**——原始 `try_run` 没返回，进程内去重仍
//! 持有那个 task_id，重试被逐次拒掉。**这种情况下 `resume` 是空操作，还会白吃决策 210⑨
//! 的次数配额。**
//!
//! 判据只认两类（「调度器处置未生效」与「owner 持有超时」），**不对正常在跑的任务生效**——
//! 否则它会变成「随便踢一脚」：清掉一个健康任务的占用，等于让同一任务跑出两个执行体。

use chrono::{DateTime, Utc};

use crate::storage::attention::AttentionKind;
use crate::storage::Store;
use crate::types::{NodeStatus, PendingContext, PendingKind, PendingReason, Task};
use crate::{Error, Result};

/// 一次「卡住」的证据（[`stuck_evidence`] 的产出）。
///
/// 待办表（票 05）与 `unstick`（票 09）共用它：**判据只有一处实现**，否则「报出来的卡住」
/// 与「解得开的卡住」会是两个集合——那正是「它说卡了、我却解不开」的来路。
#[derive(Debug, Clone, PartialEq)]
pub struct StuckEvidence {
    pub kind: AttentionKind,
    pub occurred_at: DateTime<Utc>,
    pub cursor_id: String,
    /// 那条僵死的 run（`owner_stuck` 时它仍是 `running`；`scheduler_no_effect` 时已终态）。
    pub run_id: i64,
    /// `owner_stuck` 时 run 仍是 running。
    pub run_is_live: bool,
    pub detail: serde_json::Value,
}

/// 任务卡在 `running` 上的两类证据（票 05 的 ⑤⑥，票 09 复用）。
///
/// `stuck` 是宽限：低于它的都是「还在正常重试的时间范围」，报出来只会是噪声。
pub async fn stuck_evidence(
    store: &Store,
    task: &Task,
    now: DateTime<Utc>,
    stuck: chrono::Duration,
) -> Result<Option<StuckEvidence>> {
    for cursor in store.load_live_cursors(&task.id).await? {
        if !cursor.is_runnable() {
            continue;
        }
        let Some(run) = store
            .list_runs_at(&task.id, cursor.stage, cursor.node)
            .await?
            .pop()
        else {
            continue;
        };
        if run.status == NodeStatus::Running {
            // owner 持有超时：**有主**，但心跳停了（没有主的不该报这一类——那种情况归准入）
            let last = run.last_activity_at.unwrap_or(run.started_at);
            let owned = task
                .executor_owner
                .as_deref()
                .is_some_and(|o| !o.is_empty());
            if owned && now - last > stuck {
                return Ok(Some(StuckEvidence {
                    kind: AttentionKind::OwnerStuck,
                    occurred_at: last,
                    cursor_id: cursor.cursor_id.clone(),
                    run_id: run.id,
                    run_is_live: true,
                    detail: serde_json::json!({
                        "run_id": run.id,
                        "stage": run.stage.as_str(),
                        "node": run.node.as_str(),
                        "owner": task.executor_owner,
                        "heartbeat_seconds_ago": (now - last).num_seconds(),
                    }),
                }));
            }
            continue;
        }
        // 游标 active 而 run 已终态（且已过宽限期）：**处置没生效**
        let Some(finished) = run.finished_at else {
            continue;
        };
        if now - finished > stuck {
            return Ok(Some(StuckEvidence {
                kind: AttentionKind::SchedulerNoEffect,
                occurred_at: finished,
                cursor_id: cursor.cursor_id.clone(),
                run_id: run.id,
                run_is_live: false,
                detail: serde_json::json!({
                    "run_id": run.id,
                    "run_status": run.status.as_str(),
                    "stage": run.stage.as_str(),
                    "node": run.node.as_str(),
                    "attempt": run.attempt,
                    "cursor_id": cursor.cursor_id,
                    "error": run.error,
                }),
            }));
        }
    }
    Ok(None)
}

/// 解开一次卡死的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct Unstuck {
    pub cursor_id: String,
    /// 被标终态的僵死 run（`owner_stuck` 那一条；`scheduler_no_effect` 时为空——它已经终态了）。
    pub finished_runs: Vec<i64>,
    pub run_id: i64,
    pub kind: AttentionKind,
}

/// 解开卡死的任务（决策 210⑧ / 票 09）。
///
/// 三步，顺序有意义：
/// 1. **摘进程内去重**（调用方给的 `force_release`）——不做这一步，清了 DB 也没用：
///    重试会被去重逐次拒掉（这正是实证里「重试从未发生」的机制）。
/// 2. 僵死的 run 标终态（仍 `running` 的那个）——让「它还在跑」这句话从台账里消失。
/// 3. 清 `executor_owner` + 游标转 `pending`（带可读原因）——任务回到「等人拍板」，
///    于是 `resume` 这一次能真的生效。
pub async fn unstick(
    store: &Store,
    // `+ Sync`：这个 future 要在 axum 的 handler 里被 Send（`&dyn Fn` 本身不保证 Sync）
    force_release: &(dyn Fn(&str) -> bool + Sync),
    task_id: &str,
    now: DateTime<Utc>,
    stuck: chrono::Duration,
) -> Result<Unstuck> {
    let task = store.get_task(task_id).await?;
    let evidence = stuck_evidence(store, &task, now, stuck)
        .await?
        .ok_or_else(|| {
            Error::Validation(format!(
                "任务 {task_id} 没有卡住：没有「run 已终态而游标仍 active」或「有主但心跳停了」的证据。\
                 unstick 只解这两种，不对正常在跑的任务生效"
            ))
        })?;

    force_release(task_id);

    let mut finished_runs = Vec::new();
    if evidence.run_is_live {
        store
            .finish_run(
                evidence.run_id,
                &crate::storage::observability::RunOutcome {
                    status: Some(NodeStatus::Timeout),
                    error: Some(format!(
                        "unstick：有主但心跳停了 {}s，判定僵死并标终态（决策 210⑧）",
                        evidence
                            .detail
                            .get("heartbeat_seconds_ago")
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0)
                    )),
                    ..Default::default()
                },
            )
            .await?;
        finished_runs.push(evidence.run_id);
    }

    store.release_executor(task_id).await?;

    // 游标转 pending：原因是**人话**，且带上证据里的关键事实（哪个 run、停了多久）。
    let cursor = store.get_cursor(&evidence.cursor_id).await?;
    let reason = PendingReason::new(
        PendingKind::UserDecision,
        cursor.stage,
        cursor.node,
        match evidence.kind {
            AttentionKind::OwnerStuck => format!(
                "已解除僵死占用（unstick）：run {} 有主但心跳停了，已标终态并清空执行者。\
                 现在可以 resume 继续",
                evidence.run_id
            ),
            _ => format!(
                "已解除僵死占用（unstick）：run {} 早已终态而游标仍是 active（调度器处置没生效），\
                 已清空执行者。现在可以 resume 继续",
                evidence.run_id
            ),
        },
    )
    .with_context(PendingContext::with_kind(UNSTICK_CONTEXT_KIND));
    store
        .set_cursor_pending(&evidence.cursor_id, &reason)
        .await?;
    store.sync_task_projection(task_id).await?;

    Ok(Unstuck {
        cursor_id: evidence.cursor_id,
        finished_runs,
        run_id: evidence.run_id,
        kind: evidence.kind,
    })
}

/// `unstick` 挂在 pending 上的 context kind（与 `actions::kinds` 的既有键同一族）。
pub const UNSTICK_CONTEXT_KIND: &str = "unstick";

/// 值班长那一侧的形状判据：`task` + `unstick`（决策 210⑧：进托管可自动集）。
pub fn is_unstick_action(name: &str, args: &serde_json::Value) -> bool {
    name == "task" && args.get("action").and_then(|v| v.as_str()) == Some("unstick")
}
