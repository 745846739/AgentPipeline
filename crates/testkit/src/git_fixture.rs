//! git fixture builder（决策 146）：用**系统 git CLI** 搭建场景仓库。
//!
//! 与生产 worktree / rebase / 合入的记录命令路径一致，因此 fixture 里踩到的坑就是生产会踩的坑。

use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

/// 目标项目语言（验 `{test_command}` / `{test_file_convention}` 模板变量）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    Python,
    Node,
}

/// 场景仓库。
pub struct Repo {
    dir: TempDir,
    path: PathBuf,
    /// 附属目录（remote bare 仓库、符号链接目标），保证生命周期一致。
    extra: Vec<TempDir>,
}

impl Repo {
    /// 干净仓库：main 分支、一次提交、含 AGENTS.md 与 .gitignore。
    pub fn clean() -> std::io::Result<Self> {
        let repo = Repo::init_empty()?;
        repo.write("README.md", "# 示例项目\n");
        repo.write(
            "Cargo.toml",
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        repo.write(
            "src/lib.rs",
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
        );
        repo.write("tests/acceptance.rs", "#[test]\nfn it_works() {}\n");
        repo.write("AGENTS.md", "# 项目指令\n本仓库使用系统 git CLI。\n");
        repo.write(".gitignore", "/target\n");
        repo.commit_all("chore: 初始提交");
        Ok(repo)
    }

    /// 仅 `git init`，零提交（unborn HEAD）。
    pub fn unborn_head() -> std::io::Result<Self> {
        let repo = Repo::init_empty()?;
        repo.write("README.md", "no commit yet\n");
        Ok(repo)
    }

    /// 有 remote 的仓库：额外的 bare 仓库作为 `origin`，main 已推送。
    pub fn with_remote() -> std::io::Result<(Self, Self)> {
        let remote = Repo::init_bare()?;
        let repo = Repo::clean()?;
        repo.git(&[
            "remote",
            "add",
            "origin",
            &remote.path.display().to_string(),
        ]);
        repo.git(&["push", "-u", "origin", "main"]);
        Ok((repo, remote))
    }

    fn init_empty() -> std::io::Result<Self> {
        let dir = tempfile::Builder::new()
            .prefix("agentpipeline-repo-")
            .tempdir()?;
        let path = dir.path().to_path_buf();
        run(&path, &["init", "-b", "main"]);
        // 让 diff / rebase 行为与生产一致
        run(&path, &["config", "core.autocrlf", "false"]);
        Ok(Repo {
            dir,
            path,
            extra: Vec::new(),
        })
    }

    fn init_bare() -> std::io::Result<Self> {
        let dir = tempfile::Builder::new()
            .prefix("agentpipeline-remote-")
            .tempdir()?;
        let path = dir.path().to_path_buf();
        run(&path, &["init", "--bare", "-b", "main"]);
        Ok(Repo {
            dir,
            path,
            extra: Vec::new(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 执行 git 并要求成功，返回 stdout。
    pub fn git(&self, args: &[&str]) -> String {
        let out = self.try_git(args);
        assert_eq!(out.0, 0, "git {} 失败：{}", args.join(" "), out.2);
        out.1
    }

    /// 执行 git 不校验退出码：(exit_code, stdout, stderr)。
    pub fn try_git(&self, args: &[&str]) -> (i32, String, String) {
        run_raw(&self.path, args)
    }

    pub fn write(&self, relative: &str, content: &str) {
        let path = self.path.join(relative);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(path, content).expect("fixture 写入失败");
    }

    pub fn read(&self, relative: &str) -> String {
        std::fs::read_to_string(self.path.join(relative)).expect("fixture 读取失败")
    }

    pub fn exists(&self, relative: &str) -> bool {
        self.path.join(relative).exists()
    }

    pub fn commit_all(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-m", message]);
    }

    pub fn branch(&self, name: &str) {
        self.git(&["checkout", "-b", name]);
    }

    pub fn checkout(&self, name: &str) {
        self.git(&["checkout", name]);
    }

    pub fn current_branch(&self) -> String {
        self.git(&["symbolic-ref", "--short", "HEAD"])
            .trim()
            .to_string()
    }

    pub fn head(&self, rev: &str) -> String {
        self.git(&["rev-parse", rev]).trim().to_string()
    }

    pub fn is_dirty(&self) -> bool {
        !self.git(&["status", "--porcelain"]).trim().is_empty()
    }

    /// 目标分支脏工作区（决策 61：合入时进 pending(user_decision)）。
    pub fn dirty_worktree(&self) -> std::io::Result<()> {
        self.write("uncommitted.txt", "未提交的改动\n");
        Ok(())
    }

    /// 多语言目标项目（决策 146：验 `{test_command}` 模板变量）。
    pub fn project(&self, language: Language) {
        match language {
            Language::Rust => {
                self.write(
                    "Cargo.toml",
                    "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
                );
                self.write("src/main.rs", "fn main() { println!(\"hi\"); }\n");
                self.write("tests/acceptance.rs", "#[test]\nfn it_works() {}\n");
                self.commit_all("chore: rust 项目骨架");
            }
            Language::Python => {
                self.write("pyproject.toml", "[project]\nname = \"demo\"\n");
                self.write("app/__init__.py", "");
                self.write("tests/test_app.py", "def test_ok():\n    assert True\n");
                self.commit_all("chore: python 项目骨架");
            }
            Language::Node => {
                self.write(
                    "package.json",
                    "{\"name\":\"demo\",\"version\":\"1.0.0\",\"scripts\":{\"test\":\"vitest run\"}}\n",
                );
                self.write("src/index.ts", "export const x = 1;\n");
                self.write(
                    "src/index.test.ts",
                    "import {test} from 'vitest';\ntest('x',()=>{});\n",
                );
                self.commit_all("chore: node 项目骨架");
            }
        }
    }

    /// 可自动合并的 rebase：两个分支改同一文件的不同区域。
    pub fn conflict_auto(&self) -> std::io::Result<()> {
        self.write("doc.txt", "l1\nl2\nl3\nl4\nl5\n");
        self.commit_all("chore: 冲突素材");
        self.branch("topic");
        self.write("doc.txt", "TOPIC\nl2\nl3\nl4\nl5\n");
        self.commit_all("feat: topic 改首行");
        self.checkout("main");
        self.write("doc.txt", "l1\nl2\nl3\nl4\nMAIN\n");
        self.commit_all("feat: main 改末行");
        // 停在 main，调用方按需检出 topic
        Ok(())
    }

    /// 不可自动合并的 rebase：两个分支改同一行。结束时停在 `main`。
    pub fn conflict_hard(&self) -> std::io::Result<()> {
        self.write("shared.txt", "base\n");
        self.commit_all("chore: 冲突素材");
        self.branch("topic");
        self.write("shared.txt", "topic\n");
        self.commit_all("feat: topic 改这一行");
        self.checkout("main");
        self.write("shared.txt", "main\n");
        self.commit_all("feat: main 改同一行");
        // 故意停在 main：调用方自行 checkout topic 或从 topic 建 worktree 分支
        Ok(())
    }

    /// 推进 main（验 merge 阶段 B 的基准前移 / 非 ff 合入，决策 96）。
    pub fn advance_main(&self, relative: &str, content: &str) {
        let original = self.current_branch();
        self.checkout("main");
        self.write(relative, content);
        self.commit_all("feat: main 前移");
        self.checkout(&original);
    }

    /// macOS `/tmp` → `/private/tmp` 符号链接陷阱（决策 104）。
    ///
    /// 在**符号链接前缀**下建一个临时目录，并在其中放一个指向外部的符号链接文件。
    /// 返回（受符号链接影响的目录，指向外部的链接路径）。
    pub fn symlink_trap(&mut self) -> std::io::Result<(PathBuf, PathBuf)> {
        let outside = tempfile::Builder::new()
            .prefix("agentpipeline-secret-")
            .tempdir()?;
        let secret = outside.path().join("secret.txt");
        std::fs::write(&secret, "top secret\n")?;

        // /tmp 在 macOS 上是符号链接，realpath 后才能与 canonicalize 结果对上
        let linked_root = std::path::PathBuf::from("/tmp")
            .join(format!("agentpipeline-linked-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&linked_root)?;
        let link = linked_root.join("escape.txt");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&secret, &link)?;

        self.extra.push(outside);
        Ok((linked_root, link))
    }

    /// worktree 是否仍被登记（清理断言用）。
    pub fn worktree_list(&self) -> String {
        self.git(&["worktree", "list"])
    }

    pub fn branch_exists(&self, name: &str) -> bool {
        self.try_git(&["rev-parse", "--verify", name]).0 == 0
    }

    pub fn dir(&self) -> &TempDir {
        &self.dir
    }
}

fn run(cwd: &Path, args: &[&str]) {
    let (code, _, stderr) = run_raw(cwd, args);
    assert_eq!(code, 0, "git {} 失败：{stderr}", args.join(" "));
}

fn run_raw(cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new("git")
        .args(["-c", "user.name=fixture"])
        .args(["-c", "user.email=fixture@localhost"])
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git 可执行");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_repo_has_main_branch_and_commit() {
        let repo = Repo::clean().unwrap();
        assert_eq!(repo.current_branch(), "main");
        assert!(!repo.is_dirty());
        assert_eq!(repo.head("HEAD").len(), 40);
        assert!(repo.exists("AGENTS.md"));
        assert!(repo.exists(".gitignore"));
    }

    #[test]
    fn unborn_head_repo_has_no_commit() {
        let repo = Repo::unborn_head().unwrap();
        assert_ne!(repo.try_git(&["rev-parse", "--verify", "HEAD"]).0, 0);
    }

    #[test]
    fn with_remote_sets_origin_and_pushes() {
        let (repo, remote) = Repo::with_remote().unwrap();
        let remotes = repo.git(&["remote"]);
        assert!(remotes.contains("origin"));
        // origin/main 存在
        assert_eq!(repo.try_git(&["rev-parse", "--verify", "origin/main"]).0, 0);
        assert_eq!(repo.head("origin/main"), repo.head("main"));
        assert!(remote.path().join("HEAD").exists());
    }

    #[test]
    fn conflict_hard_rebase_really_conflicts() {
        let repo = Repo::clean().unwrap();
        repo.conflict_hard().unwrap();
        repo.checkout("topic");
        let (code, _, stderr) = repo.try_git(&["rebase", "main"]);
        assert_ne!(code, 0, "应当冲突");
        assert!(
            stderr.contains("conflict") || stderr.contains("could not apply"),
            "stderr: {stderr}"
        );
    }

    #[test]
    fn conflict_auto_rebase_merges_cleanly() {
        let repo = Repo::clean().unwrap();
        repo.conflict_auto().unwrap();
        repo.checkout("topic");
        let (code, _, stderr) = repo.try_git(&["rebase", "main"]);
        assert_eq!(code, 0, "应自动合并成功：{stderr}");
        let content = repo.read("doc.txt");
        assert!(content.contains("TOPIC"));
        assert!(content.contains("MAIN"));
    }

    #[test]
    fn advance_main_moves_default_branch() {
        let repo = Repo::clean().unwrap();
        let before = repo.head("main");
        repo.advance_main("src/lib.rs", "pub fn add() {}\n");
        assert_ne!(repo.head("main"), before);
    }

    #[test]
    fn multi_language_projects_and_detection() {
        for (lang, expected) in [
            (Language::Rust, "rust"),
            (Language::Python, "python"),
            (Language::Node, "node"),
        ] {
            // 每种语言放在各自的干净仓库里（探测顺序 rust > node > python）
            let repo = Repo::unborn_head().unwrap();
            repo.project(lang);
            assert_eq!(
                agentpipeline_core::git::Git::detect_language(repo.path()).as_deref(),
                Some(expected),
                "{lang:?} 项目探测失败"
            );
        }

        // 标记文件齐全时按固定优先级取 rust（结果稳定、可断言）
        let repo = Repo::clean().unwrap();
        repo.project(Language::Rust);
        repo.project(Language::Python);
        repo.project(Language::Node);
        assert_eq!(
            agentpipeline_core::git::Git::detect_language(repo.path()).as_deref(),
            Some("rust")
        );
    }

    #[test]
    fn symlink_trap_is_under_a_symlinked_prefix() {
        let mut repo = Repo::clean().unwrap();
        let (linked_root, link) = repo.symlink_trap().unwrap();
        assert!(link.exists() || std::fs::symlink_metadata(&link).is_ok());
        let resolved = linked_root.canonicalize().unwrap();
        assert_ne!(
            resolved, linked_root,
            "该路径应位于符号链接之下（macOS /tmp），否则 realpath 断言无意义"
        );
    }

    #[test]
    fn dirty_worktree_makes_status_dirty() {
        let repo = Repo::clean().unwrap();
        assert!(!repo.is_dirty());
        repo.dirty_worktree().unwrap();
        assert!(repo.is_dirty());
    }
}
