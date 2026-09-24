//! AgentPipeline 核心库。
//!
//! 模块划分按领域而非技术分层（见 docs/implementation.md §11 与项目结构决策）：
//! pipeline（DAG / 路由 / executor）、agent（LLM 接缝 / 工具 / prompt / 上下文）、
//! storage（SQLite）、scheduler（tick 六项职责）、git（worktree / rebase / 合入）。
//!
//! 四条可测试性接缝（决策 143）先于业务模块落地：
//! [`clock::Clock`]、[`home`]（`AGENTPIPELINE_HOME`）、[`process::ProcessKiller`]、
//! [`scheduler::KanbanScheduler::tick`]（手动驱动）。

pub mod actions;
pub mod agent;
pub mod clock;
pub mod config;
pub mod error;
pub mod git;
pub mod home;
pub mod host_policy;
pub mod metrics;
pub mod notify;
pub mod pipeline;
pub mod process;
pub mod scheduler;
pub mod sse;
pub mod storage;
pub mod types;

pub use error::{Error, Result};
