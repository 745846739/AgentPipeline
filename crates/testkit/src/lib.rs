//! AgentPipeline 测试基建（决策 146 / 148）。
//!
//! - [`TestHome`]：`AGENTPIPELINE_HOME` 指向每测试独占临时目录 + 全量迁移的临时文件库
//!   （决策 143 / 145）；
//! - [`ManualClock`]：假时钟，手动推进（决策 143 接缝①）；
//! - [`RecordingKiller`]：进程组终止器替身，只记录不真杀（接缝③）；
//! - [`git_fixture::Repo`]：系统 git CLI 搭建的场景仓库（决策 146）；
//! - [`FakeAgent`]：脚本化 LLM 替身，**只替换 LLM 响应流，工具层真实执行**（决策 148）；
//! - [`SseRecorder`] / 断言助手：事件序列与游标 / run 计数断言。

pub mod assertions;
pub mod clock;
pub mod git_fixture;
pub mod home;
pub mod killer;
pub mod mock_llm;
pub mod script;
pub mod skill_fixture;

pub use assertions::{
    assert_cursor_at, backdate_run, command_count, cursor_counts, live_cursor_for_branch,
    llm_run_count, run_count, SseRecorder,
};
pub use clock::ManualClock;
pub use git_fixture::{Language, Repo};
pub use home::{seed_project, seed_task, seed_task_full, EnvGuard, TestHome};
pub use killer::RecordingKiller;
pub use mock_llm::{MockLlm, MockRoute, RecordedRequest};
pub use script::{FakeAgent, Script, Step};
pub use skill_fixture::{skill_zip, write_raw_skill_dir, write_skill_dir, zip_bytes};
