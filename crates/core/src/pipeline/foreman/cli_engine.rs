//! CLI 引擎（决策 423 / 424，票 talk-cli-engine 02）：对讲台会话的**整段执行器**。
//!
//! 形状 A 的落地件：会话整体交给本机 claude code 无头模式（`-p --output-format
//! stream-json`），它自带工具循环做读写分析；本模块只负责**组装命令、解析事件流、
//! 攒出这一轮的产出**——落库与提议纪律在 [`super::runner`]。
//!
//! 权限面（决策 424①）：写类工具显式禁（`--disallowedTools`），Bash 只放**只读白名单**
//! （`--allowedTools`，与决策 232/395 的白名单哲学同形）；无头模式下其余要许可的工具
//! 一律被自动拒绝——值班长手里没有写刀，改状态只能走提议通道（票 05 接 MCP 后）。
//!
//! 事件流解析（决策 423⑤）：**宽容**——未知行跳过不炸流；认得出的只有四类：
//! `system/init`（领 session id）、`assistant`（文本 / 思考 / 工具调用）、
//! `user`（工具结果回执）、`result`（收口与权威 usage）。解析失败 fail-fast 的
//! 读数在票 06 收口，这里未知行先落台账（跳过计数进 [`CliTurnOutput::skipped_lines`]）。

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use super::{push_foreman_live_delta, LiveTurn, FOREMAN_AGENT_TYPE};
use super::{ForemanSegment, ForemanTrace, FOREMAN_TOOL_RESULT_MAX_CHARS};
use crate::agent::tools::CommandFinish;
use crate::agent::tools::CommandSse;
use crate::exec::{CommandOutput, CommandOwner, CommandRequest, CommandRunner, Rewrite, SpawnForm};
use crate::sse::{Channel, SseEvent, SseSink, ToolPhase};
use crate::storage::Store;
use crate::types::{CommandSource, Node, Stage};
use crate::Result;

/// 一期的 CLI 引擎二进制（决策 423⑥：一期只做 claude code；经 PATH 解析）。
pub const CLI_FOREMAN_BINARY: &str = "claude";

/// 写类工具的显式禁单（决策 424①）：无头模式下这些工具没有许可人，本来就会被拒——
/// 显式再禁一遍是**形状**（读代码的人不用去查无头模式的缺省语义才知道值班长没有写刀）。
pub const CLI_WRITE_DENIED: &str = "Edit,Write,MultiEdit,NotebookEdit,EnterPlanMode,ExitPlanMode";

/// Bash 的**只读白名单**（决策 424①「允许跑只读命令」的落地）：
/// git 的读取子命令 + 常见只读文本工具。与 claude 的 allowlist 语法同形
/// （`Bash(前缀:*)` = 前缀匹配）。命中白名单之外的命令照样被许可层拒绝。
pub const CLI_READONLY_BASH: &str = "Bash(git status),Bash(git log:*),Bash(git diff:*),\
Bash(git show:*),Bash(git branch),Bash(git branch:*),Bash(ls:*),Bash(cat:*),Bash(head:*),\
Bash(tail:*),Bash(wc:*),Bash(rg:*),Bash(grep:*),Bash(find:*),Bash(fd:*)";

/// 重建时带入的台账行数（决策 424③）：摘要取自会话落库，取最近的这些行
/// （`summarize_input` 自己再做字符截断——离窗口最近的轮次最要紧）。
pub const CLI_REBUILD_DIGEST_MESSAGES: usize = 40;

/// 重建告知的标记（决策 424③「不静默」）：system 行与重建输入各带一次。
pub const CLI_REBUILT_MARK: &str = "【引擎会话重建】";

/// 一次 CLI 回合的超时上限。它是**整段回合**的墙钟（claude 自带工具循环可能合法地
/// 跑几分钟），与流水线逐调用的空闲界（决策 288）不同层——空闲判死由 CLI 自己的
/// 进程活性兜底，这里只防「挂死不归」。
pub const CLI_TURN_TIMEOUT_SEC: u64 = 900;

/// 一次 CLI 回合的产出（票 02：全量攒完；票 03 在此之上接实时桥）。
#[derive(Debug, Default, Clone)]
pub struct CliTurnOutput {
    pub reply: String,
    /// 推理原文（决策 244）：`result` 事件之外的 thinking 块拼段。
    pub thinking: String,
    /// 按发生顺序的工具调用痕迹（收口文本不在其中——那是 `reply`）。
    pub traces: Vec<ForemanTrace>,
    /// 按发生顺序的步骤序列（决策 273）：先想 → 再查 → 然后说。
    pub segments: Vec<ForemanSegment>,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    /// CLI 侧会话 id（`system/init` 事件）：本轮结束由 runner 回填会话行。
    pub cli_session_id: Option<String>,
    /// 进程退出码；`None` = 没跑起来（与 exec 收口同一口径）。
    pub exit_code: Option<i32>,
    /// 失败读数（退出码非零 / `result.is_error` / 没跑起来）：**重建判定**看它。
    pub error_text: Option<String>,
    /// 宽容跳过的未知行数（决策 423⑤：不炸流，但留个读数）。
    pub skipped_lines: u32,
}

/// 一次 CLI 回合的输入。`message` 已含态势前缀与值守标记（组装在 runner）。
pub struct CliTurnRequest {
    pub message: String,
    pub system_prompt: String,
    /// `--resume` 的续接锚；`None` = 开新 CLI 会话。
    pub resume_session_id: Option<String>,
    pub timeout_sec: u64,
    /// 台账归属（决策 204④）：值班长的命令挂会话——挂了记录器（流式路径）就
    /// 必须恰好一个归属，探针那种无归属形态只在无记录器时合法。
    pub session_id: String,
}

/// 跑一轮：组装 argv → 经 exec 收口 spawn（决策 297）→ 逐行解析 stream-json。
///
/// 程序名经 PATH 解析（Argv 形态走父进程 PATH——服务进程的 PATH 怎么配就怎么用）；
/// 测试注入用 [`Self::run_turn_with_program`]。
pub async fn run_cli_turn(req: CliTurnRequest) -> Result<CliTurnOutput> {
    run_cli_turn_with_program(CLI_FOREMAN_BINARY, req).await
}

/// 测试缝：程序路径由调用点给（假脚本）。
///
/// 为什么不在 PATH 前置上做缝：`with_path_prefix` 只改**子进程** env 的 `PATH`，
/// 而 Argv 形态的程序名解析走**父进程** PATH——前置目录影子不到名字解析，
/// 测试会跑到本机真 `claude`（票 01 的探针吃过这个亏，同一教训不再吃一遍）。
pub async fn run_cli_turn_with_program(
    program: &str,
    req: CliTurnRequest,
) -> Result<CliTurnOutput> {
    let runner = CommandRunner::new(std::sync::Arc::new(crate::process::RealProcessKiller));
    run_cli_turn_with_runner(&runner, program, req).await
}

/// 完整测试缝：runner 也可注入（路径前置 / 记录器由测试决定）。
pub async fn run_cli_turn_with_runner(
    runner: &CommandRunner,
    program: &str,
    req: CliTurnRequest,
) -> Result<CliTurnOutput> {
    let mut args: Vec<String> = vec![
        "-p".into(),
        req.message.clone(),
        "--output-format".into(),
        "stream-json".into(),
        // 逐字（票 talk-cli-engine 03）：partial 才有 text_delta / thinking_delta，
        // 没有它一次 assistant 消息只在收齐后整段出现——三声道就成了三声闷响。
        "--include-partial-messages".into(),
        "--append-system-prompt".into(),
        req.system_prompt.clone(),
        "--disallowedTools".into(),
        CLI_WRITE_DENIED.into(),
        "--allowedTools".into(),
        CLI_READONLY_BASH.into(),
    ];
    if let Some(id) = &req.resume_session_id {
        args.push("--resume".into());
        args.push(id.clone());
    }
    // 台账记录形态：消息太长，只留头一屏（全文在消息行的 content 列里，不重记）。
    let head: String = req.message.chars().take(80).collect();
    let command = format!(
        "{program} -p {head}… --output-format stream-json{}",
        if req.resume_session_id.is_some() {
            " --resume <cli-session>"
        } else {
            ""
        }
    );
    let owner = CommandOwner {
        task_id: None,
        session_id: (!req.session_id.is_empty()).then_some(req.session_id.clone()),
        run_id: None,
        stage: Stage::Init,
        node: Node::Execute,
        source: CommandSource::System,
    };
    let request = CommandRequest {
        owner,
        command: &command,
        cwd: Path::new("."),
        timeout_sec: req.timeout_sec,
        spawn: SpawnForm::Argv {
            program,
            args: &args,
        },
        rewrite: Rewrite::None,
    };
    let (_, out) = runner
        .run(request, |out: &CommandOutput| {
            Ok((
                CommandFinish {
                    exit_code: out.exit_code,
                    stdout_path: None,
                    stdout_preview: None,
                    stderr_preview: None,
                    duration_ms: out.duration_ms,
                },
                out.clone(),
            ))
        })
        .await?;
    Ok(parse_stream_json(&out))
}

/// 实时桥（票 talk-cli-engine 03）：exec 收口的**逐行推流**翻译成对讲台事件。
///
/// exec 的流式机构只会发 `SseEvent::CommandOutput`（推的是命令输出行）——值班长
/// 要的是 `ConversationDelta` / `ToolEvent`。桥坐在两者之间：收到 CommandOutput
/// 就把那一行当 stream-json 事件翻译，**原始行不外发**（stream-json 的行不是给人
/// 看的命令输出）；其余事件原样转发。
///
/// 翻译口径：
/// - `stream_event/content_block_delta` 的 `text_delta` / `thinking_delta` →
///   `ConversationDelta`（content / reasoning 声道，决策 244），随广播推进在途现场
///   （`push_foreman_live_delta`，位置戳随事件走）；
/// - `assistant` 消息里的 `tool_use` → `ToolEvent::Start`（此刻完整 input 已知，
///   且是 CLI 真去执行之前——与 API 路径的发射位同时态）；
/// - `user` 消息里的 `tool_result` → `ToolEvent::End` / `Error`（带结果文本）。
pub struct CliBridgeSink {
    inner: Arc<dyn SseSink>,
    session_id: String,
    live: Arc<LiveTurn>,
    tools: std::sync::Mutex<BTreeMap<String, (String, String)>>,
}

impl CliBridgeSink {
    pub fn new(inner: Arc<dyn SseSink>, session_id: &str, live: Arc<LiveTurn>) -> Self {
        CliBridgeSink {
            inner,
            session_id: session_id.to_string(),
            live,
            tools: std::sync::Mutex::new(BTreeMap::new()),
        }
    }

    fn emit_delta(&self, channel: Channel, text: &str) {
        if text.is_empty() {
            return;
        }
        let stamp = push_foreman_live_delta(&self.session_id, channel, text);
        self.inner.emit(SseEvent::ConversationDelta {
            task_id: String::new(),
            branch: String::new(),
            run_id: 0,
            agent_type: FOREMAN_AGENT_TYPE.to_string(),
            session_id: self.session_id.clone(),
            channel,
            role: "assistant".into(),
            text: text.to_string(),
            prompt_tokens: 0,
            completion_tokens: 0,
            ledger_id: stamp.map(|(id, _)| id),
            seq: stamp.map(|(_, seq)| seq),
        });
    }

    fn emit_tool(
        &self,
        tool: &str,
        phase: ToolPhase,
        args_summary: &str,
        args: &str,
        result: Option<&str>,
    ) {
        let stamp = self.live.reserve_event_seq();
        self.inner.emit(SseEvent::ToolEvent {
            task_id: String::new(),
            branch: String::new(),
            run_id: 0,
            agent_type: FOREMAN_AGENT_TYPE.to_string(),
            session_id: self.session_id.clone(),
            tool: tool.to_string(),
            phase,
            args_summary: args_summary.to_string(),
            args: args.to_string(),
            result: result.map(str::to_string),
            ledger_id: Some(stamp.0),
            seq: Some(stamp.1),
        });
    }

    /// 一行 stream-json 事件的翻译。同步、无锁外 I/O：它挂在逐行推流上。
    pub(crate) fn translate(&self, line: &str) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            return;
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("stream_event") => {
                let Some(ev) = v.get("event") else { return };
                if ev.get("type").and_then(|t| t.as_str()) != Some("content_block_delta") {
                    return;
                }
                let Some(delta) = ev.get("delta") else { return };
                match delta.get("type").and_then(|t| t.as_str()) {
                    Some("text_delta") => {
                        let text = delta.get("text").and_then(|t| t.as_str()).unwrap_or("");
                        self.emit_delta(Channel::Content, text);
                    }
                    Some("thinking_delta") => {
                        let text = delta.get("thinking").and_then(|t| t.as_str()).unwrap_or("");
                        self.emit_delta(Channel::Reasoning, text);
                    }
                    _ => {}
                }
            }
            Some("assistant") => {
                let Some(content) = v
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                else {
                    return;
                };
                for block in content {
                    if block.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
                        continue;
                    }
                    let id = block
                        .get("id")
                        .and_then(|i| i.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let name = block
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let input = block.get("input").cloned().unwrap_or_default();
                    let args_full = truncate_chars(
                        &serde_json::to_string(&input).unwrap_or_default(),
                        FOREMAN_TOOL_RESULT_MAX_CHARS,
                    );
                    let args_summary = truncate_chars(&compact_json(&input), 200);
                    self.tools
                        .lock()
                        .unwrap()
                        .insert(id, (name.clone(), args_full.clone()));
                    self.emit_tool(&name, ToolPhase::Start, &args_summary, &args_full, None);
                }
            }
            Some("user") => {
                let Some(content) = v
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                else {
                    return;
                };
                for block in content {
                    let Some(id) = block.get("tool_use_id").and_then(|i| i.as_str()) else {
                        continue;
                    };
                    let Some((name, args)) = self.tools.lock().unwrap().remove(id) else {
                        continue;
                    };
                    let is_err = block
                        .get("is_error")
                        .and_then(|e| e.as_bool())
                        .unwrap_or(false);
                    let result = block
                        .get("content")
                        .map(result_content_text)
                        .map(|r| truncate_chars(&r, FOREMAN_TOOL_RESULT_MAX_CHARS));
                    self.emit_tool(
                        &name,
                        if is_err {
                            ToolPhase::Error
                        } else {
                            ToolPhase::End
                        },
                        &truncate_chars(&args, 200),
                        &args,
                        result.as_deref(),
                    );
                }
            }
            _ => {}
        }
    }
}

impl SseSink for CliBridgeSink {
    fn emit(&self, event: SseEvent) {
        if let SseEvent::CommandOutput { chunk, .. } = &event {
            self.translate(chunk);
            return;
        }
        self.inner.emit(event);
    }
}

/// 流式入口（票 talk-cli-engine 03）：exec 收口挂上记录器（领 command_id，逐行推流
/// 才会发射）与桥 sink，其余与 [`run_cli_turn_with_runner`] 同一条。
pub async fn run_cli_turn_streaming(
    program: &str,
    req: CliTurnRequest,
    store: &Store,
    bridge: CliBridgeSink,
) -> Result<CliTurnOutput> {
    let runner = CommandRunner::new(Arc::new(crate::process::RealProcessKiller))
        .with_recorder(Arc::new(store.clone()))
        .with_sse(CommandSse {
            sink: Arc::new(bridge),
            task_id: String::new(),
            branch: String::new(),
        });
    run_cli_turn_with_runner(&runner, program, req).await
}

/// stream-json 逐行解析 → 一轮产出（纯函数，宽容口径见模块头）。
fn parse_stream_json(out: &CommandOutput) -> CliTurnOutput {
    let mut o = CliTurnOutput {
        exit_code: out.exit_code,
        ..Default::default()
    };
    let mut text_parts: Vec<String> = Vec::new();
    let mut reply_authoritative: Option<String> = None;
    // 工具结果按 tool_use_id 对号入座：result 回执来的时候要找到它那次调用。
    let mut tool_ids: BTreeMap<String, usize> = BTreeMap::new();
    let mut stderr_head = String::new();

    for line in out.stdout.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            o.skipped_lines += 1;
            continue;
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("system") => {
                if let Some(id) = v.get("session_id").and_then(|s| s.as_str()) {
                    o.cli_session_id = Some(id.to_string());
                }
            }
            Some("assistant") => {
                if let Some(usage) = v
                    .get("message")
                    .and_then(|m| m.get("usage"))
                    .and_then(parse_usage)
                {
                    o.prompt_tokens += usage.0;
                    o.completion_tokens += usage.1;
                }
                let Some(content) = v
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                else {
                    continue;
                };
                for block in content {
                    match block.get("type").and_then(|t| t.as_str()) {
                        Some("text") => {
                            let Some(text) = block.get("text").and_then(|t| t.as_str()) else {
                                continue;
                            };
                            text_parts.push(text.to_string());
                            o.segments.push(ForemanSegment::Text {
                                text: text.to_string(),
                            });
                        }
                        Some("thinking") => {
                            let Some(text) = block.get("thinking").and_then(|t| t.as_str()) else {
                                continue;
                            };
                            o.thinking.push_str(text);
                            o.segments.push(ForemanSegment::Thinking {
                                text: text.to_string(),
                            });
                        }
                        Some("tool_use") => {
                            let name = block
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("unknown")
                                .to_string();
                            let input = block.get("input").cloned().unwrap_or_default();
                            let args_full = truncate_chars(
                                &serde_json::to_string(&input).unwrap_or_default(),
                                FOREMAN_TOOL_RESULT_MAX_CHARS,
                            );
                            let args_summary = truncate_chars(&compact_json(&input), 200);
                            tool_ids.insert(
                                block
                                    .get("id")
                                    .and_then(|i| i.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                                o.traces.len(),
                            );
                            o.traces.push(ForemanTrace {
                                tool: name,
                                args_summary,
                                args: args_full,
                                result: String::new(),
                                ok: true,
                            });
                            o.segments.push(ForemanSegment::Tool {
                                tool: o.traces.last().unwrap().tool.clone(),
                                args_summary: o.traces.last().unwrap().args_summary.clone(),
                                args: o.traces.last().unwrap().args.clone(),
                                result: String::new(),
                                ok: true,
                            });
                        }
                        _ => o.skipped_lines += 1,
                    }
                }
            }
            Some("user") => {
                // 工具结果回执：按 tool_use_id 找回那次调用，把结果文本填进 trace 与段序。
                let Some(content) = v
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                else {
                    continue;
                };
                for block in content {
                    let Some(id) = block.get("tool_use_id").and_then(|i| i.as_str()) else {
                        continue;
                    };
                    let result_text = block
                        .get("content")
                        .map(result_content_text)
                        .unwrap_or_default();
                    let is_err = block
                        .get("is_error")
                        .and_then(|e| e.as_bool())
                        .unwrap_or(false);
                    if let Some(&idx) = tool_ids.get(id) {
                        let trace = &mut o.traces[idx];
                        trace.result = truncate_chars(&result_text, FOREMAN_TOOL_RESULT_MAX_CHARS);
                        trace.ok = !is_err;
                        if let Some(ForemanSegment::Tool { result, ok, .. }) = o.segments.iter_mut().rev().find(|s| matches!(s, ForemanSegment::Tool { result, .. } if result.is_empty())) {
                            *result = trace.result.clone();
                            *ok = trace.ok;
                        }
                    }
                }
            }
            Some("result") => {
                if let Some(usage) = v.get("usage").and_then(parse_usage) {
                    // result 事件的 usage 是整轮权威值，覆盖累计。
                    o.prompt_tokens = usage.0;
                    o.completion_tokens = usage.1;
                }
                // 收口正文：result 事件的 `result` 是权威，覆盖 assistant 文本段拼接。
                if let Some(t) = v.get("result").and_then(|r| r.as_str()) {
                    if !t.trim().is_empty() {
                        reply_authoritative = Some(t.to_string());
                    }
                }
                if v.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false) {
                    o.error_text = Some(
                        v.get("error")
                            .and_then(|r| r.as_str())
                            .unwrap_or("result 事件标记为 error")
                            .to_string(),
                    );
                }
            }
            _ => o.skipped_lines += 1,
        }
    }
    if !out.stderr.trim().is_empty() {
        stderr_head = truncate_chars(out.stderr.trim(), 500);
    }
    if o.exit_code.is_none() && o.error_text.is_none() {
        o.error_text = Some(if out.timed_out {
            format!("CLI 回合超时（{}s）", CLI_TURN_TIMEOUT_SEC)
        } else {
            format!(
                "CLI 进程没有跑起来{}",
                if stderr_head.is_empty() {
                    String::new()
                } else {
                    format!("：{stderr_head}")
                }
            )
        });
    } else if o.exit_code.is_some_and(|c| c != 0) && o.error_text.is_none() {
        o.error_text = Some(format!(
            "CLI 退出码 {}{}",
            o.exit_code.unwrap(),
            if stderr_head.is_empty() {
                String::new()
            } else {
                format!("：{stderr_head}")
            }
        ));
    }
    // 回话正文：result 事件的 `result` 是收口权威；没有它（异常收口）用攒的文本段兜底。
    o.reply = reply_authoritative.unwrap_or_else(|| text_parts.join(""));
    o
}

/// 工具结果的 content 形态：字符串直接用；块数组拼 text 块。
fn result_content_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join(""),
        other => other.to_string(),
    }
}

/// usage 的两种键形（token 计量口径随 CLI 版本漂过）：input/output 与
/// prompt/completion 都认，缺的按 0。
fn parse_usage(v: &serde_json::Value) -> Option<(u32, u32)> {
    let in_tok = v
        .get("input_tokens")
        .or_else(|| v.get("prompt_tokens"))
        .and_then(|n| n.as_u64());
    let out_tok = v
        .get("output_tokens")
        .or_else(|| v.get("completion_tokens"))
        .and_then(|n| n.as_u64());
    match (in_tok, out_tok) {
        (Some(i), Some(o)) => Some((i as u32, o as u32)),
        _ => None,
    }
}

fn compact_json(v: &serde_json::Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

/// 组装 CLI 值班长的追加系统提示词（人格 + 只读纪律）。
///
/// 不复用 API 值班长的完整 system prompt：那份广告的是流水线内置工具清单（决策 247），
/// CLI 引擎下工具面是 CLI 自己的——广告一份不存在的清单等于教模型说谎。
pub(crate) fn cli_system_prompt(persona: Option<&str>) -> String {
    let mut p = String::from(
        "你是 AgentPipeline 对讲台的值班长，运行在本机 claude code 引擎上（决策：CLI 整段执行）。\n\
         纪律：\n\
         1. 只读。你可以读文件、跑只读命令（git 读取与常见文本工具在白名单里）；\
         改文件、写文件、改任何状态都被禁止——写类工具已被禁用。\n\
         2. 要动手的活用话说出来：给值班经理讲清楚该开什么任务、改什么、为什么，\
         由人和流水线去执行，不要试图绕过禁令。\n\
         3. 回话用中文，简明、有据；引用任务时带任务 id。\n\
         4. 【值守摘要】等带【】标记的输入是系统注入的说明，不是值班经理说的话。\n",
    );
    if let Some(persona) = persona {
        p.push_str("\n## 人格\n\n");
        p.push_str(persona);
        p.push('\n');
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(stdout: &str, exit: Option<i32>) -> CommandOutput {
        CommandOutput {
            command_id: None,
            exit_code: exit,
            stdout: stdout.into(),
            stderr: String::new(),
            timed_out: false,
            duration_ms: 5,
        }
    }

    #[test]
    fn init_gives_session_id_and_assistant_text_accumulates() {
        let out = parse_stream_json(&output(
            concat!(
                r#"{"type":"system","subtype":"init","session_id":"sess-1"}"#,
                "\n",
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"你好"}],"usage":{"input_tokens":10,"output_tokens":5}}}"#,
                "\n",
                r#"{"type":"result","subtype":"success","is_error":false,"result":"你好，值班经理。","usage":{"input_tokens":12,"output_tokens":7}}"#,
                "\n"
            ),
            Some(0),
        ));
        assert_eq!(out.cli_session_id.as_deref(), Some("sess-1"));
        assert_eq!(out.reply, "你好，值班经理。", "result 权威正文覆盖文本段");
        assert_eq!(
            (out.prompt_tokens, out.completion_tokens),
            (12, 7),
            "result usage 覆盖累计"
        );
    }

    #[test]
    fn tool_use_and_result_pair_up() {
        let out = parse_stream_json(&output(
            concat!(
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"a.rs"}}]}}"#,
                "\n",
                r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":false,"content":"fn main() {}"}]}}"#,
                "\n"
            ),
            Some(0),
        ));
        assert_eq!(out.traces.len(), 1);
        assert_eq!(out.traces[0].tool, "Read");
        assert_eq!(out.traces[0].result, "fn main() {}");
        assert!(out.traces[0].ok);
    }

    #[test]
    fn unknown_lines_are_skipped_with_a_count() {
        let out = parse_stream_json(&output(
            concat!(
                r#"{"type":"something-new","payload":1}"#,
                "\n",
                "not-json-at-all",
                "\n",
                r#"{"type":"result","subtype":"success","is_error":false,"result":"ok"}"#,
                "\n"
            ),
            Some(0),
        ));
        assert_eq!(out.skipped_lines, 2, "未知行跳过并计数");
        assert_eq!(out.reply, "ok");
    }

    #[test]
    fn nonzero_exit_and_missing_binary_carry_error_text() {
        let o = parse_stream_json(&output("", Some(1)));
        assert!(o.error_text.is_some(), "非零退出有读数");
        let o = parse_stream_json(&output("", None));
        assert!(o.error_text.is_some(), "没跑起来有读数");
    }

    #[test]
    fn usage_keys_tolerate_both_shapes() {
        assert_eq!(
            parse_usage(&serde_json::json!({"input_tokens":1,"output_tokens":2})),
            Some((1, 2))
        );
        assert_eq!(
            parse_usage(&serde_json::json!({"prompt_tokens":3,"completion_tokens":4})),
            Some((3, 4))
        );
        assert_eq!(parse_usage(&serde_json::json!({})), None);
    }

    #[test]
    fn system_prompt_carries_persona_and_readonly_discipline() {
        let p = cli_system_prompt(Some("永远先说结论。"));
        assert!(p.contains("只读"));
        assert!(p.contains("永远先说结论。"));
        let p = cli_system_prompt(None);
        assert!(p.contains("只读"));
        assert!(!p.contains("人格"), "无人格时不出现空段落");
    }

    #[tokio::test]
    async fn full_turn_against_a_fake_script() {
        let home = testkit::TestHome::new().unwrap();
        let dir = home.home().root().join("bin");
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("fake-claude");
        std::fs::write(
            &script,
            concat!(
                "#!/bin/sh\n",
                "echo '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-9\"}'\n",
                "echo '{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"收到\"}]}}'\n",
                "echo '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"收到，值班经理。\",\"usage\":{\"input_tokens\":5,\"output_tokens\":3}}'\n",
            ),
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let out = run_cli_turn_with_program(
            script.to_str().unwrap(),
            CliTurnRequest {
                message: "在吗".into(),
                system_prompt: "测试".into(),
                resume_session_id: None,
                timeout_sec: 30,
                session_id: String::new(),
            },
        )
        .await
        .unwrap();
        assert_eq!(out.cli_session_id.as_deref(), Some("sess-9"));
        assert_eq!(out.reply, "收到，值班经理。");
    }
}
