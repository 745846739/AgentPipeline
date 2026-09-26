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

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use serde::{Deserialize, Serialize};

use crate::agent::client::{LlmClient, LlmRequest, Message, ToolDef};
use crate::agent::tools::{ToolCallContext, ToolExecutor};
use crate::config::Settings;
use crate::home::Home;
use crate::pipeline::proposals::StoreProposalSink;
use crate::process::RealProcessKiller;
use crate::sse::{SseEvent, SseSink, ToolPhase};
use crate::storage::foreman::{
    ForemanMessage, ForemanSession, NewForemanMessage, FOREMAN_ROLE_ASSISTANT,
    FOREMAN_SESSION_KIND_WATCH, FOREMAN_WATCH_SESSION_TITLE,
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

/// 一个工具的完整规格：名字 + 给人看的中文标签 + 广告语 + 参数 schema。
///
/// **唯一事实源**：广告给模型的那一份（[`FOREMAN_TOOL_SPECS`] → `tool_defs()`）、执行点的
/// 白名单（`ToolExecutor::with_allowed_tools` 收的那一份）与人格里工具纪律段的两组名单
/// 都从这里来（三处吃同一个 `available`，决策 247）；界面读的回执标签（`label` →
/// `GET /foreman/tools`）也从这里来。两处各写一份名字的后果是
/// 「模型看得见一个调用就被拒的工具」（或反过来，一个能调但没人告诉它的工具），
/// 两种都很难从现象定位——这正是票 01 要求「同源」的理由。
///
/// **没有「层」字段**（决策 247 删掉了 `ForemanToolLayer`）：「会改动东西」由档位谓词
/// （`is_env_write_tool` ∨ `is_service_write_tool`）判，手标一份枚举等于第二份要与档位表
/// 对账的镜像——而镜像漂了没人看得见（分组说的与闸门判的可以不同）。
pub struct ForemanToolSpec {
    pub name: &'static str,
    /// 给人看的中文词（回执与提议徽章上那个）。**必填**（决策 247④）：加工具不写标签
    /// 直接编译不过——前端那张 18 键手抄表（`TOOL_LABELS`）由此能整个删掉，漂移进不了 diff。
    /// 空串由单测拦（编译器只管「有没有」）。
    pub label: &'static str,
    pub description: &'static str,
    /// 参数 JSON-Schema 的**文本**：常量表里放不了 `serde_json::Value`，
    /// 用文本 + 一处解析（`tool_defs()`），并由单测钉住它是合法 JSON。
    pub parameters: &'static str,
}

/// 此刻**有哪几班正在跑一轮**（决策 260）。
///
/// 为什么需要它：一轮回话跑在独立任务里（决策 223），**它不随请求一起死**——本地放弃只
/// 丢掉这一次的同步回包，回话照旧落库。可这条实情此前只写在文案里（超时那一类的
/// `failureNotice`），**没有任何读数**：刷新页面之后，界面上既没有「这一轮还在跑」的
/// 那一轮（乐观轮与流式轮都住在 `sending` 这把局部状态里），也就**不再累积增量**——
/// 于是「它在说话」这件事只有等回话落地、重读台账才看得见。本节补的正是这个读数。
///
/// 登记的是**会话 id**、不是「一轮」的标识：界面要知道的是「这一班此刻有一轮在跑」，
/// 它据此才敢把到达的增量接进时间线（否则那一段字属于谁就无从判断）。
///
/// **进程内**（照 [`crate::pipeline::executor`] 的 `EXECUTOR_REGISTRY` 同一姿态）：跨进程
/// 的情形是**上一个实例留下的**，那种「还在跑」是假的——而它恰好由启动时的
/// `orphan_inflight_model_requests` 与 `requeue_running_tasks` 一起收口（决策 255④ / 226）。
static FOREMAN_TURNS: LazyLock<Mutex<HashMap<String, usize>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// 一轮在跑的登记凭据：**退出即摘**。
///
/// 记账用**计数**而不是一个布尔 / 一格代次：同一班**可以**同时跑着两轮——值守轮（决策 209）
/// 与人打的一句话各起一轮，两者之间没有任何互斥。按格覆盖的话，先退出的那一轮会把另一轮
/// 的登记一起摘掉，界面于是在真的还在跑的时候读到「不在跑了」；计数不会：它就是「此刻有几轮
/// 在跑」这个读数本身。
struct ForemanTurnGuard {
    session_id: String,
}

impl Drop for ForemanTurnGuard {
    fn drop(&mut self) {
        let mut turns = FOREMAN_TURNS.lock().unwrap();
        match turns.get_mut(&self.session_id) {
            Some(count) if *count > 1 => *count -= 1,
            _ => {
                turns.remove(&self.session_id);
            }
        }
    }
}

/// 登记「这一班开始跑一轮」，返回退出即自动摘的凭据（决策 260）。
fn begin_foreman_turn(session_id: &str) -> ForemanTurnGuard {
    *FOREMAN_TURNS
        .lock()
        .unwrap()
        .entry(session_id.to_string())
        .or_insert(0) += 1;
    ForemanTurnGuard {
        session_id: session_id.to_string(),
    }
}

/// 此刻这一班**有一轮在跑**吗（决策 260）。
///
/// 界面刷新之后靠它决定「要不要把到达的增量接进时间线」——见 [`FOREMAN_TURNS`]。
/// 读的是**登记**而不是台账：台账里没有「在跑」这一行（回话落库才算数），而这一轮的
/// 现场（乐观轮 / 流式文本）本来就全在界面那侧，刷新即丢。
pub fn foreman_turn_in_flight(session_id: &str) -> bool {
    FOREMAN_TURNS.lock().unwrap().contains_key(session_id)
}

/// 人的那一轮**单独**的在飞计数（决策 289 / 票 03）：`say` 起、`say` 落；值守轮不置它。
///
/// 为什么不读 [`FOREMAN_TURNS`]：那格答的是「这一班有没有一轮在跑」，两条时间线分家之后
/// 各自登记各自的班次——值守轮在另一个 session_id 上，从那里看不出人正在说话。而裁决 2
/// 的排队判据要的恰是「**人**在不在跑」：同一时刻两份十几万 token 的上下文打同一个
/// provider（2026-09-26 实测并行 7.8 分钟），既是浪费也拖慢人的那一轮。
///
/// 计数挂在 **runner 实例**上而不是全局 static：生产里 `AppState` 只有一个值班长，
/// 实例级与进程级是同一个读数；全局 static 则会让「人在跑」这个现场漏进任何一段
/// 无关的并发代码——测试（并跑的用例各建各的 runner）是它第一个咬到的地方。
#[derive(Default)]
struct HumanTurns(Arc<AtomicUsize>);

/// [`HumanTurns`] 的登记凭据：退出即摘（与 [`ForemanTurnGuard`] 同一姿态）。
pub struct HumanTurnGuard(Arc<AtomicUsize>);

impl Drop for HumanTurnGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl HumanTurns {
    fn begin(&self) -> HumanTurnGuard {
        self.0.fetch_add(1, Ordering::SeqCst);
        HumanTurnGuard(Arc::clone(&self.0))
    }

    fn in_flight(&self) -> bool {
        self.0.load(Ordering::SeqCst) > 0
    }
}

/// 值班长的工具清单（决策 182⑭ → 决策 188 / 207）。
///
/// 与 [`crate::pipeline::subagent::SUB_AGENT_TOOLS`] 同一姿态：这是**安全边界本身**，
/// 不是配置项。任何「给值班长加个工具」的改动都必须先改这里，从而在 diff 里显式可见。
///
/// A 层的六个新读数（票 01）**一律复用后端既有口径**，不新造一套：看板读任务表、
/// 指标走 `metrics::*` 纯函数、项目 / 阶段配置 / 技能 / provider 各读自己那张表的既有读法。
/// 唯一需要加工的是 provider：库里存的是**明文密钥**（决策 112），故只回显掩码。
pub const FOREMAN_TOOL_SPECS: [ForemanToolSpec; 24] = [
    ForemanToolSpec {
        name: "read_task",
        label: "读任务台账",
        description: "读某个任务的台账详情：标题、状态、当前工位、待办原因原文、\
                      后端下发的可用动作、各分支游标。卡住的细节问它。",
        parameters: r#"{"type":"object","properties":{"task_id":{"type":"string","description":"任务 id（快照里方括号内那串）"}},"required":["task_id"]}"#,
    },
    ForemanToolSpec {
        name: "read_conversation",
        label: "读工位会话",
        description: "读某个任务某次节点运行的会话回执（工位当时说了什么）。\
                      run_id 省略时取该任务最近一次会话。引用工位结论时要标出来源。",
        parameters: r#"{"type":"object","properties":{"task_id":{"type":"string","description":"任务 id"},"run_id":{"type":"integer","description":"运行 id，省略取最近一次"}},"required":["task_id"]}"#,
    },
    ForemanToolSpec {
        name: "read_board",
        label: "读看板",
        description: "读整块看板：每个任务的状态与当前工位，以及按状态的分组计数。\
                      想知道「一共有多少活、都在哪一档」时问它。",
        parameters: r#"{"type":"object","properties":{}}"#,
    },
    ForemanToolSpec {
        name: "read_metrics",
        label: "读指标",
        description: "读全局指标：任务数、成功率、validate 首过率、token 与调用总量、\
                      按阶段的聚合。这些数与指标页同源。",
        parameters: r#"{"type":"object","properties":{}}"#,
    },
    ForemanToolSpec {
        name: "read_projects",
        label: "读项目",
        description: "读已接入的项目清单：名字、仓库路径、默认分支、语言与测试命令。",
        parameters: r#"{"type":"object","properties":{}}"#,
    },
    ForemanToolSpec {
        name: "read_stage_configs",
        label: "读阶段配置",
        description: "读各阶段的配置：provider、采样参数、人格文件、技能声明、超时。\
                      想解释「为什么这个工位表现是这样」时问它。",
        parameters: r#"{"type":"object","properties":{}}"#,
    },
    ForemanToolSpec {
        name: "read_skills",
        label: "读技能清单",
        description: "读当前可用的技能清单（技能根下的 markdown）：名字、描述、\
                      被哪些阶段引用。",
        parameters: r#"{"type":"object","properties":{}}"#,
    },
    ForemanToolSpec {
        name: "read_providers",
        label: "读 provider",
        description: "读已配置的 provider 清单：id、厂商、模型、是否启用、上下文窗口。\
                      **密钥只回显掩码**——你看到的是「有没有配」，不是密钥本身。",
        parameters: r#"{"type":"object","properties":{}}"#,
    },
    // ── B 层：环境只读（决策 206）。域是家目录根，`data/` 按前缀拒——库里明文存着
    //    provider 密钥（决策 112），而默认那份**模式**名单盖不住一个 `.db` 文件。
    //    **`logs/` 曾同样按前缀拒，决策 226 撤销了那一条**：体量该由 `read_file` 的字节上限
    //    管，不该用一堵把 877 字节的日志也一并挡在外面的墙（实测代价见 2026-09-19 那次僵死）。
    //    这三件事在 `deny` 档下连广告都不给（`foreman_available_tools` 筛的），
    //    在 `ask` 档下照常直接执行（只读不需要人按键）。
    //
    //    `spawn_sub_agent` **不在列**：它要注入一个子代理运行器才有意义，而值班长手上
    //    没有（也不该有——它是面向人的对话者，不是流水线节点）。加一个只会回「未启用」的
    //    工具，等于让模型每轮都看得见一个它调了也没用的东西。
    ForemanToolSpec {
        name: "read_file",
        label: "读文件",
        description: "读一个文件（路径相对家目录根）。看配置、看产物、看你提议要改的那个\
                      文件现在长什么样——**改之前先看**。日志、运行记录这类追加写的文件\
                      要尾巴就用 tail=true。",
        parameters: r#"{"type":"object","properties":{"path":{"type":"string","description":"相对家目录根的路径"},"offset":{"type":"integer","description":"起始行（从 0 数）；tail 为真时忽略"},"limit":{"type":"integer","description":"最多读几行"},"tail":{"type":"boolean","description":"读尾部而不是头部（日志用它）"}},"required":["path"]}"#,
    },
    ForemanToolSpec {
        name: "list_dir",
        label: "列目录",
        description: "列一个目录（路径相对家目录根，省略取根）。不知道东西在哪儿时先列一层。",
        parameters: r#"{"type":"object","properties":{"path":{"type":"string","description":"相对家目录根的路径"},"recursive":{"type":"boolean","description":"是否递归"}}}"#,
    },
    ForemanToolSpec {
        name: "Skill",
        label: "取技能正文",
        description: "按技能名取它的正文（技能目录下的 markdown）。有人问「某个技能到底干什么」\
                      时用它，不要凭名字猜。",
        parameters: r#"{"type":"object","properties":{"name":{"type":"string","description":"技能名（技能目录里列出的那个）"}},"required":["name"]}"#,
    },
    // 诊断包（决策 211③ / 票 03）：一族一个工具，一次调用给出定因所需的全部证据。
    // 它**不扩 `read_task`**——后者是每轮值守都会调的高频、便宜读数，混进来会让
    // 「看一眼任务状态」开始烧 12k 字符。排在只读层末尾：往这份清单里加东西，
    // diff 里永远是末尾多一段（`FOREMAN_TOOL_SPECS` 的顺序被冻结断言逐条钉住）。
    ForemanToolSpec {
        name: "read_diagnosis",
        label: "读诊断包",
        description:
            "读某个任务的**诊断包**：一次拿到定因所需的全部证据——每条 run 的状态 / 耗时 / \
                      token / error / 是否挂过进程组、命令台账与闸门输出路径、阶段产出与验收标准、\
                      待办原因原文、组装后的 prompt 原文、失败 run 的最后几条工具往来。\
                      「它为什么卡住 / 为什么失败 / 是不是 prompt 问题」这类问题问它；\
                      只看「现在什么状态」用 read_task（便宜得多）。",
        parameters: r#"{"type":"object","properties":{"task_id":{"type":"string","description":"任务 id（快照里方括号内那串）"},"runs":{"type":"integer","description":"最多带回多少条 run（默认 30，最近的在前）"}},"required":["task_id"]}"#,
    },
    // 只读取证（决策 232 / 237 / 票 02）：白名单就是它的全部能力——`date` / `ps` / `pgrep` /
    // `lsof` / `wc` / `tail` / `sample`，argv 直出不经 shell，`sample` 只许对本服务的进程。
    // 它属**只读层**：改不了任何东西，故不受档位管、也不吃值守轮的 deny 清单。
    ForemanToolSpec {
        name: "run_readonly",
        label: "只读取证",
        description:
            "跑一条**只读**的诊断命令（白名单：date / ps / pgrep / lsof / wc / tail / sample）。\
                     不经 shell——command 与 args 是两个独立的参数，分号、管道、$(...) 都没有落点。\
                     路径参数必须落在你的域内（家目录根，data/ 读不到）；sample 只能对本服务自己的\
                     进程树取证。取证优先用它，不要等人按键：它是「夜里自己把事定死」的那只手。",
        parameters: r#"{"type":"object","properties":{"command":{"type":"string","enum":["date","ps","pgrep","lsof","wc","tail","sample"],"description":"白名单里的命令名（不经 shell，直接 exec）"},"args":{"type":"array","items":{"type":"string"},"description":"命令参数（必须是字符串数组，例如 [\"-n\",\"50\",\"logs/agentpipeline.log\"]；字符串形态会被拒）"},"timeout_sec":{"type":"integer","description":"超时秒数（可选，缺省按阶段配置）"}},"required":["command"]}"#,
    },
    // 内容搜索（决策 267 / 票 01）：只读层的找内容之手——纯 Rust 正则 + 域内行走，
    // 不碰系统二进制（grep/rg 的旗标差异与二进制在场都不再是它的前提）。与
    // `run_readonly` 同属只读层：不受档位管（改不了任何东西，237 判据）、值守轮
    // deny 清单**不摘**它（夜里找证据不需要人在场）。
    ForemanToolSpec {
        name: "search_content",
        label: "搜内容",
        description: "在你的文件域里**按正则找内容**（报错串、配置键、函数名落在哪几处）。\
                     pattern 是正则（如 `timeout_sec|TIMEOUT`，区分大小写）；path 缺省整个\
                     域根，也可指定域内子目录（data/ 读不到——与 read_file 同一条域规则）。\
                     不跟符号链接；二进制文件跳过；命中按「路径:行号:行文本」带回并有行数\
                     上限。列文件名用 list_dir、跑诊断命令用 run_readonly——找**内容**用它。",
        parameters: r#"{"type":"object","properties":{"pattern":{"type":"string","description":"正则表达式（区分大小写）"},"path":{"type":"string","description":"从哪个目录开始找（相对域根，缺省整个域；data/ 被拒）"}},"required":["pattern"]}"#,
    },
    // 受治理的网口（决策 266 / 票 02）：GET-only、https 出环、白名单走 `NetworkPolicy`
    // **同一张**（决策 179，零第二版本）、每次取数与每次被拒都落命令台账。同属只读层故
    // 不受档位管（237 的判据：改不了任何东西），但值守轮的 deny 清单收它——与
    // `run_readonly` 的区别正在这一条上：夜里没人盯外发。
    ForemanToolSpec {
        name: "web_fetch",
        label: "读网页",
        description: "用 GET 取一个网页 / 接口的**文本**正文（受治理的只读网口）。\
                     只出 https（回环地址例外可 http）；可达域由出口白名单决定——默认只有回环，\
                     放行域在 config.toml 的 `[pipeline] egress_allow_hosts`（`*.example.com` \
                     覆盖子域）。缺省 15 秒超时，可用 timeout_sec 覆盖（1–120）；只收文本类 \
                     content-type（text/*、json、xml），超 512KiB 截断；不跟随重定向（302 会给 \
                     Location，换 URL 再取一次）。每一次（含被拒的）都落命令台账。\
                     **不要把密钥、令牌或敏感正文拼进 URL**——query 会同时进台账与对端。\
                     值守轮没有这个工具。",
        parameters: r#"{"type":"object","properties":{"url":{"type":"string","description":"要取的完整 URL（https）"},"timeout_sec":{"type":"integer","description":"可选：本次超时秒数（1–120，缺省 15）"}},"required":["url"]}"#,
    },
    // ── C 层：环境写（决策 206 / 207）。在 `ask` 档下**不执行**，生成提议等人按键；
    //    `auto` 直通；`deny` 连广告都不给。域与 B 层同一份（家目录根 + `data` / `logs` 前缀 deny）。
    ForemanToolSpec {
        name: "write_file",
        label: "写文件",
        description: "写一个文件（整份覆盖，路径相对家目录根）。**改之前先 read_file 看一眼**\
                      ——覆盖是不可逆的，你不知道原来有什么就会把有用的东西抹掉。\
                      这条动作要值班经理按键确认。",
        parameters: r#"{"type":"object","properties":{"path":{"type":"string","description":"相对家目录根的路径"},"content":{"type":"string","description":"文件的全部内容（整份覆盖）"}},"required":["path","content"]}"#,
    },
    ForemanToolSpec {
        name: "edit_file",
        label: "改文件",
        description: "改一个文件里的一处（把 old_text 换成 new_text，路径相对家目录根）。\
                      只想动一小段时用它，不要整份重写——整份重写会把文件里其它地方没读到的内容\
                      一起抹掉。改之前先 read_file。这条动作要值班经理按键确认。",
        parameters: r#"{"type":"object","properties":{"path":{"type":"string","description":"相对家目录根的路径"},"old_text":{"type":"string","description":"要被替换的原文（须在文件中出现，且只替换第一处）"},"new_text":{"type":"string","description":"替换成什么"}},"required":["path","old_text","new_text"]}"#,
    },
    ForemanToolSpec {
        name: "run_command",
        label: "跑命令",
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
        label: "修复",
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
        label: "服务动作",
        description: "本服务自己的运维动作。action 取值：`restart`（重启服务）。\
                      它是**全局**动作：会打断所有在跑的任务（本服务没有自重启能力，\
                      按下后先做恢复序列——清占用 + 把中断的 running 任务归队——\
                      再告诉你需要在你启动它的地方重启一次）。永远要值班经理按键确认。",
        parameters: r#"{"type":"object","properties":{"action":{"type":"string","enum":["restart"],"description":"要做的动作"}},"required":["action"]}"#,
    },
    ForemanToolSpec {
        name: "task",
        label: "任务动作",
        description: "对一个流水线任务动作。action 取值：\
                      `create`（建任务，要 project_id 与 title）、\
                      `resume`（让人拍过板的 pending 继续走，要 task_id 与 resume_action）、\
                      `retry`（重跑一个终态任务，回到 init）、\
                      `pause`（把**在跑**的任务按住：中止在飞的那一轮、位置保留，要 task_id）、\
                      `rerun`（把**当前阶段**从入口重跑一遍，那一轮不算，要 task_id）、\
                      `cancel`（取消）、\
                      `review`（人工评审通过/打回，要 approved）、\
                      `merge`（合入决定，要 decision=approve 或 reject）、\
                      `unstick`（解除僵死占用：run 已终态而游标仍 active / 有主但心跳停了——\
                      清执行者、标终态、游标转 pending；**只对真卡住的任务生效**）。\
                      参数与界面上那个按钮点下去时发的一模一样——先 read_task 看清它现在卡在\
                      哪个 pending、允许的动作是什么，再决定 action 与 resume_action。\
                      被 `pause` 按住过的任务（pending 原因 `user_paused`）用 `resume` 松开：\
                      动作名取 read_task 的 allowed_actions——续跑是 `continue`、\
                      重跑本阶段是带落点的 `goto`。\
                      每条都要值班经理按键确认。",
        parameters: r#"{"type":"object","properties":{"action":{"type":"string","enum":["create","resume","retry","pause","rerun","cancel","review","merge","unstick"],"description":"要做的动作"},"task_id":{"type":"string","description":"目标任务（create 之外的 action 都要）"},"project_id":{"type":"string","description":"create：建在哪个项目下"},"title":{"type":"string","description":"create：任务标题"},"description":{"type":"string","description":"create：任务描述"},"depends_on":{"type":"array","items":{"type":"string"},"description":"create：依赖的任务 id"},"review_mode":{"type":"string","enum":["agent","human"],"description":"create：评审模式"},"cursor_id":{"type":"string","description":"resume：指定游标（多条活跃游标时必填）"},"resume_action":{"type":"string","description":"resume：拍板的动作名（read_task 的 allowed_actions 里那几个）"},"target_stage":{"type":"string","description":"resume：跳到哪个阶段"},"target_node":{"type":"string","description":"resume：跳到哪个节点"},"input":{"type":"string","description":"resume：给这次拍板的说明 / 打回意见"},"approved":{"type":"boolean","description":"review：通过还是打回"},"comments":{"type":"string","description":"review：打回时带给下游的意见"},"decision":{"type":"string","enum":["approve","reject"],"description":"merge：合入还是打回"}},"required":["action"]}"#,
    },
    ForemanToolSpec {
        name: "config",
        label: "改阶段配置",
        description: "改流水线的阶段配置。action 取值：`set`（整条替换某个阶段的配置，要 stage；\
                      留空的字段会被清成默认——这是整条替换不是局部修改）、\
                      `delete`（删掉这个阶段的覆盖行，回到系统默认，要 stage）。\
                      要值班经理按键确认。",
        parameters: r#"{"type":"object","properties":{"action":{"type":"string","enum":["set","delete"],"description":"set 或 delete"},"stage":{"type":"string","description":"阶段键（如 develop / review / foreman）"},"provider_id":{"type":"string","description":"set：用哪个 provider"},"temperature":{"type":"number","description":"set：采样温度"},"max_tokens":{"type":"integer","description":"set：输出上限"},"persona_path":{"type":"string","description":"set：人格文件路径"},"persona_append":{"type":"string","description":"set：追加指令"},"env_mode":{"type":"string","enum":["auto","ask","deny"],"description":"set：环境层权限档位"},"tools_json":{"description":"set：工具声明（与界面那个框同形）"},"skills_json":{"description":"set：技能声明（与界面那个框同形）"},"node_overrides_json":{"description":"set：节点级覆盖（与界面那个框同形）"},"idle_timeout_sec":{"type":"integer","description":"set：空闲超时"},"max_duration_sec":{"type":"integer","description":"set：最长时长"},"max_rounds":{"type":"integer","description":"set：只对 foreman 行有意义——一轮里最多几次模型调用，**正整数**（0 / 负数会被拒，没有「无上限」）"},"watch_token_budget":{"type":"integer","description":"set：只对 foreman 行有意义——值守轮一轮的生成 token 预算，**正整数**（缺省 120000；0 / 负数会被拒，没有「无预算」这一档）"}},"required":["action","stage"]}"#,
    },
    ForemanToolSpec {
        name: "skills",
        label: "技能动作",
        description: "装 / 卸技能。action 取值：`install`（从一个本地技能目录导入，要 path，\
                      该目录自身含 SKILL.md）、`delete`（卸载一个已装技能，要 name）。\
                      卸载不检查引用——仍被阶段配置引用的技能卸掉之后，那个阶段解析会报错。\
                      要值班经理按键确认。",
        parameters: r#"{"type":"object","properties":{"action":{"type":"string","enum":["install","delete"],"description":"install 或 delete"},"path":{"type":"string","description":"install：技能目录路径"},"name":{"type":"string","description":"delete：技能名"},"overwrite":{"type":"boolean","description":"install：同名时是否覆盖"}},"required":["action"]}"#,
    },
    // 结构化选项提问（决策 265 / 票 01）：**第三种轮型**的入口。不在两段写清单里
    // （问话不是打算执行的动作，`gate_decision` 恒 Execute），值守轮的 deny 清单收它
    // （在叫人、不在问人）；载荷落那一轮 assistant 行的 `ask_json`（迁移 0026）。
    ForemanToolSpec {
        name: "ask",
        label: "提问",
        description: "需要值班经理在 **2–4 个选项**里拍板、而你猜不出 TA 的偏好时用：\
                     把问题与选项发出去，TA 在时间线上点选（或自己写一句）作为下一条消息回来。\
                     选项是短语数组（2–4 个，各一句）；**一轮只许问一个**，问完就简短收口、\
                     不要把问题再复述一遍。它不是提议（不需要按键执行），也不进任何执行链。\
                     值守轮没有这个工具——那一轮在叫人，不在问人。",
        parameters: r#"{"type":"object","properties":{"question":{"type":"string","description":"一句话问清楚"},"options":{"type":"array","minItems":2,"maxItems":4,"items":{"type":"string"},"description":"2–4 个可点选项，各一句短语"}},"required":["question","options"]}"#,
    },
];

/// 「会改动东西」的判据（决策 247）：工具纪律段的两组按它分家，[`crate::agent::tools::gate_decision`]
/// 按同一组档位表分流——**分类只此一份事实源**（`ENV_WRITE_TOOLS` / `SERVICE_WRITE_TOOLS`），
/// 不再另有手标的层枚举与它对账。
fn mutates_something(name: &str) -> bool {
    crate::agent::tools::is_env_write_tool(name) || crate::agent::tools::is_service_write_tool(name)
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
    // 这一轮**真正拿得到**的工具名（决策 247）：由 `respond_inner` 算一次传进来，
    // 与广告集、工具纪律段吃同一份——三处不再各筛一次。
    available: &[&'static str],
    // 托管动作的执行者（票 08）。`None` = 不放行：D 层照旧恒提议。
    steward: Option<Arc<dyn crate::agent::tools::StewardActionRunner>>,
    // 台账读数**不预截**（决策 291 / 票 06(a)）：人的那一轮与「按键执行」那一趟为真，
    // 值守轮为假——分级纪律（它只读台账与诊断包摘要，决策 265 / 266）一字不动。
    ledger_unbounded: bool,
) -> (ToolExecutor, ToolCallContext) {
    // 值班长的域就是家目录根（决策 207 的「分两组」：流水线阶段仍限任务工作区），
    // 并按路径前缀拒掉 `data/`——库里明文存着 provider 密钥（决策 112），
    // 而默认那份**模式**名单（`.env*` / `*.pem` / …）盖不住一个 `.db` 文件。
    // `file_access_unrestricted`（决策 283）打开时连家目录根这一层也放开：允许根由
    // 设置决定，`data/` 的前缀拒绝照旧。
    let tools = ToolExecutor::new(
        home.clone(),
        crate::agent::file_policy::foreman_file_policy(
            home.root(),
            settings.file_access_unrestricted,
        ),
        settings.clone(),
        Arc::new(RealProcessKiller),
    )
    .with_ledger(store.clone())
    // 命令记录（§12.4.4）：值班长的命令要落 `kanban_node_commands`，`task_id` 为 NULL、
    // 归属走会话（迁移 0012）。它同时是**出口策略拒绝**的落点（决策 179）——被拒的命令
    // 也要留一行，否则策略在审计面完全不可见，只剩模型侧的一次报错。
    .with_recorder(Arc::new(store.clone()))
    .with_env_mode(env_mode)
    .with_allowed_tools(available.to_vec());
    let tools = if ledger_unbounded {
        tools.with_ledger_unbounded()
    } else {
        tools
    };
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

/// 值守轮**失败账**的标记（决策 271）。
///
/// 与 [`FOREMAN_FAILED_TURN_MARK`]（人的那一轮失败）分开的原因只有一个：线上形态的
/// 「这一行是什么」由角色 + 正文前缀判（`message_wire`），而两者的**名牌**不该一样——
/// 「发送失败」说的是「你刚发出去的那条没到」，而值守轮的失败账里值班经理一个字节都没发。
/// 前缀是后端写的、后端认，前端仍只看字段（决策 252 的口径不变）。
pub const FOREMAN_WATCH_FAILED_TURN_MARK: &str = "【值守没跑起来】";

/// 值守轮失败后的**重试退避**（决策 271）：瞬时类 30s 起、每失败一次翻倍，600s 封顶。
///
/// 起因是 2026-09-24 的实测：provider 断供 6 分钟，对讲台多了 37 行一模一样的失败账——
/// 失败轮不消费待办（对）+ 每 10 秒一趟（`WATCH_INTERVAL`）+ 失败不计入每小时唤醒上限
/// （那片账只记成功与触顶），三条叠起来就是一台闹钟。
const FOREMAN_WATCH_RETRY_BASE_SECS: i64 = 30;
const FOREMAN_WATCH_RETRY_MAX_SECS: i64 = 600;
/// **等也不会好**的那些类别另起一档（更长）：余额不足 / 密钥错 / 模型名错 / 上下文超窗——
/// 按网络的节奏重试毫无意义，只会把噪声放大。
const FOREMAN_WATCH_RETRY_CONFIG_BASE_SECS: i64 = 300;
const FOREMAN_WATCH_RETRY_CONFIG_MAX_SECS: i64 = 1800;

/// 值守轮（**自动那一轮**）不许用的工具（决策 209⑥ / 票 07；265 / 266 各扩一项）。
///
/// 前两项的判据是「这一次你不在场」——主动播报是「固定成本 × 时间」，而你不在场时没有
/// 任何收益能摊平它：
/// - `read_conversation`：一次最多 12k 字符的会话原文；
/// - `run_command`：在本机上跑命令。
///
/// 后两项的判据是「这一轮没有人在场」，形状不同但同一句话（用户四问裁决，265 / 266）：
/// - `ask`：**在叫人、不在问人**——值守轮发起的提问没有人点选，只会悬在时间线上；
/// - `web_fetch`：**夜间外发无人盯**——外发是治理面（179），无人值守时不开口。
///
/// 四项在**被追问时**照常可用（那是人的那一轮，`say` 走的是完整工具集）——所以这不是
/// 削减能力，是**分级**：自动轮只读台账与诊断包摘要（`read_diagnosis` 自带 12k 上限）。
pub const FOREMAN_WATCH_TOOL_DENY: [&str; 4] =
    ["read_conversation", "run_command", "ask", "web_fetch"];

/// 一次值守轮最多把多少条待办喂进简报。
///
/// 与 `FOREMAN_HISTORY_BUDGET_CHARS` 同一姿态：真正的账是字符，这个数只是「别把一夜的
/// 事件都塞进一次简报」的粗兜底。超出的部分留在表里，下一轮（或被追问时）再处理。
const FOREMAN_ATTENTION_FETCH_LIMIT: usize = 50;

/// 单次回话的工具往返轮数**缺省值**（决策 182④，8 → 30 由决策 224，30 → 300 由决策 233①/239，
/// 300 → 1000 由决策 292 / 票 07）。
///
/// 正常靠「模型不再发起 tool_call」自然结束；这个上限是防御性的——模型若陷入
/// 「查一个任务 → 再查一个」的循环，必须有人喊停。
///
/// **现在它是「缺省」而不是「唯一取值」**（决策 233① / 239）：真正生效的数住在
/// `stage_configs` 的 `foreman` 行（`max_rounds`，只收正整数，见 `Store::upsert_stage_config`
/// 的写入校验与 `validate_startup`），这一份是没配过时的缺省。
///
/// **抬到 1000 的来历**（决策 292 / 票 07）：真正的成本界是 token 预算（值守轮 120k 硬界、
/// 人的那一轮只落软告警），轮数退为**兜底**——而它要兜的是「模型行为失控」，那件事在
/// 300 上会被一次正常的深查误伤（实测里 24 次调用就有一次被别的界砍掉，那时它还在干活）。
/// 数量级上 1000 轮 × 每轮千级 token 已远超任何一条 token 预算，故它够不着正常路径，
/// 只在预算管不到的地方（人的那一轮没有硬界）当最后一道闸；人真正的停法是票 09 那颗钮。
///
/// **它不管时间也不管 token**：整轮墙钟已撤（决策 288 / 票 05，兑现决策 233 如实记 (i)），
/// 单次调用的界是流上的空闲判死（foreman 行的 `idle_timeout_sec`）；整轮的预算界是
/// [`FOREMAN_WATCH_TOKEN_BUDGET`] 那条 token 分档（决策 292）。
pub const FOREMAN_MAX_ROUNDS: usize = 1000;

/// 值班长一轮的**生成 token 预算**缺省值（决策 292 / 票 07）：**只对值守轮是硬界**。
///
/// 取值 120k 的来历（票面）：2026-09-26 那次实测一轮烧了 85,135 生成 token，120k 是它的
/// 约 1.4 倍——按同一批实测的 ~45 token/s 折算约 45 分钟，足够一次夜巡把该查的查完，
/// 又能在真失控时（重复查同一件事）及时收口。
///
/// **分档**（裁决 7）：人的那一轮**无硬界**——终点由人决定（人在看着本身就是那道界，
/// 票 09 的停钮是它的动作面）；同一条线在人的那一轮上只落一条**软告警**（只落账不拦）。
pub const FOREMAN_WATCH_TOKEN_BUDGET: u32 = 120_000;

/// 值班长回话完成通知（决策 272③）的**门**：`say` 轮至少动过这么多次工具，才算
/// 「干了一轮活」、才叫通知出口。判据是这一轮自己的产出（`traces.len()`），收口处
/// 现成可得——不用墙上时钟、不加配置项（决策 256 的尺子：没人会调的旋钮比没有更坏）。
///
/// 取 3 的理由：一两次工具调用多半是「顺手查一眼」，人多半还在屏幕前；连续三次是
/// 「做了一系列动作」——那才是会离开屏幕的轮。**诚实记账**（决策 272）：零工具但
/// 生成很慢的一轮判不到（不通知，判为可接受）。门必须在 `notify()` **之前**过
/// ——短轮连 `foreman_reply` 的 cooldown 槽都不碰（cooldown 倒挂的坑）。
pub const FOREMAN_REPLY_MIN_TOOL_CALLS: usize = 3;

/// 这一轮**为什么没能正常收口**（决策 292 / 票 07 / 293 / 票 08）。
///
/// 它管「触顶」「中途失败」「打转」三类——轮数上限那一类由循环自然跑完表达（`stop` 为空，
/// 决策 233② 那条最老的路）。四条非正常结束共用同一条收口路径，差别写在这里
/// （「人按停」是第四条，票 09）。
#[derive(Debug)]
enum StopReason {
    /// 成本门（值守轮）：生成 token 到了预算线。带的是**触发那一刻的累计值**（标注里要写它）。
    Budget(u32),
    /// 循环检测（票 08）：提醒过一次仍在打转，强制收口。带的是判定本身（证据要进标注）。
    Loop(crate::agent::loops::Loop),
    /// 中途失败（空闲判死 / 超长 / 取消 / 内部错误）：错误原样带回外框做失败记账。
    Failed(Error),
}

/// 触到上限时那一段的标记（决策 233② / 292）：**部分结论落库并标注**。
///
/// 它必须看得见：一则让值班经理知道「这不是结论而是没说完」，二则让下一轮（或下一班）
/// 读历史时知道那一段是半成品——把半成品当结论用，正是这一批要修的那类失真。
///
/// 决策 292 起它不再只挂「轮数上限」那一支：**所有非正常结束**（触顶 / 中途失败 / 取消）
/// 都走同一条收口路径带上它（行为一样的几个理由共用同一个标记；「为什么」写在标记之后）。
pub const FOREMAN_PARTIAL_TURN_MARK: &str = "【未收口】";

/// 「在原地打转」的收口标记（决策 293 / 票 08，与 [`FOREMAN_PARTIAL_TURN_MARK`] **同族**）：
/// 停止的原因不同、形状要能一眼分开（裁决 9 的三个标记：触顶 / 打转 / 人按停）。
pub const FOREMAN_LOOP_TURN_MARK: &str = "【未收口·在打转】";

/// 打转提醒的标记（决策 293 / 票 08）：以**一条带标记的 user 轮**注入本轮转录
/// （与 [`COMPACTION_MARK`] / [`FOREMAN_WATCH_DIGEST_MARK`] 同一先例——进模型上下文才拦得住；
/// user 轮这一形态是必须的：值班长的转录里「该轮到谁说话」由最后一条 user 承担）。
///
/// 提醒本身**不落库**：落库的 user 行是值班经理说的话，把系统提醒写进去会让时间线上
/// 凭空多出一句「他说过的话」（前端只看角色，分不开）。提醒的证据留在两处——`tracing::warn!`
/// 那一条日志，和这一轮 `traces` 里那串重复的调用本身；真收口时还有收口行上的标记与
/// 写明的原因（[`FOREMAN_LOOP_TURN_MARK`]）。
pub const FOREMAN_LOOP_REMINDER_MARK: &str = "【操作台提醒·别在原地打转】";

/// 历史窗口的字符预算（决策 182⑫）。
///
/// **按字符裁剪而不是硬编码轮数**：一轮的长短差两个数量级（「在吗」3 字 vs 贴一段回执
/// 2000 字），按轮数裁会让长轮挤出上下文、短轮浪费预算。被裁掉的历史仍在库里（`list_foreman_messages`
/// 只影响这一轮注入了什么，不影响台账）。
pub const FOREMAN_HISTORY_BUDGET_CHARS: usize = 24_000;

/// 上下文压缩（决策 269 / 票 foreman-within-boundary 03）的锚点前缀标记。
/// 锚点以**一条 user 轮**的形态带头拼进历史头部（system 行重注入走 user 的先例，
/// 204 同姿态）；测试按它认锚点（`over_budget_history_is_summarized_…`）。
pub const COMPACTION_MARK: &str = "【更早的对话已压缩成下面这段摘要——原始轮次仍在班次台账里】";

/// 跨时间线互喂的两个标记（决策 289 / 票 03）：与 [`COMPACTION_MARK`] 同族——都是
/// 「以一条带标记的 user 轮注入的摘要」，进模型上下文才拦得住，落库之后还能审计
/// （摘要本身不落库，落库的是它各自的原始轮次）。
/// 人的那一轮读到**值守台账的摘要**：值班经理指着播报说「处理一下」时，模型知道
/// 说的是哪一件——而它看到的是摘要，不是整本流水账（预算的另一半不吃）。
pub const FOREMAN_WATCH_DIGEST_MARK: &str = "【值守摘要】";
/// 值守轮读到**人的对话的摘要**（裁决 2：它仍读得到人说的话——以摘要形态）。
pub const FOREMAN_TALK_DIGEST_MARK: &str = "【人的对话摘要】";

/// 摘要器的专用指令（269②：同一 `llm.complete`、同 provider 的**无工具**小补全）。
const SUMMARIZER_SYSTEM_PROMPT: &str = "你是对话历史压缩器。把给定的历史压成**一段摘要**，\
    作为值班长后续轮次的上下文锚点。保留：任务与结论、关键报错与证据原文（尽量短）、\
    值班经理给过的方向与决定、未决问题与承诺。丢弃：寒暄、重复、过程细节。\
    只输出摘要正文——不要标题、不要解释、不要列表符号。";
/// 摘要调用的绝对上限：它是主轮之外的一次附加调用，挂住不能把整轮拖死
/// （超时即回退现状，269④）。
const SUMMARIZER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// 摘要输出上限（锚点自身要能进预算算术，269②）。
const SUMMARIZER_MAX_TOKENS: u32 = 800;
const SUMMARIZER_MAX_CHARS: usize = 4_000;
/// 摘要输入上限：掉出预算的区间可能很长，超限**留尾巴**（离窗口最近的轮次最要紧）。
const SUMMARIZER_INPUT_MAX_CHARS: usize = 60_000;
/// 锚点缓存的会话槽数（FIFO 淘汰；重启重算可接受——锚点不落库，269③）。
const COMPACTION_CACHE_SLOTS: usize = 16;

/// 一条会话的压缩缓存（决策 269③）：`covered_until` = 摘要已覆盖到的消息 id
/// （第一条**没**被覆盖的那条）。窗口边界在会话内单调前进，下一轮从这里接着增量压
/// ——每条轮次一生只被压一次，最老原文不整段重发。
#[derive(Clone)]
struct CompactionEntry {
    covered_until: i64,
    summary: String,
}

/// 缓存本体：按会话分槽、FIFO 淘汰（上限 [`COMPACTION_CACHE_SLOTS`]）。
#[derive(Default)]
struct CompactionCache {
    entries: HashMap<String, CompactionEntry>,
    order: std::collections::VecDeque<String>,
}

impl CompactionCache {
    fn get(&self, session: &str) -> Option<&CompactionEntry> {
        self.entries.get(session)
    }

    fn put(&mut self, session: &str, entry: CompactionEntry) {
        if !self.entries.contains_key(session) {
            while self.order.len() >= COMPACTION_CACHE_SLOTS {
                if let Some(old) = self.order.pop_front() {
                    self.entries.remove(&old);
                }
            }
            self.order.push_back(session.to_string());
        }
        self.entries.insert(session.to_string(), entry);
    }
}

/// 摘要器的输入（269③）：已有摘要打头（增量）+ 新掉出预算的轮次逐条带发言者；
/// 整段超限时留尾巴——首条掉队的原文从此只活在旧摘要里，不整段重发。
fn summarize_input(newly: &[ForemanMessage], prefix: Option<&str>) -> String {
    let mut out = String::new();
    match prefix {
        Some(p) => {
            out.push_str("【已有摘要】\n");
            out.push_str(p);
            out.push_str("\n\n【新掉出预算的轮次】\n");
        }
        None => out.push_str("【掉出预算的对话历史】\n"),
    }
    for m in newly {
        let who = if m.role == crate::storage::foreman::FOREMAN_ROLE_USER {
            "值班经理"
        } else if m.role == crate::storage::foreman::FOREMAN_ROLE_SYSTEM {
            "操作台"
        } else {
            "值班长"
        };
        out.push_str(&format!("[{who}]\n{}\n\n", m.content));
    }
    let n = out.chars().count();
    if n > SUMMARIZER_INPUT_MAX_CHARS {
        let kept: String = out.chars().skip(n - SUMMARIZER_INPUT_MAX_CHARS).collect();
        format!("（更早部分略）\n{kept}")
    } else {
        out
    }
}

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

/// 一轮里**按发生顺序**记下的一步（决策 273）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ForemanSegment {
    /// 一次模型调用的推理原文。
    Thinking { text: String },
    /// 这一轮**中途**说出口的话（收口那一句是 `content` 列，不在段序里）。
    Text { text: String },
    /// 一次工具调用（含它成没成）。
    Tool {
        tool: String,
        args_summary: String,
        ok: bool,
    },
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

/// 值守轮的失败状态（决策 271）：两次尝试之间隔多久、这一批失败落过什么账。
///
/// **只在内存**：退避是「这一次进程的运行节奏」，不是跨重启的账。一批失败的次数与类别链
/// 也只在进程里活着——进程被杀则汇总行不出现，首行仍在，读起来是「失败过一次」：
/// 比 37 行诚实，也不假装完整（2026-09-24 实测里进程正是被杀在批中间）。
#[derive(Debug, Default)]
struct WatchFailureState {
    /// 连续失败次数（成功一次——含静默那一轮——即归零）。
    consecutive: u32,
    /// 下一次允许尝试的时刻；`None` = 不在退避里。
    next_attempt_at: Option<chrono::DateTime<chrono::Utc>>,
    /// 本批**最后落过账**的类别（去重键）：同类不再逐条落行，换类别才有下一行。
    noted_kind: Option<String>,
    /// 本批出现过的类别链（汇总行用）：去重、保序。
    burst_kinds: Vec<String>,
    /// 本批失败次数（含没落行的那些）。
    burst_count: u32,
    /// 本批最后一次失败的时刻。
    burst_last_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl WatchFailureState {
    /// 退了多久之后再试（秒）。取值是常量不是配置项——照决策 224 的姿态：没有第二种诉求
    /// 之前不扩契约。
    fn delay_secs(kind: &str, consecutive: u32) -> i64 {
        let (base, max) = if Self::waits_pointlessly(kind) {
            (
                FOREMAN_WATCH_RETRY_CONFIG_BASE_SECS,
                FOREMAN_WATCH_RETRY_CONFIG_MAX_SECS,
            )
        } else {
            (FOREMAN_WATCH_RETRY_BASE_SECS, FOREMAN_WATCH_RETRY_MAX_SECS)
        };
        let double = 1i64 << (consecutive.saturating_sub(1)).min(6);
        base.saturating_mul(double).min(max)
    }

    /// 「等一等不会自己好」的类别：账单 / 鉴权 / 模型名 / 上下文超窗 / 配置。
    ///
    /// 归在这一档不是「不重试」（provider 那边续了费它就该自己恢复），而是**换一个节奏**：
    /// 按网络的节奏重试余额不足，只是把噪声放大（2026-09-24 实测里前 20 行就是这个形状）。
    fn waits_pointlessly(kind: &str) -> bool {
        matches!(
            kind,
            "llm_auth" | "llm_model_not_found" | "llm_quota" | "llm_context_window" | "config"
        )
    }

    /// 记一次失败，并回答「**这一次要不要落行**」（本批第一次 / 换了类别 → 落）。
    fn note_failure(&mut self, kind: &str, now: chrono::DateTime<chrono::Utc>) -> bool {
        self.consecutive = self.consecutive.saturating_add(1);
        self.burst_count = self.burst_count.saturating_add(1);
        self.burst_last_at = Some(now);
        self.next_attempt_at =
            Some(now + chrono::Duration::seconds(Self::delay_secs(kind, self.consecutive)));
        if !self.burst_kinds.iter().any(|k| k == kind) {
            self.burst_kinds.push(kind.to_string());
        }
        let first_of_kind = self.noted_kind.as_deref() != Some(kind);
        if first_of_kind {
            self.noted_kind = Some(kind.to_string());
        }
        first_of_kind
    }

    /// 成功一轮之后把这一批的账取走（清空状态）——`None` = 上一轮没失败过，不用收口。
    ///
    /// 归零的是**连续失败计数**与整批记录：下一次失败从 30s 档重新起算。
    fn take_burst(&mut self) -> Option<(u32, Vec<String>, chrono::DateTime<chrono::Utc>)> {
        let taken = self
            .burst_last_at
            .map(|last| (self.burst_count, self.burst_kinds.clone(), last));
        self.consecutive = 0;
        self.next_attempt_at = None;
        self.noted_kind = None;
        self.burst_kinds.clear();
        self.burst_count = 0;
        self.burst_last_at = None;
        taken.filter(|(count, _, _)| *count > 0)
    }

    /// 现在还在退避里吗。
    fn waiting(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        self.next_attempt_at.is_some_and(|at| now < at)
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
    /// 上下文压缩的每会话锚点缓存（决策 269③）。锁只在读改写缓存时短暂持有——
    /// 摘要调用本身在锁外跑，绝不持锁跨 await。
    compaction: Mutex<CompactionCache>,
    /// 值守轮的失败状态（决策 271）：退避窗口 + 这一批失败落过什么账。
    /// 与 `compaction` 同一姿态：锁只在读改写时短暂持有，绝不持锁跨 await。
    watch_failures: Mutex<WatchFailureState>,
    /// 人的那一轮的在飞计数（决策 289 / 票 03，见 [`HumanTurns`]）。
    human_turns: HumanTurns,
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
        // 模型请求留痕（决策 231）：值班长的请求没有 run 行，归属靠 `session_id`——
        // 而「这一轮第几次调用、烧了多少字节、最后一次收字节是何时」正是它 2026-09-19
        // 那次实测里把它带偏（把活栈记在错的 run 名下）的那个缺口。
        let llm = Arc::new(crate::agent::recording::RecordingLlm::new(
            llm,
            store.clone(),
        ));
        ForemanRunner {
            store,
            settings,
            home,
            llm,
            sse,
            steward_actions: None,
            compaction: Mutex::new(CompactionCache::default()),
            watch_failures: Mutex::new(WatchFailureState::default()),
            human_turns: HumanTurns::default(),
        }
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

    /// 把「人的那一轮在跑」这个现场**直接摆出来**（决策 289 / 票 03 的排队判据接缝）。
    ///
    /// 生产里只有 [`Self::respond`] 的人的那一支会登记；值守轮不置它——这正是它与
    /// [`FOREMAN_TURNS`] 的分野。公开给测试是因为排队判据不靠真并发验：进程级旗子在
    /// 并跑的用例之间会互相排队（实测 4 个用例因此翻面），而「登记 → 值守让路 → 摘除 →
    /// 值守照常」这四拍在单线程里就是全部语义。
    pub fn begin_human_turn(&self) -> HumanTurnGuard {
        self.human_turns.begin()
    }

    /// 轮内压缩的触发线（决策 291 / 票 06(b)）：**窗口的 80%**。
    ///
    /// 复用流水线那套容量算术（[`crate::agent::context::estimate_context_capacity`] 的
    /// 系统/用户预留与 `OUTPUT_RESERVE`），只把触发线从软限 60% 抬到 80%——理由见票面：
    /// 压缩会打断 provider 的 prefix 缓存（2026-09-26 实测 94% 命中，是这套东西唯一便宜的
    /// 地方），压一次之后下一次调用几乎全量重算，故触发要**迟钝**（到窗口 ~80% 才压）。
    const FOREMAN_INLOOP_COMPACT_RATIO: f64 = 0.8;

    /// 这一轮的窗口容量（决策 291 / 票 06(b)）：provider 行的 `context_window` + 上面那份
    /// 算术。查不到（无可用 provider / 未登记窗口 / 读库失败）→ `None` = 跳过分档。
    ///
    /// **不因为查不到窗口就让这一轮失败**：流水线侧的显式失败（决策 110）守的是「超硬限
    /// 时挂 pending」那条路——对讲台没有那条路，而「窗口没登记」不该让一次对话说不了话。
    /// 真撞墙还有 (c) 那条恢复路兜着（靠 provider 自己的报错，不靠我们猜的窗口）。
    async fn turn_capacity(
        &self,
        cfg: Option<&crate::types::StageConfig>,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Option<crate::agent::context::ContextCapacity> {
        let providers = self.store.load_providers().await.ok()?;
        let fallback = providers.iter().find(|p| p.enabled).map(|p| p.id.as_str());
        let provider_id = crate::storage::catalog::resolve_provider_id(None, None, cfg, fallback)?;
        let provider = providers.into_iter().find(|p| p.id == provider_id)?;
        if provider.context_window == 0 {
            return None;
        }
        let mut capacity = crate::agent::context::estimate_context_capacity(
            provider.context_window as usize,
            system_prompt,
            user_prompt,
            &self.settings,
        );
        capacity.soft_limit = (capacity.total as f64 * Self::FOREMAN_INLOOP_COMPACT_RATIO) as usize;
        Some(capacity)
    }

    /// 本轮起点的下标（决策 291 / 票 06(b)）：`transcript` 里承载「这一轮要处理的那句话」
    /// 的那条消息。压缩拿它当锚点（[`crate::agent::context::compact_messages_from`] 的
    /// `current_start`）——载入的历史不得顶替它，否则真正的起点会被压成摘要。
    ///
    /// 按内容倒着找而不是记住一个下标：压缩会重排下标，而这句话本身不变。
    fn round_start_of(transcript: &[Message], question: &str) -> usize {
        transcript
            .iter()
            .rposition(|m| {
                m.role == crate::agent::client::Role::User && m.content.as_deref() == Some(question)
            })
            .unwrap_or(0)
    }

    /// 轮内预算门（票 06(b)）：过线就按轮压缩，返回压掉的段数（0 = 没触发 / 压不动）。
    ///
    /// 判据与流水线逐字同源（[`crate::agent::context::should_compact`] 对
    /// [`crate::agent::context::estimate_messages_tokens`] 的全文读数）——差别只有那条
    /// 触发线（80% 而不是软限 60%，理由见 [`Self::FOREMAN_INLOOP_COMPACT_RATIO`]）。
    fn compact_inline_if_over_budget(
        &self,
        transcript: &mut Vec<Message>,
        question: &str,
        system_prompt: &str,
        user_prompt: &str,
        capacity: Option<crate::agent::context::ContextCapacity>,
    ) -> usize {
        let Some(capacity) = capacity else {
            return 0;
        };
        let estimate =
            crate::agent::context::estimate_messages_tokens(system_prompt, user_prompt, transcript);
        if !crate::agent::context::should_compact(estimate, capacity) {
            return 0;
        }
        self.compact_inline_forced(transcript, question)
    }

    /// 无条件压一轮（票 06(b) 的触发与 (c) 的撞墙恢复共用）。
    fn compact_inline_forced(&self, transcript: &mut Vec<Message>, question: &str) -> usize {
        let before = transcript.len();
        let start = Self::round_start_of(transcript, question);
        let outcome = crate::agent::context::compact_messages_from(
            transcript,
            self.settings.keep_recent_rounds,
            start,
        );
        if outcome.compacted_messages == 0 {
            return 0;
        }
        tracing::info!(
            before,
            after = outcome.messages.len(),
            compacted = outcome.compacted_messages,
            "值班长轮内上下文超线，已按轮压缩（票 06(b)）"
        );
        *transcript = outcome.messages;
        outcome.compacted_messages
    }

    /// 回一句话，落进指定的会话。
    ///
    /// 超预算才摘要（决策 269 / 票 foreman-within-boundary 03）：把**掉出预算**的最老
    /// 区间压成一段锚点，锚点按上限**预留**进同一本预算（预留式算术，见方法内注释——
    /// 事后重裁会掉出一截没人摘要的轮次）；预算内逐字照旧
    /// （两条既有钉子走的就是未改动的 [`trim_history`]）、失败回退现状、DB 不动。
    ///
    /// 缓存按会话分槽（[`CompactionCache`]）：边界没动 → 直接复用、零 token；
    /// 边界前进 → 旧摘要 + 新掉队轮增量再压（每条轮次一生只被压一次）。
    /// 摘要失败/超时 → 不写缓存、不加锚点，窗口原样（269④——轮次绝不因摘要挂掉而挂掉）。
    async fn compact_history(
        &self,
        session_id: &str,
        history: &[ForemanMessage],
        budget_chars: usize,
        provider_id: Option<String>,
    ) -> (Vec<ForemanMessage>, Option<String>) {
        // 第一步照旧：按**原预算**裁一次——预算内逐字照旧（269①），连缓存都不碰；
        // 它同时是摘要失败时的回退窗口（现状 = 从头丢，269④）。
        let probe = trim_history(history, budget_chars);
        if probe.len() == history.len() {
            return (probe, None);
        }
        // 预算算术用**预留**而不是事后重裁：锚点长度要摘要完才知道，事后重裁会再掉
        // 一截**没人摘要**的轮次（先有鸡还是先有蛋）。按锚点上限（标记 +
        // SUMMARIZER_MAX_CHARS）预留头部，窗口裁完不再动——摘要覆盖的区间 = 最终掉出
        // 预算的区间，一条不漏，也不需要二段压。代价是锚点实际更短时窗口也不回填
        // （最多让出 4k 字的保守余量，24k 预算下可接受）。
        let reserve = COMPACTION_MARK.chars().count() + 1 + SUMMARIZER_MAX_CHARS;
        let window = trim_history(history, budget_chars.saturating_sub(reserve));
        let dropped_count = history.len() - window.len();
        let boundary = window[0].id;

        let entry = self
            .compaction
            .lock()
            .ok()
            .and_then(|cache| cache.get(session_id).cloned());

        let (summary, fresh) = match &entry {
            // 边界没动：没有新掉队的轮次，直接复用——一个 token 都不烧。
            Some(e) if e.covered_until == boundary => (e.summary.clone(), false),
            other => {
                let (prefix, newly) = match other {
                    Some(e) => match history.iter().position(|m| m.id == e.covered_until) {
                        Some(i) if i <= dropped_count => {
                            (Some(e.summary.as_str()), &history[i..dropped_count])
                        }
                        // 覆盖点找不到 / 反常后退：防御性全量重算（单调会话走不到）。
                        _ => (None, &history[..dropped_count]),
                    },
                    None => (None, &history[..dropped_count]),
                };
                let input = summarize_input(newly, prefix);
                let Some(s) = self
                    .summarize_interval(session_id, &input, provider_id)
                    .await
                else {
                    return (probe, None); // 回退现状（269④）：不写缓存，下一轮再试
                };
                (s, true)
            }
        };
        if fresh {
            if let Ok(mut cache) = self.compaction.lock() {
                cache.put(
                    session_id,
                    CompactionEntry {
                        covered_until: boundary,
                        summary: summary.clone(),
                    },
                );
            }
        }

        // 锚点自身计入预算算术（269②）——预留已在上面扣过，这里兑现承诺：
        debug_assert!(
            COMPACTION_MARK.chars().count() + 1 + summary.chars().count() <= reserve,
            "摘要器输出必须落在 SUMMARIZER_MAX_CHARS 的预留内"
        );
        (window, Some(summary))
    }

    /// 摘要调用本体：同一 `llm.complete`、同 provider、无工具的小补全（269②）。
    /// 超时 / 失败 / 空回一律 `None`（调用方回退现状）；它会作为一条按会话归属的
    /// 模型请求留在台账里（决策 231 的留痕口径——可见、无 run 行、不算作一轮）。
    async fn summarize_interval(
        &self,
        session_id: &str,
        input: &str,
        provider_id: Option<String>,
    ) -> Option<String> {
        let request = LlmRequest {
            stage: Stage::Init,
            node: Node::Execute,
            attempt: 1,
            system_prompt: SUMMARIZER_SYSTEM_PROMPT.to_string(),
            user_prompt: input.to_string(),
            messages: Vec::new(),
            tools: Vec::new(),
            temperature: Some(0.2),
            max_tokens: Some(SUMMARIZER_MAX_TOKENS),
            provider_id,
            run: Some(crate::agent::client::RunContext {
                task_id: String::new(),
                branch: String::new(),
                run_id: 0,
                agent_type: FOREMAN_AGENT_TYPE.to_string(),
                session_id: session_id.to_string(),
            }),
            idle_timeout_sec: None,
        };
        let response = tokio::time::timeout(SUMMARIZER_TIMEOUT, self.llm.complete(request))
            .await
            .ok()?
            .ok()?;
        let text = response.content?.trim().to_string();
        if text.is_empty() {
            return None;
        }
        if text.chars().count() > SUMMARIZER_MAX_CHARS {
            let clipped: String = text.chars().take(SUMMARIZER_MAX_CHARS).collect();
            Some(format!("{clipped}…"))
        } else {
            Some(text)
        }
    }

    /// 跨时间线互喂（决策 289 / 票 03）：把**对方那条时间线**的摘要填进 `out`。
    ///
    /// - 人的那一轮 → 值守台账的摘要（[`FOREMAN_WATCH_DIGEST_MARK`]）：值守轮醒过几次、
    ///   看到了什么、怎么收的场——人指着播报说「处理一下」时模型才知道说的是哪件。
    ///   值守台账还不存在（值守轮一次都没醒过）就不注入，也不**顺手建**它——
    ///   那本台账归值守轮所有。
    /// - 值守轮 → 最近活动的人的班次的摘要（[`FOREMAN_TALK_DIGEST_MARK`]）：裁决 2 的
    ///   「它仍读得到人说的话」，以摘要形态（不是整本原文）。
    ///
    /// 摘要机器复用 [`Self::compact_history`] 那一套（[`CompactionCache`] 增量缓存 +
    /// [`Self::summarize_interval`]）：每条轮次一生只被压一次，边界没动零 token；
    /// 摘要失败 → 不注入、不报错（轮次绝不因摘要挂掉而挂掉，269④ 同一姿态）。
    async fn inject_cross_digest(
        &self,
        out: &mut Vec<Message>,
        input: &TurnInput,
        provider_id: Option<String>,
    ) {
        enum Source {
            Talk,
            Watch,
        }
        let (source, mark) = if input.is_watch() {
            (Source::Talk, FOREMAN_TALK_DIGEST_MARK)
        } else {
            (Source::Watch, FOREMAN_WATCH_DIGEST_MARK)
        };
        // 两条来源各取各的「最近」，读不到就跳过（都不**顺手建**行：值守台账归值守轮所有，
        // 人还没说过话时也没有可摘要的东西）。
        let source_session = match source {
            Source::Talk => match self.store.latest_foreman_session().await {
                Ok(Some(session)) => session,
                Ok(None) => return,
                Err(e) => {
                    tracing::warn!("互喂摘要读不到人的班次：{e}");
                    return;
                }
            },
            Source::Watch => {
                match self
                    .store
                    .latest_foreman_session_of_kind(FOREMAN_SESSION_KIND_WATCH)
                    .await
                {
                    Ok(Some(session)) => session,
                    Ok(None) => return,
                    Err(e) => {
                        tracing::warn!("互喂摘要读不到值守台账：{e}");
                        return;
                    }
                }
            }
        };
        let history = match self
            .store
            .list_foreman_messages(&source_session.id, FOREMAN_HISTORY_FETCH_LIMIT)
            .await
        {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!(session = %source_session.id, "互喂摘要读不到台账：{e}");
                return;
            }
        };
        let cache_key = format!("cross:{}", source_session.id);
        let Some(summary) = self
            .cross_digest(&cache_key, &source_session.id, &history, provider_id)
            .await
        else {
            return;
        };
        out.push(Message::user(format!(
            "{mark}（以下是对方时间线的摘要，不是原文；原始轮次在它自己的台账里）\n{summary}"
        )));
    }

    /// 摘要的本体：增量缓存 + 一次无工具小补全（[`Self::summarize_interval`]）。
    ///
    /// 形状与 [`Self::compact_history`] 的锚点那一半同构，差别只有一处：这里**没有**
    /// 「窗口」——摘要覆盖到源时间线的**最新一行**（跨时间线要的是全景，不是预算内的尾巴）。
    /// `covered_until` 之外的增量只有新掉队的那些；覆盖点找不到（行被清理）就整段重算。
    async fn cross_digest(
        &self,
        cache_key: &str,
        source_session_id: &str,
        history: &[ForemanMessage],
        provider_id: Option<String>,
    ) -> Option<String> {
        let last = history.last()?;
        let boundary = last.id;
        let entry = self
            .compaction
            .lock()
            .ok()
            .and_then(|cache| cache.get(cache_key).cloned());
        let (summary, fresh) = match &entry {
            // 边界没动：对方没有新动静，直接复用——一个 token 都不烧。
            Some(e) if e.covered_until == boundary => (e.summary.clone(), false),
            other => {
                let (prefix, newly) = match other {
                    Some(e) => match history.iter().position(|m| m.id == e.covered_until) {
                        Some(i) => (Some(e.summary.as_str()), &history[i + 1..]),
                        // 覆盖点找不到（行被清理）：防御性全量重算。
                        _ => (None, history),
                    },
                    None => (None, history),
                };
                let input = summarize_input(newly, prefix);
                // 留痕按**源会话**归属（决策 231 的口径）：这次摘要调用读的是那本台账。
                let summary = self
                    .summarize_interval(source_session_id, &input, provider_id)
                    .await?;
                (summary, true)
            }
        };
        if fresh {
            if let Ok(mut cache) = self.compaction.lock() {
                cache.put(
                    cache_key,
                    CompactionEntry {
                        covered_until: boundary,
                        summary: summary.clone(),
                    },
                );
            }
        }
        Some(summary)
    }

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
        // 这一轮从这一刻算起（决策 233③：一轮死了要作废**它那一轮**提的提议）——
        // 用「刚落库的这句人话的时刻」而不是墙上现在，因为提议是在这之后才可能落库的。
        let round_started_at = self.store.now();
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
            // 人的轮：永远落账（没有「同批合并」这回事——人每一次都该看到自己那句话的结果），
            // 也不受值守轮那条退避管（决策 271：退避只挡值守轮）。
            self.record_failed_turn(&session, error, round_started_at, false, true)
                .await;
        }
        // 值班长回话完成 → 离线通知（决策 272②③）：第二触发面，**不**写 attention 表。
        // 门（`traces.len()`）在 notify() 之前过——短轮连 cooldown 槽都不碰。
        if let Ok(turn) = &result {
            self.notify_reply_completed(turn, true);
        }
        result
    }

    /// 一轮回话的**外框**（决策 288 / 票 05）：登记「这一班有一轮在跑」，然后进内里。
    ///
    /// **这一轮不再有整轮墙钟**（显式兑现决策 233 的如实记 (i)：「抬墙钟另立一条」）：
    /// 2026-09-26 的实测里一轮 23 次调用**全部成功**仍被 30 分钟墙钟整段砍掉——墙钟杀的是
    /// 「跑了很久但一直在干活」，而真正该杀的是「一个字节都没有」。后者的界在
    /// [`Self::respond_inner`] 的逐调用空闲判死上（foreman 行的 `idle_timeout_sec`，
    /// 缺省与节点同数 300s）；前者的终点由人决定（值守轮由 token 预算收口，票 07）。
    async fn respond(&self, session: &ForemanSession, input: TurnInput) -> Result<ForemanTurn> {
        // 「这一班有一轮在跑」的登记（决策 260）就落在这个**唯一漏斗**上：`say` 与
        // `watch` 都过它，故两条路各写一遍的漂移从形状上不可能。凭据在函数返回（含
        // 提前 `?` 退出）时随 `Drop` 摘掉——登记与一轮的真实寿命因此是同一条。
        let _turn = begin_foreman_turn(&session.id);
        // 人的那一轮的独立在飞标志（决策 289 / 票 03）：值守轮据此排队，不并行。
        // 凭据同样退出即摘——登记与一轮的真实寿命是同一条。
        let _human = (!input.is_watch()).then(|| self.human_turns.begin());
        let cfg = self.stage_config().await?;
        self.respond_inner(session, input, cfg).await
    }

    /// 这一轮的**轮数上限**（决策 233① / 239）：`stage_configs` 的 `foreman` 行配了就用它，
    /// 没配过用缺省 [`FOREMAN_MAX_ROUNDS`]。
    ///
    /// 墙钟已撤（决策 288 / 票 05）：它不再是「另一个界」，只是模型行为失控时的兜底
    /// （真正的预算界见票 07 的 token 分档）。解析**只有这一处**——写入路径只收正整数，读回来是 `None` = 没配过（`0` / 负数在
    /// `StageConfigRow::into_config` 里已经被挡在门外，而 `validate_startup` 对存量里的
    /// `0` 直接拒绝启动）。
    fn round_limit(&self, cfg: Option<&crate::types::StageConfig>) -> usize {
        cfg.and_then(|c| c.max_rounds)
            .map(|v| v as usize)
            .unwrap_or(FOREMAN_MAX_ROUNDS)
    }

    /// 这一轮的**成本线**（决策 292 / 票 07）：`stage_configs` 的 `foreman` 行配了就用它，
    /// 没配过用缺省 [`FOREMAN_WATCH_TOKEN_BUDGET`]。
    ///
    /// 它是**生成 token** 的线（不是 prompt token）：一轮的真实成本 = 生成 token ÷ provider
    /// 吞吐（2026-09-26 实测三个链都稳定在 ~45 token/s，与上下文大小无关），故「能烧多少」
    /// 由它量。分档在调用点：值守轮触顶即停，人的那一轮只落一条软告警。
    ///
    /// 解析**只有这一处**——写入路径只收正整数，读回来 `None` = 没配过（`0` / 负数在
    /// `StageConfigRow::into_config` 里已被挡在门外，`validate_startup` 对存量里的 `0`
    /// 直接拒绝启动）。
    fn token_budget(&self, cfg: Option<&crate::types::StageConfig>) -> u32 {
        cfg.and_then(|c| c.watch_token_budget)
            .unwrap_or(FOREMAN_WATCH_TOKEN_BUDGET)
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
        // 环境层档位（决策 206）：广告集、执行点白名单与人格里的工具纪律段**同源**——
        // 由下面的 `available` 筛**一次**，三处消费者各取所需（决策 247 兑现了这句注释）。
        // 缺省 `ask`（值班长的输入是人可以随便打的任意文本）。
        let env_mode = crate::types::effective_env_mode(
            self.settings.env_mode,
            FOREMAN_STAGE_KEY,
            cfg.as_ref(),
        );
        // 分级诊断（票 07）：自动那一轮摘掉贵的两件（会话原文 / 跑命令）。**先算它**——
        // 紧接着 `available` 算**一次**，三处消费者（广告集 / 执行点白名单 / 工具纪律段）
        // 吃同一份（决策 247）。顺序是这条承诺的全部：从前 `deny` 晚于 `system_prompt`
        // 才算，纪律段于是广告着一个这一轮已被摘掉的工具——模型被告知去调一个必被拒的
        // 名字，而这种错从现象上与「闸门坏了」分不开（架构评审候选 7 抓到的顺序 bug）。
        let deny: &[&str] = if input.is_watch() {
            &FOREMAN_WATCH_TOOL_DENY
        } else {
            &[]
        };
        let available = foreman_available_tools_except(env_mode, deny);
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
        let system_prompt = self.system_prompt(cfg.as_ref(), env_mode, stewarded, &available)?;
        let provider_id =
            crate::storage::catalog::resolve_provider_id(None, None, cfg.as_ref(), None);

        let history = self
            .store
            .list_foreman_messages(&session.id, FOREMAN_HISTORY_FETCH_LIMIT)
            .await?;
        // 超预算才摘要（决策 269 / 票 03）：预算内逐字照旧；跨线时把掉出预算的最老
        // 区间压成锚点（会话缓存增量、失败回退现状），锚点与窗口共用同一本预算。
        // 值守轮同路径（共用 respond），零分支（269⑤）。
        let (window, anchor) = self
            .compact_history(
                &session.id,
                &history,
                FOREMAN_HISTORY_BUDGET_CHARS,
                provider_id.clone(),
            )
            .await;
        // `user_prompt` **只放快照**，问题由 transcript 的最后一条承担。
        //
        // 适配器组装的 body 是 `[system][user(user_prompt)] + messages`（见
        // `openai.rs::build_body`），所以把问题同时写进 user_prompt 和在 transcript 里
        // 再带一遍，模型会连着看到同一个问题两三次——白烧 token，还会让它以为是不同的话。
        // 快照放 user_prompt 而不是塞进 system：它每轮都变，进系统段会让 prompt cache
        // 每轮全失效（§12.13.5）。
        let user_prompt = briefing.render();

        // 分级诊断摘在**源头上**（决策 247）：`deny` 早于三处消费者算好，广告集、
        // 执行点白名单与纪律段都吃 `available`，故「模型看得见一个调用就被拒的工具」
        // 这件事在自动轮里同样不会发生。
        // 问话载荷槽（决策 265）：每轮新建一个——工具写、本轮收口时取走挂到 assistant 行。
        let ask_slot: Arc<tokio::sync::Mutex<Option<serde_json::Value>>> =
            Arc::new(tokio::sync::Mutex::new(None));
        let (tools, ctx) = foreman_tooling(
            &self.store,
            &self.settings,
            &self.home,
            self.sse.clone(),
            &session.id,
            env_mode,
            ForemanMoment::Conversation,
            &available,
            self.steward_actions.clone(),
            // 人的那一轮放开台账读数（票 06(a)）：值守轮按摘要形态读（分级纪律）。
            !input.is_watch(),
        );
        let tools = tools.with_ask_slot(ask_slot.clone());

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
        let mut head_inserts: Vec<Message> = Vec::new();
        if let Some(anchor) = anchor {
            // 锚点以**标记 user 轮**带头插在历史头部（system 行重注入走 user 的先例）。
            // 两种适配器都不会把它错位：OpenAI 把 `[system, user_prompt]` 拼在 messages
            // 之前、Anthropic 抽走 system 段后 user_prompt 仍占 wire 头——锚点只是 messages
            // 的第一条普通 user 轮（providers 的 `messages[0]` 只有 mock 在读，且读的是
            // 组装后的 wire 头，不受影响）。
            head_inserts.push(Message::user(format!("{COMPACTION_MARK}\n{anchor}")));
        }
        // 跨时间线互喂（决策 289 / 票 03）：两条时间线各喂对方一份**摘要**，都以
        // 带标记的 user 轮注入（与锚点、操作台记账轮同一先例）——进上下文才拦得住，
        // 且两边都不是整本原文（预算不吃第二份）。锚点之后、正文之前。
        self.inject_cross_digest(&mut head_inserts, &input, provider_id.clone())
            .await;
        transcript.splice(0..0, head_inserts);
        // 历史最后一条就是刚落库的这句 user 消息；但若它被 `trim_history` 之外的原因
        // 漏掉（例如库被外部清空），仍要保证本轮的问题在场。
        // 值守简报**总是**追加成最后一条：它带署名（`TurnInput::transcript_text`），
        // 而历史里最后一条也是 user（刚落的用户行）时不能靠「已经有了」跳过它——
        // 那会让这一轮真正要处理的东西消失。人的话反过来：历史里最后一条就是它。
        // 这一轮要处理的那句话（本轮起点）：人格里那句「问题由 transcript 的最后一条承担」
        // 说的就是它。两处用它——末尾那条消息由它兜底（见上），轮内压缩拿它当锚点（票 06(b)）。
        let question = input.transcript_text();
        match (&input, transcript.last()) {
            (TurnInput::Human(text), Some(m)) if m.role == crate::agent::client::Role::User => {
                let _ = text;
            }
            _ => transcript.push(Message::user(question.clone())),
        }

        let tool_defs = Self::tool_defs(&available);
        let mut tokens = (0u32, 0u32);
        let mut traces: Vec<ForemanTrace> = Vec::new();
        // 这一轮**按发生顺序**的步骤（决策 273）。
        // 上面三份（`thinking` / `traces_json` / `content`）都是聚合视图，各自只剩一类东西；
        // 顺序一丢，界面就答不出「先想了什么、再查了什么、然后说了什么」。故另记一份段序，
        // 与聚合列并存——聚合列照旧服务各自的消费者，段序服务时间线。
        let mut segments: Vec<ForemanSegment> = Vec::new();
        let mut reply: Option<String> = None;
        // 这一轮里各次模型调用产出的推理原文，按顺序拼起来（决策 244）。
        // **跨轮累积**：一轮可能调用模型好几次（先思考再查台账再收口），而界面上那条
        // 折叠块说的是「这一轮它想了什么」，不是「最后一次调用想了什么」。
        let mut thinking = String::new();
        // 空内容与「轮数耗尽」是两回事，报错必须分得开——否则一次「模型返回空」会被
        // 说成「它可能一直在查台账」，把人引到完全错误的方向上去查。
        let mut empty_replies = 0usize;
        // 到目前为此它说过的**最后一句有内容的话**（决策 233②）：触到上限时不再整轮作废，
        // 把这一段带上标注落库。它会随工具调用一起出现（模型一边查一边说），故与 `reply`
        // 分开记——`reply` 只在「这一轮收口了」时赋值。
        let mut last_text: Option<String> = None;
        let round_limit = self.round_limit(cfg.as_ref());
        // 成本线（决策 292 / 票 07）：值守轮触顶即停（下面 `break` 走收口路径），
        // 人的那一轮只在这条线上落一条软告警（`cost_warned`，收口时附在该轮台账上）。
        let token_line = self.token_budget(cfg.as_ref());
        let mut cost_warned = false;
        // 这一轮**为什么出循环**（票 07）：`None` = 正常收口（模型自己说完了）或轮数触顶
        // （那一条的文案在下面现成拼，见 `reply` 的收口）。
        let mut stop: Option<StopReason> = None;
        // 循环检测（决策 293 / 票 08）的流水：这一轮每一次工具调用的（工具 + 参数原串 + 结果指纹）。
        // 判等用**参数原串**而不是 `traces` 里那份截断过的 `args_summary`——摘要会把两个
        // 不同的调用判成同一个，而误伤的代价正是它要防的那件事（好轮被收口）。
        let mut loop_log: Vec<crate::agent::loops::CallRecord> = Vec::new();
        // 提醒注入点的**下一位**：`Some(n)` = 已经提醒过，从此只判 `loop_log[n..]`
        // （「提醒过了再犯」与「第一次犯」用同一个函数、同一把尺子，只是换了窗口）。
        let mut loop_reminded_at: Option<usize> = None;
        // 逐调用空闲界（决策 288 / 票 05）：foreman 行的 `idle_timeout_sec` > 全局
        // `node_idle_timeout_sec`（缺省 300s，与节点同一个数）。每一次模型调用各带一份
        // ——「N 秒没有新字节」判的是单次调用，不是整轮。
        let idle_timeout_secs = crate::config::effective_idle_timeout(
            self.settings.node_idle_timeout_sec,
            cfg.as_ref().and_then(|c| c.idle_timeout_sec),
            crate::config::NodeTimeouts::default(),
        );
        // 轮内窗口预算（决策 291 / 票 06(b)）：**把已经造好的那套机器接上**——流水线的
        // 容量算术（系统/用户两段预留 + `OUTPUT_RESERVE` + 软硬限）对讲台此前一次都没读过。
        // 窗口来自 provider 行（`context_window`）；查不到（无可用 provider / 未登记）→
        // `None` = 跳过分档，不臆造窗口（决策 110 的姿态）——撞墙那条路（c）还在。
        let capacity = self
            .turn_capacity(cfg.as_ref(), &system_prompt, &user_prompt)
            .await;

        for _ in 0..round_limit {
            // 每次调用前查一次预算（票 06(b)）：到线就按轮压缩（规则化、不调 LLM）。
            // **查在组装请求之前**，故这一轮发出去的已经是压过的那一份。
            self.compact_inline_if_over_budget(
                &mut transcript,
                &question,
                &system_prompt,
                &user_prompt,
                capacity,
            );
            // 成本门（决策 292 / 票 07）：每次调用前查一次——与上面那条窗口门同一位置，
            // 故「已经不划算的下一轮」根本不会发出去。**分档**：值守轮触顶即停（出循环走
            // 收口路径，部分结论 + 【未收口】）；人的那一轮无硬界（终点由人决定），
            // 只把这条线记下来，收口时落一条软告警（只落账不拦）。
            if tokens.1 >= token_line {
                if input.is_watch() {
                    stop = Some(StopReason::Budget(tokens.1));
                    break;
                }
                cost_warned = true;
            }
            let mut request = LlmRequest {
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
                idle_timeout_sec: Some(idle_timeout_secs),
            };
            let response = match self.complete_with_retry(request.clone()).await {
                Ok(response) => response,
                // 撞墙恢复（决策 291 / 票 06(c)）：provider 报上下文超长时**不原地判败**
                // ——把这一轮的转录压一遍再重试这一次调用（只一次；再撞就是真的放不下，
                // 那时报错才是诚实的）。压缩是**无条件**的：这个错误说明算术低估了
                // （真 tokenizer 与 4 字符≈1 的估算、工具定义都占窗口），不按触发线走。
                Err(e) if is_context_window(&e) => {
                    let compacted = self.compact_inline_forced(&mut transcript, &question);
                    if compacted == 0 {
                        stop = Some(StopReason::Failed(e));
                        break;
                    }
                    tracing::warn!(
                        session = %session.id,
                        compacted,
                        "provider 报上下文超长：压缩本轮转录后重试这一次调用（票 06(c)）"
                    );
                    request.messages = transcript.clone();
                    match self.complete_with_retry(request).await {
                        Ok(response) => response,
                        Err(e) => {
                            stop = Some(StopReason::Failed(e));
                            break;
                        }
                    }
                }
                // 中途失败（决策 292 / 票 07）：**不原地把这一轮丢掉**——出循环走收口路径
                // （有话说就带上标注落库），错误原样带去外框做失败记账（类别 / 通知 /
                // 悬空提议作废都不变）。
                Err(e) => {
                    stop = Some(StopReason::Failed(e));
                    break;
                }
            };
            tokens.0 += response.prompt_tokens;
            tokens.1 += response.completion_tokens;
            // 推理原文（决策 244）：**只攒起来展示，不回灌**——既不进 `transcript`，
            // 也不进下一轮的 prompt（它不是 assistant 消息的一部分）。
            if let Some(thought) = response.reasoning.as_deref().filter(|t| !t.is_empty()) {
                if !thinking.is_empty() {
                    thinking.push_str("\n\n");
                }
                thinking.push_str(thought);
                // 段序里它就在这次调用的位置上（决策 273）：聚合的 `thinking` 装得下原文，
                // 装不下「它是在哪次工具调用之前想的」。
                segments.push(ForemanSegment::Thinking {
                    text: thought.to_string(),
                });
            }
            transcript.push(Message::assistant(
                response.content.clone(),
                response.tool_calls.clone(),
            ));
            if let Some(text) = response
                .content
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
            {
                last_text = Some(text.to_string());
            }

            if response.tool_calls.is_empty() {
                reply = response.content.filter(|s| !s.trim().is_empty());
                if reply.is_none() {
                    empty_replies += 1;
                }
                break;
            }
            // 走到这里说明这次调用**带工具调用**，故它说出口的正文是「中途的话」而不是
            // 收口那一句（收口那句由 `content` 列承载，见下面 `reply` 的赋值）——决策 273。
            // 判据与 `last_text` 同一姿态（trim 后判空）：两处认的是同一件事。
            // 顺序上它排在这批工具之前：真实发生的就是先说话、再调工具。
            if let Some(text) = response
                .content
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
            {
                segments.push(ForemanSegment::Text {
                    text: text.to_string(),
                });
            }
            for call in &response.tool_calls {
                let args_summary = summarize_args(&call.arguments);
                // 工具调用**当场**推给界面（决策 244）：落库的 `traces_json` 要等到这一轮
                // 收口才写，而诉求正是「不要在对话完结后才展示」。两处记的是同一件事的两种
                // 时态——实时事件说「此刻在查什么」，落库痕迹说「这一轮查过什么」（审计）。
                self.emit_tool_event(&session.id, &call.name, ToolPhase::Start, &args_summary);
                let (content, ok) = match self.run_tool(&tools, call, &ctx).await {
                    Ok(outcome) => (outcome.content, true),
                    // 工具失败**不**上升为整次回话失败（§12.8 的同一姿态）：
                    // 把错误文本回给模型让它改道，而不是让人看到一条报错。
                    Err(e) => (format!("工具执行失败：{e}"), false),
                };
                self.emit_tool_event(
                    &session.id,
                    &call.name,
                    if ok { ToolPhase::End } else { ToolPhase::Error },
                    &args_summary,
                );
                // 同一个工具事件在两处各记一份（决策 273）：`traces` 是「查过什么」的聚合，
                // `segments` 要的是它在整轮里的**位置**。摘要先给段序，再交给聚合那一份。
                segments.push(ForemanSegment::Tool {
                    tool: call.name.clone(),
                    args_summary: args_summary.clone(),
                    ok,
                });
                traces.push(ForemanTrace {
                    tool: call.name.clone(),
                    args_summary,
                    ok,
                });
                // 回灌的上限（决策 291 / 票 06(a)）：值守轮照旧按 12k 预截；人的那一轮
                // **逐字回灌**——体量已由 L2 卸载管住（大结果只剩回执 + 路径），再截一次
                // 只会把「卸载了可以回读」这件事变成假话（截掉的那半没有路径可回读）。
                let content = if input.is_watch() {
                    truncate(&content)
                } else {
                    content
                };
                // 循环检测的流水（决策 293 / 票 08）：结果指纹要在 `content` 被移进转录**之前**
                // 算——判定它「有没有读到新东西」看的正是回灌给模型的那一份。
                loop_log.push(crate::agent::loops::CallRecord {
                    tool: call.name.clone(),
                    arguments: call.arguments.clone(),
                    result_digest: crate::agent::loops::result_digest(&content),
                });
                transcript.push(Message::tool_result(call, content));
            }
            // 循环检测（决策 293 / 票 08）：**一批工具跑完再判**——工具结果必须紧跟发起它们的
            // assistant 消息（转录的契约），中途插一条 user 轮会把那一批切成两半。
            if let Some(hit) =
                crate::agent::loops::detect(&loop_log[loop_reminded_at.unwrap_or(0)..])
            {
                let reason = hit.reason();
                match loop_reminded_at {
                    // 第一次：注入一条带标记的 user 轮提醒，**只提醒一次**、不拦——模型多半
                    // 只是没意识到自己在重复。提醒进转录（下一轮调用带着它出去）才拦得住。
                    None => {
                        tracing::warn!(session = %session.id, %reason, "值班长在原地打转：注入提醒（票 08）");
                        transcript.push(Message::user(format!(
                            "{FOREMAN_LOOP_REMINDER_MARK}{reason}。若现有信息已经够回答这一轮的\
                             问题，现在就收口；若确实还要查，**换个角度**——别再重复刚才那几个调用。"
                        )));
                        loop_reminded_at = Some(loop_log.len());
                    }
                    // 提醒过了仍在打转：收口（部分结论 + 【未收口·在打转】），不再陪着烧 token。
                    Some(_) => {
                        tracing::warn!(session = %session.id, %reason, "提醒之后仍在打转：强制收口（票 08）");
                        stop = Some(StopReason::Loop(hit));
                        break;
                    }
                }
            }
        }

        // 触到上限**不再整轮作废**（决策 233②）：那 30 轮其实查到了东西（实测里两条提议
        // 就是证明），把已经付过钱的结论整段扔掉是那次实测里最贵的一件事。故只要有话说，
        // 就带上标注落库；**一句话都没说过的**仍旧按失败处置（没有东西可留，报错才是诚实的）。
        //
        // 决策 292 / 票 07 把这一支扩成**一条收口路径**：轮数上限 / token 预算 / 中途失败
        // 三条非正常结束共用它（票 08 的循环、票 09 的按停接在后面）——差别只有「为什么」。
        // 失败那一条同时把错误**原样带回**外框，失败记账（类别 / 通知 / 悬空提议作废）不变。
        // 中途失败但有话说：**先把那一轮落库再报错**（票 07 的全部意义——不许把已经查到
        // 的东西整段丢掉）。`stop_error` 就是原样的那个错误，由下面的 `?` 带给外框。
        let (reply, stop_error) = match (reply, stop) {
            // 模型自己收口了：正常那一句（预算门在它之后才可能踩线，故这里不看 `stop`）。
            (Some(reply), _) => (reply, None),
            (None, stop) if last_text.is_some() && empty_replies == 0 => {
                let partial = last_text.unwrap_or_default();
                // 这段文字此前已作为 `Text` 段落进过段序（它正是随工具调用一起说出口的
                // 「中途的话」，决策 233②），而现在它被拼成**收口的那一句**落进 `content`
                // 列——收口的话不在段序里（决策 273），故把它从段序尾部弹掉。
                // 只弹**尾部且内容一致**的那一段：它就是这句话本尊；内容不符说明段序里的是
                // 另一次说话，错弹会把「中途说过什么」抹掉，那比多留一段更坏。
                let closes_with_the_same_text = matches!(
                    segments.last(),
                    Some(ForemanSegment::Text { text }) if text == &partial
                );
                if closes_with_the_same_text {
                    segments.pop();
                }
                // 「为什么没说完」与**挂哪个标记**按停机原因分（票 07 / 08）：打转那条有自己的
                // 标记（与【未收口】同族、形状不同——裁决 9 让三种停法一眼分得开），其余共用它。
                let (mark, why, error) = match stop {
                    Some(StopReason::Budget(used)) => (
                        FOREMAN_PARTIAL_TURN_MARK,
                        format!(
                            "这一轮的 token 预算到了（{token_line} 生成 token，已烧 {used}），\
                             话没说完——以上是已经确定的部分。要接着查可以让我再来一轮（带上线索）。"
                        ),
                        None,
                    ),
                    Some(StopReason::Loop(hit)) => (
                        FOREMAN_LOOP_TURN_MARK,
                        format!(
                            "{}——提醒过一次仍未改道，这一轮我就停了。\
                             以上是已经确定的部分。要接着查可以让我再来一轮（换个线索）。",
                            hit.reason()
                        ),
                        None,
                    ),
                    Some(StopReason::Failed(e)) => {
                        let reason = turn_failure_reason(&e).1;
                        (
                            FOREMAN_PARTIAL_TURN_MARK,
                            format!(
                                "这一轮中途断了（{reason}），话没说完——以上是已经确定的部分。\
                                 要接着查可以让我再来一轮（带上线索）。"
                            ),
                            Some(e),
                        )
                    }
                    // 轮数上限（决策 233② 的那条老路）：循环自然跑完，`stop` 为空。
                    None => (
                        FOREMAN_PARTIAL_TURN_MARK,
                        format!(
                            "这一轮到了 {round_limit} 轮的收口上限，\
                             话没说完——以上是已经确定的部分。要接着查可以让我再来一轮（带上线索）。"
                        ),
                        None,
                    ),
                };
                tracing::warn!(
                    round_limit,
                    token_line,
                    used = tokens.1,
                    "值班长没能收口：部分结论落库并标注（决策 233② / 292 / 293）"
                );
                (format!("{partial}\n\n{mark}{why}"), error)
            }
            // 中途失败且**一句有内容的话都没说过**：没有东西可留，错误原样带回。
            (None, Some(StopReason::Failed(e))) => return Err(e),
            // 打转到底**一句话都没说过**：同样没有部分结论可留（与上面那一支同姿态），
            // 但归因要说实话——「它在原地打转」与「它一直在查台账」是两件事，指错方向
            // 会让人去查一个不存在的毛病。
            (None, Some(StopReason::Loop(hit))) => {
                return Err(Error::LlmClassified {
                    kind: "model_looping".into(),
                    message: format!("值班长在原地打转（{}），一句话都没说就停了", hit.reason()),
                    raw: format!("循环检测命中：{}", hit.reason()),
                });
            }
            (None, _) => {
                // 归因**走 `LlmClassified` 的 kind 机制**而不是新造一种错误（票 04）：
                // 这两条是模型行为，不是内部故障，而「哪一类」正是排查要的入口。
                return Err(if empty_replies > 0 {
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
                            "值班长在 {round_limit} 轮内没有给出回话——它可能一直在查台账"
                        ),
                        raw: format!("达到轮数上限 = {round_limit} 仍未收口"),
                    }
                });
            }
        };

        let traces_json = if traces.is_empty() {
            None
        } else {
            Some(serde_json::to_value(&traces)?)
        };
        // 顺序留痕（决策 273）：与 `traces_json` 同一口径——没有任何一步时存 `None`。
        // 「这一轮什么都没记下」与「记下了一个空序列」是两件事：前者下发时该是 null。
        let segments_json = if segments.is_empty() {
            None
        } else {
            Some(serde_json::to_value(&segments)?)
        };
        // 播报的标记由**后端**加，而模型会从历史里学会自己写一份（2026-09-25 实测：库里存成
        // 「【值守播报】【值守播报】**无需你处置。**…」）。故这里先剥一遍，再判静默、再加——
        // 标记只加一次。剥的自始至终是**我们自己**的标记，不是模型的措辞：剥完仍以别的字样
        // 开头就照常播报（§2.4 的静默判据偏向播报，本改动不动它）。
        let reply = if input.is_watch() {
            reply
                .trim_start()
                .strip_prefix(FOREMAN_WATCH_MARK)
                .map(str::trim_start)
                .unwrap_or_else(|| reply.trim_start())
                .to_string()
        } else {
            reply
        };
        // 静默规则（决策 209④ / §2.4）：值守轮判定「无需处理」时不落**播报**——
        // 一次自愈的风吹草动不该变成一条消息，而消息本身会挤占历史窗口预算（24k 字符）。
        // 痕迹留在日志里；台账那一栏的「我处理过没有」由待办表的 `consumed_at` 回答。
        //
        // 判据吃的是**剥完之后**的正文，且 `watch()` 那边按同一个 `turn.reply` 再判一次
        // （那里决定唤醒账记静默还是播报）——两处因此看的是同一个东西。
        // 中途失败那一支**不判静默**：静默是「模型看过一圈、判定无需处理」，而这一轮是断掉的
        // ——把它说成静默等于把失败藏起来（票 07）。
        if input.is_watch() && stop_error.is_none() && reply.starts_with(FOREMAN_NO_ACTION_MARK) {
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
        // 前端靠它把主动播报与回话分开渲染，模型不该有机会说错。`reply` 已在上面剥过一遍，
        // 故这里加的是**唯一**那一个。
        // 成本软告警（决策 292 / 票 07）：**只落账不拦**——人的那一轮无硬界（终点由人定），
        // 但「这一轮已经烧了这么多」该被看见。落在**这一轮自己的台账行**上（不另起一条
        // 系统消息：那会刷屏，还会挤占历史窗口），不进 `turn.reply`——通知出口发的是模型
        // 自己说过的话，账目跟着台账走。
        let content = if input.is_watch() {
            format!("{FOREMAN_WATCH_MARK}{reply}")
        } else if cost_warned {
            format!(
                "{reply}\n\n{OPERATION_LOG_MARK}这一轮已烧 {} 生成 token，过了值守轮那条预算线（{}）——人的那一轮没有硬界，继续还是停由你定。",
                tokens.1, token_line
            )
        } else {
            reply.clone()
        };
        // 问话载荷随行落地（决策 265②）：取走即清——一轮至多挂一行，坏轮 / 失败轮
        // 走不到这里（没有 assistant 行可挂，半截的问题不该比它所属的那一轮活得久）。
        let ask_json = ask_slot.lock().await.take();
        self.store
            .append_foreman_message(NewForemanMessage {
                session_id: session.id.clone(),
                role: FOREMAN_ROLE_ASSISTANT.to_string(),
                content,
                prompt_tokens: tokens.0,
                completion_tokens: tokens.1,
                briefing_json: Some(briefing_json),
                traces_json,
                // 顺序留痕（决策 273）：与上面两份聚合列并存——聚合各服务自己的消费者，
                // 段序给时间线（先想了什么、再查了什么、然后说了什么）。
                segments_json,
                // 空串存 `None`（不存空文本）：与 `briefing_json` / `traces_json` 同一条
                // 口径——「没有」与「有但是空的」是两件事，前者该在下发时是 null。
                thinking: (!thinking.trim().is_empty()).then_some(thinking),
                ask_json,
            })
            .await?;

        // 半份结论已经落库，中途失败的那个错误现在才带出去（票 07）：外框照旧按类别落失败账、
        // 通知、作废这一轮提的悬空提议——两条记载各说各的，一条也不丢。
        if let Some(error) = stop_error {
            return Err(error);
        }
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
    ///
    /// **但「下一趟」不是 10 秒之后**（决策 271）：失败要退避（瞬时类 30s 起翻倍，600s 封顶；
    /// 账单 / 配置类 300s 起，1800s 封顶），退避期内这一趟**不问、不看不说话**。退避只挡值守轮，
    /// `say()` 一个字不动——人随时可以自己再试一次。见 [`WatchFailureState`] 的文档。
    pub async fn watch(&self) -> Result<Option<ForemanTurn>> {
        // 全局开关（决策 287 / 票 02）：**最前面**问这一趟该不该开口——关掉 = 跑都不跑
        // （有待办也不醒、不消费、不花钱）；在飞的那一轮不受影响（它已经过了这道门）。
        // 单一事实源在库里，循环每 10s 到这里问一次：界面保存后下一趟即生效，
        // 不必重启、也不需要在进程里再养一份开关状态跟库对账。
        if !self.store.foreman_watch_enabled().await? {
            return Ok(None);
        }
        // 人在跑时**排队**（裁决 2 / 决策 289 / 票 03）：值守轮不起跑——返回 `Ok(None)`
        // 且**待办不消费**，留给下一趟（与失败退避同一姿态；下一次唤醒把窗口内的事件
        // 一起带上，一条不丢）。排队挡的是「同一时刻两份大上下文打同一个 provider」。
        if self.human_turns.in_flight() {
            return Ok(None);
        }
        // 这一轮的起点（决策 233③）：与 `say()` 同一个用途——死轮只作废**它自己**提的提议。
        let started_at = self.store.now();
        // 退避窗口（决策 271）：先问这一趟该不该开口，再谈有没有待办——provider 不通时
        // 「有没有待办」这个问题的答案不影响结论。
        if self
            .watch_failures
            .lock()
            .unwrap()
            .waiting(self.store.now())
        {
            return Ok(None);
        }
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
        let session = self.resolve_watch_session().await?;

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
                // 恢复汇总（决策 271）：退避之后**第一次成功**，把上一批失败的次数与类别链收口。
                // 静默那一轮同样落——「provider 通了」正是最该被记下的一刻。
                // 先取再 await：`std::sync::Mutex` 的锁不跨 await 持有（与 `compaction` 同规矩）。
                let recovered = self.watch_failures.lock().unwrap().take_burst();
                if let Some((count, kinds, last)) = recovered {
                    self.note_turn(
                        &session.id,
                        format!(
                            "【值守】已恢复：上一批连续失败 {count} 次（{}），最后一次 {last}。",
                            kinds.join(" → ")
                        ),
                    )
                    .await;
                }
                let ids: Vec<i64> = waking.iter().map(|i| i.id).collect();
                if let Err(e) = self.store.consume_attention(&ids).await {
                    // 消费失败只记日志：下一趟会重复看到这批事件，多醒一次比丢事件便宜
                    tracing::error!(session = %session.id, "值守轮消费待办失败：{e}");
                }
                // 与 `respond` 里那条静默判据吃的是同一个 `turn.reply`（那里已剥掉自家标记）
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
                    // 值班长播报完成（决策 272②③）：恒通知——它本就是「没人在场」的
                    // 定义，不设 traces 门；静默轮在上面那条腿里，根本走不到这里。
                    self.notify_reply_completed(&turn, false);
                    Ok(Some(turn))
                }
            }
            Err(error) => {
                // 失败也留痕（票 04 那条路），且**不消费**——这批事件下一趟还在。
                // 值守轮的起点就是这一趟本身（它没有「人说的那句话」那条界线）。
                //
                // 但两件事与人的轮不同（决策 271）：
                // 1. **退避**：记一次失败并按类别定下一次允许尝试的时刻（`note_failure`），
                //    退避期内 `watch()` 在顶上就返回了——不需要在这里做别的；
                // 2. **同批同类只落一行**：`note_failure` 回答「这一次要不要落行」，
                //    换类别才有下一行（「余额不足」与「连不上」是两个修法）。
                let now = self.store.now();
                let (kind, _) = turn_failure_reason(&error);
                let note_row = self.watch_failures.lock().unwrap().note_failure(&kind, now);
                self.record_failed_turn(&session, &error, started_at, true, note_row)
                    .await;
                Err(error)
            }
        }
    }

    /// 落一条**失败回合**的账（决策 211④ / 票 04）。
    ///
    /// `role = system` 复用「操作台记账」那条路（决策 207）：对讲台把它渲染成一轮，
    /// 模型下一轮也会看到它——于是「上一轮我为什么没回话」对它自己也是已知的一件事。
    async fn record_failed_turn(
        &self,
        session: &ForemanSession,
        error: &Error,
        started_at: chrono::DateTime<chrono::Utc>,
        from_watch: bool,
        // `false` = 这一批里**同类失败已经落过一行**了（决策 271）：账不重复落，但「作废
        // 悬空提议」照做——那是这一轮失败的后果，与要不要再写一行无关。
        note_row: bool,
    ) {
        let (kind, reason) = turn_failure_reason(error);
        if note_row {
            // 两个标记分开：值守轮的失败账不该顶着「发送失败」那块名牌（决策 271）——
            // 名牌说的是「你刚发出去的那条没到」，而那一批里值班经理一个字节都没发。
            let mark = if from_watch {
                FOREMAN_WATCH_FAILED_TURN_MARK
            } else {
                FOREMAN_FAILED_TURN_MARK
            };
            let tail = if from_watch {
                "（此后同类失败不再逐条落账；本批恢复或换类别时才有下一行）"
            } else {
                ""
            };
            self.note_turn(
                &session.id,
                format!("{mark}这一轮没跑起来（{kind}）：{reason}{tail}"),
            )
            .await;
            // 失败收口必通知（决策 272③）：类 = `failed`（恒发）。与台账**同拍**——
            // `note_row = false` 的那一批后续失败不逐条叫人（同一把尺：决策 271 的
            // 「同批同类只落一行」，否则失败风暴会把手机刷成第二个对讲台）。
            if let Some(notifier) = self.store.notifier() {
                notifier.notify_foreman_failure(&session.title, &kind, self.store.now());
            }
        }
        self.invalidate_round_proposals(&session.id, started_at)
            .await;
    }

    /// 一轮死掉之后，**它那一轮提的提议随之失效**（决策 233③）。
    ///
    /// 提议是「等这一次问答收口」的产物：那一轮已经死了，钮就该随之作废——否则有人还能按
    /// 一条**已经没人等它**的提议。实测里那两条悬空提议（`01M2VSYN…`）从 09-18 一直挂在
    /// `pending`，正是这个缺口的形状。
    ///
    /// 只在**失败的轮**上作废：正常收口那一轮的提议照旧等人按键（那是它的正常归宿）。
    /// 不额外广播事件（与每小时的过期扫描同一姿态）：界面重读会话时看到 `expired`，
    /// 两颗钮随之变灰——为此新造一个 SSE 事件只是多一条要维护的契约。
    async fn invalidate_round_proposals(
        &self,
        session_id: &str,
        started_at: chrono::DateTime<chrono::Utc>,
    ) {
        match self
            .store
            .invalidate_pending_foreman_proposals(session_id, started_at)
            .await
        {
            Ok(0) => {}
            Ok(count) => tracing::info!(
                session = session_id,
                count,
                "这一轮没跑起来：把它提的悬空提议一并作废（决策 233③）"
            ),
            // 作废失败不改变「这一轮失败了」这个事实，故只记一行。
            Err(e) => tracing::warn!(session = session_id, error = %e, "作废悬空提议失败"),
        }
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
        // 这一轮从哪一刻算起：**最后一条消息的时刻**（`say()` 已经把用户那一句落了库，
        // 而它就是这一轮的起点）。读不到就退到「现在」——那只会收得更少，不会误收上一轮的。
        let started_at = self
            .store
            .list_foreman_messages(&session.id, 1)
            .await
            .ok()
            .and_then(|m| m.last().map(|m| m.created_at))
            .unwrap_or_else(|| self.store.now());
        self.note_turn(
            &session.id,
            format!("{FOREMAN_FAILED_TURN_MARK}这一轮没跑完（{why}）：回话没有落库"),
        )
        .await;
        // 失败收口必通知（决策 272③）：panic 被端点层接住的那一条也走 `failed` 类。
        if let Some(notifier) = self.store.notifier() {
            notifier.notify_foreman_failure(&session.title, "interrupted", self.store.now());
        }
        self.invalidate_round_proposals(&session.id, started_at)
            .await;
    }

    /// 值班长回话完成 → 离线通知（决策 272②③）。**第二触发面**：`respond()` 收口之后
    /// 的新入口，**不**写 attention 表——那张表是「待办」（`task_id NOT NULL` + 外键，
    /// 由值守轮消费 `consumed_at`），回话是播报，写进去会污染唤醒判据（§2.1）。
    ///
    /// `gated`：`say` 轮要过 [`FOREMAN_REPLY_MIN_TOOL_CALLS`] 的门，`watch` 播报轮
    /// 恒通知（它本就是「没人在场」的定义）。门在 [`crate::notify::WebhookNotifier::
    /// notify_foreman_reply`] **之前**过——短轮连 `foreman_reply` 的 cooldown 槽
    /// 都不碰（272③ 的 cooldown 倒挂坑）。
    fn notify_reply_completed(&self, turn: &ForemanTurn, gated: bool) {
        if gated && turn.traces.len() < FOREMAN_REPLY_MIN_TOOL_CALLS {
            return;
        }
        let Some(notifier) = self.store.notifier() else {
            return;
        };
        notifier.notify_foreman_reply(&turn.session.title, &turn.reply, self.store.now());
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
                // 值守台账是**只读的一本账**（决策 286 / 票 01）：往里说话会往一本
                // 给人看的流水账里塞进一段对话，而且那段对话在界面上的只读约束里
                // 没有出口。说话面只落人的班次——这一条在这里挡住，比指望每个
                // 前端都记得藏输入坞可靠。
                if session.kind == FOREMAN_SESSION_KIND_WATCH {
                    return Err(Error::Validation(
                        "值守台账是只读的一本账，不收对话——想把某条播报接进对话，\
                         用它上面的「转去对话」"
                            .into(),
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

    /// 值守班次的解析（决策 286 / 票 01）：值守轮写的话落**它自己的班次**，不再混进
    /// 人的时间线。不存在就建一个（固定标题[`FOREMAN_WATCH_SESSION_TITLE`]）——
    /// 与 `say` 的缺省落点同一条「没有就新开」的姿态，只是身份不同。
    ///
    /// 为什么不接 `session_id` 参数：值守轮没有「指定班次」的入口（它由调度器唤醒，
    /// 不由人挑地方），一个班次就是一本台账，不存在第二本。
    async fn resolve_watch_session(&self) -> Result<ForemanSession> {
        match self
            .store
            .latest_foreman_session_of_kind(FOREMAN_SESSION_KIND_WATCH)
            .await?
        {
            Some(session) => Ok(session),
            None => {
                self.store
                    .create_foreman_session_of_kind(
                        FOREMAN_SESSION_KIND_WATCH,
                        FOREMAN_WATCH_SESSION_TITLE,
                    )
                    .await
            }
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
    fn tool_defs(available: &[&'static str]) -> Vec<ToolDef> {
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

    /// 一次模型调用，**空闲判死重试一次**（决策 288 / 票 05）。
    ///
    /// 空闲判死是最典型的瞬时失败：provider 抖一下、流断在半路——同一份请求原样再发
    /// 一次，仍失败才让这一轮失败。只认 `llm_idle_timeout` 这一类（别的类别各有各的
    /// 处置：配置类重试无益、上下文超窗归票 06 的压缩重试）。请求是值，克隆无妨。
    async fn complete_with_retry(
        &self,
        request: crate::agent::client::LlmRequest,
    ) -> Result<crate::agent::client::AgentResponse> {
        match self.llm.complete(request.clone()).await {
            Ok(response) => Ok(response),
            Err(e) if is_idle_timeout(&e) => {
                tracing::warn!(
                    kind = "llm_idle_timeout",
                    "模型调用空闲判死：同一份请求重试一次"
                );
                self.llm.complete(request).await
            }
            Err(e) => Err(e),
        }
    }

    /// 工具调用事件的发射（决策 244）。
    ///
    /// 身份串填 [`FOREMAN_AGENT_TYPE`] + 本班次 `session_id`——路由那头按**同一个判据**
    /// （`SseEvent::is_foreman_event`）过滤，与对话增量走同一条路。`task_id` / `branch`
    /// 是恒空串、`run_id` 恒 0，与值班长的对话增量同一条口径（决策 182⑥/⑨：它不挂任务、
    /// 不落 run 行）。
    fn emit_tool_event(&self, session_id: &str, tool: &str, phase: ToolPhase, args_summary: &str) {
        self.sse.emit(SseEvent::ToolEvent {
            task_id: String::new(),
            branch: String::new(),
            run_id: 0,
            agent_type: FOREMAN_AGENT_TYPE.to_string(),
            session_id: session_id.to_string(),
            tool: tool.to_string(),
            phase,
            args_summary: args_summary.to_string(),
        });
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
        // 这一轮真正拿得到的工具名（决策 247）：纪律段的两组从**它**派生，不再读全量清单
        // ——否则值守轮与 `deny` 档的纪律段会广告一个这一轮已被摘掉的工具。
        available: &[&'static str],
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
        // 两组从 `available` 按档位谓词分家（决策 247）：并 = 这一轮拿得到的、交为空，
        // 「会改动东西」由 `ENV_WRITE_TOOLS` / `SERVICE_WRITE_TOOLS` 判——与 `gate_decision`
        // 同一份事实源，不再另标一份层枚举与它对账。
        let mut direct: Vec<&str> = Vec::new();
        let mut mutating: Vec<&str> = Vec::new();
        for name in available {
            if mutates_something(name) {
                mutating.push(name);
            } else {
                direct.push(name);
            }
        }
        out.push_str(&format!(
            "\n## 工具纪律\n\
             - 你能直接用的工具是：{}。\n",
            direct.join(" / ")
        ));
        out.push_str(&format!("{}\n", self.power_discipline(env_mode, stewarded)));
        if !mutating.is_empty() {
            out.push_str(&format!(
                "- 会改动东西的工具是：{}。\n",
                mutating.join(" / ")
            ));
        }
        out.push_str(&attribution_discipline());
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

// ───────────────── 播报的归因类别（决策 227 / 235 / 238）─────────────────

/// 归因结构块的哨兵（决策 238）：**回话文本里约定的一段**，后端解析。
///
/// 为什么走回话文本而不是新工具：仓里已经靠回话里的哨兵 / 前缀传机器可读信号
/// （`【无需处理】` 模型发后端认、[`FOREMAN_WATCH_MARK`] 后端加前端认），而归因类别
/// **每一轮播报都要带**——做成工具调用等于每轮多一次模型往返，而实测里一轮的往返成本
/// 已经在十万 token 量级（决策 238 的两条理由）。
///
/// 为什么不是 `traces_json`（决策 235 明确否决）：那是**后端自己写的**观测数据，装不了
/// 「模型的归因声明」——而那正是要被校验的东西，让产者与证者同一人等于没校验。
pub const FOREMAN_ATTRIBUTION_MARK: &str = "【归因】";

/// 归因类别（决策 227 的四类）。**「我不知道是什么问题」不是结论**，故四类之外不许收口。
///
/// 四类的证据面、修法、授权都不一样，用一个词盖住就会在播报里丢掉「该找谁」这个信息——
/// 而值班长的全部价值就在这个信息上（决策 227 的起因：一次实测里三条故障各自属于不同类别）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributionKind {
    /// 宿主环境：未签名的壳、系统调用被拦、磁盘 / 权限一类。
    Host,
    /// 流水线运行：节点自身跑挂、重试耗尽、循环 / 调度。
    Pipeline,
    /// 目标项目代码：只有这一类才吃 `repair` 那条 worktree 链。
    ProjectCode,
    /// prompt 与配置：provider / persona / 阶段配置 / 工具声明。
    PromptConfig,
}

impl AttributionKind {
    pub const ALL: [AttributionKind; 4] = [
        AttributionKind::Host,
        AttributionKind::Pipeline,
        AttributionKind::ProjectCode,
        AttributionKind::PromptConfig,
    ];

    /// 稳定标识（落库 / 线上 / 界面都按它判，不按文案）。
    pub fn as_str(self) -> &'static str {
        match self {
            AttributionKind::Host => "host",
            AttributionKind::Pipeline => "pipeline",
            AttributionKind::ProjectCode => "project_code",
            AttributionKind::PromptConfig => "prompt_config",
        }
    }

    /// 给人看的词（界面标记与播报里的那一类）。
    pub fn label(self) -> &'static str {
        match self {
            AttributionKind::Host => "宿主环境",
            AttributionKind::Pipeline => "流水线运行",
            AttributionKind::ProjectCode => "目标项目代码",
            AttributionKind::PromptConfig => "prompt 与配置",
        }
    }

    /// 稳定标识或中文词 → 类别。两者都认是**刻意的**：模型写出中文词是一件正常事，
    /// 不认它只会多出一条「格式不合规」的假未定位。产物一律是稳定标识（[`Self::as_str`]）。
    pub fn parse(raw: &str) -> Option<Self> {
        let value = raw.trim();
        Self::ALL
            .into_iter()
            .find(|k| k.as_str() == value || k.label() == value)
    }
}

/// 一次回话里那个结构块的解析结果。
///
/// **「根本没给」与「四类之外」由这一个解析点判出**（决策 238）：两条都算**未定位**，
/// 因为对判据（决策 230 的第四项）而言它们是同一件事——这一次收口没有可校验的归因类别。
/// 分开记的是原因，不是结论。
///
/// **`run_id` 是判据①的校验面**（决策 230 的「证据归错 run 与没有证据同判失败」）：
/// 类别单独一个字段装不下「这次说的是哪条 run」，而 09-19 翻车的正是这一件——回话读起来
/// 毫无破绽、类别也合规，只是把 run 27 的活栈记在了 run 26 名下，而**没有任何断言拦得住**。
/// 故结构块同时带 `run_id`：它让「回话说的是哪条 run」与「证据说的是哪条 run」可比。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attribution {
    /// 定位成功：给出了四类之一。
    Located {
        kind: AttributionKind,
        /// 回话自己指名的 run（判据①）。**可以为空**：不是每条播报都针对某一条 run
        /// （「无需处理」、或一次说的是任务级态势），而那种情况下「没指名」是诚实的，
        /// 不该被逼着编一个数。校验交给读它的那一方（诊断包的 `latest_attribution`
        /// 与总闸用例），这里只如实带出。
        run_id: Option<i64>,
    },
    /// 没给结构块（回话里一行哨兵都没有）。
    Missing,
    /// 给了但不可用：四类之外 / 缺 `attribution` / JSON 坏了 / 多处自相矛盾。
    Invalid { payload: String, why: &'static str },
}

impl Attribution {
    pub fn kind(&self) -> Option<AttributionKind> {
        match self {
            Attribution::Located { kind, .. } => Some(*kind),
            _ => None,
        }
    }

    /// 回话明确指名的 run（判据①）。未定位或没指名时是 `None`。
    pub fn run_id(&self) -> Option<i64> {
        match self {
            Attribution::Located { run_id, .. } => *run_id,
            _ => None,
        }
    }

    /// 这一次收口算不算「定位成功」的那一项（决策 230 判据④）。
    pub fn is_located(&self) -> bool {
        matches!(self, Attribution::Located { .. })
    }

    /// 线上 / 界面用的串：四类之一，或 `unlocated`（**不编一个假的类别**）。
    pub fn wire(&self) -> &'static str {
        self.kind().map(|k| k.as_str()).unwrap_or("unlocated")
    }

    /// 未定位时的原因串（定位成功时为 `None`）——给排查看，不给界面当类别使。
    pub fn reason(&self) -> Option<&'static str> {
        match self {
            Attribution::Located { .. } => None,
            Attribution::Missing => Some("missing"),
            Attribution::Invalid { why, .. } => Some(why),
        }
    }
}

/// 结构块的载荷上限：超出的部分不留在结果里（它是模型写歪的一段文本，不是证据）。
const ATTRIBUTION_PAYLOAD_MAX_CHARS: usize = 200;

/// 从回话文本里解析归因类别（决策 235 的载体 + 决策 238 的发射方式）。
///
/// 三条判据：
/// * **结构块以整行出现**（行首哨兵，载荷在同一行）才算块——行文里提一句哨兵是散文，
///   不参与判定。于是「夹带」只剩一种形态：再写一行块，而那一行照样要过下面的判据。
/// * **按最严的一处判**：一条回话里有多个块时，任何一处不合规都不收口（决策 235 的
///   「四类之外不许收口」）；多处互相矛盾同样不收口——自相矛盾不是结论。
/// * 找不到块 = `Missing`；**它与「四类之外」在判据上是同一件事**（都没给出可校验的
///   类别），分开记的只是原因（决策 238）。
///
/// 这个方向是刻意的：宁可判未定位，也不让一段夹带的一个合规字样把整轮说成已定位。
pub fn parse_attribution(text: &str) -> Attribution {
    let payloads: Vec<&str> = text
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix(FOREMAN_ATTRIBUTION_MARK)
                .map(|rest| rest.trim())
        })
        .collect();
    if payloads.is_empty() {
        return Attribution::Missing;
    }
    // 类别与它自己指名的 run 同批记：**两处「类别一致但 run 不同」是矛盾**（一处说
    // run 26、一处说 run 27），而只比类别会把这种形状当成「复述同一件事」放过去——
    // 那正是 09-19 那次回话的形状（决策 230 判据①）。
    let mut located: Option<(AttributionKind, Option<i64>)> = None;
    for payload in payloads {
        let truncated =
            || crate::storage::observability::truncate_text(payload, ATTRIBUTION_PAYLOAD_MAX_CHARS);
        let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
            return Attribution::Invalid {
                payload: truncated(),
                why: "JSON 解析失败",
            };
        };
        let Some(raw) = value.get("attribution").and_then(|v| v.as_str()) else {
            return Attribution::Invalid {
                payload: truncated(),
                why: "缺 attribution 字段",
            };
        };
        let Some(kind) = AttributionKind::parse(raw) else {
            return Attribution::Invalid {
                payload: truncated(),
                why: "四类之外",
            };
        };
        // 判据① 的校验面：`run_id` 与类别同批给。**它不是必填**（有的播报说的是任务级态势，
        // 指不出单条 run），但一旦给了就必须是正整数——给个字符串或 0 会把「按 run 对账」
        // 这件事悄悄变成一个假读数（0 在库里同样是锚点值，与决策 231 的哨兵归一同一姿态）。
        let run_id = match value.get("run_id") {
            None | Some(serde_json::Value::Null) => None,
            Some(v) => match v.as_i64() {
                Some(id) if id > 0 => Some(id),
                _ => {
                    return Attribution::Invalid {
                        payload: truncated(),
                        why: "run_id 不是正整数",
                    }
                }
            },
        };
        match located {
            None => located = Some((kind, run_id)),
            Some((previous, previous_run)) if previous == kind && previous_run == run_id => {}
            // 类别一致但 run 不同：**那不是复述，是两处互相矛盾**——正是要拦的形状
            // （一处说 run 26、一处说 run 27，两处都「合规」，合起来是错的）。
            Some(_) => {
                return Attribution::Invalid {
                    payload: truncated(),
                    why: "多处自相矛盾",
                }
            }
        }
    }
    match located {
        Some((kind, run_id)) => Attribution::Located { kind, run_id },
        None => Attribution::Missing,
    }
}

/// 播报里那段「必填」的规格（决策 227 的必填 + 235 的载体 + 238 的形态）。
///
/// 写进 system prompt 而不是人格文本：人格是可被 `persona_path` 覆盖的（决策 7），
/// 而这条要求**没有校验点就不能少**——它是决策 230 判据④的载体，覆盖人格的人不该
/// 顺手把校验面一起覆盖掉。四类的词与稳定标识都由 [`AttributionKind`] 生成（同源）。
fn attribution_discipline() -> String {
    let choices = AttributionKind::ALL
        .iter()
        .map(|k| format!("{}（{}）", k.as_str(), k.label()))
        .collect::<Vec<_>>()
        .join(" / ");
    format!(
        "\n## 播报的归因（必填）\n\
         每一次**播报**（值守轮，以及你回答「哪里出了什么问题」时）末尾都要单独一行给出归因类别，\n\
         机器可读，照这个形状写：\n\
         {FOREMAN_ATTRIBUTION_MARK}{{\"attribution\":\"host\",\"run_id\":123}}\n\
         取值只能是这四类之一：{choices}。\n\
         四类之外不许收口：写别的词等于没给。四类的证据面、修法、授权各不相同——\n\
         「我不知道是什么问题」不是结论，宁可用一条证据把范围收到最像的那一类并说清还缺什么。\n\
         说某一条 run 时**同时带上它的 id**（`run_id`，正整数，取台账里的那一行）：\n\
         你采到的证据是**哪一条 run 的**，写在别处没人对得了账——报错 run 与没有 run 同判未定位。\n\
         说的若是任务级态势、指不出单条 run，就**不写** `run_id`（不写是诚实的，编一个数不是）。\n"
    )
}

/// 这次失败是「空闲判死」吗（决策 288 / 票 05）——重试外框的判据。
///
/// 按 **kind 字段**判，不按报文字样（决策 259 的同一口径）：类别在构造点带上
/// （`LlmErrorKind::IdleTimeout`），这里只认那枚字段。
fn is_idle_timeout(error: &Error) -> bool {
    matches!(
        error,
        Error::LlmClassified { kind, .. } if kind == "llm_idle_timeout"
    )
}

/// provider 判的这一轮请求放不下它的窗口（决策 291 / 票 06(c)）。
///
/// 类别串与 `LlmErrorKind::ContextWindow::as_str()` 同一份（`llm_context_window`）——
/// 认的是**生产侧那个稳定标识**，不是错误文本里有没有「context」。
fn is_context_window(error: &Error) -> bool {
    matches!(
        error,
        Error::LlmClassified { kind, .. } if kind == "llm_context_window"
    )
}

/// 一轮回话失败的归因（票 04）：`(稳定类别, 人话原因)`。
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
