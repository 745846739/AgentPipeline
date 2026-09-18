//! 修复的生命周期（决策 210③④⑤⑥ / 票 10、11）。
//!
//! 一次「修复」= 在一个**独立于任务工作区**的 git worktree 里改代码 → 过闸门 → 单独成 commit
//! → 出 diff 交给人按。
//!
//! 三条边界写在这里，不散到调用方：
//!
//! 1. **载体是 worktree**（票 10）。本仓（以及任何在 `~/Documents` 下的项目）值班长的
//!    `write_file` / `edit_file` **一个字都写不了**——`FileToolPolicy` 对读写都强制
//!    「路径必须落在允许根内」，而它的允许根是 `home.root()`。worktree 落在
//!    `{home}/worktrees/repair-{id}` 下，于是可达；同时与你正在开发的工作区隔离，
//!    且 diff 天然是 `{base}..{branch}`。
//! 2. **闸门必过**（票 11）。没过就播报失败、**不出 diff**——「快」的定义是「不用等你来钉它」，
//!    不是「跳过验证」。你早上审的若不是一份补丁，那就是一份赌注。
//! 3. **单独成 commit 且带标记**（票 11）。不做标记的账单会在三个月后到期：那时你会盯着一行
//!    代码想「这是谁写的、谁让它这么写的」，而答案不在库里。
//!
//! **不新建 `kanban_tasks` 行**（用户的裁决「修复要快」）。代价如实记：这条修复不在看板上、
//! 不进 `kanban_node_cursors` 那套账——账由修复提议（票 12）承担。

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::git::Git;
use crate::home::Home;
use crate::pipeline::executor::test_command_for;
use crate::agent::tools::{CommandFinish, CommandRecorder, CommandStart};
use crate::storage::Store;
use crate::types::{CommandSource, Node, Stage};
use crate::{Error, Result};

/// 修复分支的 commit message 标记（票 11 的审计线）。
///
/// `git log --grep` 一条就能把「agent 写的改动」全部捞出来——这就是它存在的全部理由。
pub const REPAIR_COMMIT_MARK: &str = "[repair]";

/// 修复的闸门命令（system 命令，落 `kanban_node_commands` 的域）。
const REPAIR_STAGE: Stage = Stage::Merge;

/// 一次修复的现场（票 10 / 11 的产出，票 12 的提议载荷）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepairOutcome {
    pub repair_id: String,
    /// 修复 worktree（在 `{home}/worktrees/` 下）。
    pub worktree_path: String,
    pub branch: String,
    /// 修复分支从哪个基准分出（`origin/main` 或 `main`）。
    pub base_ref: String,
    pub base_commit: String,
    /// 闸门过了没有。**没过则 `commit` / `diff` / `diff_stat` 全为 `None`**。
    pub gate_passed: bool,
    /// 闸门命令的读数（lint / test 各一条），失败时也在这里。
    pub gate: Vec<GateReading>,
    pub commit: Option<String>,
    pub diff: Option<String>,
    pub diff_stat: Option<String>,
}

/// 闸门里一条命令的读数（票 11：闸门读数要显示在那条提议上）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateReading {
    /// `lint` / `test`。
    pub kind: String,
    pub command: String,
    pub exit_code: i32,
    pub duration_ms: u64,
    /// 命令全文的落点（`gate-output-repair-{id}-{kind}.log`）。
    pub output_path: Option<String>,
    pub output_preview: String,
}

/// 一次修复的起点：建好 worktree（票 10）。
#[derive(Debug, Clone, PartialEq)]
pub struct RepairSession {
    pub repair_id: String,
    pub worktree: PathBuf,
    pub branch: String,
    pub base_ref: String,
}

/// 拉起一次修复的载体（决策 210③ / 票 10）。
///
/// base 的取法与 merge 阶段一致（有 `origin` 用 `origin/{default}`——[`Git::init_worktree_named`]
/// 里那一份实现），worktree 在家目录下（值班长可达），分支带 `repair/` 前缀（可分）。
pub async fn start_repair(
    home: &Home,
    project_path: &Path,
    default_branch: &str,
    repair_id: &str,
) -> Result<RepairSession> {
    let worktree = home.repair_worktree_path(repair_id);
    let branch = Git::repair_branch_for(repair_id);
    let base_ref = Git
        .init_worktree_named(project_path, &branch, &worktree, default_branch)
        .await?;
    Ok(RepairSession {
        repair_id: repair_id.to_string(),
        worktree,
        branch,
        base_ref,
    })
}

/// 跑闸门（决策 210④ / 票 11）：lint（如配置）+ 测试，**全过才算改完**。
///
/// 命令与 executor 的系统闸门**同一处映射**（`test_command_for`）：两处各写一份的下场是
/// 「流水线里跑的是 `cargo test --quiet`、修复这边跑的是别的」，而两边都自称过了闸门。
pub async fn run_repair_gate(
    store: &Store,
    home: &Home,
    session: &RepairSession,
    lint_command: Option<&str>,
    test_framework: Option<&str>,
) -> Result<Vec<GateReading>> {
    let mut readings = Vec::new();
    let mut commands: Vec<(&str, String)> = Vec::new();
    if let Some(lint) = lint_command.filter(|c| !c.trim().is_empty()) {
        commands.push(("lint", lint.to_string()));
    }
    commands.push(("test", test_command_for(test_framework).to_string()));

    for (kind, command) in commands {
        let reading = run_gate_command(store, home, session, kind, &command).await?;
        let failed = reading.exit_code != 0;
        readings.push(reading);
        if failed {
            // 闸门不过就**停在这里**：后面那条命令的读数没有意义（它是另一件事的验证），
            // 而人要看的是「哪一步没过、为什么」。
            break;
        }
    }
    Ok(readings)
}

/// 跑一条闸门命令并落库（`kanban_node_commands` 的 system 源 + 全文日志）。
///
/// 修复没有任务、没有游标，故 `task_id` / `run_id` 都是 `None`：它挂的是
/// **修复自己**（归属由命令内容与日志文件名说清）。这是票 11 要求的「命名要自洽」。
async fn run_gate_command(
    store: &Store,
    home: &Home,
    session: &RepairSession,
    kind: &str,
    command: &str,
) -> Result<GateReading> {
    let command_id = store
        .record_start(CommandStart {
            task_id: None,
            // 修复既不是任务也不是值班会话：两处归属都为空。迁移 0012 的 CHECK 只约束
            // 「不能都是非空」，这里都是 NULL —— 于是这条命令在界面上按「修复」看。
            session_id: None,
            run_id: None,
            stage: REPAIR_STAGE,
            node: Node::Execute,
            source: CommandSource::System,
            command: command.to_string(),
            cwd: session.worktree.display().to_string(),
        })
        .await?;

    let started = std::time::Instant::now();
    let output = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(&session.worktree)
        .output()
        .await;
    let duration_ms = started.elapsed().as_millis() as u64;
    let (exit_code, stdout, stderr) = match output {
        Ok(out) => (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        ),
        Err(e) => (-1, String::new(), format!("命令无法执行：{e}")),
    };
    // 全文落可读路径（与 executor 的闸门日志同一约定，文件名自洽：带修复 id 与 kind）
    let full_path = home
        .worktrees_dir()
        .join(format!("gate-output-repair-{}-{kind}.log", session.repair_id));
    let full_log = format!("[stdout]\n{stdout}\n[stderr]\n{stderr}");
    let _ = std::fs::write(&full_path, &full_log);
    let preview = crate::agent::tools::head_tail(&full_log, 10, 20);

    store
        .record_finish(
            command_id,
            CommandFinish {
                exit_code: Some(exit_code),
                stdout_path: Some(full_path.display().to_string()),
                stdout_preview: Some(preview.clone()),
                stderr_preview: None,
                duration_ms,
            },
        )
        .await?;

    Ok(GateReading {
        kind: kind.to_string(),
        command: command.to_string(),
        exit_code,
        duration_ms,
        output_path: Some(full_path.display().to_string()),
        output_preview: preview,
    })
}

/// 把工作区里的改动落成**一个** commit（票 11）。
///
/// 用 `git2` 而不是 shell：这是「改完」这一步的落点，它得有一份可测的返回（commit oid）
/// 与一句确定的 message。`summary` 是诊断结论那一句话，跟着标记一起进 message——
/// 三个月后 `git log --grep` 出来时，读到的不只是一个 oid。
pub async fn commit_repair(
    session: &RepairSession,
    headline: &str,
    now: DateTime<Utc>,
) -> Result<String> {
    let worktree = session.worktree.clone();
    let repair_id = session.repair_id.clone();
    let headline = headline.to_string();
    crate::git::blocking(move || {
        let repo = git2::Repository::open(&worktree).map_err(crate::git::gerr)?;
        let mut index = repo.index().map_err(crate::git::gerr)?;
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .map_err(crate::git::gerr)?;
        index.write().map_err(crate::git::gerr)?;
        let tree_id = index.write_tree().map_err(crate::git::gerr)?;
        let tree = repo.find_tree(tree_id).map_err(crate::git::gerr)?;
        let parent = repo
            .head()
            .and_then(|h| h.peel_to_commit())
            .map_err(crate::git::gerr)?;
        let message = format!(
            "{REPAIR_COMMIT_MARK} 值班长修复 {repair_id}：{headline}\n\n\
             来源：值班长的自动修复轮（决策 210⑤）。改动与 agent 手写的代码在 diff 里长得一样，\n\
             这一行标记是它们之间唯一的区别。\n\
             时间：{}\n",
            now.to_rfc3339()
        );
        let seconds = now.timestamp();
        let signature = git2::Signature::new(
            "agentpipeline-foreman",
            "foreman@agentpipeline.local",
            &git2::Time::new(seconds, 0),
        )
        .map_err(crate::git::gerr)?;
        let oid = repo
            .commit(
                Some("HEAD"),
                &signature,
                &signature,
                &message,
                &tree,
                &[&parent],
            )
            .map_err(crate::git::gerr)?;
        Ok(oid.to_string())
    })
    .await
}

/// 出 diff（`{base}..{branch}`，与 merge 阶段同一口径）。
pub async fn repair_diff(project_path: &Path, session: &RepairSession) -> Result<(String, String)> {
    let range = format!("{}..{}", session.base_ref, session.branch);
    let diff = Git.diff_range(project_path, &range).await?;
    let stat = Git.diff_stat(project_path, &range).await?;
    Ok((diff, stat))
}

/// 回收一次修复（决策 210⑦ / 票 10）。
///
/// - **合入成功**：删分支 + 删 worktree。
/// - **被拒 / 年龄清理**：**保留分支、删 worktree**——分支是唯一的证据，
///   与决策 207「过期只让按钮变灰、那一轮留在时间线」同一理由。
pub async fn finish_repair(
    project_path: &Path,
    session: &RepairSession,
    merged: bool,
) -> Result<()> {
    Git.remove_worktree(project_path, &session.worktree, true).await?;
    if merged {
        Git.delete_branch(project_path, &session.branch).await?;
    }
    Ok(())
}

/// 修复分支当前的 commit（不存在时报错——「合入」前要确认它还在）。
pub async fn repair_head(project_path: &Path, session: &RepairSession) -> Result<String> {
    Git.rev_parse(project_path, &session.branch).await
}

/// 把修复的 diff 落成可读文件（提议面板要能展开看全文）。
pub fn write_repair_diff(home: &Home, repair_id: &str, diff: &str) -> Result<PathBuf> {
    let path = home
        .worktrees_dir()
        .join(format!("repair-{repair_id}.diff"));
    std::fs::write(&path, diff)?;
    Ok(path)
}

/// 修复 id 生成（ULID：时间有序，`git branch --list repair/*` 天然按时间排）。
pub fn new_repair_id() -> String {
    ulid::Ulid::new().to_string()
}

/// 修复闸门失败时说清「哪一步没过」（票 11：播报失败原因，lint 还是 test）。
pub fn gate_failure_note(readings: &[GateReading]) -> String {
    match readings.iter().find(|r| r.exit_code != 0) {
        Some(r) => format!(
            "闸门没过（{}）：`{}` 退出码 {}，用时 {}ms。完整输出在 {}",
            r.kind,
            r.command,
            r.exit_code,
            r.duration_ms,
            r.output_path.as_deref().unwrap_or("（未落盘）")
        ),
        None => "闸门没过（读数缺失）".to_string(),
    }
}

/// 修复的判据：这个项目能不能修（有没有 git 仓、有没有默认分支）。
///
/// 报错要说得清「为什么不能修」，而不是让调用方去猜一个 `Err`。
pub fn repair_supported(project: &crate::types::Project) -> Result<()> {
    if project.local_path.trim().is_empty() {
        return Err(Error::Validation(format!(
            "项目 {} 没有仓库路径，无法拉起修复 worktree",
            project.id
        )));
    }
    if !Path::new(&project.local_path).join(".git").exists() {
        return Err(Error::Validation(format!(
            "项目 {} 的路径下不是 git 仓库（{}），修复需要版本控制",
            project.id, project.local_path
        )));
    }
    Ok(())
}
