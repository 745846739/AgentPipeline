//! 临时 home（决策 143 接缝② / 145）：每测试独占目录 + 独立 SQLite 文件库。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agentpipeline_core::clock::Clock;
use agentpipeline_core::home::{Home, HOME_ENV};
use agentpipeline_core::storage::Store;
use agentpipeline_core::types::{Project, ReviewMode, Task};
use agentpipeline_core::Result;
use tempfile::TempDir;

use crate::clock::ManualClock;

/// 每测试独占的家目录。
pub struct TestHome {
    dir: TempDir,
    home: Home,
}

impl TestHome {
    /// 新建：家目录在临时目录之下，不与真实 `~/.agentpipeline` 交叉。
    pub fn new() -> Result<Self> {
        let dir = tempfile::Builder::new()
            .prefix("agentpipeline-test-")
            .tempdir()
            .map_err(agentpipeline_core::Error::Io)?;
        let home = Home::new(dir.path().join("home"));
        home.ensure_dirs()?;
        Ok(TestHome { dir, home })
    }

    pub fn home(&self) -> &Home {
        &self.home
    }

    /// 家目录根路径。
    pub fn path(&self) -> &Path {
        self.home.root()
    }

    /// 临时目录根（放 fixture 仓库等）。
    pub fn scratch(&self) -> PathBuf {
        self.dir.path().join("scratch")
    }

    pub fn scratch_dir(&self, name: &str) -> PathBuf {
        let p = self.scratch().join(name);
        let _ = std::fs::create_dir_all(&p);
        p
    }

    /// 打开临时文件库并跑全量迁移（决策 145：真实行为优先）。
    pub async fn store(&self, clock: Arc<dyn Clock>) -> Result<Store> {
        Store::open(self.home.clone(), clock).await
    }

    /// 常用组合：手动时钟 + 记录型终止器 + 临时库。
    pub async fn setup(&self) -> Result<(Store, ManualClock)> {
        let clock = ManualClock::fixed();
        let store = self.store(Arc::new(clock.clone())).await?;
        Ok((store, clock))
    }

    /// 安装 `AGENTPIPELINE_HOME` 环境变量并返回自动还原的 guard。
    ///
    /// 环境变量是进程级的：并发测试使用会互相干扰。默认测试请直接传 [`Home`]，
    /// 只有验证"环境变量接缝本身"时才用本方法。
    pub fn install_env(&self) -> EnvGuard {
        let previous = std::env::var(HOME_ENV).ok();
        std::env::set_var(HOME_ENV, self.home.root());
        EnvGuard { previous }
    }
}

/// `AGENTPIPELINE_HOME` 的还原 guard。
pub struct EnvGuard {
    previous: Option<String>,
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(v) => std::env::set_var(HOME_ENV, v),
            None => std::env::remove_var(HOME_ENV),
        }
    }
}

/// 播种一个项目（测试数据的常规起点）。
pub async fn seed_project(
    store: &Store,
    project_id: &str,
    name: &str,
    local_path: &Path,
    default_branch: &str,
) -> Result<Project> {
    let project = Project {
        id: project_id.to_string(),
        name: name.to_string(),
        local_path: local_path.display().to_string(),
        default_branch: default_branch.to_string(),
        language: agentpipeline_core::git::Git::detect_language(local_path),
        test_framework: agentpipeline_core::git::Git::detect_test_framework(
            local_path,
            agentpipeline_core::git::Git::detect_language(local_path).as_deref(),
        ),
        lint_command: agentpipeline_core::git::Git::detect_lint_command(
            local_path,
            agentpipeline_core::git::Git::detect_language(local_path).as_deref(),
        ),
        agents_md_path: agentpipeline_core::git::Git::agents_md_path(local_path)
            .map(|p| p.display().to_string()),
        created_at: store.now(),
    };
    store.create_project(&project).await?;
    Ok(project)
}

/// 播种一个任务（默认无依赖）。
pub async fn seed_task(store: &Store, task_id: &str, project_id: &str) -> Result<Task> {
    seed_task_full(store, task_id, project_id, ReviewMode::Agent, &[]).await
}

/// 播种任务的完整形态（可指定 review 模式与依赖）。
pub async fn seed_task_full(
    store: &Store,
    task_id: &str,
    project_id: &str,
    review_mode: ReviewMode,
    depends_on: &[&str],
) -> Result<Task> {
    let mut new_task = agentpipeline_core::storage::tasks::NewTask::new(
        task_id,
        format!("任务 {task_id}"),
        project_id,
    );
    new_task.review_mode = review_mode;
    new_task.depends_on = depends_on.iter().map(|s| s.to_string()).collect();
    store.create_task(&new_task).await
}
