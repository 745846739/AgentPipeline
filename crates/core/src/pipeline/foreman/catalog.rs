use std::path::PathBuf;
use std::sync::Arc;

use crate::agent::tools::{ToolCallContext, ToolExecutor};
use crate::config::Settings;
use crate::home::Home;
use crate::pipeline::proposals::StoreProposalSink;
use crate::process::RealProcessKiller;
use crate::sse::SseSink;
use crate::storage::Store;
use crate::types::{CommandSource, Node, Stage};

// ============================== 工具规格（从 foreman.rs 原地搬入，决策 351）

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
pub(super) fn mutates_something(name: &str) -> bool {
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
    // rtk 开关（决策 297 / 票 04）：每条命令现读一次库里的那一行（`RtkSource::Store`），
    // 于是「保存即活」；**不注入 = 不改写**，命令按原样跑。
    .with_rtk_store(store.clone())
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
