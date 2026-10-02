//! 续接简报（决策 376 裁决② · 票 04）：超时梯子第 3 档（空白重跑）的**起跑现场**。
//!
//! ## 为什么有这个模块
//!
//! 事故形状（`.scratch/monitor-01M3X472FJF8NW9082K6BFZPKC.md`）：一个只读走查任务连续超时，
//! 每轮重置都从零重新侦察——git status/log、freshness 检查、产物是否已存在、重跑 playwright
//! ——而侦察结果只活在**转录**里。第 3 档的空白重跑把转录也丢了，于是同一段侦察第三次花钱买。
//!
//! 本模块把那四样东西**落成文本**交给新起的一段对话：任务描述、阶段产物文件清单、
//! 未提交改动清单、最近收口摘要。它不是转录的摘要（那是 L3 压缩的活），而是**现场投影**：
//! 每条都能从库 / 盘 / git 直接读到，没有一句是推测。
//!
//! ## 边界
//!
//! - **永不出错**：每一节各自降级为一句「读不到」的说明。这一档本来就是兜底，缺一节
//!   远好过让整轮组装失败退出（`build` 的返回类型因此不是 `Result`）。
//! - **有界**：目录清单 ≤ `MAX_FILES` 条、未提交改动 ≤ `DIRTY_FILES_MAX`（在 `git.rs`）。
//! - **不带转录**：形态由 `RunLedger::take_continuation` 的原因分流决定（`ContinuationMode::Brief`）。
//! - 人按 resume 与梯子第 1–2 档**不经过这里**（决策 320 / 376 的边界）。

use std::path::{Path, PathBuf};

use crate::git::Git;
use crate::storage::Store;
use crate::types::{Node, Stage, Task};

/// 任务目录清单的条数上限（含子目录项）。
const MAX_FILES: usize = 60;
/// 递归深度上限：任务目录自身算第 1 层，够覆盖 `.scratch/<feature>/issues/` 这类两层。
const MAX_DEPTH: usize = 3;

/// 组装空白重跑的起跑简报。**永不出错**（见模块 doc）。
pub(crate) async fn build(store: &Store, task: &Task, stage: Stage, node: Node) -> String {
    let home = store.home();
    let worktree = task
        .worktree_path
        .clone()
        .unwrap_or_else(|| home.worktree_path(&task.id).display().to_string());
    let task_dir = home.task_dir(&task.id);

    let mut out = String::new();
    out.push_str("## 任务描述\n");
    out.push_str(&format!("### {}\n", task.title));
    let desc = task.description.trim();
    out.push_str(if desc.is_empty() {
        "（无描述）"
    } else {
        desc
    });
    out.push('\n');

    out.push_str("\n## 阶段产物文件清单\n");
    out.push_str(
        &products_section(store, task, stage, node, &task_dir, Path::new(&worktree)).await,
    );

    out.push_str("\n## 未提交改动清单（worktree）\n");
    out.push_str(&worktree_section(Path::new(&worktree)).await);

    out.push_str("\n## 最近收口摘要\n");
    out.push_str(&last_outcome_section(store, task, stage, node).await);
    out
}

/// 本节点的**目标**（「未落盘目标」那一条要交代的东西）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProductTarget {
    /// 统一落在任务目录下的一个规范文件（决策 115 的缺省名）。
    File(&'static str),
    /// 产出是 worktree 里的代码改动——目标不是一个文件，点名「看未提交改动清单」。
    WorktreeChanges,
    /// 校验类节点：没有产物目标。
    None,
}

/// 节点 → 目标产物。**这几个缺省文件名有三处同源**：这里的表、`model_request::template_vars`
/// 的 `stored_path` 缺省、`model_invoke` 落 `upsert_stage_output` 时的 `unwrap_or_else`——
/// 改一处就得三处一起改，否则简报点名的文件名会与真正落盘的那个不是同一个。
fn product_target(stage: Stage, node: Node) -> ProductTarget {
    match (stage, node) {
        (Stage::ArchitectDesign, Node::Execute) => ProductTarget::File("design.md"),
        (Stage::DevelopDesign, Node::Execute) => ProductTarget::File("dev-plan.md"),
        (Stage::TestDesign, Node::Execute) => ProductTarget::File("test-scenarios.md"),
        (Stage::Review, Node::Execute) => ProductTarget::File("review-report.md"),
        (Stage::Test, Node::Execute) => ProductTarget::File("test-report.md"),
        (Stage::Develop, Node::Execute) => ProductTarget::WorktreeChanges,
        _ => ProductTarget::None,
    }
}

/// 已登记的阶段产出 + 本节点目标 + 任务目录现状 + 工作区 `.scratch/` 现状。
///
/// 事故的产物落在**工作区**的 `.scratch/<feature>/` 下（票面点名的位置），而管线的
/// 阶段产出落在任务目录里——两处都列，漏掉任何一处都会让「上一轮的侦察」重新花钱买。
async fn products_section(
    store: &Store,
    task: &Task,
    stage: Stage,
    node: Node,
    task_dir: &Path,
    worktree: &Path,
) -> String {
    let mut out = String::new();
    match store.list_stage_outputs(&task.id).await {
        Ok(outputs) if outputs.is_empty() => {
            out.push_str("（还没有登记过任何阶段产出）\n");
        }
        Ok(outputs) => {
            out.push_str("已登记的阶段产出：\n");
            for o in &outputs {
                let exists = task_dir.join(&o.file_path).exists();
                out.push_str(&format!(
                    "- {} / {} → {}{}\n",
                    o.stage.as_str(),
                    o.output_type,
                    o.file_path,
                    if exists { "" } else { "（未落盘）" }
                ));
            }
        }
        Err(e) => out.push_str(&format!("（阶段产出登记读不到：{e}）\n")),
    }

    match product_target(stage, node) {
        ProductTarget::File(name) => {
            let on_disk = task_dir.join(name).exists();
            out.push_str(&format!(
                "\n本节点（{}.{}）的目标产物：{name}{}\n",
                stage.as_str(),
                node.as_str(),
                if on_disk {
                    ""
                } else {
                    "（**未落盘**——这就是本轮要落的盘）"
                }
            ));
        }
        ProductTarget::WorktreeChanges => {
            out.push_str(&format!(
                "\n本节点（{}.{}）的目标是 worktree 里的代码改动——见下方「未提交改动清单」；\n\
                 那里已有的改动**就是上一轮的进度**，不要重做。\n",
                stage.as_str(),
                node.as_str()
            ));
        }
        ProductTarget::None => {}
    }

    out.push_str("\n任务目录现状（管线产物落这里）：\n");
    out.push_str(&listing_block(&list_dir(task_dir, MAX_DEPTH, MAX_FILES)));

    out.push_str("\n工作区 `.scratch/` 现状（任务自身产物常落这里）：\n");
    let scratch = worktree.join(".scratch");
    if scratch.is_dir() {
        let lines: Vec<String> = list_dir(&scratch, MAX_DEPTH, MAX_FILES)
            .into_iter()
            .map(|l| format!(".scratch/{l}"))
            .collect();
        out.push_str(&listing_block(&lines));
    } else {
        out.push_str("（不存在——本任务还没往这里落过东西）\n");
    }
    out
}

fn listing_block(lines: &[String]) -> String {
    if lines.is_empty() {
        return "（空）\n".to_string();
    }
    let mut s = String::new();
    for line in lines {
        s.push_str(&format!("- {line}\n"));
    }
    s
}

/// worktree 里未提交的改动：逐条列出，并显式提醒「别重做、别丢」。
async fn worktree_section(worktree: &Path) -> String {
    match Git.dirty_files(worktree).await {
        Ok(files) if files.is_empty() => "（工作区干净：没有未提交改动）\n".to_string(),
        Ok(files) => {
            let mut s = String::from(
                "以下改动**已经在 worktree 里**，重跑不会清掉它们——不要重做，\
                 收尾时确认它们仍在、没有被自己的新改动覆盖：\n",
            );
            for f in files {
                s.push_str(&format!("- {f}\n"));
            }
            s
        }
        Err(e) => format!("（读不到 worktree 状态：{e}）\n"),
    }
}

/// 本节点**最近一条已收口**的 run 的记录（状态 / 耗时 / 错误）——「上一轮是怎么结束的」。
///
/// 取「最后一条**非 running**」而不是「最后一条」：起跑这一轮自己的 run 行此刻已经落库
/// （`ledger.begin` 在组装之前），拿最后一条只会读到「running，耗时 0 秒」——那是当下，
/// 不是历史。全是 running（不该发生）时如实说一句。
async fn last_outcome_section(store: &Store, task: &Task, stage: Stage, node: Node) -> String {
    match store.list_runs_at(&task.id, stage, node).await {
        Ok(runs) => match runs
            .iter()
            .rev()
            .find(|r| r.status != crate::types::NodeStatus::Running)
        {
            Some(r) => {
                let mut s = format!(
                    "上一轮 attempt {}：{}，耗时 {} 秒。\n",
                    r.attempt,
                    r.status.as_str(),
                    r.duration_ms / 1000
                );
                if let Some(err) = r.error.as_deref().filter(|e| !e.trim().is_empty()) {
                    s.push_str("收口记录：\n");
                    s.push_str(err.trim());
                    s.push('\n');
                }
                s
            }
            None => "（本节点还没有已收口的历史 run）\n".to_string(),
        },
        Err(e) => format!("（读不到历史 run：{e}）\n"),
    }
}

/// 目录清单（深度 ≤ `max_depth`、条数 ≤ `max`、按名字排序）。读不到就返回空表——
/// 上层按「空目录」呈现，不报错。
fn list_dir(root: &Path, max_depth: usize, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    walk(root, "", max_depth, max, &mut out);
    out.sort();
    out
}

fn walk(dir: &Path, prefix: &str, depth: usize, max: usize, out: &mut Vec<String>) {
    if depth == 0 || out.len() >= max {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if out.len() >= max {
            return;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let is_dir = path.is_dir();
        if is_dir {
            out.push(format!("{rel}/"));
            walk(&path, &rel, depth - 1, max, out);
        } else {
            match std::fs::metadata(&path) {
                Ok(meta) => out.push(format!("{rel}（{} 字节）", meta.len())),
                Err(_) => out.push(rel),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_target_names_the_goal_of_each_execute_node() {
        assert_eq!(
            product_target(Stage::ArchitectDesign, Node::Execute),
            ProductTarget::File("design.md")
        );
        assert_eq!(
            product_target(Stage::Develop, Node::Execute),
            ProductTarget::WorktreeChanges,
            "develop 的产物是 worktree 里的代码改动，不是一个文件"
        );
        assert_eq!(
            product_target(Stage::ArchitectDesign, Node::ValidateInput),
            ProductTarget::None,
            "校验节点没有产物"
        );
    }

    #[test]
    fn list_dir_is_nested_bounded_and_sorted() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("issues")).unwrap();
        std::fs::write(root.join("spec.md"), "x").unwrap();
        std::fs::write(root.join("issues/01.md"), "y").unwrap();

        let listing = list_dir(root, MAX_DEPTH, MAX_FILES);
        assert_eq!(
            listing,
            vec![
                "issues/".to_string(),
                "issues/01.md（1 字节）".to_string(),
                "spec.md（1 字节）".to_string()
            ]
        );

        // 条数上限：只取前 N 条（排序后）——「有界」不是口号。
        assert_eq!(list_dir(root, MAX_DEPTH, 1).len(), 1);
    }

    #[test]
    fn list_dir_degrades_to_empty_when_unreadable() {
        assert!(list_dir(
            Path::new("/definitely/not/here/at-all"),
            MAX_DEPTH,
            MAX_FILES
        )
        .is_empty());
    }
}
