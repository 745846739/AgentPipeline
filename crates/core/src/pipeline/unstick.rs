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
//! 判据认**三类**（「调度器处置未生效」、owner 持有超时（活 run）、owner 持有超时
//! （无活 run）——后两类是同一条判据的两个分支，决策 305），**不对正常在跑的任务生效**——
//! 否则它会变成「随便踢一脚」：清掉一个健康任务的占用，等于让同一任务跑出两个执行体。

use chrono::{DateTime, Utc};

use crate::storage::attention::AttentionKind;
use crate::storage::Store;
use crate::types::{CursorStatus, NodeStatus, PendingContext, PendingKind, PendingReason, Task};
use crate::{Error, Result};

use super::advance::{advance, Landing};

/// 一次「卡住」的证据（[`stuck_evidence`] 的产出）。
///
/// 待办表（票 05）与 `unstick`（票 09）共用它：**判据只有一处实现**，否则「报出来的卡住」
/// 与「解得开的卡住」会是两个集合——那正是「它说卡了、我却解不开」的来路。
#[derive(Debug, Clone, PartialEq)]
pub struct StuckEvidence {
    pub kind: AttentionKind,
    pub occurred_at: DateTime<Utc>,
    pub cursor_id: String,
    /// 那条僵死的 run。`owner_stuck`（活 run）时它仍是 `running`；`scheduler_no_effect`
    /// 时已终态；**尚无 run**（claim 了却还没建出 run）第三类时是 `None`（决策 305）。
    pub run_id: Option<i64>,
    /// 有没有一条**还活着**的 run（同上，`None` 那条 run 时为 false）。
    pub run_is_live: bool,
    pub detail: serde_json::Value,
}

/// 任务卡在 `running` 上的三类证据（票 05 的 ⑤⑥，票 09 复用，**决策 305 补第三类**）。
///
/// 判据**只有这一处实现**：报出来的卡住与解得开的卡住必须是同一个集合，否则「它说卡了、
/// 我却解不开」迟早发生。
///
/// `stuck` 是宽限：低于它的都是「还在正常重试的时间范围」，报出来只会是噪声。
///
/// 三类：
/// 1. **调度器处置未生效**——游标仍 `active` 而它那条 run 已终态且过了宽限；
/// 2. **owner 持有超时（活 run）**——有主、run 仍 `Running`，但心跳停了；
/// 3. **owner 持有超时（无活 run）**——有主，而 run 已终态**或压根还没有 run**，且执行权
///    已被攥着超过宽限。**决策 305 补的就是这一类**：2026-09-27 实测的形态是「游标
///    `pending` + run 已终态 + 执行权仍持有」，旧判据两条都不覆盖——第一条被「游标必须
///    可运行」先行挡掉，第二条要求 run 仍 `Running`，于是兜底机制在它本该兜的那一格失效，
///    只能靠重启（实测任务 `01M3BGVCXDWFPT0Q3BZYAGZP8Q` 就此僵死）。
///
/// 2 与 3 是**同一条判据的两个分支**（判据 = 「有主且持有超阈值」；有无活 run 只决定
/// [`StuckEvidence::run_is_live`] 与 `run_id` 填什么），不拆成两条——拆了就是「报出来的卡住」
/// 与「解得开的卡住」变成两个集合的开始。
pub async fn stuck_evidence(
    store: &Store,
    task: &Task,
    now: DateTime<Utc>,
    stuck: chrono::Duration,
) -> Result<Option<StuckEvidence>> {
    let owned = task
        .executor_owner
        .as_deref()
        .is_some_and(|o| !o.is_empty());
    // 「执行权攥了多久」的读法：持有权的**换手**都会碰 `updated_at`
    // （`try_claim_executor` 刚写下它、`release_executor` 又清它），所以它是持有起点的
    // 上界估计——别的写库把 `updated_at` 推后时，我们只会**晚**一点报出来，不会误报。
    // 方向是对的：第三类的代价是「晚报」，不是「冤枉健康的任务」。
    let held_since = task.updated_at;

    for cursor in store.load_live_cursors(&task.id).await? {
        // 只认 `active` 与 `pending`：前者是「本来该在动的」，后者是第三类要补的那一格。
        // `waiting_join` 已到 join 边界、没有节点要跑，`archived` 已被读口滤掉。
        if !matches!(cursor.status, CursorStatus::Active | CursorStatus::Pending) {
            continue;
        }
        let run = store
            .list_runs_at(&task.id, cursor.stage, cursor.node)
            .await?
            .pop();

        // ── 第一类：游标仍 active，而它那条 run 已终态且过了宽限 → 调度器处置没生效。
        if cursor.is_runnable() {
            if let Some(run) = run.as_ref() {
                if run.status != NodeStatus::Running {
                    if let Some(finished) = run.finished_at {
                        if now - finished > stuck {
                            return Ok(Some(StuckEvidence {
                                kind: AttentionKind::SchedulerNoEffect,
                                occurred_at: finished,
                                cursor_id: cursor.cursor_id.clone(),
                                run_id: Some(run.id),
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
                }
            }
        }

        // ── 第二、三类：**执行权持有超过阈值**（决策 305）。同一判据、两个分支。
        //    前提是**有主**——没主的不该报这一类，那种情况归准入。
        if !owned {
            continue;
        }
        let (run_id, run_is_live, since, heartbeat_seconds_ago, shape) = match run.as_ref() {
            // 分支 a：还有一条活 run —— 心跳停了才算（心跳在走 = 正常在跑，不许误判）。
            Some(r) if r.status == NodeStatus::Running => {
                let last = r.last_activity_at.unwrap_or(r.started_at);
                (
                    Some(r.id),
                    true,
                    last,
                    (now - last).num_seconds(),
                    "live_run",
                )
            }
            // 分支 b：run 已终态 —— 判据落在「这条 run 结束之后执行权还攥了多久」上。
            Some(r) => (
                Some(r.id),
                false,
                r.finished_at.unwrap_or(held_since),
                -1,
                "terminal_run",
            ),
            // 分支 c：尚无 run（claim 了却还没建出 run）。
            None => (None, false, held_since, -1, "no_run"),
        };
        if now - since <= stuck {
            continue;
        }
        return Ok(Some(StuckEvidence {
            kind: AttentionKind::OwnerStuck,
            occurred_at: since,
            cursor_id: cursor.cursor_id.clone(),
            run_id,
            run_is_live,
            detail: serde_json::json!({
                "run_id": run_id,
                "shape": shape,
                "stage": cursor.stage.as_str(),
                "node": cursor.node.as_str(),
                "cursor_status": cursor.status.as_str(),
                "owner": task.executor_owner,
                "heartbeat_seconds_ago": heartbeat_seconds_ago,
                "held_seconds_ago": (now - since).num_seconds(),
                "error": run.as_ref().and_then(|r| r.error.clone()),
            }),
        }));
    }
    Ok(None)
}

/// 解开一次卡死的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct Unstuck {
    pub cursor_id: String,
    /// 被标终态的僵死 run（`owner_stuck` 的**活 run** 那一条；`scheduler_no_effect` 与第三类
    /// 「尚无 run」时为空——它们没有要标终态的 run）。
    pub finished_runs: Vec<i64>,
    /// 那一条 run（尚无 run 的第三类形态为 `None`，决策 305）。
    pub run_id: Option<i64>,
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
///
/// **三步对三类证据都成立**（决策 305）：第三类（无活 run / 尚无 run）在第 2 步无事可做
/// （没有要标终态的 run），但第 1、3 步正是它需要的那两下——不做第 1 步，那个攥着执行权的
/// 卡死执行体照样把重试逐个拒掉。
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
                "任务 {task_id} 没有卡住：没有「run 已终态而游标仍 active」、「有主但心跳停了」\
                 或「执行权被攥着超过宽限」的证据。unstick 只解这三类，不对正常在跑的任务生效"
            ))
        })?;

    force_release(task_id);

    let mut finished_runs = Vec::new();
    if evidence.run_is_live {
        let run_id = evidence
            .run_id
            .expect("活 run 的证据必带 run_id（stuck_evidence 两个分支都填）");
        store
            .finish_run(
                run_id,
                &crate::storage::observability::RunOutcome {
                    status: Some(NodeStatus::Timeout),
                    error: Some(format!(
                        "unstick：有主但心跳停了 {}s，判定僵死并标终态",
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
        finished_runs.push(run_id);
    }

    store.release_executor(task_id).await?;

    // 游标转 pending：原因是**人话**，且带上证据里的关键事实（哪个 run、停了多久）。
    // 落库走 `advance` 那扇门（决策 245），同步投影在门外。
    //
    // **这里不发 SSE，与 resume 那条的处置不同**：`unstick` 的调用方是值班长的动作面
    // （决策 210⑧），事件面怎么设计是另一件事，本票不扩大范围——resume 补 SSE 是因为
    // 「人按了按钮却毫无反应」，而这条路本来就只有值班长在走。
    let cursor = store.get_cursor(&evidence.cursor_id).await?;
    let reason = PendingReason::new(
        PendingKind::UserDecision,
        cursor.stage,
        cursor.node,
        match (
            evidence.kind,
            evidence.detail.get("shape").and_then(|v| v.as_str()),
        ) {
            (AttentionKind::OwnerStuck, Some("live_run")) => format!(
                "已解除僵死占用（unstick）：run {} 有主但心跳停了，已标终态并清空执行者。\
                 现在可以 resume 继续",
                evidence.run_id.unwrap_or_default()
            ),
            // 决策 305 的第三类：run 已终态或压根没有 run，但执行权被攥着。
            (AttentionKind::OwnerStuck, shape) => format!(
                "已解除僵死占用（unstick）：执行权被攥着超过宽限（{}），已清空执行者。\
                 现在可以 resume 继续",
                match shape {
                    Some("terminal_run") => "run 已终态而游标没在动",
                    Some("no_run") => "已抢占执行权但还没建出 run",
                    _ => "长时间没有活动",
                }
            ),
            _ => format!(
                "已解除僵死占用（unstick）：run {} 早已终态而游标仍是 active（调度器处置没生效），\
                 已清空执行者。现在可以 resume 继续",
                evidence.run_id.unwrap_or_default()
            ),
        },
    )
    .with_context(PendingContext::with_kind(UNSTICK_CONTEXT_KIND));
    advance(
        store,
        task_id,
        &cursor,
        Landing::Pause { reason },
        // 挂起不写流转行，这个 trigger 不会被读到（门的签名对各落点是同一个）。
        crate::types::TransitionTrigger::AutoResume,
        None,
    )
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
