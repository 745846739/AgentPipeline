//! 落点表：阶段入口与 `skip` 落点（决策 69 / 93 / 115）。
//!
//! `goto` 复用 [`entry_node`]；`skip` **不复用** entry_node（决策 93）——architect-design 的
//! next 是分裂成两个阶段，而并行分支的 skip 若直接跳到 sync-check 就等于绕过 G5 的 join 条件。

use crate::types::{Node, Stage};

/// 阶段入口节点（决策 69 的 `goto` 落点查表）。
pub fn entry_node(stage: Stage) -> Node {
    match stage {
        Stage::ArchitectDesign | Stage::DevelopDesign | Stage::TestDesign => Node::ValidateInput,
        Stage::Init
        | Stage::SyncCheck
        | Stage::Develop
        | Stage::Review
        | Stage::Test
        | Stage::Merge
        | Stage::Done => Node::Execute,
    }
}

/// 某阶段实际拥有的节点（§1.2 节点集例外）。
pub fn nodes_for_stage(stage: Stage) -> &'static [Node] {
    match stage {
        Stage::Init | Stage::SyncCheck | Stage::Merge | Stage::Done => &[Node::Execute],
        Stage::ArchitectDesign | Stage::DevelopDesign | Stage::TestDesign => {
            &[Node::ValidateInput, Node::Execute, Node::ValidateOutput]
        }
        // develop / review / test 跳过 validate_input（上游已保证输入充分）
        Stage::Develop | Stage::Review | Stage::Test => &[Node::Execute, Node::ValidateOutput],
    }
}

/// 阶段是否拥有该节点。
pub fn stage_has_node(stage: Stage, node: Node) -> bool {
    nodes_for_stage(stage).contains(&node)
}

/// `next` 的落点阶段（architect-design 返回两个，是并行分裂点）。
pub fn next_stages(stage: Stage) -> &'static [Stage] {
    match stage {
        Stage::Init => &[Stage::ArchitectDesign],
        Stage::ArchitectDesign => &[Stage::DevelopDesign, Stage::TestDesign],
        Stage::DevelopDesign | Stage::TestDesign => &[Stage::SyncCheck],
        Stage::SyncCheck => &[Stage::Develop],
        Stage::Develop => &[Stage::Review],
        Stage::Review => &[Stage::Test],
        Stage::Test => &[Stage::Merge],
        Stage::Merge => &[Stage::Done],
        Stage::Done => &[],
    }
}

/// 跨阶段推进的落点分类——游标「走出本阶段」时的**唯一查表**。
///
/// 两个调用方共用它，避免同一张落点表被复刻两份（票 03）：
/// - executor 的 `apply_edge`（`EdgeKind::Next` 的跨阶段分支）；
/// - `advance_after_judge_continue`（决策 135：用户裁决合格，越过本阶段剩余节点）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageLanding {
    /// 并行分裂点：architect-design → develop-design ∥ test-design（决策 90）。
    Split,
    /// 下一阶段是 join 边界 → 本游标置 `waiting_join`（决策 107）。
    JoinBoundary,
    /// 串行下一阶段入口（决策 69）。
    StageEntry(Stage, Node),
    /// 无下一阶段（done 之后）。
    Terminal,
}

/// 按阶段查跨阶段落点（不涉及阶段内的 validate_input → execute → validate_output）。
pub fn stage_landing(stage: Stage) -> StageLanding {
    let nexts = next_stages(stage);
    if nexts.len() > 1 {
        StageLanding::Split
    } else if next_is_join(stage) {
        StageLanding::JoinBoundary
    } else if let Some(&next) = nexts.first() {
        StageLanding::StageEntry(next, entry_node(next))
    } else {
        StageLanding::Terminal
    }
}

/// join 节点：sync-check 是游标无关的屏障，不占游标行（决策 107）。
pub const JOIN_STAGE: Stage = Stage::SyncCheck;

/// 下一阶段是 join 时，`advance_cursor` 把游标置为 `waiting_join`（决策 107）。
pub fn next_is_join(stage: Stage) -> bool {
    next_stages(stage).contains(&JOIN_STAGE)
}

/// `skip` 的落点（决策 93 skip 表）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipLanding {
    /// architect-design 放行 → 游标分裂，两条都落在各自阶段的入口。
    SplitCursors,
    /// 并行分支放行 → 本分支置 `waiting_join` + `skipped_to_join = true`（不越过 join）。
    ToJoinBoundary,
    /// 串行阶段 → 下一阶段入口节点。
    StageEntry(Stage, Node),
    /// 无 skip（merge 决策 86；init / sync-check / done 无 pending）。
    Forbidden,
}

/// `skip` 落点查表。
pub fn skip_landing(stage: Stage) -> SkipLanding {
    match stage {
        // 等价于"强制通过 architect-design"：分裂到两个设计阶段的入口
        Stage::ArchitectDesign => SkipLanding::SplitCursors,
        // 不得越过 join（否则绕过 G5）
        Stage::DevelopDesign | Stage::TestDesign => SkipLanding::ToJoinBoundary,
        Stage::Develop => SkipLanding::StageEntry(Stage::Review, Node::Execute),
        Stage::Review => SkipLanding::StageEntry(Stage::Test, Node::Execute),
        Stage::Test => SkipLanding::StageEntry(Stage::Merge, Node::Execute),
        // merge 无 skip：skip 等于越过测试闸门直接合入（决策 86）
        Stage::Merge | Stage::Init | Stage::SyncCheck | Stage::Done => SkipLanding::Forbidden,
    }
}

/// architect-design 分裂出的两条分支及其落点。
pub fn split_targets() -> [(Stage, &'static str); 2] {
    [
        (Stage::DevelopDesign, "develop-design"),
        (Stage::TestDesign, "test-design"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ALL_STAGES;

    #[test]
    fn entry_node_full_table() {
        // 「落点表逐行」：architect / develop-design / test-design → validate_input
        for stage in [
            Stage::ArchitectDesign,
            Stage::DevelopDesign,
            Stage::TestDesign,
        ] {
            assert_eq!(entry_node(stage), Node::ValidateInput, "{stage}");
        }
        for stage in [
            Stage::Init,
            Stage::SyncCheck,
            Stage::Develop,
            Stage::Review,
            Stage::Test,
            Stage::Merge,
            Stage::Done,
        ] {
            assert_eq!(entry_node(stage), Node::Execute, "{stage}");
        }
    }

    #[test]
    fn skip_landing_full_table() {
        assert_eq!(
            skip_landing(Stage::ArchitectDesign),
            SkipLanding::SplitCursors
        );
        assert_eq!(
            skip_landing(Stage::DevelopDesign),
            SkipLanding::ToJoinBoundary
        );
        assert_eq!(skip_landing(Stage::TestDesign), SkipLanding::ToJoinBoundary);
        assert_eq!(
            skip_landing(Stage::Develop),
            SkipLanding::StageEntry(Stage::Review, Node::Execute)
        );
        assert_eq!(
            skip_landing(Stage::Review),
            SkipLanding::StageEntry(Stage::Test, Node::Execute)
        );
        assert_eq!(
            skip_landing(Stage::Test),
            SkipLanding::StageEntry(Stage::Merge, Node::Execute)
        );
        // merge 无 skip（决策 86）；init / sync-check / done 无 pending
        for stage in [Stage::Merge, Stage::Init, Stage::SyncCheck, Stage::Done] {
            assert_eq!(skip_landing(stage), SkipLanding::Forbidden, "{stage}");
        }
    }

    #[test]
    fn skip_never_lands_on_join_node() {
        // 决策 93 的核心：并行分支的 skip 不得越过 join。
        for stage in ALL_STAGES {
            if let SkipLanding::StageEntry(target, _) = skip_landing(stage) {
                assert_ne!(target, Stage::SyncCheck, "{stage} 的 skip 越过了 join");
            }
        }
    }

    #[test]
    fn skip_target_is_a_real_stage_entry() {
        for stage in ALL_STAGES {
            if let SkipLanding::StageEntry(target, node) = skip_landing(stage) {
                assert_eq!(
                    node,
                    entry_node(target),
                    "{stage} → {target} 落点不是入口节点"
                );
                assert!(stage_has_node(target, node));
            }
        }
    }

    #[test]
    fn node_set_exceptions() {
        assert_eq!(nodes_for_stage(Stage::Init), &[Node::Execute]);
        assert_eq!(nodes_for_stage(Stage::Done), &[Node::Execute]);
        assert_eq!(nodes_for_stage(Stage::Merge), &[Node::Execute]);
        assert_eq!(nodes_for_stage(Stage::SyncCheck), &[Node::Execute]);
        // develop / review / test 跳过 validate_input
        for stage in [Stage::Develop, Stage::Review, Stage::Test] {
            assert!(!stage_has_node(stage, Node::ValidateInput), "{stage}");
            assert!(stage_has_node(stage, Node::ValidateOutput));
        }
        // 三个设计阶段三节点齐全
        for stage in [
            Stage::ArchitectDesign,
            Stage::DevelopDesign,
            Stage::TestDesign,
        ] {
            for n in crate::types::ALL_NODES {
                assert!(stage_has_node(stage, n), "{stage}.{n}");
            }
        }
    }

    #[test]
    fn next_stages_full_table() {
        assert_eq!(next_stages(Stage::Init), &[Stage::ArchitectDesign]);
        // architect-design 是并行分裂点
        assert_eq!(
            next_stages(Stage::ArchitectDesign),
            &[Stage::DevelopDesign, Stage::TestDesign]
        );
        assert_eq!(next_stages(Stage::DevelopDesign), &[Stage::SyncCheck]);
        assert_eq!(next_stages(Stage::TestDesign), &[Stage::SyncCheck]);
        assert_eq!(next_stages(Stage::SyncCheck), &[Stage::Develop]);
        assert_eq!(next_stages(Stage::Develop), &[Stage::Review]);
        assert_eq!(next_stages(Stage::Review), &[Stage::Test]);
        assert_eq!(next_stages(Stage::Test), &[Stage::Merge]);
        assert_eq!(next_stages(Stage::Merge), &[Stage::Done]);
        assert!(next_stages(Stage::Done).is_empty());
    }

    #[test]
    fn only_parallel_branches_point_at_join() {
        for stage in ALL_STAGES {
            let expect = matches!(stage, Stage::DevelopDesign | Stage::TestDesign);
            assert_eq!(next_is_join(stage), expect, "{stage}");
        }
    }

    #[test]
    fn stage_landing_full_table() {
        // 跨阶段落点唯一查表（票 03）：executor 的 Next 与 judge continue 共用。
        assert_eq!(
            stage_landing(Stage::ArchitectDesign),
            StageLanding::Split
        );
        for stage in [Stage::DevelopDesign, Stage::TestDesign] {
            assert_eq!(stage_landing(stage), StageLanding::JoinBoundary, "{stage}");
        }
        assert_eq!(
            stage_landing(Stage::Init),
            StageLanding::StageEntry(Stage::ArchitectDesign, Node::ValidateInput)
        );
        assert_eq!(
            stage_landing(Stage::Develop),
            StageLanding::StageEntry(Stage::Review, Node::Execute)
        );
        assert_eq!(
            stage_landing(Stage::Review),
            StageLanding::StageEntry(Stage::Test, Node::Execute)
        );
        assert_eq!(
            stage_landing(Stage::Test),
            StageLanding::StageEntry(Stage::Merge, Node::Execute)
        );
        assert_eq!(
            stage_landing(Stage::Merge),
            StageLanding::StageEntry(Stage::Done, Node::Execute)
        );
        // sync-check 不占游标行：它是 join 屏障，不是游标的下一阶段入口
        assert_eq!(
            stage_landing(Stage::SyncCheck),
            StageLanding::StageEntry(Stage::Develop, Node::Execute)
        );
        assert_eq!(stage_landing(Stage::Done), StageLanding::Terminal);
    }

    #[test]
    fn stage_entry_landing_matches_next_landing() {
        // 两者必须同源：StageLanding::StageEntry 的落点 = next_landing 的唯一元素。
        for stage in ALL_STAGES {
            if let StageLanding::StageEntry(s, n) = stage_landing(stage) {
                let landings = next_stages(stage);
                assert_eq!(landings.len(), 1, "{stage}");
                assert_eq!((s, n), (landings[0], entry_node(landings[0])), "{stage}");
            }
        }
    }
}
