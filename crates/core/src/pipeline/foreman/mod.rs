//! 值班长：对讲台背后的对话 agent（决策 182，票 01 / 02 / 05）。
//!
//! **存在理由**：看板说得出「什么状态」，说不出「为什么」与「该怎么办」。值班长把夜班
//! 态势读成人话，需要深挖时它自己翻只读台账。它**不依赖任何任务的存在**——首启空 home
//! 也能对话（这是本特性最初被否掉的前提：「对话不需要依赖任务」）。
//!
//! ## 三条硬边界
//!
//! 1. **动手分层、不直连状态机。** 写动作按**档位与族**走（决策 206/207）：环境层
//!    （`write_file` / `edit_file` / `run_command` / `repair`）在 `ask` 档落成**提议**等按键、
//!    `auto` 档直通、`deny` 档连广告都不给；本服务写接口（`task` / `config` / `skills` /
//!    `service`）**恒为提议、不看档位**，按下确认钮后由 `run_proposal_tool` 走**既有端点**
//!    （同一套校验，没有第二条改状态的路）；决策 210 的托管另对单任务放免按键
//!    `task resume(continue)`。工具集是**清单驱动**的（[`FOREMAN_TOOL_SPECS`]，顺序有冻结断言钉住），
//!    白名单在 [`ToolExecutor::with_allowed_tools`] 的执行点强制。它的回复里也永远不出现
//!    按钮——这个约束落在前端（票 04），时间线上唯一的钮是提议轮的确认钮（决策 207③）。
//! 2. **域是家目录根，不是任务工作区。** 文件与命令工具（决策 206/207 起在列）的路径都
//!    相对家目录根（`home.root()`），`data/` 按路径前缀拒（库里明文存着 provider 密钥）；
//!    `logs/` 的体量由 `read_file` 的字节上限管（决策 226 撤掉了前缀墙）。流水线自身调用的
//!    只读子代理是另一个边界（[`crate::pipeline::subagent`]），与这里无关。
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

/// `stage_configs.stage` 的第 4 个伪键（决策 182①）。
///
/// 与 [`crate::pipeline::pseudo::PseudoStage`] 的三个键并列，但**不是** `PseudoStage` 的
/// 变体：那三个都是流水线节点内同步发起的调用，值班长与流水线无关。
pub const FOREMAN_STAGE_KEY: &str = "foreman";

/// run / SSE 载荷里的身份串（决策 182⑥）。
///
/// 现在不落 run 行（值班长没有运行行，见 `say` 的注释），但 SSE 增量事件按它过滤。
pub const FOREMAN_AGENT_TYPE: &str = "foreman";

mod attribution;
mod briefing;
mod catalog;
mod conversation;
mod registry;
mod runner;

pub use attribution::*;
pub use briefing::*;
pub use catalog::*;
pub use conversation::*;
pub use registry::*;
pub use runner::*;
