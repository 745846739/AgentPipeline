//! 游标谓词与焦点投影（决策 80 / 82 / 92 / 117 / 130）。
//!
//! `kanban_tasks.current_stage` / `current_node` / `validate_attempts` 只是焦点游标的投影，
//! 执行状态的唯一事实来源是游标各行。调度器的提醒 / stalled 判据必须谓词化，
//! 不得只看 `task.status`——一个分支 pending、另一分支在跑的任务 `status = pending` 但没卡住。

use crate::types::{CursorStatus, NodeCursor, TaskStatus};

/// 活跃游标（非 archived 的历史行不参与执行）。
pub fn live_cursors(cursors: &[NodeCursor]) -> Vec<&NodeCursor> {
    cursors
        .iter()
        .filter(|c| c.status != CursorStatus::Archived)
        .collect()
}

/// 可继续执行的游标（决策 82）。
pub fn runnable_cursors(cursors: &[NodeCursor]) -> Vec<&NodeCursor> {
    cursors.iter().filter(|c| c.is_runnable()).collect()
}

/// 被阻塞的游标。
pub fn pending_cursors(cursors: &[NodeCursor]) -> Vec<&NodeCursor> {
    cursors.iter().filter(|c| c.is_pending()).collect()
}

pub fn has_runnable_cursor(cursors: &[NodeCursor]) -> bool {
    cursors.iter().any(|c| c.is_runnable())
}

pub fn has_pending_cursor(cursors: &[NodeCursor]) -> bool {
    cursors.iter().any(|c| c.is_pending())
}

/// 这条 pending **是人自己按下的暂停**（决策 276）。
///
/// 调度器的两处自动行为按它豁免——**人按住的东西不需要别人来管**：
/// - `remind_pending_tasks` 的停滞提醒 / `stalled` 标记（那是给「没人管的 pending」准备的）；
/// - `note_discoveries` 的待办落表（那是给「值班长该看一眼」准备的）。
///
/// 判据落在**游标**而不是任务上（决策 82 的同一姿态）：任务状态只是投影，一个分支被人按住、
/// 另一分支在跑的任务，不该因为这条投影被当成「等人管」。
pub fn is_human_hold(cursor: &NodeCursor) -> bool {
    cursor.is_pending()
        && cursor
            .pending_reason
            .as_ref()
            .is_some_and(|r| r.kind == crate::types::PendingKind::UserPaused)
}

/// 这个任务**所有**的 pending 都是人按下的暂停（决策 276）。
///
/// 提醒 / 停滞标记要的是这一档而不是 [`is_human_hold`] 的「有一条算一条」：两条游标里
/// 一条被人按住、另一条挂着真待办时，任务确实还需要人管——只有「全部被人按住」才是
/// 「别去打扰他」。无 pending 的任务返回 `false`（没有待办 ≠ 人按住了）。
pub fn all_pending_are_human_holds(cursors: &[NodeCursor]) -> bool {
    let pending: Vec<&NodeCursor> = pending_cursors(cursors);
    !pending.is_empty() && pending.iter().all(|c| is_human_hold(c))
}

/// join 条件：所有活跃游标都到达边界（`waiting_join`）且均无 pending（决策 83）。
///
/// 至少要有两条游标——单游标任务不构成 join（sync-check 是并行区间的屏障）。
pub fn is_join_ready(cursors: &[NodeCursor]) -> bool {
    let live = live_cursors(cursors);
    live.len() >= 2 && live.iter().all(|c| c.status == CursorStatus::WaitingJoin)
}

/// 焦点游标（决策 92 / 130）：优先取 pending 游标，否则取 `updated_at` 最新者；
/// 双 pending 时取 `updated_at` 最新。串行时即 main。
pub fn focus_cursor(cursors: &[NodeCursor]) -> Option<&NodeCursor> {
    let live = live_cursors(cursors);
    let pending: Vec<&NodeCursor> = live
        .iter()
        .copied()
        .filter(|c| c.status == CursorStatus::Pending)
        .collect();
    if !pending.is_empty() {
        return pick_latest(pending);
    }
    pick_latest(live)
}

/// 取 `updated_at` 最新者。
fn pick_latest(pool: Vec<&NodeCursor>) -> Option<&NodeCursor> {
    pool.into_iter().max_by_key(|c| c.updated_at)
}

/// 任务级 status 的投影（决策 82）：任一游标 pending 即 `pending`，否则只要有可运行游标即 `running`。
///
/// `waiting` / `queued` 由准入与依赖路径决定，不由游标投影推导，因此调用方只在
/// 已经 `running` / `pending` 的任务上使用本函数。
pub fn project_task_status(cursors: &[NodeCursor]) -> TaskStatus {
    if has_pending_cursor(cursors) {
        TaskStatus::Pending
    } else {
        TaskStatus::Running
    }
}

/// 任务级 pending 投影：所有 pending 游标中 `updated_at` 最新者（决策 130 ④）。
pub fn project_pending_reason(cursors: &[NodeCursor]) -> Option<crate::types::PendingReason> {
    pending_cursors(cursors)
        .into_iter()
        .max_by_key(|c| c.updated_at)
        .and_then(|c| c.pending_reason.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Node, PendingKind, PendingReason, Stage};
    use chrono::{Duration, Utc};

    fn cursor(id: &str, status: CursorStatus, secs_ago: i64) -> NodeCursor {
        let t = Utc::now() - Duration::seconds(secs_ago);
        NodeCursor {
            cursor_id: id.into(),
            task_id: "t1".into(),
            branch: if id == "main" {
                "main"
            } else {
                "develop-design"
            }
            .into(),
            stage: Stage::DevelopDesign,
            node: Node::Execute,
            status,
            validate_attempts: 0,
            skipped_to_join: false,
            pending_reason: if status == CursorStatus::Pending {
                Some(PendingReason::new(
                    PendingKind::Timeout,
                    Stage::DevelopDesign,
                    Node::Execute,
                    format!("{id} timeout"),
                ))
            } else {
                None
            },
            created_at: t,
            updated_at: t,
        }
    }

    #[test]
    fn focus_prefers_pending_over_newer_active() {
        let cursors = vec![
            cursor("active", CursorStatus::Active, 0),     // 更新
            cursor("pending", CursorStatus::Pending, 100), // 更旧但 pending
        ];
        assert_eq!(focus_cursor(&cursors).unwrap().cursor_id, "pending");
    }

    #[test]
    fn focus_takes_latest_among_double_pending() {
        // 决策 130 ④：双游标同时 pending 取 updated_at 最新
        let cursors = vec![
            cursor("p-old", CursorStatus::Pending, 50),
            cursor("p-new", CursorStatus::Pending, 5),
        ];
        assert_eq!(focus_cursor(&cursors).unwrap().cursor_id, "p-new");
    }

    #[test]
    fn focus_falls_back_to_latest_active() {
        let cursors = vec![
            cursor("a-old", CursorStatus::Active, 30),
            cursor("a-new", CursorStatus::Active, 1),
        ];
        assert_eq!(focus_cursor(&cursors).unwrap().cursor_id, "a-new");
    }

    #[test]
    fn archived_cursors_are_ignored() {
        let cursors = vec![
            cursor("archived-new", CursorStatus::Archived, 0),
            cursor("active-old", CursorStatus::Active, 100),
        ];
        assert_eq!(focus_cursor(&cursors).unwrap().cursor_id, "active-old");
        assert_eq!(live_cursors(&cursors).len(), 1);
    }

    #[test]
    fn pending_does_not_make_task_stalled_when_other_branch_runs() {
        // 决策 92：一个分支 pending、另一个 active → 任务并未卡住
        let cursors = vec![
            cursor("p", CursorStatus::Pending, 5),
            cursor("a", CursorStatus::Active, 1),
        ];
        assert!(has_pending_cursor(&cursors));
        assert!(has_runnable_cursor(&cursors));
        assert!(!is_join_ready(&cursors));
        assert_eq!(project_task_status(&cursors), TaskStatus::Pending);
    }

    #[test]
    fn join_requires_all_branches_at_boundary() {
        let waiting = vec![
            cursor("d", CursorStatus::WaitingJoin, 5),
            cursor("t", CursorStatus::WaitingJoin, 1),
        ];
        assert!(is_join_ready(&waiting));

        let partial = vec![
            cursor("d", CursorStatus::WaitingJoin, 5),
            cursor("t", CursorStatus::Active, 1),
        ];
        assert!(!is_join_ready(&partial));
    }

    #[test]
    fn join_not_ready_with_single_cursor() {
        let single = vec![cursor("main", CursorStatus::WaitingJoin, 1)];
        assert!(!is_join_ready(&single));
    }

    #[test]
    fn pending_projection_takes_latest_pending_reason() {
        let cursors = vec![
            cursor("p1", CursorStatus::Pending, 90),
            cursor("p2", CursorStatus::Pending, 2),
        ];
        let reason = project_pending_reason(&cursors).unwrap();
        assert_eq!(reason.message, "p2 timeout");
    }
}
