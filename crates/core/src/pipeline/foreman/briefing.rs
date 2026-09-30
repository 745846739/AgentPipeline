use serde::{Deserialize, Serialize};

use crate::storage::tasks::TaskFilter;
use crate::storage::Store;
use crate::types::TaskStatus;
use crate::{Error, Result};

/// 夜班态势快照（决策 182⑬）：**只装「需要有人管的」**。
///
/// 不装全量看板、不装运行读数——那些用 `read_task` 按需查。快照是每轮都要付的固定成本，
/// 装得越多，能留给历史窗口的预算越少。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForemanBriefing {
    /// 等人拍板的任务。**必须带原因原文**（`message`），否则值班长只能说枚举名
    /// ——「merge_approval」对人没有信息量，「合入提案等你拍板」才有。
    pub pending: Vec<BriefingPending>,
    pub running: Vec<BriefingRunning>,
    pub failed: Vec<BriefingFailed>,
    pub projects: Vec<BriefingProject>,
    /// 已完成的计数（不列清单——收工的不需要有人管）。
    pub done_count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BriefingPending {
    pub task_id: String,
    pub title: String,
    pub stage: String,
    pub kind: String,
    /// pending 原因的**原文**，取自 `PendingReason::message`。
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BriefingRunning {
    pub task_id: String,
    pub title: String,
    pub stage: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BriefingFailed {
    pub task_id: String,
    pub title: String,
    pub stage: String,
    /// 失败原因（游标上的 pending message 或任务级错误），可能没有。
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BriefingProject {
    pub id: String,
    pub name: String,
}

impl ForemanBriefing {
    /// 渲染成注入 prompt 的文本。
    ///
    /// 空 home 给出明确的「空班」句而不是空字符串：值班长看到一段空白会开始自己编现状
    /// （「暂无数据」和「没有任务」在模型眼里不是一回事）。这句话同时也把「首启该干什么」
    /// 的引导落到事实层——值班经理问「现在能做什么」时它有据可依。
    pub fn render(&self) -> String {
        let mut out = String::from("## 夜班态势快照（每轮重新读取，以此为准）\n");
        if self.projects.is_empty() {
            out.push_str("项目：一个都没有（这台机器还没接入任何项目）。\n");
        } else {
            let names: Vec<String> = self
                .projects
                .iter()
                .map(|p| format!("{}（{}）", p.name, p.id))
                .collect();
            out.push_str(&format!("项目：{}。\n", names.join("、")));
        }
        out.push_str(&format!(
            "任务：{} 个在跑、{} 个等人拍板、{} 个已失败、{} 个已完成。\n",
            self.running.len(),
            self.pending.len(),
            self.failed.len(),
            self.done_count
        ));
        if self.pending.is_empty() {
            out.push_str("等人拍板的：无。\n");
        } else {
            out.push_str("等人拍板的：\n");
            for p in &self.pending {
                out.push_str(&format!(
                    "- [{}] 「{}」停在 {}，原因：{}（内部类型 {}）\n",
                    p.task_id, p.title, p.stage, p.message, p.kind
                ));
            }
        }
        if !self.running.is_empty() {
            out.push_str("在跑的：\n");
            for r in &self.running {
                out.push_str(&format!(
                    "- [{}] 「{}」当前在 {}\n",
                    r.task_id, r.title, r.stage
                ));
            }
        }
        if !self.failed.is_empty() {
            out.push_str("失败的：\n");
            for f in &self.failed {
                out.push_str(&format!(
                    "- [{}] 「{}」停在 {}{}\n",
                    f.task_id,
                    f.title,
                    f.stage,
                    f.message
                        .as_ref()
                        .map(|m| format!("，原因：{m}"))
                        .unwrap_or_default()
                ));
            }
        }
        if self.pending.is_empty() && self.running.is_empty() && self.failed.is_empty() {
            out.push_str("当前没有任何需要你处理的事。\n");
        }
        out
    }
}

/// 从台账组装快照（决策 182⑬）。
///
/// 各表为空 → 空清单，**不报错**：首启空 home 是合法状态，而且是最该能对话的一种
/// （用户故事 11：一台全新机器上第一次打开就能被引导去开工）。
pub async fn build_briefing(store: &Store) -> Result<ForemanBriefing> {
    let tasks = store
        .list_tasks(&TaskFilter {
            include_archived: false,
            ..Default::default()
        })
        .await?;
    let projects = store.list_projects().await?;

    let mut pending = Vec::new();
    let mut running = Vec::new();
    let mut failed = Vec::new();
    let mut done_count = 0usize;
    for t in &tasks {
        match t.status {
            TaskStatus::Pending => pending.push(BriefingPending {
                task_id: t.id.clone(),
                title: t.title.clone(),
                stage: t.current_stage.as_str().to_string(),
                kind: t
                    .pending_reason
                    .as_ref()
                    .map(|r| r.kind.as_str().to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                // 原因原文。缺失时给一句可读的兜底，而不是空串——
                // 空串会让值班长把这一条读成「无原因」。
                message: t
                    .pending_reason
                    .as_ref()
                    .map(|r| r.message.clone())
                    .unwrap_or_else(|| "（后端未给出原因原文）".to_string()),
            }),
            TaskStatus::Running => running.push(BriefingRunning {
                task_id: t.id.clone(),
                title: t.title.clone(),
                stage: t.current_stage.as_str().to_string(),
            }),
            TaskStatus::Failed => failed.push(BriefingFailed {
                task_id: t.id.clone(),
                title: t.title.clone(),
                stage: t.current_stage.as_str().to_string(),
                message: t.pending_reason.as_ref().map(|r| r.message.clone()),
            }),
            TaskStatus::Done => done_count += 1,
            TaskStatus::Queued | TaskStatus::Waiting | TaskStatus::Cancelled => {}
        }
    }
    Ok(ForemanBriefing {
        pending,
        running,
        failed,
        projects: projects
            .into_iter()
            .map(|p| BriefingProject {
                id: p.id,
                name: p.name,
            })
            .collect(),
        done_count,
    })
}

/// 提议的**态势指纹**（决策 207 的拒执判据）：任务状态 + 后端此刻下发的动作集。
///
/// 「现在的情况已经不是它当时说的那样」这句话要有东西可比——提议成立时存一份，
/// 执行时再取一份，两者不等即拒执。取的三样是**会让人改变主意**的东西：
/// 任务状态（`queued` 与 `pending` 是两回事）、当前工位（阶段 / 节点变了），
/// 以及 `allowed_actions`（那颗键还在不在，由后端权威下发，决策 101）。
///
/// **任务不存在也是一份合法的指纹**（`{"missing": true}`）：模型可能提了一个后来被删掉的
/// 任务，那也是「情况变了」，而且是最该被拒的一种。
pub async fn situation_fingerprint(store: &Store, task_id: &str) -> Result<serde_json::Value> {
    let task = match store.get_task(task_id).await {
        Ok(t) => t,
        Err(Error::Task(_)) => {
            return Ok(serde_json::json!({ "task_id": task_id, "missing": true }))
        }
        Err(e) => return Err(e),
    };
    let actions: Vec<String> = task
        .pending_reason
        .as_ref()
        .map(|r| crate::actions::allowed_actions(r, None))
        .unwrap_or_default()
        .into_iter()
        .map(|a| a.action)
        .collect();
    Ok(serde_json::json!({
        "task_id": task.id,
        "status": task.status.as_str(),
        "stage": task.current_stage.as_str(),
        "node": task.current_node.as_str(),
        "allowed_actions": actions,
    }))
}

/// 态势漂移的一句话说明；没漂移时 `None`（决策 207 的「拒执并报出」）。
///
/// **整份比较**而不是逐字段挑着比：指纹里的三样都是判据的一部分，挑着比就得为每一处
/// 新增字段补一行，而漏掉的那一行会让一条本该被拒的提议通过。逐字段只用来**说清楚**
/// 变的是哪一样——那是给人看的理由，不是判定本身。
pub fn situation_drift(before: &serde_json::Value, after: &serde_json::Value) -> Option<String> {
    if before == after {
        return None;
    }
    if before.get("missing") == Some(&serde_json::Value::Bool(true))
        || after.get("missing") == Some(&serde_json::Value::Bool(true))
    {
        return Some("那个任务在台账里已经找不到（或刚刚才出现）".to_string());
    }
    let mut changed: Vec<String> = Vec::new();
    if before.get("status") != after.get("status") {
        changed.push(format!(
            "任务状态从 {} 变成了 {}",
            cell(before, "status"),
            cell(after, "status")
        ));
    }
    if before.get("stage") != after.get("stage") || before.get("node") != after.get("node") {
        changed.push(format!(
            "当前工位从 {}.{} 换到了 {}.{}",
            cell(before, "stage"),
            cell(before, "node"),
            cell(after, "stage"),
            cell(after, "node")
        ));
    }
    if before.get("allowed_actions") != after.get("allowed_actions") {
        changed.push(format!(
            "可按下的事从 [{}] 变成了 [{}]",
            list(before, "allowed_actions"),
            list(after, "allowed_actions")
        ));
    }
    if changed.is_empty() {
        changed.push("它当时依据的那份读数已经对不上了".to_string());
    }
    Some(changed.join("；"))
}

fn cell(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string()
}

fn list(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(" / ")
        })
        .unwrap_or_else(|| "?".to_string())
}
