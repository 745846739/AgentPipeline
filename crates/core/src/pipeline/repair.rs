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

use crate::agent::tools::{CommandFinish, CommandRecorder, CommandStart};
use crate::git::Git;
use crate::home::Home;
use crate::pipeline::merge::test_command_for;
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
    /// 这次修复归属的班次。**必填**：修复是值班长在一次对话里决定做的事，命令台账里
    /// 就该记在那一班名下——迁移 0012 的归属是**严格 XOR**（任务 xor 班次），
    /// 「两边都不属于」那一行会被存储层直接拒掉（`observability.rs` 的归属归一）。
    pub session_id: String,
    pub worktree: PathBuf,
    pub branch: String,
    pub base_ref: String,
}

impl RepairSession {
    /// 从一条提议的载荷（[`RepairOutcome`]）**重建**现场（决策 255）。
    ///
    /// 载荷里带着 `repair_id` / `worktree_path` / `branch` / `base_ref` 四件，正是本结构
    /// 除 `session_id` 之外的全部字段；`session_id` 来自提议归属的班次。此前这条重建在
    /// 三处各写了一遍（提议执行、拒绝时的回收、以及 `finish_repair_round` 那条），
    /// 收成一处之后「重建一次修复现场」只有一个答案。
    pub fn from_outcome(outcome: &RepairOutcome, session_id: &str) -> RepairSession {
        RepairSession {
            repair_id: outcome.repair_id.clone(),
            session_id: session_id.to_string(),
            worktree: PathBuf::from(&outcome.worktree_path),
            branch: outcome.branch.clone(),
            base_ref: outcome.base_ref.clone(),
        }
    }
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
    session_id: &str,
) -> Result<RepairSession> {
    let worktree = home.repair_worktree_path(repair_id);
    let branch = Git::repair_branch_for(repair_id);
    let base_ref = Git
        .init_worktree_named(project_path, &branch, &worktree, default_branch)
        .await?;
    Ok(RepairSession {
        repair_id: repair_id.to_string(),
        session_id: session_id.to_string(),
        worktree,
        branch,
        base_ref,
    })
}

/// 按 `repair_id` 重建一次修复的现场（票 10 的路径与分支命名是**确定**的，故重建只差一个
/// base 的解析）。
///
/// 为什么要能重建：值班长的工具调用是**一轮一次**的，`repair(action=start)` 与
/// `repair(action=finish)` 之间隔着整个「改代码」的过程——现场不可能留在内存里，
/// 只能靠 id 从命名规则复原（这也正是票 10 要求「命名自洽」的用处）。
pub async fn repair_session_for(
    home: &Home,
    project_path: &Path,
    default_branch: &str,
    repair_id: &str,
    session_id: &str,
) -> Result<RepairSession> {
    Ok(RepairSession {
        repair_id: repair_id.to_string(),
        session_id: session_id.to_string(),
        worktree: home.repair_worktree_path(repair_id),
        branch: Git::repair_branch_for(repair_id),
        // 与 `start_repair` 走同一个解析（有 `origin` 用 `origin/{default}`）：diff 的范围是
        // `{base}..{branch}`，两处解析出不同的名字就等于量错了范围。
        base_ref: Git.base_ref(project_path, default_branch).await?,
    })
}

/// 一次修复轮的收口结果（决策 210④⑤ / 票 11、12）。
#[derive(Debug, Clone, PartialEq)]
pub enum RepairRound {
    /// 闸门没过：**没有 commit、没有 diff、没有提议**——「改完」这句话还不成立。
    GateFailed {
        gate: Vec<GateReading>,
        note: String,
    },
    /// 闸门过了：已 commit（带标记）、已出 diff、已落一条不设 TTL 的提议。
    ///
    /// `outcome` 是**装箱**的：它是这一族里唯一的大载荷（272 字节，其余变体只有几十），
    /// 而 clippy 的 `large_enum_variant` 拦正是拦这个——按值传一个被立刻拆开的结果不值当。
    Proposed {
        outcome: Box<RepairOutcome>,
        proposal_id: String,
        summary: String,
    },
}

/// 把一次修复**收口**：闸门 → （过了才）commit → diff → 落提议 → 两处留痕。
///
/// 整条序列写在**一个**函数里，而不是散在工具分派那几行：每一步都有「不做会怎样」的后果
/// （不过闸门就不许出 diff、commit 必须带标记、提议必须带载荷与不设 TTL），散开写迟早漂成
/// 「某条路少了一步」。工具层因此只剩「按动作分派」。
///
/// `task_id` 是这条修复为之而做的任务（可选）：给了就留两处痕——班次里一条
/// 「等修复合入」（人读对话时知道它在等什么），**任务上**也一句（人在看板上看那条任务时
/// 知道它在等什么）。两处都要，因为它们回答的是两个场景的问题（票 11 / 决策 210⑨）。
pub async fn finish_repair_round(
    store: &Store,
    home: &Home,
    project: &crate::types::Project,
    session: &RepairSession,
    conclusion: &str,
    task_id: Option<&str>,
    now: DateTime<Utc>,
) -> Result<RepairRound> {
    let gate = run_repair_gate(
        store,
        home,
        session,
        project.lint_command.as_deref(),
        project.test_framework.as_deref(),
    )
    .await?;
    let failed = gate.iter().any(|r| r.exit_code != 0);
    if failed {
        return Ok(RepairRound::GateFailed {
            note: gate_failure_note(&gate),
            gate,
        });
    }

    let commit = commit_repair(session, conclusion, now).await?;
    let (diff, diff_stat) = repair_diff(Path::new(&project.local_path), session).await?;
    write_repair_diff(home, &session.repair_id, &diff)?;
    let outcome = RepairOutcome {
        repair_id: session.repair_id.clone(),
        worktree_path: session.worktree.display().to_string(),
        branch: session.branch.clone(),
        base_ref: session.base_ref.clone(),
        base_commit: Git
            .rev_parse(Path::new(&project.local_path), &session.base_ref)
            .await?,
        gate_passed: true,
        gate,
        commit: Some(commit),
        diff: Some(diff),
        diff_stat: Some(diff_stat),
    };
    let proposal = propose_repair(store, &session.session_id, project, &outcome).await?;
    if let Some(task_id) = task_id {
        // 两处留痕都是**善后**：写不进去不该让「这次修复已经就绪」这件事变成一次失败——
        // 提议已经落库了，那才是人按键的地方。故各留一条 warn。
        let note = format!("等修复合入（修复 {} 已过闸门）", session.repair_id);
        if let Err(e) = store
            .append_foreman_message(crate::storage::NewForemanMessage::system(
                &session.session_id,
                format!(
                    "【等修复合入】任务 {task_id} 的修复已就绪（{}）：{}——合入之前那条任务不会\
                     自己往前走。",
                    session.repair_id, proposal.summary
                ),
            ))
            .await
        {
            tracing::warn!(%task_id, "「等修复合入」的班次留痕写不进去：{e}");
        }
        match store.note_task_awaiting_repair_merge(task_id, &note).await {
            // 任务不在 pending 上：它没有在等任何东西，这句话无处可写——不是故障，故只是 debug
            // （修复一条没停下来的任务是正常用法，不是要人处理的事）。
            Ok(false) => {
                tracing::debug!(%task_id, "任务不在 pending 上，「等修复合入」只落在班次里")
            }
            Ok(true) => {}
            Err(e) => tracing::warn!(%task_id, "「等修复合入」的任务留痕写不进去：{e}"),
        }
    }
    Ok(RepairRound::Proposed {
        summary: proposal.summary.clone(),
        proposal_id: proposal.id,
        outcome: Box::new(outcome),
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
/// 修复没有任务、没有游标，故 `task_id` / `run_id` 都是 `None`；归属是**那一班**——
/// 迁移 0012 的 CHECK 要求「任务 xor 班次」恰好一个，命令台账里于是能读到
/// 「值班长为了这次修复跑了什么」，与它别的命令排在同一条时间线上。
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
            session_id: Some(session.session_id.clone()),
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
    let full_path = home.worktrees_dir().join(format!(
        "gate-output-repair-{}-{kind}.log",
        session.repair_id
    ));
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
    Git.remove_worktree(project_path, &session.worktree, true)
        .await?;
    if merged {
        Git.delete_branch(project_path, &session.branch).await?;
    }
    Ok(())
}

/// 修复分支当前的 commit（不存在时报错——「合入」前要确认它还在）。
pub async fn repair_head(project_path: &Path, session: &RepairSession) -> Result<String> {
    Git.rev_parse(project_path, &session.branch).await
}

/// 回收**没人按过、已经过了保留期**的修复 worktree（决策 212③ / 票 12 的最后一格）。
///
/// 这是 [`finish_repair`] 的第三个调用者，补的是「一直没人按」那一条：合入与拒绝都由人按下，
/// 而一条没人理的修复提议最终会被年龄清理删掉——在删行之前把它建的 worktree 收掉，
/// **保留分支**（与拒绝同一理由：分支是唯一的证据）。
///
/// 为什么在删行之前：`args.project_id` 与载荷里的 worktree 路径是唯一知道那个目录属于谁、
/// 在哪个仓里的东西。行一删，那个目录就成了无主残留——这正是这个函数存在的理由。
///
/// 四处**不致命**的跳过（维护作业不该被一行坏数据打断）：载荷解析不出来、项目已不在台账里、
/// 目录已经不在了（人按过拒绝但行还留着的情况）、以及**回收本身失败**（项目路径已经不在磁盘上、
/// worktree 的登记已被别的东西清掉）。每一处都留一条 `warn`：静默跳过会让残留悄悄地留下来。
///
/// 最后那一处为什么要收住：这个函数跑在**小时级维护作业**里，而它后面就是保留期清理
/// （对话 / 提议 / 待办）。让一个 git 失败把整趟维护带走，就等于「一个 bad row 让所有清理停摆」
/// ——那比留下一个目录坏得多。
pub async fn recycle_unpressed_repair_worktrees(
    store: &Store,
    cutoff: DateTime<Utc>,
) -> Result<usize> {
    let stale = store.list_pending_repair_proposals_before(cutoff).await?;
    let mut recycled = 0;
    for proposal in stale {
        let Some(payload) = &proposal.payload else {
            tracing::warn!(proposal = %proposal.id, "修复提议没有载荷，回收不了它的 worktree");
            continue;
        };
        let Ok(outcome) = serde_json::from_value::<RepairOutcome>(payload.clone()) else {
            tracing::warn!(proposal = %proposal.id, "修复提议的载荷解析不出修复现场，跳过回收");
            continue;
        };
        let Some(project_id) = proposal.args.get("project_id").and_then(|v| v.as_str()) else {
            tracing::warn!(proposal = %proposal.id, "修复提议没写 project_id，回收不了它的 worktree");
            continue;
        };
        let Some(project) = store.get_project(project_id).await? else {
            tracing::warn!(project = %project_id, "修复提议指向的项目已不在台账里，跳过回收");
            continue;
        };
        let session = RepairSession::from_outcome(&outcome, &proposal.session_id);
        if !session.worktree.exists() {
            // 「拒绝」已经收过一遍而行还留着（或人自己删的）：不是故障，无事可做。
            continue;
        }
        if let Err(e) = finish_repair(Path::new(&project.local_path), &session, false).await {
            tracing::warn!(
                proposal = %proposal.id,
                worktree = %session.worktree.display(),
                "回收过期修复 worktree 失败（行照旧会被年龄清理删掉）：{e}"
            );
            continue;
        }
        recycled += 1;
    }
    Ok(recycled)
}

/// 把修复的 diff 落成可读文件（提议面板要能展开看全文）。
pub fn write_repair_diff(home: &Home, repair_id: &str, diff: &str) -> Result<PathBuf> {
    let path = home
        .worktrees_dir()
        .join(format!("repair-{repair_id}.diff"));
    std::fs::write(&path, diff)?;
    Ok(path)
}

/// 把一次修复落成**一条提议**（决策 212① / 票 12）。
///
/// 为什么复用提议表而不是新开一张：它的每个字段都对得上修复这件事（见迁移 0022 的注释）。
/// 这里只做一件表里没有的事——**写载荷**（`kind = repair` + 现场 JSON）与**不设 TTL**
/// （由存储层按 `kind` 判）。
///
/// `summary` 是人在按下之前唯一读的那一行，故它必须说清三件事：修哪个项目、依据什么结论、
/// 闸门过了没有。含糊的一句「合入一个修复」等于让人对着一个分支名按键。
pub async fn propose_repair(
    store: &Store,
    session_id: &str,
    project: &crate::types::Project,
    outcome: &RepairOutcome,
) -> Result<crate::storage::proposals::ForemanProposal> {
    let gate_note = if outcome.gate_passed {
        "闸门已过".to_string()
    } else {
        format!("**闸门未过**：{}", gate_failure_note(&outcome.gate))
    };
    let summary = format!(
        "合入修复分支 {} → {}（{}）：{gate_note}",
        outcome.branch, project.default_branch, project.name
    );
    // 指纹换义（决策 212①）：不是「任务状态变没变」，而是「这个分支还能不能干净地 rebase
    // 到基准上」——分支不会因为别的事变迁而失效，而 base 会前进。
    let situation = serde_json::json!({
        "repair": {
            "branch": outcome.branch,
            "base_ref": outcome.base_ref,
            "base_commit": outcome.base_commit,
            "project_id": project.id,
        }
    });
    store
        .create_foreman_proposal(crate::storage::proposals::NewForemanProposal {
            session_id: session_id.to_string(),
            // 修复没有对应的工具端点：`tool` 这个名字是**执行分派的键**
            // （`run_proposal_tool` 按它走 `repair` 那一支），不是某个工具的名字。
            tool: "repair".to_string(),
            args: serde_json::json!({"project_id": project.id}),
            summary,
            situation: Some(situation),
            kind: crate::storage::proposals::ForemanProposalKind::Repair,
            payload: Some(serde_json::to_value(outcome)?),
        })
        .await
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
