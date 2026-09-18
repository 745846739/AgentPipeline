//! 提议的写入侧（决策 188 / 207，票 02 / 04 / 05 / 06）。
//!
//! 工具层（[`crate::agent::tools`]）说的是「这次调用要变成一条提议」；**从参数到人读的
//! 那句话、以及态势指纹的取法**落在这里——它们是呈现与台账的知识，工具层不该有第二份。
//!
//! **提议 ≠ 动作**（决策 188 的第一约束）：这里只落一条「打算做什么」。执行是
//! `POST /foreman/proposals/{id}/execute` 的事，它按提议里的 `(工具, 参数)` 走
//! **既有的那条**端点——同一套校验、同一套闸门。本模块**不执行任何东西**。

use std::sync::Arc;

use futures::future::BoxFuture;

use crate::agent::tools::{ProposalRequest, ProposalSink};
use crate::sse::{SseEvent, SseSink};
use crate::storage::proposals::NewForemanProposal;
use crate::storage::Store;
use crate::Result;

/// 生产实现：落库 + 广播（决策 207 的提议事件）。
pub struct StoreProposalSink {
    store: Store,
    sse: Arc<dyn SseSink>,
}

impl StoreProposalSink {
    pub fn new(store: Store, sse: Arc<dyn SseSink>) -> Self {
        StoreProposalSink { store, sse }
    }
}

impl ProposalSink for StoreProposalSink {
    fn propose(&self, request: ProposalRequest) -> BoxFuture<'static, Result<String>> {
        let store = self.store.clone();
        let sse = self.sse.clone();
        Box::pin(async move {
            // 态势指纹只在**参数里带 `task_id`** 时取（决策 207 的拒执判据）。
            //
            // 规则是**一条**而不是每个工具一个开关：任务族的提议都能回答「它当时说的是哪个
            // 任务、那时是什么样」，而文件 / 命令族的提议**没有**态势可判——那种情况下提议
            // 的成立与否由端点自己的校验回答（同一套校验，不是第二套）。
            //
            // **判据取 `args.task_id`，不是 `ProposalRequest.task_id`**：后者是**调用上下文**
            // 的归属，而值班长不挂任务（`foreman_ctx` 给它空 task_id）；任务族的提议把 id 放在
            // **参数**里（`{"action":"cancel","task_id":"t1"}`）。照上下文取的话指纹**永远是
            // None**，而「执行时态势变了就拒执」这条规则会静默失效——一条按下去要在**当时**
            // 才成立的事，靠的就是它。
            let situation = match task_id_of(&request) {
                Some(task_id) => {
                    Some(super::foreman::situation_fingerprint(&store, &task_id).await?)
                }
                None => None,
            };
            let summary = proposal_summary(&request);
            let proposal = store
                .create_foreman_proposal(NewForemanProposal {
                    session_id: request.session_id.clone().unwrap_or_default(),
                    tool: request.tool.clone(),
                    args: request.args.clone(),
                    summary: summary.clone(),
                    situation,
                })
                .await?;
            if !proposal.session_id.is_empty() {
                sse.emit(SseEvent::ForemanProposal {
                    task_id: String::new(),
                    branch: String::new(),
                    session_id: proposal.session_id.clone(),
                    proposal_id: proposal.id.clone(),
                    tool: proposal.tool.clone(),
                    status: proposal.status.as_str().to_string(),
                    summary: proposal.summary.clone(),
                    expires_at: proposal.expires_at.to_rfc3339(),
                });
            }
            // 回灌给模型的那句话把它**止在此处**：写工具调用不是一次失败，模型该接着把话
            // 说完（「我已经提了这件事，等值班经理按键」），而不是换一个写法再试一遍。
            Ok(format!(
                "已生成一条待确认的提议（{}）：{summary}。\
                 这件事**没有执行**——它需要值班经理在界面上按下确认钮。\
                 不要换一种写法重试，也不要说你已经做了这件事；继续把你要报告的话说完即可。",
                proposal.id
            ))
        })
    }
}

/// 这条提议说的是哪个任务（`None` = 与任务无关）。
///
/// 两个来源按序看：**参数**里的 `task_id`（任务族的提议都放在那里）优先，其次才是调用上下
/// 文的归属（流水线节点自己发的调用才有它）。取一致的判据是「它当时说的是哪个任务」——
/// 那是执行时要拿来对比的那一个。
fn task_id_of(request: &ProposalRequest) -> Option<String> {
    request
        .args
        .get("task_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| request.task_id.clone().filter(|s| !s.is_empty()))
}

/// 提议的一句话说明（决策 207 的 `summary`）。
///
/// **由系统从 `(工具, 参数)` 生成，不是模型写的自由文本**。理由是这条说明的作用：
/// 它是人在按下之前唯一读的那一行，而模型完全可以在这里写一句与参数不符的话
/// （「只是读一个文件」配着一条 `rm -rf`）。人看到的必须与将要发生的是同一件事。
pub fn proposal_summary(request: &ProposalRequest) -> String {
    let args = &request.args;
    let s = |key: &str| args.get(key).and_then(|v| v.as_str()).unwrap_or("");
    match request.tool.as_str() {
        "write_file" => format!("写入文件 {}", s("path")),
        "edit_file" => format!("修改文件 {}", s("path")),
        "delete_file" => format!("删除文件 {}", s("path")),
        "run_command" => format!("执行命令：{}", first_line(s("command"))),
        // D 层三族（决策 207④：一族一个工具 + 动作参数）。**这一层更要紧**——它改的是
        // 流水线的事实，而人读到的只有这一行；回落到「调用 task」等于让人对着一个工具名按键。
        "task" => task_summary(request),
        "config" => format!(
            "{}阶段配置 {}",
            if s("action") == "delete" {
                "删除"
            } else {
                "改写"
            },
            s("stage")
        ),
        "skills" => {
            if s("action") == "delete" {
                format!("卸载技能 {}", s("name"))
            } else {
                format!("安装技能（来自 {}）", s("path"))
            }
        }
        // 全局动作（决策 210⑧ / 票 09）：文案必须说清它的影响面——「会打断在跑的任务」
        // 是值班经理按下之前唯一能读到的一句话。
        "service" => "重启服务（**会打断所有在跑的任务**；按下后先清理占用并把中断的任务归队，\
                      本进程没有自重启能力，需要你在启动它的地方重启一次）"
            .to_string(),
        other => format!("调用 {other}"),
    }
}

/// `task` 族的说明：**动作 + 哪个任务 + 那个动作自己的关键参数**。
fn task_summary(request: &ProposalRequest) -> String {
    let args = &request.args;
    let s = |key: &str| args.get(key).and_then(|v| v.as_str()).unwrap_or("");
    let task_id = s("task_id");
    match s("action") {
        "create" => format!("新建任务「{}」（项目 {}）", s("title"), s("project_id")),
        // 恢复动作由模型填（`continue` / `skip` / …），与动作集同一套词表
        "resume" => format!("让任务 {task_id} 的 {} 继续", s("resume_action")),
        "retry" => format!("重跑任务 {task_id}"),
        "unstick" => format!("解除任务 {task_id} 的僵死占用（清执行者 + 标终态 + 转 pending）"),
        "cancel" => format!("取消任务 {task_id}"),
        "review" => format!(
            "{}任务 {task_id} 的人工评审",
            if args.get("approved").and_then(|v| v.as_bool()) == Some(true) {
                "通过"
            } else {
                "打回"
            }
        ),
        "merge" => format!(
            "对任务 {task_id} 的合入提案{}",
            if s("decision") == "approve" {
                "按下合入"
            } else {
                "选择返回修改"
            }
        ),
        // 认不出的动作（模型报了个不存在的）：照抄，不编——端点的校验会给出真正的答案
        other => format!("对任务 {task_id} 执行 {other}"),
    }
}

/// 命令的一行摘要：多行命令只留第一行（说明不是正文，长的东西在参数里）。
fn first_line(text: &str) -> String {
    let head = text.lines().next().unwrap_or("").trim();
    if head.is_empty() {
        return "（空命令）".to_string();
    }
    if text.lines().count() > 1 {
        format!("{head} …（共 {} 行）", text.lines().count())
    } else {
        head.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn req(tool: &str, args: serde_json::Value) -> ProposalRequest {
        ProposalRequest {
            session_id: Some("s1".into()),
            task_id: None,
            tool: tool.into(),
            args,
        }
    }

    /// 说明里的路径 / 命令**来自参数本身**，故人读到的那行与将要发生的是同一件事。
    #[test]
    fn the_summary_is_generated_from_the_args() {
        assert_eq!(
            proposal_summary(&req("write_file", json!({"path": "notes.md"}))),
            "写入文件 notes.md"
        );
        assert_eq!(
            proposal_summary(&req("run_command", json!({"command": "cargo test --all"}))),
            "执行命令：cargo test --all"
        );
        // 多行命令只留第一行 + 行数（说明不是正文）
        assert_eq!(
            proposal_summary(&req(
                "run_command",
                json!({"command": "cd /tmp\nrm -rf x\necho hi"})
            )),
            "执行命令：cd /tmp …（共 3 行）"
        );
        // 认不出的工具不编故事，只说调用了什么
        assert_eq!(
            proposal_summary(&req("task_action", json!({"action": "resume"}))),
            "调用 task_action"
        );
        // 缺参数也不 panic（模型可能漏字段；参数错由端点自己拒）
        assert_eq!(proposal_summary(&req("write_file", json!({}))), "写入文件 ");
    }

    /// D 层三族的说明（决策 207④）：人按键之前读到的那一行要能回答「它到底要干什么」。
    ///
    /// 这一条比文件 / 命令那几条更要紧——那两族的工具自己就说明了动作，而 `task` 只有
    /// 一个工具名，回落成「调用 task」等于让人对着一个名字按键。
    #[test]
    fn the_service_write_tools_say_what_they_will_do() {
        assert_eq!(
            proposal_summary(&req(
                "task",
                json!({"action": "create", "title": "修好登录", "project_id": "p1"})
            )),
            "新建任务「修好登录」（项目 p1）"
        );
        assert_eq!(
            proposal_summary(&req("task", json!({"action": "cancel", "task_id": "01KX"}))),
            "取消任务 01KX"
        );
        assert_eq!(
            proposal_summary(&req(
                "task",
                json!({"action": "resume", "task_id": "01KX", "resume_action": "continue"})
            )),
            "让任务 01KX 的 continue 继续"
        );
        assert_eq!(
            proposal_summary(&req(
                "task",
                json!({"action": "merge", "task_id": "t", "decision": "approve"})
            )),
            "对任务 t 的合入提案按下合入"
        );
        assert_eq!(
            proposal_summary(&req(
                "task",
                json!({"action": "review", "task_id": "t", "approved": false})
            )),
            "打回任务 t 的人工评审"
        );
        assert_eq!(
            proposal_summary(&req("config", json!({"action": "set", "stage": "develop"}))),
            "改写阶段配置 develop"
        );
        assert_eq!(
            proposal_summary(&req(
                "config",
                json!({"action": "delete", "stage": "develop"})
            )),
            "删除阶段配置 develop"
        );
        assert_eq!(
            proposal_summary(&req(
                "skills",
                json!({"action": "install", "path": "/tmp/s/grill"})
            )),
            "安装技能（来自 /tmp/s/grill）"
        );
        assert_eq!(
            proposal_summary(&req("skills", json!({"action": "delete", "name": "grill"}))),
            "卸载技能 grill"
        );
        // 认不出的动作照抄，不编
        assert_eq!(
            proposal_summary(&req("task", json!({"action": "teleport", "task_id": "t"}))),
            "对任务 t 执行 teleport"
        );
    }

    /// 态势指纹的判据取 **`args.task_id`**：值班长不挂任务，上下文里的 task_id 恒为空。
    ///
    /// 这一条钉的是一个会让整条拒执规则静默失效的错法——照上下文取的话，任务族提议的
    /// `situation` 永远是 `None`，而「执行时态势变了就拒执」再也拦不住任何东西。
    #[test]
    fn the_task_id_for_the_situation_comes_from_the_args() {
        let mut r = req("task", json!({"action": "cancel", "task_id": "t-42"}));
        assert_eq!(task_id_of(&r).as_deref(), Some("t-42"));
        // 上下文里那份（流水线节点自己发的调用）仍然认
        r.args = json!({});
        r.task_id = Some("t-7".into());
        assert_eq!(task_id_of(&r).as_deref(), Some("t-7"));
        // 都没有 → 没有态势可判（文件 / 命令族）
        r.task_id = None;
        assert_eq!(task_id_of(&r), None);
        // 空串不是归属
        r.args = json!({"task_id": ""});
        r.task_id = Some(String::new());
        assert_eq!(task_id_of(&r), None);
    }
}
