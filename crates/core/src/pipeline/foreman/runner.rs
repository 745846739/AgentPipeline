use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::agent::client::{LlmClient, LlmRequest, Message};
use crate::agent::providers::is_context_window;
use crate::agent::tools::{ToolCallContext, ToolExecutor};
use crate::config::Settings;
use crate::home::Home;
use crate::sse::{SseEvent, SseSink, ToolPhase};
use crate::storage::foreman::{
    ForemanMessage, ForemanSession, NewForemanMessage, FOREMAN_MESSAGE_IN_FLIGHT,
    FOREMAN_ROLE_ASSISTANT, FOREMAN_SESSION_KIND_WATCH, FOREMAN_WATCH_SESSION_TITLE,
};
use crate::storage::Store;
use crate::types::{Node, Stage};
use crate::{Error, Result};

use super::*;

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

/// 这一轮**为什么没能正常收口**（决策 292 / 票 07 / 293 / 票 08 / 294 / 票 09）。
///
/// 它管「触顶」「中途失败」「打转」「人按停」四类——轮数上限那一类由循环自然跑完表达
/// （`stop` 为空，决策 233② 那条最老的路）。五条非正常结束共用同一条收口路径，
/// 差别写在这里。
#[derive(Debug)]
enum StopReason {
    /// 成本门（值守轮）：生成 token 到了预算线。带的是**触发那一刻的累计值**（标注里要写它）。
    Budget(u32),
    /// 循环检测（票 08）：提醒过一次仍在打转，强制收口。带的是判定本身（证据要进标注）。
    Loop(crate::agent::loops::Loop),
    /// **人按停**（票 09）：值班经理按了停钮。带的是**停在第几轮**（标注里要写它）。
    Stopped(usize),
    /// 中途失败（空闲判死 / 超长 / 取消 / 内部错误）：错误原样带回外框做失败记账。
    Failed(Error),
}

/// 一次**可被打断**的模型调用的结果（决策 294 / 票 09）。
enum CallOutcome {
    /// 停钮先到：这一次调用被放弃，整轮走收口路径。
    Stopped,
    /// 调用自己有了结果（成功或失败，与从前一字不差）。
    Done(Result<crate::agent::client::AgentResponse>),
}

/// 「人按停」的收口标记（决策 294 / 票 09，与 [`FOREMAN_PARTIAL_TURN_MARK`] /
/// [`FOREMAN_LOOP_TURN_MARK`] 同族）：裁决 9 的三族标记要一眼分得开——触顶 / 打转 / 人按停。
pub const FOREMAN_STOPPED_TURN_MARK: &str = "【已停】";

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
     你也能**起草补丁**（`repair` 工具）：先 `start` 拿一个独立的修复 worktree，\
     在**那个目录里**改代码（项目工作区你写不进去），改完 `finish` 或 `deliver`——\
     两者都跑闸门（lint + 测试），过了才单独成一个带标记的 commit；`finish` 出 diff、\
     落一条等人按合入的提议；`deliver` 把补丁当场落进那条停着的任务（必填 task_id）的 \
     worktree、在任务工作区再过一遍闸门，托管开着时替值班经理自动重试\
     （没开或次数用完自动退回「等合入」的提议）。没过闸门就什么都不出，\
     回执会说清是哪一步没过。你的改动在值班经理按合入或托管放行之前**没有进主干**——\
     所以只说「我改了什么、为什么、闸门过没过」，绝不说「我已经修好了」；\
     它也**不会**被自动合入。";

/// 值班长的前言。**不复用** [`crate::agent::prompts::build_system_prompt`]。
///
/// 那条路会强制拼上 `BASELINE_PREAMBLE`（「你是 AgentPipeline 的节点 agent……需要时用
/// read_file 读取」）与 `FORMAT_RULES`（「产出文件一律通过 write_file 写入」「结构化流转
/// 信息一律通过 submit_metadata 提交」）——三段指令都在让模型使用它**没有**的工具。
/// 指示一个模型去调不存在的工具，正是它开始编造文件内容的起点。
pub(super) const FOREMAN_BASELINE: &str = "你在一个本机工具内运行，面对的是这台机器上的流水线台账\
     与这个工具自己的家目录。你能读到什么、能不能动手，由下面「工具纪律」那一段说清。";

/// 该轮调用过的只读工具痕迹（票 05：进审计，与快照一起回答「依据什么」）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForemanTrace {
    pub tool: String,
    /// 参数摘要（截断的紧凑 JSON，200 字符）：收起的那一行用它。
    pub args_summary: String,
    /// 完整参数原串，截到 [`FOREMAN_TOOL_RESULT_MAX_CHARS`]（决策 301，修订本结构
    /// 「不存完整参数」的旧口径——界面要能展开看工具详情，审计与展示同一份原文）。
    #[serde(default)]
    pub args: String,
    /// 工具结果 / 错误文本，同一上限（决策 301）。老行没有这个字段，缺省空串。
    #[serde(default)]
    pub result: String,
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
        /// 完整参数原文与工具结果（决策 301，给界面的展开详情），各截到 12k。
        /// 老行（决策 273 之前的 `segments_json`）没有这两个字段，缺省空串——
        /// 前端照旧回落到 `args_summary`（决策 273⑤ 的老行回退口径）。
        #[serde(default)]
        args: String,
        #[serde(default)]
        result: String,
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
    /// 这一轮是**被人按停**的吗（决策 294 / 票 09）。
    ///
    /// 它与 `reply` 里有没有 [`FOREMAN_STOPPED_TURN_MARK`] 是同一件事，分成两个字段是因为
    /// 判它的那一处（`say` 外框）要据此分流一件事：被停在半路那一轮提的**悬空提议保留**
    /// （显式修订 233③），失败那一轮才作废——而两个字段各说各的（`content` 列里那句标注
    /// 是给人读的，这个布尔是给代码判的），从字符串前缀反推会把它变成一条脆耦合。
    pub stopped: bool,
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
    ///
    /// 时长算术住 [`crate::interrupt::backoff_secs`]（决策 355）：这里只负责**选档**
    /// （「等一等不会自己好」的类别换一组 base / max），翻倍与封顶不在这里。
    fn delay_secs(kind: &str, consecutive: u32) -> i64 {
        let (base, max) = if Self::waits_pointlessly(kind) {
            (
                FOREMAN_WATCH_RETRY_CONFIG_BASE_SECS,
                FOREMAN_WATCH_RETRY_CONFIG_MAX_SECS,
            )
        } else {
            (FOREMAN_WATCH_RETRY_BASE_SECS, FOREMAN_WATCH_RETRY_MAX_SECS)
        };
        crate::interrupt::backoff_secs(consecutive, base, max)
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
    ///
    /// 「到点了吗」这条判据住 [`crate::interrupt::waiting`]（决策 355）——它与去抖、
    /// 任务冷却、notify 的每类节流是同一条。
    fn waiting(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        self.next_attempt_at
            .is_some_and(|at| crate::interrupt::waiting(now, at))
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

    /// 这一轮的窗口（决策 291 / 票 06(b)）：provider 行的 `context_window`。
    ///
    /// **只取数，不算术**：容量算术（系统/用户两段预留、软硬限、80% 的触发线）要
    /// `system_prompt`，而那要到组装时才成立——故它随组装裁定走
    /// （[`TurnPlan`]，`model_window` 是喂进去的事实之一）。
    ///
    /// 查不到（无可用 provider / 未登记窗口 / 读库失败）→ `None` = 跳过分档。
    /// **不因为查不到窗口就让这一轮失败**：流水线侧的显式失败（决策 110）守的是「超硬限
    /// 时挂 pending」那条路——对讲台没有那条路，而「窗口没登记」不该让一次对话说不了话。
    /// 真撞墙还有 (c) 那条恢复路兜着（靠 provider 自己的报错，不靠我们猜的窗口）。
    async fn turn_window(&self, cfg: Option<&crate::types::StageConfig>) -> Option<usize> {
        let providers = self.store.load_providers().await.ok()?;
        let fallback = providers.iter().find(|p| p.enabled).map(|p| p.id.as_str());
        let provider_id = crate::storage::catalog::resolve_provider_id(None, None, cfg, fallback)?;
        let provider = providers.into_iter().find(|p| p.id == provider_id)?;
        (provider.context_window > 0).then_some(provider.context_window as usize)
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

    /// 跨时间线互喂（决策 289 / 票 03）：把**对方那条时间线**的摘要取出来（`None` = 不注入）。
    ///
    /// - 人的那一轮 → 值守台账的摘要：值守轮醒过几次、看到了什么、怎么收的场——人指着
    ///   播报说「处理一下」时模型才知道说的是哪件。值守台账还不存在（值守轮一次都没醒过）
    ///   就不取，也不**顺手建**它——那本台账归值守轮所有。
    /// - 值守轮 → 最近活动的人的班次的摘要：裁决 2 的「它仍读得到人说的话」，以摘要形态
    ///   （不是整本原文）。
    ///
    /// **只取正文**：往转录里插哪一条、加哪个标记、插在什么位置，是组装裁定的事
    /// （[`TurnPlan::assemble`]）——这里只有读库与摘要（决策 356 的取数 / 裁定分工）。
    ///
    /// 摘要机器复用 [`Self::compact_history`] 那一套（[`CompactionCache`] 增量缓存 +
    /// [`Self::summarize_interval`]）：每条轮次一生只被压一次，边界没动零 token；
    /// 摘要失败 → 不注入、不报错（轮次绝不因摘要挂掉而挂掉，269④ 同一姿态）。
    async fn cross_digest_text(
        &self,
        input: &TurnInput,
        provider_id: Option<String>,
    ) -> Option<String> {
        enum Source {
            Talk,
            Watch,
        }
        let source = if input.is_watch() {
            Source::Talk
        } else {
            Source::Watch
        };
        // 两条来源各取各的「最近」，读不到就跳过（都不**顺手建**行：值守台账归值守轮所有，
        // 人还没说过话时也没有可摘要的东西）。
        let source_session = match source {
            Source::Talk => match self.store.latest_foreman_session().await {
                Ok(Some(session)) => session,
                Ok(None) => return None,
                Err(e) => {
                    tracing::warn!("互喂摘要读不到人的班次：{e}");
                    return None;
                }
            },
            Source::Watch => {
                match self
                    .store
                    .latest_foreman_session_of_kind(FOREMAN_SESSION_KIND_WATCH)
                    .await
                {
                    Ok(Some(session)) => session,
                    Ok(None) => return None,
                    Err(e) => {
                        tracing::warn!("互喂摘要读不到值守台账：{e}");
                        return None;
                    }
                }
            }
        };
        let history = match self
            .store
            .list_foreman_messages(&source_session.id, FOREMAN_HISTORY_FETCH_LIMIT, None)
            .await
        {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!(session = %source_session.id, "互喂摘要读不到台账：{e}");
                return None;
            }
        };
        // 对方时间线上的在途半截行同样不进摘要（票 01，与本会话历史同一条口径）：
        // 半截话既不该被当成品格化引用，也不该把 `covered_until` 的边界推到一行还没说完的话上。
        let history: Vec<ForemanMessage> = history
            .into_iter()
            .filter(|m| m.status.as_deref() != Some(FOREMAN_MESSAGE_IN_FLIGHT))
            .collect();
        let cache_key = format!("cross:{}", source_session.id);
        self.cross_digest(&cache_key, &source_session.id, &history, provider_id)
            .await
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
            // 被人按停的那一轮：它提的悬空提议**保留**（决策 294，显式修订 233③），
            // 只标一个「来自一轮没说完的话」。失败轮走的是另一条路（作废）——分流就在这里。
            if turn.stopped {
                self.keep_round_proposals(&turn.session.id, round_started_at)
                    .await;
            }
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
        // 停钮通道（决策 294 / 票 09）：**只有人这一轮**登记（值守轮归开关，裁决 10），
        // 凭据退出即摘。登记在这一层而不是内里，是因为它是「这一轮的寿命」的另一面——
        // 与上面两格同一条：请求到达时那一轮要么还在（通道在），要么已经结束了（通道没了）。
        let cancel = (!input.is_watch()).then(|| begin_turn_cancel(&session.id));
        let cfg = self.stage_config().await?;
        self.respond_inner(
            session,
            input,
            cfg,
            cancel.as_ref().map(TurnCancelGuard::signal),
        )
        .await
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
    ///
    /// `cancel`（决策 294 / 票 09）：人这一轮的停钮通道，`None` = 值守轮（没有停钮）。
    async fn respond_inner(
        &self,
        session: &ForemanSession,
        input: TurnInput,
        cfg: Option<crate::types::StageConfig>,
        cancel: Option<&TurnCancel>,
    ) -> Result<ForemanTurn> {
        let briefing = build_briefing(&self.store).await?;
        // 有效设置（long-run-budget 票 02）：启动冻结的 settings 之上叠 DB 覆盖层
        // （`kanban_compaction`，NULL 列 = 读 config 值）——与流水线侧
        // `ModelInvoke::effective_settings` 同源（决策 291），懒读、保存即对下一轮生效。
        let mut settings = self.settings.clone();
        let overrides = self.store.compaction_overrides().await?;
        if let Some(tokens) = overrides.conversation_max_tokens {
            settings.conversation_max_tokens = tokens;
        }
        if let Some(rounds) = overrides.keep_recent_rounds {
            settings.keep_recent_rounds = rounds;
        }
        let settings = &settings;
        // 阶段配置由外框读了一次传进来（人格 + provider / 采样参数共用那一份）。
        // provider 解析只有一处（[`TurnPlan::provider_id`]）：组装要它来登记身份，下面两次
        // 摘要（历史锚点 / 跨线互喂）要它来选摘要器。
        let provider_id = TurnPlan::provider_id(cfg.as_ref());
        // 人格正文的读盘留在编排侧（组装是纯计算，不做 I/O）：`persona_path` 不可读就是
        // 配置错，报文照旧带上完整路径。**读在摘要之前**——坏人格配置要在花掉一次摘要调用
        // 之前就失败（与从前的顺序一致）。
        let persona = match cfg.as_ref().and_then(|c| c.persona_path.as_deref()) {
            Some(path) => {
                let full = self.home.root().join(path);
                Some(std::fs::read_to_string(&full).map_err(|e| {
                    Error::Config(format!(
                        "foreman persona_path 不可读（{}）：{e}",
                        full.display()
                    ))
                })?)
            }
            None => None,
        };

        // 取史：本会话的历史窗口（决策 204②：会话隔离的是上下文），滤掉在途半截行
        // （票 01——喂给模型等于让它读到自己正在说的半句话，历史要的是**收口了**的话）。
        let mut history = self
            .store
            .list_foreman_messages(&session.id, FOREMAN_HISTORY_FETCH_LIMIT, None)
            .await?;
        history.retain(|m| m.status.as_deref() != Some(FOREMAN_MESSAGE_IN_FLIGHT));
        // 「这一轮」从哪一刻算起（决策 311，票 03）：**本轮那条 user 消息的时刻**。
        // 与作废 / 标注那两条路（`created_at >= since`）同一把尺，收场文案数的是同一批东西。
        // 在这里取而不是在收口处取 `history.last()`：循环里会往里 push 助手消息，
        // 到那时 `last()` 已经不是本轮起点。
        let round_since = history
            .last()
            .map(|m| m.created_at)
            .unwrap_or_else(|| self.store.now());
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
        // 这一轮的窗口（决策 291 / 票 06(b)）：provider 行的 `context_window`。查不到
        // （无可用 provider / 未登记窗口 / 读库失败）→ `None` = 跳过分档，容量算术在
        // [`TurnPlan::assemble`] 里——它要 system_prompt，而那要到组装时才有。
        let model_window = self.turn_window(cfg.as_ref()).await;

        // 跨时间线互喂（决策 289 / 票 03）：摘要机器要读库 + 调模型，故在组装之前取好，
        // 只把正文交给计划——往转录里怎么插（标记、位置、方向）是组装裁定的事。
        let cross_digest = self.cross_digest_text(&input, provider_id.clone()).await;

        // ── 组装裁定（决策 356 / 票 01）：档位 → 可用工具 → 系统提示词 → 转录
        // （历史 → 尾部注入 → 本轮合并轮）→ 窗口容量 → 每轮的两道门。纯计算，
        // 全部输入都是上面取好的事实。
        let briefing_text = briefing.render();
        let question = input.transcript_text();
        let mut plan = TurnPlan::assemble(TurnFacts {
            settings,
            session_id: &session.id,
            is_watch: input.is_watch(),
            cfg: cfg.as_ref(),
            window: &window,
            anchor: anchor.as_deref(),
            cross_digest: cross_digest.as_deref(),
            briefing_text: &briefing_text,
            question: &question,
            stewarded,
            persona: persona.as_deref(),
            model_window,
        });
        // 转录从此由编排侧推进（循环里 push 助手 / 工具结果），plan 只提供裁定与请求字段。
        let mut transcript = plan.take_transcript();

        // 分级诊断摘在**源头上**（决策 247）：`deny` 早于三处消费者算好，广告集、
        // 执行点白名单与纪律段都吃 `plan.available`（组装里同源），故「模型看得见一个
        // 调用就被拒的工具」这件事在自动轮里同样不会发生。
        // 问话载荷槽（决策 265）：每轮新建一个——工具写、本轮收口时取走挂到 assistant 行。
        let ask_slot: Arc<tokio::sync::Mutex<Option<serde_json::Value>>> =
            Arc::new(tokio::sync::Mutex::new(None));
        let (tools, ctx) = foreman_tooling(
            &self.store,
            &self.settings,
            &self.home,
            self.sse.clone(),
            &session.id,
            plan.env_mode,
            ForemanMoment::Conversation,
            &plan.available,
            self.steward_actions.clone(),
            // 人的那一轮放开台账读数（票 06(a)）：值守轮按摘要形态读（分级纪律）。
            !input.is_watch(),
        );
        let tools = tools.with_ask_slot(ask_slot.clone());

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
        // 逐调用空闲界（决策 288 / 票 05）与轮内窗口容量（决策 291 / 票 06(b)）都随组装
        // 裁定走（[`TurnPlan`]）：前者要 `system_prompt` 之外的一切，后者要它本身——
        // 而它到组装时才成立，故容量算术只能在计划里算。

        // ── 在途半截行（票 01，spec 决策 1）：一轮开工即建，收口时写成完整行。
        // 建在**历史读完、请求组装完之后**：这一轮喂给模型的历史因此一个字都不变
        // （半截行既不进上下文，也不与收口那行抢 id——它就是收口那行自己）。
        // `briefing_json` 在这里一并算好：快照从开工那一刻就成立，建行时就该在场上。
        let briefing_json = serde_json::to_value(&briefing)?;
        let live =
            LiveTurn::begin(self.store.clone(), &session.id, Some(briefing_json.clone())).await?;

        for round in 0..round_limit {
            // 停钮（决策 294 / 票 09）：**每次调用前看一眼**。信号可能在上一次工具调用
            // 期间到达（那一批跑完才回到这里），也可能是「信号先到、观察者后建」那一格
            // ——`Notify` 存的许可只在 `wait()` 上兑现，故这里必须另查一次布尔值
            // （与流水线 `CancelSignal` 的两条路同一条理由）。命中就不再把下一次调用发出去。
            if cancel.is_some_and(TurnCancel::is_requested) {
                stop = Some(StopReason::Stopped(round + 1));
                break;
            }
            // 每次调用前查一次预算（票 06(b)）：到线就按轮压缩（规则化、不调 LLM）。
            // **查在组装请求之前**，故这一轮发出去的已经是压过的那一份。
            plan.check_window_budget(&mut transcript);
            // 成本门（决策 292 / 票 07）：每次调用前查一次——与上面那条窗口门同一位置，
            // 故「已经不划算的下一轮」根本不会发出去。**分档**：值守轮触顶即停（出循环走
            // 收口路径，部分结论 + 【未收口】）；人的那一轮无硬界（终点由人决定），
            // 只把这条线记下来，收口时落一条软告警（只落账不拦）。
            match cost_verdict(input.is_watch(), tokens.1, token_line) {
                CostVerdict::KeepGoing => {}
                CostVerdict::WarnHuman => cost_warned = true,
                CostVerdict::StopWatch => {
                    stop = Some(StopReason::Budget(tokens.1));
                    break;
                }
            }
            let mut request = plan.request(transcript.clone());
            let response = match self.complete_cancellable(request.clone(), cancel).await {
                // 人按停（决策 294 / 票 09）：这一次调用被放弃，整轮走收口路径
                // （部分结论 + 【已停】）。**不是失败**——不落失败账、不发失败通知。
                CallOutcome::Stopped => {
                    stop = Some(StopReason::Stopped(round + 1));
                    break;
                }
                CallOutcome::Done(Ok(response)) => response,
                // 撞墙恢复（决策 291 / 票 06(c)）：provider 报上下文超长时**不原地判败**
                // ——把这一轮的转录压一遍再重试这一次调用（只一次；再撞就是真的放不下，
                // 那时报错才是诚实的）。压缩是**无条件**的：这个错误说明算术低估了
                // （真 tokenizer 与 4 字符≈1 的估算、工具定义都占窗口），不按触发线走。
                CallOutcome::Done(Err(e)) if is_context_window(&e) => {
                    let compacted = plan.compact_forced(&mut transcript);
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
                    // 这一次重试同样可被按停（票 09）：人按停的那一刻不该因为「它正在
                    // 重试」而多等一轮。
                    match self.complete_cancellable(request, cancel).await {
                        CallOutcome::Stopped => {
                            stop = Some(StopReason::Stopped(round + 1));
                            break;
                        }
                        CallOutcome::Done(Ok(response)) => response,
                        CallOutcome::Done(Err(e)) => {
                            stop = Some(StopReason::Failed(e));
                            break;
                        }
                    }
                }
                // 中途失败（决策 292 / 票 07）：**不原地把这一轮丢掉**——出循环走收口路径
                // （有话说就带上标注落库），错误原样带去外框做失败记账（类别 / 通知 /
                // 悬空提议作废都不变）。
                CallOutcome::Done(Err(e)) => {
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
            // 在途现场的**调用边界**（票 01）：段序 / 累计推理 / token 此刻是权威值，
            // 整体覆盖并立刻刷一次——不等这一轮的工具跑完（一次工具可以跑几十秒，
            // 那段等待里「它刚才想了什么」就该已经能从台账读回来）。
            live.observe_call(&segments, &thinking, tokens);
            live.flush(true).await?;

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
                // 展开详情的原文（决策 301）：与摘要同一刻取，事件与两份留痕共用这一份——
                // 各截各的会让「界面看到的」与「台账记的」出现两个版本。
                let args_detail = truncate(&call.arguments);
                // 工具调用**当场**推给界面（决策 244）：落库的 `traces_json` 要等到这一轮
                // 收口才写，而诉求正是「不要在对话完结后才展示」。两处记的是同一件事的两种
                // 时态——实时事件说「此刻在查什么」，落库痕迹说「这一轮查过什么」（审计）。
                // 位置戳（票 02）：每个事件先领号再广播，段序收场时才把号变成「已覆盖」。
                self.emit_tool_event(
                    &session.id,
                    &call.name,
                    ToolPhase::Start,
                    &args_summary,
                    &args_detail,
                    None,
                    Some(live.reserve_event_seq()),
                );
                let (content, ok) = match self.run_tool(&tools, call, &ctx).await {
                    Ok(outcome) => (outcome.content, true),
                    // 工具失败**不**上升为整次回话失败（§12.8 的同一姿态）：
                    // 把错误文本回给模型让它改道，而不是让人看到一条报错。
                    Err(e) => (format!("工具执行失败：{e}"), false),
                };
                // 结果详情（决策 301）：给界面展开用，恒截 12k。与回灌那份**不是同一件事**：
                // 回灌只在值守轮预截（决策 291，人那一轮逐字回灌），详情则两条轮一视同仁。
                let result_detail = truncate(&content);
                self.emit_tool_event(
                    &session.id,
                    &call.name,
                    if ok { ToolPhase::End } else { ToolPhase::Error },
                    &args_summary,
                    &args_detail,
                    Some(&result_detail),
                    Some(live.reserve_event_seq()),
                );
                // 同一个工具事件在两处各记一份（决策 273）：`traces` 是「查过什么」的聚合，
                // `segments` 要的是它在整轮里的**位置**。摘要先给段序，再交给聚合那一份。
                segments.push(ForemanSegment::Tool {
                    tool: call.name.clone(),
                    args_summary: args_summary.clone(),
                    args: args_detail.clone(),
                    result: result_detail.clone(),
                    ok,
                });
                traces.push(ForemanTrace {
                    tool: call.name.clone(),
                    args_summary,
                    args: args_detail,
                    result: result_detail,
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
                //
                // 顺带带上**当时的落库状态指纹**（决策 310 判据③）：提议数 / 任务状态 /
                // 游标位置。取不到（读库失败）就用 `Default`——那会让连续计数按「没变」走，
                // 方向是**偏保守**（宁可能提醒一次，也不要因为一次读库失败而漏掉整整一段）。
                let readings = self
                    .store
                    .foreman_state_readings(&session.id, round_since)
                    .await
                    .unwrap_or_default();
                loop_log.push(crate::agent::loops::CallRecord {
                    tool: call.name.clone(),
                    arguments: call.arguments.clone(),
                    result_digest: crate::agent::loops::result_digest(&content),
                    state_digest: readings.digest(),
                });
                transcript.push(Message::tool_result(call, content));
            }
            // 在途现场的**迭代收场**（票 01）：这一轮的段序与痕迹此刻含全部已收场步骤，
            // 正文与推理的「正在冒」两段并了进去、清零——下一次调用的增量从干净底子上长。
            live.settle_round(&segments, &thinking, &traces, tokens);
            live.flush(true).await?;
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
                    //
                    // **判据③ 不在此列**（决策 310）：它是**提醒级、不收口**——「提醒过了
                    // 仍在原地」对它只意味着「再提醒一次」，收口的权力仍留在 293 原两条判据
                    // 手里。它比另两条更容易误伤（开放型问题上「三项读数全无变化」是常态），
                    // 而一刀切断一轮正当的深查，比多花一点钱坏得多。
                    Some(_) if hit.is_remind_only() => {
                        tracing::warn!(session = %session.id, %reason, "判据③ 又攒满一段无进展：只提醒，不收口（决策 310）");
                        transcript.push(Message::user(format!(
                            "{FOREMAN_LOOP_REMINDER_MARK}{reason}。若现有信息已经够回答这一轮的\
                             问题，现在就收口；若确实还要查，**换个角度**——别再重复刚才那几个调用。"
                        )));
                        // 提醒过的那一段划掉：下一次只在**又攒满一整段**时才再提醒一次。
                        loop_reminded_at = Some(loop_log.len());
                    }
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
        //
        // 第三个返回值是**「这一轮被人按停了吗」**（票 09）：它决定外框要不要在那一轮提的
        // 悬空提议上做标注（`ForemanTurn::stopped`）。判据是「收口这一行挂的是【已停】」，
        // 而不是「通道上收到过请求」——请求可能在模型已经收口之后才到（那一刻这一轮没被
        // 停掉，只是慢了一步），拿那个当判据会把一条正常收口的轮标注成半成品。
        // 收场文案要按**实际提议数**说话（决策 311，票 foreman-burns 03）。
        //
        // 2026-09-27 实测：该会话提议数为 **0**，而收场文案写着「这一轮提的提议都还在，
        // 照样可以按」——它让人去按一个不存在的东西（唯一相关的那张提议属于上一个会话，
        // 且早在一小时前过期作废）。故这一格先问库：这一轮到底提了几条。
        //
        // 问不出来（读库失败）按 **0** 处置：宁说「本轮没有提任何提议」，也不要再复述一次
        // 那句可能不成立的保证——这句话的谎正是本票要停掉的东西。
        let round_proposals = self
            .store
            .count_round_foreman_proposals(&session.id, round_since)
            .await
            .unwrap_or(0);
        // 收场那一句按实际提议数分支（决策 311）：
        // - **有提议**：原句**一字不改**——它是决策 294 / 修订 233③「被停在半路那轮提的
        //   提议保留」的兑现点，丢了它人就不会去看那批卡片了；
        // - **0 条**：明说 0 条，不再复述一句不成立的保证（2026-09-27 那次它就是这么说的谎）。
        let proposal_note = if round_proposals > 0 {
            "这一轮提的提议都还在，照样可以按。"
        } else {
            "本轮没有提任何提议。"
        };
        let (reply, stopped, stop_error) = match (reply, stop) {
            // 模型自己收口了：正常那一句（预算门在它之后才可能踩线，故这里不看 `stop`）。
            (Some(reply), _) => (reply, false, None),
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
                // 「为什么没说完」与**挂哪个标记**按停机原因分（票 07 / 08 / 09）：打转与
                // 按停各有自己的标记（与【未收口】同族、形状不同——裁决 9 让三种停法一眼
                // 分得开），其余共用它。
                let was_stopped = matches!(stop, Some(StopReason::Stopped(_)));
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
                    // 人按停（决策 294 / 票 09）：说清「是你停的」而不是「它自己断了」——
                    // 这一行的读者可能不是按停的那个人（手机与电脑同时开着的时候）。
                    Some(StopReason::Stopped(round)) => (
                        FOREMAN_STOPPED_TURN_MARK,
                        format!(
                            "这一轮你按了停（停在第 {round} 轮），话没说完——以上是已经确定的部分。\
                             要接着查可以让我再来一轮（带上线索）；{proposal_note}"
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
                    "值班长没能收口：部分结论落库并标注（决策 233② / 292 / 293 / 294）"
                );
                (format!("{partial}\n\n{mark}{why}"), was_stopped, error)
            }
            // 人按停而它**一句话都没说过**：照样落一行（决策 294 / 票 09）。
            // 这一支刻意**不走**失败处置（与上面那两支不同）：把「你要它停」写成「它没跑起来」
            // 是两处失真——归因反了（人干的，不是它坏的），还会顺手发一条失败通知（按停的人
            // 就在屏幕前，那是纯噪声）。行里也没有部分结论可留，故只留这句事实本身。
            (None, Some(StopReason::Stopped(round))) => (
                format!(
                    "{FOREMAN_STOPPED_TURN_MARK}这一轮你按了停（停在第 {round} 轮）——\
                     它还没说出什么，没有部分结论可留。要接着查可以让我再来一轮（带上线索）；\
                     {proposal_note}"
                ),
                true,
                None,
            ),
            // 中途失败且**一句有内容的话都没说过**：没有东西可留，错误原样带回。
            // 半截行跟着一起丢（票 01）：今天这一轮在库里没有回话行，失败账由外框的
            // `record_failed_turn` 落 `system` 行——留着半截行会让它永远显示「正在说」。
            (None, Some(StopReason::Failed(e))) => {
                live.discard("中途失败且没有可留的话").await;
                return Err(e);
            }
            // 打转到底**一句话都没说过**：同样没有部分结论可留（与上面那一支同姿态），
            // 但归因要说实话——「它在原地打转」与「它一直在查台账」是两件事，指错方向
            // 会让人去查一个不存在的毛病。
            (None, Some(StopReason::Loop(hit))) => {
                live.discard("原地打转且一句话都没说过").await;
                return Err(Error::LlmClassified {
                    kind: "model_looping".into(),
                    message: format!("值班长在原地打转（{}），一句话都没说就停了", hit.reason()),
                    raw: format!("循环检测命中：{}", hit.reason()),
                });
            }
            (None, _) => {
                // 归因**走 `LlmClassified` 的 kind 机制**而不是新造一种错误（票 04）：
                // 这两条是模型行为，不是内部故障，而「哪一类」正是排查要的入口。
                live.discard("一轮到底没有回话").await;
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
            // 静默轮**不落回话行**（这一支此前就返回在追加之前）：半截行跟着一起丢，
            // 语义与今天逐字一致——库里没有这一轮的话，只是中途曾经有过现场。
            live.discard("值守轮判定无需处理：不落回话行").await;
            return Ok(ForemanTurn {
                session: session.clone(),
                reply,
                stopped,
                prompt_tokens: tokens.0,
                completion_tokens: tokens.1,
                briefing,
                traces,
            });
        }
        // `briefing_json` 用开工时算好的那一份（建在途行时已写进同一行，两处同一值）。
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
        // **收口**（票 01，spec 决策 1）：写开工时建的那条在途行，而不是追加新行——
        // 台账最终形状与从前逐字一致（一次回话 = user 行 + assistant 行），边流边写
        // 只改中途是否可见。终态以这里的权威值为准，此前的节流刷写全部作废。
        live.close(NewForemanMessage {
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
            stopped,
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
        // 判据在 [`crate::interrupt::debounce_elapsed`]（决策 355）。
        let debounce = chrono::Duration::seconds(self.settings.watch_debounce_sec as i64);
        if let Some(oldest) = candidates.iter().map(|i| i.created_at).min() {
            if !crate::interrupt::debounce_elapsed(self.store.now(), oldest, debounce) {
                return Ok(None);
            }
        }
        // 同任务冷却（决策 209⑤ / 票 07）：刚被处理过的任务，新事件**不单独唤醒**——
        // 留在表里不消费，冷却到期后与那时的事件合并播报。判据是「这个任务最近有没有
        // 被消费过的待办」：那一行就是「刚有人看过它」的账。
        // 窗口的左沿由 [`crate::interrupt::window_start`] 算（查库那一侧含左沿，
        // 与 `waiting` 同一个窗口的两种读数，口径写在那个模块的头注里）。
        // **每件各读一次钟**——与从前逐字一致：左沿随之往前挪一点，正好把这一趟扫过前面
        // 几件花掉的时间算进去，而不是用一趟开始时那个更早的读数。
        let cooldown = chrono::Duration::minutes(self.settings.watch_task_cooldown_minutes as i64);
        let mut waking = Vec::new();
        for item in candidates {
            let recently_handled = self
                .store
                .count_consumed_attention_since(
                    &item.task_id,
                    crate::interrupt::window_start(self.store.now(), cooldown),
                )
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
        // 上限判据与「触顶通知只发一次」住 [`crate::interrupt`]（决策 355）：
        // `over_hourly_cap` / `cap_notice_due`——决策 350 那一类口径调整此后只碰那个文件。
        let hour_ago = self.store.now() - chrono::Duration::hours(1);
        let wakes = self.store.count_watch_wakes_since(hour_ago).await?;
        if crate::interrupt::over_hourly_cap(wakes, self.settings.watch_max_wakes_per_hour) {
            let noted = self
                .store
                .count_watch_wakes_with(
                    crate::storage::attention::WatchWakeOutcome::Capped,
                    hour_ago,
                )
                .await?;
            if crate::interrupt::cap_notice_due(noted) {
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
                notifier.notify_foreman_failure(
                    &session.id,
                    &session.title,
                    &kind,
                    self.store.now(),
                );
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

    /// **人按停**那一轮提的悬空提议：**保留**，只标一个来路（决策 294 / 票 09）。
    ///
    /// 与 [`Self::invalidate_round_proposals`] 是同一把判据、相反的两条出路，分岔全在
    /// 「轮是怎么结束的」：轮**自己**死了（失败 / panic）→ 等它的人已经没了，钮必须作废；
    /// 人主动按停 → 「这话先这样，按你提的第 2 条办」——提议正是他还想按的东西。
    /// 作废它等于把他刚点的菜端走，而这正是**显式修订决策 233③** 的那一条：
    /// 作废只对「轮自己死了」，人按停不算。
    ///
    /// 标注（`stopped_round`）只落库里那一个读数，不改状态也不另发事件：界面重读会话时
    /// 看到它，在那一张卡片上多写一行来路——与作废那条「不额外广播」同一姿态。
    async fn keep_round_proposals(
        &self,
        session_id: &str,
        started_at: chrono::DateTime<chrono::Utc>,
    ) {
        match self
            .store
            .mark_pending_foreman_proposals_stopped(session_id, started_at)
            .await
        {
            Ok(0) => {}
            Ok(count) => tracing::info!(
                session = session_id,
                count,
                "这一轮被按停：它提的悬空提议保留并标注来路（决策 294）"
            ),
            // 标注失败不改变「这一轮被按停了」这个事实，故只记一行（与作废那条同一姿态）。
            Err(e) => tracing::warn!(session = session_id, error = %e, "标注被停轮的悬空提议失败"),
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
            .list_foreman_messages(&session.id, 1, None)
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
            notifier.notify_foreman_failure(
                &session.id,
                &session.title,
                "interrupted",
                self.store.now(),
            );
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
        notifier.notify_foreman_reply(
            &turn.session.id,
            &turn.session.title,
            &turn.reply,
            self.store.now(),
        );
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

    // 工具定义（从清单生成）与系统提示词的三段，随组装裁定搬进 [`TurnPlan`]
    // （决策 356 / 票 01）：它们是「这一轮给模型看什么」的纯计算，与广告集 / 执行点
    // 白名单同源（决策 247），放在计划里才能被独立断言。

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

    /// 一次**可被停钮打断**的模型调用（决策 294 / 票 09）。
    ///
    /// `select!` 的形状照流水线那套（决策 226 / 276 的 `CancelSignal`）：停在 await 上的
    /// 正是这次调用，故停钮必须能把它唤醒——只在下一次调用去查的**协作式**中止在这里不够用
    /// （一次调用可以跑几分钟，而人按停要的是「现在」）。`biased;` 让它先被看见：信号已经
    /// 在那儿（`Notify` 存的许可）时，这一次调用根本不该发出去。
    ///
    /// `None`（值守轮）走的分支与从前一字不差：没有停钮可等的调用就是普通调用。
    async fn complete_cancellable(
        &self,
        request: crate::agent::client::LlmRequest,
        cancel: Option<&TurnCancel>,
    ) -> CallOutcome {
        match cancel {
            Some(signal) => tokio::select! {
                biased;
                _ = signal.wait() => CallOutcome::Stopped,
                result = self.complete_with_retry(request) => CallOutcome::Done(result),
            },
            None => CallOutcome::Done(self.complete_with_retry(request).await),
        }
    }

    /// 工具调用事件的发射（决策 244）。
    ///
    /// 身份串填 [`FOREMAN_AGENT_TYPE`] + 本班次 `session_id`——路由那头按**同一个判据**
    /// （`SseEvent::is_foreman_event`）过滤，与对话增量走同一条路。`task_id` / `branch`
    /// 是恒空串、`run_id` 恒 0，与值班长的对话增量同一条口径（决策 182⑥/⑨：它不挂任务、
    /// 不落 run 行）。
    ///
    /// `stamp`（票 02）：`(在途行 id, 行内位置号)`，由 [`LiveTurn::reserve_event_seq`]
    /// 先领后发——前端据此与快照对账（`seq > seq0` 才接）。
    #[allow(clippy::too_many_arguments)] // 与流水线那个同形出口同宽（详情两列 + `stamp` 都是这条 wire 的字段）
    fn emit_tool_event(
        &self,
        session_id: &str,
        tool: &str,
        phase: ToolPhase,
        args_summary: &str,
        args: &str,
        result: Option<&str>,
        stamp: Option<(i64, u64)>,
    ) {
        self.sse.emit(SseEvent::ToolEvent {
            task_id: String::new(),
            branch: String::new(),
            run_id: 0,
            agent_type: FOREMAN_AGENT_TYPE.to_string(),
            session_id: session_id.to_string(),
            tool: tool.to_string(),
            phase,
            args_summary: args_summary.to_string(),
            // 原文与结果由调用点按 [`FOREMAN_TOOL_RESULT_MAX_CHARS`] 截好——与同一刻写进
            // `segments` / `traces` 的是同一份（决策 301）：事件与留痕不许各截各的。
            args: args.to_string(),
            result: result.map(str::to_string),
            ledger_id: stamp.map(|(id, _)| id),
            seq: stamp.map(|(_, seq)| seq),
        });
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
/// 流水线出口 `pipeline::events::emit_tool_event` 的 `args` / `result` 详情（决策 301）也走这份。
pub(crate) fn truncate_tool_result(text: &str) -> String {
    truncate(text)
}
