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

/// 文件工具路径策略（`SystemBaseline.file_tool_policy`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileToolPolicy {
    /// 文件工具的允许根：worktree + 任务目录。
    pub workdir_bound: Vec<PathBuf>,
    /// 禁止访问的路径模式。
    pub deny_paths: Vec<String>,
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
            resolve_realpath: true,
            refuse_symlink_write: true,
        }
    }
}

/// 默认拒绝名单（决策 104 / agents.md §10.6.2）。
pub fn default_deny_paths() -> Vec<String> {
    vec![
        ".env*".to_string(),
        "*.pem".to_string(),
        "*.key".to_string(),
        "id_rsa*".to_string(),
        "~/.ssh".to_string(),
    ]
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
    fn deny_beats_allow() {
        // .env 落在允许根**之内**，仍必须被拒绝（deny 优先）
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let policy = policy_for(&root);
        assert!(policy.check_read(&root.join("src/lib.rs")).is_ok());
        assert!(policy.check_read(&root.join(".env")).is_err());
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
}
