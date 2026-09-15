//! 只读子代理（决策 172③，票 08）。
//!
//! **存在理由**：把「读 20 个文件」的原文挡在父上下文之外，只回摘要。这让
//! `research`、`code-review`（两轴并行）、`codebase-design`（DESIGN-IT-TWICE）
//! 这批以子代理为前提的技能真正跑起来。
//!
//! **安全边界在本模块**（不是工具层）：子代理的工具集**硬编码为只读**——
//! [`SUB_AGENT_TOOLS`] 只有 `read_file` / `list_dir`。三条硬约束：
//!
//! 1. **不继承阶段声明的工具**：父节点声明了 `run_command` 也不会传给子代理，
//!    阶段配置因此无法给子代理扩权。工具集不从父节点**推导**，而是常量。
//! 2. **无 `run_command`**：子代理不能起 shell。本系统没有 OS 级沙箱（决策 19 修订 /
//!    104），一旦子代理能跑命令，它就成了注入攻击的加速通道——约束只剩进程边界。
//! 3. **不再派子代理**：深度固定一层（决策 9）。子代理的工具集里没有
//!    `spawn_sub_agent`，所以它在结构上就派不出去，不靠运行时计数拦截。
//!
//! **与 L4 的关系**（决策 154 的边界不变）：本模块给的是**技能可调用的能力**，
//! 不是上下文超限兜底手段。「L4 兜底只有两级」与「分批 / 拆子代理不作为 L4 兜底」
//! 的原裁决不受影响——`agent/context.rs` 的 `plan_l4` 一行未动。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::future::BoxFuture;

use crate::agent::client::{AgentResponse, LlmClient, LlmRequest, Message, ToolDef};
use crate::agent::file_policy::FileToolPolicy;
use crate::agent::prompts::{build_system_prompt, load_agents_context};
use crate::agent::tools::{SubAgentRequest, SubAgentRunner, ToolCallContext, ToolExecutor};
use crate::config::Settings;
use crate::home::Home;
use crate::process::ProcessKiller;
use crate::storage::observability::{NewRun, RunOutcome};
use crate::storage::Store;
use crate::types::{CommandSource, Node, NodeStatus, Stage};
use crate::{Error, Result};

/// 子代理的 run 行 / 会话行的 `agent_type`（决策 172③）。
pub const SUB_AGENT_TYPE: &str = "subagent";

/// 子代理**固定**的只读工具集（决策 172③，票 08）。
///
/// 这是安全边界本身，不是配置项：故意写成常量，任何「按阶段扩权」的改动都必须先改
/// 这里，从而在 diff 里显式可见。
pub const SUB_AGENT_TOOLS: [&str; 2] = ["read_file", "list_dir"];

/// 子代理的收尾前言：要求它只回摘要（父上下文要的是摘要，不是原文）。
const SUB_AGENT_PERSONA: &str = "你是一个只读检索子代理。你的唯一任务是按父代理给出的描述\
     检索并阅读文件，然后回**精炼摘要**：结论、关键位置（文件:行）、必要的短引文。\
     不要把读到的文件原文整段复制回来——父代理只要摘要，这正是你存在的理由。\
     你没有写权限与命令执行权限，也不需要它们。";

/// 单次子代理循环的最大轮数（工具往返）。
///
/// 子代理没有 `submit_metadata` 收口，正常靠「模型不再发起 tool_call」自然结束；
/// 这个上限是防御性的——模型若陷入「读一个文件 → 再读一个」的循环，必须有人喊停，
/// 否则会持续烧 token 直到外层超时。
pub const SUB_AGENT_MAX_ROUNDS: usize = 12;

/// 子代理运行的构造参数（避免构造点长成一排位置参数）。
///
/// `Clone` 是必需的：`SubAgentRunner::run` 返回 `BoxFuture<'static>`，future 必须
/// **拥有**自己需要的一切，不能借用 `&self`。
#[derive(Clone)]
pub struct SubAgentRunnerConfig {
    pub store: Store,
    pub settings: Settings,
    pub llm: Arc<dyn LlmClient>,
    pub killer: Arc<dyn ProcessKiller>,
    pub home: Home,
    pub task_id: String,
    /// 父节点的游标：子代理 run 行复用它，使 run 列表里子行与父行归属同一游标。
    pub cursor_id: String,
    pub stage: Stage,
    pub node: Node,
    /// 父节点的 attempt：子代理是挂在父节点下的辅助调用，不产生自己的 attempt。
    pub attempt: u32,
    pub branch: String,
    /// 父 run：子代理 run 行的 `parent_run_id`，心跳也刷它。
    pub parent_run_id: i64,
    pub worktree_path: PathBuf,
    pub task_dir: PathBuf,
    /// AGENTS.md 的来源（项目仓库根，与父节点同源）。
    pub project_root: PathBuf,
    pub language: Option<String>,
    pub test_framework: Option<String>,
    /// 父节点的阶段配置采样参数（子代理沿用，决策 46）。
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    /// 本次调用的上限（票 08：沿用节点级 `node_max_duration_sec`）。
    pub max_duration: Duration,
}

/// 子代理一次运行的 token 计量（与 executor 的 `RunTokens` 同口径，决策 100）。
#[derive(Debug, Clone, Copy, Default)]
pub struct RunTokens {
    pub prompt: u32,
    pub completion: u32,
    pub cache_read: u32,
    pub cache_write: u32,
}

impl RunTokens {
    fn add(&mut self, response: &AgentResponse) {
        self.prompt += response.prompt_tokens;
        self.completion += response.completion_tokens;
        self.cache_read += response.cache_read_tokens;
        self.cache_write += response.cache_write_tokens;
    }
}

/// 真正的子代理运行器（决策 172③，票 08）。
///
/// 按**一次父节点执行**（task + cursor + attempt + 父 run）构造：这些在同一次执行内
/// 固定，因此不放进每次调用的 [`SubAgentRequest`]。
pub struct StoreSubAgentRunner {
    cfg: SubAgentRunnerConfig,
}

impl StoreSubAgentRunner {
    pub fn new(cfg: SubAgentRunnerConfig) -> Self {
        StoreSubAgentRunner { cfg }
    }

    /// 子代理**固定只读**的工具定义。
    ///
    /// 注意它不经 `effective_tools`——那条路会并入基线强制工具（含 `run_command` /
    /// `write_file`），正是本票要挡掉的东西。
    fn tool_defs() -> Vec<ToolDef> {
        SUB_AGENT_TOOLS
            .iter()
            .map(|name| ToolDef {
                name: (*name).to_string(),
                description: String::new(),
                parameters: serde_json::json!({"type": "object"}),
            })
            .collect()
    }

    /// 检查该 agent_type 是否出现在子代理可用工具里（测试与文档的自证）。
    ///
    /// 单独成函数是为了让「子代理拿不到 run_command」这条断言有一个**实现侧的**
    /// 落点，而不是只在测试里硬编码字符串。
    pub fn allows_tool(name: &str) -> bool {
        SUB_AGENT_TOOLS.contains(&name)
    }
}

impl SubAgentRunner for StoreSubAgentRunner {
    fn run(&self, request: SubAgentRequest) -> BoxFuture<'static, Result<String>> {
        // future 是 `'static` 的，只能**拥有**自己需要的一切，不能借用 `&self`。
        // 因此克隆一份运行器进 future；`run_loop` 借用的是这个局部所有权，合法。
        let runner = StoreSubAgentRunner {
            cfg: self.cfg.clone(),
        };
        Box::pin(async move {
            let cfg = &runner.cfg;
            let started = Instant::now();
            let worktree = cfg.worktree_path.display().to_string();
            let task_dir = cfg.task_dir.display().to_string();

            // 子代理的系统前言是**摘要任务前言**，不是父节点 persona（决策 172③：
            // 「persona 可简化为摘要任务前言」）。父 persona 往往写着「你负责写
            // design.md」这类与检索无关的职责，会误导子代理。
            let workdirs = format!("worktree：{worktree}\n任务目录：{task_dir}");
            let system_prompt = build_system_prompt(
                &load_agents_context(
                    &cfg.project_root,
                    cfg.language.as_deref(),
                    cfg.test_framework.as_deref(),
                ),
                SUB_AGENT_PERSONA,
                &workdirs,
                // 子代理不注入技能：它是检索工，不是技能执行者。注入全文态技能会把
                // 父上下文的问题原样搬进子上下文，与「只回摘要」相悖。
                &[],
            );

            let run_id = cfg
                .store
                .insert_run(&NewRun {
                    task_id: cfg.task_id.clone(),
                    cursor_id: cfg.cursor_id.clone(),
                    stage: cfg.stage,
                    node: cfg.node,
                    attempt: cfg.attempt,
                    agent_type: SUB_AGENT_TYPE.to_string(),
                    parent_run_id: Some(cfg.parent_run_id),
                    prompt_template_hash: None,
                    process_group_id: None,
                })
                .await?;

            // 子代理的只读工具执行器：路径策略与父节点同源（同样锁在 worktree + 任务
            // 目录），但**不注入** `with_recorder` / `with_sse` / `with_sub_agent`——
            // 记录器会以子代理名义记命令（子代理跑不了命令），另两者是父节点专有能力。
            let policy = FileToolPolicy::new(vec![cfg.worktree_path.clone(), cfg.task_dir.clone()]);
            let tools = ToolExecutor::new(
                cfg.home.clone(),
                policy,
                cfg.settings.clone(),
                cfg.killer.clone(),
            );
            let ctx = ToolCallContext {
                task_id: cfg.task_id.clone(),
                stage: cfg.stage,
                node: cfg.node,
                worktree_path: cfg.worktree_path.clone(),
                task_dir: cfg.task_dir.clone(),
                run_id: Some(run_id),
                command_source: CommandSource::Agent,
                default_cwd: Some(cfg.worktree_path.clone()),
            };

            // token 记在 session 里，使**超时**也能落回已发生的用量（决策 100 的口径
            // 不因退出路径不同而漏记）——外层 future 被 drop 时 session 仍在。
            let mut session = SubAgentSession {
                run_id,
                system_prompt,
                tools,
                ctx,
                tokens: std::sync::Mutex::new(RunTokens::default()),
                transcript: Vec::new(),
            };

            let outcome = tokio::time::timeout(
                cfg.max_duration,
                runner.run_loop(&mut session, &request.task),
            )
            .await;

            let tokens_now = *session.tokens.lock().unwrap();
            let (summary, status, error) = match outcome {
                // 外层超时（票 08：节点级 max_duration 就是该次调用的上限）。
                // **不返回 `Err`**：`Err` 会被父代理算作工具失败并累计 `tool_retry_max`，
                // 重试同一个慢检索没有意义——把「超时」当文本交回去，让模型拆小或改道。
                Err(_) => (
                    format!(
                        "子代理超时（{}s）：这次检索没能在时限内完成。\
                         请把子任务拆得更具体，或改用 read_file / list_dir 自己查。",
                        cfg.max_duration.as_secs()
                    ),
                    NodeStatus::Timeout,
                    Some(format!("子代理超时（{}s）", cfg.max_duration.as_secs())),
                ),
                Ok(Err(e)) => {
                    let text = format!("子代理运行失败：{e}");
                    // 即使失败也落会话行与 run 行：失败的子代理「读了什么」同样要可复盘，
                    // 否则一次失败会留下一个无法解释的 token 空洞。
                    finish_run(
                        &cfg.store,
                        run_id,
                        started,
                        NodeStatus::Failed,
                        Some(e.to_string()),
                        tokens_now,
                    )
                    .await?;
                    write_conversation(cfg, run_id, &session.transcript, tokens_now).await?;
                    return Ok(text);
                }
                Ok(Ok(summary)) => (summary, NodeStatus::Success, None),
            };

            finish_run(&cfg.store, run_id, started, status, error, tokens_now).await?;
            write_conversation(cfg, run_id, &session.transcript, tokens_now).await?;
            Ok(summary)
        })
    }
}

/// 一次子代理调用的运行态（run 行 + 循环所需的全部句柄）。
struct SubAgentSession {
    run_id: i64,
    system_prompt: String,
    tools: ToolExecutor,
    ctx: ToolCallContext,
    tokens: std::sync::Mutex<RunTokens>,
    /// 对话累积。放在 session 里（而非循环的局部变量）是为了让**超时也留痕**：
    /// 外层 future 被 drop 后，这段对话仍可写进会话行。
    transcript: Vec<Message>,
}

impl StoreSubAgentRunner {
    /// 子代理的对话循环：LLM 调用 → 只读工具执行 → 累积，直到模型不再发起 tool_call。
    ///
    /// 返回摘要文本。轮数耗尽时返回 `Err`——它会被上层转成给父代理看的文本，而不是
    /// 烧掉 `tool_retry_max`。
    async fn run_loop(&self, session: &mut SubAgentSession, task: &str) -> Result<String> {
        session.transcript.push(Message::user(task.to_string()));
        for _ in 0..SUB_AGENT_MAX_ROUNDS {
            let req = LlmRequest {
                stage: self.cfg.stage,
                node: self.cfg.node,
                attempt: self.cfg.attempt,
                system_prompt: session.system_prompt.clone(),
                // 子任务既在 user_prompt，也在首条 user message 里：前者给只看 user_prompt
                // 的适配器，后者给按 messages 组装的适配器，两条路都不落空。
                user_prompt: task.to_string(),
                messages: session.transcript.clone(),
                tools: Self::tool_defs(),
                temperature: self.cfg.temperature,
                max_tokens: self.cfg.max_tokens,
                provider_id: None,
                run: Some(crate::agent::client::RunContext {
                    task_id: self.cfg.task_id.clone(),
                    branch: self.cfg.branch.clone(),
                    run_id: session.run_id,
                    agent_type: SUB_AGENT_TYPE.to_string(),
                }),
            };
            let response = self.cfg.llm.complete(req).await?;
            session.tokens.lock().unwrap().add(&response);
            // 心跳写父 run（决策 88 同一种做法）：子代理在跑，父节点就没闲着，
            // 不能让父节点因空闲超时被误杀。
            let _ = self
                .cfg
                .store
                .touch_run_heartbeat(self.cfg.parent_run_id)
                .await;
            session.transcript.push(Message::assistant(
                response.content.clone(),
                response.tool_calls.clone(),
            ));

            if response.tool_calls.is_empty() {
                return response
                    .content
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| Error::Validation("子代理返回了空摘要".into()));
            }
            for call in &response.tool_calls {
                match session.tools.execute(call, &session.ctx).await {
                    Ok(outcome) => session
                        .transcript
                        .push(Message::tool_result(call, outcome.content)),
                    // 子代理内的工具失败**不**上升为节点失败（§12.8：单次工具调用失败
                    // 应让模型自行调整），只把错误文本回给子代理继续。
                    Err(e) => session
                        .transcript
                        .push(Message::tool_result(call, format!("工具执行失败：{e}"))),
                }
            }
        }
        Err(Error::Validation(format!(
            "子代理在 {SUB_AGENT_MAX_ROUNDS} 轮内未收口，请把子任务拆得更具体"
        )))
    }
}

/// 收尾子代理 run 行（决策 100：token 记在子代理自己的 run 行上，父 run 不重复累加）。
async fn finish_run(
    store: &Store,
    run_id: i64,
    started: Instant,
    status: NodeStatus,
    error: Option<String>,
    tokens: RunTokens,
) -> Result<()> {
    store
        .finish_run(
            run_id,
            &RunOutcome {
                status: Some(status),
                prompt_tokens: tokens.prompt,
                completion_tokens: tokens.completion,
                cache_read_tokens: tokens.cache_read,
                cache_write_tokens: tokens.cache_write,
                duration_ms: started.elapsed().as_millis() as u64,
                error,
                ..Default::default()
            },
        )
        .await
}

/// 子代理独立会话行（决策 77 / 100）：不与父会话混进同一个 `messages_json`，
/// 但同样带 `agent_type` / `parent_run_id`，使「这次子代理读了什么」可复盘。
async fn write_conversation(
    cfg: &SubAgentRunnerConfig,
    run_id: i64,
    messages: &[Message],
    tokens: RunTokens,
) -> Result<()> {
    let msgs = serde_json::to_value(messages)?;
    cfg.store
        .insert_conversation(
            &cfg.task_id,
            run_id,
            cfg.stage,
            cfg.node,
            cfg.attempt,
            SUB_AGENT_TYPE,
            Some(cfg.parent_run_id),
            &msgs,
            None,
            tokens.prompt,
            tokens.completion,
        )
        .await?;
    cfg.store.refresh_task_totals(&cfg.task_id).await?;
    Ok(())
}
