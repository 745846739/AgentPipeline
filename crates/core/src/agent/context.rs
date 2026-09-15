//! 上下文管理与四级压缩（§12.13，决策 110）。
//!
//! L0 容量预估 → L1 工具结果裁剪（常开、零成本）→ L2 大结果卸载（唯一阈值
//! `offload_threshold_tokens`，默认 4000）→ L3 对话压缩（规则化优先）→ L4 兜底。
//!
//! 核心前提：**文件系统是 source of truth**——任何被裁剪/丢弃的文件内容都能低成本重读，
//! 因此压缩是安全的。

use super::client::{Message, Role};
use crate::config::Settings;
use crate::types::{Node, Stage};

/// 模型输出预留（L0 容量预估用）。
pub const OUTPUT_RESERVE: usize = 4096;

/// L1：`read_file` 默认返回头部行数。
pub const READ_FILE_HEAD_LINES: usize = 200;
/// L1：`run_command` 保留前 N 行。
pub const RUN_COMMAND_HEAD_LINES: usize = 50;
/// L1：`run_command` 保留后 N 行。
pub const RUN_COMMAND_TAIL_LINES: usize = 100;
/// L1：`list_dir` 最多列出的条目数。
pub const LIST_DIR_MAX_ITEMS: usize = 200;

/// 粗略 token 估算（4 字符 ≈ 1 token）。仅用于分层阈值判定，不用于计费。
pub fn count_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

// ─────────────────────────────── L0 容量预估 ───────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextCapacity {
    pub total: usize,
    pub reserved_system: usize,
    pub reserved_user: usize,
    pub soft_limit: usize,
    pub hard_limit: usize,
    pub available_for_tools: usize,
}

/// L0：调用前容量预估（§12.13.3）。
pub fn estimate_context_capacity(
    model_window: usize,
    system_prompt: &str,
    user_prompt: &str,
    settings: &Settings,
) -> ContextCapacity {
    let reserved_system = count_tokens(system_prompt);
    let reserved_user = count_tokens(user_prompt);
    ContextCapacity {
        total: model_window,
        reserved_system,
        reserved_user,
        soft_limit: (model_window as f64 * settings.context_soft_limit_ratio) as usize,
        hard_limit: (model_window as f64 * settings.context_hard_limit_ratio) as usize,
        available_for_tools: model_window
            .saturating_sub(reserved_system + reserved_user + OUTPUT_RESERVE),
    }
}

/// 是否触发 L3 压缩（超过 soft limit）。
pub fn should_compact(current_tokens: usize, capacity: ContextCapacity) -> bool {
    current_tokens > capacity.soft_limit
}

/// 是否触发 L4 兜底（超过 hard limit）。
pub fn over_hard_limit(current_tokens: usize, capacity: ContextCapacity) -> bool {
    current_tokens > capacity.hard_limit
}

// ─────────────────────────────── L1 裁剪 ───────────────────────────────

/// L1：`read_file` 裁剪——默认头部 200 行 + 结构大纲 + 分段读取提示。
pub fn trim_read_file(content: &str, limit_lines: Option<usize>) -> String {
    let max = limit_lines.unwrap_or(READ_FILE_HEAD_LINES);
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() <= max {
        return content.to_string();
    }
    let head = lines[..max].join("\n");
    let mut outline = Vec::new();
    for (i, line) in lines.iter().enumerate().skip(max) {
        if is_definition_line(line) && outline.len() < 200 {
            outline.push(format!("L{}: {}", i + 1, line.trim()));
        }
    }
    let mut out = format!(
        "{head}\n\n... 已省略 {} 行（文件共 {} 行）...",
        lines.len() - max,
        lines.len()
    );
    if !outline.is_empty() {
        out.push_str("\n[结构大纲]\n");
        out.push_str(&outline.join("\n"));
    }
    out.push_str("\n如需完整内容，请用 read_file 的 offset / limit 分段读取。");
    out
}

/// L1：`run_command` 裁剪——前 50 + 后 100 行；中间普通行省略、重复行折叠，
/// **错误行始终保留**。
pub fn trim_run_command(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    let keep = RUN_COMMAND_HEAD_LINES + RUN_COMMAND_TAIL_LINES;
    if lines.len() <= keep {
        return output.to_string();
    }
    let head = &lines[..RUN_COMMAND_HEAD_LINES];
    let middle = &lines[RUN_COMMAND_HEAD_LINES..lines.len() - RUN_COMMAND_TAIL_LINES];
    let tail = &lines[lines.len() - RUN_COMMAND_TAIL_LINES..];

    // 错误行始终保留（grep -i error|fail|traceback）
    let error_lines: Vec<String> = middle
        .iter()
        .filter(|l| is_error_line(l))
        .map(|l| l.to_string())
        .collect();
    // 中间的长重复段折叠成一行标记
    let repeat_markers: Vec<String> = fold_repeats(middle)
        .into_iter()
        .filter(|l| l.contains("（重复 "))
        .collect();

    let mut out: Vec<String> = head.iter().map(|l| l.to_string()).collect();
    out.push(format!("... 已省略 {} 行中间输出 ...", middle.len()));
    if !repeat_markers.is_empty() {
        out.push(format!("[重复行折叠 {} 处]", repeat_markers.len()));
        out.extend(repeat_markers);
    }
    if !error_lines.is_empty() {
        out.push(format!("[错误行保留 {} 条]", error_lines.len()));
        out.extend(error_lines);
    }
    out.extend(tail.iter().map(|l| l.to_string()));
    out.join("\n")
}

/// 折叠连续重复行：`x` × N → 一行 + 计数。
fn fold_repeats(lines: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let mut j = i + 1;
        while j < lines.len() && lines[j] == lines[i] {
            j += 1;
        }
        if j - i > 1 {
            out.push(format!("{}  （重复 {} 次）", lines[i], j - i));
        } else {
            out.push(lines[i].to_string());
        }
        i = j;
    }
    out
}

/// 疑似错误行。
pub fn is_error_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("error") || lower.contains("fail") || lower.contains("traceback")
}

/// 疑似代码定义行（结构大纲用）。
pub fn is_definition_line(line: &str) -> bool {
    let t = line.trim_start();
    const KEYWORDS: [&str; 14] = [
        "fn ",
        "struct ",
        "enum ",
        "trait ",
        "impl ",
        "mod ",
        "class ",
        "def ",
        "interface ",
        "function ",
        "type ",
        "const ",
        "export ",
        "async fn ",
    ];
    if KEYWORDS.iter().any(|k| t.starts_with(k)) {
        return true;
    }
    // `pub fn` / `pub(crate) struct` 这类可见性前缀
    let after_pub = t
        .strip_prefix("pub ")
        .or_else(|| t.split_once(") ").map(|(_, rest)| rest));
    after_pub.is_some_and(|rest| KEYWORDS.iter().any(|k| rest.starts_with(k)))
}

/// L1：`list_dir` 裁剪——最多 200 项，超出折叠为目录摘要。
pub fn trim_list_dir(entries: &[String]) -> String {
    if entries.len() <= LIST_DIR_MAX_ITEMS {
        return entries.join("\n");
    }
    let mut out: Vec<String> = entries[..LIST_DIR_MAX_ITEMS].to_vec();
    out.push(format!(
        "... 另有 {} 项已折叠（共 {} 项）",
        entries.len() - LIST_DIR_MAX_ITEMS,
        entries.len()
    ));
    out.join("\n")
}

// ─────────────────────────────── L2 卸载 ───────────────────────────────

/// L2：是否卸载（唯一阈值，决策 110 合并了 `tool_result_max_tokens`）。
pub fn needs_offload(content: &str, settings: &Settings) -> bool {
    count_tokens(content) > settings.offload_threshold_tokens
}

/// L2：卸载后的替代文本（context 只留预览 + 路径）。
pub fn offload_replacement(tool: &str, path: &str, tokens: usize, preview: &str) -> String {
    format!(
        "[工具 {tool} 输出过大，已卸载]\n完整内容：{path}（{tokens} token）\n预览：\n{preview}\n如需完整内容，用 read_file(\"{path}\") 或分段读取。"
    )
}

// ─────────────────────────────── L3 对话压缩 ───────────────────────────────

/// 压缩结果。
#[derive(Debug, Clone, PartialEq)]
pub struct CompactionOutcome {
    pub messages: Vec<Message>,
    /// 规则化摘要（注入为一条 `[摘要]` 消息）。
    pub summary: String,
    /// 被压缩掉的轮次数。
    pub compacted_messages: usize,
}

/// L3：规则化压缩（§12.13.3 压缩规则表）。
///
/// - system / 首条 user 原样保留；
/// - 最近 `keep_recent_rounds` 轮完整保留；
/// - 更早的轮次按消息形态替换为一行摘要。
pub fn compact_messages(messages: &[Message], keep_recent_rounds: usize) -> CompactionOutcome {
    if messages.len() <= keep_recent_rounds + 2 {
        return CompactionOutcome {
            messages: messages.to_vec(),
            summary: String::new(),
            compacted_messages: 0,
        };
    }

    // tool_call_id → (工具名, 路径)，用于把工具结果还原成"已写入/已读取 {path}"（压缩规则表）
    let mut calls: std::collections::HashMap<String, (String, String)> =
        std::collections::HashMap::new();
    for msg in messages {
        for call in &msg.tool_calls {
            let path = serde_json::from_str::<serde_json::Value>(&call.arguments)
                .ok()
                .and_then(|v| v.get("path").and_then(|p| p.as_str()).map(str::to_string))
                .or_else(|| {
                    serde_json::from_str::<serde_json::Value>(&call.arguments)
                        .ok()
                        .and_then(|v| {
                            v.get("command")
                                .and_then(|p| p.as_str())
                                .map(str::to_string)
                        })
                })
                .unwrap_or_default();
            calls.insert(call.id.clone(), (call.name.clone(), path));
        }
    }

    let keep_from = messages.len().saturating_sub(keep_recent_rounds);
    let mut kept: Vec<Message> = Vec::new();
    let mut summary_lines: Vec<String> = Vec::new();
    let mut compacted = 0usize;

    for (i, msg) in messages.iter().enumerate() {
        let must_keep = msg.role == Role::System
            || (msg.role == Role::User && !kept.iter().any(|m| m.role == Role::User))
            || i >= keep_from;
        if must_keep {
            kept.push(msg.clone());
            continue;
        }
        compacted += 1;
        if let Some(line) = summarize_message(msg, &calls) {
            summary_lines.push(line);
        }
    }

    let summary = if summary_lines.is_empty() {
        String::new()
    } else {
        format!("[摘要] 已完成的操作：\n{}", summary_lines.join("\n"))
    };

    // 摘要在首条 user 之后、最近轮次之前插入
    if !summary.is_empty() {
        let insert_at = kept
            .iter()
            .position(|m| m.role == Role::User)
            .map(|i| i + 1)
            .unwrap_or(kept.len());
        kept.insert(insert_at, Message::user(summary.clone()));
    }

    CompactionOutcome {
        messages: kept,
        summary,
        compacted_messages: compacted,
    }
}

/// 单条消息的规则化摘要（压缩规则表逐行实现）。
fn summarize_message(
    msg: &Message,
    calls: &std::collections::HashMap<String, (String, String)>,
) -> Option<String> {
    let content = msg.content.as_deref().unwrap_or("");
    let resolved = msg
        .tool_call_id
        .as_ref()
        .and_then(|id| calls.get(id))
        .map(|(name, path)| (name.as_str(), path.as_str()));
    let tool_name = resolved.map(|(n, _)| n).or(msg.name.as_deref());
    let tool_path = resolved.map(|(_, p)| p).unwrap_or("");

    match msg.role {
        Role::System => None,
        Role::Assistant => {
            if !msg.tool_calls.is_empty() {
                // tool_calls 序列 → 简表（工具名 + 参数摘要）
                let names: Vec<String> = msg
                    .tool_calls
                    .iter()
                    .map(|c| format!("{}（{}）", c.name, args_summary(&c.arguments)))
                    .collect();
                return Some(format!("- 调用：{}", names.join("、")));
            }
            // assistant 推理文本 → 关键决策一句话
            let first = content.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
            if first.is_empty() {
                None
            } else {
                Some(format!("- 关键决策：{}", first.trim()))
            }
        }
        Role::Tool => match tool_name {
            // 已 write_file 的内容直接丢弃（文件在磁盘）
            Some("write_file") => Some(format!("- 已写入 {tool_path}")),
            // 已 read_file 的内容替换为路径 + 行数
            Some("read_file") => Some(format!(
                "- 已读取 {tool_path}（{} 行）",
                content.lines().count()
            )),
            // run_command 输出 → 退出码 + 错误摘要 + 卸载路径
            Some("run_command") => Some(format!(
                "- 命令 `{tool_path}` 结果：{}",
                first_error_or_exit(content)
            )),
            Some(other) => Some(format!("- {other} 结果：{} 字", content.chars().count())),
            None => Some(format!("- 工具结果：{} 字", content.chars().count())),
        },
        Role::User => {
            let first = content.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
            Some(format!("- 用户输入：{}", first.trim()))
        }
    }
}

fn args_summary(arguments: &str) -> String {
    // 参数摘要只取第一个字段的值，避免把整段 content 带进摘要
    match serde_json::from_str::<serde_json::Value>(arguments) {
        Ok(v) => v
            .get("path")
            .and_then(|p| p.as_str())
            .map(str::to_string)
            .or_else(|| {
                v.as_object()
                    .and_then(|o| o.keys().next())
                    .map(|k| format!("{k}=…"))
            })
            .unwrap_or_else(|| "…".to_string()),
        Err(_) => "…".to_string(),
    }
}

fn first_error_or_exit(content: &str) -> String {
    // 优先真正的错误行（error / traceback），再退到 fail 状态行
    let hard_error = |l: &str| {
        let lower = l.to_ascii_lowercase();
        lower.contains("error") || lower.contains("traceback")
    };
    let pick = |pred: &dyn Fn(&str) -> bool| -> Option<String> {
        content
            .lines()
            .find(|l| pred(l))
            .map(|l| l.trim().chars().take(120).collect::<String>())
    };
    if let Some(line) = pick(&hard_error) {
        return format!("失败：{line}");
    }
    if let Some(line) = pick(&is_error_line) {
        return format!("失败：{line}");
    }
    content
        .lines()
        .find(|l| l.contains("exit"))
        .map(|l| l.trim().to_string())
        .unwrap_or_else(|| "成功".to_string())
}

// ─────────────────────────────── L4 兜底 ───────────────────────────────

/// L4 动作（§12.13.3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum L4Action {
    /// 按节点类型分批（test 分批执行 / review 分批评审）。
    BatchByNode,
    /// 拆子代理（仅 spawn_sub_agent 开启时）。
    SpawnSubAgents,
    /// 进入 pending(context_overflow)。
    PendingContextOverflow,
}

/// L4 计划。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct L4Plan {
    /// 强制压缩时保留的轮数。
    pub force_keep_recent_rounds: usize,
    pub action: L4Action,
}

/// L4：L3 后仍超限时的降级路径。
///
/// 这里是 **L4 的自动降级**：`spawn_sub_agent_enabled` 表示「阶段声明了
/// `spawn_sub_agent`」这一事实，用来判断理论上可否自动拆分。注意它与
/// [`crate::pipeline::subagent`] 的**只读子代理工具**是两件事——后者（票 08）是模型
/// 在对话中主动调用的能力，不参与 L4 判定，也没有改变本节的现状。
///
/// `BatchByNode` / `SpawnSubAgents` 从未实现，executor 一律按
/// `pending(context_overflow)` 收口（决策 148 ⑦ / 154：L4 兜底只有两级）。
///
/// **既存偏差（非票 08 引入，且票 08 未改动它）**：决策 154① 裁定删除
/// `L4Plan` / `L4Action` / `plan_l4` / `l4_pending_kind` 这一组，但它们在
/// `84d2c8f`（2026-09-12）随执行器落地后**始终未被删除**；executor 只读 `plan.action`
/// 来打一条 warn，`force_keep_recent_rounds` 与 `l4_pending_kind` 仍只有测试引用。
/// 该清理不属于票 08（本票只改注释、不动语义），登记为既存瑕疵。
pub fn plan_l4(stage: Stage, node: Node, spawn_sub_agent_enabled: bool) -> L4Plan {
    let action = match (stage, node) {
        (Stage::Test, Node::Execute) | (Stage::Review, Node::Execute) => L4Action::BatchByNode,
        (Stage::Develop, Node::Execute) if spawn_sub_agent_enabled => L4Action::SpawnSubAgents,
        _ => L4Action::PendingContextOverflow,
    };
    L4Plan {
        force_keep_recent_rounds: 2,
        action,
    }
}

/// L4 兜底产生的 pending 类型（决策 148 ⑦：未开启子代理 → context_overflow）。
pub fn l4_pending_kind(plan: L4Plan) -> Option<crate::types::PendingKind> {
    match plan.action {
        L4Action::PendingContextOverflow => Some(crate::types::PendingKind::ContextOverflow),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::client::ToolCall;

    fn settings() -> Settings {
        Settings::default()
    }

    #[test]
    fn l0_capacity_uses_configured_ratios() {
        let s = settings();
        let cap = estimate_context_capacity(1000, "sys", "user", &s);
        assert_eq!(cap.total, 1000);
        assert_eq!(cap.soft_limit, 600);
        assert_eq!(cap.hard_limit, 900);
        assert_eq!(cap.reserved_system, count_tokens("sys"));
        assert_eq!(cap.reserved_user, count_tokens("user"));
        assert!(cap.available_for_tools < 1000);
    }

    // ── L1 逐工具裁剪 ──

    #[test]
    fn l1_read_file_keeps_head_and_outline() {
        let mut content = String::new();
        for i in 0..500 {
            if i == 400 {
                content.push_str("pub fn later_symbol() {\n");
            } else {
                content.push_str(&format!("let x{i} = {i};\n"));
            }
        }
        let out = trim_read_file(&content, None);
        assert!(out.contains("let x0 = 0;"));
        assert!(!out.contains("let x450 = 450;"), "超出头部的普通行应被省略");
        assert!(out.contains("已省略 300 行"));
        assert!(out.contains("[结构大纲]"));
        assert!(out.contains("pub fn later_symbol()"));
        assert!(out.contains("offset / limit"));
    }

    #[test]
    fn l1_read_file_short_file_untouched() {
        let content = "a\nb\nc\n";
        assert_eq!(trim_read_file(content, None), content);
    }

    #[test]
    fn l1_run_command_head_tail_and_error_lines() {
        let mut lines: Vec<String> = (0..400).map(|i| format!("log line {i}")).collect();
        lines[250] = "ERROR: test_login failed".to_string();
        let out = trim_run_command(&lines.join("\n"));
        assert!(out.contains("log line 0"));
        assert!(out.contains("log line 399"));
        assert!(out.contains("ERROR: test_login failed"), "错误行必须保留");
        assert!(!out.contains("log line 200"), "中间普通行应折叠");
        assert!(out.contains("[错误行保留 1 条]"));
    }

    #[test]
    fn l1_run_command_folds_repeats() {
        let mut lines = vec!["start".to_string()];
        lines.extend(std::iter::repeat("same".to_string()).take(200));
        lines.push("end".to_string());
        lines.extend((0..120).map(|i| format!("tail {i}")));
        let out = trim_run_command(&lines.join("\n"));
        assert!(out.contains("重复"));
    }

    #[test]
    fn l1_run_command_short_output_untouched() {
        let out = trim_run_command("ok\ndone");
        assert_eq!(out, "ok\ndone");
    }

    #[test]
    fn l1_list_dir_caps_at_200() {
        let entries: Vec<String> = (0..250).map(|i| format!("file{i}.rs")).collect();
        let out = trim_list_dir(&entries);
        assert!(out.contains("file0.rs"));
        assert!(out.contains("file199.rs"));
        assert!(!out.contains("file200.rs"));
        assert!(out.contains("另有 50 项已折叠"));
        assert!(out.contains("共 250 项"));

        let small: Vec<String> = (0..10).map(|i| format!("f{i}")).collect();
        assert_eq!(trim_list_dir(&small), small.join("\n"));
    }

    // ── L2：唯一阈值 4000（决策 110）──

    #[test]
    fn l2_threshold_is_strictly_greater_and_unique() {
        let s = settings();
        assert_eq!(s.offload_threshold_tokens, 4000);
        // 恰好等于阈值不卸载（"超过"才卸载）
        let at_threshold = "a".repeat(4000 * 4);
        assert!(!needs_offload(&at_threshold, &s));
        let over = "a".repeat(4000 * 4 + 4);
        assert!(needs_offload(&over, &s));
    }

    #[test]
    fn l2_replacement_carries_path_and_tokens() {
        let text = offload_replacement(
            "run_command",
            "/home/t/.context/abc.txt",
            12345,
            "head...tail",
        );
        assert!(text.contains("/home/t/.context/abc.txt"));
        assert!(text.contains("12345 token"));
        assert!(text.contains("已卸载"));
        assert!(text.contains("head...tail"));
    }

    // ── L3：压缩规则表逐行 ──

    fn round(call: ToolCall, result: &str) -> Vec<Message> {
        vec![
            Message::assistant(None, vec![call.clone()]),
            Message::tool_result(&call, result),
        ]
    }

    #[test]
    fn l3_drops_written_content_keeps_path() {
        let call = ToolCall {
            id: "c1".into(),
            name: "write_file".into(),
            arguments: r#"{"path":"dev-plan.md","content":"很长很长的内容"}"#.into(),
        };
        let mut messages = vec![Message::system("sys"), Message::user("task")];
        messages.extend(round(call, r#"{"success":true,"path":"dev-plan.md"}"#));
        for _ in 0..6 {
            messages.push(Message::assistant(Some("继续".into()), vec![]));
        }
        let out = compact_messages(&messages, 5);
        assert!(out.summary.contains("已写入"));
        // 第 1 轮被压缩，原始内容不进摘要
        assert!(!out.summary.contains("很长很长的内容"));
    }

    #[test]
    fn l3_replaces_read_content_with_path_and_line_count() {
        let call = ToolCall {
            id: "c1".into(),
            name: "read_file".into(),
            arguments: r#"{"path":"src/a.rs"}"#.into(),
        };
        let mut messages = vec![Message::system("sys"), Message::user("task")];
        messages.extend(round(call, "line1\nline2\nline3"));
        for _ in 0..6 {
            messages.push(Message::assistant(Some("继续".into()), vec![]));
        }
        let out = compact_messages(&messages, 5);
        assert!(out.summary.contains("已读取 src/a.rs"));
        assert!(out.summary.contains("3 行"));
    }

    #[test]
    fn l3_keeps_command_exit_and_error_summary() {
        let call = ToolCall {
            id: "c1".into(),
            name: "run_command".into(),
            arguments: r#"{"command":"cargo test"}"#.into(),
        };
        let mut messages = vec![Message::system("sys"), Message::user("task")];
        messages.extend(round(
            call,
            "running\ntest result: FAILED\nERROR: boom\nexit 1",
        ));
        for _ in 0..6 {
            messages.push(Message::assistant(Some("继续".into()), vec![]));
        }
        let out = compact_messages(&messages, 5);
        assert!(out.summary.contains("失败："));
        assert!(out.summary.contains("ERROR: boom"));
    }

    #[test]
    fn l3_keeps_recent_rounds_verbatim_and_system_user() {
        let mut messages = vec![Message::system("sys"), Message::user("task")];
        for i in 0..10 {
            messages.push(Message::assistant(Some(format!("round {i}")), vec![]));
        }
        let out = compact_messages(&messages, 3);
        assert_eq!(out.messages[0].role, Role::System);
        assert_eq!(out.messages[0].content.as_deref(), Some("sys"));
        assert_eq!(out.messages[1].role, Role::User);
        assert_eq!(out.messages[1].content.as_deref(), Some("task"));
        // 最近 3 轮完整保留
        assert!(out
            .messages
            .iter()
            .any(|m| m.content.as_deref() == Some("round 9")));
        assert!(out
            .messages
            .iter()
            .any(|m| m.content.as_deref() == Some("round 7")));
        // 更早的轮次被压缩
        assert!(!out
            .messages
            .iter()
            .any(|m| m.content.as_deref() == Some("round 0")));
    }

    #[test]
    fn l3_no_op_when_conversation_is_short() {
        let messages = vec![
            Message::system("sys"),
            Message::user("task"),
            Message::assistant(Some("ok".into()), vec![]),
        ];
        let out = compact_messages(&messages, 5);
        assert_eq!(out.compacted_messages, 0);
        assert!(out.summary.is_empty());
        assert_eq!(out.messages, messages);
    }

    #[test]
    fn l3_summary_inserted_after_first_user_message() {
        let mut messages = vec![Message::system("sys"), Message::user("task")];
        for i in 0..8 {
            messages.push(Message::assistant(Some(format!("r{i}")), vec![]));
        }
        let out = compact_messages(&messages, 2);
        assert_eq!(out.messages[0].role, Role::System);
        assert_eq!(out.messages[1].role, Role::User);
        assert!(out.messages[2]
            .content
            .as_deref()
            .unwrap()
            .starts_with("[摘要]"));
    }

    // ── L4 兜底 ──

    #[test]
    fn l4_without_sub_agents_goes_to_context_overflow_pending() {
        // 决策 148 ⑦：子代理默认关闭，L4 直接 pending(context_overflow)
        let plan = plan_l4(Stage::Develop, Node::Execute, false);
        assert_eq!(plan.action, L4Action::PendingContextOverflow);
        assert_eq!(plan.force_keep_recent_rounds, 2);
        assert_eq!(
            l4_pending_kind(plan),
            Some(crate::types::PendingKind::ContextOverflow)
        );
    }

    #[test]
    fn l4_batches_by_node_type_for_test_and_review() {
        assert_eq!(
            plan_l4(Stage::Test, Node::Execute, false).action,
            L4Action::BatchByNode
        );
        assert_eq!(
            plan_l4(Stage::Review, Node::Execute, false).action,
            L4Action::BatchByNode
        );
    }

    #[test]
    fn l4_uses_sub_agents_when_enabled_for_develop_only() {
        assert_eq!(
            plan_l4(Stage::Develop, Node::Execute, true).action,
            L4Action::SpawnSubAgents
        );
        assert_eq!(
            l4_pending_kind(plan_l4(Stage::Develop, Node::Execute, true)),
            None
        );
        // 其他节点即便开启也用分批 / pending
        assert_eq!(
            plan_l4(Stage::Test, Node::Execute, true).action,
            L4Action::BatchByNode
        );
    }

    #[test]
    fn compact_and_hard_limit_predicates() {
        let s = settings();
        let cap = estimate_context_capacity(10_000, "", "", &s);
        assert!(!should_compact(cap.soft_limit, cap));
        assert!(should_compact(cap.soft_limit + 1, cap));
        assert!(!over_hard_limit(cap.hard_limit, cap));
        assert!(over_hard_limit(cap.hard_limit + 1, cap));
    }
}
