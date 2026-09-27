//! 上下文管理与四级压缩（§12.13，决策 110）。
//!
//! L0 容量预估 → L1 工具结果裁剪（常开、零成本）→ L2 大结果卸载（唯一阈值
//! `offload_threshold_tokens`，默认 4000）→ L3 对话压缩（规则化优先）→ L4 兜底。
//!
//! 核心前提：**文件系统是 source of truth**——任何被裁剪/丢弃的文件内容都能低成本重读，
//! 因此压缩是安全的。

use super::client::{Message, Role};
use crate::config::Settings;

/// 模型输出预留（L0 容量预估用）。
pub const OUTPUT_RESERVE: usize = 4096;

/// L1：`read_file` 默认返回头部行数。
pub const READ_FILE_HEAD_LINES: usize = 200;
/// L1：`read_file` 单次**最多读入**的字节数（决策 226）。
///
/// 它管的是「体量」这件事本身：超过它就不再整份读（小文件仍走原路，好保住
/// [`trim_read_file`] 的结构大纲），改读需要的那一段。
///
/// 这个上限同时**替代**了此前「按路径前缀拒绝 `{root}/logs`」那条规则——那条的理由写着
/// 「价值不在秘密而在体量：一个 200MB 的日志文件进上下文的代价」，而体量是**可结构地**
/// 管的，不必用一堵把 877 字节的日志也一起挡住的墙（2026-09-19 实测：定死那次僵死根因的
/// 就是那条日志，而值班长读不到它）。
pub const READ_FILE_MAX_BYTES: usize = 4 * 1024 * 1024;
/// L1：`run_command` 保留前 N 行。
pub const RUN_COMMAND_HEAD_LINES: usize = 50;
/// L1：`run_command` 保留后 N 行。
pub const RUN_COMMAND_TAIL_LINES: usize = 100;
/// L1：`list_dir` 最多列出的条目数。
pub const LIST_DIR_MAX_ITEMS: usize = 200;

/// 粗略 token 估算——**加权启发式**（决策 309，票 foreman-burns-without-guard 01）。
///
/// 规则：**CJK 字符按 1 token 计，其余按 4 字符 ≈ 1 token**。
///
/// **为什么必须加权**（实测，2026-09-27）：旧口径 `chars ÷ 4` 对这个仓库的**中英混合日志**
/// 低估 **≥ 5.5 倍**——第 94 轮真实输入 **561,210** 时估算只有 ≤ 102,400。后果不是「估算
/// 不准」这么轻：轮内压缩的触发线是「估算是窗口的 80%」，而估算永远摸不到那条线，
/// **95 轮一次都没触发**（全库 0 条「值班长轮内上下文超线」，而管道侧同款压缩有命中）。
///
/// **为什么不上 tokenizer**：全仓当前没有任何 tokenizer 依赖；vendor 是 `openai` 但 model 经
/// 第三方代理，**词表未必相同**——引 `tiktoken` 可能把「已知偏低 5.5 倍」换成「不可见的
/// 偏差」，后者更难查（哪个才对没有台账可对）。加权启发式的偏差方向与量级都是**可测**的，
/// 而偏差可测才谈得上标定（见 [`TOKEN_ESTIMATE_TOLERANCE`] 与
/// [`crate::pipeline::window_calibration`] 里那 95 条真实读数）。
///
/// **单点实现**：这一处同时供流水线 L3 压缩与值班长的轮内压缩用（决策 291「两端同源」），
/// 所以改这里两侧一起变——这正是「同一批改动要同时跑两侧压缩测试」的由来。
///
/// 判 CJK 用码点区间而不是「非 ASCII」：日文假名、韩文谚语与中文一样是 1 token/字，
/// 而带重音的拉丁字母（`é`）属于「其余」那一档。
pub fn count_tokens(text: &str) -> usize {
    let mut cjk = 0usize;
    let mut other = 0usize;
    for ch in text.chars() {
        if is_cjk(ch) {
            cjk += 1;
        } else {
            other += 1;
        }
    }
    cjk + other.div_ceil(4)
}

/// 估算与真读数的允许区间（决策 309）：`估算 ÷ 真读数 ∈ [0.7, 1.5]`。
///
/// **故意不对称**：估算偏低会让触发线摸不到（这次的病），故下界卡得紧（7 折）；
/// 估算偏高只会早一点压缩（代价是 prefix 缓存命中率），允许更松（1.5 倍）。
pub const TOKEN_ESTIMATE_TOLERANCE: (f64, f64) = (0.7, 1.5);

/// 这个码点算不算「一个字一个 token」的那一档：CJK 表意文字、假名、谚文、全角标点。
fn is_cjk(ch: char) -> bool {
    matches!(ch as u32,
        0x3000..=0x303F        // CJK 标点（全角逗号句号、括号、顿号）
        | 0x3040..=0x30FF      // 平假名 / 片假名
        | 0x3400..=0x4DBF      // 扩展 A
        | 0x4E00..=0x9FFF      // 基本区
        | 0xAC00..=0xD7AF      // 谚文音节
        | 0xF900..=0xFAFF      // 兼容表意文字
        | 0xFF00..=0xFF60      // 全角 ASCII
        | 0xFFE0..=0xFFE6      // 全角符号（¥ 那一档）
        | 0x20000..=0x2FA1F    // 扩展 B 及以上
    )
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

/// 一次请求全文的 token 估算：两段静态 prompt + 一组消息。
///
/// 两端同源（决策 291 / 票 06）：流水线每轮的预算检查（`RequestPlan::check_budget`）
/// 与值班长的轮内压缩吃**同一份**算术——两处各写一份的下场是「一边到线了、另一边还没到」，
/// 而它们说的是同一件事。
pub fn estimate_messages_tokens(system: &str, user: &str, messages: &[Message]) -> usize {
    count_tokens(user) + count_tokens(system) + messages.iter().map(message_tokens).sum::<usize>()
}

/// 单条消息的估算：正文 + 模型自己发出的 tool_calls 参数（同样占窗口，漏算会低估）。
fn message_tokens(message: &Message) -> usize {
    let text = count_tokens(message.content.as_deref().unwrap_or(""));
    let args: usize = message
        .tool_calls
        .iter()
        .map(|c| count_tokens(&c.name) + count_tokens(&c.arguments))
        .sum();
    text + args
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
    compact_messages_from(messages, keep_recent_rounds, 0)
}

/// 带「本轮起点」的压缩（决策 180，票 13 必要条件三）。
///
/// `current_start` 之下标之前是**载入的历史**（上一 attempt 续接下来的一段），
/// 它们不得充当「第一条 user 消息」这个锚点：载入历史后那条 user 消息是**上一轮**的提问，
/// 把它当锚点保下来会占掉 keep 预算，把本轮真正的起点挤成摘要。
///
/// `current_start = 0` 时与不带起点完全等价（历史为空）。
pub fn compact_messages_from(
    messages: &[Message],
    keep_recent_rounds: usize,
    current_start: usize,
) -> CompactionOutcome {
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
    // 摘要的插入位：本轮第一条 user 之后（没有则末尾）
    let mut anchor: Option<usize> = None;

    for (i, msg) in messages.iter().enumerate() {
        // 锚点候选：本轮（`i >= current_start`）的第一条 user 消息。载入的历史不算——
        // 那条是上一轮的提问
        let is_anchor = msg.role == Role::User && i >= current_start && anchor.is_none();
        let must_keep = msg.role == Role::System || is_anchor || i >= keep_from;
        if must_keep {
            kept.push(msg.clone());
            if is_anchor {
                anchor = Some(kept.len());
            }
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

    // 摘要在本轮首条 user 之后、最近轮次之前插入
    if !summary.is_empty() {
        let insert_at = anchor.unwrap_or(kept.len());
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
//
// **L4 在 v1 只有两级**（决策 154）：压缩（`compact_messages_from`，L3 的那一次）→
// `pending(context_overflow)`。§12.13.3 设计阶梯里的第二级（按节点分批 / 拆子代理）
// 整体不做，因此本模块**没有**任何 L4 计划结构——原先的 `L4Plan` / `L4Action` /
// `plan_l4` / `l4_pending_kind` 整组（`84d2c8f` 落地）已按决策 154① 删除：那两个
// 非 pending 变体永不可达（executor 返回后立刻否决），`force_keep_recent_rounds`
// 无消费者。pending 由 `pipeline::executor` 在压缩后仍超硬限时**直接构造**。
//
// 与 `spawn_sub_agent` 的边界（决策 172③）：那个只读子代理是**模型在对话中主动调用的
// 工具**，与这里的自动降级路径互不相干——它不参与 L4 判定（决策 154 的原裁决不变）。

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::client::ToolCall;

    fn settings() -> Settings {
        Settings::default()
    }

    /// **加权估算**（决策 309，票 01）：中文一个字算一个 token，其余仍是四字符一个。
    ///
    /// 旧口径 `chars ÷ 4` 对中英混合日志低估 ≥ 5.5 倍——这一条把那 5.5 倍的来路钉在
    /// 最小可复现的形状上：同一段中文，新口径的读数是旧口径的 **4 倍**。
    #[test]
    fn count_tokens_charges_one_token_per_cjk_character() {
        let chinese = "值班长在执行体里挂住了四小时";
        let chars = chinese.chars().count();
        assert_eq!(chars, 14);
        assert_eq!(count_tokens(chinese), chars, "中文：一字一 token");
        // 旧口径（chars ÷ 4）在同一段上只报 4：一字一 token 与四字一 token 的差，
        // 就是那 5.5 倍偏差的主体。
        assert_eq!(chars.div_ceil(4), 4);
        assert_eq!(count_tokens(chinese), 4 * chars.div_ceil(4) - 2);
    }

    /// 英文/ASCII 那一档**逐字沿用旧口径**（4 字符 ≈ 1 token）：加权只加在中文上。
    #[test]
    fn count_tokens_leaves_ascii_on_the_old_rule() {
        for text in [
            "hello world",
            "{\"tool\":\"read_file\",\"arguments\":{\"path\":\"src/lib.rs\"}}",
            "",
            "a",
            "abcd",
            "abcde",
        ] {
            assert_eq!(
                count_tokens(text),
                text.chars().count().div_ceil(4),
                "{text:?} 这一档不该变"
            );
        }
    }

    /// 混排：中英各算各的，再相加——不是一个整体比例。
    #[test]
    fn count_tokens_mixes_the_two_bands() {
        // 3 个中文 + 6 个 ASCII（4 字符 ≈ 1 token ⇒ 6/4 上取整 = 2）
        assert_eq!(count_tokens("值班长 hello"), 3 + 2);
        // 日文假名与韩文谚文与中文同档（它们同样是「一字一 token」的语言）
        assert_eq!(count_tokens("テスト"), 3);
        assert_eq!(count_tokens("한국어"), 3);
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

    /// 载入历史里的 user 消息**不是**本轮的锚点（决策 180，票 13 必要条件三）。
    ///
    /// 锚点决定摘要插在哪：插在「本轮第一条 user」之后，本轮的工具往来才能留在摘要之后。
    /// 续接把上一轮的 messages 载到前面，其中那条 user 消息（若有）会比本轮起点更早，
    /// 不设边界就会顶掉锚点——本用例把它钉死。
    ///
    /// 今天的真线上载入历史里只有 assistant / tool 消息（user prompt 走 `user_prompt`
    /// 字段，不进 `messages`），故这条边界是**防御性**的：规则本身在此直接钉住，等 `messages`
    /// 里真出现 user 消息（多轮形态）时立刻生效。
    #[test]
    fn l3_anchor_ignores_the_loaded_history_and_takes_the_current_round() {
        let mut messages = vec![
            Message::system("sys"),
            // 载入的历史：上一轮的提问（`current_start = 2` 之前）
            Message::user("上一轮的提问"),
            Message::assistant(Some("上一轮的回答".into()), vec![]),
            // 本轮起点
            Message::user("本轮提问"),
        ];
        for i in 0..8 {
            messages.push(Message::assistant(Some(format!("r{i}")), vec![]));
        }
        let out = compact_messages_from(&messages, 2, 2);
        assert_eq!(out.messages[0].role, Role::System);
        assert_eq!(out.messages[1].role, Role::User);
        assert_eq!(
            out.messages[1].content.as_deref(),
            Some("本轮提问"),
            "锚点须是本轮第一条 user，历史上那条已被压成摘要：{:?}",
            out.summary
        );
        assert!(out.messages[2]
            .content
            .as_deref()
            .unwrap()
            .starts_with("[摘要]"));
    }

    // ── L4 兜底 ──
    //
    // 「压缩后仍超硬限 → `pending(context_overflow)`」这条行为的等价断言在 L2：
    // `crates/core/tests/integration/executor.rs::context_overflow_ctx` 造成真超限现场（窗口 1000 /
    // 硬限 900 + 一次大块元数据），两条用例分别钉住 pending 的 kind 与「退出路径补写会话行」。
    // 它落在执行器而非本模块——构造 pending 的是 `executor::enforce_context_budget`，
    // 本模块只提供 `should_compact` / `over_hard_limit` 两个谓词（`compact_and_hard_limit_predicates`）。
    //
    // 原先此处还有三条针对已删除的 `plan_l4` / `L4Action` 的断言；它们钉的是
    // 「哪个变体被选中」，而那两个非 pending 变体从未实现也从不可达（决策 154①），
    // 故随该组一并删除。

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
