//! 文件工具路径策略（决策 104）——**不是系统级沙箱**。
//!
//! 只约束 6 个文件工具（write / edit / read / delete / list_dir + 任务目录写入）；
//! `run_command` 的 shell 不受限（决策 19 已修订，残余风险见 §9 / §12.14）。
//!
//! 四条规则：workdir_bound（允许根）、deny_paths（拒绝名单）、判定前 realpath 解析、
//! 拒绝写符号链接。deny 优先于 allow。

use std::path::{Component, Path, PathBuf};

use crate::{Error, Result};

/// 文件工具读 / 写操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOp {
    Read,
    Write,
}

/// 阶段写入面白名单的一条（决策 395）：`root` 之下、文件名（或相对路径）命中
/// `pattern` 的写入才放行。**只管写**——读不受它约束（review 要读 worktree 里的代码）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteAllow {
    /// 允许写入的根（任务目录或 worktree 的绝对路径）。
    pub root: PathBuf,
    /// 相对该根的模式（复用 [`matches_pattern`] 的轻量 glob 语义：不含 `/` 的模式按
    /// 文件名匹配；`*` = 该根下任意文件）。
    pub pattern: String,
}

/// 文件工具路径策略（`SystemBaseline.file_tool_policy`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileToolPolicy {
    /// 文件工具的允许根：worktree + 任务目录。
    pub workdir_bound: Vec<PathBuf>,
    /// 禁止访问的路径模式。
    pub deny_paths: Vec<String>,
    /// **写入面正向白名单**（决策 395）：非空时，写操作必须命中其中一条才放行；
    /// 空 = 不限制（值班长 / 子代理 / 决策 395 之前的形状）。
    ///
    /// 它与 `workdir_bound` 管的不是同一件事：允许根管「这台机器上文件工具够得着哪」
    /// （决策 104），白名单管「**这个阶段**的 agent 允许改动什么」（决策 395 的内容
    /// 边界）——后者是阶段纪律，不随 `file_access_unrestricted`（决策 283）放开：
    /// 部署侧放宽操作范围，不等于 review 获得了改代码的授权。
    pub allow_writes: Vec<WriteAllow>,
    /// 判定前对目标路径做 realpath 解析（macOS /etc → /private/etc、/tmp → /private/tmp）。
    pub resolve_realpath: bool,
    /// 拒绝写符号链接。
    pub refuse_symlink_write: bool,
}

impl Default for FileToolPolicy {
    fn default() -> Self {
        FileToolPolicy {
            workdir_bound: Vec::new(),
            deny_paths: default_deny_paths(),
            allow_writes: Vec::new(),
            resolve_realpath: true,
            refuse_symlink_write: true,
        }
    }
}

/// 默认拒绝名单（决策 104 / agents.md §10.6.2）。
///
/// **模式**名单（按文件名匹配）——它盖不住这一条：`{home}/data/agentpipeline.db`
/// 里明文存着 provider 密钥（决策 112），而 `.db` 不是 `.env` 也不是 `.pem`。
/// 那一条由 [`foreman_file_policy`] 按**路径前缀**补上。
pub fn default_deny_paths() -> Vec<String> {
    vec![
        ".env*".to_string(),
        "*.pem".to_string(),
        "*.key".to_string(),
        "id_rsa*".to_string(),
        "~/.ssh".to_string(),
    ]
}

/// 值班长的文件域与补偿（决策 206；其中 `logs/` 一条由决策 226 撤销）：
/// 域 = `home.root()`，按**路径前缀**拒掉 `{root}/data`。
///
/// * `data/`——`agentpipeline.db` 明文存 provider 密钥（决策 112）。**这是补偿，
///   不是边界**：`run_command` 不受文件策略管（命令自己 `cd` 就出去了），故它只挡住
///   「用文件工具顺手读走密钥」这一条路，`auto` 档下的命令那条路**无补偿**——决策 206
///   已把这条残余风险登记在案（`docs/operations.md` 的残余风险表）。**密钥是秘密，
///   这一条留着**。
/// * `logs/`——**决策 226 撤掉了这一条**。它原来的理由没错：这台机器的日志价值不在秘密
///   而在体量，一个 200MB 的文件进上下文的代价远大于它能回答的问题。错的是**手段**——
///   体量是**可以结构地**管的（`read_file` 现在按
///   [`crate::agent::context::READ_FILE_MAX_BYTES`] 有界读、并支持 `tail`），而一堵按前缀
///   拦的墙会把 877 字节的日志一起挡在外面。实测代价就在那里：定死 2026-09-19 那次僵死
///   根因的那一行（`resume 触发被在跑的 executor 持续挡下，放弃本次触发`）住在一个 877
///   字节的日志里，值班长读不到它，于是连报四轮、从台账反推症状。日志里没有密钥
///   （决策 112 的密钥只在库里与配置里），故撤掉它不新增秘密面。
///
/// **前缀语义**是靠 `matches_pattern` 的既有规则给的：含 `/` 且不含 `*` 的模式按
/// 「等于它或落在它之下」判定。值是**绝对路径**，故 `data` 这个目录名在别处出现
/// （比如某个项目自己有个 `data/`）不受影响——收的是这一个，不是所有同名目录。
pub fn foreman_file_policy(home_root: &Path, unrestricted: bool) -> FileToolPolicy {
    let mut deny = default_deny_paths();
    deny.push(home_root.join("data").display().to_string());
    FileToolPolicy {
        workdir_bound: if unrestricted {
            Vec::new()
        } else {
            vec![home_root.to_path_buf()]
        },
        deny_paths: deny,
        ..Default::default()
    }
}

/// 流水线节点 / 子代理的文件域（决策 104）：缺省两个根 = worktree + 任务目录。
///
/// `unrestricted`（`[pipeline] file_access_unrestricted`，决策 283）时**允许根清空**——
/// 文件工具可以读写任何路径，拒绝名单照旧生效。
///
/// 为什么是一个旋钮而不是把两根直接删掉：默认姿态（文件工具锁在任务域内）是本仓对外的
/// 一半设计（决策 104），而「这台机器上要让 agent 像本地开发一样随手读写」是一个**部署
/// 决定**——两个方向的读者不同，故由配置分开。它同时收掉一处自相矛盾：命令那条路从来
/// 不受文件策略管（决策 104 / 19 修订自认不是系统级沙箱），于是此前「用 `read_file` 读不到
/// 的路径，用 `run_command cat` 读得到」——同一个 agent 的两只手，一只被绑着。
pub fn pipeline_file_policy(
    worktree: &Path,
    task_dir: &Path,
    unrestricted: bool,
) -> FileToolPolicy {
    if unrestricted {
        return FileToolPolicy {
            workdir_bound: Vec::new(),
            ..Default::default()
        };
    }
    FileToolPolicy::new(vec![worktree.to_path_buf(), task_dir.to_path_buf()])
}

impl FileToolPolicy {
    pub fn new(roots: Vec<PathBuf>) -> Self {
        FileToolPolicy {
            workdir_bound: roots,
            ..Default::default()
        }
    }

    /// 校验并返回解析后的绝对路径。拒绝时返回 [`Error::PolicyDenied`]。
    pub fn check(&self, path: &Path, op: FileOp) -> Result<PathBuf> {
        let resolved = if self.resolve_realpath {
            resolve(path)
        } else {
            absolutize(path)
        };

        // ① deny 优先于 allow：先查拒绝名单（原始与解析后两个形态都查）
        for candidate in [path, resolved.as_path()] {
            if let Some(pattern) = self.denied_pattern(candidate) {
                return Err(Error::PolicyDenied(format!(
                    "路径命中拒绝名单（{pattern}）：{}",
                    candidate.display()
                )));
            }
        }

        // ② 拒绝写符号链接：已存在符号链接按解析后的目标判定，堵住逃逸
        if op == FileOp::Write && self.refuse_symlink_write && is_symlink(path) {
            return Err(Error::PolicyDenied(format!(
                "拒绝写符号链接：{}",
                path.display()
            )));
        }

        // ③ workdir_bound：解析后的路径必须落在某个允许根之内
        if !self.workdir_bound.is_empty() {
            let roots: Vec<PathBuf> = self
                .workdir_bound
                .iter()
                .map(|r| {
                    if self.resolve_realpath {
                        resolve(r)
                    } else {
                        absolutize(r)
                    }
                })
                .collect();
            if !roots.iter().any(|root| resolved.starts_with(root)) {
                return Err(Error::PolicyDenied(format!(
                    "路径超出文件工具允许范围：{}",
                    resolved.display()
                )));
            }
        }

        // ④ 写入面正向白名单（决策 395）：非空时写 op 必须命中其中一条。
        // 排在允许根之后——「够得着」（允许根）与「允许改」（白名单）是两道门，
        // 报错时把允许面一并说清，让 agent 当轮自纠（回灌自愈的既有姿态）。
        if op == FileOp::Write && !self.allow_writes.is_empty() {
            let hit = self.allow_writes.iter().any(|a| {
                let root = if self.resolve_realpath {
                    resolve(&a.root)
                } else {
                    absolutize(&a.root)
                };
                resolved.starts_with(&root) && matches_pattern(&a.pattern, &resolved)
            });
            if !hit {
                let allowed = self
                    .allow_writes
                    .iter()
                    .map(|a| format!("{} 下的 {}", a.root.display(), a.pattern))
                    .collect::<Vec<_>>()
                    .join("；");
                return Err(Error::PolicyDenied(format!(
                    "路径不在本节点的写入面白名单内（决策 395）：{}。本节点允许写入：{allowed}",
                    resolved.display()
                )));
            }
        }

        Ok(resolved)
    }

    /// 读 / 写便捷入口。
    pub fn check_read(&self, path: &Path) -> Result<PathBuf> {
        self.check(path, FileOp::Read)
    }

    pub fn check_write(&self, path: &Path) -> Result<PathBuf> {
        self.check(path, FileOp::Write)
    }

    fn denied_pattern(&self, path: &Path) -> Option<String> {
        self.deny_paths
            .iter()
            .find(|p| matches_pattern(p, path))
            .cloned()
    }
}

/// realpath 解析：存在则 canonicalize；不存在则解析最深的已存在祖先再拼回剩余部分。
///
/// 这一步是必须的——macOS 上 `/tmp` 是指向 `/private/tmp` 的符号链接，不解析会让
/// `deny_paths` 静默失效（决策 104）。
pub fn resolve(path: &Path) -> PathBuf {
    let absolute = absolutize(path);
    if let Ok(canon) = absolute.canonicalize() {
        return canon;
    }
    // 逐级向上找已存在的祖先
    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    let mut cur = absolute.clone();
    loop {
        match cur.canonicalize() {
            Ok(canon) => {
                let mut out = canon;
                for part in suffix.iter().rev() {
                    out.push(part);
                }
                return out;
            }
            Err(_) => match (cur.parent(), cur.file_name()) {
                (Some(parent), Some(name)) if parent != cur => {
                    suffix.push(name.to_os_string());
                    cur = parent.to_path_buf();
                }
                _ => return absolute,
            },
        }
    }
}

/// 相对路径按当前工作目录补全；不解析符号链接。
pub fn absolutize(path: &Path) -> PathBuf {
    if path.is_absolute() {
        normalize(path)
    } else {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        normalize(&cwd.join(path))
    }
}

/// 纯词法归一化（不触碰文件系统）：消掉 `.` 与 `..`。
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// 轻量 glob 匹配：支持 `*`（任意串）、前缀 / 后缀 / 中缀模式，以及 `~` 家目录前缀。
pub fn matches_pattern(pattern: &str, path: &Path) -> bool {
    let expanded = if let Some(rest) = pattern.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() {
            format!("{}/{rest}", home.trim_end_matches('/'))
        } else {
            pattern.to_string()
        }
    } else {
        pattern.to_string()
    };

    let path_str = absolutize(path).to_string_lossy().to_string();
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    // 不含 `/` 的模式只比对文件名；含 `/` 的比对整路径前缀/子串
    if !expanded.contains('/') {
        return glob_match(&expanded, &file_name);
    }

    // `~/.ssh` 这类目录模式：既是前缀也匹配其子路径
    if let Some(prefix) = expanded.strip_suffix("/*") {
        return path_str.starts_with(prefix) || path_str.contains(&expanded);
    }
    if expanded.contains('*') {
        return glob_match(&expanded, &path_str) || path_str.contains(&expanded);
    }
    path_str == expanded || path_str.starts_with(&format!("{expanded}/"))
}

/// 单模式 glob：按 `*` 切分后顺序匹配。
fn glob_match(pattern: &str, text: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == text;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut cursor = text;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            // 首段必须前缀匹配
            if !cursor.starts_with(part) {
                return false;
            }
            cursor = &cursor[part.len()..];
        } else if i == parts.len() - 1 {
            // 末段必须后缀匹配
            return cursor.ends_with(part);
        } else if let Some(pos) = cursor.find(part) {
            cursor = &cursor[pos + part.len()..];
        } else {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy_for(root: &Path) -> FileToolPolicy {
        FileToolPolicy::new(vec![root.to_path_buf()])
    }

    #[test]
    fn allows_path_inside_workdir_bound() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let policy = policy_for(&root);
        let file = root.join("src/main.rs");
        let resolved = policy.check_write(&file).unwrap();
        assert!(resolved.starts_with(&root));
    }

    #[test]
    fn denies_path_outside_workdir_bound() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let policy = policy_for(&root);
        let err = policy
            .check_read(&outside.path().join("x.txt"))
            .unwrap_err();
        assert!(matches!(err, Error::PolicyDenied(_)));
    }

    #[test]
    fn denies_env_pem_and_ssh_key_patterns() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let policy = policy_for(&root);
        for name in [
            ".env",
            ".env.local",
            "server.pem",
            "private.key",
            "id_rsa",
            "id_rsa.pub",
        ] {
            let err = policy.check_read(&root.join(name)).unwrap_err();
            assert!(
                matches!(err, Error::PolicyDenied(_)),
                "{name} 应被拒绝，实际：{err:?}"
            );
        }
    }

    #[test]
    fn the_foreman_root_denies_the_key_store_but_not_logs() {
        // 决策 226：`data/` 按前缀拒（密钥在那里），`logs/` **不再拒**——体量由
        // `read_file` 的字节上限管，按前缀拦会把 877 字节的日志一起挡在外面。
        // 前缀语义 = 「等于它或落在它之下」，故两个目录里的文件与目录本身都要断言到。
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let policy = foreman_file_policy(&root, false);
        let data = root.join("data");
        let logs = root.join("logs");
        for path in [data.clone(), data.join("agentpipeline.db")] {
            assert!(
                policy.check_read(&path).is_err(),
                "{} 必须在拒绝名单里",
                path.display()
            );
            assert!(policy.check_write(&path).is_err());
        }
        for path in [logs.clone(), logs.join("agentpipeline.log")] {
            assert!(
                policy.check_read(&path).is_ok(),
                "{} 应读得到（决策 226 撤销了那条拒绝）",
                path.display()
            );
            assert!(policy.check_write(&path).is_ok());
        }
        // 同名目录在别处不受影响：收的是这一个前缀，不是所有叫 data 的目录
        assert!(policy
            .check_read(&root.join("project/data/keep.json"))
            .is_ok());
    }

    #[test]
    fn deny_beats_allow() {
        // .env 落在允许根**之内**，仍必须被拒绝（deny 优先）
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let policy = policy_for(&root);
        assert!(policy.check_read(&root.join("src/lib.rs")).is_ok());
        assert!(policy.check_read(&root.join(".env")).is_err());
    }

    /// 决策 283：`file_access_unrestricted` 清空**允许根**，拒绝名单照旧生效。
    ///
    /// 三件事都要钉住：① 缺省仍锁在两根内（默认姿态不变）；② 打开后根外可读可写；
    /// ③ 打开后 `.env` / 密钥那几条仍然拒——秘密保护与操作范围不共用这个旋钮。
    #[test]
    fn unrestricted_pipeline_policy_drops_the_roots_but_keeps_the_deny_list() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path().canonicalize().unwrap();
        let task_dir = worktree.join("task");
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().canonicalize().unwrap().join("x.txt");

        // ① 缺省：根外拒绝，根内放行
        let bounded = pipeline_file_policy(&worktree, &task_dir, false);
        assert!(bounded.check_read(&outside_file).is_err());
        assert!(bounded.check_write(&worktree.join("src/main.rs")).is_ok());

        // ② 打开：根外可读可写
        let open = pipeline_file_policy(&worktree, &task_dir, true);
        assert!(open.check_read(&outside_file).is_ok());
        assert!(open.check_write(&outside_file).is_ok());

        // ③ 拒绝名单不受开关影响
        assert!(open.check_read(&worktree.join(".env")).is_err());
        assert!(open.check_read(&worktree.join("server.pem")).is_err());
        assert!(open.check_write(&worktree.join("id_rsa")).is_err());
    }

    /// 决策 283：值班长的家目录根由同一个开关决定，`data/` 的前缀拒绝照旧。
    #[test]
    fn unrestricted_foreman_policy_keeps_the_key_store_denied() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let policy = foreman_file_policy(&root, true);
        assert!(policy
            .check_read(&outside.path().canonicalize().unwrap().join("y.txt"))
            .is_ok());
        assert!(policy
            .check_read(&root.join("data/agentpipeline.db"))
            .is_err());
    }

    #[test]
    fn refuses_symlink_write_but_allows_read() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let target = root.join("real.txt");
        std::fs::write(&target, "x").unwrap();
        let link = root.join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let policy = policy_for(&root);
        assert!(policy.check_read(&link).is_ok());
        let err = policy.check_write(&link).unwrap_err();
        assert!(matches!(err, Error::PolicyDenied(_)));
    }

    #[test]
    fn symlink_escaping_workdir_is_denied_after_realpath() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "s").unwrap();
        let escape = root.join("escape.txt");
        std::os::unix::fs::symlink(outside.path().join("secret.txt"), &escape).unwrap();

        let policy = policy_for(&root);
        // 解析后落在允许根之外 → 拒绝（realpath 解析堵住逃逸）
        assert!(policy.check_read(&escape).is_err());
    }

    #[test]
    fn realpath_resolves_symlinked_prefix() {
        // macOS：/tmp → /private/tmp。用系统真实情况断言，不硬编码平台。
        let canonical_tmp = std::path::Path::new("/tmp").canonicalize().unwrap();
        let resolved = resolve(std::path::Path::new("/tmp/agentpipeline-probe"));
        assert_eq!(
            resolved,
            canonical_tmp.join("agentpipeline-probe"),
            "realpath 应把符号链接前缀解析掉"
        );
        if std::path::Path::new("/tmp")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
        {
            assert_ne!(
                resolved,
                std::path::PathBuf::from("/tmp/agentpipeline-probe")
            );
        }
    }

    #[test]
    fn realpath_via_tempdir_matches_canonicalize() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("a/b/c.txt");
        assert_eq!(
            resolve(&nested),
            tmp.path().canonicalize().unwrap().join("a/b/c.txt")
        );
        assert_eq!(
            resolve(tmp.path()).canonicalize().unwrap(),
            tmp.path().canonicalize().unwrap()
        );
    }

    #[test]
    fn tilde_ssh_pattern_matches_home_path() {
        let home = std::env::var("HOME").unwrap();
        assert!(matches_pattern(
            "~/.ssh",
            &PathBuf::from(&home).join(".ssh/id_rsa")
        ));
        assert!(matches_pattern(
            "~/.ssh",
            &PathBuf::from(&home).join(".ssh")
        ));
        assert!(!matches_pattern("~/.ssh", &PathBuf::from("/tmp/other/ssh")));
    }

    #[test]
    fn glob_matcher_basics() {
        assert!(glob_match(".env*", ".env"));
        assert!(glob_match(".env*", ".env.production"));
        assert!(!glob_match(".env*", "env"));
        assert!(glob_match("*.pem", "a.pem"));
        assert!(!glob_match("*.pem", "a.pemx"));
        assert!(glob_match("id_rsa*", "id_rsa.pub"));
        assert!(!glob_match("id_rsa*", "xid_rsa"));
    }

    #[test]
    fn empty_workdir_bound_means_no_root_check() {
        let policy = FileToolPolicy {
            workdir_bound: vec![],
            ..Default::default()
        };
        assert!(policy
            .check_read(std::path::Path::new("/somewhere/else.txt"))
            .is_ok());
    }

    // ── 决策 395：写入面正向白名单 ──

    /// 白名单为空的旧形状行为不变：允许根内写放行。
    #[test]
    fn empty_allow_writes_keeps_the_old_shape() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let policy = policy_for(&root);
        assert!(policy.check_write(&root.join("any/thing.txt")).is_ok());
    }

    /// review 的形状：任务目录只许写 `review-report.md`，其余任务目录文件与
    /// worktree 全部拒。
    #[test]
    fn single_file_allowlist_denies_everything_else() {
        let tmp = tempfile::tempdir().unwrap();
        let task_dir = tmp.path().canonicalize().unwrap();
        let wt_tmp = tempfile::tempdir().unwrap();
        let worktree = wt_tmp.path().canonicalize().unwrap();
        let policy = FileToolPolicy {
            workdir_bound: vec![worktree.clone(), task_dir.clone()],
            allow_writes: vec![WriteAllow {
                root: task_dir.clone(),
                pattern: "review-report.md".into(),
            }],
            ..Default::default()
        };
        // 命中白名单 → 放行
        assert!(policy
            .check_write(&task_dir.join("review-report.md"))
            .is_ok());
        // 任务目录其他文件 → 拒（报错里带允许面，agent 可自纠）
        let err = policy.check_write(&task_dir.join("notes.md")).unwrap_err();
        assert!(matches!(err, Error::PolicyDenied(ref m) if m.contains("review-report.md")));
        // worktree 里的文件（绝对路径构造，模拟 `write_root_for` 之外的落点）→ 拒
        assert!(policy.check_write(&worktree.join("src/lib.rs")).is_err());
        // 读不受白名单管：review 要读 worktree 代码
        assert!(policy.check_read(&worktree.join("src/lib.rs")).is_ok());
    }

    /// develop 的形状：只许写 worktree；任务目录零条目 = 拒（绝对路径也绕不过）。
    #[test]
    fn worktree_only_allowlist_blocks_the_task_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let task_dir = tmp.path().canonicalize().unwrap();
        let wt_tmp = tempfile::tempdir().unwrap();
        let worktree = wt_tmp.path().canonicalize().unwrap();
        let policy = FileToolPolicy {
            workdir_bound: vec![worktree.clone(), task_dir.clone()],
            allow_writes: vec![WriteAllow {
                root: worktree.clone(),
                pattern: "*".into(),
            }],
            ..Default::default()
        };
        assert!(policy.check_write(&worktree.join("src/main.rs")).is_ok());
        assert!(policy
            .check_write(&worktree.join("tests/deep/nested_test.rs"))
            .is_ok());
        assert!(policy.check_write(&task_dir.join("scratch.md")).is_err());
    }

    /// deny 名单优先于白名单：`.env` 即使命中白名单根也必须拒（顺序不变量）。
    #[test]
    fn deny_paths_beat_the_write_allowlist() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let policy = FileToolPolicy {
            workdir_bound: vec![root.clone()],
            allow_writes: vec![WriteAllow {
                root: root.clone(),
                pattern: "*".into(),
            }],
            ..Default::default()
        };
        assert!(policy.check_write(&root.join(".env")).is_err());
        assert!(policy.check_write(&root.join("src/lib.rs")).is_ok());
    }

    /// 符号链接逃逸在白名单下同样不成立：realpath 解析后不在允许根内即拒。
    #[test]
    fn symlink_escape_beats_the_write_allowlist() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "s").unwrap();
        let escape = root.join("escape.md");
        std::os::unix::fs::symlink(outside.path().join("secret.txt"), &escape).unwrap();
        let policy = FileToolPolicy {
            workdir_bound: vec![root.clone()],
            allow_writes: vec![WriteAllow {
                root: root.clone(),
                pattern: "escape.md".into(),
            }],
            ..Default::default()
        };
        // 白名单按文件名会命中，但 realpath 解析后目标在根外 → 允许根先拒
        assert!(policy.check_write(&escape).is_err());
    }
}
