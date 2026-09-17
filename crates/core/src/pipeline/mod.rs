//! 流水线：图拓扑、条件边路由、落点表、游标谓词。

pub mod cursor;
pub mod executor;
pub mod foreman;
pub mod graph;
pub mod landing;
pub mod proposals;
pub mod pseudo;
pub mod routes;
pub mod subagent;

pub use cursor::{
    focus_cursor, has_pending_cursor, has_runnable_cursor, is_join_ready, live_cursors,
    pending_cursors, project_pending_reason, project_task_status, runnable_cursors,
};
pub use executor::Executor;
pub use foreman::{
    build_briefing, trim_history, ForemanBriefing, ForemanRunner, ForemanTrace, ForemanTurn,
    FOREMAN_AGENT_TYPE, FOREMAN_MAX_ROUNDS, FOREMAN_PERSONA, FOREMAN_STAGE_KEY, FOREMAN_TOOL_SPECS,
};
pub use graph::{build_pipeline_graph, PipelineGraph};
pub use landing::{
    entry_node, next_is_join, next_stages, nodes_for_stage, skip_landing, stage_has_node,
    stage_landing, SkipLanding, StageLanding, JOIN_STAGE,
};
pub use routes::{
    resolve_validate_output, route, route_after_validate_output, route_by_readiness,
    route_code_gate, route_merge, MetadataView, RouteContext, ValidateOutcome,
};
