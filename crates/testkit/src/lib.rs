//! AgentPipeline 测试基建（决策 146 / 148）。
//!
//! - [`TestHome`]：`AGENTPIPELINE_HOME` 指向每测试独占临时目录 + 全量迁移的临时文件库
//!   （决策 143 / 145）；
//! - [`ManualClock`]：假时钟，手动推进（决策 143 接缝①）；
//! - [`RecordingKiller`]：进程组终止器替身，默认只记录不真杀，`with_real_kill` 可记账 + 真收口（接缝③）；
//! - [`git_fixture::Repo`]：系统 git CLI 搭建的场景仓库（决策 146）；
//! - [`FakeAgent`]：脚本化 LLM 替身，**只替换 LLM 响应流，工具层真实执行**（决策 148）；
//! - [`repo_fixture`]：技能来源仓的离线 fixture（真 libgit2 打本地裸仓 / 离线 smart HTTP，
//!   决策 194）；
//! - [`repo_fixture::RepoFixture`] / [`repo_fixture::SmartHttp`]：GitHub 来源的离线 fixture
//!   ——临时裸仓 + 离线 smart HTTP（决策 194，票 01）；[`repo_fixture::RemoteBehaviour`] 另有两种
//!   远端形态（私有仓 401 / 坏包），供八类失败里打不到的两类当可测输入（票 23）；
//! - [`SseRecorder`] / 断言助手：事件序列与游标 / run 计数断言。

pub mod assertions;
pub mod clock;
pub mod git_fixture;
pub mod home;
pub mod killer;
pub mod mock_llm;
pub mod repo_fixture;
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
pub use repo_fixture::{RemoteBehaviour, RepoFixture, SmartHttp};
pub use script::{FakeAgent, Script, Step};
pub use skill_fixture::{skill_zip, write_raw_skill_dir, write_skill_dir, zip_bytes};
