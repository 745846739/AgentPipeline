//! 游标推进：把「落点 → 一串写」收成**一笔 `BEGIN IMMEDIATE`**（决策 245）。
//!
//! 为什么单独成模块：在此之前「把游标搬到下一个落点」有四份实现（executor 的
//! `apply_edge`、`Store::apply_resume`、`advance_after_judge_continue`、调度器与
//! `unstick` 的手抄两连），而调用方必须自己记住 3–5 笔存储调用的**顺序**。其中
//! `EdgeKind::Retry` 是三笔独立的 autocommit——中间那一瞬游标是「attempts+1 但还指着
//! `validate_output`」。`storage/cursors.rs` 的注释早就写明了这个形状的危害与绕开它的
//! 方法，retry 边恰好没绕。
//!
//! **门吃落点 + 修饰，不吃原因。** 四种原因（`EdgeKind` / `ResumeAction` / 超时耗尽 /
//! 用户裁决）各有自己的校验（judge-continue 不接受 `Terminal`、goto 必须落在
//! `entry_node`、skip 在 merge 上非法），这些校验留在门外各自翻译。门若吃原因就得把它们
//! 搬进来，于是门会重新长成第二个 [`crate::pipeline::route`]。
//!
//! **门不发 SSE、不做任务投影。** [`advance`] 只落库并把事务后的新游标交回来；同步投影
//! 与事件由调用方完成——投影是派生读模型（允许延迟一致），SSE 是观测面（形状因调用方而异）。
//! 两者进了事务只会把一个纯 DB 模块变成要接 `&dyn Fn` 的模块。
//!
//! **进程内存 / 重派不进事务模块。** 这是本模块的边界判据：`unstick` 的 `force_release` 是
//! 不可回滚的内存操作、`handle_timeout` 的重试支带一个进程级重派——它们都留在自己家里，
//! 只有动 DB 的那一步进来。

use crate::storage::Store;
use crate::types::{Node, NodeCursor, PendingReason, Stage, TransitionTrigger};
use crate::Result;

use super::JOIN_STAGE;

/// 推进的落点（决策 245）。
#[derive(Debug, Clone, PartialEq)]
pub enum Landing {
    /// 落到某 `(stage, node)`，并把 `validate_attempts` 归零（决策 43）。
    /// 跨阶段的 `Next`、kickback / goto、resume 的 goto 与串行 skip 走这一条。
    Entry(Stage, Node),
    /// 同 [`Landing::Entry`]，但这是**人按下的「重跑本阶段」**（决策 276）。
    ///
    /// 落点与 `Entry` 逐字相同（含 `validate_attempts` 归零）；差别只在**离开 pending 时
    /// 记下的原因**：重跑记 `user_rerun`，于是续接判定表给它 `false`（重跑 = 这一轮不算、
    /// 重开一段对话），台账里也读得出「这一轮是人按了重跑」。
    ///
    /// 为什么不给 `advance` 再添一个 `resume_cause` 形参：那个参数只服务这一条落点，
    /// 而落点**本来就是这个门吃的语言**（「门吃落点 + 修饰，不吃原因」）——把「这是重跑」
    /// 说成一种落点，比让每个调用点多传一个恒为 `None` 的参数诚实。
    Rerun(Stage, Node),
    /// 同 [`Landing::Entry`]，但落完再记一次尝试：`validate_attempts` 归零后 +1，**恒为 1**。
    ///
    /// 决策 135 的 judge goto（裁决不合格 → 打回本阶段 `execute`）走这一条。它今天的实现是
    /// `set_cursor_stage`（归零）紧跟 `increment_cursor_attempts`（+1），净效果恒为 1 而不是
    /// 「从当前值 +1」——那是既有语义，本门逐字复刻，不顺手改。
    EntryWithAttempt(Stage, Node),
    /// 阶段内重试：落回本阶段 `execute`，`validate_attempts` +1（决策 82）。
    /// 这是本模块相对旧实现的**唯一行为变化点**：三笔 autocommit → 一笔事务。
    Retry,
    /// 原地：不改落点、不碰 `validate_attempts`，只离开 pending。
    /// resume 的 continue 里有三条是这一类（`dependency_failed` / `info_insufficient` /
    /// 兜底 continue）——它们今天只清 pending，不动游标。
    Stay,
    /// 到 join 边界（决策 107）；`skipped` 置 `skipped_to_join`（决策 93）。
    JoinBoundary {
        /// `true` = 并行分支 `skip` 进 join；`false` = 正常跑到边界。
        skipped: bool,
    },
    /// 挂起（决策 82）：只写 pending，不动落点，**也不写流转行**——挂起不是一次推进。
    ///
    /// `reason` 整条带进来而不是拆成 `kind/context/message`：`NodeOutput::Pending` 里有
    /// 构造者指定的 `(stage, node)`（例如 conflict_wait 写死 `(architect-design, execute)`），
    /// 由门按游标重算会悄悄改写这条待办的落点。
    Pause { reason: PendingReason },
    // Split / Replace **不在这里**：它们已经是原子的 Store 方法（`split_cursors` /
    // `replace_cursors_with_main`，都走 `begin_write`）。今天无人依赖「游标 + 流转行」这对
    // 是原子的，把它扩成一对是新约束而非修 bug（决策 245）。
}

/// 一次推进的落库结果（决策 245）。**门不发 SSE、不做任务投影**——由调用方完成。
///
/// 草案里还有一个 `replaced: bool`（Split / Replace 走过门时置位）。那两个落点最终**没有**
/// 进 [`Landing`]，字段会恒为 `false`——一个永远为假的判据就是死代码，故不设。
#[derive(Debug, Clone, PartialEq)]
pub struct Advanced {
    /// 事务后受影响游标的新位置（调用方据此发 `CursorChanged` / `StageChanged`）。
    pub cursors: Vec<NodeCursor>,
}

/// 把游标推进到 `landing`，**游标行与流转行在同一笔事务里写完**。
///
/// `cursor` 是**推进前**的快照：流转行的 `from` 与 `branch` 都取自它。
/// 除 `Pause` 外的落点会顺带清 pending——对 active 游标是 no-op，对 pending 游标是必须的
/// （落到新地方就不再是「等在原地」）；清的原因仍由 [`crate::storage::cursors::Store::clear_cursor_pending`]
/// 那套规则分类，门不自己发明一套。
pub async fn advance(
    store: &Store,
    task_id: &str,
    cursor: &NodeCursor,
    landing: Landing,
    trigger: TransitionTrigger,
    reason: Option<&str>,
) -> Result<Advanced> {
    let mut tx = store.begin_write().await?;

    // 落点 → 游标行的写；`to` = 这次推进在流转行里记的落点（挂起不写流转行）。
    let to: Option<(Stage, Node)> = match landing {
        Landing::Entry(stage, node) => {
            store
                .clear_pending_in_tx(&mut tx, &cursor.cursor_id, None)
                .await?;
            store
                .set_cursor_stage_in_tx(&mut tx, &cursor.cursor_id, stage, node)
                .await?;
            Some((stage, node))
        }
        Landing::Rerun(stage, node) => {
            // 「人按了重跑」由**按的是哪颗键**决定，与 merge / review 那两条同一个理由
            // （`clear_pending_in_tx` 的 `explicit`）：同一条 `user_paused` 的两颗出口键
            // （续跑 / 重跑）在 pending 原因上同名，只有端点知道按的是哪一颗。
            store
                .clear_pending_in_tx(
                    &mut tx,
                    &cursor.cursor_id,
                    Some(crate::types::ResumeCause::UserRerun),
                )
                .await?;
            store
                .set_cursor_stage_in_tx(&mut tx, &cursor.cursor_id, stage, node)
                .await?;
            Some((stage, node))
        }
        Landing::EntryWithAttempt(stage, node) => {
            store
                .clear_pending_in_tx(&mut tx, &cursor.cursor_id, None)
                .await?;
            store
                .set_cursor_stage_in_tx(&mut tx, &cursor.cursor_id, stage, node)
                .await?;
            store
                .increment_cursor_attempts_in_tx(&mut tx, &cursor.cursor_id)
                .await?;
            Some((stage, node))
        }
        Landing::Retry => {
            store
                .clear_pending_in_tx(&mut tx, &cursor.cursor_id, None)
                .await?;
            // 两笔写在同一事务里：「attempts+1 但还指着 validate_output」这个中间态
            // 在事务外永远读不到（决策 245 的直接动因）。
            store
                .increment_cursor_attempts_in_tx(&mut tx, &cursor.cursor_id)
                .await?;
            store
                .move_cursor_in_tx(&mut tx, &cursor.cursor_id, cursor.stage, Node::Execute)
                .await?;
            Some((cursor.stage, Node::Execute))
        }
        Landing::Stay => {
            store
                .clear_pending_in_tx(&mut tx, &cursor.cursor_id, None)
                .await?;
            Some((cursor.stage, cursor.node))
        }
        Landing::JoinBoundary { skipped } => {
            store
                .clear_pending_in_tx(&mut tx, &cursor.cursor_id, None)
                .await?;
            if skipped {
                // 决策 93：skip 进 join 的那条路今天还额外把 attempts 归零（`apply_resume`
                // 的 `reset_cursor_attempts`）。归零与否对 waiting_join 的游标不可达——
                // join 通过 / 回退都会换成新行——但两条路现在合到同一处，一并归零，
                // 免得留下「skip 归零、正常到界不归零」这种读不出来的差别。
                store
                    .mark_cursor_skipped_to_join_in_tx(&mut tx, &cursor.cursor_id)
                    .await?;
                store
                    .reset_cursor_attempts_in_tx(&mut tx, &cursor.cursor_id)
                    .await?;
            } else {
                store
                    .set_cursor_waiting_join_in_tx(&mut tx, &cursor.cursor_id)
                    .await?;
            }
            Some((JOIN_STAGE, Node::Execute))
        }
        Landing::Pause { reason } => {
            store
                .set_cursor_pending_in_tx(&mut tx, &cursor.cursor_id, &reason)
                .await?;
            None
        }
    };

    if let Some(to) = to {
        store
            .insert_transition_in_tx(
                &mut tx,
                task_id,
                &cursor.branch,
                Some((cursor.stage, cursor.node)),
                to,
                trigger,
                reason,
            )
            .await?;
    }

    tx.commit().await?;

    Ok(Advanced {
        cursors: vec![store.get_cursor(&cursor.cursor_id).await?],
    })
}
