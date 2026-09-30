//! 流水线：图拓扑、条件边路由、落点表、游标谓词。

pub mod advance;
pub mod cursor;
pub(crate) mod events;
pub mod executor;
pub mod foreman;
pub mod foreman_actions;
pub mod landing;
pub mod merge;
pub(crate) mod model_invoke;
pub(crate) mod model_request;
pub mod pause;
pub mod proposals;
pub mod pseudo;
pub mod repair;
pub mod resume;
pub mod routes;
pub(crate) mod run_ledger;
pub mod subagent;
pub mod unstick;
pub mod window_calibration;

pub use advance::{advance, Advanced, Landing};

pub use cursor::{
    focus_cursor, has_pending_cursor, has_runnable_cursor, is_join_ready, live_cursors,
    pending_cursors, project_pending_reason, project_task_status, runnable_cursors,
};
pub use executor::Executor;
pub use foreman::{
    build_briefing, foreman_turn_in_flight, parse_attribution, trim_history, Attribution,
    AttributionKind, ForemanBriefing, ForemanRunner, ForemanSegment, ForemanTrace, ForemanTurn,
    FOREMAN_AGENT_TYPE, FOREMAN_ATTRIBUTION_MARK, FOREMAN_MAX_ROUNDS, FOREMAN_PERSONA,
    FOREMAN_STAGE_KEY, FOREMAN_TOOL_SPECS,
};
pub use landing::{
    entry_node, next_is_join, next_stages, nodes_for_stage, skip_landing, stage_has_node,
    stage_landing, SkipLanding, StageLanding, JOIN_STAGE,
};
pub use routes::{
    resolve_validate_output, route, route_after_validate_output, route_by_readiness,
    route_code_gate, route_merge, MetadataView, RouteContext, ValidateOutcome,
};
