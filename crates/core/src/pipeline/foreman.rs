//! 值班长：对讲台背后的对话 agent（决策 182，票 01 / 02 / 05）。
//!
//! **存在理由**：看板说得出「什么状态」，说不出「为什么」与「该怎么办」。值班长把夜班
//! 态势读成人话，需要深挖时它自己翻只读台账。它**不依赖任何任务的存在**——首启空 home
//! 也能对话（这是本特性最初被否掉的前提：「对话不需要依赖任务」）。
//!
//! ## 三条硬边界
//!
//! 1. **不动手。** 写动作（resume / retry / 拍板 / merge / 建任务）一律由后端下发、由人按下。
//!    值班长的工具集是**清单驱动**的（[`FOREMAN_TOOL_SPECS`]，票 01 起 8 个只读工具），白名单在
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
use crate::agent::tools::{ToolCallContext, ToolExecutor};
use crate::config::Settings;
use crate::home::Home;
use crate::pipeline::proposals::StoreProposalSink;
use crate::process::RealProcessKiller;
use crate::sse::SseSink;
use crate::storage::foreman::{
    ForemanMessage, ForemanSession, NewForemanMessage, FOREMAN_ROLE_ASSISTANT,
};
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

/// 工具的一层（决策 188 / 206 的两段白名单在权限模式那一批细化）。
///
/// 本票（票 01）只有 `Read` 一层有内容；写工具由票 04 / 05 / 06 逐个加进来，而**每一层
/// 能不能自动放行**由决策 206 的档位管——档位表不在本模块，这里只标身份。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForemanToolLayer {
    /// A 层：只读（台账 + 环境读数）。**永不改动任何东西**，故不需要确认钮。
    Read,
    /// C / D / E 层：会改动东西（文件 / 本服务状态 / 环境）。`ask` 档下转成提议。
    Write,
}

/// 一个工具的完整规格：名字 + 层级 + 广告语 + 参数 schema。
///
/// **唯一事实源**：广告给模型的那一份（[`FOREMAN_TOOL_SPECS`] → `tool_defs()`）与执行点的
/// 白名单（`ToolExecutor::with_allowed_tools` 收的那一份）都从这里来。两处各写一份名字的后果是
/// 「模型看得见一个调用就被拒的工具」（或反过来，一个能调但没人告诉它的工具），
/// 两种都很难从现象定位——这正是票 01 要求「同源」的理由。
pub struct ForemanToolSpec {
    pub name: &'static str,
    pub layer: ForemanToolLayer,
    pub description: &'static str,
    /// 参数 JSON-Schema 的**文本**：常量表里放不了 `serde_json::Value`，
    /// 用文本 + 一处解析（`tool_defs()`），并由单测钉住它是合法 JSON。
    pub parameters: &'static str,
}

/// 值班长的工具清单（决策 182⑭ → 决策 188 / 207）。
///
/// 与 [`crate::pipeline::subagent::SUB_AGENT_TOOLS`] 同一姿态：这是**安全边界本身**，
/// 不是配置项。任何「给值班长加个工具」的改动都必须先改这里，从而在 diff 里显式可见。
///
/// A 层的六个新读数（票 01）**一律复用后端既有口径**，不新造一套：看板读任务表、
/// 指标走 `metrics::*` 纯函数、项目 / 阶段配置 / 技能 / provider 各读自己那张表的既有读法。
/// 唯一需要加工的是 provider：库里存的是**明文密钥**（决策 112），故只回显掩码。
pub const FOREMAN_TOOL_SPECS: [ForemanToolSpec; 20] = [
    ForemanToolSpec {
        name: "read_task",
        layer: ForemanToolLayer::Read,
        description: "读某个任务的台账详情：标题、状态、当前工位、待办原因原文、\
                      后端下发的可用动作、各分支游标。卡住的细节问它。",
        parameters: r#"{"type":"object","properties":{"task_id":{"type":"string","description":"任务 id（快照里方括号内那串）"}},"required":["task_id"]}"#,
    },
    ForemanToolSpec {
        name: "read_conversation",
        layer: ForemanToolLayer::Read,
        description: "读某个任务某次节点运行的会话回执（工位当时说了什么）。\
                      run_id 省略时取该任务最近一次会话。引用工位结论时要标出来源。",
        parameters: r#"{"type":"object","properties":{"task_id":{"type":"string","description":"任务 id"},"run_id":{"type":"integer","description":"运行 id，省略取最近一次"}},"required":["task_id"]}"#,
    },
    ForemanToolSpec {
        name: "read_board",
        layer: ForemanToolLayer::Read,
        description: "读整块看板：每个任务的状态与当前工位，以及按状态的分组计数。\
                      想知道「一共有多少活、都在哪一档」时问它。",
        parameters: r#"{"type":"object","properties":{}}"#,
    },
    ForemanToolSpec {
        name: "read_metrics",
        layer: ForemanToolLayer::Read,
        description: "读全局指标：任务数、成功率、validate 首过率、token 与调用总量、\
                      按阶段的聚合。这些数与指标页同源。",
        parameters: r#"{"type":"object","properties":{}}"#,
    },
    ForemanToolSpec {
        name: "read_projects",
        layer: ForemanToolLayer::Read,
        description: "读已接入的项目清单：名字、仓库路径、默认分支、语言与测试命令。",
        parameters: r#"{"type":"object","properties":{}}"#,
    },
    ForemanToolSpec {
        name: "read_stage_configs",
        layer: ForemanToolLayer::Read,
        description: "读各阶段的配置：provider、采样参数、人格文件、技能声明、超时。\
                      想解释「为什么这个工位表现是这样」时问它。",
        parameters: r#"{"type":"object","properties":{}}"#,
    },
    ForemanToolSpec {
        name: "read_skills",
        layer: ForemanToolLayer::Read,
        description: "读当前可用的技能清单（技能根下的 markdown）：名字、描述、\
                      被哪些阶段引用。",
        parameters: r#"{"type":"object","properties":{}}"#,
    },
    ForemanToolSpec {
        name: "read_providers",
        layer: ForemanToolLayer::Read,
        description: "读已配置的 provider 清单：id、厂商、模型、是否启用、上下文窗口。\
                      **密钥只回显掩码**——你看到的是「有没有配」，不是密钥本身。",
        parameters: r#"{"type":"object","properties":{}}"#,
    },
    // ── B 层：环境只读（决策 206）。域是家目录根，`data/` 与 `logs/` 按前缀拒——库里
    //    明文存着 provider 密钥（决策 112），而默认那份**模式**名单盖不住一个 `.db` 文件。
    //    这三件事在 `deny` 档下连广告都不给（`foreman_available_tools` 筛的），
    //    在 `ask` 档下照常直接执行（只读不需要人按键）。
    //
    //    `spawn_sub_agent` **不在列**：它要注入一个子代理运行器才有意义，而值班长手上
    //    没有（也不该有——它是面向人的对话者，不是流水线节点）。加一个只会回「未启用」的
    //    工具，等于让模型每轮都看得见一个它调了也没用的东西。
    ForemanToolSpec {
        name: "read_file",
        layer: ForemanToolLayer::Read,
        description: "读一个文件（路径相对家目录根）。看配置、看产物、看你提议要改的那个\
                      文件现在长什么样——**改之前先看**。",
        parameters: r#"{"type":"object","properties":{"path":{"type":"string","description":"相对家目录根的路径"},"offset":{"type":"integer","description":"起始行（从 0 数）"},"limit":{"type":"integer","description":"最多读几行"}},"required":["path"]}"#,
    },
    ForemanToolSpec {
        name: "list_dir",
        layer: ForemanToolLayer::Read,
        description: "列一个目录（路径相对家目录根，省略取根）。不知道东西在哪儿时先列一层。",
        parameters: r#"{"type":"object","properties":{"path":{"type":"string","description":"相对家目录根的路径"},"recursive":{"type":"boolean","description":"是否递归"}}}"#,
    },
    ForemanToolSpec {
        name: "Skill",
        layer: ForemanToolLayer::Read,
        description: "按技能名取它的正文（技能目录下的 markdown）。有人问「某个技能到底干什么」\
                      时用它，不要凭名字猜。",
        parameters: r#"{"type":"object","properties":{"name":{"type":"string","description":"技能名（技能目录里列出的那个）"}},"required":["name"]}"#,
    },
    // 诊断包（决策 211③ / 票 03）：一族一个工具，一次调用给出定因所需的全部证据。
    // 它**不扩 `read_task`**——后者是每轮值守都会调的高频、便宜读数，混进来会让
    // 「看一眼任务状态」开始烧 12k 字符。排在只读层末尾：往这份清单里加东西，
    // diff 里永远是末尾多一段（`foreman_tool_names` 的顺序被冻结断言逐条钉住）。
    ForemanToolSpec {
        name: "read_diagnosis",
        layer: ForemanToolLayer::Read,
        description:
            "读某个任务的**诊断包**：一次拿到定因所需的全部证据——每条 run 的状态 / 耗时 / \
                      token / error / 是否挂过进程组、命令台账与闸门输出路径、阶段产出与验收标准、\
                      待办原因原文、组装后的 prompt 原文、失败 run 的最后几条工具往来。\
                      「它为什么卡住 / 为什么失败 / 是不是 prompt 问题」这类问题问它；\
                      只看「现在什么状态」用 read_task（便宜得多）。",
        parameters: r#"{"type":"object","properties":{"task_id":{"type":"string","description":"任务 id（快照里方括号内那串）"},"runs":{"type":"integer","description":"最多带回多少条 run（默认 30，最近的在前）"}},"required":["task_id"]}"#,
    },
    // ── C 层：环境写（决策 206 / 207）。在 `ask` 档下**不执行**，生成提议等人按键；
    //    `auto` 直通；`deny` 连广告都不给。域与 B 层同一份（家目录根 + `data` / `logs` 前缀 deny）。
    ForemanToolSpec {
        name: "write_file",
        layer: ForemanToolLayer::Write,
        description: "写一个文件（整份覆盖，路径相对家目录根）。**改之前先 read_file 看一眼**\
                      ——覆盖是不可逆的，你不知道原来有什么就会把有用的东西抹掉。\
                      这条动作要值班经理按键确认。",
        parameters: r#"{"type":"object","properties":{"path":{"type":"string","description":"相对家目录根的路径"},"content":{"type":"string","description":"文件的全部内容（整份覆盖）"}},"required":["path","content"]}"#,
    },
    ForemanToolSpec {
        name: "edit_file",
        layer: ForemanToolLayer::Write,
        description: "改一个文件里的一处（把 old_text 换成 new_text，路径相对家目录根）。\
                      只想动一小段时用它，不要整份重写——整份重写会把文件里其它地方没读到的内容\
                      一起抹掉。改之前先 read_file。这条动作要值班经理按键确认。",
        parameters: r#"{"type":"object","properties":{"path":{"type":"string","description":"相对家目录根的路径"},"old_text":{"type":"string","description":"要被替换的原文（须在文件中出现，且只替换第一处）"},"new_text":{"type":"string","description":"替换成什么"}},"required":["path","old_text","new_text"]}"#,
    },
    ForemanToolSpec {
        name: "run_command",
        layer: ForemanToolLayer::Write,
        description: "在这台机器上跑一条 shell 命令（默认工作目录是家目录根）。\
                      命令的输出会作为工具回执回来，也会落进这个班次的命令台账。\
                      这条动作会不会立即执行由权限档位决定（`ask` 档要值班经理按键确认）。",
        parameters: r#"{"type":"object","properties":{"command":{"type":"string","description":"要执行的命令原文"},"cwd":{"type":"string","description":"工作目录（默认家目录根）"},"timeout_sec":{"type":"integer","description":"超时秒数"}},"required":["command"]}"#,
    },
    // ── 修复轮（决策 210③④ / 票 10–12）：**它的载体是环境**——在项目仓上拉一个 worktree、
    //    跑闸门、落一个带标记的 commit，故它归 C 层由档位管（`ask` 下每步要按键、`auto`
    //    下整轮自己跑完），而**合入永远人按**。它没有进托管自动集，也没有进 D 层：
    //    `finish` 的产物本身就是一条提议，放 D 层会变成两层按不完的钮。
    ForemanToolSpec {
        name: "repair",
        layer: ForemanToolLayer::Write,
        description:
            "起草一份补丁（三步走，**改动在你按下合入之前不进主干**）。\
                      `start`：给某个项目拉一个独立的修复 worktree，回执里有可写目录与 \
                      repair_id——**补丁只能写在那个目录里**（项目工作区在文件域之外）；\
                      `finish`：跑闸门（lint + 测试）→ 过了才单独成一个带标记的 commit → \
                      出 diff → 落一条等你按合入的提议（没过就什么都不出，回执里说得清是哪一步）；\
                      `discard`：不修了，回收 worktree（分支留着当证据）。\
                      顺序是用法的一部分：先 start、写完再 finish，finish 时给一句 conclusion\
                      （它进 commit message）。绝不说「我已经修好了」——你只是提了一条等人按的提议。",
        parameters: r#"{"type":"object","properties":{"action":{"type":"string","enum":["start","finish","discard"],"description":"要做的动作"},"project_id":{"type":"string","description":"修哪个项目（read_projects 里有 id）"},"repair_id":{"type":"string","description":"finish / discard：start 回执里那个 id"},"conclusion":{"type":"string","description":"finish：一句话诊断结论（进 commit message）"},"task_id":{"type":"string","description":"finish：若这次修复是为某个任务做的，填它的 id——那条任务上会留下「等修复合入」"}},"required":["action","project_id"]}"#,
    },
    // ── D 层：本服务的写接口（决策 206 / 207）。**一族一个工具 + 动作参数**：粒度对着
    //    `allowed_actions` 的类型走。这一层**不读档位**——「本服务自己的写接口需要人按
    //    确认钮」不随权限档位变（把值班长配成 `auto` 只放开环境层）。
    //
    //    三个**排除项**（决策 207⑤，故意没有对应工具）：重置配对令牌、局域网开关、
    //    仓名单增删。判据是「改的是**谁能访问这台机器**」——让模型能提议它们，等于让它能
    //    给自己开门。本仓对此有专门的用例断言清单里不含它们（`tests/foreman.rs`）。
    // 全局动作（票 09）：`action=restart`——**永远只提议**（会打断所有在跑的任务），
    // 既不受档位影响，也不在托管自动集里。
    ForemanToolSpec {
        name: "service",
        layer: ForemanToolLayer::Write,
        description: "本服务自己的运维动作。action 取值：`restart`（重启服务）。\
                      它是**全局**动作：会打断所有在跑的任务（本服务没有自重启能力，\
                      按下后先做恢复序列——清占用 + 把中断的 running 任务归队——\
                      再告诉你需要在你启动它的地方重启一次）。永远要值班经理按键确认。",
        parameters: r#"{"type":"object","properties":{"action":{"type":"string","enum":["restart"],"description":"要做的动作"}},"required":["action"]}"#,
    },
    ForemanToolSpec {
        name: "task",
        layer: ForemanToolLayer::Write,
        description: "对一个流水线任务动作。action 取值：\
                      `create`（建任务，要 project_id 与 title）、\
                      `resume`（让人拍过板的 pending 继续走，要 task_id 与 resume_action）、\
                      `retry`（重跑一个终态任务）、\
                      `cancel`（取消）、\
                      `review`（人工评审通过/打回，要 approved）、\
                      `merge`（合入决定，要 decision=approve 或 reject）、\
                      `unstick`（解除僵死占用：run 已终态而游标仍 active / 有主但心跳停了——\
                      清执行者、标终态、游标转 pending；**只对真卡住的任务生效**）。\
                      参数与界面上那个按钮点下去时发的一模一样——先 read_task 看清它现在卡在\
                      哪个 pending、允许的动作是什么，再决定 action 与 resume_action。\
                      每条都要值班经理按键确认。",
        parameters: r#"{"type":"object","properties":{"action":{"type":"string","enum":["create","resume","retry","cancel","review","merge","unstick"],"description":"要做的动作"},"task_id":{"type":"string","description":"目标任务（create 之外的 action 都要）"},"project_id":{"type":"string","description":"create：建在哪个项目下"},"title":{"type":"string","description":"create：任务标题"},"description":{"type":"string","description":"create：任务描述"},"depends_on":{"type":"array","items":{"type":"string"},"description":"create：依赖的任务 id"},"review_mode":{"type":"string","enum":["agent","human"],"description":"create：评审模式"},"cursor_id":{"type":"string","description":"resume：指定游标（多条活跃游标时必填）"},"resume_action":{"type":"string","description":"resume：拍板的动作名（read_task 的 allowed_actions 里那几个）"},"target_stage":{"type":"string","description":"resume：跳到哪个阶段"},"target_node":{"type":"string","description":"resume：跳到哪个节点"},"input":{"type":"string","description":"resume：给这次拍板的说明 / 打回意见"},"approved":{"type":"boolean","description":"review：通过还是打回"},"comments":{"type":"string","description":"review：打回时带给下游的意见"},"decision":{"type":"string","enum":["approve","reject"],"description":"merge：合入还是打回"}},"required":["action"]}"#,
    },
    ForemanToolSpec {
        name: "config",
        layer: ForemanToolLayer::Write,
        description: "改流水线的阶段配置。action 取值：`set`（整条替换某个阶段的配置，要 stage；\
                      留空的字段会被清成默认——这是整条替换不是局部修改）、\
                      `delete`（删掉这个阶段的覆盖行，回到系统默认，要 stage）。\
                      要值班经理按键确认。",
        parameters: r#"{"type":"object","properties":{"action":{"type":"string","enum":["set","delete"],"description":"set 或 delete"},"stage":{"type":"string","description":"阶段键（如 develop / review / foreman）"},"provider_id":{"type":"string","description":"set：用哪个 provider"},"temperature":{"type":"number","description":"set：采样温度"},"max_tokens":{"type":"integer","description":"set：输出上限"},"persona_path":{"type":"string","description":"set：人格文件路径"},"persona_append":{"type":"string","description":"set：追加指令"},"env_mode":{"type":"string","enum":["auto","ask","deny"],"description":"set：环境层权限档位"},"tools_json":{"description":"set：工具声明（与界面那个框同形）"},"skills_json":{"description":"set：技能声明（与界面那个框同形）"},"node_overrides_json":{"description":"set：节点级覆盖（与界面那个框同形）"},"idle_timeout_sec":{"type":"integer","description":"set：空闲超时"},"max_duration_sec":{"type":"integer","description":"set：最长时长"}},"required":["action","stage"]}"#,
    },
    ForemanToolSpec {
        name: "skills",
        layer: ForemanToolLayer::Write,
        description: "装 / 卸技能。action 取值：`install`（从一个本地技能目录导入，要 path，\
                      该目录自身含 SKILL.md）、`delete`（卸载一个已装技能，要 name）。\
                      卸载不检查引用——仍被阶段配置引用的技能卸掉之后，那个阶段解析会报错。\
                      要值班经理按键确认。",
        parameters: r#"{"type":"object","properties":{"action":{"type":"string","enum":["install","delete"],"description":"install 或 delete"},"path":{"type":"string","description":"install：技能目录路径"},"name":{"type":"string","description":"delete：技能名"},"overwrite":{"type":"boolean","description":"install：同名时是否覆盖"}},"required":["action"]}"#,
    },
];

/// 清单里某一层的工具名（票 01 起有 `Read`，票 04 / 05 / 06 往上加 `Write`）。
pub fn foreman_tool_names(layer: ForemanToolLayer) -> Vec<&'static str> {
    FOREMAN_TOOL_SPECS
        .iter()
        .filter(|s| s.layer == layer)
        .map(|s| s.name)
        .collect()
}

/// 值班长此刻**真正拿得到**的工具名（决策 206 的 `deny` 档：连广告都不给）。
///
/// 广告集（[`ForemanRunner::tool_defs`]）与执行点白名单
/// （[`ToolExecutor::with_allowed_tools`]）都从这里来——**同源**是票 01 的硬要求，
/// 而档位是筛在这个源头上的一道，不是两处各筛一次。
///
/// 只读台账工具**不受档位影响**（它们不在 [`crate::agent::tools::ENV_TOOLS`] 里）：
/// `deny` 收的是「能碰机器」的手，不是「能读台账」的眼。
pub fn foreman_available_tools(mode: crate::types::EnvMode) -> Vec<&'static str> {
    foreman_available_tools_except(mode, &[])
}

/// 同 [`foreman_available_tools`]，再摘掉一层（分级诊断，票 07）。
///
/// 广告集与执行点白名单**仍然同源**：两处都从这里来，故「模型看得见一个调用就被拒的工具」
/// 这件事在自动轮里也不会发生——分级诊断最容易犯的错就是把工具只从执行点摘掉。
pub fn foreman_available_tools_except(
    mode: crate::types::EnvMode,
    deny: &[&str],
) -> Vec<&'static str> {
    FOREMAN_TOOL_SPECS
        .iter()
        .filter(|s| !crate::agent::tools::denied_by_tier(s.name, mode))
        .filter(|s| !deny.contains(&s.name))
        .map(|s| s.name)
        .collect()
}

/// 值班长的**调用上下文**：不挂任务、只挂会话，域是家目录根。
///
/// 与执行器一起从 [`foreman_tooling`] 出来，不单独暴露——两处各拼一份的后果是
/// 「提议时的域」与「执行时的域」可以不同（提议存的是参数，域是执行时才拼的，
/// 漂移会静默发生）。
fn foreman_ctx(home: &Home, session_id: &str) -> ToolCallContext {
    ToolCallContext {
        // 值班长不挂任务：任务 id 来自工具参数，不是上下文。
        task_id: String::new(),
        // 命令 / 文件动作的归属走会话（迁移 0012 的 CHECK：恰好一个归属）。
        session_id: Some(session_id.to_string()),
        stage: Stage::Init,
        node: Node::Execute,
        // 域 = 家目录根（决策 206 / 207）：流水线阶段仍限任务工作区，那一条不动。
        worktree_path: PathBuf::from(home.root()),
        task_dir: PathBuf::from(home.root()),
        run_id: None,
        command_source: CommandSource::Agent,
        default_cwd: None,
    }
}

/// 值班长执行器的**时刻**（决策 207）——同一个构造，两种走向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForemanMoment {
    /// 对话里那一轮：模型说它想做什么，写动作在此**落成提议**。
    Conversation,
    /// 人按下确认钮那一刻：闸门已经走过一次，这一趟是执行。
    ConfirmedPress,
}

/// 值班长的工具执行器**与调用上下文**——对话轮与按键执行共用这一处构造。
///
/// `env_mode` 决定两件事：白名单（`deny` 档把环境层整个摘掉）与第三道闸的走向。
///
/// **按键执行为什么能关掉第三道闸**（决策 207）：提议生成时已经走过一次闸——`ask` 档下
/// 的那次调用正是被它拦下来变成了提议。执行时再拦一次就会自己吃掉自己（按钮按下去又生成
/// 一条新提议）。关的是「要不要问人」，不是「准不准」：白名单照旧按**当前**档位判，
/// 于是档位在提议之后被收紧到 `deny` 时，那条提议按不下去。
#[allow(clippy::too_many_arguments)]
pub fn foreman_tooling(
    store: &Store,
    settings: &Settings,
    home: &Home,
    sse: Arc<dyn SseSink>,
    session_id: &str,
    env_mode: crate::types::EnvMode,
    moment: ForemanMoment,
    // 分级诊断（票 07）：这一轮再摘掉哪些工具。空 = 不摘（人的那一轮）。
    deny: &[&str],
    // 托管动作的执行者（票 08）。`None` = 不放行：D 层照旧恒提议。
    steward: Option<Arc<dyn crate::agent::tools::StewardActionRunner>>,
) -> (ToolExecutor, ToolCallContext) {
    // 值班长的域就是家目录根（决策 207 的「分两组」：流水线阶段仍限任务工作区），
    // 并按路径前缀拒掉 `data/` 与 `logs/`——库里明文存着 provider 密钥（决策 112），
    // 而默认那份**模式**名单（`.env*` / `*.pem` / …）盖不住一个 `.db` 文件。
    let tools = ToolExecutor::new(
        home.clone(),
        crate::agent::file_policy::foreman_file_policy(home.root()),
        settings.clone(),
        Arc::new(RealProcessKiller),
    )
    .with_ledger(store.clone())
    // 命令记录（§12.4.4）：值班长的命令要落 `kanban_node_commands`，`task_id` 为 NULL、
    // 归属走会话（迁移 0012）。它同时是**出口策略拒绝**的落点（决策 179）——被拒的命令
    // 也要留一行，否则策略在审计面完全不可见，只剩模型侧的一次报错。
    .with_recorder(Arc::new(store.clone()))
    .with_env_mode(env_mode)
    .with_allowed_tools(foreman_available_tools_except(env_mode, deny));
    let tools = match steward {
        Some(runner) => tools.with_steward_actions(runner),
        None => tools,
    };
    let tools = match moment {
        // 提议接缝只在对话轮注入。按键执行那一次若还带着它，`ask` 档会把执行改成再提一条。
        ForemanMoment::ConfirmedPress => tools.confirmed_once(),
        ForemanMoment::Conversation => {
            tools.with_proposal_sink(Arc::new(StoreProposalSink::new(store.clone(), sse)))
        }
    };
    (tools, foreman_ctx(home, session_id))
}

/// 操作台记的那几轮在**模型看到的对话**里的标记（决策 207）。
///
/// 两种立场、三个角色：`user` 与 `assistant` 是「谁在说话」，而**提议的执行结果是操作台记的账**
/// ——它既不是值班经理说的，也不是值班长做的。转写成 `user` 必须带这个标记，否则模型会把这句
/// 当成人对它下的指令；不带标记也不转写（整段丢掉）则会让它以为提议还挂着、于是重提一遍。
pub const OPERATION_LOG_MARK: &str = "【操作台】";

/// 主动播报的标记（决策 209④ / 票 06）。
///
/// 播报落进班次时**由后端加上**，不由模型自己说：它是「这句话不是回话、是值守轮说的」
/// 这个事实的载体，而前端要靠它区分两种轮，模型不该有机会说错。
pub const FOREMAN_WATCH_MARK: &str = "【值守播报】";

/// 值守轮的**静默哨兵**（决策 209④ / 票 06）：诊断结论是「无需处理」时，让模型只回这一行。
///
/// 为什么敢让模型回哨兵：判据是「明确说了无需处理」才静默，任何别的输出照常播报——
/// 漏报一条真问题比多播一条便宜（§2.4 的静默规则是为**历史窗口预算**设的，
/// 不是为省 token 设的）。
pub const FOREMAN_NO_ACTION_MARK: &str = "【无需处理】";

/// 分级诊断（决策 209⑥ / 票 07）：**自动那一轮**不许用的工具。
///
/// 判据是「这一次你不在场」——主动播报是「固定成本 × 时间」，而你不在场时没有任何收益
/// 能摊平它。贵的两件事各一：
/// - `read_conversation`：一次最多 12k 字符的会话原文；
/// - `run_command`：在本机上跑命令。
///
/// 两者在**被追问时**照常可用（那是人的那一轮，`say` 走的是完整工具集）——所以这不是
/// 削减能力，是**分级**：自动轮只读台账与诊断包摘要（`read_diagnosis` 自带 12k 上限）。
pub const FOREMAN_WATCH_TOOL_DENY: [&str; 2] = ["read_conversation", "run_command"];

/// 一次值守轮最多把多少条待办喂进简报。
///
/// 与 `FOREMAN_HISTORY_BUDGET_CHARS` 同一姿态：真正的账是字符，这个数只是「别把一夜的
/// 事件都塞进一次简报」的粗兜底。超出的部分留在表里，下一轮（或被追问时）再处理。
const FOREMAN_ATTENTION_FETCH_LIMIT: usize = 50;

/// 单次回话的最大工具往返轮数（决策 182④，**2026-09-18 由决策 224 从 8 改为 30**）。
///
/// 正常靠「模型不再发起 tool_call」自然结束；这个上限是防御性的——模型若陷入
/// 「查一个任务 → 再查一个」的循环，必须有人喊停，否则会持续烧 token 直到 HTTP 超时。
///
/// **8 太小**：一轮正常的定位本来就要十几次工具调用——两轮**跑成功**的回话在痕迹里
/// 分别留下 16 / 17 条（09-18 14:59 那条与 09-17 15:20 那条），而**同一班次**
/// `01M2QZCNN4CC65SSBQS1FJG402` 里 14:54 与 15:47 那两轮就是撞在这个数上报
/// `model_no_reply` 的：不是模型失控，是这个上限比一次正常轮还短。
/// 取值参照同类 agent 运行时的口径：zcode 的子代理默认 4 轮，但它**面向人的主会话没有
/// 轮数上限**，专家工作流的 react 循环默认 30——值班长属于前者的反面（面向人、被任意
/// 提问），故取 30 而不是照抄「查得动就行」的小数。
///
/// **它不管墙钟**：一轮跑多久由 [`Self::respond`] 的 timeout 兜底（决策 223②，默认
/// 1800s）。这个数只管「很快地空转」那一种病——**一轮里能跑几次模型调用**，与
/// 「一轮能跑多久」是两个正交的界，各自守各自的。
pub const FOREMAN_MAX_ROUNDS: usize = 30;

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
     你手上有一只读的手和一只写的手。写的那只手**先提建议、不直接动**：\
     改状态、改文件、跑命令都是如此，除非这个阶段的权限档位被配成了自动——两种情况\
     下面「工具纪律」那一段都会说清，以它为准。\
     因此绝不要声称你已经改了任何东西：你能说的是「我提了一条建议，等你按键」。\
     态势快照之外的细节用 read_task / read_conversation 自己查，不要凭印象猜。\
     你有时会**自己醒过来说话**（值守轮）：那一段不是回话，是你按事件主动播报——\
     语气照旧（只报事实与建议、不寒暄、短句优先），并说清是哪件事把你叫醒的。\
     你也能**起草补丁**（`repair` 工具，三步）：先 `start` 拿一个独立的修复 worktree，\
     在**那个目录里**改代码（项目工作区你写不进去），改完 `finish`——它跑闸门（lint + 测试），\
     过了才单独成一个带标记的 commit、出一份 diff、落一条等人按合入的提议；没过就什么都不出，\
     回执会说清是哪一步没过。你的改动在值班经理按下之前**没有进主干**——所以只说\
     「我改了什么、为什么、闸门过没过」，绝不说「我已经修好了」；它也**不会**被自动合入。";

/// 值班长的前言。**不复用** [`crate::agent::prompts::build_system_prompt`]。
///
/// 那条路会强制拼上 `BASELINE_PREAMBLE`（「你是 AgentPipeline 的节点 agent……需要时用
/// read_file 读取」）与 `FORMAT_RULES`（「产出文件一律通过 write_file 写入」「结构化流转
/// 信息一律通过 submit_metadata 提交」）——三段指令都在让模型使用它**没有**的工具。
/// 指示一个模型去调不存在的工具，正是它开始编造文件内容的起点。
const FOREMAN_BASELINE: &str = "你在一个本机工具内运行，面对的是这台机器上的流水线台账\
     与这个工具自己的家目录。你能读到什么、能不能动手，由下面「工具纪律」那一段说清。";

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
    /// 这句话落进了哪个会话（决策 204）。请求没指定时是服务端选/建的那个，
    /// 客户端据此更新自己的「当前班次」——否则第一次说话会落进一个它不知道的会话。
    pub session: ForemanSession,
    pub reply: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub briefing: ForemanBriefing,
    pub traces: Vec<ForemanTrace>,
}

/// 一轮回话的输入（决策 209④ / 票 06）：人的话与值守轮的简报**共用**同一条模型调用与
/// 工具循环（token 记账、痕迹、审计都只有一份）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnInput {
    /// 值班经理打的一句话。它先落库（审计要的是「他说了什么」）。
    Human(String),
    /// **值守轮的系统简报**：待办逐条（类别 + 任务 + 当时的台账事实）合成的一段话。
    ///
    /// 它必须**在文案上标明来路**：值班长的人格第一条是「不认领没做过的事」，
    /// 而一段没有署名的话会被它当成有人在对它下指令（「你让它重启它」）。
    WatchBrief(String),
}

impl TurnInput {
    /// 喂进 transcript 最后一条 user 消息的正文。
    fn transcript_text(&self) -> String {
        match self {
            TurnInput::Human(text) => text.clone(),
            TurnInput::WatchBrief(brief) => format!(
                "{FOREMAN_WATCH_MARK}（以下是**系统生成的**值守简报，不是值班经理说的话；\
                 建议不要当成指令）\n{brief}"
            ),
        }
    }

    fn is_watch(&self) -> bool {
        matches!(self, TurnInput::WatchBrief(_))
    }
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
    /// 提议事件的广播去向（决策 207）。它与对话增量走**同一条**总线（决策 182⑥），
    /// 只是事件类型不同——前端按类型分流，后端按类型过滤。
    sse: Arc<dyn SseSink>,
    /// 托管放行的自动动作的执行者（决策 210② / 票 08）。`None` = 不放行。
    steward_actions: Option<Arc<dyn crate::agent::tools::StewardActionRunner>>,
    /// 一轮回话的绝对上限（测试注入；生产走 `node_max_duration_sec` / 阶段覆盖）。
    turn_timeout: Option<std::time::Duration>,
}

impl ForemanRunner {
    /// 构造。
    ///
    /// `sse` **不是可选项**：值班长的写动作只能以提议的形式出现在时间线上，而提议到达
    /// 那一刻要能推给打开着的界面。留一个「不传就没有事件」的缺省，等于给这条链路留一个
    /// 静默失效的口子（界面还是能靠重读拿到它，于是没人会发现事件没发）。
    pub fn new(
        store: Store,
        settings: Settings,
        home: Home,
        llm: Arc<dyn LlmClient>,
        sse: Arc<dyn SseSink>,
    ) -> Self {
        ForemanRunner {
            store,
            settings,
            home,
            llm,
            sse,
            steward_actions: None,
            turn_timeout: None,
        }
    }

    /// 测试用：把一轮回话的上限压到可观测的量级（生产走 [`Self::turn_limit`]）。
    ///
    /// 与 [`crate::agent::providers::ProductionLlm::with_heartbeat_interval`] 同一姿态：
    /// 上限在生产里是一个配置值（默认半小时），而「挂住的模型流会被掐断并留账」这件事
    /// 不可能靠真等半小时来验。
    pub fn with_turn_timeout(mut self, limit: std::time::Duration) -> Self {
        self.turn_timeout = Some(limit);
        self
    }

    /// 注入托管动作的执行者（决策 210② / 票 08）。
    ///
    /// 由 **app 层**注入，不在 core 里实现：resume 的唯一实现在
    /// [`crate::pipeline::resume::apply_resume`]，而它的另一个调用者是 HTTP 端点
    /// （`POST /tasks/{id}/resume`）——两处必须逐字同源，故执行者由知道端点的那一层提供。
    /// 不注入 = 不放行（D 层照旧恒提议）。
    pub fn with_steward_actions(
        mut self,
        runner: Arc<dyn crate::agent::tools::StewardActionRunner>,
    ) -> Self {
        self.steward_actions = Some(runner);
        self
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// 回一句话，落进指定的会话。
    ///
    /// 顺序是刻意的：**值班经理说的话先落库**，再叫模型，最后落值班长的回话。
    /// 中间任何一步失败，人说过的那句话仍在台账里（审计要的是「他说了什么」，
    /// 不是「他说的哪句成功被答复」）。
    ///
    /// `session_id` 为 `None` 时落到最近活动的未归档会话；一个都没有就新开一个
    /// （首启空 home 的第一次说话走这条）。**历史窗口只取该会话**（决策 204②：
    /// 会话隔离的是上下文）——换会话就是换一段上下文，台账本身照旧全局可查。
    ///
    /// **不落 run 行**：`kanban_node_runs` 的归属约束与指标口径都不接受一个无阶段、
    /// 无任务的运行行（决策 182⑨）。没有 run 行也就没有心跳可打——值班长的活性信号
    /// 是 HTTP 请求本身（决策 182⑦）。
    pub async fn say(&self, session_id: Option<&str>, user_text: &str) -> Result<ForemanTurn> {
        let text = user_text.trim();
        if text.is_empty() {
            return Err(Error::Validation("空消息不入账".into()));
        }
        let session = self.resolve_session(session_id).await?;
        self.store
            .append_foreman_user_message(&session.id, text)
            .await?;
        // 标题可能是刚由这句话派生的（首次说话命名，决策 204②），故重读一次会话行，
        // 让返回值里的标题与库里的标题是同一个——客户端拿它直接更新 chip。
        let session = self
            .store
            .get_foreman_session(&session.id)
            .await?
            .unwrap_or(session);

        // 失败也要留痕（决策 211④ / 票 04）：`?` 会把这一轮的现场一起带走，库里只剩
        // 一条孤立的 user 行——2026-09-17 实测里值班长两次没回话，「为什么没回话」
        // 一个字都查不到。故外框兜住所有出口，失败时落一条 `system` 账。
        let result = self
            .respond(&session, TurnInput::Human(text.to_string()))
            .await;
        if let Err(error) = &result {
            self.record_failed_turn(&session, error).await;
        }
        result
    }

    /// 一轮回话的**外框**：给这一轮加上时限，让它有界。
    ///
    /// **为什么必须有界**（2026-09-18 实测）：对讲台这一轮没有 run 行，也就没有心跳可打
    /// （决策 182⑨），于是它此前**完全不受任何时限约束**——模型流一挂住，`say()` 就一直
    /// 挂在 `stream.next()` 上。那一晚两条消息都是这个形状：`Err` 到不了，失败外框也
    /// 就不执行，库里只剩孤立的用户行。有界之后，「挂住」变成一条**可归因的失败**——
    /// 与流水线节点同一个口径（见 [`Self::turn_limit`]）。
    async fn respond(&self, session: &ForemanSession, input: TurnInput) -> Result<ForemanTurn> {
        let cfg = self.stage_config().await?;
        let limit = self.turn_limit(cfg.as_ref());
        match tokio::time::timeout(limit, self.respond_inner(session, input, cfg)).await {
            Ok(inner) => inner,
            Err(_) => Err(Error::Llm(format!(
                "这一轮超过 {} 没有结束（模型或网络挂住）：已中止，重发一次通常能过去",
                human_duration(limit)
            ))),
        }
    }

    /// 一轮回话的时限：**与流水线节点取同一个数**（决策 66 的四级解析）。
    ///
    /// 节点级覆盖 > 阶段配置（`[foreman]` 那行的 `max_duration_sec`）> 全局
    /// `node_max_duration_sec`（默认 1800s = 半小时）。取同一个数的理由是：对讲台这一轮
    /// 与一个节点在「一次有界的执行」这件事上是同一种东西，各写一个数字只会让两个地方
    /// 各自漂移。
    ///
    /// 取值**宁可宽**：它的职责是让挂死有界，不是让长轮次失败——一轮里可以有多次模型
    /// 调用（`FOREMAN_MAX_ROUNDS` 次上限），每次实测在分钟量级。
    fn turn_limit(&self, cfg: Option<&crate::types::StageConfig>) -> std::time::Duration {
        if let Some(injected) = self.turn_timeout {
            return injected;
        }
        let secs = crate::config::effective_max_duration(
            self.settings.node_max_duration_sec,
            cfg.and_then(|c| c.max_duration_sec),
            crate::config::NodeTimeouts::default(),
        );
        std::time::Duration::from_secs(secs)
    }

    /// 一轮回话的**内里**（现场由 [`Self::say`] / [`Self::watch`] 的外框记账，
    /// 时限由 [`Self::respond`] 给）。
    async fn respond_inner(
        &self,
        session: &ForemanSession,
        input: TurnInput,
        cfg: Option<crate::types::StageConfig>,
    ) -> Result<ForemanTurn> {
        let briefing = build_briefing(&self.store).await?;
        // 阶段配置由外框读了一次传进来（人格 + provider / 采样参数共用那一份）。
        // 环境层档位（决策 206）：广告集、执行点白名单与人格里的工具纪律段**同源**，
        // 都由它筛一次。缺省 `ask`（值班长的输入是人可以随便打的任意文本）。
        let env_mode = crate::types::effective_env_mode(
            self.settings.env_mode,
            FOREMAN_STAGE_KEY,
            cfg.as_ref(),
        );
        // 有没有任务在被托管（决策 210① / 票 08）：只在真有时才在人格里说那一段——
        // 一段笼统的「你可以直接动手」会立刻变成一句假话（别的任务上它照样只能提）。
        let stewarded = self
            .store
            .list_tasks(&crate::storage::tasks::TaskFilter {
                include_archived: false,
                ..Default::default()
            })
            .await?
            .iter()
            .any(|t| t.stewardship.as_ref().is_some_and(|s| s.enabled));
        let system_prompt = self.system_prompt(cfg.as_ref(), env_mode, stewarded)?;
        let provider_id =
            crate::storage::catalog::resolve_provider_id(None, None, cfg.as_ref(), None);

        let history = self
            .store
            .list_foreman_messages(&session.id, FOREMAN_HISTORY_FETCH_LIMIT)
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

        // 分级诊断（票 07）：自动那一轮摘掉贵的两件（会话原文 / 跑命令）。摘在**源头上**
        // ——广告集与执行点白名单都从这一份名单来，故「模型看得见一个调用就被拒的工具」
        // 这件事在自动轮里同样不会发生。
        let deny: &[&str] = if input.is_watch() {
            &FOREMAN_WATCH_TOOL_DENY
        } else {
            &[]
        };
        let (tools, ctx) = foreman_tooling(
            &self.store,
            &self.settings,
            &self.home,
            self.sse.clone(),
            &session.id,
            env_mode,
            ForemanMoment::Conversation,
            deny,
            self.steward_actions.clone(),
        );

        // 三种角色 → 两种说话的立场（决策 204 / 207）。**操作台记的那几轮（`system`）必须与
        // 值班长自己的话分开**：写成助理轮，它下一轮读历史时会把「提议已执行：写文件 notes.md」
        // 当成自己说过的话——那正是人格第一条纪律（不得声称自己动了手）要挡的东西。
        //
        // 转写成 `user` 而不是丢掉：丢掉它，模型就不知道人按了什么键，会以为提议还挂着
        // （于是重提一遍）。剩下的问题是「user 这一侧还有值班经理」——故加一句与前缀一起
        // 说明发言者是谁，而不是靠角色去暗示。
        let mut transcript: Vec<Message> = window
            .iter()
            .map(|m| {
                if m.role == crate::storage::foreman::FOREMAN_ROLE_USER {
                    Message::user(m.content.clone())
                } else if m.role == crate::storage::foreman::FOREMAN_ROLE_SYSTEM {
                    Message::user(format!(
                        "{}（操作台记的一轮）\n{}",
                        OPERATION_LOG_MARK, m.content
                    ))
                } else {
                    Message::assistant(Some(m.content.clone()), Vec::new())
                }
            })
            .collect();
        // 历史最后一条就是刚落库的这句 user 消息；但若它被 `trim_history` 之外的原因
        // 漏掉（例如库被外部清空），仍要保证本轮的问题在场。
        // 值守简报**总是**追加成最后一条：它带署名（`TurnInput::transcript_text`），
        // 而历史里最后一条也是 user（刚落的用户行）时不能靠「已经有了」跳过它——
        // 那会让这一轮真正要处理的东西消失。人的话反过来：历史里最后一条就是它。
        match (&input, transcript.last()) {
            (TurnInput::Human(text), Some(m)) if m.role == crate::agent::client::Role::User => {
                let _ = text;
            }
            _ => transcript.push(Message::user(input.transcript_text())),
        }

        let tool_defs = Self::tool_defs(env_mode, deny);
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
                tools: tool_defs.clone(),
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
                    // 会话身份（决策 204⑥）：手机与电脑同时连着时，前端靠它把增量
                    // 归到正确的会话，而不是把两台设备的回话混成一段。
                    session_id: session.id.clone(),
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
                // 归因**走 `LlmClassified` 的 kind 机制**而不是新造一种错误（票 04）：
                // 这两条是模型行为，不是内部故障，而「哪一类」正是排查要的入口。
                Error::LlmClassified {
                    kind: "model_empty_reply".into(),
                    message: "值班长这一轮没有回话（模型返回了空内容）。\
                              若这是本机第一次使用，先确认 provider 与模型名配对了；\
                              也可以换个模型再试——有些模型在被要求用工具时会返回空内容。"
                        .into(),
                    raw: "模型返回空内容（无 tool_calls、无文本）".into(),
                }
            } else {
                Error::LlmClassified {
                    kind: "model_no_reply".into(),
                    message: format!(
                        "值班长在 {FOREMAN_MAX_ROUNDS} 轮内没有给出回话——它可能一直在查台账"
                    ),
                    raw: format!("达到 FOREMAN_MAX_ROUNDS = {FOREMAN_MAX_ROUNDS} 仍未收口"),
                }
            }
        })?;

        let traces_json = if traces.is_empty() {
            None
        } else {
            Some(serde_json::to_value(&traces)?)
        };
        // 静默规则（决策 209④ / §2.4）：值守轮判定「无需处理」时不落**播报**——
        // 一次自愈的风吹草动不该变成一条消息，而消息本身会挤占历史窗口预算（24k 字符）。
        // 痕迹留在日志里；台账那一栏的「我处理过没有」由待办表的 `consumed_at` 回答。
        if input.is_watch() && reply.trim_start().starts_with(FOREMAN_NO_ACTION_MARK) {
            tracing::info!(
                session = %session.id,
                reply = %reply,
                "值守轮判定无需处理：静默入库，不播报"
            );
            return Ok(ForemanTurn {
                session: session.clone(),
                reply,
                prompt_tokens: tokens.0,
                completion_tokens: tokens.1,
                briefing,
                traces,
            });
        }
        let briefing_json = serde_json::to_value(&briefing)?;
        // 播报的标记由**后端**加上（不由模型自己说）：它是「这一轮不是回话」这个事实的载体，
        // 前端靠它把主动播报与回话分开渲染，模型不该有机会说错。
        let content = if input.is_watch() {
            format!("{FOREMAN_WATCH_MARK}{reply}")
        } else {
            reply.clone()
        };
        self.store
            .append_foreman_message(NewForemanMessage {
                session_id: session.id.clone(),
                role: FOREMAN_ROLE_ASSISTANT.to_string(),
                content,
                prompt_tokens: tokens.0,
                completion_tokens: tokens.1,
                briefing_json: Some(briefing_json),
                traces_json,
            })
            .await?;

        Ok(ForemanTurn {
            session: session.clone(),
            reply,
            prompt_tokens: tokens.0,
            completion_tokens: tokens.1,
            briefing,
            traces,
        })
    }

    /// **值守轮**（决策 209④ / 票 06）：有待办、且去抖窗口已到，就自己醒一次。
    ///
    /// 与人回话的区别只有一个——**输入不是人打的话，而是系统生成的简报**；模型调用、
    /// 工具循环、token 记账、`briefing_json` / `traces_json` 审计全部共用（[`Self::respond`]）。
    ///
    /// 返回 `Ok(None)` 的三种情形都是「这一趟不说话」：
    /// - 没有**会唤醒**的待办（§2.1 那条纪律：只有需要有人管的事才吵醒它）；
    /// - 有，但最早那件还没到去抖窗口（攒批：一夜的风吹草动该合成一次）；
    /// - 醒了，但判定无需处理（§2.4 静默规则）。
    ///
    /// **消费只在成功之后**：失败（模型报错 / 没回话）不置 `consumed_at`，下一趟还看得见
    /// 同一批——否则一次网络抖动就等于把这批事件丢了。
    pub async fn watch(&self) -> Result<Option<ForemanTurn>> {
        let open = self
            .store
            .open_attention(FOREMAN_ATTENTION_FETCH_LIMIT)
            .await?;
        let candidates: Vec<_> = open.iter().filter(|i| i.kind.wakes()).collect();
        if candidates.is_empty() {
            // 空闲时**零次模型调用**（票 06 的牙齿之一）
            return Ok(None);
        }
        // 去抖：从**最早那件**算窗口。攒批的代价是响得慢一点，收益是不为一件事吵两次；
        // 窗口过后的第一趟就把窗口内所有件一起带上（不许丢事件）。
        let debounce = chrono::Duration::seconds(self.settings.watch_debounce_sec as i64);
        if let Some(oldest) = candidates.iter().map(|i| i.created_at).min() {
            if self.store.now() - oldest < debounce {
                return Ok(None);
            }
        }
        // 同任务冷却（决策 209⑤ / 票 07）：刚被处理过的任务，新事件**不单独唤醒**——
        // 留在表里不消费，冷却到期后与那时的事件合并播报。判据是「这个任务最近有没有
        // 被消费过的待办」：那一行就是「刚有人看过它」的账。
        let cooldown = chrono::Duration::minutes(self.settings.watch_task_cooldown_minutes as i64);
        let mut waking = Vec::new();
        for item in candidates {
            let recently_handled = self
                .store
                .count_consumed_attention_since(&item.task_id, self.store.now() - cooldown)
                .await?
                > 0;
            if !recently_handled {
                waking.push(item);
            }
        }
        if waking.is_empty() {
            // 全在冷却里：这一趟不说话（事件仍在表里，等下一条路）
            return Ok(None);
        }
        let session = self.resolve_session(None).await?;

        // 全局唤醒上限（决策 209⑤）：触顶时**不静默丢弃**——留一行「本小时已达上限，
        // N 条待办未播报」给值班经理，且同一小时只留一行（否则触顶本身变成刷屏源）。
        // 待办**不消费**：下一小时继续，一条不丢。
        let hour_ago = self.store.now() - chrono::Duration::hours(1);
        let wakes = self.store.count_watch_wakes_since(hour_ago).await?;
        if wakes >= self.settings.watch_max_wakes_per_hour as usize {
            let noted = self
                .store
                .count_watch_wakes_with(
                    crate::storage::attention::WatchWakeOutcome::Capped,
                    hour_ago,
                )
                .await?;
            if noted == 0 {
                let content = format!(
                    "【值守】本小时唤醒已达上限（{} 次），{} 条待办未播报；下一小时继续，不会丢。",
                    self.settings.watch_max_wakes_per_hour,
                    waking.len()
                );
                if let Err(e) = self
                    .store
                    .append_foreman_message(NewForemanMessage::system(session.id.clone(), content))
                    .await
                {
                    tracing::error!(session = %session.id, "触顶提示写不进去：{e}");
                }
                self.store
                    .record_watch_wake(
                        Some(&session.id),
                        crate::storage::attention::WatchWakeOutcome::Capped,
                        waking.len(),
                        0,
                        0,
                    )
                    .await?;
            }
            return Ok(None);
        }

        let brief = render_watch_brief(&waking);
        match self.respond(&session, TurnInput::WatchBrief(brief)).await {
            Ok(turn) => {
                let ids: Vec<i64> = waking.iter().map(|i| i.id).collect();
                if let Err(e) = self.store.consume_attention(&ids).await {
                    // 消费失败只记日志：下一趟会重复看到这批事件，多醒一次比丢事件便宜
                    tracing::error!(session = %session.id, "值守轮消费待办失败：{e}");
                }
                let silent = turn.reply.trim_start().starts_with(FOREMAN_NO_ACTION_MARK);
                // 唤醒账（票 07）：静默那一轮**也花钱**，故它同样入账——数「花了多少」
                // 与数「醒了几次」用的是同一张表（会话行那边漏掉静默轮）。
                self.store
                    .record_watch_wake(
                        Some(&session.id),
                        if silent {
                            crate::storage::attention::WatchWakeOutcome::Silent
                        } else {
                            crate::storage::attention::WatchWakeOutcome::Broadcast
                        },
                        waking.len(),
                        turn.prompt_tokens,
                        turn.completion_tokens,
                    )
                    .await?;
                if silent {
                    Ok(None)
                } else {
                    Ok(Some(turn))
                }
            }
            Err(error) => {
                // 失败也留痕（票 04 那条路），且**不消费**——这批事件下一趟还在。
                self.record_failed_turn(&session, &error).await;
                Err(error)
            }
        }
    }

    /// 落一条**失败回合**的账（决策 211④ / 票 04）。
    ///
    /// `role = system` 复用「操作台记账」那条路（决策 207）：对讲台把它渲染成一轮，
    /// 模型下一轮也会看到它——于是「上一轮我为什么没回话」对它自己也是已知的一件事。
    async fn record_failed_turn(&self, session: &ForemanSession, error: &Error) {
        let (kind, reason) = turn_failure_reason(error);
        self.note_turn(
            &session.id,
            format!("{FOREMAN_FAILED_TURN_MARK}这一轮没跑起来（{kind}）：{reason}"),
        )
        .await;
    }

    /// 落一条**没跑完**的账（决策 223）：这一轮的 future 被 panic 带走时用。
    ///
    /// 与 [`Self::record_failed_turn`] 分开的理由是它拿不到 `Error`——`say()` 的失败外框
    /// 本身就在 panic 里没了，能看见这件事的只有调用方（HTTP 端点拿到 `JoinError`）。
    /// 会话取**此刻最近活动的未归档班次**：`say()` 已经先把用户那一句落了库，而落库会
    /// 更新该会话的 `last_active_at`，故那一行就在它里面。
    pub async fn record_interrupted_turn(&self, why: &str) {
        let session = match self.store.latest_foreman_session().await {
            Ok(Some(session)) => session,
            Ok(None) => return,
            Err(e) => {
                tracing::error!("记「没跑完」这一刻读不到班次：{e}");
                return;
            }
        };
        self.note_turn(
            &session.id,
            format!("{FOREMAN_FAILED_TURN_MARK}这一轮没跑完（{why}）：回话没有落库"),
        )
        .await;
    }

    /// 往台账里写一条**操作台自己**的账（`role = system`，决策 207 那条路）。
    ///
    /// 写不进去只记日志：要带回去的是「这一轮为什么没成」，不是「记账为什么没成」。
    async fn note_turn(&self, session_id: &str, content: String) {
        if let Err(e) = self
            .store
            .append_foreman_message(NewForemanMessage::system(session_id.to_string(), content))
            .await
        {
            tracing::error!(session = %session_id, "回合留痕写不进去：{e}");
        }
    }

    /// 把「哪个会话」解析成一个确实存在的会话行（决策 204）。
    ///
    /// 指定的会话必须存在且**未归档**：归档是把会话从列表里收起来，往一个已经不
    /// 露面的会话里继续说话，只会生成一段谁也看不见的记录。没指定时落到最近活动的
    /// 未归档会话，一个都没有就新开——首启空 home 的第一句话走的就是这条。
    async fn resolve_session(&self, session_id: Option<&str>) -> Result<ForemanSession> {
        match session_id {
            Some(id) => {
                let id = id.trim();
                let session = self
                    .store
                    .get_foreman_session(id)
                    .await?
                    .ok_or_else(|| Error::Task(format!("会话不存在：{id}")))?;
                if session.archived_at.is_some() {
                    return Err(Error::Validation(
                        "这个班次已归档——先新建或切到别的班次再说话".into(),
                    ));
                }
                Ok(session)
            }
            None => match self.store.latest_foreman_session().await? {
                Some(session) => Ok(session),
                None => self.store.create_foreman_session("").await,
            },
        }
    }

    /// 工具定义：**从清单生成**（票 01）。
    ///
    /// 与 [`crate::pipeline::subagent::StoreSubAgentRunner::tool_defs`] 同样的立场：
    /// **不经 `effective_tools`**——那条路会并入基线强制工具（含 `run_command` /
    /// `write_file`），正是本模块要挡掉的东西。
    ///
    /// 手写这两个 `ToolDef` 的时候，广告集与执行点白名单是两份独立的名单，而
    /// 「同源」是票 01 的硬要求：两处各写一份名字，迟早出现「模型看得见一个调用就被拒
    /// 的工具」这种不好定位的错。
    fn tool_defs(env_mode: crate::types::EnvMode, deny: &[&str]) -> Vec<ToolDef> {
        let available = foreman_available_tools_except(env_mode, deny);
        FOREMAN_TOOL_SPECS
            .iter()
            .filter(|spec| available.contains(&spec.name))
            .map(|spec| ToolDef {
                name: spec.name.to_string(),
                description: spec.description.to_string(),
                parameters: serde_json::from_str(spec.parameters)
                    .expect("清单里的参数 schema 必须是合法 JSON（单测钉住）"),
            })
            .collect()
    }

    async fn run_tool(
        &self,
        tools: &ToolExecutor,
        call: &crate::agent::client::ToolCall,
        ctx: &ToolCallContext,
    ) -> Result<crate::agent::tools::ToolOutcome> {
        tools.execute(call, ctx).await
    }

    /// 「你能动手到什么程度」那一段（决策 206 / 188）。
    ///
    /// **按档位写，不写一句笼统的「你没有权限」**：模型是照着这段描述自己汇报的，
    /// 描述与事实不符时它会说出与事实不符的话（「我已经写好了」/「我读不到文件」）。
    /// 这一段与 `ToolExecutor` 那道闸是同一件事的两种说法——一处给模型看，一处真的执行。
    ///
    /// **托管那一段是条件说的**（决策 210① / 票 08）：只有真开着托管的方案才说明它的存在，
    /// 否则模型会以为自己对任何任务都能免按键动手——而它实际只对**被托管的那几个**能。
    fn power_discipline(&self, env_mode: crate::types::EnvMode, stewarded: bool) -> String {
        let mut discipline = match env_mode {
            crate::types::EnvMode::Ask => "\
                 - 文件与命令这类**会改动东西**的动作：你调用之后**不会立即发生**，\
                 而是生成为一条待确认的提议，等值班经理在界面上按下确认钮才真正执行。\
                 `read_file` / `list_dir` / `Skill` 是只读的，直接执行。\
                 - 因此**绝不要说你已经做了那件事**：你可以说「我提了一条建议，等你按键」。\
                 - 本服务自己的写接口（建任务、拍板、合入、改配置……）一律走提议，\
                 这件事不随档位变。\n"
                .to_string(),
            crate::types::EnvMode::Auto => "\
                 - 文件与命令这类动作**会立即执行**（这个阶段被配成 auto 档）。执行结果\
                 会以工具回执的形式回来，写进时间线；你汇报时以回执为准，不要凭印象说。\
                 - 本服务自己的写接口（建任务、拍板、合入、改配置……）**仍然**要人按键，\
                 不随档位变——那类动作会改变流水线的事实。\n"
                .to_string(),
            crate::types::EnvMode::Deny => "\
                 - 这个阶段的环境层被关掉了：文件读写、命令执行、技能拉取都不可用，\
                 连工具都看不到。台账读数照常可用。**不要提议这类动作**——它无处可去。\n"
                .to_string(),
        };
        if stewarded {
            // 只对**开着托管的任务**这么说（票 08）：一段笼统的「你可以直接动手」会立刻
            // 变成一句假话——别的任务上它照样只能提。
            discipline.push_str(
                "- 有任务被值班经理**托管**（态势快照里标出「托管中」的那几个）：对它们你可以\
                 直接 `task` + `resume` + `resume_action=continue`，**不用等他按键**——\
                 这一条是例外，只对它、只对这个动作。其余动作（retry / merge / review /\
                 cancel / create）与其余任务照旧要按键。动手之后说明你做了什么、依据是什么。\n",
            );
        }
        discipline
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
    fn system_prompt(
        &self,
        cfg: Option<&crate::types::StageConfig>,
        env_mode: crate::types::EnvMode,
        stewarded: bool,
    ) -> Result<String> {
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
        // 工具纪律：**按清单生成**（票 01）。手写一份工具名清单的下场是它与
        // `FOREMAN_TOOL_SPECS` 各自漂移——模型于是要么看不见某个能调的工具，
        // 要么被告知去调一个不存在的工具（后者正是它开始编造读数的起点）。
        //
        // 档位（决策 206）也要在这里说：模型对「它做了什么」的描述**必须与事实一致**。
        // 上一版这段写的是「读不到文件系统，也不能执行命令」——那在 B 层落地之后是假的，
        // 而一段假的能力说明会直接变成一句假话（「我读过那个文件」）。
        let read_tools = foreman_tool_names(ForemanToolLayer::Read);
        let write_tools = foreman_tool_names(ForemanToolLayer::Write);
        out.push_str(&format!(
            "\n## 工具纪律\n\
             - 你能直接用的工具是：{}。\n",
            read_tools.join(" / ")
        ));
        out.push_str(&format!("{}\n", self.power_discipline(env_mode, stewarded)));
        if !write_tools.is_empty() {
            out.push_str(&format!(
                "- 会改动东西的工具是：{}。\n",
                write_tools.join(" / ")
            ));
        }
        out.push_str(
            "- 快照里已经有的（待拍板原因、在跑、失败、项目清单）不要再查一遍。\n\
             - 引用工位结论时必须标出它来自哪个工位、哪次运行。\n\
             - 你不知道的事就说不知道。",
        );
        Ok(out)
    }
}

/// 失败回合在台账里的标记（票 04）。对讲台按它把这一轮渲染成失败轮，不是一个中性轮。
pub const FOREMAN_FAILED_TURN_MARK: &str = "【没跑起来】";

/// 时限的人话说法（写进失败账里给人看的那一句）。
///
/// **整分钟才说分钟**：`max_duration_sec` 是个秒数，90 秒按整除写成「1 分钟」是在少报现场——
/// 而这一句正是人拿去判断「它到底挂了多久」的东西。
fn human_duration(limit: std::time::Duration) -> String {
    let secs = limit.as_secs();
    if secs >= 60 && secs % 60 == 0 {
        format!("{} 分钟", secs / 60)
    } else {
        format!("{secs} 秒")
    }
}

/// 一轮回话失败的归因（票 04）：`(稳定类别, 人话原因)`。
///
/// 与 [`Error::llm_classified`] 同一姿态：**能确证才标类别，其余退回原始串**——误标类别
/// 比不标更坏，它会把排查引向一个错误的方向（这条纪律的来历见 `error.rs` 的 `LlmClassified`）。
/// 四个面：网络 / 配置 / 模型 / 内部。
fn turn_failure_reason(error: &Error) -> (String, String) {
    match error {
        // `message` 是**人话那一段**（「这一轮没有回话」「密钥不对」这种），`raw` 是
        // 排查时能拿去搜的原始串。台账那一行两段都要有：只留原文，值班经理读不懂
        // 「无 tool_calls、无文本」是什么意思；只留人话，现场那串就丢了。
        Error::LlmClassified {
            kind, message, raw, ..
        } => {
            let reason = if raw.is_empty() || raw == message {
                message.clone()
            } else {
                format!("{message}（原文：{raw}）")
            };
            (kind.clone(), reason)
        }
        // 适配器层的失败（HTTP 错误 / 响应解析 / 连接被拒）：它就是网络那一类
        Error::Llm(msg) => ("llm_network".into(), msg.clone()),
        Error::Config(msg) => ("config".into(), msg.clone()),
        Error::Db(e) => ("db".into(), e.to_string()),
        Error::Migrate(e) => ("db".into(), e.to_string()),
        other => ("internal".into(), other.to_string()),
    }
}

/// 待办 → 值班长读的那段简报（票 06）。
///
/// 逐条给「类别 + 任务 + 当时的台账事实」，并**明确标注这是系统简报**（署名在
/// [`TurnInput::transcript_text`] 里，两者一起才完整：一段没有署名的事实清单会被
/// 当成有人在给它下指令）。
fn render_watch_brief(items: &[&crate::storage::attention::AttentionItem]) -> String {
    let mut out = String::from("值守简报：以下事件是调度器发现的，请判断哪些需要值班经理处置。\n");
    for item in items {
        let detail = item
            .detail_json
            .as_ref()
            .map(|d| d.to_string())
            .unwrap_or_else(|| "（无细节）".into());
        out.push_str(&format!(
            "- [{}] 任务 {}｜发生 {}｜{}\n",
            item.kind.as_str(),
            item.task_id,
            item.occurred_at.to_rfc3339(),
            detail
        ));
    }
    out.push_str(&format!(
        "\n若这些都不需要处置，只回一行 {FOREMAN_NO_ACTION_MARK}（不会打扰值班经理）；\
         有需要他管的，就说清哪台工位、卡在什么上、可以怎么做。"
    ));
    out
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
