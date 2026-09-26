//! 模型调用编排（决策 249 · 片③，票 03）：agent 节点循环（重试 / 工具往返 / 元数据 /
//! 异族复判 / 会话落库）与**并入的伪阶段**（conflict_check / validator_cross_check /
//! 语义冲突 / project_analysis——同构：一次 LLM 调用 + 一条 run 行）收成一片。
//!
//! 依赖**显式化**（决策 249 Q2）：本片只拿票面点名的那几个——`store` / `settings` / `llm` /
//! `killer` / `sse` / `clock`，**不伸手拿 `&Executor`**（拆完后全仓 `&Executor` 参数归零，
//! `post_process` 的 9 臂按臂拆清）。与两翼的接缝：
//!
//! - **01 模型请求组装**：每 attempt 一次 [`RequestPlan::assemble`]、每轮
//!   [`RequestPlan::check_budget`] + [`RequestPlan::request`]；**Overflow 的翻译在本片**
//!   ——先落快照与会话行、再 `NodeOutput::Pending(context_overflow)`：构造落点是编排的
//!   职责（决策 245「门吃落点不吃原因」：01 检测、03 翻译）。
//! - **02 run 台账**：`begin / mark_step / finish / finish_cancelled / link / take` 全经
//!   [`RunLedger`]，承重顺序四条在本片的调用侧兑现（见 `run_ledger` 模块 doc）。
//!
//! **SSE 发射留守**：事件形状的唯一出口在 `executor`（`emit_tool_event` /
//! `emit_node_started` / `finish_run_with_sse`）——本片经既有 emit 函数调用，不自建事件面。
//! 外部 6 触点里的 `try_run` / `run` 编排外壳与 `project_analysis`（内部转发）都留守核。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::agent::client::{LlmClient, LlmRequest, Message};
use crate::agent::metadata::parse_metadata;
use crate::agent::prompts::{build_system_prompt, load_agents_context};
use crate::agent::tools::{ToolCallContext, ToolExecutor};
use crate::clock::Clock;
use crate::config::Settings;
use crate::pipeline::pseudo::{ConflictCheckResult, CrossCheckResult, PseudoStage};
use crate::process::ProcessKiller;
use crate::sse::{SseSink, ToolPhase};
use crate::storage::observability::{NewRun, PromptSnapshot, RunOutcome};
use crate::storage::Store;
use crate::types::{
    CommandSource, DuplicateRisk, Gate, GateFailureKind, Node, NodeCursor, NodeStatus,
    PendingContext, PendingKind, PendingReason, Project, Stage, Task,
};
use crate::{Error, Result};

use super::executor::{
    cancel_signal, emit_node_started, emit_tool_event, finish_run_with_sse, project_or_err,
    NodeOutput, OUTPUT_CODE_CHANGES, OUTPUT_DESIGN_DOC, OUTPUT_DEV_DOC, OUTPUT_REVIEW_REPORT,
    OUTPUT_TEST_REPORT, OUTPUT_TEST_SCENARIOS, PIPELINE_AGENT_TYPE,
};
use super::model_request::{
    json_string_list, workdirs_line, AttemptCtx, BudgetCheck, OverflowFacts, Prepared, RequestPlan,
};
use super::run_ledger::{since_ms, RunLedger};
use crate::pipeline::subagent::RunTokens;

/// 一次尝试的失败现场：错误 + **这一轮已经烧掉的 token**（决策 226）+ **这一轮的
/// 完整对话**（决策 278）。
///
/// 前两者必须一起回来：此前失败路径给 `finish_run` 传的是 `RunTokens::default()`，于是台账
/// 里的「0」同时意味着两件事：「一次模型调用都没发生」与「发生了但记账丢了」。2026-09-19
/// 值班长正是据那个 0 推出「两次尝试连第一次 LLM 调用都没落账」，把它当成关键证据报了
/// 四轮——而同一个 0 也长在死因完全已知的 run 上（init 的 `git 操作超时（180s）`）。
/// 一个既是读数又是哨兵的字段，读的人只能猜；把真读数带上，它才只是读数。
///
/// `messages` 服务的是另一件事（决策 278，显式修订决策 205 裁决②）：自动重试的下一轮
/// 从「这份转录原样保留 + 一条错误 turn」起步，不再空对话——探索只付一次钱（run40/41
/// 同批文件读了三遍的实测教训）。
struct AttemptFailure {
    error: Error,
    tokens: RunTokens,
    messages: Vec<Message>,
}

/// `tool_event` 的参数摘要（决策 123：只给摘要，不外发全量参数）。
fn args_summary(text: &str) -> String {
    const LIMIT: usize = 120;
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.chars().count() <= LIMIT {
        compact
    } else {
        let cut: String = compact.chars().take(LIMIT).collect();
        format!("{cut}…")
    }
}

/// 一次 agent 尝试的现场（票 01 / 决策 211①）。
///
/// 失败也要落会话，所以现场必须活到函数出口——`?` 会把内存里的 `messages` 一起带走，
/// 那正是那次实测「失败的那一轮什么都不留」的机制。`persisted` 保证一条 run 至多一条
/// 会话行（决策 99）：成功路径已经写过时，失败收尾只往那一行补错误上下文。
#[derive(Default)]
struct AttemptTrace {
    messages: Vec<Message>,
    tokens: RunTokens,
    /// 组装后的两段原文（票 02）：**成功与失败都要写**——「这是 prompt 问题」这句判断
    /// 在失败的那一轮才最需要证据。
    prompts: Option<(String, String)>,
    persisted: bool,
}

/// 失败会话写在 `metadata_json` 里的上下文（票 01）：读会话的人先看到它，才知道这条
/// 对话为什么停在这里。可归因的 LLM 失败额外带上类别与原始诊断——它们回答的是
/// 「该去改什么」，与 `error` 那句「发生了什么」不是一回事。
fn failure_metadata(error: &Error) -> serde_json::Value {
    let mut meta = serde_json::json!({
        "failed": true,
        "error": error.to_string(),
    });
    if let Some((kind, raw)) = error.llm_classified() {
        meta["classified"] = serde_json::json!({ "kind": kind, "raw": raw });
    }
    meta
}

/// 校验类失败的用户可见文案（决策 278「报错人话化」）：裸诊断串对用户不是话。
/// 只翻译能确定的几族签名，认不出的一律原文返回——宁可给原始串，不给错误的翻译。
/// 措辞对 validate / execute 两类节点通用（「输出」而非「判定」）。原始诊断不受
/// 影响：run 行的 error 仍是 `last_error` 原文。
fn humanize_agent_failure(raw: &str) -> String {
    if raw.contains("未找到结构化元数据") || raw.contains("缺少结构化元数据") {
        "输出未按契约提交（没有等到 submit_metadata）".to_string()
    } else if raw.contains("元数据校验失败") {
        "已提交的内容不符合输出契约".to_string()
    } else if raw.contains("工具参数 JSON 解析失败") {
        "提交的参数不是合法 JSON".to_string()
    } else if raw.contains("工具失败超过 tool_retry_max") {
        "工具连续失败超过上限".to_string()
    } else {
        raw.to_string()
    }
}

/// 读「用户补充输入」的正文（决策 279）：`user-input.md` 是决策 79 落盘的留痕
/// （带 `# 用户补充输入` 标题行），user turn 要的是用户说过的话——剥掉标题行取正文。
/// 文件不存在 / 正文为空 → `None`（落盘纪律不动，读不到就当没有补充）。
fn supplement_input(home: &crate::home::Home, task_id: &str) -> Option<String> {
    let raw = std::fs::read_to_string(home.task_file(task_id, "user-input.md")).ok()?;
    let body = raw.strip_prefix("# 用户补充输入").unwrap_or(&raw).trim();
    (!body.is_empty()).then(|| body.to_string())
}

/// 模型调用编排片的依赖面（决策 249 · 票 03）：只拿票面点名的那几个，字段全可廉价克隆
/// （连接池 / Arc / 配置）——留守核每次派发 `clone` 一份，不借 `&Executor`。
pub(crate) struct ModelInvoke {
    pub(crate) store: Store,
    pub(crate) settings: Settings,
    pub(crate) llm: Arc<dyn LlmClient>,
    pub(crate) killer: Arc<dyn ProcessKiller>,
    pub(crate) sse: Arc<dyn SseSink>,
    pub(crate) clock: Arc<dyn Clock>,
}

impl ModelInvoke {
    /// 台账入口（经 02 的 interface；计时读数与留守核同源——同一个 `Clock` 实例）。
    fn ledger(&self) -> RunLedger<'_> {
        RunLedger::new(&self.store, self.clock.as_ref())
    }

    /// agent 节点：独立对话（决策 33）+ 工具真实执行（决策 148）+
    /// `agent_retry_max` 重试（决策 33 / G13 分层计数；决策 278 起重试轮续接转录＋错误 turn，
    /// 不再是「干净对话重试」——显式修订决策 205 裁决②）。
    pub(crate) async fn agent_node(&self, task: &Task, cursor: &NodeCursor) -> Result<NodeOutput> {
        let kind = AgentNodeKind::of(cursor.stage, cursor.node).ok_or_else(|| {
            Error::Validation(format!("{}.{} 不是 agent 节点", cursor.stage, cursor.node))
        })?;
        let project = project_or_err(&self.store, &task.project_id).await?;

        let mut last_error = String::new();
        // 可归因的 LLM 配置类失败（主流程票 03）：跨 attempt 保留最近一次的
        // (类别, 原始诊断)，耗尽后随重试耗尽错误一起带给 pending——否则它会被
        // 下面那层 `Error::Validation` 包装吞掉，用户只剩一段没有任何指引的文本。
        let mut last_classified: Option<(String, String)> = None;
        // 续接素材只在**进循环之前**取一次（红线④ / 决策 180：读清语义与「为什么只取
        // 一次」见 `run_ledger` 模块 doc；resume 边界每轮只有一个，循环内的重试续接
        // 走决策 278 的转录保留，不经这里）。
        let continuation = self.ledger().take_continuation(cursor).await?;
        // 决策 279：info_insufficient 续接时，补充输入作为 user turn 追加到**转录末尾**
        // （不再经 segment 重渲染进首条消息——那会把 prompt 前缀全变、缓存打穿，
        // 且转录里查无此人）。只在真追加了 turn 时才停用 segment：
        // `AttemptCtx.user_input_as_turn`。
        let mut carried: Vec<Message> = continuation
            .as_ref()
            .map(|c| c.messages.clone())
            .unwrap_or_default();
        let mut supplement_as_turn = false;
        if matches!(cursor.node, Node::ValidateInput) && !carried.is_empty() {
            if let Some(text) = supplement_input(self.store.home(), &task.id) {
                // 决策 79 的落盘纪律是「空输入不落文件」但**也不清旧档**——同一
                // info_insufficient 第二次「继续」且不带新输入时，文件里还是上一次
                // 的补充，而它已经作为 user turn 在 carried 转录里了。转录里已有的
                // 同文 turn 不再追加（评审发现的重放边缘）。
                let already_carried = carried.iter().any(|m| {
                    m.role == crate::agent::client::Role::User
                        && m.content.as_deref() == Some(text.as_str())
                });
                if !already_carried {
                    carried.push(Message::user(text));
                    supplement_as_turn = true;
                }
            }
        }
        for round in 0..self.settings.agent_retry_max {
            let (run_id, attempt) = self
                .ledger()
                .begin(task, &cursor.cursor_id, cursor.stage, cursor.node, "main")
                .await?;
            // 续接链接只落 round 0（红线③ / 决策 180——落错轮会让 metrics 把同一段历史
            // 排除两次、token 少算；规则与理由见 `run_ledger` 模块 doc）。决策 278 的
            // 重试轮续接**不落链**：被续接的是上一 attempt 的会话行，链接语义不变。
            self.ledger()
                .link_continuation(run_id, round, continuation.as_ref())
                .await?;
            emit_node_started(&*self.sse, task, cursor, attempt, run_id);
            let started = self.clock.now();
            match self
                .agent_attempt(
                    task,
                    &project,
                    cursor,
                    kind,
                    run_id,
                    attempt,
                    &carried,
                    supplement_as_turn,
                )
                .await
            {
                Ok((output, tokens)) => {
                    // run 行与 NodeFinished 事件的 token 计量（决策 100）
                    finish_run_with_sse(
                        &*self.sse,
                        &self.ledger(),
                        run_id,
                        task,
                        cursor,
                        attempt,
                        false,
                        started,
                        None,
                        &tokens,
                    )
                    .await?;
                    self.store.refresh_task_totals(&task.id).await?;
                    return Ok(output);
                }
                Err(AttemptFailure {
                    error,
                    tokens,
                    messages,
                }) => {
                    // 被中止（红线② / 决策 226 / 276）：**谁判的终态谁说了算**——判超时那条路
                    // 已经写过 Timeout，人按停那条路不判终态，于是这里自己收成 `Cancelled`
                    // （行已是终态时这一笔退化成「只补用量」）。不收的话，一次被按停的 run 会
                    // 永远留在 `running` 上：台账里那句「还在跑」是假的，而 `check_timeouts`
                    // 日后还会把它判一次超时，把人的处置覆盖掉。
                    if error.is_cancelled() {
                        self.ledger()
                            .finish_cancelled(run_id, started, error.to_string(), &tokens)
                            .await?;
                        // 用量变了，任务投影就得跟着走（`total_tokens` 是从 run 行**重算**的）
                        self.store.refresh_task_totals(&task.id).await?;
                        return Err(error);
                    }
                    last_classified = error
                        .llm_classified()
                        .map(|(k, r)| (k.to_string(), r.to_string()));
                    last_error = error.to_string();
                    finish_run_with_sse(
                        &*self.sse,
                        &self.ledger(),
                        run_id,
                        task,
                        cursor,
                        attempt,
                        true,
                        started,
                        Some(last_error.clone()),
                        // 失败也照实记这一轮烧掉的 token（决策 226）：
                        // 记 0 会让「从未调用过模型」与「调用过但失败」在台账里长得一样。
                        &tokens,
                    )
                    .await?;
                    // run 行的 token 变了，任务投影就得跟着走：`total_tokens` 是**从 run 行重算**
                    // 出来的，不刷新它，库里那份读数会比 run 行汇总出来的小——决策 226 把失败轮的
                    // token 记真之后这个差第一次看得见（此前失败一律记 0，两者恰好相等）。
                    self.store.refresh_task_totals(&task.id).await?;
                    // 决策 278（显式修订决策 205 裁决②）：整体失败的自动重试不再空对话起步——
                    // 下一轮从「这一轮的转录原样保留 + 一条错误 turn」接着跑。错误 turn 由
                    // retry_prompt（决策 33 的「错误回填」）按原始诊断生成；转录不折叠、不省略
                    // （决策 278 明确不做有损处理），体量交给既有的 L3/L4 预算门。
                    let mut next = messages;
                    next.push(Message::user(crate::agent::metadata::retry_prompt(
                        &last_error,
                    )));
                    carried = next;
                }
            }
        }
        // 分类信息穿透重试耗尽包装（主流程票 03）：message 保持「哪个节点 + 可操作提示」，
        // 原始诊断仍由 run_inner 写进 pending.context.diagnostic。校验类的内层文案按
        // 决策 278 人话化——裸诊断串（「未找到结构化元数据」）对用户不是话；原始诊断
        // 仍完整落在 run 行的 error 里（finish_run_with_sse 收口时已写入）。
        match last_classified {
            Some((kind, raw)) => Err(Error::LlmClassified {
                kind,
                message: format!(
                    "agent 节点 {}.{} 重试耗尽：{last_error}",
                    cursor.stage, cursor.node
                ),
                raw,
            }),
            None => Err(Error::Validation(format!(
                "agent 节点 {}.{} 重试耗尽：{}",
                cursor.stage,
                cursor.node,
                humanize_agent_failure(&last_error)
            ))),
        }
    }

    /// 一次 agent 尝试的**外框**（票 01 / 决策 211①）：失败也要落会话，所以现场
    /// （`messages` / `tokens`）必须活到函数出口——`?` 会把它们一起带走，那正是
    /// 2026-09-17 实测里「失败的那一轮什么都不留」的机制。
    ///
    /// `persisted` 保证**一条 run 至多一条会话行**（决策 99）：成功路径已经写过时，
    /// 失败收尾只把错误上下文并进那一行，不再插新行。
    ///
    /// 内层 [`Self::agent_attempt_inner`] 的参数多于 clippy 的默认阈值：`run_id` /
    /// `attempt` / `carried` 三者都是**本次尝试**的入参，绑成结构体只是把同一份信息换个
    /// 地方写，不改变调用点的可读性。返回（结论，token 计量）；失败时计量跟着错误一起回来
    /// （见 [`AttemptFailure`]）。
    #[allow(clippy::too_many_arguments)]
    async fn agent_attempt(
        &self,
        task: &Task,
        project: &Project,
        cursor: &NodeCursor,
        kind: AgentNodeKind,
        run_id: i64,
        attempt: u32,
        carried: &[Message],
        supplement_as_turn: bool,
    ) -> std::result::Result<(NodeOutput, RunTokens), AttemptFailure> {
        let mut trace = AttemptTrace {
            messages: carried.to_vec(),
            ..Default::default()
        };
        match self
            .agent_attempt_inner(
                task,
                project,
                cursor,
                kind,
                run_id,
                attempt,
                &mut trace,
                supplement_as_turn,
            )
            .await
        {
            Ok((output, tokens)) => Ok((output, tokens)),
            Err(error) => {
                self.record_failed_attempt(task, cursor, run_id, attempt, &trace, &error)
                    .await;
                Err(AttemptFailure {
                    error,
                    tokens: trace.tokens,
                    // 决策 278：转录随失败一起回来——下一轮续接它（再追加错误 turn）
                    messages: trace.messages,
                })
            }
        }
    }

    /// 失败现场的落库。**落库失败不覆盖原错误**：调用方要带回去的是节点为什么失败，
    /// 不是记账为什么失败——后者只值一条 error 日志。
    async fn record_failed_attempt(
        &self,
        task: &Task,
        cursor: &NodeCursor,
        run_id: i64,
        attempt: u32,
        trace: &AttemptTrace,
        error: &Error,
    ) {
        let metadata = failure_metadata(error);
        let prompts = trace
            .prompts
            .as_ref()
            .map(|(system, user)| PromptSnapshot { system, user });
        let result = if trace.persisted {
            self.store
                .annotate_conversation_failure(&task.id, run_id, &metadata)
                .await
        } else {
            let msgs = match serde_json::to_value(&trace.messages) {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!(run_id, "失败会话不可序列化：{e}");
                    return;
                }
            };
            self.store
                .insert_conversation(
                    &task.id,
                    run_id,
                    cursor.stage,
                    cursor.node,
                    attempt,
                    "main",
                    None,
                    &msgs,
                    prompts,
                    Some(&metadata),
                    trace.tokens.prompt,
                    trace.tokens.completion,
                )
                .await
                .map(|_| ())
        };
        if let Err(e) = result {
            tracing::error!(run_id, "失败会话落库失败（原错误仍照原样上报）：{e}");
        }
    }

    /// 一次尝试的**内里**：与外框同签名，外加现场。它专管「跑」，
    /// 出口的记账（成功写一行、失败补上下文）归 [`Self::agent_attempt`]。
    #[allow(clippy::too_many_arguments)]
    async fn agent_attempt_inner(
        &self,
        task: &Task,
        project: &Project,
        cursor: &NodeCursor,
        kind: AgentNodeKind,
        run_id: i64,
        attempt: u32,
        trace: &mut AttemptTrace,
        supplement_as_turn: bool,
    ) -> Result<(NodeOutput, RunTokens)> {
        let home = self.store.home().clone();
        home.ensure_task_dirs(&task.id)?;
        let worktree = task
            .worktree_path
            .clone()
            .unwrap_or_else(|| home.worktree_path(&task.id).display().to_string());
        let task_dir = home.task_dir(&task.id).display().to_string();

        let policy = crate::agent::file_policy::pipeline_file_policy(
            Path::new(&worktree),
            Path::new(&task_dir),
            self.settings.file_access_unrestricted,
        );
        // 阶段配置消费（§10.6.3 / 决策 22 / 46 / 111）：persona、采样参数、工具与技能增量。
        // **在构造执行器之前读**：环境层档位要喂给执行点那道闸（决策 206），而它来自
        // 这一份配置——构造完再读就得回头改执行器的状态。
        let stage_cfg = self.store.get_stage_config(cursor.stage.as_str()).await?;
        let env_mode = crate::types::effective_env_mode(
            self.settings.env_mode,
            cursor.stage.as_str(),
            stage_cfg.as_ref(),
        );
        let tools = ToolExecutor::new(
            home.clone(),
            policy,
            self.settings.clone(),
            self.killer.clone(),
        )
        .with_recorder(Arc::new(self.store.clone()))
        // 命令输出按行推流（票 14 / 决策 100）：长命令期间前端能看到增量输出。
        .with_sse(crate::agent::tools::CommandSse {
            sink: self.sse.clone(),
            task_id: task.id.clone(),
            branch: cursor.branch.clone(),
        })
        // 第三道闸（决策 206）：环境层按档位分三路。**不给它接提议通道**——流水线节点的
        // `ask` 档下写工具会被拒，那是刻意的：提议是**人的**确认钮的载体，而流水线节点
        // 背后没有人盯着，落一条没人会按的提议等于静默丢弃（决策 206 的档位是给
        // 「有值班经理看着」的值班长用的，流水线阶段要么 auto 要么 deny）。
        .with_env_mode(env_mode);
        if env_mode == crate::types::EnvMode::Ask {
            tracing::warn!(
                stage = cursor.stage.as_str(),
                "阶段被配成 ask 档：环境层工具会因没有提议通道而被拒（值班长的确认钮不服务流水线节点）"
            );
        }
        let declared_tools =
            json_string_list(stage_cfg.as_ref().and_then(|c| c.tools_json.as_ref()));

        // 子代理（决策 172③，票 08）：**只有阶段显式声明才注入运行器**。不声明时
        // `spawn_sub_agent` 调用会拿到一句「未启用」的说明文本（工具层没有运行器），
        // 这就是「扩展工具、默认关闭」的落点。节点级超时作为该次调用的上限（票 08）。
        let sub_agent: Option<Arc<dyn crate::agent::SubAgentRunner>> = declared_tools
            .iter()
            .any(|t| t == crate::agent::SPAWN_SUB_AGENT_TOOL)
            .then(|| {
                let node_override =
                    crate::config::node_timeouts(stage_cfg.as_ref(), cursor.node.as_str());
                let max_duration = crate::config::effective_max_duration(
                    self.settings.node_max_duration_sec,
                    stage_cfg.as_ref().and_then(|c| c.max_duration_sec),
                    node_override,
                );
                Arc::new(crate::pipeline::subagent::StoreSubAgentRunner::new(
                    crate::pipeline::subagent::SubAgentRunnerConfig {
                        store: self.store.clone(),
                        settings: self.settings.clone(),
                        llm: self.llm.clone(),
                        killer: self.killer.clone(),
                        home: home.clone(),
                        task_id: task.id.clone(),
                        cursor_id: cursor.cursor_id.clone(),
                        stage: cursor.stage,
                        node: cursor.node,
                        attempt,
                        branch: cursor.branch.clone(),
                        parent_run_id: run_id,
                        worktree_path: worktree.clone().into(),
                        task_dir: task_dir.clone().into(),
                        project_root: PathBuf::from(&project.local_path),
                        language: project.language.clone(),
                        test_framework: project.test_framework.clone(),
                        temperature: stage_cfg.as_ref().and_then(|c| c.temperature),
                        max_tokens: stage_cfg.as_ref().and_then(|c| c.max_tokens),
                        env_mode,
                        max_duration: std::time::Duration::from_secs(max_duration),
                    },
                )) as Arc<dyn crate::agent::SubAgentRunner>
            });
        let tools = match sub_agent {
            Some(runner) => tools.with_sub_agent(runner),
            None => tools,
        };
        // 模型请求组装（决策 249 · 票 01）：prompt 两段 / 工具定义 / 上下文容量按 attempt
        // 一次拼齐。组装期判出的超限（静态两段已超硬限，压缩无从下手）**带着 plan 回来**
        // ——先落原文再收口是形状保证（决策 180 退出路径条件）；越界的翻译（会话行 +
        // pending）不在门里，在进轮循环处统一做（决策 245「门吃落点不吃原因」）。
        let prepared = RequestPlan::assemble(AttemptCtx {
            store: &self.store,
            settings: &self.settings,
            task,
            project,
            cursor,
            stage_cfg: stage_cfg.as_ref(),
            attempt,
            kind,
            // 决策 279：补充输入已作为 user turn 进转录（validate_input 续接），
            // segment 不再重渲染——见 `model_request::load_segments`。
            user_input_as_turn: supplement_as_turn,
        })
        .await?;
        let (plan, mut assemble_overflow) = match prepared {
            Prepared::Ready(plan) => (plan, None),
            Prepared::Overflow { plan, facts } => (plan, Some(facts)),
        };
        // 原文落现场（票 02）：hash 与原文同时写——hash 是索引，原文是权威（决策 211②）。
        self.store.set_run_template_hash(run_id, &plan.hash).await?;
        let snapshot = plan.snapshot();
        trace.prompts = Some((snapshot.system.to_owned(), snapshot.user.to_owned()));

        // 压缩锚点的边界（决策 180，票 13 必要条件三）：`carried` 是**上一轮**的对话，
        // 它里面的 user 消息不得充当「本轮第一条 user 消息」这个锚点——否则载入历史后，
        // keep 预算会被上一轮的提问占掉。
        let carried_len = trace.messages.len();
        let mut tool_failures = 0u32;
        let mut submitted: Option<serde_json::Value> = None;

        loop {
            // 中止请求（决策 226 / 276）：每一轮开头先看一眼。被叫醒的那一轮由下面模型调用处
            // 的 `select!` 打断；这里拦的是另外两种情形——信号在两轮之间到达、以及已经请求过
            // 中止却又进了一轮（重试循环会走到这里）。
            let cancel = cancel_signal(&task.id);
            if let Some(signal) = &cancel {
                if signal.is_requested() {
                    return Err(Error::Cancelled(format!(
                        "{}.{} 的本次执行已按{}中止",
                        cursor.stage,
                        cursor.node,
                        signal.origin().as_str()
                    )));
                }
            }
            // 预算门（票 01 检测、本侧翻译——决策 245「门吃落点不吃原因」、决策 154）：
            // 组装期超限优先（还没进过轮、压缩无从谈起），否则每轮同步查一次。
            // 任一超限 → 先补会话行再收口为 pending，**永不继续循环**（票 13 必要条件一）。
            let overflow = assemble_overflow.take().or_else(|| {
                match plan.check_budget(&mut trace.messages, carried_len) {
                    BudgetCheck::Ok { .. } => None,
                    BudgetCheck::Overflow {
                        estimate,
                        hard_limit,
                    } => Some(OverflowFacts {
                        estimate,
                        hard_limit,
                    }),
                }
            });
            if let Some(facts) = overflow {
                return self
                    .context_overflow_exit(task, cursor, run_id, attempt, trace, facts)
                    .await;
            }

            let req = plan.request(
                &trace.messages,
                Some(crate::agent::client::RunContext {
                    task_id: task.id.clone(),
                    session_id: String::new(),
                    branch: cursor.branch.clone(),
                    run_id,
                    agent_type: PIPELINE_AGENT_TYPE.into(),
                }),
            );
            // 模型调用是**最容易无限期停住**的地方：一个不返回的请求两侧都没有心跳，
            // 于是它既正是调度器判超时的对象，也是执行体身上唯一能观察中止请求的 await 点
            // （决策 226）——判超时那边不需要「有进程组可杀」，从这里就能把执行体叫停。
            // 取不到观察点（进程内没有这一号登记）时照旧直连，行为与加这条通道之前一致。
            let response = match &cancel {
                Some(signal) => tokio::select! {
                    r = self.llm.complete(req) => r?,
                    _ = signal.wait() => {
                        return Err(Error::Cancelled(format!(
                            "{}.{} 的模型调用已按{}中止",
                            cursor.stage,
                            cursor.node,
                            signal.origin().as_str()
                        )));
                    }
                },
                None => self.llm.complete(req).await?,
            };
            trace.tokens.add(&response);
            trace.messages.push(Message::assistant(
                response.content.clone(),
                response.tool_calls.clone(),
            ));
            self.store.touch_run_heartbeat(run_id).await?;

            if response.tool_calls.is_empty() {
                break;
            }
            for call in &response.tool_calls {
                let summary = args_summary(&call.arguments);
                emit_tool_event(
                    &*self.sse,
                    task,
                    cursor,
                    run_id,
                    &call.name,
                    ToolPhase::Start,
                    &summary,
                );
                let ctx = ToolCallContext {
                    task_id: task.id.clone(),
                    session_id: None,
                    stage: cursor.stage,
                    node: cursor.node,
                    worktree_path: worktree.clone().into(),
                    task_dir: task_dir.clone().into(),
                    run_id: Some(run_id),
                    command_source: CommandSource::Agent,
                    default_cwd: Some(worktree.clone().into()),
                };
                match tools.execute(call, &ctx).await {
                    Ok(outcome) => {
                        if let Some(m) = outcome.metadata {
                            submitted = Some(m);
                        }
                        trace
                            .messages
                            .push(Message::tool_result(call, outcome.content));
                        emit_tool_event(
                            &*self.sse,
                            task,
                            cursor,
                            run_id,
                            &call.name,
                            ToolPhase::End,
                            &summary,
                        );
                    }
                    Err(e) => {
                        // error 阶段的 args_summary 仍是参数摘要（决策 123）；
                        // 错误详情走 messages 的 tool_result（已脱敏）
                        emit_tool_event(
                            &*self.sse,
                            task,
                            cursor,
                            run_id,
                            &call.name,
                            ToolPhase::Error,
                            &summary,
                        );
                        // G13：工具失败在 agent loop 内重试，只计 tool_retry_max 次
                        tool_failures += 1;
                        trace
                            .messages
                            .push(Message::tool_result(call, format!("工具执行失败：{e}")));
                        if tool_failures > self.settings.tool_retry_max {
                            return Err(Error::Validation(format!(
                                "工具失败超过 tool_retry_max：{e}"
                            )));
                        }
                    }
                }
            }
        }

        // 元数据抽取（决策 33：解析/校验失败计入节点重试）
        let value = match submitted {
            Some(v) => v,
            None => {
                let final_resp = crate::agent::client::AgentResponse {
                    content: trace.messages.iter().rev().find_map(|m| m.content.clone()),
                    ..Default::default()
                };
                let extracted = crate::agent::metadata::extract_metadata(&final_resp);
                extracted.value.ok_or_else(|| {
                    Error::Validation(extracted.error.unwrap_or_else(|| "缺少结构化元数据".into()))
                })?
            }
        };
        kind.validate(&value)?;

        // decision 134 / 135：agent 型 validate_output 首判不合格 → 同步调用异族复判。
        // 复判合格（与首判分歧）→ 节点内直接 pending(user_decision, judge_disagreement)，
        // **不进入路由**（路由只看到两侧一致不合格）。
        let mut judge_disagreement: Option<PendingReason> = None;
        if kind == AgentNodeKind::DesignValidateOutput && self.settings.cross_family_judge {
            let first: crate::types::ValidateOutputMetadata =
                serde_json::from_value(value.clone())?;
            if !first.passed {
                let prompt = format!(
                    "请复核上游 validate_output 对 {} 阶段产出「不合格」的判定。\n\
                     阶段：{}\n节点：{}\n首判元数据：{}\n\
                     若你认为产出实际合格，请 submit_metadata passed=true；否则 passed=false。",
                    cursor.stage, cursor.stage, cursor.node, value
                );
                let (cross_value, _cross_tokens) = self
                    .call_pseudo_stage(
                        task,
                        cursor,
                        run_id,
                        attempt,
                        PseudoStage::ValidatorCrossCheck,
                        prompt,
                    )
                    .await?;
                let cross: CrossCheckResult = parse_metadata(&cross_value)?;
                if matches!(
                    crate::pipeline::resolve_validate_output(true, false, Some(cross.passed)),
                    crate::pipeline::ValidateOutcome::JudgeDisagreement
                ) {
                    let detail = if cross.blockers.is_empty() {
                        String::new()
                    } else {
                        format!("（复判备注：{}）", cross.blockers.join("；"))
                    };
                    judge_disagreement = Some(
                        PendingReason::new(
                            PendingKind::UserDecision,
                            cursor.stage,
                            cursor.node,
                            format!("异族复判与首判分歧：首判不合格、复判合格，请用户终审{detail}"),
                        )
                        .with_context(PendingContext::with_kind(
                            crate::actions::kinds::JUDGE_DISAGREEMENT,
                        )),
                    );
                }
            }
        }

        // 会话落库（§12.4.3；1:1 对调 LLM 的 run，决策 99）
        let msgs = serde_json::to_value(&trace.messages)?;
        self.store
            .insert_conversation(
                &task.id,
                run_id,
                cursor.stage,
                cursor.node,
                attempt,
                "main",
                None,
                &msgs,
                trace
                    .prompts
                    .as_ref()
                    .map(|(system, user)| PromptSnapshot { system, user }),
                Some(&value),
                trace.tokens.prompt,
                trace.tokens.completion,
            )
            .await?;
        // 这一条 run 的会话行已经写过：后面若在 `post_process` 上失败，外框只把错误
        // 上下文并进这一行（票 01），不会再插一行——1:1 的口径不因失败路径而破。
        trace.persisted = true;
        self.store.refresh_task_totals(&task.id).await?;

        // 分歧路径在节点内直接置 pending，不经 post_process / 路由（决策 135）
        if let Some(reason) = judge_disagreement {
            return Ok((NodeOutput::Pending(reason), trace.tokens));
        }

        let output = kind
            .post_process(self, task, cursor, run_id, attempt, value)
            .await?;
        Ok((output, trace.tokens))
    }

    // ─────────────────────── 伪阶段（决策 48 / 60 / 67 / 88 / 100 / 113 / 134）───────────────────────

    /// 同步调用一个伪阶段（不占游标）：落独立 run + 会话行，心跳归父 run。
    ///
    /// `run.agent_type = pseudo:*`（FakeAgent 据此路由脚本）；`cursor_id` 继承父游标
    /// （决策 113）；`agent_type` 非 `system` → 计入 `total_calls`（决策 130 ②）。
    async fn call_pseudo_stage(
        &self,
        task: &Task,
        cursor: &NodeCursor,
        parent_run_id: i64,
        attempt: u32,
        pseudo: PseudoStage,
        user_prompt: String,
    ) -> Result<(serde_json::Value, RunTokens)> {
        let project = project_or_err(&self.store, &task.project_id).await?;
        let home = self.store.home().clone();
        let worktree = task
            .worktree_path
            .clone()
            .unwrap_or_else(|| home.worktree_path(&task.id).display().to_string());
        let task_dir = home.task_dir(&task.id).display().to_string();
        let stage_cfg = self.store.get_stage_config(pseudo.stage_key()).await?;

        // persona：persona_path 优先，否则内嵌（决策 7 / 87）；persona_append 追加
        let mut persona = match stage_cfg.as_ref().and_then(|c| c.persona_path.as_deref()) {
            Some(path) => {
                let p = home.root().join(path);
                std::fs::read_to_string(&p).map_err(|e| {
                    Error::Config(format!(
                        "伪阶段 {} 的 persona_path 不可读：{}（{e}）",
                        pseudo.stage_key(),
                        p.display()
                    ))
                })?
            }
            None => pseudo.embedded_persona().to_string(),
        };
        if let Some(append) = stage_cfg.as_ref().and_then(|c| c.persona_append.as_deref()) {
            if !append.trim().is_empty() {
                persona.push_str(&format!("\n\n{append}"));
            }
        }
        let system_prompt = build_system_prompt(
            &load_agents_context(
                Path::new(&project.local_path),
                project.language.as_deref(),
                project.test_framework.as_deref(),
            ),
            &persona,
            &workdirs_line(&worktree, &task_dir),
            &[],
        );

        let run_id = self
            .store
            .insert_run(&NewRun {
                task_id: task.id.clone(),
                cursor_id: cursor.cursor_id.clone(),
                stage: cursor.stage,
                node: cursor.node,
                attempt,
                agent_type: pseudo.agent_type().to_string(),
                parent_run_id: Some(parent_run_id),
                prompt_template_hash: None,
                process_group_id: None,
            })
            .await?;

        let provider_id = crate::storage::catalog::resolve_provider_id(
            None,
            task.model_override.as_deref(),
            stage_cfg.as_ref(),
            None,
        );
        let request = LlmRequest {
            stage: cursor.stage,
            node: cursor.node,
            attempt,
            system_prompt: system_prompt.clone(),
            user_prompt: user_prompt.clone(),
            messages: Vec::new(),
            tools: vec![pseudo.submit_tool()],
            temperature: stage_cfg.as_ref().and_then(|c| c.temperature),
            max_tokens: stage_cfg.as_ref().and_then(|c| c.max_tokens),
            provider_id,
            run: Some(crate::agent::client::RunContext {
                task_id: task.id.clone(),
                branch: cursor.branch.clone(),
                run_id,
                agent_type: pseudo.agent_type().to_string(),
                session_id: String::new(),
            }),
            idle_timeout_sec: None,
        };

        let started = self.clock.now();
        let response = match self.llm.complete(request).await {
            Ok(r) => r,
            Err(e) => {
                self.store
                    .finish_run(
                        run_id,
                        &RunOutcome {
                            status: Some(NodeStatus::Failed),
                            duration_ms: since_ms(self.clock.now(), started),
                            error: Some(e.to_string()),
                            ..Default::default()
                        },
                    )
                    .await?;
                return Err(e);
            }
        };
        // 心跳写父 run（决策 88：伪阶段不得让父节点被空闲超时误杀）
        let _ = self.store.touch_run_heartbeat(parent_run_id).await;
        let mut tokens = RunTokens::default();
        tokens.add(&response);
        self.store
            .finish_run(
                run_id,
                &RunOutcome {
                    status: Some(NodeStatus::Success),
                    duration_ms: since_ms(self.clock.now(), started),
                    prompt_tokens: tokens.prompt,
                    completion_tokens: tokens.completion,
                    cache_read_tokens: tokens.cache_read,
                    cache_write_tokens: tokens.cache_write,
                    ..Default::default()
                },
            )
            .await?;

        let extracted = crate::agent::metadata::extract_metadata(&response);
        let value = extracted.value.ok_or_else(|| {
            Error::Validation(
                extracted
                    .error
                    .unwrap_or_else(|| "伪阶段缺少结构化元数据".into()),
            )
        })?;
        // 伪阶段独立会话行（决策 100）
        let msgs = serde_json::to_value(vec![Message::assistant(
            response.content.clone(),
            response.tool_calls.clone(),
        )])?;
        self.store
            .insert_conversation(
                &task.id,
                run_id,
                cursor.stage,
                cursor.node,
                attempt,
                pseudo.agent_type(),
                Some(parent_run_id),
                &msgs,
                Some(PromptSnapshot {
                    system: &system_prompt,
                    user: &user_prompt,
                }),
                Some(&value),
                tokens.prompt,
                tokens.completion,
            )
            .await?;
        self.store.refresh_task_totals(&task.id).await?;
        Ok((value, tokens))
    }

    /// project_analysis 伪阶段（decision 48 / 78 / 130）：确定性探测事实由调用方给出，
    /// 伪阶段只负责写人读摘要并标注可疑项，**合并**进分析结果。
    ///
    /// 项目级调用没有 task / 游标，故不落 run 行（v1 的 app 接线由票 17/20 完成）。
    pub async fn project_analysis(
        &self,
        project: &Project,
        mut facts: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let stage_cfg = self
            .store
            .get_stage_config(PseudoStage::ProjectAnalysis.stage_key())
            .await?;
        let persona = match stage_cfg.as_ref().and_then(|c| c.persona_path.as_deref()) {
            Some(path) => std::fs::read_to_string(self.store.home().root().join(path))
                .map_err(|e| Error::Config(format!("project_analysis persona_path 不可读：{e}")))?,
            None => PseudoStage::ProjectAnalysis.embedded_persona().to_string(),
        };
        let system_prompt = build_system_prompt(
            &load_agents_context(
                Path::new(&project.local_path),
                project.language.as_deref(),
                project.test_framework.as_deref(),
            ),
            &persona,
            &workdirs_line(&project.local_path, &project.local_path),
            &[],
        );
        let user_prompt = format!(
            "以下是确定性探测得到的事实清单（JSON）：\n{facts}\n\n\
             请写一段人读摘要（summary）并列出可疑项（suspicious），用 submit_metadata 返回。"
        );
        let provider_id =
            crate::storage::catalog::resolve_provider_id(None, None, stage_cfg.as_ref(), None);
        let request = LlmRequest {
            stage: Stage::Init,
            node: Node::Execute,
            attempt: 1,
            system_prompt,
            user_prompt,
            messages: Vec::new(),
            tools: vec![PseudoStage::ProjectAnalysis.submit_tool()],
            temperature: stage_cfg.as_ref().and_then(|c| c.temperature),
            max_tokens: stage_cfg.as_ref().and_then(|c| c.max_tokens),
            provider_id,
            run: Some(crate::agent::client::RunContext {
                task_id: String::new(),
                branch: String::new(),
                run_id: 0,
                agent_type: PseudoStage::ProjectAnalysis.agent_type().to_string(),
                session_id: String::new(),
            }),
            idle_timeout_sec: None,
        };
        let response = self.llm.complete(request).await?;
        let extracted = crate::agent::metadata::extract_metadata(&response);
        let value = extracted.value.ok_or_else(|| {
            Error::Validation(
                extracted
                    .error
                    .unwrap_or_else(|| "project_analysis 缺少结构化元数据".into()),
            )
        })?;
        let result: crate::pipeline::pseudo::ProjectAnalysisResult = parse_metadata(&value)?;
        if let Some(obj) = facts.as_object_mut() {
            obj.insert("summary".into(), serde_json::Value::String(result.summary));
            obj.insert(
                "suspicious".into(),
                serde_json::to_value(result.suspicious)?,
            );
        }
        Ok(facts)
    }

    /// 语义第二层冲突检测（决策 60 / 67）：模块路径重叠但符号名无交集时，
    /// 同步调 `conflict_check`；`duplicate_risk = high` → pending(user_decision, duplicate_risk)。
    async fn semantic_conflict_check(
        &self,
        task: &Task,
        cursor: &NodeCursor,
        run_id: i64,
        attempt: u32,
    ) -> Result<Option<PendingReason>> {
        let mine = self.store.overlap_keys(&task.id).await?;
        let mut candidates: Vec<(String, String, Vec<String>, Vec<String>)> = Vec::new();
        for other in self
            .store
            .list_tasks(&crate::storage::tasks::TaskFilter {
                include_archived: false,
                ..Default::default()
            })
            .await?
        {
            if other.id == task.id || !other.status.is_active() {
                continue;
            }
            let theirs = self.store.overlap_keys(&other.id).await?;
            let module_overlap = mine
                .symbols
                .iter()
                .any(|(m, _)| theirs.symbols.iter().any(|(tm, _)| module_overlaps(m, tm)));
            let symbol_overlap = mine.symbols.iter().any(|s| theirs.symbols.contains(s));
            let file_overlap = mine.files.iter().any(|f| theirs.files.contains(f));
            // 模块路径重叠、符号名无交集、文件也无交集 → 需要语义层判断
            if module_overlap && !symbol_overlap && !file_overlap {
                candidates.push((
                    other.id.clone(),
                    other.title.clone(),
                    theirs.files.clone(),
                    theirs
                        .symbols
                        .iter()
                        .map(|(m, n)| format!("{m}::{n}"))
                        .collect(),
                ));
            }
        }
        if candidates.is_empty() {
            return Ok(None);
        }
        let ids: Vec<String> = candidates.iter().map(|c| c.0.clone()).collect();
        let mut prompt = format!(
            "本任务「{}」的架构设计新增符号所在模块与以下活跃任务重叠，但符号名无交集。\n\
             请判断是否存在语义重复（同一功能被两个任务各自实现，duplicate_risk = high）。\n\n\
             本任务新增符号：\n",
            task.title
        );
        for (m, n) in &mine.symbols {
            prompt.push_str(&format!("- {m}::{n}\n"));
        }
        prompt.push_str("\n候选任务：\n");
        for (id, title, files, symbols) in &candidates {
            prompt.push_str(&format!(
                "- 「{title}」（{id}）：文件 [{}]；符号 [{}]\n",
                files.join("、"),
                symbols.join("、")
            ));
        }
        prompt
            .push_str("\n请 submit_metadata 返回 duplicate_risk（low / medium / high）与 reason。");
        let (value, _tokens) = self
            .call_pseudo_stage(
                task,
                cursor,
                run_id,
                attempt,
                PseudoStage::ConflictCheck,
                prompt,
            )
            .await?;
        let result: ConflictCheckResult = parse_metadata(&value)?;
        if result.duplicate_risk != crate::types::DuplicateRisk::High {
            return Ok(None);
        }
        let detail = result
            .reason
            .as_deref()
            .map(|r| format!("：{r}"))
            .unwrap_or_default();
        Ok(Some(
            PendingReason::new(
                PendingKind::UserDecision,
                Stage::ArchitectDesign,
                Node::Execute,
                format!(
                    "语义重复风险（模块路径重叠、符号名无交集）{detail}；冲突任务：{}",
                    ids.join("、")
                ),
            )
            .with_context(PendingContext {
                kind: Some(crate::actions::kinds::DUPLICATE_RISK.to_string()),
                conflict_task_ids: ids,
                ..Default::default()
            }),
        ))
    }

    /// 预算越界的**翻译**（决策 245「门吃落点不吃原因」——门只报读数，构造 `Pending`
    /// 是编排的职责；决策 154 L4）。两条越界路（组装期静态超限、每轮压缩后超限）都汇到
    /// 这里：先补写会话行，再收口为 `pending(context_overflow)`。
    ///
    /// 票 13 的必要条件一（决策 180）：这条退出路径在会话落库**之前**返回，于是「开了续接
    /// 却读不到上一轮」会是一条静默无效的路——会话行正是续接最需要的那个失败现场。
    /// **永不继续循环**：压缩救不回来的节点继续跑只会无限循环（L4 的「绝不放行」）。
    async fn context_overflow_exit(
        &self,
        task: &Task,
        cursor: &NodeCursor,
        run_id: i64,
        attempt: u32,
        trace: &mut AttemptTrace,
        facts: OverflowFacts,
    ) -> Result<(NodeOutput, RunTokens)> {
        tracing::warn!(
            task = %task.id,
            stage = %cursor.stage,
            node = %cursor.node,
            estimate = facts.estimate,
            hard_limit = facts.hard_limit,
            "压缩后仍超硬限，挂 pending(context_overflow)（§12.13 L4）"
        );
        let msgs = serde_json::to_value(&trace.messages)?;
        self.store
            .insert_conversation(
                &task.id,
                run_id,
                cursor.stage,
                cursor.node,
                attempt,
                "main",
                None,
                &msgs,
                trace
                    .prompts
                    .as_ref()
                    .map(|(system, user)| PromptSnapshot { system, user }),
                None,
                trace.tokens.prompt,
                trace.tokens.completion,
            )
            .await?;
        // 这一条 run 的会话行已经写过（外框的失败收尾只补上下文，不再插行）
        trace.persisted = true;
        Ok((
            NodeOutput::Pending(PendingReason::new(
                PendingKind::ContextOverflow,
                cursor.stage,
                cursor.node,
                "上下文压缩后仍超过硬限，请拆分任务 / 换长上下文模型 / 取消",
            )),
            trace.tokens,
        ))
    }
}

/// agent 节点种类：元数据类型与后处理按此分发（决策 38）。
/// `pub(crate)`：组装侧（`model_request::tool_defs`）按它分发 `submit_metadata` 的 schema。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentNodeKind {
    ValidateInput,
    ArchitectExecute,
    DevelopDesignExecute,
    TestDesignExecute,
    DesignValidateOutput,
    DevelopExecute,
    ReviewExecute,
    TestExecute,
}

impl AgentNodeKind {
    fn of(stage: Stage, node: Node) -> Option<Self> {
        use AgentNodeKind::*;
        Some(match (stage, node) {
            (
                Stage::ArchitectDesign | Stage::DevelopDesign | Stage::TestDesign,
                Node::ValidateInput,
            ) => ValidateInput,
            (Stage::ArchitectDesign, Node::Execute) => ArchitectExecute,
            (Stage::DevelopDesign, Node::Execute) => DevelopDesignExecute,
            (Stage::TestDesign, Node::Execute) => TestDesignExecute,
            (
                Stage::ArchitectDesign | Stage::DevelopDesign | Stage::TestDesign,
                Node::ValidateOutput,
            ) => DesignValidateOutput,
            (Stage::Develop, Node::Execute) => DevelopExecute,
            (Stage::Review, Node::Execute) => ReviewExecute,
            (Stage::Test, Node::Execute) => TestExecute,
            _ => return None,
        })
    }

    /// 类型化校验（决策 33：元数据解析/校验失败 → 节点重试）。
    fn validate(&self, value: &serde_json::Value) -> Result<()> {
        macro_rules! check {
            ($t:ty) => {
                parse_metadata::<$t>(value).map(|_| ())
            };
        }
        match self {
            AgentNodeKind::ValidateInput => check!(crate::types::ValidateInputMetadata),
            AgentNodeKind::ArchitectExecute => check!(crate::types::ArchitectExecuteMetadata),
            AgentNodeKind::DevelopDesignExecute => check!(crate::types::DevelopDesignMetadata),
            AgentNodeKind::TestDesignExecute => check!(crate::types::TestDesignMetadata),
            AgentNodeKind::DesignValidateOutput => check!(crate::types::ValidateOutputMetadata),
            AgentNodeKind::DevelopExecute => check!(crate::types::CodeChanges),
            AgentNodeKind::ReviewExecute => check!(crate::types::ReviewResult),
            AgentNodeKind::TestExecute => check!(crate::types::TestResult),
        }
    }

    /// 节点后处理：execute 类节点落阶段产出 + 返回路由视图。
    async fn post_process(
        &self,
        inv: &ModelInvoke,
        task: &Task,
        cursor: &NodeCursor,
        run_id: i64,
        attempt: u32,
        value: serde_json::Value,
    ) -> Result<NodeOutput> {
        use crate::pipeline::MetadataView;
        let view = match self {
            AgentNodeKind::ValidateInput => {
                let m: crate::types::ValidateInputMetadata = serde_json::from_value(value)?;
                // 决策 277④：blockers 随投影带走——info_insufficient 的 pending 消息
                // 要把「要问什么」带给用户，而不是让他们去翻会话记录。
                MetadataView::readiness_with_blockers(m.readiness, m.blockers)
            }
            AgentNodeKind::DesignValidateOutput => {
                let m: crate::types::ValidateOutputMetadata = serde_json::from_value(value)?;
                MetadataView::passed(m.passed)
            }
            AgentNodeKind::ArchitectExecute => {
                let m: crate::types::ArchitectExecuteMetadata =
                    serde_json::from_value(value.clone())?;
                let path = m
                    .design_doc_path
                    .clone()
                    .unwrap_or_else(|| "design.md".into());
                inv.store
                    .upsert_stage_output(
                        &task.id,
                        Stage::ArchitectDesign,
                        OUTPUT_DESIGN_DOC,
                        &path,
                        Some(&value),
                    )
                    .await?;
                // 两层冲突检测的第一层（决策 53 / 60 / 71 / 102）
                let conflicts = inv.store.first_layer_conflicts(&task.id).await?;
                // 只有文件/符号真交集（High）才 conflict_wait；纯 name 重合是 Low，
                // 只告警不阻塞（决策 71② / 120）。
                let hard: Vec<_> = conflicts
                    .iter()
                    .filter(|w| w.duplicate_risk == Some(DuplicateRisk::High))
                    .collect();
                for warning in conflicts
                    .iter()
                    .filter(|w| w.duplicate_risk == Some(DuplicateRisk::Low))
                {
                    tracing::warn!(
                        task = %task.id,
                        other = %warning.task_id,
                        symbols = ?warning.overlapping_symbols,
                        "纯符号名重合：仅告警，不触发 conflict_wait（决策 71② / 120）"
                    );
                }
                if !hard.is_empty() {
                    let ids: Vec<String> = hard.iter().map(|w| w.task_id.clone()).collect();
                    let reason = PendingReason::new(
                        PendingKind::ConflictWait,
                        Stage::ArchitectDesign,
                        Node::Execute,
                        format!("与活跃任务存在文件/符号冲突：{}", ids.join("、")),
                    )
                    .with_context(PendingContext {
                        conflict_task_ids: ids,
                        ..Default::default()
                    });
                    return Ok(NodeOutput::Pending(reason));
                }
                // 第二层：模块路径重叠但符号名无交集 → conflict_check 语义比对（决策 60 / 67）。
                // 命中 high → pending(user_decision, duplicate_risk)（决策 60 / 132）。
                if inv.settings.semantic_conflict_check {
                    if let Some(reason) = inv
                        .semantic_conflict_check(task, cursor, run_id, attempt)
                        .await?
                    {
                        return Ok(NodeOutput::Pending(reason));
                    }
                }
                MetadataView::default()
            }
            AgentNodeKind::DevelopDesignExecute => {
                let m: crate::types::DevelopDesignMetadata = serde_json::from_value(value.clone())?;
                let path = m
                    .dev_doc_path
                    .clone()
                    .unwrap_or_else(|| "dev-plan.md".into());
                inv.store
                    .upsert_stage_output(
                        &task.id,
                        Stage::DevelopDesign,
                        OUTPUT_DEV_DOC,
                        &path,
                        Some(&value),
                    )
                    .await?;
                MetadataView::default()
            }
            AgentNodeKind::TestDesignExecute => {
                let m: crate::types::TestDesignMetadata = serde_json::from_value(value.clone())?;
                let path = m
                    .test_scenarios_path
                    .clone()
                    .unwrap_or_else(|| "test-scenarios.md".into());
                inv.store
                    .upsert_stage_output(
                        &task.id,
                        Stage::TestDesign,
                        OUTPUT_TEST_SCENARIOS,
                        &path,
                        Some(&value),
                    )
                    .await?;
                MetadataView::default()
            }
            AgentNodeKind::DevelopExecute => {
                let m: crate::types::CodeChanges = serde_json::from_value(value.clone())?;
                inv.store
                    .upsert_stage_output(
                        &task.id,
                        Stage::Develop,
                        OUTPUT_CODE_CHANGES,
                        "code-changes.json",
                        Some(&value),
                    )
                    .await?;
                let _ = m;
                MetadataView::default()
            }
            AgentNodeKind::ReviewExecute => {
                let m: crate::types::ReviewResult = serde_json::from_value(value.clone())?;
                let path = m
                    .review_report_path
                    .clone()
                    .unwrap_or_else(|| "review-report.md".into());
                inv.store
                    .upsert_stage_output(
                        &task.id,
                        Stage::Review,
                        OUTPUT_REVIEW_REPORT,
                        &path,
                        Some(&value),
                    )
                    .await?;
                // review 的判定在 validate_output（纯代码）做，execute 只产出
                MetadataView::default()
            }
            AgentNodeKind::TestExecute => {
                let mut m: crate::types::TestResult = serde_json::from_value(value.clone())?;
                let path = m
                    .test_report_path
                    .clone()
                    .unwrap_or_else(|| "test-report.md".into());
                // decision 109：被 merge 测试闸门打回后的复检，系统置 `gate_recheck = true`
                if let Some(merge) = inv.store.merge_metadata(&task.id).await? {
                    if merge.gate == Some(Gate::Fail)
                        && matches!(merge.gate_failure_kind, Some(GateFailureKind::Test) | None)
                    {
                        m.gate_recheck = true;
                    }
                }
                let persisted = serde_json::to_value(&m)?;
                inv.store
                    .upsert_stage_output(
                        &task.id,
                        Stage::Test,
                        OUTPUT_TEST_REPORT,
                        &path,
                        Some(&persisted),
                    )
                    .await?;
                // test 的判定在 validate_output（纯代码）做
                MetadataView::default()
            }
        };
        Ok(NodeOutput::Route(view))
    }
}

/// 模块路径是否重叠（决策 60 第二层：模块路径重叠但符号名无交集）。
fn module_overlaps(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a == b || a.starts_with(&format!("{b}::")) || b.starts_with(&format!("{a}::"))
}

#[test]
fn module_overlap_detection() {
    assert!(module_overlaps("auth", "auth"));
    assert!(module_overlaps("crate::auth", "crate::auth::login"));
    assert!(module_overlaps("crate::auth::login", "crate::auth"));
    assert!(!module_overlaps("auth", "billing"));
    // 前缀相同但不是模块边界（auth vs authorize）不算重叠
    assert!(!module_overlaps("auth", "authorize"));
    assert!(!module_overlaps("", "auth"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::client::AgentResponse;
    use crate::clock::SystemClock;
    use crate::home::Home;
    use crate::sse::SseEvent;
    use crate::storage::tasks::NewTask;

    /// 桩 LLM：返回一段可被 `extract_metadata` 取走的平衡 JSON（伪阶段共用一条成功路径）。
    struct StubLlm;
    impl LlmClient for StubLlm {
        fn complete(
            &self,
            _request: LlmRequest,
        ) -> futures::future::BoxFuture<'static, Result<AgentResponse>> {
            Box::pin(async {
                Ok(AgentResponse {
                    content: Some(r#"{"duplicate_risk": "low", "reason": "桩"}"#.into()),
                    ..Default::default()
                })
            })
        }
    }

    struct NoopKiller;
    impl ProcessKiller for NoopKiller {
        fn kill_process_group(&self, _pgid: i32) -> Result<()> {
            Ok(())
        }
    }

    struct NoopSse;
    impl SseSink for NoopSse {
        fn emit(&self, _event: SseEvent) {}
    }

    /// 临时库 + 播种的项目/任务/游标 + 只拿票面点名依赖的编排片（**不建 Executor**）。
    async fn base() -> (tempfile::TempDir, Store, Task, NodeCursor, ModelInvoke) {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = Home::new(tmp.path().join("home"));
        let store = Store::open(home, Arc::new(SystemClock)).await.unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let project = Project {
            id: "p1".into(),
            name: "proj".into(),
            local_path: repo.display().to_string(),
            default_branch: "main".into(),
            language: None,
            test_framework: None,
            lint_command: None,
            agents_md_path: None,
            created_at: chrono::Utc::now(),
        };
        store.create_project(&project).await.unwrap();
        let task = store
            .create_task(&NewTask::new("t1", "任务 t1", "p1"))
            .await
            .unwrap();
        let cursor = store.resolve_sole_cursor("t1").await.unwrap().unwrap();
        let inv = ModelInvoke {
            store: store.clone(),
            settings: Settings::default(),
            llm: Arc::new(StubLlm),
            killer: Arc::new(NoopKiller),
            sse: Arc::new(NoopSse),
            clock: Arc::new(SystemClock),
        };
        (tmp, store, task, cursor, inv)
    }

    /// post_process 的臂在依赖显式化后**逐臂直测**（票 03 验收）：不建 Executor、
    /// 不跑节点循环——`&Executor` 参数归零的直接兑现。
    #[tokio::test]
    async fn post_process_persists_stage_output_without_an_executor() {
        let (_tmp, store, task, cursor, inv) = base().await;
        let out = AgentNodeKind::DevelopDesignExecute
            .post_process(
                &inv,
                &task,
                &cursor,
                1,
                1,
                serde_json::json!({"readiness": true, "dev_doc_path": "plans/dev.md"}),
            )
            .await
            .unwrap();
        assert!(matches!(out, NodeOutput::Route(_)));
        let row = store
            .get_stage_output(&task.id, Stage::DevelopDesign, OUTPUT_DEV_DOC)
            .await
            .unwrap()
            .expect("develop-design 的产出行必须已落库");
        assert_eq!(row.file_path, "plans/dev.md");
    }

    #[tokio::test]
    async fn post_process_validate_output_maps_passed_to_the_route_view() {
        let (_tmp, _store, task, cursor, inv) = base().await;
        let out = AgentNodeKind::DesignValidateOutput
            .post_process(
                &inv,
                &task,
                &cursor,
                1,
                1,
                serde_json::json!({"passed": true}),
            )
            .await
            .unwrap();
        match out {
            NodeOutput::Route(view) => assert!(view.passed, "passed=true 透传进路由视图"),
            other => {
                let _ = other;
                panic!("期望 Route");
            }
        }
        let out = AgentNodeKind::DesignValidateOutput
            .post_process(
                &inv,
                &task,
                &cursor,
                1,
                2,
                serde_json::json!({"passed": false}),
            )
            .await
            .unwrap();
        match out {
            NodeOutput::Route(view) => assert!(!view.passed),
            other => {
                let _ = other;
                panic!("期望 Route");
            }
        }
    }

    /// 伪阶段的 run 行形状**不跑全循环**就可测（票 03 验收）：恰一条、挂 parent、
    /// attempt 与 agent_type 照伪阶段写——同构「一次 LLM 调用 + 一条 run 行」的见证。
    #[tokio::test]
    async fn pseudo_stage_writes_one_run_row_without_the_full_loop() {
        let (_tmp, store, task, cursor, inv) = base().await;
        let (parent, _) = inv
            .ledger()
            .begin(&task, &cursor.cursor_id, cursor.stage, cursor.node, "main")
            .await
            .unwrap();
        let (value, _tokens) = inv
            .call_pseudo_stage(
                &task,
                &cursor,
                parent,
                3,
                PseudoStage::ConflictCheck,
                "判断语义重复".into(),
            )
            .await
            .unwrap();
        assert_eq!(value["duplicate_risk"], "low", "桩返回的元数据原样到手");

        let runs = store.list_runs(&task.id).await.unwrap();
        let pseudo: Vec<_> = runs
            .iter()
            .filter(|r| r.agent_type == "pseudo:conflict_check")
            .collect();
        assert_eq!(pseudo.len(), 1, "伪阶段恰一条 run 行");
        let run = pseudo[0];
        assert_eq!(run.parent_run_id, Some(parent), "挂父 run");
        assert_eq!(run.attempt, 3);
        assert_eq!(run.status, NodeStatus::Success);
        assert_eq!(run.stage, cursor.stage);
    }

    /// 决策 279：user turn 取的是用户说过的话——剥掉决策 79 落盘时的标题行。
    #[test]
    fn supplement_input_strips_the_header_and_ignores_empty_bodies() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = crate::home::Home::new(tmp.path());

        // 文件不存在 → None
        assert_eq!(supplement_input(&home, "t1"), None);

        // 决策 79 的落盘形态：标题行 + 正文
        std::fs::create_dir_all(home.task_dir("t1")).unwrap();
        std::fs::write(
            home.task_file("t1", "user-input.md"),
            "# 用户补充输入\n\n部署在 k8s，单机即可\n",
        )
        .unwrap();
        assert_eq!(
            supplement_input(&home, "t1").as_deref(),
            Some("部署在 k8s，单机即可")
        );

        // 只有标题（空输入不落正文）→ None
        std::fs::write(home.task_file("t1", "user-input.md"), "# 用户补充输入\n\n").unwrap();
        assert_eq!(supplement_input(&home, "t1"), None);
    }
}
