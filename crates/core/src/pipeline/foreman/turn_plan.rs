//! 值班长一轮的**组装裁定**（决策 356，票 01）：事实快照进，请求 + 工具广告集 +
//! 压缩 / 截断 / 预算判定出。落 `foreman/turn_plan.rs`。
//!
//! ## 为什么有这个模块
//!
//! `respond_inner` 此前一个函数做完档位解析、简报构建、工具广告集、取史 + trim + 锚点注入、
//! 内联压缩、超窗强制压缩的循环、工具往返、ask 槽、归因、SSE、落库、预算判界。纯件
//! （[`trim_history`] / `situation_drift` / `parse_attribution`）各有单测，**bug 却全长在
//! 「怎么调」上**——决策 288「23 次调用全成功仍被 30 分钟墙钟整段砍掉」坏的是循环外界，
//! 而测试只有 FakeAgent 整轮脚本，粒度粗到只有碰巧钉住才红。
//!
//! ## 边界（窄边界，拷问定案）
//!
//! **只管组装**：档位 → 可用工具 → 系统提示词 → 转录（历史 → 尾部注入 → 本轮合并轮）→
//! 窗口容量 → 每轮的两道门（窗口预算 / 成本线）。**不管推进**：LLM 循环、工具往返、SSE、
//! 落库、`LiveTurn`、停钮全留在 `respond_inner`——切开它们要动接缝的消费方式（决策 356
//! 明文「不做」）。
//!
//! 取数（简报 / 历史 / 人格读盘 / provider 窗口 / 摘要调用）也在调用方：本模块**不读库、
//! 不读盘、不调模型**，唯一借的配置是 [`Settings`]（容量算术的两个比例与
//! `keep_recent_rounds`——与 [`crate::pipeline::model_request::RequestPlan`] 借
//! `ctx.settings` 同一姿态）。
//!
//! ## 签名之外必须知道的不变量
//!
//! 1. [`TurnPlan::assemble`] 每轮**恰一次**；`transcript` 由 [`TurnPlan::take_transcript`]
//!    交给编排侧，此后**只由编排侧推进**（循环里 push 助手 / 工具结果）——裁定方法
//!    （[`TurnPlan::check_window_budget`]）吃 `&mut Vec<Message>` 正是这个分工的兑现。
//! 2. **注入位置是纪律不是风格**：锚点与互喂摘要追加在**历史之后、本轮问题之前**，
//!    本轮问题永远是最后一条。头部注入会让其后全部历史的前缀缓存整体打穿
//!    （spec `.scratch/prompt-cache`；对决策 269 / 289 注入位置的显式修订）。
//! 3. **`user_prompt` 恒空**：适配器拼的是 `[system][user(user_prompt)] + messages`，
//!    而快照每轮必变——放这儿等于每轮把整段历史的前缀缓存打穿（决策 182⑤）。
//! 4. **档位先于人格**：`deny` 算在 `system_prompt` 之前，广告集 / 执行点白名单 / 工具
//!    纪律段三处吃同一份 `available`（决策 247）。从前 `deny` 晚于它才算，纪律段于是
//!    广告着一个这一轮已被摘掉的工具（架构评审候选 7 抓到的顺序 bug）。
//! 5. `capacity = None`（无 provider / 未登记窗口）→ 两道窗口门恒不触发（决策 110：
//!    不臆造窗口）；撞墙那条路（provider 自己报错）仍在编排侧。
//! 6. 轮内压缩的触发线是**窗口的 80%**（[`FOREMAN_INLOOP_COMPACT_RATIO`]），比流水线
//!    那条软限迟钝——压缩会打断 provider 的 prefix 缓存（2026-09-26 实测 94% 命中）。
//!
//! ## 这是一次搬家，不是改口径
//!
//! `respond_inner` 的组装段原样搬进这里，行为逐字不变（foreman 那 136 条 FakeAgent
//! 整轮脚本用例一个字没改就是证据）。搬走的是**判定**，留下的是取数与编排。
//!
//! ## 依赖分类（DEEPENING）
//!
//! 组装与两道门是 in-process 纯计算；`interface 就是测面`——窄测试直接构造
//! [`TurnFacts`]，不建 `ForemanRunner`、不建 Store、不建 LLM。

use crate::agent::client::{LlmRequest, Message, Role, RunContext, ToolDef};
use crate::agent::context::{
    compact_messages_from, estimate_context_capacity, estimate_messages_tokens,
    should_compact_with_floor, ContextCapacity,
};
use crate::config::Settings;
use crate::storage::foreman::{ForemanMessage, FOREMAN_ROLE_SYSTEM, FOREMAN_ROLE_USER};
use crate::types::{EnvMode, Node, Stage, StageConfig};

use super::attribution::attribution_discipline;
use super::catalog::{foreman_available_tools_except, mutates_something, FOREMAN_TOOL_SPECS};
use super::conversation::{COMPACTION_MARK, FOREMAN_TALK_DIGEST_MARK, FOREMAN_WATCH_DIGEST_MARK};
use super::runner::{
    FOREMAN_BASELINE, FOREMAN_PERSONA, FOREMAN_WATCH_TOOL_DENY, OPERATION_LOG_MARK,
};
use super::{FOREMAN_AGENT_TYPE, FOREMAN_STAGE_KEY};

/// 组装要的事实快照。**全部由调用方取好**（见模块头注的取数清单）。
pub struct TurnFacts<'a> {
    /// 容量算术的两个比例与 `keep_recent_rounds` 的来源（与 `RequestPlan` 同一姿态）。
    pub settings: &'a Settings,
    pub session_id: &'a str,
    /// 人的那一轮还是值守轮：定 `deny` 档、注入方向与合并规则。
    pub is_watch: bool,
    /// `stage_configs` 的 `foreman` 行（可能不存在——「不配置也能用」）。
    pub cfg: Option<&'a StageConfig>,
    /// 已落库、已按预算 trim、**已滤掉在途半截行**的历史窗口。
    pub window: &'a [ForemanMessage],
    /// 历史锚点（`compact_history` 的产物）；`None` = 预算内逐字照旧。
    pub anchor: Option<&'a str>,
    /// 跨时间线摘要的**正文**（方向由 `is_watch` 定，标记在本模块加）。
    pub cross_digest: Option<&'a str>,
    /// 简报正文（`ForemanBriefing::render()`）。
    pub briefing_text: &'a str,
    /// 本轮问题（`TurnInput::transcript_text()`）。
    pub question: &'a str,
    /// 有没有任务在被托管（决策 210①）：只在真有时才在人格里说那一段。
    pub stewarded: bool,
    /// persona 正文；`None` = 用内置人格（`persona_path` 的读盘在调用方）。
    pub persona: Option<&'a str>,
    /// provider 行的 `context_window`；`None` / `0` = 跳过分档（决策 110）。
    pub model_window: Option<usize>,
}

/// 轮内压缩的触发线（决策 291 / 票 06(b)）：**窗口的 80%**。
///
/// 理由见票面：压缩会打断 provider 的 prefix 缓存（2026-09-26 实测 94% 命中，是这套东西
/// 唯一便宜的地方），压一次之后下一次调用几乎全量重算，故触发要**迟钝**（到窗口 ~80% 才压）。
const FOREMAN_INLOOP_COMPACT_RATIO: f64 = 0.8;

/// 一轮的组装结果：自持 String / Vec，无生命周期，编排侧跨轮自由持有。
#[derive(Debug)]
pub struct TurnPlan {
    pub session_id: String,
    /// 环境层档位（决策 206）——广告集、执行点白名单与人格里的工具纪律段**同源**的那一份。
    pub env_mode: EnvMode,
    /// 这一轮真正拿得到的工具名（决策 247）：编排侧拿它建 `ToolExecutor`（同源）。
    pub available: Vec<&'static str>,
    pub system_prompt: String,
    /// **恒空**（不变量 3）：快照并进本轮最后一条 user 轮。
    pub user_prompt: String,
    pub tools: Vec<ToolDef>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub provider_id: Option<String>,
    /// `None` = 跳过分档（决策 110：不臆造窗口）。
    pub capacity: Option<ContextCapacity>,
    pub keep_recent_rounds: usize,
    /// L3 硬底（票 03）：转录字符量超过它就强制压缩，与 80% 那条线取「或」。
    /// 与流水线同源（决策 291）——同一个 `Settings.conversation_max_tokens`
    /// （long-run-budget 票 02：触发判据从字符硬底改为 token 硬底）。
    pub conversation_max_tokens: usize,
    /// 本轮**合并轮的全文**（快照 + 问题，不是裸问题——叫 `question` 会名不副实）：
    /// 轮内压缩按内容倒着找它当锚点（不变量 1）。
    pub merged_round: String,
    pub idle_timeout_sec: Option<u64>,
    /// 种子转录：历史 → 尾部注入（锚点 / 互喂摘要）→ 本轮合并轮。
    /// 由 [`TurnPlan::take_transcript`] 交给编排侧推进。
    transcript: Vec<Message>,
}

/// 每一轮模型调用之前的**成本门**裁定（决策 292 / 票 07）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostVerdict {
    /// 还没到线：照常发下一次调用。
    KeepGoing,
    /// 人的那一轮过了线：**只落软告警不拦**（终点由人定）。
    WarnHuman,
    /// 值守轮触顶：出循环走收口路径（部分结论 + 【未收口】）。
    StopWatch,
}

/// 成本门的判据（纯）。
///
/// 分档是这条线的全部意义：值守轮没有人在场，触顶即停；人的那一轮无硬界——同一根线
/// 在那里只说明「已经烧了这么多」。
pub fn cost_verdict(is_watch: bool, generated_tokens: u32, line: u32) -> CostVerdict {
    if generated_tokens < line {
        return CostVerdict::KeepGoing;
    }
    if is_watch {
        CostVerdict::StopWatch
    } else {
        CostVerdict::WarnHuman
    }
}

impl TurnPlan {
    /// provider 解析（决策 182②）：按 foreman **自己的 key** 读阶段配置，取它的 provider。
    ///
    /// 组装要它来登记身份，编排侧要它来选摘要器与窗口——**一处解析、两处引用**。
    /// 任务级覆盖恒为 `None`：值班长不挂任务，没有任务级可覆盖。
    pub fn provider_id(cfg: Option<&StageConfig>) -> Option<String> {
        crate::storage::catalog::resolve_provider_id(None, None, cfg, None)
    }

    /// 环境层档位（决策 206）：阶段配置 → 该阶段缺省 → 全局默认。缺省 `ask`。
    fn env_mode(settings: &Settings, cfg: Option<&StageConfig>) -> EnvMode {
        crate::types::effective_env_mode(settings.env_mode, FOREMAN_STAGE_KEY, cfg)
    }

    /// 这一轮真正拿得到的工具名（决策 247）：值守轮先摘掉贵的两件（分级诊断，票 07）。
    fn available_tools(env_mode: EnvMode, is_watch: bool) -> Vec<&'static str> {
        let deny: &[&str] = if is_watch {
            &FOREMAN_WATCH_TOOL_DENY
        } else {
            &[]
        };
        foreman_available_tools_except(env_mode, deny)
    }

    /// 组装：事实进，计划出（纯；每轮恰一次）。
    pub fn assemble(facts: TurnFacts<'_>) -> TurnPlan {
        // 档位与广告集**先算**（不变量 4）：下面 `system_prompt` 的纪律段吃同一份
        // `available`——顺序是这条承诺的全部。
        let env_mode = Self::env_mode(facts.settings, facts.cfg);
        let available = Self::available_tools(env_mode, facts.is_watch);
        let system_prompt = system_prompt(
            facts.persona.unwrap_or(FOREMAN_PERSONA),
            facts.cfg.and_then(|c| c.persona_append.as_deref()),
            env_mode,
            facts.stewarded,
            &available,
        );
        let user_prompt = String::new();

        // 容量算术（决策 291 / 票 06(b)）：窗口来自 provider 行，比例来自 Settings，
        // 触发线从流水线的软限抬到 80%（不变量 6）。查不到窗口 → `None`（不变量 5）。
        let capacity = facts.model_window.map(|window| {
            let mut capacity =
                estimate_context_capacity(window, &system_prompt, &user_prompt, facts.settings);
            capacity.soft_limit = (capacity.total as f64 * FOREMAN_INLOOP_COMPACT_RATIO) as usize;
            capacity
        });

        // 逐调用空闲界（决策 288 / 票 05）：foreman 行的 `idle_timeout_sec` > 全局
        // `node_idle_timeout_sec`（缺省 300s，与节点同一个数）。每一次模型调用各带一份
        // ——「N 秒没有新字节」判的是单次调用，不是整轮。
        let idle_timeout_sec = crate::config::effective_idle_timeout(
            facts.settings.node_idle_timeout_sec,
            facts.cfg.and_then(|c| c.idle_timeout_sec),
            crate::config::NodeTimeouts::default(),
        );
        let idle_timeout_sec = Some(idle_timeout_sec);

        let tools = tool_defs(&available);

        // 三种角色 → 两种说话的立场（决策 204 / 207）。**操作台记的那几轮（`system`）必须与
        // 值班长自己的话分开**：写成助理轮，它下一轮读历史时会把「提议已执行：写文件 notes.md」
        // 当成自己说过的话——那正是人格第一条纪律（不得声称自己动了手）要挡的东西。
        //
        // 转写成 `user` 而不是丢掉：丢掉它，模型就不知道人按了什么键，会以为提议还挂着
        // （于是重提一遍）。剩下的问题是「user 这一侧还有值班经理」——故加一句与前缀一起
        // 说明发言者是谁，而不是靠角色去暗示。
        let mut transcript: Vec<Message> = facts
            .window
            .iter()
            .map(|m| {
                if m.role == FOREMAN_ROLE_USER {
                    Message::user(m.content.clone())
                } else if m.role == FOREMAN_ROLE_SYSTEM {
                    Message::user(format!(
                        "{OPERATION_LOG_MARK}（操作台记的一轮）\n{}",
                        m.content
                    ))
                } else {
                    Message::assistant(Some(m.content.clone()), Vec::new())
                }
            })
            .collect();

        // ── 本轮问题的定位必须在**注入之前**（不变量 2）：它是「历史里最后一条 user」
        // 这件事在读历史那一刻成立的，不是读注入之后。人的那一轮问题已随历史落库
        // （`say()` 先落 user 行），故它与快照合并的是**历史里那一条**；值守简报反过来
        // **总是**新建一条（历史里最后那条 user 是本轮刚落的人话，不能靠「已经有了」跳过
        // ——那会让这一轮真正要处理的东西消失）。
        let final_text = match (facts.is_watch, transcript.last()) {
            (false, Some(m)) if m.role == Role::User => {
                let last = transcript.pop().expect("上一行刚判过 Some");
                let base = last.content.unwrap_or_else(|| facts.question.to_string());
                format!("{}\n{}", facts.briefing_text, base)
            }
            _ => format!("{}\n{}", facts.briefing_text, facts.question),
        };

        // ── 尾部注入（不变量 2）：锚点与互喂摘要追加在**历史之后、本轮问题之前**，
        // 不再 splice 在转录头部。标记、不落库、失败不注入不报错（决策 269④）一概不变。
        if let Some(anchor) = facts.anchor {
            // 锚点以**标记 user 轮**注入（system 行重注入走 user 的先例）。
            transcript.push(Message::user(format!("{COMPACTION_MARK}\n{anchor}")));
        }
        if let Some(summary) = facts.cross_digest {
            transcript.push(cross_digest_message(facts.is_watch, summary));
        }
        transcript.push(Message::user(final_text.clone()));

        TurnPlan {
            session_id: facts.session_id.to_string(),
            env_mode,
            available,
            system_prompt,
            user_prompt,
            tools,
            temperature: facts.cfg.and_then(|c| c.temperature),
            max_tokens: facts.cfg.and_then(|c| c.max_tokens),
            provider_id: Self::provider_id(facts.cfg),
            capacity,
            keep_recent_rounds: facts.settings.keep_recent_rounds,
            conversation_max_tokens: facts.settings.conversation_max_tokens,
            merged_round: final_text,
            idle_timeout_sec,
            transcript,
        }
    }

    /// 取走种子转录：编排侧在循环里推进它，plan 此后只提供裁定与请求字段。
    pub fn take_transcript(&mut self) -> Vec<Message> {
        std::mem::take(&mut self.transcript)
    }

    /// 组装一次模型请求（决策 182② 的身份戳：占位阶段 + `run` 里的会话身份）。
    ///
    /// 占位阶段让既有的 provider 解析链跑通，真正生效的 provider 从 `provider_id` 进来；
    /// `task_id` / `branch` 恒空串、`run_id` 恒 0——既有任务级 SSE 路由按 task id 精确匹配，
    /// 空串永不等于真实任务 id，故零干扰（决策 182⑥）。
    pub fn request(&self, messages: Vec<Message>) -> LlmRequest {
        LlmRequest {
            stage: Stage::Init,
            node: Node::Execute,
            attempt: 1,
            system_prompt: self.system_prompt.clone(),
            user_prompt: self.user_prompt.clone(),
            messages,
            tools: self.tools.clone(),
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            provider_id: self.provider_id.clone(),
            run: Some(RunContext {
                task_id: String::new(),
                branch: String::new(),
                run_id: 0,
                agent_type: FOREMAN_AGENT_TYPE.to_string(),
                // 会话身份（决策 204⑥）：手机与电脑同时连着时，前端靠它把增量归到
                // 正确的会话，而不是把两台设备的回话混成一段。
                session_id: self.session_id.clone(),
            }),
            idle_timeout_sec: self.idle_timeout_sec,
        }
    }

    /// 轮内预算门（票 06(b)）：过线就按轮压缩，返回压掉的段数（0 = 没触发 / 压不动）。
    ///
    /// 判据与流水线逐字同源（[`should_compact_with_floor`]，long-run-budget 票 02）：
    /// 80% 那条线（[`FOREMAN_INLOOP_COMPACT_RATIO`] 经 capacity 进来）**或** token 估算
    /// 超 `conversation_max_tokens`——硬底是绝对数，不看 provider 窗口登记的脸色，
    /// `capacity = None` 时硬底照判。
    pub fn check_window_budget(&self, messages: &mut Vec<Message>) -> usize {
        let estimate = estimate_messages_tokens(&self.system_prompt, &self.user_prompt, messages);
        if !should_compact_with_floor(estimate, self.capacity, self.conversation_max_tokens) {
            return 0;
        }
        self.compact_forced(messages)
    }

    /// 无条件压一轮（票 06(b) 的触发与 (c) 的撞墙恢复共用）。
    pub fn compact_forced(&self, messages: &mut Vec<Message>) -> usize {
        let before = messages.len();
        let start = round_start_of(messages, &self.merged_round);
        let outcome = compact_messages_from(messages, self.keep_recent_rounds, start);
        if outcome.compacted_messages == 0 {
            return 0;
        }
        tracing::info!(
            before,
            after = outcome.messages.len(),
            compacted = outcome.compacted_messages,
            "值班长轮内上下文超线，已按轮压缩（票 06(b)）"
        );
        *messages = outcome.messages;
        outcome.compacted_messages
    }
}

/// 本轮起点的下标（决策 291 / 票 06(b)）：`transcript` 里承载「这一轮要处理的那句话」的那条
/// 消息。压缩拿它当锚点（[`compact_messages_from`] 的 `current_start`）——载入的历史不得顶替
/// 它，否则真正的起点会被压成摘要。
///
/// 按内容倒着找而不是记住一个下标：压缩会重排下标，而这句话本身不变。
fn round_start_of(transcript: &[Message], question: &str) -> usize {
    transcript
        .iter()
        .rposition(|m| m.role == Role::User && m.content.as_deref() == Some(question))
        .unwrap_or(0)
}

/// 跨时间线互喂那一条注入消息（决策 289 / 票 03）：方向由**这一轮是谁**定——
/// 人的那一轮读值守台账的摘要，值守轮读最近活动的人的班次。
fn cross_digest_message(is_watch: bool, summary: &str) -> Message {
    let mark = if is_watch {
        FOREMAN_TALK_DIGEST_MARK
    } else {
        FOREMAN_WATCH_DIGEST_MARK
    };
    Message::user(format!(
        "{mark}（以下是对方时间线的摘要，不是原文；原始轮次在它自己的台账里）\n{summary}"
    ))
}

/// 工具定义：**从清单生成**（票 01）。
///
/// 与 [`crate::pipeline::subagent::StoreSubAgentRunner::tool_defs`] 同样的立场：
/// **不经 `effective_tools`**——那条路会并入基线强制工具（含 `run_command` / `write_file`），
/// 正是本模块要挡掉的东西。
///
/// 手写这两个 `ToolDef` 的时候，广告集与执行点白名单是两份独立的名单，而「同源」是票 01 的
/// 硬要求：两处各写一份名字，迟早出现「模型看得见一个调用就被拒的工具」这种不好定位的错。
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

/// 系统提示词：`[须知][人格][工具纪律]`，人格可被 `persona_path` / `persona_append` 覆盖。
///
/// 不注入技能（与只读子代理同一立场）：值班长是面向人的对话者，不是技能执行者；
/// 把项目技能正文灌进来只会挤占历史窗口。
fn system_prompt(
    persona: &str,
    persona_append: Option<&str>,
    env_mode: EnvMode,
    stewarded: bool,
    available: &[&'static str],
) -> String {
    let mut out = format!("{FOREMAN_BASELINE}\n\n{persona}\n");
    if let Some(append) = persona_append {
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
    out.push_str(&format!("{}\n", power_discipline(env_mode, stewarded)));
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
    out
}

/// 「你能动手到什么程度」那一段（决策 206 / 188）。
///
/// **按档位写，不写一句笼统的「你没有权限」**：模型是照着这段描述自己汇报的，
/// 描述与事实不符时它会说出与事实不符的话（「我已经写好了」/「我读不到文件」）。
/// 这一段与 `ToolExecutor` 那道闸是同一件事的两种说法——一处给模型看，一处真的执行。
///
/// **托管那一段是条件说的**（决策 210① / 票 08）：只有真开着托管的方案才说明它的存在，
/// 否则模型会以为自己对任何任务都能免按键动手——而它实际只对**被托管的那几个**能。
fn power_discipline(env_mode: EnvMode, stewarded: bool) -> String {
    let mut discipline = match env_mode {
        EnvMode::Ask => "\
             - 文件与命令这类**会改动东西**的动作：你调用之后**不会立即发生**，\
             而是生成为一条待确认的提议，等值班经理在界面上按下确认钮才真正执行。\
             `read_file` / `list_dir` / `Skill` 是只读的，直接执行。\
             - 因此**绝不要说你已经做了那件事**：你可以说「我提了一条建议，等你按键」。\
             - 本服务自己的写接口（建任务、拍板、合入、改配置……）一律走提议，\
             这件事不随档位变。\n"
            .to_string(),
        EnvMode::Auto => "\
             - 文件与命令这类动作**会立即执行**（这个阶段被配成 auto 档）。执行结果\
             会以工具回执的形式回来，写进时间线；你汇报时以回执为准，不要凭印象说。\
             - 本服务自己的写接口（建任务、拍板、合入、改配置……）**仍然**要人按键，\
             不随档位变——那类动作会改变流水线的事实。\n"
            .to_string(),
        EnvMode::Deny => "\
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::client::Message;
    use chrono::{DateTime, Utc};

    fn settings() -> Settings {
        Settings::default()
    }

    fn row(id: i64, role: &str, content: &str) -> ForemanMessage {
        ForemanMessage {
            id,
            session_id: "s1".into(),
            role: role.into(),
            content: content.into(),
            prompt_tokens: 0,
            completion_tokens: 0,
            briefing_json: None,
            traces_json: None,
            segments_json: None,
            thinking: None,
            ask_json: None,
            status: None,
            seq: 0,
            interrupted_at: None,
            created_at: DateTime::<Utc>::from_timestamp(1_700_000_000 + id, 0).unwrap(),
        }
    }

    fn facts<'a>(
        settings: &'a Settings,
        window: &'a [ForemanMessage],
        question: &'a str,
    ) -> TurnFacts<'a> {
        TurnFacts {
            settings,
            session_id: "s1",
            is_watch: false,
            cfg: None,
            window,
            anchor: None,
            cross_digest: None,
            briefing_text: "【态势】一切正常",
            question,
            stewarded: false,
            persona: None,
            model_window: None,
        }
    }

    fn text_of(m: &Message) -> &str {
        m.content.as_deref().unwrap_or("")
    }

    #[test]
    fn the_turn_question_is_the_last_message_and_the_briefing_rides_with_it() {
        let s = settings();
        let window = vec![row(1, FOREMAN_ROLE_USER, "第一句")];
        let plan = TurnPlan::assemble(facts(&s, &window, "第一句"));
        let mut plan = plan;
        let transcript = plan.take_transcript();

        assert_eq!(
            transcript.len(),
            1,
            "人的那一轮：历史那条 user 就是本轮问题"
        );
        assert_eq!(transcript[0].role, Role::User);
        assert!(text_of(&transcript[0]).starts_with("【态势】一切正常\n"));
        assert!(
            text_of(&transcript[0]).ends_with("第一句"),
            "问题仍是最后一句"
        );
    }

    #[test]
    fn a_watch_turn_always_appends_its_own_question_row() {
        let s = settings();
        let window = vec![row(1, FOREMAN_ROLE_USER, "人说的话")];
        let plan = TurnPlan::assemble(TurnFacts {
            is_watch: true,
            question: "【值守播报】简报正文",
            ..facts(&s, &window, "【值守播报】简报正文")
        });
        let mut plan = plan;
        let transcript = plan.take_transcript();

        assert_eq!(transcript.len(), 2, "历史 + 本轮的简报，一条不少一条不多");
        assert_eq!(
            text_of(&transcript[1]),
            "【态势】一切正常\n【值守播报】简报正文"
        );
        assert_eq!(
            text_of(&transcript[0]),
            "人说的话",
            "历史那条不被顶替，也不被合并"
        );
    }

    #[test]
    fn injections_land_between_the_history_and_the_turn_question() {
        let s = settings();
        let window = vec![
            row(1, FOREMAN_ROLE_USER, "旧问题"),
            row(2, "assistant", "旧回答"),
            row(3, FOREMAN_ROLE_USER, "新问题"),
        ];
        let mut plan = TurnPlan::assemble(TurnFacts {
            anchor: Some("早先那些轮的摘要"),
            cross_digest: Some("对方时间线的摘要"),
            ..facts(&s, &window, "新问题")
        });
        let transcript = plan.take_transcript();

        // 人的那一轮，末尾那条 user 被取出来与快照合并（它成了最后一条）：
        // 历史剩 2 条 → 锚点 → 互喂摘要 → 合并轮，共 5 条。
        assert_eq!(transcript.len(), 5);
        assert_eq!(text_of(&transcript[0]), "旧问题");
        assert_eq!(text_of(&transcript[1]), "旧回答");
        assert!(
            text_of(&transcript[2]).starts_with(COMPACTION_MARK),
            "锚点在前"
        );
        assert!(
            text_of(&transcript[3]).starts_with(FOREMAN_WATCH_DIGEST_MARK),
            "互喂摘要在后"
        );
        assert_eq!(transcript[4].role, Role::User, "本轮问题永远是最后一条");
        assert!(text_of(&transcript[4]).ends_with("新问题"));
        assert!(
            text_of(&transcript[4]).starts_with("【态势】一切正常\n"),
            "快照与它合并"
        );
    }

    /// 不变量 2 的牙齿：注入不改变**它之前**那一段的任何一条消息——稳定前缀
    /// （system + 历史去掉被合并的那条）逐轮不变，provider 的 prefix 缓存才命中。
    /// 头部注入会让这里整段翻面。
    #[test]
    fn injections_keep_the_history_prefix_byte_identical() {
        let s = settings();
        let window = vec![
            row(1, FOREMAN_ROLE_USER, "旧问题"),
            row(2, "assistant", "旧回答"),
            row(3, FOREMAN_ROLE_USER, "新问题"),
        ];
        let without = {
            let mut p = TurnPlan::assemble(facts(&s, &window, "新问题"));
            p.take_transcript()
        };
        let with = {
            let mut p = TurnPlan::assemble(TurnFacts {
                anchor: Some("摘要"),
                cross_digest: Some("互喂"),
                ..facts(&s, &window, "新问题")
            });
            p.take_transcript()
        };
        // 稳定前缀 = 历史里**不参与合并**的那几条（末尾那条 user 会被取出来重排到最后）。
        for i in 0..2 {
            assert_eq!(
                (with[i].role, text_of(&with[i])),
                (without[i].role, text_of(&without[i])),
                "第 {i} 条历史被注入改动过"
            );
        }
        assert_eq!(
            (with.last().map(|m| m.role), with.last().map(text_of)),
            (without.last().map(|m| m.role), without.last().map(text_of)),
            "本轮问题在两侧都是同一条、同一个位置（末尾）"
        );
        assert_eq!(without.len(), 3, "无注入时：2 条历史 + 合并轮");
        assert_eq!(with.len(), 5, "有注入时：注入夹在历史与合并轮之间");
    }

    #[test]
    fn the_system_rail_is_empty_and_the_history_never_rides_in_it() {
        let s = settings();
        let window = vec![row(1, FOREMAN_ROLE_USER, "问题")];
        let plan = TurnPlan::assemble(facts(&s, &window, "问题"));
        assert!(plan.user_prompt.is_empty(), "user 槽位恒空（不变量 3）");
        assert!(plan.system_prompt.contains("## 工具纪律"));
    }

    #[test]
    fn the_watch_turn_denies_the_expensive_tools_before_the_prompt_is_built() {
        let s = settings();
        let window = vec![row(1, FOREMAN_ROLE_USER, "问题")];
        let plan = TurnPlan::assemble(TurnFacts {
            is_watch: true,
            ..facts(&s, &window, "问题")
        });
        // 纪律段里那两份**枚举名单**是不变量 4 的对象：它们从 `available` 派生，
        // 故被摘掉的名字一个都不该出现在里面。按 ` / ` 切**名字**来比，不做子串判断
        // ——`task` 名字里就含 `ask` 三个字母。
        let named: Vec<String> = plan
            .system_prompt
            .lines()
            .filter(|l| l.contains("工具是："))
            .flat_map(|l| {
                l.split_once("工具是：")
                    .map(|(_, names)| names)
                    .unwrap_or("")
                    .trim_end_matches('。')
                    .split(" / ")
                    .map(|n| n.trim().to_string())
                    .collect::<Vec<_>>()
            })
            .collect();
        assert!(
            named.contains(&"read_file".to_string()),
            "这一轮有只读工具：{named:?}"
        );
        for denied in FOREMAN_WATCH_TOOL_DENY {
            assert!(!plan.available.contains(&denied), "值守轮拿不到 {denied}");
            assert!(
                !named.iter().any(|n| n == denied),
                "名单里不许广告一个这一轮已被摘掉的工具：{denied}"
            );
        }
        assert_eq!(
            plan.tools.len(),
            plan.available.len(),
            "广告集与执行点白名单同源（决策 247）"
        );
    }

    #[test]
    fn a_window_the_provider_never_registered_skips_both_budget_gates() {
        let s = settings();
        let window = vec![row(1, FOREMAN_ROLE_USER, "问题")];
        let mut plan = TurnPlan::assemble(facts(&s, &window, "问题"));
        assert!(plan.capacity.is_none(), "不臆造窗口（决策 110）");
        let mut transcript = plan.take_transcript();
        assert_eq!(plan.check_window_budget(&mut transcript), 0);
    }

    #[test]
    fn an_over_window_transcript_is_compacted_into_an_anchor_round() {
        let s = settings();
        // 一条小窗口（1200 token）× 80% = 960：下面的历史早就过线了。
        let window: Vec<ForemanMessage> = (1..=8)
            .map(|i| row(i, FOREMAN_ROLE_USER, &"很长的历史".repeat(60)))
            .collect();
        let mut plan = TurnPlan::assemble(TurnFacts {
            model_window: Some(1_200),
            ..facts(&s, &window, "问题")
        });
        assert!(plan.capacity.is_some());
        let mut transcript = plan.take_transcript();
        let before = transcript.len();
        let compacted = plan.check_window_budget(&mut transcript);
        assert!(compacted > 0, "过线就该压（触发线 80%）");
        assert!(transcript.len() < before, "压完条数必须变少");
        assert!(
            plan.check_window_budget(&mut transcript) == 0 || transcript.len() < before,
            "压不动时返回 0，绝不空转"
        );
    }

    #[test]
    fn a_transcript_well_inside_the_window_is_left_alone() {
        let s = settings();
        let window = vec![row(1, FOREMAN_ROLE_USER, "问题")];
        let mut plan = TurnPlan::assemble(TurnFacts {
            model_window: Some(200_000),
            ..facts(&s, &window, "问题")
        });
        let mut transcript = plan.take_transcript();
        let before = transcript.clone();
        assert_eq!(plan.check_window_budget(&mut transcript), 0, "预算内不压");
        assert_eq!(transcript, before, "一字未动");
    }

    #[test]
    fn the_cost_line_stops_a_watch_turn_and_only_warns_a_human_one() {
        assert_eq!(cost_verdict(true, 0, 120_000), CostVerdict::KeepGoing);
        assert_eq!(cost_verdict(true, 119_999, 120_000), CostVerdict::KeepGoing);
        assert_eq!(cost_verdict(true, 120_000, 120_000), CostVerdict::StopWatch);
        assert_eq!(cost_verdict(true, 999_999, 120_000), CostVerdict::StopWatch);
        // 同一根线在人的那一轮只落软告警（终点由人定，决策 292）。
        assert_eq!(
            cost_verdict(false, 119_999, 120_000),
            CostVerdict::KeepGoing
        );
        assert_eq!(
            cost_verdict(false, 120_000, 120_000),
            CostVerdict::WarnHuman
        );
        assert_eq!(
            cost_verdict(false, 999_999, 120_000),
            CostVerdict::WarnHuman
        );
    }

    #[test]
    fn the_request_carries_the_session_identity_and_the_empty_user_slot() {
        let s = settings();
        let window = vec![row(1, FOREMAN_ROLE_USER, "问题")];
        let mut plan = TurnPlan::assemble(facts(&s, &window, "问题"));
        let messages = plan.take_transcript();
        let request = plan.request(messages.clone());
        assert_eq!(request.messages.len(), messages.len());
        assert!(request.user_prompt.is_empty(), "user 槽位恒空（不变量 3）");
        assert_eq!(request.tools.len(), plan.tools.len());
        let run = request.run.expect("值班长的请求恒带身份戳");
        assert_eq!(run.session_id, "s1");
        assert_eq!(run.task_id, "", "不挂任务（决策 182⑨）");
        assert_eq!(run.run_id, 0);
    }

    #[test]
    fn the_round_anchor_is_found_by_content_not_by_index() {
        let s = settings();
        let window = vec![row(1, FOREMAN_ROLE_USER, "问题")];
        let mut plan = TurnPlan::assemble(facts(&s, &window, "问题"));
        let mut transcript = plan.take_transcript();
        // 模拟循环推进：历史重排之后这句话仍按内容找得到。
        transcript.insert(0, Message::user("更早的一句话"));
        let start = round_start_of(&transcript, &plan.merged_round);
        assert_eq!(
            text_of(&transcript[start]),
            plan.merged_round,
            "锚点是那句话本身，不是记住了的下标"
        );
        let fallback = round_start_of(&transcript, "找不到的一句话");
        assert_eq!(fallback, 0, "找不到就退到 0（不 panic）");
    }

    #[test]
    fn the_config_rows_ride_into_the_plan() {
        let s = settings();
        let cfg = StageConfig {
            stage: FOREMAN_STAGE_KEY.into(),
            temperature: Some(0.3),
            max_tokens: Some(2048),
            idle_timeout_sec: Some(900),
            env_mode: Some(EnvMode::Auto),
            ..StageConfig::default()
        };
        let window = vec![row(1, FOREMAN_ROLE_USER, "问题")];
        let plan = TurnPlan::assemble(TurnFacts {
            cfg: Some(&cfg),
            ..facts(&s, &window, "问题")
        });
        assert_eq!(plan.temperature, Some(0.3));
        assert_eq!(plan.max_tokens, Some(2048));
        assert_eq!(plan.env_mode, EnvMode::Auto, "阶段行覆盖全局缺省");
        assert_eq!(plan.idle_timeout_sec, Some(900));
    }
}
