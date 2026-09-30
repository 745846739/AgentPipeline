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
/// 命令执行的唯一收口（决策 297）：启动 → 采集 → 超时收口 → 脱敏 → 台账。
pub mod exec;
pub mod git;
pub mod home;
pub mod host_policy;
/// 打断策略原语（决策 355）：「一个事件只打扰人一次」的三件原语——
/// 去抖窗口、按主体冷却、小时上限 + 上限通知去重（另加指数退避）。
/// 纯函数、无 I/O，watch 与 notify 各当一个适配器。
pub mod interrupt;
pub mod metrics;
pub mod notify;
pub mod pipeline;
pub mod process;
/// rtk（Rust Token Killer）的改写与可用性（决策 297）。
pub mod rtk;
pub mod scheduler;
pub mod sse;
pub mod storage;
pub mod types;
/// 浏览器推送的密码学三件（spec `.scratch/pwa-webpush/` 票 02）：RFC 8291 的报文加密、
/// RFC 8188 的 `aes128gcm` 记录、RFC 8292 的 VAPID 鉴权。原语走 ring（已在依赖树里），
/// 组装那几行由 RFC 的公开测试向量钉住（`webpush.rs` 的 KAT）。
pub mod webpush;

pub use error::{Error, Result};
