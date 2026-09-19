//! 工具执行层（§10.2，决策 45 / 104 / 118 / 110）。
//!
//! **工具层全部真实执行**（决策 148）：write_file 真写、run_command 真跑、FileToolPolicy
//! 真拦、输出脱敏真过、L2 卸载真落盘、命令真记 `kanban_node_commands`。FakeAgent 只替换
//! LLM 响应流，因此集成测试顺带覆盖整个工具子系统。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use futures::future::BoxFuture;

use super::client::ToolCall;
use super::context::{
    count_tokens, needs_offload, offload_replacement, trim_list_dir, trim_read_file,
    trim_run_command,
};
use super::file_policy::FileToolPolicy;
use super::sanitize::sanitize_text;
use crate::config::{effective_run_command_timeout, Settings};
use crate::home::Home;
use crate::process::ProcessKiller;
use crate::storage::tasks::TaskFilter;
use crate::storage::Store;
use crate::types::{CommandSource, Node, NodeRun, NodeStatus, Stage};
use crate::{Error, Result};

use super::egress::NetworkPolicy;

/// 值班长读回执时的消息裁剪（决策 182⑭）：只留最后 N 条，且总量压在字符上限内。
///
/// 两次裁剪的必要性不同：条数上限挡住「一次几百轮的会话」，字符上限挡住「一条消息
/// 本身就有几万字」（`run_command` 的完整输出会整段进 messages）。只做前者会在一条
/// 巨长的消息上失效，只做后者会让一万条短消息挤满预算。
fn trim_conversation_messages(messages: &serde_json::Value) -> Vec<serde_json::Value> {
    use crate::pipeline::foreman::{
        FOREMAN_CONVERSATION_MAX_CHARS, FOREMAN_CONVERSATION_MAX_MESSAGES,
    };

    let Some(all) = messages.as_array() else {
        return Vec::new();
    };
    let tail = if all.len() > FOREMAN_CONVERSATION_MAX_MESSAGES {
        &all[all.len() - FOREMAN_CONVERSATION_MAX_MESSAGES..]
    } else {
        &all[..]
    };
    let mut kept: Vec<serde_json::Value> = Vec::new();
    let mut used = 0usize;
    for msg in tail.iter().rev() {
        let rendered = msg.get("content").and_then(|c| c.as_str()).unwrap_or("");
        let cost = rendered.chars().count();
        if !kept.is_empty() && used + cost > FOREMAN_CONVERSATION_MAX_CHARS {
            break;
        }
        used += cost;
        kept.push(msg.clone());
    }
    kept.reverse();
    kept
}

/// **环境层**工具（决策 206）：权限档位管的就是这一层。
///
/// 为什么是一层而不是一个个打补丁：这一层的共同点是「效果落在文件与机器里」——错了可以
/// 回滚、有日志可查，故它**可配**（`auto` 直接执行 / `ask` 转提议 / `deny` 连广告都不给）。
/// 与之相对的是**本服务写接口**（[`SERVICE_WRITE_TOOLS`]）：那一边的错误会改变流水线的
/// 事实（有依赖边、有 worktree 准入、有合入门），故它**恒为提议 + 确认钮**、不读档位。
///
/// 名分说明：决策 206 的原文用的是 `FOREMAN_ENV_TOOLS`。这里改用不带前缀的名字，因为
/// 这道闸对**每一个**执行器都生效——流水线阶段也有档位（全局默认 `auto` = 与今天逐字
/// 相同），若只在值班长那一侧判，就成了执行点上的一个 per-caller 特例，而「判断落在一处」
/// 正是这一层存在的理由。
///
/// `delete_file` **在列**：决策 206 的清单里没写它（那是按「文件读写 + 命令」举例的），
/// 但它与 `write_file` 是同一件事的两种形态——漏掉它，`deny` 就成了一堵带门的墙。
pub const ENV_TOOLS: [&str; 9] = [
    "read_file",
    "write_file",
    "edit_file",
    "delete_file",
    "list_dir",
    "run_command",
    // 修复轮（决策 210③④，票 10–12）：它动的是**环境**——在项目仓上拉一个 worktree、
    // 跑闸门、落一个带标记的 commit。放在这一层而不是 D 层，是**授权形状**决定的：
    // 档位就是这个特性的开关（`ask` 下每一步要人按键，`auto` 下整轮自己跑完，而
    // 「合入」永远人按）。放进 D 层它会恒为提议，而 `finish` 的产物**本身就是**一条提议
    // ——那会变成两层按不完的钮。
    "repair",
    "Skill",
    "spawn_sub_agent",
];

/// **本服务写接口**工具（决策 206）：恒为提议 + 确认钮，**不读档位**。
///
/// 三个名字**一族一个工具 + 动作参数**（决策 207④）：粒度对着 `allowed_actions` 的类型走，
/// 理由有三——白名单短、确认钮的前端渲染不用按端点分叉、persona 描述 token 成本低。
/// 代价是模型可能选错动作，而确认钮正是为拦这个而存在（后端按 `(端点位, 参数)` 重走一遍
/// 既有校验）。
///
/// **故意缺席的三项**（决策 207⑤）：重置配对令牌、局域网开关、仓名单增删。判据是
/// 「改的是**谁能访问这台机器**」——让模型能提议它们等于让它能给自己开门。本清单里没有
/// 它们对应的名字，而清单与实现**同源**（白名单从工具清单生成、执行点按名字分派），
/// 故「能提议一个开门的动作」这件事在代码里没有落点。
pub const SERVICE_WRITE_TOOLS: [&str; 4] = ["task", "config", "skills", "service"];

/// 环境层里**会改动东西**的那些（决策 206 的 C / E 两层）：`ask` 档下转成提议。
///
/// 与 [`ENV_TOOLS`] 分成两段是必要的：`deny` 收的是**整层**（读也不给），而 `ask` 收的
/// 只是**动手**那一半。「读一个文件也要人按键」不是在收紧权限，是在把确认钮变成噪声
/// ——而噪声会让人开始无脑按，那时它挡不住真正该挡的那一次。
pub const ENV_WRITE_TOOLS: [&str; 5] = [
    "write_file",
    "edit_file",
    "delete_file",
    "run_command",
    // `repair` 三件（`start` / `finish` / `discard`）都会改动东西：建分支与 worktree、
    // 落一个 commit、删掉 worktree。**整个族都算动手**，不看具体动作——「只读的
    // `discard`」这种细分只会让人以为其中某个动作是安全的。
    "repair",
];

/// 这个工具属于环境层吗（档位管它）。
pub fn is_env_tool(name: &str) -> bool {
    ENV_TOOLS.contains(&name)
}

/// 这个环境层工具在 `ask` 档下要人按键吗（= 它会改动东西）。
pub fn is_env_write_tool(name: &str) -> bool {
    ENV_WRITE_TOOLS.contains(&name)
}

/// 这个工具是本服务的写接口吗（恒提议，不看档位）。
pub fn is_service_write_tool(name: &str) -> bool {
    SERVICE_WRITE_TOOLS.contains(&name)
}

/// `deny` 档把哪些工具挡在**广告**之外（决策 206）：整层环境工具。
///
/// 判据只有这一处：工具清单（值班长那一侧的 `foreman_available_tools`）与阶段节点的
/// `tool_defs` 都问它，两处各写一份谓词的后果是「一侧摘掉了、另一侧还广告着」这种只能靠
/// 现象定位的漂移。`deny` 收的是**整层**（读也不给），与 `ask` 只收动手那一半不是一回事。
pub fn denied_by_tier(name: &str, env_mode: crate::types::EnvMode) -> bool {
    env_mode == crate::types::EnvMode::Deny && is_env_tool(name)
}

/// 一次调用的第三道闸怎么判（决策 206 / 207）。**这是判据的唯一实现**——
/// [`ToolExecutor::execute`] 按它分派，[`needs_confirmation`] 与测试按它读数。
///
/// 分成三态而不是一个布尔，是因为「要按键」回答不了「然后呢」：拦下来之后是**生成提议**
/// 还是**拒绝**，取决于有没有提议通道（流水线节点没有），而那件事只有执行器自己知道。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateDecision {
    /// 照常执行（`auto` 档；或只读的东西——「只读的不需要人按键」是决策 188 的原话）。
    Execute,
    /// 拦下来生成提议（决策 188 / 207 的表）。
    Propose,
    /// 拒绝，且这一层整个不可用（`deny` 档）。
    Refuse,
}

/// 这个工具此刻会走哪一条（判据见 [`GateDecision`]）。
pub fn gate_decision(name: &str, env_mode: crate::types::EnvMode) -> GateDecision {
    // D 层：**不读档位**（决策 206）。它的错误会改变流水线的事实，故这一半永远是提议。
    if is_service_write_tool(name) {
        return GateDecision::Propose;
    }
    if !is_env_tool(name) {
        // 台账只读工具（`read_task` 那一批）**不在**这两层里，故它们永远直接执行。
        return GateDecision::Execute;
    }
    match env_mode {
        crate::types::EnvMode::Auto => GateDecision::Execute,
        // `ask` 只收**动手**那一半：只读的环境层工具（`read_file` / `list_dir` / `Skill` /
        // 子代理）照常直接执行——「读一个文件也要人按键」是把确认钮变成噪声。
        crate::types::EnvMode::Ask if is_env_write_tool(name) => GateDecision::Propose,
        crate::types::EnvMode::Ask => GateDecision::Execute,
        crate::types::EnvMode::Deny => GateDecision::Refuse,
    }
}

/// D 层恒提议的**唯一例外**的形状判据（决策 210② / 票 08）：
/// `task` + `resume` + `resume_action = continue`。
///
/// **只有形状，不含托管状态**：形状是纯函数（可以单独测、可以在两处复用），
/// 状态由调用方从库里读（`Store::get_task` → `task.stewardship`）。形状与状态分开的
/// 后果是「界面说托管中、执行点却仍在提议」这种不一致变得不可能——两边问的是同一个函数。
///
/// 为什么恰好这一个动作（决策 210② 的清单，逐条有理由）：
/// - `retry` 会 `git reset --hard` + `git clean -fdx`，会洗掉工作区；
/// - `merge` / `review` 写回主干或替人拍板；
/// - `cancel` / `create` 一个丢掉工作、一个花钱；
/// - `skip` / `goto` 在 pending 上直接改流转目标，等于替人重排流水线。
pub fn is_stewardable_resume(name: &str, args: &serde_json::Value) -> bool {
    name == "task"
        && args.get("action").and_then(|v| v.as_str()) == Some("resume")
        && args.get("resume_action").and_then(|v| v.as_str()) == Some("continue")
}

/// 托管可自动集里**还有** `unstick`（决策 210⑧ / 票 09）。
///
/// 它与 `resume` 是两回事，但进同一个集合的理由相同：只影响一个任务、可逆、且它是
/// `resume` 能生效的**前提**（去重摘不掉时 resume 是空操作）。
///
/// 「重启服务」**不在**这里——它是全局动作，永远只提议（决策 210⑧ 的硬规矩）。
pub fn is_stewardable_unstick(name: &str, args: &serde_json::Value) -> bool {
    crate::pipeline::unstick::is_unstick_action(name, args)
}

/// 这一次调用在整个托管自动集里吗（形状判据的**唯一入口**）。
pub fn is_stewardable_action(name: &str, args: &serde_json::Value) -> bool {
    is_stewardable_resume(name, args) || is_stewardable_unstick(name, args)
}

/// 托管放行的自动动作怎么**执行**（决策 210② / 票 08）。
///
/// 为什么执行者由注入决定、而不是在 core 里实现：resume 的唯一实现在
/// [`crate::pipeline::resume::apply_resume`]，它的调用方（`POST /tasks/{id}/resume` 与
/// 托管动作）都必须走那一份。core 不知道 HTTP 那一层，注入进来的正是「谁来跑那一段」。
///
/// 不注入 = 不放行：D 层照旧恒提议（与「不注入不放行」的既有姿态一致）。
pub trait StewardActionRunner: Send + Sync + 'static {
    fn run(
        &self,
        call: crate::agent::client::ToolCall,
        ctx: ToolCallContext,
    ) -> BoxFuture<'static, Result<ToolOutcome>>;
}

/// 这个工具受**确认钮**管吗（决策 206 / 207）：[`gate_decision`] 的那一态，取名给人读。
pub fn needs_confirmation(name: &str, env_mode: crate::types::EnvMode) -> bool {
    gate_decision(name, env_mode) == GateDecision::Propose
}

/// 命令日志记录的启动信息（§12.4.4）。
#[derive(Debug, Clone)]
pub struct CommandStart {
    /// 归属任务。值班长的命令给 `None`，改为挂 [`Self::session_id`]（决策 204④）。
    /// 空串由存储层归一成 `None`——空串不是归属，它是「没有」的伪装。
    pub task_id: Option<String>,
    /// 归属会话（值班长的命令）。流水线命令为 `None`。
    pub session_id: Option<String>,
    pub run_id: Option<i64>,
    pub stage: Stage,
    pub node: Node,
    pub source: CommandSource,
    pub command: String,
    pub cwd: String,
}

/// 命令日志记录的收尾信息。
#[derive(Debug, Clone, Default)]
pub struct CommandFinish {
    pub exit_code: Option<i32>,
    pub stdout_path: Option<String>,
    pub stdout_preview: Option<String>,
    pub stderr_preview: Option<String>,
    pub duration_ms: u64,
}

/// 命令记录器接缝（存储层实现；测试用记录器）。
pub trait CommandRecorder: Send + Sync + 'static {
    fn record_start(&self, start: CommandStart) -> BoxFuture<'static, Result<i64>>;
    fn record_finish(
        &self,
        command_id: i64,
        finish: CommandFinish,
    ) -> BoxFuture<'static, Result<()>>;
    /// 刷新所属 run 的 `last_activity_at`（决策 100：长命令不得被空闲超时误杀）。
    fn touch_heartbeat(&self, run_id: Option<i64>) -> BoxFuture<'static, Result<()>>;

    /// 回填 run 的真实进程组 id（决策 66 / 票 17）：scheduler 超时时据此杀整个进程组。
    /// 默认空实现——不关心 pgid 的记录器（含测试替身）无需改。
    fn set_process_group(&self, _run_id: i64, _pgid: i32) -> BoxFuture<'static, Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

/// 一条提议的请求（决策 188 / 207，票 02 / 04 / 05 / 06）。
///
/// 工具层只说「这次调用要变成一条提议」；**说明与态势指纹由接缝的实现去算**
/// （[`crate::pipeline::proposals::StoreProposalSink`]）——从参数到人读的那句话是呈现层的
/// 知识，工具层不该有第二份。
pub struct ProposalRequest {
    /// 归属会话。值班长的提议挂在它的会话上；流水线节点没有会话，则落 `None`
    /// （那种情况下 `ask` 档仍然可用，提议只是不挂在任何班次下）。
    pub session_id: Option<String>,
    /// 归属任务（流水线节点；值班长为空）。执行时按它取态势指纹。
    pub task_id: Option<String>,
    pub tool: String,
    pub args: serde_json::Value,
}

/// 提议写入接缝（决策 188 / 207）。
///
/// 与 [`SubAgentRunner`] / [`CommandRecorder`] 同一种做法：工具层声明「我需要一个能落提议
/// 的东西」，真正的落库与广播住在 pipeline 层（它才持有 store 与 SSE）。
///
/// **不注入即不可用**：`ask` 档下没有接缝时，写工具调用被**拒**（不是放行）——
/// 「接不上确认钮就先别动」是这条接缝唯一安全的默认。
pub trait ProposalSink: Send + Sync + 'static {
    /// 落一条提议，返回**回灌进对话**的一句话（让模型知道「已经提议、等人按键」）。
    fn propose(&self, request: ProposalRequest) -> BoxFuture<'static, Result<String>>;
}

/// 子代理调用的入参（决策 172③，票 08）。
///
/// 只带**每次调用不同**的东西：子任务文本。任务 / 分支 / attempt / 父 run / 只读工具集
/// 这些「同一次节点执行内固定」的信息由 [`SubAgentRunner`] 的实现持有——它是按
/// attempt 构造的，天然知道自己在哪个节点、哪一次执行、属于哪个父 run 里。
pub struct SubAgentRequest {
    /// 父代理给出的子任务描述。
    pub task: String,
}

/// 子代理执行接缝（决策 172③，票 08）。
///
/// 与 [`CommandRecorder`] 同一种做法：工具层只声明「我需要一个能跑子代理的东西」，
/// 真正的 agent 循环住在 pipeline 层（它才持有 LLM 接缝）。**工具层不认识 LLM**，
/// 所以子代理不能从工具层自己长出来。
///
/// 没有注入实现时 `spawn_sub_agent` 不可用——这正是「默认关闭」的落点：能力由阶段
/// 声明与 executor 接线共同决定，而不是由工具层假装支持。
pub trait SubAgentRunner: Send + Sync + 'static {
    fn run(&self, request: SubAgentRequest) -> BoxFuture<'static, Result<String>>;
}

/// 工具调用上下文。
#[derive(Debug, Clone)]
pub struct ToolCallContext {
    pub task_id: String,
    /// 归属会话（值班长的命令挂会话，决策 204④）。流水线节点为 `None`——
    /// 它不是「另一个称呼的 task_id」，是另一条归属。
    pub session_id: Option<String>,
    pub stage: Stage,
    pub node: Node,
    pub worktree_path: PathBuf,
    pub task_dir: PathBuf,
    pub run_id: Option<i64>,
    /// 命令来源：agent 的 `run_command` 为 [`CommandSource::Agent`]。
    pub command_source: CommandSource,
    /// `run_command` 的默认真实 cwd（卫生默认值，**不是安全边界**）。
    pub default_cwd: Option<PathBuf>,
}

impl ToolCallContext {
    /// 该阶段产出写入哪个根（§6 / §8）。
    ///
    /// 设计 / 评审文档写任务目录；代码写 worktree；test 阶段的集成代码写 worktree，
    /// 但 `test-report.md` 是任务目录的固定产出（pipeline-spec §6：test 写集成代码到
    /// worktree、test-report.md到任务目录）。
    pub fn write_root_for(&self, relative: &str) -> &Path {
        match self.stage {
            Stage::ArchitectDesign | Stage::DevelopDesign | Stage::TestDesign | Stage::Review => {
                &self.task_dir
            }
            Stage::Test
                if Path::new(relative).file_name()
                    == Some(std::ffi::OsStr::new("test-report.md")) =>
            {
                &self.task_dir
            }
            _ => &self.worktree_path,
        }
    }

    /// 读路径解析顺序：先 worktree，再任务目录。
    pub fn read_candidates(&self, relative: &str) -> Vec<PathBuf> {
        vec![
            self.worktree_path.join(relative),
            self.task_dir.join(relative),
        ]
    }
}

/// 工具执行结果。
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutcome {
    /// 回填进 messages 的文本（**已脱敏**，决策 118）。
    pub content: String,
    /// `submit_metadata` 提交的结构化元数据。
    pub metadata: Option<serde_json::Value>,
}

impl ToolOutcome {
    /// 成功回执。`pub` 是给**托管动作的执行者**用的（决策 210② / 票 08）：它在 app 层
    /// 实现，需要构造与工具回执同形的返回值。
    pub fn ok(content: impl Into<String>) -> Self {
        ToolOutcome {
            content: content.into(),
            metadata: None,
        }
    }
}

/// 工具执行器。
pub struct ToolExecutor {
    home: Home,
    policy: FileToolPolicy,
    settings: Settings,
    recorder: Option<Arc<dyn CommandRecorder>>,
    killer: Arc<dyn ProcessKiller>,
    /// `run_command` 运行期间的心跳周期（决策 100）；默认 5s，测试可调短。
    command_heartbeat_interval: Duration,
    /// 流式输出去向（决策 100 / §12.4.4，票 14）：`run_command` 按行推送命令输出。
    /// `None` = 不推流（纯单测 / 无订阅者场景），行为与既有缓冲一致。
    sse: Option<CommandSse>,
    /// 子代理执行器（决策 172③，票 08）。`None` = `spawn_sub_agent` 不可用
    /// ——默认关闭即由此表达，而非让工具层假装支持。
    sub_agent: Option<Arc<dyn SubAgentRunner>>,
    /// **强制**的工具白名单（决策 172③，票 08）。`None` = 无限制（父节点的常态）。
    ///
    /// 这是子代理只读边界的真正落点。只限制「广告出去的 tool 定义」是不够的：
    /// [`Self::execute`] 按 `call.name` 路由，模型完全可以无视 tool 定义直接发一个
    /// `run_command`，那样它就真被跑掉了。故边界必须落在**执行点**。
    ///
    /// 类型是 `Vec<&'static str>` 而不是 `&'static [&'static str]`：值班长的白名单要能
    /// **由工具清单生成**（票 01 的「广告集与白名单同源」），而 const 数组做不到按层过滤。
    /// 名字本身仍是 `'static`，故「白名单是硬编码的，不是运行时可配的」这条性质不变。
    allow: Option<Vec<&'static str>>,
    /// `run_command` 的出口策略（决策 179，票 12）。
    ///
    /// 构造时从 [`Settings`] 取一次（见 [`Self::new`]），执行点不再读配置——策略与「这次执行
    /// 用的哪份设置」不会错位。默认姿态保守：空清单 + 不放行全部，只放行回环。
    egress: NetworkPolicy,
    /// 台账读句柄（决策 182⑭，票 02）。`None` = `read_task` / `read_conversation` 不可用。
    ///
    /// 这两个工具**只面向值班长**：它的输入是人可以随便打的任意文本，故它的能力必须来自
    /// 一个显式注入的只读句柄，而不是继承流水线节点那套（含文件与命令）的上下文
    /// ——「不注入即不可用」让「它到底能碰什么」在构造点就看得见。
    ledger: Option<Store>,
    /// 环境层档位（决策 206）。缺省 [`EnvMode::Auto`] = 与档位出现之前逐字相同。
    ///
    /// 把它放在执行器上而不是每个工具里：判断落在**一处**（[`needs_confirmation`] 的两段
    /// 清单 + 这个档位），加一个环境层工具不需要碰分叉逻辑。
    env_mode: crate::types::EnvMode,
    /// 提议写入接缝（决策 188 / 207）。`None` = 需要确认的动作**被拒**（不注入不放行）。
    proposals: Option<Arc<dyn ProposalSink>>,
    /// **人已经按过键了**（决策 207）：关掉第三道闸。
    ///
    /// 只给「执行提议」那一条路用。提议生成时已经走过一次闸（`ask` 档下那次调用正是被拦
    /// 下来变成了提议），执行时再拦一次就会自己吃掉自己——按钮按下去又生成一条新提议。
    /// 这不是绕过闸门：按下的那个动作就是被拦下来的那一次调用，参数逐字取自提议行，
    /// 而白名单（[`Self::allow`]）照旧按**当前**档位判——档位在提议之后被收紧到 `deny`
    /// 时，这条提议按不下去。
    confirmed: bool,
    /// 托管放行的自动动作的执行者（决策 210② / 票 08）。`None` = 不放行（D 层恒提议）。
    steward_actions: Option<Arc<dyn StewardActionRunner>>,
}

/// 命令输出推流的上下文（票 14）：SSE 去向 + 事件里要带的任务/分支。
#[derive(Clone)]
pub struct CommandSse {
    pub sink: Arc<dyn crate::sse::SseSink>,
    pub task_id: String,
    pub branch: String,
}

/// 单行推流上限（票 14 的节流策略之一）：超长行截断并标注，避免一行撑爆事件。
pub const STREAM_MAX_LINE_CHARS: usize = 4_000;
/// 单条命令最多推送的行数（票 14 的节流策略之二）：高频输出超过后停止推流并标注，
/// **完整输出仍全量缓冲**用于命令记录与回填——推流是观测面，不是数据来源。
pub const STREAM_MAX_LINES: usize = 2_000;

/// 逐行收集的命令输出（票 14）：完整缓冲 + 推流计数。
#[derive(Debug, Clone, Default)]
struct CollectedOutput {
    stdout: String,
    stderr: String,
    /// 已推送的行数（用于节流；两条流合计）。
    streamed_lines: usize,
}

/// 心跳默认周期：远小于 300s 空闲超时，600s 级测试命令也能存活（决策 100）。
pub const COMMAND_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

impl ToolExecutor {
    pub fn new(
        home: Home,
        policy: FileToolPolicy,
        settings: Settings,
        killer: Arc<dyn ProcessKiller>,
    ) -> Self {
        ToolExecutor {
            home,
            policy,
            egress: NetworkPolicy::from_settings(&settings),
            settings,
            recorder: None,
            killer,
            command_heartbeat_interval: COMMAND_HEARTBEAT_INTERVAL,
            sse: None,
            sub_agent: None,
            allow: None,
            ledger: None,
            env_mode: crate::types::EnvMode::Auto,
            proposals: None,
            confirmed: false,
            steward_actions: None,
        }
    }

    /// 设定环境层档位（决策 206）。生产路径由调用方按阶段配置解析后传入
    /// （[`crate::types::effective_env_mode`] 是唯一的解析实现）。
    pub fn with_env_mode(mut self, mode: crate::types::EnvMode) -> Self {
        self.env_mode = mode;
        self
    }

    /// **人已经按过键了**：这次调用是提议被确认之后的执行，不再走第三道闸。
    ///
    /// 只给「按下确认钮」那一条路用（[`crate::pipeline::foreman`] 的说明里有理由）。
    /// 白名单不受影响——档位在提议之后收紧到 `deny` 时，那条提议照样按不下去。
    pub fn confirmed_once(mut self) -> Self {
        self.confirmed = true;
        self
    }

    /// 注入托管动作的执行者（决策 210② / 票 08、票 09）。见 [`StewardActionRunner`]。
    /// 不注入 = 不放行：没有执行者时 D 层照旧恒提议。
    pub fn with_steward_actions(mut self, runner: Arc<dyn StewardActionRunner>) -> Self {
        self.steward_actions = Some(runner);
        self
    }

    /// 注入提议写入接缝（决策 188 / 207）。不注入时 `ask` 档与 D 层写工具**一律被拒**。
    pub fn with_proposal_sink(mut self, sink: Arc<dyn ProposalSink>) -> Self {
        self.proposals = Some(sink);
        self
    }

    pub fn with_recorder(mut self, recorder: Arc<dyn CommandRecorder>) -> Self {
        self.recorder = Some(recorder);
        self
    }

    /// 注入子代理执行器（决策 172③，票 08）。不注入即 `spawn_sub_agent` 不可用。
    pub fn with_sub_agent(mut self, runner: Arc<dyn SubAgentRunner>) -> Self {
        self.sub_agent = Some(runner);
        self
    }

    /// 把工具集**收窄**为给定的白名单（决策 172③，票 08）。
    ///
    /// 与「只少给几个 tool 定义」不同：越界的调用在 [`Self::execute`] 处被拒，模型
    /// 就算硬发也执行不了。子代理的只读边界靠它成立。
    pub fn with_allowed_tools(mut self, allow: Vec<&'static str>) -> Self {
        self.allow = Some(allow);
        self
    }

    /// 注入台账读句柄（决策 182⑭，票 02）：使 `read_task` / `read_conversation` 可用。
    ///
    /// 与 [`Self::with_sub_agent`] 同一种做法——能力由接线决定，不注入即不可用。
    /// 注入的是 [`Store`] 本身而不是一层新 trait：台账是既有的存储实现，
    /// 为它再造一个可替换接缝只会多一个「测试里跑的不是真 SQL」的口子，
    /// 而这两个工具要验的恰恰是「读得到真台账」。
    pub fn with_ledger(mut self, store: Store) -> Self {
        self.ledger = Some(store);
        self
    }

    /// 注入命令输出流式去向（决策 100 / 票 14）：长命令按行推 `command_output`。
    pub fn with_sse(mut self, sse: CommandSse) -> Self {
        self.sse = Some(sse);
        self
    }

    /// 测试用：调短 `run_command` 的周期心跳。
    pub fn with_command_heartbeat_interval(mut self, interval: Duration) -> Self {
        self.command_heartbeat_interval = interval;
        self
    }

    /// 覆盖 `run_command` 的出口策略（决策 179，票 12）。
    ///
    /// 生产路径由 [`Self::new`] 从 [`Settings`] 直接取，没有这一步；它存在是为了让用例
    /// 能单独钉住策略（尤其是「同一份设置下放行 / 拒绝两种走向」），而不必绕道配置。
    pub fn with_egress(mut self, egress: NetworkPolicy) -> Self {
        self.egress = egress;
        self
    }

    pub fn egress(&self) -> &NetworkPolicy {
        &self.egress
    }

    pub fn policy(&self) -> &FileToolPolicy {
        &self.policy
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// 执行一次工具调用。
    ///
    /// 白名单（[`Self::with_allowed_tools`]）在**这里**生效——先于任何分发。只靠
    /// tool 定义约束是纸糊的：模型可以无视定义直接发 `run_command`。
    pub async fn execute(&self, call: &ToolCall, ctx: &ToolCallContext) -> Result<ToolOutcome> {
        if let Some(allow) = &self.allow {
            if !allow.contains(&call.name.as_str()) {
                return Err(Error::Validation(format!(
                    "工具 {} 不在本次调用的允许集内（只读子代理仅允许：{}）",
                    call.name,
                    allow.join(" / ")
                )));
            }
        }
        // 第三道闸（决策 206 / 207）：`(工具属于哪一层, 当前档位)` 决定直接执行 / 生成提议 /
        // 拒绝。它排在白名单**之后**：一个连广告都没给的 deny 档工具，先被白名单挡下时
        // 报的是「不在允许集内」，那是更准确的一句话。
        if let Some(proposed) = self.confirm_gate(call, ctx).await? {
            return Ok(proposed);
        }
        let outcome = match call.name.as_str() {
            "write_file" => self.write_file(call, ctx).await?,
            "edit_file" => self.edit_file(call, ctx).await?,
            "read_file" => self.read_file(call, ctx).await?,
            "delete_file" => self.delete_file(call, ctx).await?,
            "list_dir" => self.list_dir(call, ctx).await?,
            "run_command" => self.run_command(call, ctx).await?,
            // 修复轮（决策 210③④ / 票 10–12）：start 给一个可写的 worktree，finish 跑闸门
            // → commit → 落提议，discard 回收。三件事的**序列**都在 `pipeline::repair` 里。
            "repair" => self.repair(call, ctx).await?,
            "submit_metadata" => self.submit_metadata(call)?,
            "Skill" => self.skill(call)?,
            "spawn_sub_agent" => self.spawn_sub_agent(call, ctx).await?,
            // 台账只读工具（决策 182⑭，票 02）。它们在白名单里的位置与其余工具相同：
            // 越权调用在函数开头的白名单检查处就被拒，这里不再重复判定「谁可以调」。
            "read_task" => self.read_task(call).await?,
            "read_conversation" => self.read_conversation(call).await?,
            // 诊断包（决策 211③，票 03）：一次调用给出定因所需的全部证据。
            "read_diagnosis" => self.read_diagnosis(call).await?,
            // A 层环境读数（决策 188 / 207，票 01）：全部只读，全部走后端既有口径。
            "read_board" => self.read_board().await?,
            "read_metrics" => self.read_metrics().await?,
            "read_projects" => self.read_projects().await?,
            "read_stage_configs" => self.read_stage_configs().await?,
            "read_skills" => self.read_skills().await?,
            "read_providers" => self.read_providers().await?,
            other => return Err(Error::Validation(format!("未知工具：{other}"))),
        };
        self.apply_l2_offload(call, ctx, outcome)
    }

    /// 需要确认钮的动作（决策 206 / 207）：`ask` 档下的环境层、以及**任何档位下**的
    /// 本服务写接口。
    ///
    /// 三种结局各自说得清：
    /// - **生成提议**（`Ok(Some(…))`）：回给模型一句话，说清「这件事已经提了、等人按键」。
    ///   它**不是**工具失败——模型该继续把话说完，而不是改道去试别的写法。
    /// - **拒绝**（`Err`）：`deny` 档，或 `ask` 档但没人接上提议通道（不注入不放行）。
    /// - **不该管**：`Ok(None)`，照常往下走。
    async fn confirm_gate(
        &self,
        call: &ToolCall,
        ctx: &ToolCallContext,
    ) -> Result<Option<ToolOutcome>> {
        // 人已经按过键了（决策 207）：不再问第二遍。
        if self.confirmed {
            return Ok(None);
        }
        match gate_decision(call.name.as_str(), self.env_mode) {
            // 与档位出现之前逐字相同：直接执行、落命令日志、走既有的一切。
            GateDecision::Execute => Ok(None),
            // D 层恒提议的**唯一例外**（决策 210② / 票 08）：托管任务上的 `resume(continue)`。
            // 三道闸缺一不可——形状对（恰一个动作）、托管中且未触顶、执行者已注入。
            GateDecision::Propose
                if self.steward_actions.is_some()
                    && is_stewardable_action(&call.name, &Self::args(call)?)
                    && self.steward_grant(call).await?.is_some() =>
            {
                self.run_steward_action(call, ctx).await.map(Some)
            }
            GateDecision::Propose => self.propose(call, ctx).await.map(Some),
            GateDecision::Refuse => Err(Error::Validation(format!(
                "工具 {} 被 deny 档挡下（决策 206）：这个阶段的环境层权限已收到底，\
                 文件、命令与技能拉取都不执行",
                call.name
            ))),
        }
    }

    /// 这一次调用拿得到托管授权吗（决策 210② / 票 08）。
    ///
    /// 三条闸**一起**判，且判据只有这一处：
    /// 1. 形状：`task` + `resume` + `continue`（[`is_stewardable_resume`]）；
    /// 2. 任务托管中（`Stewardship::enabled`）；
    /// 3. 未触顶且指纹不同（[`crate::types::Stewardship::permits`]，决策 210⑨ 的两条止损）。
    ///
    /// 返回 `Some(指纹)` 时调用方直接执行；`None` 时照常生成提议——**触顶与同指纹不是
    /// 报错**，是「这一次交给人」（任务仍停在 pending，按钮照旧给得出来）。
    async fn steward_grant(&self, call: &ToolCall) -> Result<Option<String>> {
        let Some(store) = &self.ledger else {
            return Ok(None);
        };
        let args = Self::args(call)?;
        let Some(task_id) = args.get("task_id").and_then(|v| v.as_str()) else {
            return Ok(None);
        };
        let task = match store.get_task(task_id).await {
            Ok(t) => t,
            // 查无此任务：照常走提议（模型可能记错一个 id，那不是托管该管的事）
            Err(_) => return Ok(None),
        };
        let Some(stewardship) = task.stewardship.as_ref() else {
            return Ok(None);
        };
        let fingerprint = crate::pipeline::foreman::situation_fingerprint(store, task_id)
            .await?
            .to_string();
        Ok(stewardship.permits(&fingerprint).then_some(fingerprint))
    }

    /// 执行一次托管放行的自动动作，并**当场留账**（决策 210② 的硬要求）。
    ///
    /// 账分两处，各自回答不同的问题：
    /// - 会话里一条 `system` 行（操作台记的）——「它什么时候、对哪个任务、第几次动的手」，
    ///   人第二天早上在时间线上读得到；
    /// - 任务行上的 `auto_resumes` / `last_fingerprint`——止损线要落库，重启后仍算数。
    async fn run_steward_action(
        &self,
        call: &ToolCall,
        ctx: &ToolCallContext,
    ) -> Result<ToolOutcome> {
        let runner = self
            .steward_actions
            .as_ref()
            .ok_or_else(|| Error::Validation("托管动作没有执行者（不注入不放行）".into()))?;
        let args = Self::args(call)?;
        let task_id = args
            .get("task_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        // 授权在**执行前**再取一次：上一步的检查与这一步之间没有任何 await 之外的东西，
        // 但账要记在动手之后（记「动了几次」而不是「打算动几次」）。
        let grant = self.steward_grant(call).await?;
        let outcome = runner.run(call.clone(), ctx.clone()).await?;
        if let (Some(store), Some(fingerprint)) = (&self.ledger, grant) {
            if let Ok(task) = store.get_task(&task_id).await {
                // 记账用的是**实际做的那个动作**（`resume(continue)` / `unstick`）：
                // 这一行是值班经理第二天早上唯一能读到的「它自己动过几次手」，写错动作名
                // 等于把一次 `unstick` 说成一次 resume。次数止损线两种动作**共用**一条
                // （决策 210⑨ 的 N=2）——这是更保守的那一侧：同一条线不必记两遍，
                // 而托管放开的范围本来就该窄到可审计。
                let action = match args.get("action").and_then(|v| v.as_str()) {
                    Some("unstick") => "unstick".to_string(),
                    _ => format!(
                        "resume({})",
                        args.get("resume_action")
                            .and_then(|v| v.as_str())
                            .unwrap_or("continue")
                    ),
                };
                let mut stewardship = task.stewardship.clone().unwrap_or_default();
                stewardship.note_auto_resume(&fingerprint, store.now());
                let nth = stewardship.auto_resumes;
                store.set_stewardship(&task_id, Some(&stewardship)).await?;
                if let Some(session_id) = ctx.session_id.as_deref() {
                    let content = format!(
                        "【托管】自动 {action}：任务 {task_id}（第 {nth}/{} 次自动动作）。\
                         依据指纹 {fingerprint}。超出次数或指纹相同即停手，等你按键。",
                        crate::types::STEWARDSHIP_MAX_AUTO_RESUMES
                    );
                    if let Err(e) = store
                        .append_foreman_message(crate::storage::NewForemanMessage::system(
                            session_id, content,
                        ))
                        .await
                    {
                        tracing::error!(task = %task_id, "托管动作的留痕写不进去：{e}");
                    }
                }
            }
        }
        Ok(outcome)
    }

    /// 把一次调用落成提议（决策 188 / 207），返回回灌进对话的那句话。
    async fn propose(&self, call: &ToolCall, ctx: &ToolCallContext) -> Result<ToolOutcome> {
        let args = Self::args(call)?;
        let Some(sink) = &self.proposals else {
            // 不注入不放行：接上确认钮之前，需要人按键的动作只能被拒。
            return Err(Error::Validation(format!(
                "工具 {} 需要值班经理按键确认，但本次运行没有接上提议通道（决策 188）\
                 ——动作没有执行，也没有留下提议",
                call.name
            )));
        };
        let note = sink
            .propose(ProposalRequest {
                session_id: ctx.session_id.clone(),
                task_id: (!ctx.task_id.is_empty()).then(|| ctx.task_id.clone()),
                tool: call.name.clone(),
                args,
            })
            .await?;
        Ok(ToolOutcome::ok(note))
    }

    /// L2 大结果卸载**覆盖全部工具**（决策 110 / 票 04）：任何工具结果超过
    /// `offload_threshold_tokens` 一律落盘、context 只留预览 + 路径。
    ///
    /// `run_command` 在自身路径里已按 stdout/stderr 语义卸载（保留退出码与失败行），
    /// 此处跳过避免二次卸载；`submit_metadata` 是极小 JSON，无需处理。
    ///
    /// 值班长的两个台账工具同样跳过：卸载要写 `home.context_dir(&ctx.task_id)`，
    /// 而值班长**没有 task_id**（空串会落到上下文根目录，污染下一个真实任务的文件）。
    /// 它们的结果在工具内部已按字符上限截断，不会无界增长。
    fn apply_l2_offload(
        &self,
        call: &ToolCall,
        ctx: &ToolCallContext,
        outcome: ToolOutcome,
    ) -> Result<ToolOutcome> {
        if matches!(
            call.name.as_str(),
            "run_command"
                | "submit_metadata"
                | "read_task"
                | "read_conversation"
                | "read_diagnosis"
        ) {
            return Ok(outcome);
        }
        if !needs_offload(&outcome.content, &self.settings) {
            return Ok(outcome);
        }
        let dir = self.offload_dir(ctx)?;
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.txt", ulid::Ulid::new()));
        std::fs::write(&path, &outcome.content)?;
        let tokens = count_tokens(&outcome.content);
        let preview = head_tail(&outcome.content, 30, 30);
        Ok(ToolOutcome {
            content: offload_replacement(&call.name, &path.display().to_string(), tokens, &preview),
            metadata: outcome.metadata,
        })
    }

    /// L2 卸载落在哪个目录——**按归属选维度**（决策 204④ / 206）。
    ///
    /// 空 `task_id` 不是「根目录下的 `.context`」，是「没有任务」。写进去的后果是污染
    /// 下一个真实任务的工作区（`{root}/tasks/.context` 是**所有任务共用**的那一层），
    /// 所以这里宁可报错也不退化成那个路径。值班长的卸载落会话维度，在 `tasks/` 之外。
    fn offload_dir(&self, ctx: &ToolCallContext) -> Result<std::path::PathBuf> {
        if !ctx.task_id.is_empty() {
            return Ok(self.home.context_dir(&ctx.task_id));
        }
        let session = ctx
            .session_id
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                Error::Validation(
                    "这次调用既没有任务也没有会话归属，工具输出无处卸载（不写 tasks/.context）"
                        .into(),
                )
            })?;
        Ok(self.home.foreman_context_dir(session))
    }

    fn args(call: &ToolCall) -> Result<serde_json::Value> {
        serde_json::from_str(&call.arguments)
            .map_err(|e| Error::Validation(format!("工具 {} 参数解析失败：{e}", call.name)))
    }

    async fn write_file(&self, call: &ToolCall, ctx: &ToolCallContext) -> Result<ToolOutcome> {
        let args = Self::args(call)?;
        let rel = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::Validation("write_file 缺少 path".into()))?;
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::Validation("write_file 缺少 content".into()))?;

        let target = ctx.write_root_for(rel).join(rel);
        let resolved = self.policy.check_write(&target)?;
        if let Some(parent) = resolved.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // 先清后写保证幂等（G9）
        std::fs::write(&resolved, content)?;
        Ok(ToolOutcome::ok(format!(
            "{{\"success\":true,\"path\":\"{}\",\"bytes\":{}}}",
            rel,
            content.len()
        )))
    }

    async fn edit_file(&self, call: &ToolCall, ctx: &ToolCallContext) -> Result<ToolOutcome> {
        let args = Self::args(call)?;
        let rel = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let old_text = args.get("old_text").and_then(|v| v.as_str()).unwrap_or("");
        let new_text = args.get("new_text").and_then(|v| v.as_str()).unwrap_or("");

        let target = ctx.write_root_for(rel).join(rel);
        let resolved = self.policy.check_write(&target)?;
        // 空 `old_text` 是一个**看起来成功**的错误改动：`contains("")` 恒真，`replacen("", …)`
        // 把 new_text 插在文件开头——回执说 success，文件却被改坏了。缺参数时必须拒，
        // 不能靠 `unwrap_or("")` 把它变成一个合法的空串。
        if old_text.is_empty() {
            return Err(Error::Validation(format!(
                "edit_file 缺少 old_text（{rel}）——空串会变成「往开头插一段」，不是一次替换"
            )));
        }
        let original = std::fs::read_to_string(&resolved)?;
        if !original.contains(old_text) {
            return Err(Error::Validation(format!(
                "edit_file 未找到待替换文本（{rel}）"
            )));
        }
        // 幂等：只替换一次
        let updated = original.replacen(old_text, new_text, 1);
        std::fs::write(&resolved, updated)?;
        Ok(ToolOutcome::ok(format!(
            "{{\"success\":true,\"path\":\"{rel}\"}}"
        )))
    }

    /// 派生一个**只读**子代理（决策 172③，票 08）。
    ///
    /// 父代理给出子任务描述，子代理在独立 context 里跑完并把**摘要**带回父对话——
    /// 「读 20 个文件」的原文因此不会进父上下文。工具集由 pipeline 层的运行器固定为
    /// `read_file` / `list_dir`，**不继承阶段声明的工具**（阶段配置无法给子代理扩权）。
    ///
    /// 未注入运行器时返回**错误文本而非 `Err`**：与 `Skill` 工具同一姿态（票 06）——
    /// `Err` 会被算作工具失败并累计 `tool_retry_max`，模型因此打挂整个节点；返回文本
    /// 让模型自行改道。
    async fn spawn_sub_agent(&self, call: &ToolCall, ctx: &ToolCallContext) -> Result<ToolOutcome> {
        let Some(runner) = &self.sub_agent else {
            return Ok(ToolOutcome::ok(
                "spawn_sub_agent 在当前阶段未启用。请直接完成该子任务，\
                 或在阶段配置的 tools_json 里声明 spawn_sub_agent。",
            ));
        };
        let args = Self::args(call)?;
        let task = args.get("task").and_then(|v| v.as_str()).unwrap_or("");
        if task.trim().is_empty() {
            return Ok(ToolOutcome::ok(
                "spawn_sub_agent 需要 {task} 参数（子任务描述）。请补充后重试。",
            ));
        }
        // 父 run 由运行器自己持有（它按 attempt 构造）。这里仍要求 ctx 带 run_id：
        // 缺它说明调用不在节点执行的上下文里，那种情况不该派生（会落无父的孤儿 run）。
        if ctx.run_id.is_none() {
            return Ok(ToolOutcome::ok(
                "spawn_sub_agent 需要所属 run 上下文（当前调用没有 run_id），无法派生。",
            ));
        }
        let summary = runner
            .run(SubAgentRequest {
                task: task.to_string(),
            })
            .await?;
        Ok(ToolOutcome::ok(summary))
    }

    async fn read_file(&self, call: &ToolCall, ctx: &ToolCallContext) -> Result<ToolOutcome> {
        let args = Self::args(call)?;
        let rel = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);
        // 决策 226：要**尾部**而不是头部。追加写的文件（日志、运行记录）要看的都是尾巴，
        // 而默认的头部读法在这种文件上给的恰好是最没用的那一段。
        let tail = args.get("tail").and_then(|v| v.as_bool()).unwrap_or(false);

        let mut found: Option<PathBuf> = None;
        for candidate in ctx.read_candidates(rel) {
            if candidate.exists() {
                let resolved = self.policy.check_read(&candidate)?;
                found = Some(resolved);
                break;
            }
        }
        let path = found.ok_or_else(|| Error::Validation(format!("文件不存在：{rel}")))?;
        // 决策 226：读取**不再整份进内存**（此前是 `read_to_string`，一个 200MB 的日志会
        // 整份读进来再交给 L1 裁剪）。小文件仍走原路（`trim_read_file` 的结构大纲要往后扫
        // 全文），大文件只读需要的那一段——`tail` 直接读尾部，日志与运行记录都该这么读。
        let (sliced, note) = read_text_bounded(&path, offset, limit, tail)?;
        // L1 裁剪：默认头部 200 行 + 结构大纲
        let mut out = trim_read_file(&sliced, limit);
        if let Some(note) = note {
            out.push_str(&format!("\n[{note}]"));
        }
        Ok(ToolOutcome::ok(out))
    }

    async fn delete_file(&self, call: &ToolCall, ctx: &ToolCallContext) -> Result<ToolOutcome> {
        let args = Self::args(call)?;
        let rel = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let target = ctx.write_root_for(rel).join(rel);
        let resolved = self.policy.check_write(&target)?;
        // 不存在视为成功（幂等，G9）
        if resolved.exists() {
            std::fs::remove_file(&resolved)?;
        }
        Ok(ToolOutcome::ok(format!(
            "{{\"success\":true,\"path\":\"{rel}\"}}"
        )))
    }

    async fn list_dir(&self, call: &ToolCall, ctx: &ToolCallContext) -> Result<ToolOutcome> {
        let args = Self::args(call)?;
        let rel = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let recursive = args
            .get("recursive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let root = if Path::new(rel).is_absolute() {
            PathBuf::from(rel)
        } else {
            ctx.worktree_path.join(rel)
        };
        let resolved = self.policy.check_read(&root)?;
        let mut entries = Vec::new();
        collect_entries(&resolved, recursive, &mut entries)?;
        entries.sort();
        Ok(ToolOutcome::ok(trim_list_dir(&entries)))
    }

    fn submit_metadata(&self, call: &ToolCall) -> Result<ToolOutcome> {
        let value: serde_json::Value = serde_json::from_str(&call.arguments)
            .map_err(|e| Error::Validation(format!("submit_metadata 参数解析失败：{e}")))?;
        Ok(ToolOutcome {
            content: "{\"success\":true}".to_string(),
            metadata: Some(value),
        })
    }

    /// `Skill` 工具（决策 172③，票 06）：按名取技能正文，作为 **tool result** 进 `messages`。
    ///
    /// **不走 `Err` 通道**——未知技能名返回一段说明文本而非 `Error`。理由在 agent loop 的
    /// 分层里：`Err` 会被算作工具失败并累计到 `tool_retry_max`（决策 33），模型写错一个技能名
    /// 就可能把整个节点打挂；而票 06 明确要求这种情况**让模型自行纠正**。返回文本既进上下文
    /// 又不触发失败计数，模型下一轮换个名字即可。
    ///
    /// 读的是**技能根**（loader 侧），不经 [`FileToolPolicy`]——技能根与 `{home}/data/`
    /// （provider 密钥明文存储，决策 112）同父，放宽为 agent 可读等于交出密钥。
    fn skill(&self, call: &ToolCall) -> Result<ToolOutcome> {
        let args: serde_json::Value = serde_json::from_str(&call.arguments)
            .map_err(|e| Error::Validation(format!("Skill 参数解析失败：{e}")))?;
        let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
        if name.trim().is_empty() {
            return Ok(ToolOutcome::ok(
                "Skill 工具需要一个 name 参数（技能名）。请用技能目录里列出的名字重试。",
            ));
        }
        match crate::agent::skills::load_body(&self.home.skills_dir(), name) {
            Ok(body) => Ok(ToolOutcome::ok(body)),
            Err(e) => Ok(ToolOutcome::ok(format!(
                "无法加载技能 {name}：{e}。\
                 请从技能目录里选一个名字重试；若该技能尚未安装，请先安装再调用。"
            ))),
        }
    }

    /// `read_task`（决策 182⑭，票 02）：读某个任务的台账详情。
    ///
    /// **不存在时不走 `Err` 通道**，与 [`Self::skill`] 同一理由：模型写错一个任务 id 是
    /// 最常见的失败，`Err` 会被算作工具失败并累计 `tool_retry_max`（决策 33），
    /// 一次笔误就能把整次回话打挂。返回一段说明文本既进上下文又不触发失败计数。
    async fn read_task(&self, call: &ToolCall) -> Result<ToolOutcome> {
        let store = self.ledger_or_err()?;
        let args = Self::args(call)?;
        let task_id = args
            .get("task_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if task_id.is_empty() {
            return Ok(ToolOutcome::ok(
                "read_task 需要一个 task_id 参数。任务 id 是态势快照里方括号内那串。",
            ));
        }
        // 「任务不存在」在这里是**正常回答**而不是故障：模型可能记错一个 id。
        // 只吞 `Error::Task`（那是「查无此任务」），其余错误照常上升——把库故障
        // 也说成「没有这个任务」会让模型继续拿错 id 反复猜。
        let task = match store.get_task(&task_id).await {
            Ok(t) => t,
            Err(Error::Task(_)) => {
                return Ok(ToolOutcome::ok(format!(
                    "台账里没有任务 {task_id}。请用态势快照里列出的 id 重试。"
                )))
            }
            Err(e) => return Err(e),
        };
        // allowed_actions 由后端权威下发（决策 101）——这里把它**原样**交出去，
        // 不做筛选也不做解释。值班长能替值班经理描述「可按下哪些键」，
        // 但它自己按不动（写动作仍需人来发）。
        let cursors = store.load_live_cursors(&task_id).await?;
        let actions = task
            .pending_reason
            .as_ref()
            .map(|r| crate::actions::allowed_actions(r, None))
            .unwrap_or_default();
        let value = serde_json::json!({
            "task_id": task.id,
            "title": task.title,
            "description": task.description,
            "status": task.status.as_str(),
            "current_stage": task.current_stage.as_str(),
            "current_node": task.current_node.as_str(),
            "pending_reason": task.pending_reason,
            "allowed_actions": actions,
            "cursors": cursors.iter().map(|c| serde_json::json!({
                "branch": c.branch,
                "stage": c.stage,
                "node": c.node,
                "status": c.status.as_str(),
            })).collect::<Vec<_>>(),
            "total_tokens": task.total_tokens,
            "total_calls": task.total_calls,
            "stalled": task.stalled,
            "updated_at": task.updated_at.to_rfc3339(),
        });
        // `description` 与 `allowed_actions` 都可能很长，而这个结果**不走** L2 卸载
        // （见 `apply_l2_offload`），所以上限必须在这里落。
        Ok(ToolOutcome::ok(
            crate::pipeline::foreman::truncate_tool_result(&serde_json::to_string_pretty(&value)?),
        ))
    }

    /// 台账读句柄。六个 A 层读数与两个台账工具共用它（票 01）。
    ///
    /// 不注入即不可用——值班长的能力必须来自一个显式注入的只读句柄，
    /// 而不是继承流水线节点那套（含文件与命令）的上下文。
    fn ledger_or_err(&self) -> Result<&Store> {
        self.ledger
            .as_ref()
            .ok_or_else(|| Error::Validation("这个工具不可用：本次调用没有注入台账读句柄".into()))
    }

    /// 台账文本的截断（票 03）：**与 `truncate_messages_json` 同一套纪律**——留标记，
    /// 不许静默截短。借用存储层那份实现，免得这里再长出一套「截到多少算多少」。
    fn clip(text: &str, max_chars: usize) -> String {
        crate::storage::observability::truncate_text(text, max_chars)
    }

    /// `read_diagnosis`（决策 211③，票 03）：一次调用拿到**定因**所需的全部证据。
    ///
    /// **为什么不扩 `read_task`**：那是高频、便宜的看状态（每一轮值守都会调），诊断包低频，
    /// 一击就撞 12k 上限。混在一起会让「看一眼任务状态」开始烧 12k 字符。
    ///
    /// 输出是**分节数组**而不是一个大对象，因为顺序在这里是有语义的：`serde_json` 默认按
    /// key 排序（没有 `preserve_order`），而 12k 截断是从尾部切的——被切掉的必须是长尾
    /// （命令台账、阶段产出），不是「为什么卡住」那一屏。数组保序，是这个语义的载体。
    async fn read_diagnosis(&self, call: &ToolCall) -> Result<ToolOutcome> {
        let store = self.ledger_or_err()?;
        let args = Self::args(call)?;
        let task_id = args
            .get("task_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if task_id.is_empty() {
            return Ok(ToolOutcome::ok(
                "read_diagnosis 需要一个 task_id 参数。任务 id 是态势快照里方括号内那串。",
            ));
        }
        // 「查无此任务」是正常回答，不是故障——理由与 [`Self::read_task`] 逐字相同。
        let task = match store.get_task(&task_id).await {
            Ok(t) => t,
            Err(Error::Task(_)) => {
                return Ok(ToolOutcome::ok(format!(
                    "台账里没有任务 {task_id}。请用态势快照里列出的 id 重试。"
                )))
            }
            Err(e) => return Err(e),
        };
        let runs_limit = args
            .get("runs")
            .and_then(|v| v.as_u64())
            .unwrap_or(30)
            .clamp(1, 200) as usize;

        let runs = store.list_runs(&task_id).await?;
        let commands = store.list_commands(&task_id, None, None).await?;
        let outputs = store.list_stage_outputs(&task_id).await?;

        // 最近一次**非成功**的 run：诊断的重点。全成功时退到最近一次 run——
        // 「它到底跑到哪了」在那种情况下才是问题。
        let latest_failure = runs
            .iter()
            .rev()
            .find(|r| matches!(r.status, NodeStatus::Failed | NodeStatus::Timeout))
            .or_else(|| runs.last());
        let pending = task.pending_reason.as_ref();

        // ① 为什么卡住：待办原因原文 + 最近那次失败。**必须排在最前**。
        let why = serde_json::json!({
            "why_stalled": {
                "task_id": task.id,
                "title": task.title,
                "status": task.status.as_str(),
                "current": format!("{}.{}", task.current_stage.as_str(), task.current_node.as_str()),
                "stalled": task.stalled,
                "pending": pending.map(|p| serde_json::json!({
                    "kind": p.kind.as_str(),
                    "at": format!("{}.{}", p.stage.as_str(), p.node.as_str()),
                    "message": p.message,
                    "diagnostic": p.context.as_ref().and_then(|c| c.diagnostic.clone()),
                    "suggested_actions": p.suggested_actions,
                })),
                "latest_failure": latest_failure.map(run_digest),
            }
        });

        // ② 失败那一轮的现场：组装后的两段 prompt 原文（票 02）+ 最后几次工具往来。
        //    这两件加 `runs` 里的 error，才够回答「是代码问题 / prompt 问题 / 环境问题」。
        let mut sections = vec![why];
        if let Some(run) = latest_failure {
            if let Some(conv) = store.get_conversation(&task_id, run.id).await? {
                sections.push(serde_json::json!({
                    "failed_run_context": {
                        "run_id": run.id,
                        "prompt_template_hash": run.prompt_template_hash,
                        "system_prompt": conv.system_prompt.as_deref().map(|s| Self::clip(s, PROMPT_SNAPSHOT_MAX_CHARS)),
                        "user_prompt": conv.user_prompt.as_deref().map(|s| Self::clip(s, PROMPT_SNAPSHOT_MAX_CHARS)),
                        "last_messages": tail_messages(&conv.messages_json, 4),
                    }
                }));
            }
        }

        // ③ 全部 run（最近的在前，限条数）：耗时 / token / error / 进程组。
        let run_rows: Vec<serde_json::Value> =
            runs.iter().rev().take(runs_limit).map(run_digest).collect();
        sections.push(serde_json::json!({
            "runs": run_rows,
            "runs_total": runs.len(),
        }));

        // ④ 模型请求台账（决策 231）：**归位**与**量速**的共同地基，排在命令台账之前
        //    ——「卡在哪一次调用上」比「跑过哪些命令」更靠近病因。
        //
        //    两半各答一件事：`inflight` 是**此刻**在飞的请求（`finished_at IS NULL`），
        //    它回答 run 行回答不了的问题（一次 run 里有多次请求，工具循环每轮一次）；
        //    `recent` 逐条给出 run_id + 序号 + 起止 + 用量 + 收字节总量 + 最后一次收字节的
        //    时刻，于是「这条栈属于哪一个 run」与「流是被压慢了还是 prompt 本来就大」
        //    都有可复核的落点。**不派生 bytes/s**：目录里没有对那个比值的判据，
        //    给一个没人判得了的数只会多一个误导的读数（决策 235 拒「置信度」同一条道理）。
        let inflight = store
            .inflight_model_requests(MODEL_REQUEST_INFLIGHT_LIMIT)
            .await?;
        let recent = store
            .model_requests_for_task(&task.id, MODEL_REQUEST_LIMIT)
            .await?;
        let now = store.now();
        sections.push(serde_json::json!({
            "model_requests": {
                "inflight": inflight
                    .iter()
                    .map(|r| model_request_digest(r, now))
                    .collect::<Vec<_>>(),
                "recent": recent
                    .iter()
                    .map(|r| model_request_digest(r, now))
                    .collect::<Vec<_>>(),
                "recent_limit": MODEL_REQUEST_LIMIT,
            },
        }));

        // ④ 命令台账：闸门命令与 agent 自己跑的都在这里（`stdout_path` 是全文的落点）。
        sections.push(serde_json::json!({
            "commands": commands.iter().map(|c| serde_json::json!({
                "id": c.id,
                "run_id": c.run_id,
                "stage": c.stage.as_str(),
                "node": c.node.as_str(),
                "source": c.source.as_str(),
                "command": c.command,
                "cwd": c.cwd,
                "exit_code": c.exit_code,
                "duration_ms": c.duration_ms,
                "stdout_path": c.stdout_path,
                "stdout_preview": c.stdout_preview.as_deref().map(|s| Self::clip(s, 1500)),
                "stderr_preview": c.stderr_preview.as_deref().map(|s| Self::clip(s, 1500)),
            })).collect::<Vec<_>>(),
            "commands_total": commands.len(),
        }));

        // ⑤ 闸门输出：路径 + 尾部。路径是**确定**的（按 stage 命名，重跑覆盖同一文件），
        //    所以即便尾部被截，路径也足以让人 / 后续轮次取全文。
        sections.push(serde_json::json!({
            "gate_outputs": gate_outputs(&self.home, &task.id),
        }));

        // ⑥ 阶段产出与验收标准：architect 的 acceptance_criteria 在这里。
        sections.push(serde_json::json!({
            "stage_outputs": outputs.iter().map(|o| serde_json::json!({
                "stage": o.stage.as_str(),
                "output_type": o.output_type,
                "file_path": o.file_path,
                "stale": o.stale,
                "metadata": o.metadata_json,
            })).collect::<Vec<_>>(),
        }));

        let text = serde_json::to_string_pretty(
            &serde_json::json!({ "task_id": task.id, "evidence": sections }),
        )?;
        Ok(ToolOutcome::ok(
            crate::pipeline::foreman::truncate_tool_result(&text),
        ))
    }

    /// 一个读数 → 交出去的文本。
    ///
    /// 统一收口三件事：不美化（`to_string_pretty` 便于模型读）、**按字符上限截断**
    /// （这些结果不走 L2 卸载——卸载要写 `home.context_dir(&ctx.task_id)`，而值班长没有
    /// task_id）、失败不 panic。
    fn readout(value: serde_json::Value) -> ToolOutcome {
        let text = match serde_json::to_string_pretty(&value) {
            Ok(t) => t,
            Err(e) => format!("读数序列化失败：{e}"),
        };
        ToolOutcome::ok(crate::pipeline::foreman::truncate_tool_result(&text))
    }

    /// `read_board`（票 01）：整块看板——每个任务的状态与当前工位 + 按状态的分组计数。
    ///
    /// 口径**与看板端点同源**（同一张任务表、同一组状态字符串），不新造一套读数。
    /// 与态势快照的分工：快照只装「需要有人管的」（待拍板 / 在跑 / 失败），
    /// 这里是全量——问「一共多少活」时要看得到已完成与排队。
    async fn read_board(&self) -> Result<ToolOutcome> {
        let store = self.ledger_or_err()?;
        let tasks = store
            .list_tasks(&TaskFilter {
                include_archived: false,
                ..Default::default()
            })
            .await?;
        let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        for t in &tasks {
            *counts.entry(t.status.as_str()).or_default() += 1;
        }
        // 按状态分组列出（组内按 id，保证同一份数据两次调用给出同一个顺序）
        let mut by_status: std::collections::BTreeMap<&str, Vec<serde_json::Value>> =
            std::collections::BTreeMap::new();
        for t in &tasks {
            by_status
                .entry(t.status.as_str())
                .or_default()
                .push(serde_json::json!({
                    "task_id": t.id,
                    "title": t.title,
                    "stage": t.current_stage.as_str(),
                    "node": t.current_node.as_str(),
                    "updated_at": t.updated_at.to_rfc3339(),
                }));
        }
        Ok(Self::readout(serde_json::json!({
            "counts": counts,
            "tasks_by_status": by_status,
        })))
    }

    /// `repair`（决策 210③④ / 票 10–12）：**修复轮的三步**。
    ///
    /// 它补的是这条链此前缺的那一格：`start_repair` / `run_repair_gate` / `commit_repair` /
    /// `propose_repair` 全都实现好了，却没有任何生产调用者——值班长拿不到 worktree，也就没有
    /// 合法的落点去写补丁（项目工作区在它的文件域之外，那是票 10 的硬约束）。
    ///
    /// 三步的**顺序是用法的一部分**，故这条动作的回复把下一步说清：
    /// 1. `start`：建 worktree/branch，回一个**它写得进去**的绝对路径；
    /// 2. （写代码：`write_file` / `edit_file` / `run_command`，域就是既有的家目录那一条）；
    /// 3. `finish`：闸门 → 过了才 commit → diff → 落一条提议（**不设 TTL**，等人按合入）；
    ///    设置 `task_id` 时顺带在那条任务上留下「等修复合入」的字样。
    ///
    /// `discard` 是**另一条路**（不是第三步）：不打算继续了就回收——**保留分支**，
    /// 它是唯一的证据。
    async fn repair(&self, call: &ToolCall, ctx: &ToolCallContext) -> Result<ToolOutcome> {
        use crate::pipeline::repair;

        let store = self.ledger_or_err()?.clone();
        let args = Self::args(call)?;
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
        let project_id = args
            .get("project_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if project_id.is_empty() {
            return Ok(ToolOutcome::ok(
                "repair 需要一个 project_id（用 read_projects 看有哪些项目）。",
            ));
        }
        // 「查无此项目」「这个项目不是 git 仓」都是**正常回答**而不是工具故障：与
        // `read_task` 的「查无此任务」同一姿态（决策 33 的 tool_retry_max 不该被笔误吃掉）。
        let Some(project) = store.get_project(&project_id).await? else {
            return Ok(ToolOutcome::ok(format!(
                "台账里没有项目 {project_id}。用 read_projects 看已接入的清单。"
            )));
        };
        // 归属只可能是**那一班**：`repair` 不在 `BUILTIN_TOOLS` 里，故阶段配置声明不出它
        // （`PUT /stage-configs` 与启动校验会当场拒），节点也就永远看不到这个名字——
        // 这条动作只有值班长够得到，而它每次调用都带班次（迁移 0012 的严格 XOR 由此成立）。
        let session_id = ctx.session_id.clone().unwrap_or_default();

        match action {
            "start" => {
                if let Err(e) = repair::repair_supported(&project) {
                    return Ok(ToolOutcome::ok(format!("这个项目没法修：{e}")));
                }
                let repair_id = repair::new_repair_id();
                let session = repair::start_repair(
                    &self.home,
                    std::path::Path::new(&project.local_path),
                    &project.default_branch,
                    &repair_id,
                    &session_id,
                )
                .await?;
                Ok(ToolOutcome::ok(format!(
                    "修复 worktree 已就绪。\n\
                     repair_id：{repair_id}\n\
                     可写目录：{}\n\
                     分支：{}（从 {} 分出，未进主干）\n\
                     下一步：在**那个目录里**改代码（write_file / edit_file / run_command 的路径\
                     都写在它下面），改完调用 repair(action=finish, project_id={project_id}, \
                     repair_id={repair_id}, conclusion=…一句话诊断结论)。\
                     改动在你按下「合入」之前不会进主干，所以**不要说「我已经修好了」**。",
                    session.worktree.display(),
                    session.branch,
                    session.base_ref
                )))
            }
            "finish" => {
                let Some(repair_id) = args.get("repair_id").and_then(|v| v.as_str()) else {
                    return Ok(ToolOutcome::ok(
                        "repair(action=finish) 需要一个 repair_id——start 的回执里有它。",
                    ));
                };
                let conclusion = args
                    .get("conclusion")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if conclusion.is_empty() {
                    return Ok(ToolOutcome::ok(
                        "repair(action=finish) 需要一句 conclusion：这次改了什么、为什么\
                         （它会进 commit message，三个月后靠它认账）。",
                    ));
                }
                let session = repair::repair_session_for(
                    &self.home,
                    std::path::Path::new(&project.local_path),
                    &project.default_branch,
                    repair_id,
                    &session_id,
                )
                .await?;
                if !session.worktree.exists() {
                    return Ok(ToolOutcome::ok(format!(
                        "修复 worktree 不在（{}）。它可能已经被回收——重新 start 一次，\
                         或者用 read_projects 确认这个项目还是那个项目。",
                        session.worktree.display()
                    )));
                }
                match repair::finish_repair_round(
                    &store,
                    &self.home,
                    &project,
                    &session,
                    &conclusion,
                    args.get("task_id").and_then(|v| v.as_str()),
                    store.now(),
                )
                .await?
                {
                    repair::RepairRound::GateFailed { gate, note } => Ok(ToolOutcome::ok(format!(
                        "闸门没过，**没有出 diff、也没有提提议**：{note}\n\
                         读数：{}。改完再调一次 finish。",
                        gate.iter()
                            .map(|r| format!("{} 退出码 {}", r.kind, r.exit_code))
                            .collect::<Vec<_>>()
                            .join("、")
                    ))),
                    repair::RepairRound::Proposed { summary, .. } => {
                        // 「等修复合入」的两处留痕在 `finish_repair_round` 里（那里才有完整的
                        // 序列），这里只负责把话说明白。
                        Ok(ToolOutcome::ok(format!(
                            "闸门已过，改动已单独成 commit（带 `{}` 标记）并落成一条**待你按合入**\
                             的提议：{summary}\n\
                             diff 全文：{}/worktrees/repair-{repair_id}.diff。\
                             合入永远由值班经理按——在那之前主干上没有你的改动。",
                            repair::REPAIR_COMMIT_MARK,
                            self.home.root().display()
                        )))
                    }
                }
            }
            "discard" => {
                let Some(repair_id) = args.get("repair_id").and_then(|v| v.as_str()) else {
                    return Ok(ToolOutcome::ok(
                        "repair(action=discard) 需要一个 repair_id——start 的回执里有它。",
                    ));
                };
                let session = repair::repair_session_for(
                    &self.home,
                    std::path::Path::new(&project.local_path),
                    &project.default_branch,
                    repair_id,
                    &session_id,
                )
                .await?;
                // 与 `finish` 同一姿态：目录不在就是「已经收过了」，回一句话而不是让 git 报错
                // ——「重复调一次 discard」不该看起来像工具坏了。
                if !session.worktree.exists() {
                    return Ok(ToolOutcome::ok(format!(
                        "{} 这个 worktree 已经不在（早先回收过，或人自己删的）——分支 {} 仍在。",
                        session.worktree.display(),
                        session.branch
                    )));
                }
                repair::finish_repair(std::path::Path::new(&project.local_path), &session, false)
                    .await?;
                Ok(ToolOutcome::ok(format!(
                    "worktree 已回收（分支 {} 留着——它是这次修复唯一的证据）。",
                    session.branch
                )))
            }
            other => Ok(ToolOutcome::ok(format!(
                "repair 的动作只有 start / finish / discard，收到的是「{other}」。"
            ))),
        }
    }

    /// `read_metrics`（票 01）：全局指标，**复用 `metrics::*` 纯函数口径**（决策 130② / 137）。
    ///
    /// 不在这里另写一套 SQL 聚合：那是指标页与端点的契约，两处各写一份必然漂移。
    async fn read_metrics(&self) -> Result<ToolOutcome> {
        let store = self.ledger_or_err()?;
        let tasks = store
            .list_tasks(&TaskFilter {
                include_archived: true,
                ..Default::default()
            })
            .await?;
        let statuses: Vec<crate::types::TaskStatus> = tasks.iter().map(|t| t.status).collect();
        let runs = store.all_runs().await?;
        let aggregation = store.stage_aggregation().await?;
        Ok(Self::readout(serde_json::json!({
            "tasks": tasks.len(),
            "success_rate": crate::metrics::success_rate(&statuses),
            "validate_first_pass_rate": crate::metrics::validate_first_pass_rate(&runs),
            "total_tokens": crate::metrics::total_tokens(&runs),
            "total_calls": crate::metrics::total_calls(&runs),
            // 阶段聚合的既有形状是 `(stage, 平均时长, 重试率, 总次数)`——`GET /metrics`
            // 与指标页都用这一份口径，故字段名逐字对齐（`avg_duration_ms` / `retry_rate` /
            // `total_runs`）。**不在这里给它改名换姓**：改过名的读数会让值班长把「平均时长」
            // 当成 token 数报给值班经理（第一次实现里就是错的）。
            "stage_aggregation": aggregation.iter().map(|(stage, avg_duration, retry_rate, total)| {
                serde_json::json!({
                    "stage": stage,
                    "avg_duration_ms": avg_duration,
                    "retry_rate": retry_rate,
                    "total_runs": total,
                })
            }).collect::<Vec<_>>(),
        })))
    }

    /// `read_projects`（票 01）：已接入的项目清单。
    async fn read_projects(&self) -> Result<ToolOutcome> {
        let store = self.ledger_or_err()?;
        let projects = store.list_projects().await?;
        Ok(Self::readout(serde_json::json!({
            "projects": projects.iter().map(|p| serde_json::json!({
                "project_id": p.id,
                "name": p.name,
                "local_path": p.local_path,
                "default_branch": p.default_branch,
                "language": p.language,
                "test_framework": p.test_framework,
                "lint_command": p.lint_command,
            })).collect::<Vec<_>>(),
        })))
    }

    /// `read_stage_configs`（票 01）：各阶段的配置。
    ///
    /// 原样交出去（含 `persona_path` / 技能声明）：这些是配置读数，不是秘密；
    /// 而选一个字段藏起来，模型就会开始猜「为什么这个工位是这样」。
    async fn read_stage_configs(&self) -> Result<ToolOutcome> {
        let store = self.ledger_or_err()?;
        let configs = store.list_stage_configs().await?;
        Ok(Self::readout(serde_json::json!({
            "stage_configs": configs.iter().map(|c| serde_json::json!({
                "stage": c.stage,
                "provider_id": c.provider_id,
                "temperature": c.temperature,
                "max_tokens": c.max_tokens,
                "persona_path": c.persona_path,
                "skills_json": c.skills_json,
                "idle_timeout_sec": c.idle_timeout_sec,
                "max_duration_sec": c.max_duration_sec,
            })).collect::<Vec<_>>(),
        })))
    }

    /// `read_skills`（票 01）：技能根下可用的技能 + 被谁引用。
    ///
    /// 与 `GET /skills` 同源（同一个 `discover`），不另走一条发现逻辑。
    async fn read_skills(&self) -> Result<ToolOutcome> {
        let store = self.ledger_or_err()?;
        let configs = store.list_stage_configs().await?;
        let skills = crate::agent::skills::discover(&self.home.skills_dir());
        Ok(Self::readout(serde_json::json!({
            "skills": skills.iter().map(|s| serde_json::json!({
                "name": s.name,
                "description": s.frontmatter.description,
                "path": s.path.display().to_string(),
                "declared_in": crate::config::declared_skill_where(&configs, &s.name),
            })).collect::<Vec<_>>(),
        })))
    }

    /// `read_providers`（票 01）：provider 清单，**密钥只回显掩码**（决策 112）。
    ///
    /// 库里存的是明文，故这里走**既有的**掩码读法 `list_providers_masked()`
    /// （provider 端点读的也是它），不是在这里另写一个正则。
    async fn read_providers(&self) -> Result<ToolOutcome> {
        let store = self.ledger_or_err()?;
        // `list_providers_masked` 是**既有的**那一份掩码读法（provider 端点也走它）：
        // 库里存的是明文密钥（决策 112），而「这个 provider 配没配密钥」值班长该知道，
        // 密钥本身对它没有用处。
        let providers = store.list_providers_masked().await?;
        Ok(Self::readout(serde_json::json!({
            "providers": providers.iter().map(|p| serde_json::json!({
                "provider_id": p.id,
                "vendor": p.vendor,
                "model": p.model,
                "enabled": p.enabled,
                "context_window": p.context_window,
                "base_url": p.base_url,
                // 已是掩码（上面的 `list_providers_masked`），不是原文。
                "api_key": p.api_key,
            })).collect::<Vec<_>>(),
        })))
    }

    /// `read_conversation`（决策 182⑭，票 02）：读某次节点运行的会话回执。
    ///
    /// `run_id` 缺省取该任务**最近一次**会话——人是按「那个货箱卡哪儿了」提问的，
    /// 不是按运行 id；让模型非要先查 run 列表才能问，等于把台账的内部编号变成使用门槛。
    async fn read_conversation(&self, call: &ToolCall) -> Result<ToolOutcome> {
        let store = self.ledger_or_err()?;
        let args = Self::args(call)?;
        let task_id = args
            .get("task_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if task_id.is_empty() {
            return Ok(ToolOutcome::ok(
                "read_conversation 需要一个 task_id 参数（可再加 run_id）。",
            ));
        }
        let requested_run = args.get("run_id").and_then(|v| v.as_i64());
        let run_id = match requested_run {
            Some(id) => id,
            None => match store.list_conversations(&task_id, false).await?.last() {
                Some(last) => last.run_id,
                None => {
                    return Ok(ToolOutcome::ok(format!(
                        "任务 {task_id} 还没有任何节点会话——它可能还没跑到调 LLM 的节点。"
                    )))
                }
            },
        };
        let conversation = match store.get_conversation(&task_id, run_id).await? {
            Some(c) => c,
            None => {
                return Ok(ToolOutcome::ok(format!(
                    "任务 {task_id} 的 {run_id} 号运行没有会话回执（可能是纯代码节点，\
                     或该运行已被清理）。用 read_task 看它当前停在哪个工位。"
                )))
            }
        };
        let messages = trim_conversation_messages(&conversation.messages_json);
        let value = serde_json::json!({
            "task_id": task_id,
            "run_id": run_id,
            "stage": conversation.stage.as_str(),
            "node": conversation.node.as_str(),
            "attempt": conversation.attempt,
            "agent_type": conversation.agent_type,
            "prompt_tokens": conversation.prompt_tokens,
            "completion_tokens": conversation.completion_tokens,
            "messages": messages,
        });
        // 消息已按条数与字符双重截断，但包上 stage / agent_type 等字段后仍可能略超上限；
        // 这里再兜一次（与 `read_task` 同一理由：它不走 L2 卸载）。
        Ok(ToolOutcome::ok(
            crate::pipeline::foreman::truncate_tool_result(&serde_json::to_string_pretty(&value)?),
        ))
    }

    async fn run_command(&self, call: &ToolCall, ctx: &ToolCallContext) -> Result<ToolOutcome> {
        let args = Self::args(call)?;
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::Validation("run_command 缺少 command".into()))?
            .to_string();
        let explicit_timeout = args.get("timeout_sec").and_then(|v| v.as_u64());
        let cwd = args
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .or_else(|| ctx.default_cwd.clone())
            .unwrap_or_else(|| ctx.worktree_path.clone());

        // 命令脱敏后落库（§12.4.4）
        let sanitized = super::sanitize::sanitize_command_line(&command);

        // 出口策略（决策 179，票 12）在**启动进程之前**判定：被拒的命令根本不执行。
        // 拒绝也要落 `kanban_node_commands`（与放行的命令同表）——审计面必须看得见
        // 「有过一次被拒的出口尝试」，否则策略只是一次静默失败。
        if let Err(denied) = self.egress.check(&command) {
            let id = self.record_command_start(ctx, &sanitized, &cwd).await?;
            if let (Some(rec), Some(id)) = (self.recorder.as_ref(), id) {
                rec.record_finish(
                    id,
                    CommandFinish {
                        exit_code: Some(crate::agent::egress::EGRESS_DENIED_EXIT_CODE),
                        stderr_preview: Some(denied.to_string()),
                        ..Default::default()
                    },
                )
                .await?;
            }
            return Err(denied);
        }

        let command_id = self.record_command_start(ctx, &sanitized, &cwd).await?;

        // 命令开始即刷新心跳（决策 100）
        if let Some(rec) = &self.recorder {
            rec.touch_heartbeat(ctx.run_id).await?;
        }

        let timeout_sec =
            effective_run_command_timeout(&self.settings, ctx.stage, explicit_timeout);
        // 决策 100：运行期间周期心跳——600s 级命令不被 300s 空闲超时误杀
        let heartbeat = self.spawn_command_heartbeat(ctx.run_id);
        let started = Instant::now();
        // 独立进程组启动（票 17 / 决策 66）：捕获真实 pgid 回填 node_runs，
        // 超时回调终止器杀整个进程组（此前 kill(0) 是 no-op）。
        let mut child_pgid: Option<i32> = None;
        // 逐行读 + 按行推流（票 14 / 决策 100 / §12.4.4）：输出经管道进入后台收集任务，
        // 完整内容全量缓冲用于落库与回填（推流是观测面，不改变 kanban_node_commands 口径）。
        let collected = std::sync::Arc::new(std::sync::Mutex::new(CollectedOutput::default()));
        let output = match crate::process::spawn_in_own_process_group(&command, &cwd) {
            Ok(child) => {
                child_pgid = child.id().map(|id| id as i32);
                if let (Some(rec), Some(run_id), Some(pgid)) =
                    (self.recorder.as_ref(), ctx.run_id, child_pgid)
                {
                    rec.set_process_group(run_id, pgid).await?;
                }
                // 流式收集：stdout / stderr 各起一条读行任务，边读边推 event
                let collect = self.spawn_streaming_collector(child, command_id, collected.clone());
                tokio::time::timeout(std::time::Duration::from_secs(timeout_sec), collect).await
            }
            Err(e) => Ok(Err(e)),
        };
        if let Some(task) = &heartbeat {
            task.abort();
        }

        let duration_ms = started.elapsed().as_millis() as u64;
        let (exit_code, stdout, stderr, timed_out) = match output {
            Ok(Ok(status)) => {
                let out = collected.lock().unwrap().clone();
                (status.code(), out.stdout, out.stderr, false)
            }
            Ok(Err(e)) => (None, String::new(), format!("命令启动失败：{e}"), false),
            Err(_) => {
                // 超时：杀掉整个进程组，已收到的输出仍保留（推流过的部分不丢）
                let out = collected.lock().unwrap().clone();
                if let Some(pgid) = child_pgid {
                    let _ = self.killer.kill_process_group(pgid);
                }
                (
                    None,
                    out.stdout,
                    format!("命令超时（{timeout_sec}s）"),
                    true,
                )
            }
        };

        // 决策 118：输出脱敏在**回填 messages 之前**执行
        let stdout = sanitize_text(&stdout);
        let stderr = sanitize_text(&stderr);

        // L1 裁剪 + L2 卸载（唯一阈值，决策 110）
        let (in_context, offload_path) = self.prepare_output(ctx, &stdout, &stderr)?;

        if let Some(rec) = &self.recorder {
            if let Some(id) = command_id {
                rec.record_finish(
                    id,
                    CommandFinish {
                        exit_code,
                        stdout_path: offload_path.clone(),
                        stdout_preview: Some(head_tail(&stdout, 50, 100)),
                        stderr_preview: Some(head_tail(&stderr, 50, 100)),
                        duration_ms,
                    },
                )
                .await?;
            }
            // 命令结束刷新心跳（决策 100）
            rec.touch_heartbeat(ctx.run_id).await?;
        }

        if timed_out {
            // 超时由节点级重试处理；这里把失败形态交给 agent loop，并杀掉整个进程组
            //（pgid 已在启动时捕获并回填 node_runs，决策 66 / 票 17）
            if let Some(pgid) = child_pgid {
                self.killer.kill_process_group(pgid)?;
            }
        }

        Ok(ToolOutcome::ok(in_context))
    }

    /// 落一条命令日志的「开始」并返回 id（未接记录器时 `None`）。
    ///
    /// 被拒的出口与正常执行**走同一个入口**（决策 179，票 12）：审计面必须看得见每一次
    /// 尝试，包括被拒的那些——否则策略在日志里完全不可见，只剩模型侧的一次报错。
    async fn record_command_start(
        &self,
        ctx: &ToolCallContext,
        sanitized: &str,
        cwd: &Path,
    ) -> Result<Option<i64>> {
        let Some(rec) = &self.recorder else {
            return Ok(None);
        };
        Ok(Some(
            rec.record_start(CommandStart {
                task_id: Some(ctx.task_id.clone()),
                session_id: ctx.session_id.clone(),
                run_id: ctx.run_id,
                stage: ctx.stage,
                node: ctx.node,
                source: ctx.command_source,
                command: sanitized.to_string(),
                cwd: cwd.display().to_string(),
            })
            .await?,
        ))
    }

    /// 逐行读子进程输出、全量缓冲并**按行推流**（票 14 / 决策 100 / §12.4.4）。
    ///
    /// 返回子进程退出状态；stdout / stderr 的完整内容写进 `collected`。
    /// 推流只作用于观测面：超长行截断、超高频停止推流，均**不影响**缓冲的完整输出
    /// （`kanban_node_commands` 的落库口径不变）。无 `sse` 或无 `command_id` 时
    /// 退化为纯缓冲（不阻塞、不漏内容，短命令与无订阅者场景不退化）。
    async fn spawn_streaming_collector(
        &self,
        mut child: tokio::process::Child,
        command_id: Option<i64>,
        collected: std::sync::Arc<std::sync::Mutex<CollectedOutput>>,
    ) -> std::io::Result<std::process::ExitStatus> {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let stdout_pipe = child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Unpin + Send>);
        let stderr_pipe = child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Unpin + Send>);
        let sse = self.sse.clone();

        let pump = |pipe: Option<Box<dyn tokio::io::AsyncRead + Unpin + Send>>,
                    is_stderr: bool,
                    collected: std::sync::Arc<std::sync::Mutex<CollectedOutput>>,
                    sse: Option<CommandSse>| async move {
            let Some(pipe) = pipe else { return };
            let mut lines = BufReader::new(pipe).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                // 1) 完整缓冲（未脱敏原样；脱敏在回填前统一做，保持既有顺序）
                {
                    let mut c = collected.lock().unwrap();
                    if is_stderr {
                        c.stderr.push_str(&line);
                        c.stderr.push('\n');
                    } else {
                        c.stdout.push_str(&line);
                        c.stdout.push('\n');
                    }
                    // 2) 推流（受节流约束）
                    if let (Some(sse), Some(cmd_id)) = (sse.as_ref(), command_id) {
                        if c.streamed_lines < STREAM_MAX_LINES {
                            c.streamed_lines += 1;
                            drop(c);
                            let chunk = if line.chars().count() > STREAM_MAX_LINE_CHARS {
                                let head: String =
                                    line.chars().take(STREAM_MAX_LINE_CHARS).collect();
                                format!("{head}…[本行超长已截断]")
                            } else {
                                line.clone()
                            };
                            // 推流内容同样脱敏（§12.4.4：四条路径一致）
                            let chunk = sanitize_text(&chunk);
                            sse.sink.emit(crate::sse::SseEvent::CommandOutput {
                                task_id: sse.task_id.clone(),
                                branch: sse.branch.clone(),
                                command_id: cmd_id,
                                chunk,
                            });
                            continue;
                        }
                        // 超过行数上限：只推一次「已停止推流」标注
                        if c.streamed_lines == STREAM_MAX_LINES {
                            c.streamed_lines += 1;
                            drop(c);
                            sse.sink.emit(crate::sse::SseEvent::CommandOutput {
                                task_id: sse.task_id.clone(),
                                branch: sse.branch.clone(),
                                command_id: cmd_id,
                                chunk: format!(
                                    "…[输出超过 {STREAM_MAX_LINES} 行，已停止推流；完整内容以命令记录为准]"
                                ),
                            });
                        }
                    }
                }
            }
        };

        tokio::join!(
            pump(stdout_pipe, false, collected.clone(), sse.clone()),
            pump(stderr_pipe, true, collected, sse)
        );
        child.wait().await
    }

    /// 周期心跳任务：命令结束（含超时）时由调用方 abort（决策 100）。
    fn spawn_command_heartbeat(&self, run_id: Option<i64>) -> Option<tokio::task::JoinHandle<()>> {
        let recorder = self.recorder.clone()?;
        let interval = self.command_heartbeat_interval;
        Some(tokio::spawn(async move {
            let mut tick = tokio::time::interval(interval);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            tick.tick().await; // interval 的首次 tick 立即完成，跳过（起止心跳已覆盖）
            loop {
                tick.tick().await;
                if recorder.touch_heartbeat(run_id).await.is_err() {
                    break;
                }
            }
        }))
    }

    /// 输出裁剪 + 卸载，返回（进 context 的文本，卸载路径）。
    ///
    /// 收的是 **ctx 而不是 task_id**：卸载目录按归属选维度（任务 / 值班会话），
    /// 而归属是上下文里的两列，不是单一的 task_id（票 03）。
    fn prepare_output(
        &self,
        ctx: &ToolCallContext,
        stdout: &str,
        stderr: &str,
    ) -> Result<(String, Option<String>)> {
        let combined = if stderr.is_empty() {
            stdout.to_string()
        } else {
            format!("{stdout}\n[stderr]\n{stderr}")
        };

        if !needs_offload(&combined, &self.settings) {
            return Ok((trim_run_command(&combined), None));
        }

        let dir = self.offload_dir(ctx)?;
        std::fs::create_dir_all(&dir)?;
        let name = format!("{}.txt", ulid::Ulid::new());
        let path = dir.join(name);
        std::fs::write(&path, &combined)?;
        let preview = head_tail(&combined, 30, 30);
        let tokens = count_tokens(&combined);
        Ok((
            offload_replacement("run_command", &path.display().to_string(), tokens, &preview),
            Some(path.display().to_string()),
        ))
    }
}

fn collect_entries(root: &Path, recursive: bool, out: &mut Vec<String>) -> Result<()> {
    // 只读目录，策略已通过 check_read 校验
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.file_type()?.is_dir();
        out.push(if is_dir { format!("{name}/") } else { name });
        if recursive && is_dir {
            collect_entries(&path, recursive, out)?;
        }
    }
    Ok(())
}

/// 有界读取（决策 226）：小文件整份读，大文件只读**需要的那一段**。返回（内容，省略说明）。
///
/// 省略说明非空时，内容是文件的一段而不是全部——**读的人必须知道这件事**，否则
/// 「日志里没有那一行」与「我没读到那一行」长得一样。
///
/// 三条读法的分工：
/// - **小文件（≤ [`READ_FILE_MAX_BYTES`]）走原路**：整份读。`trim_read_file` 的结构大纲
///   要往后扫全文，流式读给不了它，而小文件占绝大多数，这条路的行为必须一字不变。
/// - **大文件 + `tail`**：从尾部往回读最多一个上限，丢掉可能被切断的首行，再取最后
///   `limit`（默认 200）行。**`tail` 与 `offset` 不同时用**：给了 `tail` 就是「要尾巴」，
///   `offset` 被忽略（对着文件末尾数第 N 行不是任何人想要的读法）。
/// - **大文件 + 头部 / 区间**：按行流式读，读满所需行数即停；字节预算用尽也停。
///
/// 按字节而不是按行读，是因为「有多少行」这件事本身要先读完整个文件才知道——正是要避免的
/// 那一步。
fn read_text_bounded(
    path: &Path,
    offset: usize,
    limit: Option<usize>,
    tail: bool,
) -> Result<(String, Option<String>)> {
    use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};

    let size = std::fs::metadata(path)?.len() as usize;
    let take = limit.unwrap_or(crate::agent::context::READ_FILE_HEAD_LINES);
    let cap = crate::agent::context::READ_FILE_MAX_BYTES;
    if size <= cap {
        let content = std::fs::read_to_string(path)?;
        let lines: Vec<&str> = content.lines().collect();
        let sliced = if tail {
            last_lines(&lines, take)
        } else if offset > 0 || limit.is_some() {
            let end = limit
                .map(|l| (offset + l).min(lines.len()))
                .unwrap_or(lines.len());
            lines[offset.min(lines.len())..end].join("\n")
        } else {
            content
        };
        return Ok((sliced, None));
    }

    let mut file = std::fs::File::open(path)?;
    if tail {
        file.seek(SeekFrom::Start((size - cap) as u64))?;
        // 按字节读、再宽松解码：起点可能正好落在一个多字节字符中间（日志里中文常见），
        // 严格解码会当场报「不是一个 UTF-8 文件」，而这里只是读了半行。
        let mut raw = Vec::new();
        file.read_to_end(&mut raw)?;
        let text = String::from_utf8_lossy(&raw);
        let mut lines: Vec<&str> = text.lines().collect();
        // 首行是被 seek 切出来的半行：丢它，不丢就会把半行当成一条真日志读。
        if !lines.is_empty() {
            lines.remove(0);
        }
        let note = format!(
            "{path} 共 {size} 字节，超过单次上限 {cap} 字节：这里读到的是**尾部**（最后 \
             {cap} 字节里的最后 {} 行）。",
            lines.len().min(take),
            path = path.display()
        );
        let body = last_lines(&lines, take);
        return Ok((body, Some(note)));
    }

    let mut reader = BufReader::new(file);
    let mut out: Vec<String> = Vec::new();
    let mut seen = 0usize;
    let mut budget = cap;
    let mut hit_budget = false;
    let mut buf: Vec<u8> = Vec::new();
    loop {
        buf.clear();
        let read = reader.read_until(b'\n', &mut buf)?;
        if read == 0 {
            break;
        }
        if read > budget {
            hit_budget = true;
            break;
        }
        budget -= read;
        if seen >= offset && out.len() < take {
            out.push(
                String::from_utf8_lossy(&buf)
                    .trim_end_matches(['\n', '\r'])
                    .to_string(),
            );
        }
        seen += 1;
        if out.len() >= take {
            break;
        }
    }
    let why = if hit_budget {
        format!("读到 {cap} 字节上限就停了")
    } else {
        format!("只读了从第 {offset} 行起的 {} 行", out.len())
    };
    let note = format!(
        "{path} 共 {size} 字节，超过单次上限 {cap} 字节：{why}。\
         要看尾部请用 tail=true，要看别处请调 offset / limit。",
        path = path.display()
    );
    Ok((out.join("\n"), Some(note)))
}

/// 取最后 `take` 行（`read_text_bounded` 的两条尾部读法共用一处）。
fn last_lines(lines: &[&str], take: usize) -> String {
    lines[lines.len().saturating_sub(take)..].join("\n")
}

/// 首尾摘录（L2 预览 / 命令 preview 用）。
pub fn head_tail(text: &str, head: usize, tail: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= head + tail {
        return text.to_string();
    }
    let mut out: Vec<String> = lines[..head].iter().map(|l| l.to_string()).collect();
    out.push(format!("... 省略 {} 行 ...", lines.len() - head - tail));
    out.extend(lines[lines.len() - tail..].iter().map(|l| l.to_string()));
    out.join("\n")
}

// ─────────────────────────── 诊断包的装配（票 03）───────────────────────────

/// 诊断包里单段 prompt 原文的上限（票 02 的两列可能很长，而 12k 是**整个包**的账）。
const PROMPT_SNAPSHOT_MAX_CHARS: usize = 4_000;

/// 闸门日志的尾部上限：先把头部丢掉的那句说明保住，剩下的给尾部。
const GATE_LOG_TAIL_CHARS: usize = 1_200;

/// 诊断包里**逐条**列出的模型请求条数上限（决策 231）。整个包有 12k 的账，而请求多的 run
/// 可以有几打；`inflight` 单列一份，故这一份只承担「最近的调用长什么样」。
const MODEL_REQUEST_LIMIT: usize = 40;

/// 诊断包里**此刻在飞**的请求条数上限。在飞的本就该是个位数——超过这个数说明机器在并发
/// 跑好几个任务，那时逐条列全反而吃掉真正要看的那几行（留出 `truncated` 的余地给模型追问）。
const MODEL_REQUEST_INFLIGHT_LIMIT: usize = 20;

/// 一条模型请求的摘要（决策 231）。
///
/// `run_id` / `session_id` 是**归位**那一半：它把这一条与 `runs` 那一节里的某一行钉在一起，
/// 而 2026-09-19 那次实测里死的正是这一条（值班长把 run 27 的活栈记在 run 26 名下，
/// 而当时「栈里没有任何东西写着它是哪个 run 的」）。token 与 `bytes_received` 在**没量到**时
/// 是 `null` 而不是 0——「没有读数」与「量到零」是两件事，写 0 就是把 226③ 那个坑再挖一遍。
fn model_request_digest(
    request: &crate::storage::ModelRequest,
    now: DateTime<Utc>,
) -> serde_json::Value {
    serde_json::json!({
        "request_id": request.id,
        "run_id": request.run_id,
        "session_id": request.session_id,
        "agent_type": request.agent_type,
        "stage": request.stage,
        "node": request.node,
        "attempt": request.attempt,
        "seq": request.seq,
        "status": request.status.as_str(),
        "in_flight": request.in_flight(),
        "duration_ms": request.elapsed_ms(now),
        "prompt_tokens": request.usage.prompt_tokens,
        "completion_tokens": request.usage.completion_tokens,
        "cache_read_tokens": request.usage.cache_read_tokens,
        "cache_write_tokens": request.usage.cache_write_tokens,
        "bytes_received": request.usage.bytes_received,
        "last_byte_at": request.usage.last_byte_at.map(|t| t.to_rfc3339()),
        "error": request.error,
        "started_at": request.started_at.to_rfc3339(),
        "finished_at": request.finished_at.map(|t| t.to_rfc3339()),
    })
}

/// 一条 run 的摘要（诊断包的第 ③ 节与「最近那次失败」共用同一形状）。
///
/// `process_group_id` 不给原值、只给「空不空」：它是判「超时杀不杀得掉」的**唯一线索**
/// （决策 209 的实证：系统节点没有进程组，于是那一轮超时也杀不掉），而原值对模型没有意义。
fn run_digest(run: &NodeRun) -> serde_json::Value {
    serde_json::json!({
        "run_id": run.id,
        "stage": run.stage.as_str(),
        "node": run.node.as_str(),
        "attempt": run.attempt,
        "agent_type": run.agent_type,
        "status": run.status.as_str(),
        "duration_ms": run.duration_ms,
        "prompt_tokens": run.prompt_tokens,
        "completion_tokens": run.completion_tokens,
        "error": run.error,
        "step": run.step,
        "prompt_template_hash": run.prompt_template_hash,
        "has_process_group": run.process_group_id.is_some(),
        "continued_from_run_id": run.continued_from_run_id,
        "started_at": run.started_at.to_rfc3339(),
        "finished_at": run.finished_at.map(|t| t.to_rfc3339()),
    })
}

/// 会话的**尾巴**（最后几条往来）：失败前的最后几次工具调用正是诊断要看的现场。
/// 每条按字符截断并留标记——单条超长（一次 `write_file` 的参数）不该吃掉整屏。
fn tail_messages(messages: &serde_json::Value, take: usize) -> Vec<serde_json::Value> {
    let empty = Vec::new();
    let arr = messages.as_array().unwrap_or(&empty);
    arr[arr.len().saturating_sub(take)..]
        .iter()
        .map(|m| {
            let text = m.to_string();
            serde_json::json!({
                "role": m.get("role").and_then(|r| r.as_str()),
                "content": crate::storage::observability::truncate_text(&text, 1_500),
            })
        })
        .collect()
}

/// 闸门输出：按 stage 的**确定路径**找（与 executor 的命名同源：同一阶段重跑覆盖同一文件）。
///
/// 只报存在的那些：没有这条路的时候，硬造一条空路径反而会让模型以为「闸门跑了但输出丢了」。
fn gate_outputs(home: &Home, task_id: &str) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for stage in crate::types::ALL_STAGES {
        let path = home.task_file(task_id, &format!("gate-output-{}.log", stage.as_str()));
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        out.push(serde_json::json!({
            "stage": stage.as_str(),
            "path": path.display().to_string(),
            "tail": crate::storage::observability::truncate_text(&text, GATE_LOG_TAIL_CHARS),
        }));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 记录器替身：记录调用，不落库。
    #[derive(Default)]
    struct RecordingRecorder {
        starts: Mutex<Vec<CommandStart>>,
        finishes: Mutex<Vec<(i64, CommandFinish)>>,
        heartbeats: Mutex<u32>,
    }

    impl CommandRecorder for RecordingRecorder {
        fn record_start(&self, start: CommandStart) -> BoxFuture<'static, Result<i64>> {
            // 每次调用自增 id
            let starts = self.starts.lock().unwrap();
            let id = starts.len() as i64 + 1;
            drop(starts);
            self.starts.lock().unwrap().push(start);
            Box::pin(async move { Ok(id) })
        }

        fn record_finish(
            &self,
            command_id: i64,
            finish: CommandFinish,
        ) -> BoxFuture<'static, Result<()>> {
            self.finishes.lock().unwrap().push((command_id, finish));
            Box::pin(async move { Ok(()) })
        }

        fn touch_heartbeat(&self, _run_id: Option<i64>) -> BoxFuture<'static, Result<()>> {
            *self.heartbeats.lock().unwrap() += 1;
            Box::pin(async move { Ok(()) })
        }
    }

    struct NoKiller;
    impl ProcessKiller for NoKiller {
        fn kill_process_group(&self, _pgid: i32) -> Result<()> {
            Ok(())
        }
    }

    struct Setup {
        _tmp: tempfile::TempDir,
        home: Home,
        executor: ToolExecutor,
        ctx: ToolCallContext,
        worktree: PathBuf,
        task_dir: PathBuf,
    }

    fn setup(stage: Stage) -> Setup {
        let tmp = tempfile::tempdir().unwrap();
        let home = Home::new(tmp.path().join("home"));
        home.ensure_dirs().unwrap();
        let worktree = home.worktree_path("t1");
        let task_dir = home.task_dir("t1");
        home.ensure_task_dirs("t1").unwrap();

        let policy = FileToolPolicy::new(vec![worktree.clone(), task_dir.clone()]);
        let executor = ToolExecutor::new(
            home.clone(),
            policy,
            Settings::default(),
            Arc::new(NoKiller),
        );
        let ctx = ToolCallContext {
            task_id: "t1".into(),
            session_id: None,
            stage,
            node: Node::Execute,
            worktree_path: worktree.clone(),
            task_dir: task_dir.clone(),
            run_id: Some(1),
            command_source: CommandSource::Agent,
            default_cwd: Some(worktree.clone()),
        };
        Setup {
            _tmp: tmp,
            home,
            executor,
            ctx,
            worktree,
            task_dir,
        }
    }

    fn call(name: &str, args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "c1".into(),
            name: name.into(),
            arguments: args.to_string(),
        }
    }

    #[tokio::test]
    async fn write_file_writes_design_doc_to_task_dir() {
        let s = setup(Stage::ArchitectDesign);
        let out = s
            .executor
            .execute(
                &call(
                    "write_file",
                    serde_json::json!({"path": "design.md", "content": "# 设计"}),
                ),
                &s.ctx,
            )
            .await
            .unwrap();
        assert!(out.content.contains("\"success\":true"));
        assert_eq!(
            std::fs::read_to_string(s.task_dir.join("design.md")).unwrap(),
            "# 设计"
        );
        assert!(
            !s.worktree.join("design.md").exists(),
            "设计文档不得写进 worktree"
        );
    }

    #[tokio::test]
    async fn write_file_writes_code_to_worktree() {
        let s = setup(Stage::Develop);
        s.executor
            .execute(
                &call(
                    "write_file",
                    serde_json::json!({"path": "src/main.rs", "content": "fn main(){}"}),
                ),
                &s.ctx,
            )
            .await
            .unwrap();
        assert!(s.worktree.join("src/main.rs").exists());
    }

    #[tokio::test]
    async fn write_file_is_idempotent() {
        let s = setup(Stage::ArchitectDesign);
        for _ in 0..2 {
            s.executor
                .execute(
                    &call(
                        "write_file",
                        serde_json::json!({"path": "design.md", "content": "v1"}),
                    ),
                    &s.ctx,
                )
                .await
                .unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(s.task_dir.join("design.md")).unwrap(),
            "v1"
        );
    }

    #[tokio::test]
    async fn file_policy_blocks_dotenv_write() {
        let s = setup(Stage::Develop);
        let err = s
            .executor
            .execute(
                &call(
                    "write_file",
                    serde_json::json!({"path": ".env", "content": "K=v"}),
                ),
                &s.ctx,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, Error::PolicyDenied(_)));
    }

    #[tokio::test]
    async fn file_policy_blocks_path_outside_roots() {
        let s = setup(Stage::Develop);
        let err = s
            .executor
            .execute(
                &call(
                    "write_file",
                    serde_json::json!({"path": "/etc/passwd", "content": "x"}),
                ),
                &s.ctx,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, Error::PolicyDenied(_)));
    }

    #[tokio::test]
    async fn read_file_falls_back_from_worktree_to_task_dir() {
        let s = setup(Stage::Develop);
        std::fs::write(s.task_dir.join("design.md"), "line1\nline2").unwrap();
        let out = s
            .executor
            .execute(
                &call("read_file", serde_json::json!({"path": "design.md"})),
                &s.ctx,
            )
            .await
            .unwrap();
        assert_eq!(out.content, "line1\nline2");
    }

    #[tokio::test]
    async fn read_file_missing_is_an_error() {
        let s = setup(Stage::Develop);
        assert!(s
            .executor
            .execute(
                &call("read_file", serde_json::json!({"path": "nope.md"})),
                &s.ctx
            )
            .await
            .is_err());
    }

    #[tokio::test]
    async fn a_big_file_is_read_without_being_loaded_whole() {
        // 决策 226：超过 READ_FILE_MAX_BYTES 的文件不再整份读。造一个刚好越线的日志，
        // 两种读法各断言一次——**头部**读法要如实说「只读了这一段」，**尾部**读法要给到
        // 最后一行（追加写的文件要的就是它）。
        let s = setup(Stage::Develop);
        let cap = crate::agent::context::READ_FILE_MAX_BYTES;
        let mut body = String::new();
        let mut n = 0usize;
        while body.len() <= cap + 1024 {
            body.push_str(&format!("line-{n} 填充一下\n"));
            n += 1;
        }
        let last = n - 1;
        std::fs::write(s.worktree.join("big.log"), &body).unwrap();

        let head = s
            .executor
            .execute(
                &call(
                    "read_file",
                    serde_json::json!({"path": "big.log", "limit": 5}),
                ),
                &s.ctx,
            )
            .await
            .unwrap();
        assert!(head.content.contains("line-0"), "头部读法要给开头");
        assert!(head.content.contains("超过单次上限"), "{}", head.content);

        let tail = s
            .executor
            .execute(
                &call(
                    "read_file",
                    serde_json::json!({"path": "big.log", "tail": true, "limit": 5}),
                ),
                &s.ctx,
            )
            .await
            .unwrap();
        assert!(
            tail.content.contains(&format!("line-{last}")),
            "尾部读法要给到最后一行：{}",
            tail.content
        );
        assert!(
            !tail.content.contains("line-0\n"),
            "尾部读法不该从开头给起：{}",
            tail.content
        );
        assert!(tail.content.contains("尾部"), "{}", tail.content);
    }

    #[tokio::test]
    async fn read_file_supports_offset_limit() {
        let s = setup(Stage::Develop);
        std::fs::write(s.worktree.join("a.txt"), "l0\nl1\nl2\nl3").unwrap();
        let out = s
            .executor
            .execute(
                &call(
                    "read_file",
                    serde_json::json!({"path": "a.txt", "offset": 1, "limit": 2}),
                ),
                &s.ctx,
            )
            .await
            .unwrap();
        assert_eq!(out.content, "l1\nl2");
    }

    #[tokio::test]
    async fn edit_file_replaces_once_and_errors_when_missing() {
        let s = setup(Stage::Develop);
        std::fs::write(s.worktree.join("a.txt"), "x x x").unwrap();
        s.executor
            .execute(
                &call(
                    "edit_file",
                    serde_json::json!({"path": "a.txt", "old_text": "x", "new_text": "y"}),
                ),
                &s.ctx,
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(s.worktree.join("a.txt")).unwrap(),
            "y x x"
        );

        assert!(s
            .executor
            .execute(
                &call(
                    "edit_file",
                    serde_json::json!({"path": "a.txt", "old_text": "zzz", "new_text": "q"}),
                ),
                &s.ctx,
            )
            .await
            .is_err());
    }

    #[tokio::test]
    async fn delete_file_is_idempotent() {
        let s = setup(Stage::Develop);
        std::fs::write(s.worktree.join("a.txt"), "x").unwrap();
        for _ in 0..2 {
            s.executor
                .execute(
                    &call("delete_file", serde_json::json!({"path": "a.txt"})),
                    &s.ctx,
                )
                .await
                .unwrap();
        }
        assert!(!s.worktree.join("a.txt").exists());
    }

    #[tokio::test]
    async fn list_dir_caps_and_marks_directories() {
        let s = setup(Stage::Develop);
        std::fs::create_dir_all(s.worktree.join("src")).unwrap();
        for i in 0..5 {
            std::fs::write(s.worktree.join(format!("f{i}.rs")), "").unwrap();
        }
        let out = s
            .executor
            .execute(&call("list_dir", serde_json::json!({"path": "."})), &s.ctx)
            .await
            .unwrap();
        assert!(out.content.contains("src/"));
        assert!(out.content.contains("f0.rs"));
    }

    #[tokio::test]
    async fn run_command_executes_for_real_and_records() {
        let s = setup(Stage::Develop);
        let recorder = Arc::new(RecordingRecorder::default());
        let executor = ToolExecutor::new(
            s.home.clone(),
            FileToolPolicy::new(vec![s.worktree.clone(), s.task_dir.clone()]),
            Settings::default(),
            Arc::new(NoKiller),
        )
        .with_recorder(recorder.clone());

        let out = executor
            .execute(
                &call("run_command", serde_json::json!({"command": "echo hello"})),
                &s.ctx,
            )
            .await
            .unwrap();
        assert!(out.content.contains("hello"), "实际输出：{}", out.content);

        let starts = recorder.starts.lock().unwrap();
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0].source, CommandSource::Agent);
        assert_eq!(starts[0].command, "echo hello");
        drop(starts);

        let finishes = recorder.finishes.lock().unwrap();
        assert_eq!(finishes.len(), 1);
        assert_eq!(finishes[0].1.exit_code, Some(0));
        assert!(finishes[0].1.duration_ms < 60_000);
        drop(finishes);

        // 命令开始与结束都刷新心跳（决策 100）
        assert_eq!(*recorder.heartbeats.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn run_command_touches_heartbeat_periodically_during_long_commands() {
        // 决策 100：长命令（如 600s 测试）靠运行期周期心跳躲过 300s 空闲超时
        let s = setup(Stage::Develop);
        let recorder = Arc::new(RecordingRecorder::default());
        let executor = ToolExecutor::new(
            s.home.clone(),
            FileToolPolicy::new(vec![s.worktree.clone(), s.task_dir.clone()]),
            Settings::default(),
            Arc::new(NoKiller),
        )
        .with_recorder(recorder.clone())
        .with_command_heartbeat_interval(std::time::Duration::from_millis(50));
        executor
            .execute(
                &call("run_command", serde_json::json!({"command": "sleep 0.3"})),
                &s.ctx,
            )
            .await
            .unwrap();
        // 起止各一次 + 运行期间若干次
        assert!(*recorder.heartbeats.lock().unwrap() >= 4, "周期心跳未生效");
    }

    #[tokio::test]
    async fn run_command_output_is_sanitized_before_backfill() {
        // 决策 118：agent 看到的即脱敏后文本
        let s = setup(Stage::Develop);
        let secret = "sk-abcdefghijklmnop12345678";
        let out = s
            .executor
            .execute(
                &call(
                    "run_command",
                    serde_json::json!({"command": format!("echo {secret}")}),
                ),
                &s.ctx,
            )
            .await
            .unwrap();
        assert!(!out.content.contains(secret), "输出未脱敏：{}", out.content);
        assert!(out.content.contains("***"));
    }

    #[tokio::test]
    async fn run_command_commands_are_sanitized_in_the_log() {
        let s = setup(Stage::Develop);
        let recorder = Arc::new(RecordingRecorder::default());
        let executor = ToolExecutor::new(
            s.home.clone(),
            FileToolPolicy::new(vec![s.worktree.clone(), s.task_dir.clone()]),
            Settings::default(),
            Arc::new(NoKiller),
        )
        .with_recorder(recorder.clone());
        executor
            .execute(
                &call(
                    "run_command",
                    serde_json::json!({"command": "deploy --token sk-abcdefghijklmnop12345678"}),
                ),
                &s.ctx,
            )
            .await
            .unwrap();
        let starts = recorder.starts.lock().unwrap();
        assert!(starts[0].command.contains("--token ***"));
        assert!(!starts[0].command.contains("sk-abcdefghijklmnop"));
    }

    #[tokio::test]
    async fn env_var_values_are_sanitized_in_output_and_command_log() {
        // 票 15 / §12.4.4：环境变量值脱敏在**输出回填**与**命令记录**两条路径一致。
        let s = setup(Stage::Develop);
        let recorder = Arc::new(RecordingRecorder::default());
        let executor = ToolExecutor::new(
            s.home.clone(),
            FileToolPolicy::new(vec![s.worktree.clone(), s.task_dir.clone()]),
            Settings::default(),
            Arc::new(NoKiller),
        )
        .with_recorder(recorder.clone());
        // 命令自身含敏感环境变量赋值；输出回显同样的赋值
        let out = executor
            .execute(
                &call(
                    "run_command",
                    serde_json::json!({"command": "export API_TOKEN=abc123value; echo \"API_TOKEN=abc123value PATH=$PATH\""}),
                ),
                &s.ctx,
            )
            .await
            .unwrap();
        // 工具结果（回填 agent messages）不含明文
        assert!(
            !out.content.contains("abc123value"),
            "工具结果未脱敏：{}",
            out.content
        );
        assert!(out.content.contains("API_TOKEN=***"));
        // 命令记录同样不含明文
        let starts = recorder.starts.lock().unwrap();
        assert!(!starts[0].command.contains("abc123value"));
        assert!(starts[0].command.contains("API_TOKEN=***"));
    }

    #[tokio::test]
    async fn benign_env_var_values_survive_command_output() {
        // 票 15：无害变量（PATH / 纯数字 / 非敏感名）不被误伤，输出原样保留。
        let s = setup(Stage::Develop);
        let out = s
            .executor
            .execute(
                &call(
                    "run_command",
                    serde_json::json!({"command": "echo \"PATH=/usr/bin FOO=secret RETRIES=3\""}),
                ),
                &s.ctx,
            )
            .await
            .unwrap();
        assert!(out.content.contains("PATH=/usr/bin"));
        assert!(out.content.contains("FOO=secret"));
        assert!(out.content.contains("RETRIES=3"));
    }

    #[tokio::test]
    async fn large_output_is_offloaded_to_context_dir() {
        let s = setup(Stage::Develop);
        let settings = Settings {
            offload_threshold_tokens: 10, // 降低阈值便于测试
            ..Default::default()
        };
        let executor = ToolExecutor::new(
            s.home.clone(),
            FileToolPolicy::new(vec![s.worktree.clone(), s.task_dir.clone()]),
            settings,
            Arc::new(NoKiller),
        );
        let out = executor
            .execute(
                &call("run_command", serde_json::json!({"command": "seq 1 5000"})),
                &s.ctx,
            )
            .await
            .unwrap();
        assert!(out.content.contains("已卸载"), "应走 L2：{}", out.content);
        let offloaded = std::fs::read_dir(s.home.context_dir("t1")).unwrap().count();
        assert_eq!(offloaded, 1, "卸载文件应真实落盘（决策 148）");
    }

    #[tokio::test]
    async fn small_output_is_not_offloaded() {
        let s = setup(Stage::Develop);
        let out = s
            .executor
            .execute(
                &call("run_command", serde_json::json!({"command": "echo tiny"})),
                &s.ctx,
            )
            .await
            .unwrap();
        assert!(!out.content.contains("已卸载"));
        assert_eq!(
            std::fs::read_dir(s.home.context_dir("t1")).unwrap().count(),
            0
        );
    }

    #[tokio::test]
    async fn submit_metadata_returns_typed_payload() {
        let s = setup(Stage::ArchitectDesign);
        let out = s
            .executor
            .execute(
                &call(
                    "submit_metadata",
                    serde_json::json!({"readiness": true, "affected_files": []}),
                ),
                &s.ctx,
            )
            .await
            .unwrap();
        assert_eq!(out.metadata.unwrap()["readiness"], true);
    }

    // ── 票 06（决策 172③）：`Skill` 工具 ──

    /// 在技能根（`{home}/skills`）下写一个用户技能。
    fn write_home_skill(s: &Setup, name: &str, content: &str) {
        let dir = s.home.skills_dir().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), content).unwrap();
    }

    #[tokio::test]
    async fn skill_tool_returns_skill_body() {
        let s = setup(Stage::ArchitectDesign);
        write_home_skill(&s, "grill", "拷问协议：把设计树走完");
        let out = s
            .executor
            .execute(&call("Skill", serde_json::json!({"name": "grill"})), &s.ctx)
            .await
            .unwrap();
        assert_eq!(out.content, "拷问协议：把设计树走完");
        assert!(out.metadata.is_none(), "Skill 不是 submit_metadata");
    }

    /// 票 07：`Skill` 工具返回的正文**含一级展开的兄弟文件**。
    ///
    /// 上游技能的 `[tests.md](tests.md)` 是双向死指针（文件工具读不到技能目录），
    /// 展开走加载器；二级引用不递归。
    #[tokio::test]
    async fn skill_tool_inlines_siblings_one_level() {
        let s = setup(Stage::ArchitectDesign);
        write_home_skill(&s, "tdd", "主文档\n\n[tests.md](tests.md)");
        let dir = s.home.skills_dir().join("tdd");
        std::fs::write(
            dir.join("tests.md"),
            "兄弟：一个用例一件事\n[deep.md](deep.md)",
        )
        .unwrap();
        std::fs::write(dir.join("deep.md"), "二级内容不该出现").unwrap();

        let out = s
            .executor
            .execute(&call("Skill", serde_json::json!({"name": "tdd"})), &s.ctx)
            .await
            .unwrap();
        assert!(out.content.contains("主文档"), "{}", out.content);
        assert!(
            out.content.contains("一个用例一件事"),
            "一级兄弟文件应内联：{}",
            out.content
        );
        assert!(
            !out.content.contains("二级内容不该出现"),
            "二级引用不得展开：{}",
            out.content
        );
    }

    /// 缺失的兄弟文件 → 工具返回错误文本（走文本通道，不触发 `tool_retry_max`）。
    #[tokio::test]
    async fn skill_tool_missing_sibling_returns_text() {
        let s = setup(Stage::ArchitectDesign);
        write_home_skill(&s, "broken", "[gone.md](gone.md)");
        let out = s
            .executor
            .execute(
                &call("Skill", serde_json::json!({"name": "broken"})),
                &s.ctx,
            )
            .await
            .expect("技能包残缺不得走 Err 通道");
        assert!(out.content.contains("gone.md"), "{}", out.content);
        assert!(out.content.contains("broken"), "{}", out.content);
    }

    /// 未知技能名**不是工具失败**——返回说明文本让模型自行纠正（票 06）。
    ///
    /// 若走 `Err`，agent loop 会把它算进 `tool_retry_max`（决策 33），模型写错一个名字
    /// 就可能把整个节点打挂。
    #[tokio::test]
    async fn skill_tool_unknown_name_returns_text_not_error() {
        let s = setup(Stage::ArchitectDesign);
        let out = s
            .executor
            .execute(&call("Skill", serde_json::json!({"name": "nope"})), &s.ctx)
            .await
            .expect("未知技能名不得走 Err 通道");
        assert!(out.content.contains("nope"), "{}", out.content);
        assert!(out.content.contains("无法加载"), "{}", out.content);
    }

    /// 缺 `name` 参数同样返回可读文本，而不是 `Err`。
    #[tokio::test]
    async fn skill_tool_missing_name_returns_text() {
        let s = setup(Stage::ArchitectDesign);
        let out = s
            .executor
            .execute(&call("Skill", serde_json::json!({})), &s.ctx)
            .await
            .expect("缺参不得走 Err 通道");
        assert!(out.content.contains("name"), "{}", out.content);
    }

    /// 技能根**不经** `FileToolPolicy`——`read_file` 读技能根会被拒，`Skill` 工具能读。
    ///
    /// 这条同时钉住决策 172 的安全边界：技能根与 `{home}/data/`（provider 密钥明文存储，
    /// 决策 112）同父，**不得**被放宽为 agent 可读；兄弟文件走加载器展开而非放宽文件策略。
    #[tokio::test]
    async fn skill_tool_reads_skill_root_that_file_policy_refuses() {
        let s = setup(Stage::ArchitectDesign);
        write_home_skill(&s, "secret-ish", "技能正文");
        let skill_path = s.home.skills_dir().join("secret-ish").join("SKILL.md");

        // 文件工具被策略挡住（技能根不在 worktree / 任务目录内）
        assert!(
            s.executor
                .execute(
                    &call(
                        "read_file",
                        serde_json::json!({"path": skill_path.display().to_string()})
                    ),
                    &s.ctx,
                )
                .await
                .is_err(),
            "read_file 不得读技能根"
        );

        // 而 Skill 工具（loader 侧）能取到正文
        let out = s
            .executor
            .execute(
                &call("Skill", serde_json::json!({"name": "secret-ish"})),
                &s.ctx,
            )
            .await
            .unwrap();
        assert_eq!(out.content, "技能正文");
    }

    #[tokio::test]
    async fn unknown_tool_is_rejected() {
        let s = setup(Stage::Develop);
        assert!(s
            .executor
            .execute(
                &call("definitely_not_a_tool", serde_json::json!({})),
                &s.ctx
            )
            .await
            .is_err());
    }

    /// 票 08：未注入子代理运行器时，`spawn_sub_agent` **不是**未知工具——
    /// 它返回说明文本（走文本通道，不烧 `tool_retry_max`），与 `Skill` 的未知技能
    /// 名同一姿态。这样「工具存在但本轮未启用」与「工具名写错」是两件事。
    #[tokio::test]
    async fn spawn_sub_agent_without_runner_returns_text_not_error() {
        let s = setup(Stage::Develop);
        let out = s
            .executor
            .execute(
                &call(
                    "spawn_sub_agent",
                    serde_json::json!({"task": "找出所有调用点"}),
                ),
                &s.ctx,
            )
            .await
            .expect("未启用不得走 Err 通道");
        assert!(out.content.contains("未启用"), "{}", out.content);
    }

    /// 票 08：子代理运行器已注入但缺 `task` → 提示补参（仍是文本通道）。
    #[tokio::test]
    async fn spawn_sub_agent_requires_task_argument() {
        let s = setup(Stage::Develop);
        let executor = s.executor.with_sub_agent(std::sync::Arc::new(NoSubAgent));
        let out = executor
            .execute(&call("spawn_sub_agent", serde_json::json!({})), &s.ctx)
            .await
            .expect("缺参不得走 Err 通道");
        assert!(out.content.contains("task"), "{}", out.content);
    }

    /// 票 08：缺父 run（无 run_id）时拒绝派生——否则会落一行无父的孤儿 run。
    #[tokio::test]
    async fn spawn_sub_agent_without_run_id_is_refused() {
        let s = setup(Stage::Develop);
        let executor = s.executor.with_sub_agent(std::sync::Arc::new(NoSubAgent));
        let mut ctx = s.ctx.clone();
        ctx.run_id = None;
        let out = executor
            .execute(
                &call("spawn_sub_agent", serde_json::json!({"task": "检索"})),
                &ctx,
            )
            .await
            .expect("缺 run 上下文不得走 Err 通道");
        assert!(out.content.contains("run_id"), "{}", out.content);
    }

    /// 票 08：注入运行器后，子代理返回的摘要原样进 tool_result。
    #[tokio::test]
    async fn spawn_sub_agent_returns_runner_summary() {
        let s = setup(Stage::Develop);
        let executor = s.executor.with_sub_agent(std::sync::Arc::new(NoSubAgent));
        let out = executor
            .execute(
                &call("spawn_sub_agent", serde_json::json!({"task": "检索"})),
                &s.ctx,
            )
            .await
            .expect("正常路径不得报错");
        assert_eq!(out.content, "子代理摘要");
    }

    /// 测试替身：不调 LLM，直接回固定摘要。
    struct NoSubAgent;

    impl SubAgentRunner for NoSubAgent {
        fn run(
            &self,
            _request: SubAgentRequest,
        ) -> futures::future::BoxFuture<'static, Result<String>> {
            Box::pin(async { Ok("子代理摘要".to_string()) })
        }
    }

    #[test]
    fn head_tail_keeps_both_ends() {
        let text = (0..500)
            .map(|i| format!("l{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let out = head_tail(&text, 2, 2);
        assert!(out.contains("l0"));
        assert!(out.contains("l499"));
        assert!(out.contains("省略 496 行"));
        assert!(!out.contains("l100"));
    }

    #[tokio::test]
    async fn l2_offload_covers_non_command_tools() {
        // 票 04 / 决策 110：L2 卸载覆盖**全部工具**，不再只对 run_command 生效。
        // read_file 显式要求大 limit 时 L1 不裁剪（用户点名要这么多行），
        // 结果超 offload_threshold_tokens → 落盘 + 只留预览。
        let s = setup(Stage::Develop);
        let long = (0..20_000)
            .map(|i| format!("line {i} of a very long file"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(s.worktree.join("big.rs"), &long).unwrap();

        let out = s
            .executor
            .execute(
                &call(
                    "read_file",
                    serde_json::json!({"path": "big.rs", "limit": 20_000}),
                ),
                &s.ctx,
            )
            .await
            .unwrap();
        assert!(
            out.content.contains("已卸载"),
            "read_file 超阈值结果应走 L2 卸载：{}",
            &out.content[..out.content.len().min(200)]
        );
        // 卸载文件真实落盘且含完整内容
        let ctx_dir = s.home.context_dir("t1");
        let files: Vec<_> = std::fs::read_dir(&ctx_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(!files.is_empty(), "L2 卸载文件应落盘");
        assert!(
            files.iter().any(|f| std::fs::read_to_string(f.path())
                .map(|c| c.contains("line 19999"))
                .unwrap_or(false)),
            "卸载文件应含完整内容"
        );
    }

    #[tokio::test]
    async fn l2_offload_skips_small_results() {
        // 未超阈值的小结果原样返回（L2 不误伤）
        let s = setup(Stage::Develop);
        std::fs::write(s.worktree.join("small.rs"), "fn main() {}\n").unwrap();
        let out = s
            .executor
            .execute(
                &call("read_file", serde_json::json!({"path": "small.rs"})),
                &s.ctx,
            )
            .await
            .unwrap();
        assert!(!out.content.contains("已卸载"));
        assert!(out.content.contains("fn main"));
    }

    /// 简单 SSE 录制器（只关心 command_output，验证推流路径）。
    #[derive(Default, Clone)]
    struct SseRecorder {
        chunks: Arc<std::sync::Mutex<Vec<String>>>,
    }
    impl crate::sse::SseSink for SseRecorder {
        fn emit(&self, event: crate::sse::SseEvent) {
            if let crate::sse::SseEvent::CommandOutput { chunk, .. } = event {
                self.chunks.lock().unwrap().push(chunk);
            }
        }
    }

    #[tokio::test]
    async fn run_command_streams_output_lines_before_completion() {
        // 票 14 / 决策 100：命令执行期间按行推送 command_output，
        // 而不是等进程结束一次性缓冲。命令先输出再睡，推流应早于结束发生。
        let s = setup(Stage::Develop);
        let sse = SseRecorder::default();
        // command_id 由 recorder 分配——没有 recorder 就没有 command_id，也就没有推流
        // （这正是「无订阅者 / 无命令记录时不推流」的退化路径）。
        let recorder = Arc::new(RecordingRecorder::default());
        let executor = ToolExecutor::new(
            s.home.clone(),
            FileToolPolicy::new(vec![s.worktree.clone(), s.task_dir.clone()]),
            Settings::default(),
            Arc::new(NoKiller),
        )
        .with_recorder(recorder)
        .with_sse(crate::agent::tools::CommandSse {
            sink: Arc::new(sse.clone()),
            task_id: "t1".into(),
            branch: "main".into(),
        });
        // 输出 → 睡 0.4s → 再输出：推流分两次，逐行到达
        let out = executor
            .execute(
                &call(
                    "run_command",
                    serde_json::json!({"command": "echo first; sleep 0.4; echo second"}),
                ),
                &s.ctx,
            )
            .await
            .unwrap();
        // 完整输出仍全量回填（推流不改变回填口径）
        assert!(out.content.contains("first") && out.content.contains("second"));
        let chunks = sse.chunks.lock().unwrap().clone();
        assert!(
            chunks.iter().any(|c| c.contains("first")),
            "应推送首行：{chunks:?}"
        );
        assert!(
            chunks.iter().any(|c| c.contains("second")),
            "应推送后续行：{chunks:?}"
        );
    }

    #[tokio::test]
    async fn command_streaming_sanitizes_chunks() {
        // §12.4.4 / 票 14：推流路径同样脱敏，不出现「先推明文后脱敏」的窗口。
        let s = setup(Stage::Develop);
        let sse = SseRecorder::default();
        let executor = ToolExecutor::new(
            s.home.clone(),
            FileToolPolicy::new(vec![s.worktree.clone(), s.task_dir.clone()]),
            Settings::default(),
            Arc::new(NoKiller),
        )
        .with_recorder(Arc::new(RecordingRecorder::default()))
        .with_sse(crate::agent::tools::CommandSse {
            sink: Arc::new(sse.clone()),
            task_id: "t1".into(),
            branch: "main".into(),
        });
        executor
            .execute(
                &call(
                    "run_command",
                    serde_json::json!({"command": "export API_TOKEN=abc123secret; echo done"}),
                ),
                &s.ctx,
            )
            .await
            .unwrap();
        let chunks = sse.chunks.lock().unwrap().clone();
        assert!(
            !chunks.iter().any(|c| c.contains("abc123secret")),
            "推流内容应已脱敏：{chunks:?}"
        );
    }

    #[tokio::test]
    async fn command_without_sse_still_buffers_fully() {
        // 无订阅者场景不退化（票 14）：不推流但完整输出照常回填。
        let s = setup(Stage::Develop);
        let out = s
            .executor
            .execute(
                &call("run_command", serde_json::json!({"command": "echo hello"})),
                &s.ctx,
            )
            .await
            .unwrap();
        assert!(out.content.contains("hello"));
    }
}
