//! 只读子代理（决策 172③，票 08）。
//!
//! **存在理由**：把「读 20 个文件」的原文挡在父上下文之外，只回摘要。这让
//! `research`、`code-review`（两轴并行）、`codebase-design`（DESIGN-IT-TWICE）
//! 这批以子代理为前提的技能真正跑起来。
//!
//! **安全边界在本模块**（不是工具层）：子代理的工具集**硬编码为只读**——
//! [`SUB_AGENT_TOOLS`] 那三件（`read_file` / `list_dir` / `search_content`）。三条硬约束：
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
//! 的原裁决不受影响——`agent/context.rs` 里**没有**任何 L4 计划结构，pending 由
//! `executor` 在压缩后仍超硬限时直接构造（决策 154① 已把原 `plan_l4` 那组删掉）。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::future::BoxFuture;

use crate::agent::client::{AgentResponse, LlmClient, LlmRequest, Message, ToolDef};
use crate::agent::prompts::{build_system_prompt, load_agents_context};
use crate::agent::tools::{
    SubAgentEnd, SubAgentRequest, SubAgentRunner, ToolCallContext, ToolExecutor,
};
use crate::config::Settings;
use crate::home::Home;
use crate::process::ProcessKiller;
use crate::storage::observability::{NewRun, RunOutcome};
use crate::storage::Store;
use crate::types::{CommandSource, Node, NodeStatus, Stage};
use crate::{Error, Result};

/// 子代理的 run 行 / 会话行的 `agent_type`（决策 172③）。
pub const SUB_AGENT_TYPE: &str = "subagent";

/// 子代理**固定**的只读工具集（决策 172③，票 08；决策 408 加检索）。
///
/// 这是安全边界本身，不是配置项：故意写成常量，任何「按阶段扩权」的改动都必须先改
/// 这里，从而在 diff 里显式可见。名字引用目录表常量（决策 353）——字面量只此一份。
///
/// `search_content`（决策 408）是**补上本职**而非扩权：子代理存在的理由就是「把读
/// 20 个文件的原文挡在父上下文之外」，而它此前连找内容的手都没有——2026-10-08 的实证
/// 里三个子代理全被「扫一遍 frontend/src」这类检索型子任务打满轮数（任务 01M4CD59）。
/// 它只读、走同一份 `file_policy`、有命中行数与字节上限；`deny` 档下**同收**（见
/// [`effective_tools`]）。
pub const SUB_AGENT_TOOLS: [&str; 3] = [
    crate::agent::catalog::READ_FILE,
    crate::agent::catalog::LIST_DIR,
    crate::agent::catalog::SEARCH_CONTENT,
];

/// `search_content` 广告给子代理的那一版说明（决策 408）。
///
/// schema 与值班长**共用一份**（`catalog::SEARCH_CONTENT_PARAMETERS`），广告语各写各的：
/// 值班长那份讲他的域（「在你的文件域里」「data/ 读不到」），子代理这份讲工作区。
const SUB_AGENT_SEARCH_DESCRIPTION: &str = "在任务工作区里**按正则找内容**（报错串、\
     函数名、某句文案落在哪几处）。pattern 是正则、区分大小写；path 可指定工作区内的\
     子目录，缺省整个工作区。不跟符号链接、二进制文件跳过，命中按「路径:行号:行文本」\
     带回并有行数上限。列文件名用 list_dir——找**内容**用它。";

/// 这次调用实际拿到的只读工具集（决策 408）：`deny` 档下 `search_content` **同收**。
///
/// 档位管的是**环境层**（`ENV_TOOLS`），而 `search_content` 是只读层的扩展工具
/// （决策 267④：刻意不进 `ENV_TOOLS`，值班长的「自主轮取证」靠的就是这一条，决策
/// 232 / 237）。可子代理这一侧的档位语义是「环境层收到底」——不额外过滤就会留下一个
/// 洞：**读不了文件，却能把文件内容搜出来**。
///
/// 广告（[`StoreSubAgentRunner::tool_defs`]）与执行点白名单（`with_allowed_tools`）
/// **都从这里出**，一处判定、两处生效。
///
/// 边界如实记：`read_file` / `list_dir` 在 `deny` 档下仍留在这一份里，由档位闸在执行点
/// 拒（它们本来就是 `ENV_TOOLS`），形状与从前一致；而且 `deny` 档下父节点**根本派不出
/// 子代理**（`spawn_sub_agent` 自己也是环境层工具，`deny` 下被拒）——本条过滤是纵深
/// 防御，不是今天可达的主路径。
fn effective_tools(env_mode: crate::types::EnvMode) -> Vec<&'static str> {
    SUB_AGENT_TOOLS
        .into_iter()
        .filter(|name| {
            !(*name == crate::agent::catalog::SEARCH_CONTENT
                && env_mode == crate::types::EnvMode::Deny)
        })
        .collect()
}

/// 子代理的收尾前言：要求它只回摘要（父上下文要的是摘要，不是原文）。
const SUB_AGENT_PERSONA: &str = "你是一个只读检索子代理。你的唯一任务是按父代理给出的描述\
     检索并阅读文件，然后回**精炼摘要**：结论、关键位置（文件:行）、必要的短引文。\
     不要把读到的文件原文整段复制回来——父代理只要摘要，这正是你存在的理由。\
     你没有写权限与命令执行权限，也不需要它们。";

/// 单次子代理调用允许的最大轮数（决策 407：由配置给，缺省 200）。
///
/// 子代理没有 `submit_metadata` 收口，正常靠「模型不再发起 tool_call」自然结束；
/// 这个上限是**防御性**的——模型若陷入「读一个文件 → 再读一个」的循环，必须有人喊停。
/// 它曾是编译期常量 12：2026-10-08 的实证（任务 01M4CD59）里 12 把三个子代理全部
/// 打死在同一句「未收口」上，那一刻起它该是运维面能调的数。
///
/// **0 按 1 兜底**：解析期已拒 `sub_agent_max_rounds = 0`（`Config::validate`），
/// 这里兜的是程序内构造的 `Settings`——「跑 0 轮」与「无上限」一样不是人想要的语义。
fn effective_max_rounds(settings: &Settings) -> usize {
    settings.sub_agent_max_rounds.max(1)
}

/// 子代理**运行期间**的心跳周期（票 08）。
///
/// 子代理的 run 行不参与节点超时判定（见 `scheduler` 的 `is_node_owning_run` 过滤），
/// 但**父 run 参与**。子代理若一次 LLM 调用卡住超过 `node_idle_timeout_sec`（默认
/// 300s），父 run 的 `last_activity_at` 就会变陈旧，父节点被误判超时——正是那道过滤
/// 想避免的后果。所以子代理运行期间必须持续给父 run 打心跳，而不是只在每轮响应后打。
const SUB_AGENT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

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
    /// 父节点所在阶段的环境层档位（决策 206）。子代理是父节点的工具调用，父节点被
    /// `deny` 时不读文件的手也不该从子代理这里伸出去——档位是**阶段**的属性，不是
    /// 某一条调用路径的属性。
    pub env_mode: crate::types::EnvMode,
    /// 本次调用的上限（票 08：沿用节点级 `node_max_duration_sec`）。
    pub max_duration: Duration,
    /// **父节点那一份中止观察点**（决策 409）：与 `model_invoke` 手里的是同一个信号
    /// （`CancelSignal` 是 Arc 三件套，克隆进 cfg 即可）。子代理在三个观察点各看一眼
    /// ——每轮开头、模型调用、工具批之间——被请求中止就提前收口，不再跑满自己的预算。
    ///
    /// 这就是 2026-10-08 那次「按停按不住子代理」的补丁：按停后子代理又跑了 80 秒
    /// （任务 01M4CD59）。`None` 只在没有执行体上下文的构造里出现（测试替身等）。
    pub(crate) cancel: Option<crate::pipeline::executor::CancelSignal>,
}

/// 一次 agent 调用的 token 计量（决策 46：prompt / completion / cache 落 run 行）。
///
/// 父节点与子代理共用这一个类型——两边的口径必须一致（都落 `kanban_node_runs`
/// 的同名列），各写一份迟早会漂移。
#[derive(Debug, Clone, Copy, Default)]
pub struct RunTokens {
    pub prompt: u32,
    pub completion: u32,
    pub cache_read: u32,
    pub cache_write: u32,
}

impl RunTokens {
    pub fn add(&mut self, response: &AgentResponse) {
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
    /// 注意它不经父节点那条 `effective_declared_tools`——那条路会并入基线强制工具
    ///（含 `run_command` / `write_file`），正是本票要挡掉的东西。定义从目录表取
    ///（决策 353 / 408）——子代理同样是「看得见一个调用就被拒的工具」的受害者候选，
    /// 空壳广告一并退场。`search_content` 是唯一的例外分支：它的 schema 与值班长共用
    /// （`catalog::SEARCH_CONTENT_PARAMETERS`），广告语用子代理自己那一版。
    fn tool_defs(env_mode: crate::types::EnvMode) -> Vec<ToolDef> {
        effective_tools(env_mode)
            .into_iter()
            .map(|name| {
                if name == crate::agent::catalog::SEARCH_CONTENT {
                    return ToolDef {
                        name: name.to_string(),
                        description: SUB_AGENT_SEARCH_DESCRIPTION.to_string(),
                        parameters: serde_json::from_str(
                            crate::agent::catalog::SEARCH_CONTENT_PARAMETERS,
                        )
                        .expect("共享的 search_content schema 必须是合法 JSON（目录表单测钉住）"),
                    };
                }
                crate::agent::catalog::def_for(name).unwrap_or_else(|| {
                    panic!("SUB_AGENT_TOOLS 里的 {name} 必须有目录行（agent::catalog）")
                })
            })
            .collect()
    }
}

impl SubAgentRunner for StoreSubAgentRunner {
    fn run(&self, request: SubAgentRequest) -> BoxFuture<'static, Result<SubAgentEnd>> {
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
                )
                .await,
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

            // 子代理的只读工具执行器：路径策略与父节点同源（缺省同样锁在 worktree + 任务
            // 目录；`file_access_unrestricted` 打开时与父节点一起放开，决策 283），
            // 但**不注入** `with_recorder` / `with_sse` / `with_sub_agent`——
            // 记录器会以子代理名义记命令（子代理跑不了命令），另两者是父节点专有能力。
            //
            // `with_allowed_tools` 是安全边界的真正落点：只把 tool 定义少给几个是不够的，
            // 模型无视定义硬发 `run_command` 时必须在**执行点**被拒。
            let mut policy = crate::agent::file_policy::pipeline_file_policy(
                &cfg.worktree_path,
                &cfg.task_dir,
                cfg.settings.file_access_unrestricted,
            );
            // 阶段写入面白名单与父节点同源（决策 395）：子代理复用父节点的
            // `(stage, node)` 坐标。子代理工具集本就只读，这一层是纵深而不是边界。
            policy.allow_writes = super::continuation_brief::stage_write_scope(
                &cfg.worktree_path,
                &cfg.task_dir,
                cfg.stage,
                cfg.node,
            );
            let tools = ToolExecutor::new(
                cfg.home.clone(),
                policy,
                cfg.settings.clone(),
                cfg.killer.clone(),
            )
            // 子代理的只读工具同样受父阶段的档位管：`deny` 档下连只读文件也不给
            //（决策 206 的 deny 是「环境层收到底」）——`search_content` 不在 `ENV_TOOLS`
            // 里，档位管不到它，故由 [`effective_tools`] **显式同收**（决策 408）。
            // **不接提议通道**：与父节点同理——流水线背后没有人盯着，落一条没人会按的
            // 提议等于静默丢弃。
            .with_env_mode(cfg.env_mode)
            .with_allowed_tools(effective_tools(cfg.env_mode));
            let ctx = ToolCallContext {
                task_id: cfg.task_id.clone(),
                session_id: None,
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
                user_prompt: request.task.clone(),
                tools,
                ctx,
                tokens: std::sync::Mutex::new(RunTokens::default()),
                transcript: Vec::new(),
                reasoning: String::new(),
            };

            let outcome = tokio::time::timeout(
                cfg.max_duration,
                runner.run_loop(&mut session, &request.task),
            )
            .await;

            let tokens_now = *session.tokens.lock().unwrap();
            let end = match outcome {
                // 外层超时（票 08：节点级 max_duration 就是该次调用的上限）。
                // **不返回 `Err`**：`Err` 会被父代理算作工具失败并累计 `tool_retry_max`，
                // 重试同一个慢检索没有意义——把「超时」当文本交回去，让模型拆小或改道。
                Err(_) => SubAgentEnd::TimedOut {
                    secs: cfg.max_duration.as_secs(),
                },
                // 真失败（LLM 报错等）：台账记失败，文本同样走「交回父代理」那条路。
                Ok(Err(e)) => SubAgentEnd::Failed {
                    detail: e.to_string(),
                },
                Ok(Ok(end)) => end,
            };

            // 收场与台账**一一对应**（决策 409）：「被中止」记 `cancelled` + 来路，
            // 与主节点同一口径（决策 276）——它是父节点按停的余波，**不是**子代理失败。
            let status = match &end {
                SubAgentEnd::Completed(_) => NodeStatus::Success,
                SubAgentEnd::NotConverged { .. } | SubAgentEnd::Failed { .. } => NodeStatus::Failed,
                SubAgentEnd::TimedOut { .. } => NodeStatus::Timeout,
                SubAgentEnd::Aborted { .. } => NodeStatus::Cancelled,
            };
            let cancel_origin = match &end {
                SubAgentEnd::Aborted { origin, .. } => Some(origin.as_slug()),
                _ => None,
            };
            // 即使失败 / 被中止也落会话行与 run 行：子代理「读了什么」同样要可复盘，
            // 否则一次失败会留下一个无法解释的 token 空洞（票 02 的既有口径）。
            finish_run(
                &cfg.store,
                run_id,
                started,
                status,
                end.ledger_error(),
                cancel_origin,
                tokens_now,
            )
            .await?;
            write_conversation(cfg, &session, tokens_now).await?;
            Ok(end)
        })
    }
}

/// 一次子代理调用的运行态（run 行 + 循环所需的全部句柄）。
struct SubAgentSession {
    run_id: i64,
    system_prompt: String,
    /// 这次子任务的正文（同时是 `user_prompt` 与首条 user message）。
    /// 存下来是为了会话行（票 02）：失败子代理的 prompt 原文同样要可核对。
    user_prompt: String,
    tools: ToolExecutor,
    ctx: ToolCallContext,
    tokens: std::sync::Mutex<RunTokens>,
    /// 对话累积。放在 session 里（而非循环的局部变量）是为了让**超时也留痕**：
    /// 外层 future 被 drop 后，这段对话仍可写进会话行。
    transcript: Vec<Message>,
    /// 全部调用的思考留痕（决策 360）：按到达序以空行相连，随会话行落地、只作展示
    /// （现场时间线的思考步），不进 `transcript`、不回灌——与决策 244 同一条红线。
    reasoning: String,
}

impl StoreSubAgentRunner {
    /// 子代理的对话循环：LLM 调用 → 只读工具执行 → 累积，直到模型不再发起 tool_call。
    ///
    /// 出口是 [`SubAgentEnd`]（决策 409）：正常收口 / 未收口 / 超时 / 被中止全都走
    /// `Ok`——**只有真故障才 `Err`**。这不是风格问题：`Err` 会被父代理算作工具失败并
    /// 累计 `tool_retry_max`，而「未收口」要的是「拆小重派」，「被中止」要的是「别再
    /// 消耗」。上层把 `end.receipt()` 原样回给父代理。
    async fn run_loop(&self, session: &mut SubAgentSession, task: &str) -> Result<SubAgentEnd> {
        session.transcript.push(Message::user(task.to_string()));
        // 运行期间持续给**父 run** 打心跳：单次 LLM 调用可能长过 node_idle_timeout_sec，
        // 只在每轮响应后打会在那段时间留下空窗，父节点被误判超时（票 08）。
        let heartbeat = tokio::spawn(keep_parent_run_alive(
            self.cfg.store.clone(),
            self.cfg.parent_run_id,
        ));
        let result = self.run_rounds(session).await;
        heartbeat.abort();
        result
    }

    /// 轮循环本体。三个中止观察点（决策 409）都在这里——每轮开头、模型调用、工具批之内。
    async fn run_rounds(&self, session: &mut SubAgentSession) -> Result<SubAgentEnd> {
        let max_rounds = effective_max_rounds(&self.cfg.settings);
        let cancel = self.cfg.cancel.as_ref();
        // 轮号 1 起（进 `Aborted.round` / run 行 error）：台账里要读得出它停在哪一轮。
        for round in 1..=max_rounds {
            // 观察点①（决策 409）：每轮开头，与 `model_invoke` 同一种姿势。拦的是
            // 「信号在两轮之间到达」与「已请求中止却又进了一轮」两种情形。
            if let Some(signal) = cancel.filter(|s| s.is_requested()) {
                return Ok(aborted(signal, round));
            }
            let req = LlmRequest {
                stage: self.cfg.stage,
                node: self.cfg.node,
                attempt: self.cfg.attempt,
                system_prompt: session.system_prompt.clone(),
                // 子任务既在 user_prompt，也在首条 user message 里：前者给只看 user_prompt
                // 的适配器，后者给按 messages 组装的适配器，两条路都不落空。
                user_prompt: session.user_prompt.clone(),
                messages: session.transcript.clone(),
                tools: Self::tool_defs(self.cfg.env_mode),
                temperature: self.cfg.temperature,
                max_tokens: self.cfg.max_tokens,
                provider_id: None,
                run: Some(crate::agent::client::RunContext {
                    task_id: self.cfg.task_id.clone(),
                    branch: self.cfg.branch.clone(),
                    run_id: session.run_id,
                    agent_type: SUB_AGENT_TYPE.to_string(),
                    // 子代理不是对讲台的一部分：增量归父节点，与班次无关。
                    session_id: String::new(),
                }),
                idle_timeout_sec: None,
            };
            // 观察点②（决策 409）：模型调用用 `select!` 等信号（决策 226 的姿势）。
            // 子代理一次检索型调用可能长过人的耐心，而它正是最容易无限期停住的地方。
            let response = match cancel {
                Some(signal) => tokio::select! {
                    r = self.cfg.llm.complete(req) => r?,
                    _ = signal.wait() => return Ok(aborted(signal, round)),
                },
                None => self.cfg.llm.complete(req).await?,
            };
            session.tokens.lock().unwrap().add(&response);
            if let Some(text) = response.reasoning.as_deref().filter(|t| !t.is_empty()) {
                if !session.reasoning.is_empty() {
                    session.reasoning.push_str("\n\n");
                }
                session.reasoning.push_str(text);
            }
            // 每轮响应后再打一次：心跳任务本身有周期，这里补一次使活动记录更及时。
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
                return Ok(SubAgentEnd::Completed(
                    response
                        .content
                        .filter(|s| !s.trim().is_empty())
                        .ok_or_else(|| Error::Validation("子代理返回了空摘要".into()))?,
                ));
            }
            for call in &response.tool_calls {
                // 观察点③（决策 409）：批内每个工具调用之间。子代理的工具集**全是只读**
                // （[`SUB_AGENT_TOOLS`]），批内收口不留半写状态——这正是「批内也能打断」
                // 成立的前提，将来若有人往这里加写工具，这条就得先重新论证。
                //
                // 批内看一眼是必要的：2026-10-08 的实证里，一次 `spawn_sub_agent` 之后
                // 子代理又跑了 80 秒（任务 01M4CD59），而那 80 秒全花在「一遍遍读文件、
                // 每轮之间只隔一次模型调用」上。只有轮开头的检查时，「停」要等下一轮
                // 模型调用返回才生效。
                if let Some(signal) = cancel.filter(|s| s.is_requested()) {
                    return Ok(aborted(signal, round));
                }
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
        Ok(SubAgentEnd::NotConverged { max_rounds })
    }
}

/// 中止收口（决策 409）：三个观察点共用这一处——来路从信号上读，绝不猜。
fn aborted(signal: &crate::pipeline::executor::CancelSignal, round: usize) -> SubAgentEnd {
    SubAgentEnd::Aborted {
        origin: signal.origin(),
        round,
    }
}

/// 收尾子代理 run 行（决策 100：token 记在子代理自己的 run 行上，父 run 不重复累加）。
/// 子代理运行期间持续给父 run 打心跳，直到被 abort（票 08）。
///
/// 父 run 参与节点超时判定，而子代理的一次 LLM 调用可能长过 `node_idle_timeout_sec`；
/// 缺了这个心跳，父节点会因为「子代理在跑、父 run 看似无活动」被误判超时。
/// 与 `run_command` 的周期心跳同一种做法（决策 100）。
async fn keep_parent_run_alive(store: Store, parent_run_id: i64) {
    let mut tick = tokio::time::interval(SUB_AGENT_HEARTBEAT_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tick.tick().await; // interval 首次 tick 立即完成，调用方已打过一次，跳过
    loop {
        tick.tick().await;
        if store.touch_run_heartbeat(parent_run_id).await.is_err() {
            // 心跳写失败（库不可用等）不该打死子代理——父节点的超时判定自成一路，
            // 这里静默退出即可，真出问题会在别处显形。
            return;
        }
    }
}

async fn finish_run(
    store: &Store,
    run_id: i64,
    started: Instant,
    status: NodeStatus,
    error: Option<String>,
    // 中止来路（决策 409）：只在 `status = cancelled` 时有值——「人按停」与「判超时」
    // 在这一列上分开记（与主节点同一口径，`observability::CANCEL_ORIGIN_*`）。
    cancel_origin: Option<&'static str>,
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
                cancel_origin,
                ..Default::default()
            },
        )
        .await
}

/// 子代理独立会话行（决策 77 / 100）：不与父会话混进同一个 `messages_json`，
/// 但同样带 `agent_type` / `parent_run_id`，使「这次子代理读了什么」可复盘。
/// 两段 prompt 原文一并落地（票 02）——失败子代理的检索为什么没找到，证据在这里。
async fn write_conversation(
    cfg: &SubAgentRunnerConfig,
    session: &SubAgentSession,
    tokens: RunTokens,
) -> Result<()> {
    let run_id = session.run_id;
    let msgs = serde_json::to_value(&session.transcript)?;
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
            Some(crate::storage::observability::PromptSnapshot {
                system: &session.system_prompt,
                user: &session.user_prompt,
            }),
            None,
            tokens.prompt,
            tokens.completion,
            Some(session.reasoning.as_str()).filter(|r| !r.is_empty()),
        )
        .await?;
    cfg.store.refresh_task_totals(&cfg.task_id).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 决策 407：轮数上限跟着配置走；**0 兜底成 1**（不是「无上限」——那会让一次
    /// 子代理烧到 `max_duration` 才停，而那一刻没人看得懂为什么）。
    ///
    /// 缺省值 200 由 `config.rs::sub_agent_max_rounds_defaults_to_200_and_refuses_zero`
    /// 钉住；这里钉的是**取用点**的读数。
    #[test]
    fn round_cap_follows_the_setting_and_floors_zero_to_one() {
        let mut settings = Settings::default();
        assert_eq!(effective_max_rounds(&settings), 200, "缺省跟着配置走");

        settings.sub_agent_max_rounds = 7;
        assert_eq!(effective_max_rounds(&settings), 7);

        settings.sub_agent_max_rounds = 0;
        assert_eq!(effective_max_rounds(&settings), 1, "0 兜底成 1，绝不无限");
    }

    /// 决策 408：只读集是三件，且 `deny` 档下 `search_content` **同收**——
    /// 不同收会留下「读不了文件、却能把文件内容搜出来」的洞。
    ///
    /// 广告与执行点白名单都从 [`effective_tools`] 出，故这一条同时钉住两处。
    #[test]
    fn deny_mode_takes_the_search_tool_away_from_the_subagent() {
        use crate::types::EnvMode;
        for mode in [EnvMode::Auto, EnvMode::Ask] {
            assert_eq!(
                effective_tools(mode),
                vec!["read_file", "list_dir", "search_content"],
                "{mode:?} 档下三件都在"
            );
        }
        assert_eq!(
            effective_tools(EnvMode::Deny),
            vec!["read_file", "list_dir"],
            "deny 档下检索同收（读不了文件，就不该能把内容搜出来）"
        );
    }
}
