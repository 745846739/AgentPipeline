//! petgraph DAG：编译时确定的拓扑（决策 39 / G15）。
//!
//! 图只承载"正常流转 + 固定打回边"；pending / timeout 等中断不是图边，由游标状态承载。
//! 路由函数（[`super::routes`]）决定从当前节点走哪条边。

use petgraph::graph::DiGraph;
use petgraph::visit::EdgeRef;
use petgraph::Direction;

use crate::types::{EdgeKind, Node, Stage};

/// 图节点 = `(stage, node)`。
pub type PipelineGraph = DiGraph<(Stage, Node), EdgeKind>;

/// 构造流水线图。
pub fn build_pipeline_graph() -> PipelineGraph {
    let mut g = PipelineGraph::new();

    // 1. 所有阶段的实际节点（§1.2 例外表由 landing::nodes_for_stage 决定）
    let mut index = std::collections::HashMap::new();
    for &stage in crate::types::ALL_STAGES.iter() {
        for &node in super::landing::nodes_for_stage(stage) {
            let i = g.add_node((stage, node));
            index.insert((stage, node), i);
        }
    }

    let get = |key: (Stage, Node)| *index.get(&key).expect("节点应已注册");

    // 2. 阶段内边：validate_input → execute → validate_output，validate_output → execute(重试)
    for &stage in crate::types::ALL_STAGES.iter() {
        if super::landing::stage_has_node(stage, Node::ValidateInput) {
            g.add_edge(
                get((stage, Node::ValidateInput)),
                get((stage, Node::Execute)),
                EdgeKind::Next,
            );
        }
        if super::landing::stage_has_node(stage, Node::ValidateOutput) {
            g.add_edge(
                get((stage, Node::Execute)),
                get((stage, Node::ValidateOutput)),
                EdgeKind::Next,
            );
            // 重试是自环式的回边：validate_output → execute
            g.add_edge(
                get((stage, Node::ValidateOutput)),
                get((stage, Node::Execute)),
                EdgeKind::Retry,
            );
        }
    }

    // 3. 阶段间正常流转边（architect-design 分裂成两条）
    for &stage in crate::types::ALL_STAGES.iter() {
        let from_node = last_node(stage);
        for &next in super::landing::next_stages(stage) {
            if let Some(src) = from_node {
                if let Some(&dst) = index.get(&(next, super::landing::entry_node(next))) {
                    g.add_edge(get(src), dst, EdgeKind::Next);
                }
            }
        }
    }

    // 4. 固定打回边
    // merge 冲突打回 develop.execute（决策 74）
    g.add_edge(
        get((Stage::Merge, Node::Execute)),
        get((Stage::Develop, Node::Execute)),
        EdgeKind::KickbackDevelop,
    );
    // merge 测试闸门失败跳回 test.execute（决策 85）
    g.add_edge(
        get((Stage::Merge, Node::Execute)),
        get((Stage::Test, Node::Execute)),
        EdgeKind::GotoTest,
    );
    // review 不通过 → 用户 `goto develop.execute`（决策 2 / 131：路由落 user_decision，
    // 这条边只是 goto 的合法落点，不是 route() 的返回值）
    g.add_edge(
        get((Stage::Review, Node::ValidateOutput)),
        get((Stage::Develop, Node::Execute)),
        EdgeKind::KickbackDevelop,
    );
    // sync-check backtrack 回 architect-design.validate_input（决策 83）
    g.add_edge(
        get((Stage::SyncCheck, Node::Execute)),
        get((Stage::ArchitectDesign, Node::ValidateInput)),
        EdgeKind::Backtrack,
    );

    g
}

/// 阶段的最后一个节点（无 validate_output 的阶段取 execute）。
pub fn last_node(stage: Stage) -> Option<(Stage, Node)> {
    let nodes = super::landing::nodes_for_stage(stage);
    nodes.last().map(|&n| (stage, n))
}

/// 从某节点的可达后继（用于校验图与路由表一致）。
pub fn successors(g: &PipelineGraph, stage: Stage, node: Node) -> Vec<(Stage, Node, EdgeKind)> {
    let Some(idx) = g.node_indices().find(|&i| g[i] == (stage, node)) else {
        return Vec::new();
    };
    g.edges_directed(idx, Direction::Outgoing)
        .map(|e| {
            let (s, n) = g[e.target()];
            (s, n, *e.weight())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ALL_STAGES;

    #[test]
    fn graph_contains_every_stage_node() {
        let g = build_pipeline_graph();
        for stage in ALL_STAGES {
            for node in super::super::landing::nodes_for_stage(stage) {
                assert!(
                    g.node_indices().any(|i| g[i] == (stage, *node)),
                    "缺少节点 {stage}.{node}"
                );
            }
        }
    }

    #[test]
    fn parallel_branches_converge_on_join() {
        let g = build_pipeline_graph();
        // develop-design / test-design 的最后一个节点都指向 sync-check.execute
        for stage in [Stage::DevelopDesign, Stage::TestDesign] {
            let (s, n) = last_node(stage).unwrap();
            let succ = successors(&g, s, n);
            assert!(
                succ.iter().any(|(ts, tn, k)| *ts == Stage::SyncCheck
                    && *tn == Node::Execute
                    && *k == EdgeKind::Next),
                "{stage} 未汇入 sync-check：{succ:?}"
            );
        }
    }

    #[test]
    fn architect_design_splits_into_two_branches() {
        let g = build_pipeline_graph();
        let succ = successors(&g, Stage::ArchitectDesign, Node::ValidateOutput);
        let targets: Vec<Stage> = succ.iter().map(|(s, _, _)| *s).collect();
        assert!(targets.contains(&Stage::DevelopDesign));
        assert!(targets.contains(&Stage::TestDesign));
    }

    #[test]
    fn merge_has_all_three_outcomes() {
        let g = build_pipeline_graph();
        let succ = successors(&g, Stage::Merge, Node::Execute);
        assert!(succ
            .iter()
            .any(|(s, _, k)| *s == Stage::Done && *k == EdgeKind::Next));
        assert!(succ
            .iter()
            .any(|(s, _, k)| *s == Stage::Develop && *k == EdgeKind::KickbackDevelop));
        assert!(succ
            .iter()
            .any(|(s, _, k)| *s == Stage::Test && *k == EdgeKind::GotoTest));
    }

    #[test]
    fn sync_check_backtrack_edge_returns_to_architect() {
        // 决策 83：backtrack 的落点是 architect-design.validate_input，标签是 Backtrack
        // （与 merge lint 失败的 KickbackDevelop 区分开）。
        let g = build_pipeline_graph();
        let succ = successors(&g, Stage::SyncCheck, Node::Execute);
        assert!(
            succ.iter().any(|(s, n, k)| *s == Stage::ArchitectDesign
                && *n == Node::ValidateInput
                && *k == EdgeKind::Backtrack),
            "sync-check 缺少 backtrack 边：{succ:?}"
        );
    }

    #[test]
    fn retry_edge_exists_on_every_validate_output_stage() {
        let g = build_pipeline_graph();
        for stage in ALL_STAGES {
            if super::super::landing::stage_has_node(stage, Node::ValidateOutput) {
                let succ = successors(&g, stage, Node::ValidateOutput);
                assert!(
                    succ.iter().any(|(ts, tn, k)| *ts == stage
                        && *tn == Node::Execute
                        && *k == EdgeKind::Retry),
                    "{stage}.validate_output 缺少重试回边"
                );
            }
        }
    }
}
