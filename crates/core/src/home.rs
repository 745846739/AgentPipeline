//! 可测试性接缝②：`AGENTPIPELINE_HOME`（决策 143）。
//!
//! 所有本机数据（db / config / prompts / tasks / worktrees / logs）都在这个根之下。
//! 家目录**不得硬编码**——测试通过环境变量指向每测试独占的临时目录，才能并行且隔离。

use std::path::{Path, PathBuf};

use crate::Result;

/// 覆盖家目录的环境变量名。
pub const HOME_ENV: &str = "AGENTPIPELINE_HOME";

/// 解析家目录：`$AGENTPIPELINE_HOME` 优先，否则 `~/.agentpipeline`。
pub fn agentpipeline_home() -> PathBuf {
    if let Ok(v) = std::env::var(HOME_ENV) {
        if !v.trim().is_empty() {
            return PathBuf::from(v);
        }
    }
    let user_home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(user_home).join(".agentpipeline")
}

/// 家目录句柄：所有路径派生的唯一入口。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home {
    root: PathBuf,
    /// `[prompts] dir` 的解析结果（§10.6.5、票 16）：`Some` 时覆盖默认
    /// `{root}/prompts`，供 executor 的 persona 覆盖查找使用；`None` 回落默认。
    prompts_override: Option<PathBuf>,
    /// `[skills] dir` 的解析结果（决策 172）：`Some` 时覆盖默认 `{root}/skills`
    /// （技能根**本身**），`None` 回落默认。
    skills_override: Option<PathBuf>,
}

impl Home {
    /// 从环境变量解析（生产与测试共用同一路径逻辑）。
    pub fn from_env() -> Self {
        Home::new(agentpipeline_home())
    }

    pub fn new(root: impl Into<PathBuf>) -> Self {
        Home {
            root: root.into(),
            prompts_override: None,
            skills_override: None,
        }
    }

    /// 设置 `[prompts] dir` 覆盖（已解析为绝对路径；`None` 回落默认目录）。
    ///
    /// 链式构造：`Home::from_env().with_prompts_dir(cfg.prompts.dir.as_deref())`。
    pub fn with_prompts_dir(mut self, dir: Option<impl Into<PathBuf>>) -> Self {
        self.prompts_override = dir.map(Into::into);
        self
    }

    /// 设置 `[skills] dir` 覆盖（已解析为绝对路径；`None` 回落默认技能根）。
    ///
    /// 链式构造：`Home::from_env().with_skills_dir(cfg.skills.dir.as_deref())`。
    pub fn with_skills_dir(mut self, dir: Option<impl Into<PathBuf>>) -> Self {
        self.skills_override = dir.map(Into::into);
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    pub fn data_dir(&self) -> PathBuf {
        self.root.join("data")
    }

    /// SQLite 数据库文件（决策 17 文件名；§12.14 要求 0600）。
    pub fn db_path(&self) -> PathBuf {
        self.data_dir().join("agentpipeline.db")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// 用户可覆盖的 prompt 模板目录（决策 7）。
    ///
    /// 票 16：`[prompts] dir` 设置时返回该覆盖目录（`prompts_root` 兜底默认），
    /// 否则回落 `{root}/prompts`。
    pub fn prompts_dir(&self) -> PathBuf {
        crate::agent::prompts::prompts_root(
            &self.root.join("prompts"),
            self.prompts_override.as_deref(),
        )
    }

    /// 技能根（决策 170 / 172）：默认 `{root}/skills`，`[skills] dir` 设置时整体替换。
    ///
    /// 返回的是技能根**本身**（其下直接是 `{name}/SKILL.md`）——调用方不再拼
    /// `skills` 目录名。内嵌技能已随决策 172① 退场（票 04），这里是技能的唯一来源。
    pub fn skills_dir(&self) -> PathBuf {
        self.skills_override
            .clone()
            .unwrap_or_else(|| self.root.join(crate::agent::skills::SKILLS_DIR))
    }

    pub fn tasks_dir(&self) -> PathBuf {
        self.root.join("tasks")
    }

    /// 任务目录（设计文档等产出，**不在 worktree 内**）。
    pub fn task_dir(&self, task_id: &str) -> PathBuf {
        self.tasks_dir().join(task_id)
    }

    /// 任务目录下的上下文卸载目录（§12.13 L2）。
    pub fn context_dir(&self, task_id: &str) -> PathBuf {
        self.task_dir(task_id).join(".context")
    }

    /// 值班长会话的卸载目录（决策 204④ / 206）。
    ///
    /// **必须在 `tasks/` 之外**：值班长没有 task_id，若让它的卸载走 `context_dir("")`，
    /// 落点就是 `{root}/tasks/.context`——那是**所有任务共用**的那一层，下一次运行任意
    /// 一个真实任务时会把它读成自己的工作区残留（`agent/tools.rs` 的 `apply_l2_offload`
    /// 注释点名过这个后果）。会话维度既落在 `tasks/` 之外，又与「命令日志挂会话」
    /// 是同一条归属。
    pub fn foreman_context_dir(&self, session_id: &str) -> PathBuf {
        self.root.join("foreman").join("context").join(session_id)
    }

    pub fn worktrees_dir(&self) -> PathBuf {
        self.root.join("worktrees")
    }

    /// 任务 worktree 路径（§6 Worktree 约定）。
    pub fn worktree_path(&self, task_id: &str) -> PathBuf {
        self.worktrees_dir().join(task_id)
    }

    /// **修复 worktree** 的路径（决策 210③ / 票 10）：`{home}/worktrees/repair-{id}`。
    ///
    /// 落在家目录下是这一票的**硬约束**，不是审美：值班长的写域是 `home.root()`，
    /// 而 `FileToolPolicy` 对读写都强制「路径必须落在允许根内」——本仓（以及任何在
    /// `~/Documents` 下的项目）它一个字都写不了。落在 `{home}/worktrees/` 下才可达。
    /// 前缀 `repair-` 让它与任务 worktree（`worktrees/{task_id}`）在目录列表里一眼可分。
    pub fn repair_worktree_path(&self, repair_id: &str) -> PathBuf {
        self.worktrees_dir().join(format!("repair-{repair_id}"))
    }

    /// 任务产出的既有文件路径集合（§4.2 文件目录约定）。
    pub fn task_file(&self, task_id: &str, name: &str) -> PathBuf {
        self.task_dir(task_id).join(name)
    }

    /// 建立家目录骨架；权限收紧到 0700（§12.14）。
    ///
    /// 建的是**默认**技能根 `{root}/skills`，不是 `skills_dir()` 的返回值：`[skills] dir`
    /// 常指到用户自己维护的生态目录（如 `~/.zcode/skills`），本系统不该新建它、更不该
    /// 改它的权限（决策 172）。覆盖目录里没有技能时只是「没有可用技能」。
    pub fn ensure_dirs(&self) -> Result<()> {
        for dir in [
            self.root.clone(),
            self.data_dir(),
            self.logs_dir(),
            self.prompts_dir(),
            self.root.join(crate::agent::skills::SKILLS_DIR),
            self.tasks_dir(),
            self.worktrees_dir(),
        ] {
            std::fs::create_dir_all(&dir)?;
            restrict_permissions(&dir);
        }
        Ok(())
    }

    pub fn ensure_task_dirs(&self, task_id: &str) -> Result<()> {
        for dir in [
            self.task_dir(task_id),
            self.context_dir(task_id),
            self.worktree_path(task_id),
        ] {
            std::fs::create_dir_all(&dir)?;
            restrict_permissions(&dir);
        }
        Ok(())
    }
}

/// 把路径权限收紧为 0700（仅属主可进入）。非 unix 平台为空实现。
#[cfg(unix)]
pub fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
pub fn restrict_permissions(_path: &Path) {}

/// 把**文件**权限收紧为 0600（仅属主可读写；§12.14 对 db 的要求）。
#[cfg(unix)]
pub fn restrict_file_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
pub fn restrict_file_permissions(_path: &Path) {}

/// 检查家目录权限是否过宽（§12.14：过宽则告警，**不阻断启动**）。
///
/// 返回过宽的路径列表；空表示合规。目录 0700、db 及 `-wal` / `-shm` 0600。
#[cfg(unix)]
pub fn check_permissions(home: &Home) -> Vec<(PathBuf, u32)> {
    use std::os::unix::fs::PermissionsExt;
    let mut wide = Vec::new();
    // realpath（决策 104 注）：macOS 上 /tmp → /private/tmp，不解析会让检查落空
    let check = |wide: &mut Vec<(PathBuf, u32)>, path: &Path, limit: u32| {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if let Ok(meta) = std::fs::metadata(&path) {
            let mode = meta.permissions().mode() & 0o777;
            if mode & !limit != 0 {
                wide.push((path, mode));
            }
        }
    };
    for dir in [
        home.root().to_path_buf(),
        home.data_dir(),
        home.tasks_dir(),
        home.logs_dir(),
    ] {
        check(&mut wide, &dir, 0o700);
    }
    let db = home.db_path();
    let db_name = db
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    for file in [
        db.clone(),
        home.data_dir().join(format!("{db_name}-wal")),
        home.data_dir().join(format!("{db_name}-shm")),
    ] {
        check(&mut wide, &file, 0o600);
    }
    wide
}

#[cfg(not(unix))]
pub fn check_permissions(_home: &Home) -> Vec<(PathBuf, u32)> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_env_overrides_default() {
        // 该测试只读派生逻辑，不设置全局环境变量（避免并行测试互相干扰）。
        let home = Home::new("/tmp/xyz-home");
        assert_eq!(
            home.db_path(),
            PathBuf::from("/tmp/xyz-home/data/agentpipeline.db")
        );
        assert_eq!(home.task_dir("t1"), PathBuf::from("/tmp/xyz-home/tasks/t1"));
        assert_eq!(
            home.worktree_path("t1"),
            PathBuf::from("/tmp/xyz-home/worktrees/t1")
        );
        assert_eq!(
            home.context_dir("t1"),
            PathBuf::from("/tmp/xyz-home/tasks/t1/.context")
        );
    }

    #[test]
    fn ensure_dirs_creates_skeleton() {
        let tmp = tempfile::tempdir().unwrap();
        let home = Home::new(tmp.path());
        home.ensure_dirs().unwrap();
        assert!(home.data_dir().is_dir());
        assert!(home.prompts_dir().is_dir());
        assert!(home.tasks_dir().is_dir());
        assert!(home.worktrees_dir().is_dir());
    }

    #[test]
    fn prompts_dir_falls_back_to_home_prompts() {
        let home = Home::new("/tmp/xyz-home");
        assert_eq!(home.prompts_dir(), PathBuf::from("/tmp/xyz-home/prompts"));
    }

    #[test]
    fn prompts_dir_override_wins_over_default() {
        // 票 16：`[prompts] dir` 覆盖后，executor 经 home.prompts_dir() 读覆盖目录
        let home = Home::new("/tmp/xyz-home").with_prompts_dir(Some("/custom/prompts"));
        assert_eq!(home.prompts_dir(), PathBuf::from("/custom/prompts"));

        // None 回落默认
        let home = Home::new("/tmp/xyz-home").with_prompts_dir(None::<PathBuf>);
        assert_eq!(home.prompts_dir(), PathBuf::from("/tmp/xyz-home/prompts"));
    }

    #[test]
    fn skills_dir_falls_back_to_home_skills() {
        // 决策 172：未配置时技能根仍是 `{home}/skills`，行为逐字不变
        let home = Home::new("/tmp/xyz-home");
        assert_eq!(home.skills_dir(), PathBuf::from("/tmp/xyz-home/skills"));
    }

    #[test]
    fn skills_dir_override_wins_over_default() {
        // `[skills] dir` 覆盖后，executor 与启动校验都经 home.skills_dir() 取技能根
        let home = Home::new("/tmp/xyz-home").with_skills_dir(Some("/custom/skills"));
        assert_eq!(home.skills_dir(), PathBuf::from("/custom/skills"));

        // None 回落默认
        let home = Home::new("/tmp/xyz-home").with_skills_dir(None::<PathBuf>);
        assert_eq!(home.skills_dir(), PathBuf::from("/tmp/xyz-home/skills"));
    }

    #[test]
    fn ensure_dirs_never_touches_overridden_skills_dir() {
        // 决策 172：覆盖目录是用户自己的生态目录（`~/.zcode/skills`），家目录骨架
        // 不得新建它、更不得改它的权限——只建默认技能根。
        let tmp = tempfile::tempdir().unwrap();
        let external = tmp.path().join("external-skills");
        let home = Home::new(tmp.path().join("home")).with_skills_dir(Some(external.clone()));
        home.ensure_dirs().unwrap();

        assert!(
            !external.exists(),
            "覆盖的技能根不得被 ensure_dirs 创建：{}",
            external.display()
        );
        assert!(home.root().join("skills").is_dir(), "默认技能根仍应建立");
    }

    #[cfg(unix)]
    #[test]
    fn ensure_dirs_restricts_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let home = Home::new(tmp.path().join("home"));
        home.ensure_dirs().unwrap();
        let mode = std::fs::metadata(home.data_dir())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);
    }
}
