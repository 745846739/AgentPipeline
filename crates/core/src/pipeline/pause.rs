//! 手动暂停 / 续跑 / 重跑本阶段（决策 276）：**非 pending 任务**的那三颗手动钮。
//!
//! # 为什么这三件东西住在一起
//!
//! 它们是同一条链上的三步，共用一个动作（把人按停的意图交给在飞的执行体）：
//!
//! - [`pause`]：把任务**按住**——在飞的那一轮收口，游标挂 `user_paused`，位置保留；
//! - 续跑：不是本模块的实现，而是 **pending 动作表里 `user_paused` 那一行的 `continue`**
//!   （`crate::actions`）走既有 [`crate::pipeline::resume::apply_resume`]——人一按键，
//!   游标清 pending 回到原处接着走。这里不抄第二份「续跑做了什么」；
//! - [`rerun`]：**从本阶段入口重来**——落 [`crate::pipeline::Landing::Rerun`]，那一轮不算。
//!
//! # 为什么暂停落成 pending，而不是新造一个 `TaskStatus`
//!
//! 「暂停」在这套系统里**本来就有格子**：`docs/implementation.md` 的伪码写的是「非空 →
//! 暂停（等 resume）」，pending 就是「停着等人」那一格。新造一个状态要同时改：看板列、
//! 终态判定、`occupies_slot` 的名额口径、调度器准入、以及 `sync_task_projection`（状态是
//! 游标的投影，多一个状态就得再找一处「谁说了算」）。落成 pending 之后，续跑、看板、
//! 准入、准入后的名额占用、SSE 全都不必为它新开一条路。
//!
//! 代价如实记：暂停中的任务在看板上与其它待办**同一格**（都是 pending）。区别在原因那一栏
//! ——「已暂停（手动）」，以及调度器不再为它报「有人得管一管」：人自己按住的东西不需要
//! 别人来提醒（[`crate::pipeline::cursor::is_human_hold`]）。
//!
//! # 只对**已准入**的任务按住（`running` / `pending`）
//!
//! 队列里（`queued`）与等依赖（`waiting`）的任务**按住没有意义**：它们还没被准入，本来
//! 就没在跑。真正想做的是「别让它开跑」——那是另一件事（名额、依赖两套闸各管一段），
//! 本模块不做，也不假装做到：这两种状态被拒，报文说清为什么。
//!
//! 已准入的任务被按住之后**照旧占着名额**（决策 117：准入后、终态前恒占）——它占着一个
//! worktree、一条分支、以及一个「随时可以接着跑」的位置，而不是回到队尾重排。
//!
//! # 中止是**协作**的，账要如实
//!
//! [`crate::pipeline::executor::request_hold`] 与判超时那条路走同一个通道：执行体在下一个
//! await 点收口（决策 226），**停在不返回的阻塞调用里的执行体收不到**。故两个函数都回一个
//! `notified`：`false` = 进程内没有在跑的执行体（它早已退出，或正卡在不返回的调用里）。
//! 台账（游标挂起 / 落点）照落——执行体那边的护栏（`run_inner` 的 `held` 分支）保证它若
//! 哪天醒过来也不会把这一笔记的账推走。

use std::sync::Arc;

use crate::pipeline::advance::{advance, Landing};
use crate::pipeline::landing::entry_node;
use crate::scheduler::ResumeFn;
use crate::sse::{SseEvent, SseSink};
use crate::storage::Store;
use crate::types::{
    CursorStatus, NodeCursor, PendingKind, PendingReason, Stage, Task, TaskStatus,
    TransitionTrigger,
};
use crate::{Error, Result};

/// 一次「按住」的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paused {
    /// 被挂起的游标（各自的 `pending_reason` 都是 `user_paused`）。
    pub cursor_ids: Vec<String>,
    /// 是否确实通知到了一个在跑的执行体（`false` = 它早已退出，或卡在不返回的调用里）。
    pub notified: bool,
}

/// 一次「重跑本阶段」的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rerun {
    /// 被落回阶段入口的游标及其落点。并行区间里可能不止一条，各回**自己**阶段的入口。
    pub moved: Vec<RerunCursor>,
    /// 是否确实通知到了一个在跑的执行体（语义同 [`Paused::notified`]）。
    pub notified: bool,
}

/// 一条被重跑的游标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RerunCursor {
    pub cursor_id: String,
    /// 落点阶段（重跑的是**本阶段**，不是整条任务——整条归 `POST /tasks/{id}/retry`）。
    pub stage: Stage,
}

/// `POST /tasks/{id}/pause`：把任务按住（决策 276）。
pub async fn pause(store: &Store, sse: &Arc<dyn SseSink>, task_id: &str) -> Result<Paused> {
    let task = store.get_task(task_id).await?;
    ensure_holdable(&task)?;
    let active = active_cursors(store, task_id).await?;
    if active.is_empty() {
        return Err(Error::Validation(format!(
            "任务 {task_id} 没有在跑的游标可按住（游标都挂着等拍板，或都停在 join 边界上）\
             ——已经停着的东西不需要再暂停"
        )));
    }

    // **先请求中止、再落账**：反过来的话，一个「刚好在这一瞬跑完」的节点会在两笔写之间
    // 把游标推走，接着我们才挂上 pending——屏上看起来是按下去了，实际它继续跑了。
    // 执行体那侧还有护栏（`run_inner` 的 `held` 分支：人按停的请求一旦发出，本轮结论
    // 一个字都不许写台账），两道一起才封住这个窗口。
    let notified = crate::pipeline::executor::request_hold(task_id);

    let mut cursor_ids = Vec::new();
    for cursor in &active {
        let reason = PendingReason::new(
            PendingKind::UserPaused,
            cursor.stage,
            cursor.node,
            format!(
                "已按暂停（手动）：{}.{} 的这一轮已中止，位置保留——续跑从这里接着走",
                cursor.stage, cursor.node
            ),
        );
        advance(
            store,
            task_id,
            cursor,
            Landing::Pause {
                reason: reason.clone(),
            },
            // 挂起不写流转行，这个 trigger 不会被读到（门的签名对各落点是同一个）。
            TransitionTrigger::UserResume,
            None,
        )
        .await?;
        sse.emit(SseEvent::Pending {
            task_id: task_id.to_string(),
            branch: cursor.branch.clone(),
            cursor_id: cursor.cursor_id.clone(),
            reason,
        });
        cursor_ids.push(cursor.cursor_id.clone());
    }
    store.sync_task_projection(task_id).await?;
    Ok(Paused {
        cursor_ids,
        notified,
    })
}

/// `POST /tasks/{id}/rerun`：把**本阶段**从入口重跑一遍（决策 276）。
///
/// 与 [`pause`] 共用「先把在飞的那一轮按停」那一步，之后不挂 pending 而是落回阶段入口：
/// 人在说「这一轮不算，重来」，不是在说「先停一下」。
pub async fn rerun(
    store: &Store,
    resume: &ResumeFn,
    sse: &Arc<dyn SseSink>,
    task_id: &str,
) -> Result<Rerun> {
    let task = store.get_task(task_id).await?;
    ensure_holdable(&task)?;

    // **跑过才有可重跑的**：本阶段一条 run 行都没有（例如任务刚被准入、init.execute 还没
    // 开始）时，重跑等于什么也没做——那种「按了没反应」比一句拒绝糟糕得多。历史 run 全量
    // 取一次（一条任务的历史 run 是几十条量级），不在循环里逐条查。
    let runs = store.list_runs(task_id).await?;
    let mut targets = Vec::new();
    for cursor in active_cursors(store, task_id).await? {
        if runs.iter().any(|r| r.stage == cursor.stage) {
            targets.push(cursor);
        }
    }
    if targets.is_empty() {
        // 已经停着的任务**不由本端点重跑**：它那两颗出口键（续跑 / 重跑本阶段）都在
        // `allowed_actions` 里，走 resume 那一份唯一实现（那颗 goto 同样落 `Landing::Rerun`，
        // 决策 276）。这里被问到，说明调用方该走的是 resume——报文要点明这条分工。
        let live = store.load_live_cursors(task_id).await?;
        let stopped = live.iter().filter(|c| c.is_pending()).count();
        if stopped > 0 {
            return Err(Error::Validation(format!(
                "任务 {task_id} 已经停着（{stopped} 条游标挂着待办）：从停住的状态重跑本阶段走 \
                 resume（动作名 goto，落点 = 本阶段入口）；本端点管的是**在跑**的任务"
            )));
        }
        return Err(Error::Validation(format!(
            "任务 {task_id} 的当前阶段还没跑过（台账里没有它的 run），没有可重跑的一轮"
        )));
    }

    let notified = crate::pipeline::executor::request_hold(task_id);

    let mut moved = Vec::new();
    for cursor in &targets {
        let target = (cursor.stage, entry_node(cursor.stage));
        let from = (cursor.stage, cursor.node);
        advance(
            store,
            task_id,
            cursor,
            Landing::Rerun(target.0, target.1),
            TransitionTrigger::UserResume,
            Some("人工重跑本阶段"),
        )
        .await?;
        if from != target {
            sse.emit(SseEvent::StageChanged {
                task_id: task_id.to_string(),
                branch: cursor.branch.clone(),
                from_stage: Some(from.0),
                from_node: Some(from.1),
                to_stage: target.0,
                to_node: target.1,
                trigger: TransitionTrigger::UserResume.as_str().into(),
                reason: Some("人工重跑本阶段".into()),
            });
        }
        // 位置没挪的（本来就在入口）也发一条：`CursorChanged` 说的是「这条游标现在长这样」，
        // 而它刚刚离开 pending——状态确实变了。
        sse.emit(SseEvent::CursorChanged {
            task_id: task_id.to_string(),
            branch: cursor.branch.clone(),
            cursor_id: cursor.cursor_id.clone(),
            status: CursorStatus::Active.as_str().into(),
            stage: target.0,
            node: target.1,
        });
        moved.push(RerunCursor {
            cursor_id: cursor.cursor_id.clone(),
            stage: target.0,
        });
    }
    store.sync_task_projection(task_id).await?;
    // 落点已定，重新派一次执行体：任务本来就被准入过（上面那道闸），故不经过队列——
    // 它占的名额一直是它自己的（决策 117）。
    resume(task_id);
    Ok(Rerun { moved, notified })
}

/// 按住 / 重跑的共同前提（决策 276）：**非终态**且**已被准入**。
fn ensure_holdable(task: &Task) -> Result<()> {
    if task.status.is_terminal() {
        return Err(Error::Validation(format!(
            "任务 {} 已是终态（{}）：暂停与重跑都是「让在跑的东西停下来」，终态任务没有在跑的东西。\
             重跑整条用 retry",
            task.id,
            task.status.as_str()
        )));
    }
    if matches!(task.status, TaskStatus::Queued | TaskStatus::Waiting) {
        return Err(Error::Validation(format!(
            "任务 {} 还没被准入（{}）：它不在跑，没有什么可按住或重跑的。等它开跑再操作",
            task.id,
            task.status.as_str()
        )));
    }
    Ok(())
}

/// 本任务此刻**在跑**的游标（`active` 那一档）。
///
/// `waiting_join` 不算：它已经停在 join 边界上了，而那个位置**不能被挂起**——续跑会把
/// 游标恢复成 `active`（`clear_pending_in_tx` 只认 pending 那一档），于是它会重跑一遍
/// 已经跑完的节点，而 join 也再等不到它。
async fn active_cursors(store: &Store, task_id: &str) -> Result<Vec<NodeCursor>> {
    Ok(store
        .load_live_cursors(task_id)
        .await?
        .into_iter()
        .filter(|c| c.is_runnable())
        .collect())
}
