//! 值班长：对讲台背后的对话 agent（决策 182，票 01 / 02 / 05）。
//!
//! **存在理由**：看板说得出「什么状态」，说不出「为什么」与「该怎么办」。值班长把夜班
//! 态势读成人话，需要深挖时它自己翻只读台账。它**不依赖任何任务的存在**——首启空 home
//! 也能对话（这是本特性最初被否掉的前提：「对话不需要依赖任务」）。
//!
//! ## 三条硬边界
//!
//! 1. **不动手。** 写动作（resume / retry / 拍板 / merge / 建任务）一律由后端下发、由人按下。
//!    值班长的工具集**只有两个只读台账工具**（[`FOREMAN_TOOLS`]），白名单在
//!    [`ToolExecutor::with_allowed_tools`] 的执行点强制。它的回复里也永远不出现按钮——
//!    这个约束落在前端（票 04），这里保证的是它**没有能力**改状态。
//! 2. **读不到文件系统。** 它的输入是人可以随便打的任意文本，而流水线自身调用的只读子代理
//!    不需要面对这个（[`crate::pipeline::subagent`]）。故工具集里没有 `read_file` / `list_dir`，
//!    也没有 `run_command`。
//! 3. **不新增阶段枚举变体。** 它占据 [`FOREMAN_STAGE_KEY`] 这一个配置 key。阶段枚举是
//!    kebab-case 的公开契约（落库列 / SSE 载荷 / 前端联合类型），且流水线图遍历「全部阶段」
//!    这个定长数组构建——加一个变体会让值班长要么被静默漏掉、要么被当成流水线的一个阶段
//!    去建节点连边。provider 解析沿用 `project_analysis` 已经走通的路子：借用占位阶段让既有
//!    解析链跑通，另按自己的 key 读阶段配置，把 provider 从请求的第二级传进去。
//!
//! ## 与只读子代理的关系
//!
//! 会话循环的形态照搬 [`crate::pipeline::subagent`]（模型不再调用工具即返回自由文本收口、
//! 轮数有上限、工具失败只回灌错误文本不上升为失败），但**不用** `submit_metadata`：值班长
//! 的产出是人读的一句话，不是给状态机消费的结构化元数据。

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::agent::client::{LlmClient, LlmRequest, Message, ToolDef};
use crate::agent::file_policy::FileToolPolicy;
use crate::agent::tools::{ToolCallContext, ToolExecutor};
use crate::config::Settings;
use crate::home::Home;
use crate::process::RealProcessKiller;
use crate::storage::foreman::{ForemanMessage, NewForemanMessage, FOREMAN_ROLE_ASSISTANT};
use crate::storage::tasks::TaskFilter;
use crate::storage::Store;
use crate::types::{CommandSource, Node, Stage, TaskStatus};
use crate::{Error, Result};

/// `stage_configs.stage` 的第 4 个伪键（决策 182①）。
///
/// 与 [`crate::pipeline::pseudo::PseudoStage`] 的三个键并列，但**不是** `PseudoStage` 的
/// 变体：那三个都是流水线节点内同步发起的调用，值班长与流水线无关。
pub const FOREMAN_STAGE_KEY: &str = "foreman";

/// run / SSE 载荷里的身份串（决策 182⑥）。
///
/// 现在不落 run 行（值班长没有运行行，见 `say` 的注释），但 SSE 增量事件按它过滤。
pub const FOREMAN_AGENT_TYPE: &str = "foreman";

/// 值班长**固定**的只读台账工具集（决策 182⑭）。
///
/// 与 [`crate::pipeline::subagent::SUB_AGENT_TOOLS`] 同一姿态：这是安全边界本身，不是配置项。
/// 任何「给值班长加个工具」的改动都必须先改这里，从而在 diff 里显式可见。
pub const FOREMAN_TOOLS: [&str; 2] = ["read_task", "read_conversation"];

/// 单次回话的最大工具往返轮数（决策 182④）。
///
/// 正常靠「模型不再发起 tool_call」自然结束；这个上限是防御性的——模型若陷入
/// 「查一个任务 → 再查一个」的循环，必须有人喊停，否则会持续烧 token 直到 HTTP 超时。
pub const FOREMAN_MAX_ROUNDS: usize = 8;

/// 历史窗口的字符预算（决策 182⑫）。
///
/// **按字符裁剪而不是硬编码轮数**：一轮的长短差两个数量级（「在吗」3 字 vs 贴一段回执
/// 2000 字），按轮数裁会让长轮挤出上下文、短轮浪费预算。被裁掉的历史仍在库里（`list_foreman_messages`
/// 只影响这一轮注入了什么，不影响台账）。
pub const FOREMAN_HISTORY_BUDGET_CHARS: usize = 24_000;

/// 一次性从库里取出的历史行数上限——真正的裁剪判据是字符预算，
/// 这个数字只是「别把整晚的对话都读进内存」的粗兜底。
const FOREMAN_HISTORY_FETCH_LIMIT: usize = 200;

/// 单条工具结果回灌进对话前的截断上限（字符）。
///
/// 压在 `offload_threshold_tokens`（默认 4000 token ≈ 16000 字符）**之下**：
/// 值班长的工具结果不该走 L2 卸载——那条路会把内容写到 `context_dir/{task_id}`，
/// 而值班长没有 task_id。截断比卸载诚实：模型看到的是「这里有 12000 字，这是全部」。
pub(crate) const FOREMAN_TOOL_RESULT_MAX_CHARS: usize = 12_000;

/// `read_conversation` 返回的最后 N 条消息、以及它们的总字符上限。
///
/// 公开给 crate 内是因为真正的截断发生在工具实现（`agent::tools`）里——两处各写一份迟早
/// 漂移，而漂移的后果是「值班长读到的回执比它以为的长」这种不显眼的超支。
pub(crate) const FOREMAN_CONVERSATION_MAX_MESSAGES: usize = 20;
pub(crate) const FOREMAN_CONVERSATION_MAX_CHARS: usize = 12_000;

/// 人格内嵌段（决策 182④：可被 `persona_path` / `persona_append` 覆盖，沿用决策 7）。
///
/// 语气是**冷静的值班长**：只报事实与建议，不寒暄、短句。理由写在决策 182㉒——
/// 「有性格」迟早会演变成「有自己的看法」，而那是一个会花钱、会被任意提问的 LLM
/// 最不该有的东西。
///
/// 对**人**的称呼（值班经理）与界面名牌是同一个词（决策 193）：两处各写各的，同一块屏幕上
/// 就会有两个称呼。这个名分漂过一次——「工头」曾同时指玩家与对面这个 agent（决策 174 挂标、
/// 176 裁决），故下面这个称呼由 `tests/foreman.rs` 用例锁住。
pub const FOREMAN_PERSONA: &str = "你是夜班车间的值班长，向值班经理汇报流水线态势。\
     你只报事实与建议：说清哪台工位卡了、卡在什么原因上、可以怎么做，并标出结论的出处\
     （哪个工位、哪次回执）。不寒暄、不恭维、不铺垫，短句优先。\
     你没有动手的权力——改状态的动作一律由值班经理按下，你只建议；\
     因此也绝不要声称你已经改了任何东西。\
     态势快照之外的细节用 read_task / read_conversation 自己查，不要凭印象猜。";

/// 值班长的前言。**不复用** [`crate::agent::prompts::build_system_prompt`]。
///
/// 那条路会强制拼上 `BASELINE_PREAMBLE`（「你是 AgentPipeline 的节点 agent……需要时用
/// read_file 读取」）与 `FORMAT_RULES`（「产出文件一律通过 write_file 写入」「结构化流转
/// 信息一律通过 submit_metadata 提交」）——三段指令都在让模型使用它**没有**的工具。
/// 指示一个模型去调不存在的工具，正是它开始编造文件内容的起点。
const FOREMAN_BASELINE: &str = "你在一个本机工具内运行，面对的是这台机器上的流水线台账。\
     你只能通过工具读取台账，读不到文件系统，也不能执行命令。";

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

/// 历史窗口按字符预算裁剪（决策 182⑫）：从**最新往回**取，返回选中项（时间升序）。
///
/// 两条不变式：
/// - **最新一条一定在内**（哪怕它自己就超预算）——它是本轮刚说出口的那句话，
///   把它裁掉会让值班长答非所问；
/// - 被裁掉的历史**仍留在库里**，只是这一轮没进 prompt。
pub fn trim_history(history: &[ForemanMessage], budget_chars: usize) -> Vec<ForemanMessage> {
    let mut selected: Vec<ForemanMessage> = Vec::new();
    let mut used = 0usize;
    for msg in history.iter().rev() {
        let cost = msg.content.chars().count();
        if !selected.is_empty() && used + cost > budget_chars {
            break;
        }
        used += cost;
        selected.push(msg.clone());
    }
    selected.reverse();
    selected
}

/// 该轮调用过的只读工具痕迹（票 05：进审计，与快照一起回答「依据什么」）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForemanTrace {
    pub tool: String,
    /// 参数摘要（截断的紧凑 JSON）。不存完整参数是因为它可能很长，
    /// 而审计要回答的是「它查了哪个任务的什么」，不是逐字复现调用。
    pub args_summary: String,
    pub ok: bool,
}

/// 一次回话的结果。
#[derive(Debug, Clone)]
pub struct ForemanTurn {
    pub reply: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub briefing: ForemanBriefing,
    pub traces: Vec<ForemanTrace>,
}

/// 值班长运行器。
///
/// 与 [`crate::pipeline::subagent::StoreSubAgentRunner`] 不同，它由 `AppState` 长期持有
/// （不是「一次父节点执行」构造一次）：值班长没有父节点，也没有运行行。
pub struct ForemanRunner {
    store: Store,
    settings: Settings,
    home: Home,
    llm: Arc<dyn LlmClient>,
}

impl ForemanRunner {
    pub fn new(store: Store, settings: Settings, home: Home, llm: Arc<dyn LlmClient>) -> Self {
        ForemanRunner {
            store,
            settings,
            home,
            llm,
        }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// 回一句话。
    ///
    /// 顺序是刻意的：**值班经理说的话先落库**，再叫模型，最后落值班长的回话。
    /// 中间任何一步失败，人说过的那句话仍在台账里（审计要的是「他说了什么」，
    /// 不是「他说的哪句话被成功答复了」）。
    ///
    /// **不落 run 行**：`kanban_node_runs` 的归属约束与指标口径都不接受一个无阶段、
    /// 无任务的运行行（决策 182⑨）。没有 run 行也就没有心跳可打——值班长的活性信号
    /// 是 HTTP 请求本身（决策 182⑦）。
    pub async fn say(&self, user_text: &str) -> Result<ForemanTurn> {
        let text = user_text.trim();
        if text.is_empty() {
            return Err(Error::Validation("空消息不入账".into()));
        }
        self.store
            .append_foreman_message(NewForemanMessage::user(text))
            .await?;

        let briefing = build_briefing(&self.store).await?;
        // 阶段配置读一次、用两个地方（人格 + provider / 采样参数）。中途再读一次不会
        // 有新值可读，却会让「人格用这一份、provider 用那一份」成为可能。
        let cfg = self.stage_config().await?;
        let system_prompt = self.system_prompt(cfg.as_ref())?;
        let provider_id =
            crate::storage::catalog::resolve_provider_id(None, None, cfg.as_ref(), None);

        let history = self
            .store
            .list_foreman_messages(FOREMAN_HISTORY_FETCH_LIMIT)
            .await?;
        let window = trim_history(&history, FOREMAN_HISTORY_BUDGET_CHARS);
        // `user_prompt` **只放快照**，问题由 transcript 的最后一条承担。
        //
        // 适配器组装的 body 是 `[system][user(user_prompt)] + messages`（见
        // `openai.rs::build_body`），所以把问题同时写进 user_prompt 和在 transcript 里
        // 再带一遍，模型会连着看到同一个问题两三次——白烧 token，还会让它以为是不同的话。
        // 快照放 user_prompt 而不是塞进 system：它每轮都变，进系统段会让 prompt cache
        // 每轮全失效（§12.13.5）。
        let user_prompt = briefing.render();

        let tools = ToolExecutor::new(
            self.home.clone(),
            // 值班长的工具集里没有任何文件工具，这个策略集合因此不可达；
            // 传家目录根而不是空表，是为了万一将来有人加了文件工具，
            // 默认边界仍是最紧的那个（家目录内），而不是「什么都不许」导致的假绿。
            FileToolPolicy::new(vec![self.home.root().to_path_buf()]),
            self.settings.clone(),
            Arc::new(RealProcessKiller),
        )
        .with_ledger(self.store.clone())
        .with_allowed_tools(&FOREMAN_TOOLS);
        let ctx = ToolCallContext {
            // 值班长不挂任务：这两个字段在它的两个工具里都不参与判定
            // （任务 id 来自工具参数，不是上下文）。
            task_id: String::new(),
            stage: Stage::Init,
            node: Node::Execute,
            worktree_path: PathBuf::from(self.home.root()),
            task_dir: PathBuf::from(self.home.root()),
            run_id: None,
            command_source: CommandSource::Agent,
            default_cwd: None,
        };

        let mut transcript: Vec<Message> = window
            .iter()
            .map(|m| {
                if m.role == crate::storage::foreman::FOREMAN_ROLE_USER {
                    Message::user(m.content.clone())
                } else {
                    Message::assistant(Some(m.content.clone()), Vec::new())
                }
            })
            .collect();
        // 历史最后一条就是刚落库的这句 user 消息；但若它被 `trim_history` 之外的原因
        // 漏掉（例如库被外部清空），仍要保证本轮的问题在场。
        match transcript.last() {
            Some(m) if m.role == crate::agent::client::Role::User => {}
            _ => transcript.push(Message::user(text.to_string())),
        }

        let mut tokens = (0u32, 0u32);
        let mut traces: Vec<ForemanTrace> = Vec::new();
        let mut reply: Option<String> = None;
        // 空内容与「轮数耗尽」是两回事，报错必须分得开——否则一次「模型返回空」会被
        // 说成「它可能一直在查台账」，把人引到完全错误的方向上去查。
        let mut empty_replies = 0usize;

        for _ in 0..FOREMAN_MAX_ROUNDS {
            let request = LlmRequest {
                // 占位阶段：让既有的 provider 解析链跑通。真正生效的 provider 从
                // `provider_id` 进来（决策 182②，与 project_analysis 同一路子）。
                stage: Stage::Init,
                node: Node::Execute,
                attempt: 1,
                system_prompt: system_prompt.clone(),
                user_prompt: user_prompt.clone(),
                messages: transcript.clone(),
                tools: Self::tool_defs(),
                temperature: cfg.as_ref().and_then(|c| c.temperature),
                max_tokens: cfg.as_ref().and_then(|c| c.max_tokens),
                provider_id: provider_id.clone(),
                run: Some(crate::agent::client::RunContext {
                    // 空 task id / 空分支 / 占位 run_id：既有任务级 SSE 路由按 task id
                    // 精确匹配，空串永不等于真实任务 id，故零干扰（决策 182⑥）。
                    task_id: String::new(),
                    branch: String::new(),
                    run_id: 0,
                    agent_type: FOREMAN_AGENT_TYPE.to_string(),
                }),
            };
            let response = self.llm.complete(request).await?;
            tokens.0 += response.prompt_tokens;
            tokens.1 += response.completion_tokens;
            transcript.push(Message::assistant(
                response.content.clone(),
                response.tool_calls.clone(),
            ));

            if response.tool_calls.is_empty() {
                reply = response.content.filter(|s| !s.trim().is_empty());
                if reply.is_none() {
                    empty_replies += 1;
                }
                break;
            }
            for call in &response.tool_calls {
                let args_summary = summarize_args(&call.arguments);
                let (content, ok) = match self.run_tool(&tools, call, &ctx).await {
                    Ok(outcome) => (outcome.content, true),
                    // 工具失败**不**上升为整次回话失败（§12.8 的同一姿态）：
                    // 把错误文本回给模型让它改道，而不是让人看到一条报错。
                    Err(e) => (format!("工具执行失败：{e}"), false),
                };
                traces.push(ForemanTrace {
                    tool: call.name.clone(),
                    args_summary,
                    ok,
                });
                transcript.push(Message::tool_result(call, truncate(&content)));
            }
        }

        let reply = reply.ok_or_else(|| {
            if empty_replies > 0 {
                Error::Validation(
                    "值班长这一轮没有回话（模型返回了空内容）。\n                     若这是本机第一次使用，先确认 provider 与模型名配对了；                     也可以换个模型再试——有些模型在被要求用工具时会返回空内容。"
                        .to_string(),
                )
            } else {
                Error::Validation(format!(
                    "值班长在 {FOREMAN_MAX_ROUNDS} 轮内没有给出回话——它可能一直在查台账"
                ))
            }
        })?;

        let traces_json = if traces.is_empty() {
            None
        } else {
            Some(serde_json::to_value(&traces)?)
        };
        let briefing_json = serde_json::to_value(&briefing)?;
        self.store
            .append_foreman_message(NewForemanMessage {
                role: FOREMAN_ROLE_ASSISTANT.to_string(),
                content: reply.clone(),
                prompt_tokens: tokens.0,
                completion_tokens: tokens.1,
                briefing_json: Some(briefing_json),
                traces_json,
            })
            .await?;

        Ok(ForemanTurn {
            reply,
            prompt_tokens: tokens.0,
            completion_tokens: tokens.1,
            briefing,
            traces,
        })
    }

    /// 工具定义。与 [`crate::pipeline::subagent::StoreSubAgentRunner::tool_defs`] 同样的
    /// 立场：**不经 `effective_tools`**——那条路会并入基线强制工具（含 `run_command` /
    /// `write_file`），正是本模块要挡掉的东西。
    fn tool_defs() -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "read_task".to_string(),
                description: "读某个任务的台账详情：标题、状态、当前工位、待办原因原文、\
                              后端下发的可用动作、各分支游标。卡住的细节问它。"
                    .to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "string", "description": "任务 id（快照里方括号内那串）" }
                    },
                    "required": ["task_id"]
                }),
            },
            ToolDef {
                name: "read_conversation".to_string(),
                description: "读某个任务某次节点运行的会话回执（工位当时说了什么）。\
                              run_id 省略时取该任务最近一次会话。引用工位结论时要标出来源。"
                    .to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "string", "description": "任务 id" },
                        "run_id": { "type": "integer", "description": "运行 id，省略取最近一次" }
                    },
                    "required": ["task_id"]
                }),
            },
        ]
    }

    async fn run_tool(
        &self,
        tools: &ToolExecutor,
        call: &crate::agent::client::ToolCall,
        ctx: &ToolCallContext,
    ) -> Result<crate::agent::tools::ToolOutcome> {
        tools.execute(call, ctx).await
    }

    /// `stage_configs["foreman"]` 的覆盖行（可能不存在——「不配置也能用」）。
    ///
    /// provider 解析（决策 182②）沿用 `project_analysis` 的路子：按**自己的 key** 读阶段配置，
    /// 取它的 provider 从 `LlmRequest::provider_id` 传进去（请求的第二级）。阶段配置里没有
    /// 这一行 → `None` → 适配器回落首个启用 provider。任务级覆盖在这里恒为 `None`：
    /// 值班长不挂任务，没有任务级可覆盖。
    async fn stage_config(&self) -> Result<Option<crate::types::StageConfig>> {
        self.store.get_stage_config(FOREMAN_STAGE_KEY).await
    }

    /// 系统提示词：`[须知][人格][工具纪律]`，人格可被 `persona_path` / `persona_append` 覆盖。
    ///
    /// 不注入技能（与只读子代理同一立场）：值班长是面向人的对话者，不是技能执行者；
    /// 把项目技能正文灌进来只会挤占历史窗口。
    fn system_prompt(&self, cfg: Option<&crate::types::StageConfig>) -> Result<String> {
        let persona = match cfg.and_then(|c| c.persona_path.as_deref()) {
            Some(path) => {
                let full = self.home.root().join(path);
                std::fs::read_to_string(&full).map_err(|e| {
                    Error::Config(format!(
                        "foreman persona_path 不可读（{}）：{e}",
                        full.display()
                    ))
                })?
            }
            None => FOREMAN_PERSONA.to_string(),
        };
        let mut out = format!("{FOREMAN_BASELINE}\n\n{persona}\n");
        if let Some(append) = cfg.and_then(|c| c.persona_append.as_deref()) {
            if !append.trim().is_empty() {
                out.push_str(&format!("\n{}\n", append.trim()));
            }
        }
        out.push_str(
            "\n## 工具纪律\n\
             - 只有两个只读工具：read_task / read_conversation。你没有写权限、没有命令执行权限，\
             也读不到文件系统。\n\
             - 快照里已经有的（待拍板原因、在跑、失败、项目清单）不要再查一遍。\n\
             - 引用工位结论时必须标出它来自哪个工位、哪次运行。\n\
             - 你不知道的事就说不知道。",
        );
        Ok(out)
    }
}

/// 参数摘要：紧凑 JSON，截到 200 字符。
fn summarize_args(raw: &str) -> String {
    truncate_to(raw, 200)
}

fn truncate_to(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let head: String = text.chars().take(max_chars).collect();
    format!("{head}…（已截断）")
}

fn truncate(text: &str) -> String {
    truncate_to(text, FOREMAN_TOOL_RESULT_MAX_CHARS)
}

/// 台账工具结果的字符上限（供 `agent::tools` 的两个工头工具调用）。
///
/// 与 [`truncate`] 同一份上限：会话循环里的回灌截断与工具自己产出的截断必须是同一个数，
/// 两处各写一份会让「工具说它截到 12000、循环又按 8000 截一次」这种无声缩水出现。
pub(crate) fn truncate_tool_result(text: &str) -> String {
    truncate(text)
}
