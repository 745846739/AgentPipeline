//! 结构化输出强制与兼容解析（§12.12，决策 33）。
//!
//! 三级降级：① `submit_metadata` tool_calls 参数 → ② assistant 文本中的 ```json 块 →
//! ③ 文本中最后一个平衡的 JSON 对象 → ④ 校验错误回填重试 → `agent_retry_max` 耗尽后 pending。
//!
//! 原则：不因"agent 多说了几句话"就直接失败。

use serde::de::DeserializeOwned;

use super::client::AgentResponse;
use crate::Result;

/// 元数据的来源（用于观测 / 测试断言）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataSource {
    /// ① tool_calls 里 `submit_metadata` 的参数（最可靠）。
    ToolCall,
    /// ①降级：tool_calls 的参数**不是合法 JSON**（截断型），靠 [`rescue_truncated_json`]
    /// 补齐收尾后拿到——**这份值是残的**，被截掉的字段确实不见了（所以先试 [`Self::XmlToolCall`]）。
    ///
    /// 与 [`Self::ToolCall`] 分开记，是因为「救援过」这件事必须能从读数里看出来：
    /// 它决定了校验失败时该报「参数被截断」还是「缺字段」（票 01）。
    ToolCallRescued,
    /// ①之上：assistant 正文里的 `<tool_call><function=…><parameter=…>` 文本形态
    /// （本模型在长上下文下会走这条路，见 `.scratch/silent-degradation/spec.md` 缺陷 1）。
    ///
    /// 它比 [`Self::ToolCallRescued`] 完整——实测里被腰斩的是结构化 `tool_calls`，
    /// 而正文里的这一份内容是齐的。
    XmlToolCall,
    /// ② assistant 文本中的 ```json 围栏块。
    FencedJsonBlock,
    /// ③ 文本中最后一个平衡的 `{...}`。
    BalancedJson,
}

/// 提取结果。
#[derive(Debug, Clone, PartialEq)]
pub struct MetadataExtraction {
    pub value: Option<serde_json::Value>,
    pub error: Option<String>,
    pub source: Option<MetadataSource>,
}

impl MetadataExtraction {
    pub fn ok(value: serde_json::Value, source: MetadataSource) -> Self {
        MetadataExtraction {
            value: Some(value),
            error: None,
            source: Some(source),
        }
    }

    pub fn err(message: impl Into<String>) -> Self {
        MetadataExtraction {
            value: None,
            error: Some(message.into()),
            source: None,
        }
    }

    pub fn is_ok(&self) -> bool {
        self.value.is_some()
    }
}

/// 从响应中提取结构化元数据（§12.12 的实现）。
pub fn extract_metadata(response: &AgentResponse) -> MetadataExtraction {
    // ① 优先从 tool_calls 提取 submit_metadata 的参数。
    //    注意：工具参数存在但 JSON 坏掉时**不静默改用别的来源**（agent 明确调用了
    //    submit_metadata，此时换成从正文里捞 JSON 反而危险）。坏法分两种，处理不同：
    //
    //    · **截断型**（本仓已知的上游现象，见 `.scratch/silent-degradation/spec.md` 缺陷 1）：
    //      结构化 `tool_calls` 被腰斩，而**同一轮助手正文里的 `<tool_call>` 文本形态
    //      往往仍是完整的**（实测：args 只剩 29–1136 字符，正文那份八/三个字段一个不缺）。
    //      先拿正文那一份——它更完整，而且是模型真实写下的东西。
    //    · 其余坏法：照旧快速失败，或按既有逻辑救援成残值（记为 ToolCallRescued，
    //      好让下游把「缺字段」如实报成「参数被截断」）。
    for tc in &response.tool_calls {
        if tc.name == "submit_metadata" {
            return match serde_json::from_str::<serde_json::Value>(&tc.arguments) {
                Ok(value) => MetadataExtraction::ok(value, MetadataSource::ToolCall),
                Err(e) => {
                    if let Some(value) = response
                        .content
                        .as_deref()
                        .and_then(|c| find_xml_tool_call(c, "submit_metadata"))
                    {
                        return MetadataExtraction::ok(value, MetadataSource::XmlToolCall);
                    }
                    match rescue_truncated_json(&tc.arguments) {
                        Some(value) => {
                            MetadataExtraction::ok(value, MetadataSource::ToolCallRescued)
                        }
                        None => MetadataExtraction::err(format!("工具参数 JSON 解析失败：{e}")),
                    }
                }
            };
        }
    }

    let Some(content) = response.content.as_deref() else {
        return MetadataExtraction::err("未找到结构化元数据");
    };

    // ①′ 没有 submit_metadata 的 tool_call，但正文里写了**文本形态**的工具调用。
    //     它比 ②③ 的「碰运气从正文里捞 JSON」可靠得多（前者是显式调用，后者是猜），
    //     故排在它们之前。
    if let Some(value) = find_xml_tool_call(content, "submit_metadata") {
        return MetadataExtraction::ok(value, MetadataSource::XmlToolCall);
    }

    // ② assistant 文本中的 ```json / ``` 围栏块（容错）
    if let Some(block) = find_fenced_json(content) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&block) {
            return MetadataExtraction::ok(value, MetadataSource::FencedJsonBlock);
        }
    }

    // ③ 最后一个平衡的 {...}
    if let Some(block) = find_last_balanced_json(content) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&block) {
            return MetadataExtraction::ok(value, MetadataSource::BalancedJson);
        }
    }

    MetadataExtraction::err("未找到结构化元数据")
}

/// 从助手正文里解析**文本形态**的工具调用：
///
/// ```text
/// <tool_call><function=NAME><parameter=K>V</parameter><parameter=K2>V2</parameter></function></tool_call>
/// ```
///
/// 这是本模型（实测 `xiaomi/mimo-v2.6-flash` 经 OpenAI 兼容网关）在长上下文下的真实
/// 落法；此时结构化 `tool_calls` 可能是被腰斩的一份（见 spec 缺陷 1，本节顶部的注释）。
///
/// 参数值按裸文本取，**先试 JSON 再退回字符串**：数组 / 布尔 / 数字都是常态——
/// 实测里 test-design 的 `test_scenarios` 值本身就是一段 JSON 数组，
/// 而 `design_doc_path` 是一段裸路径（`/root/...` 不构成 JSON，必须原样留下）。
pub fn find_xml_tool_call(content: &str, tool: &str) -> Option<serde_json::Value> {
    let marker = format!("<function={tool}>");
    let mut rest = content;
    while let Some(at) = rest.find(&marker) {
        let body = &rest[at + marker.len()..];
        // 只取 `</function>` 之前的那一段参数；没有收尾标签时按到末尾算
        // （半截文本里参数本身仍可能是齐的，缺的只是尾巴）。
        let end = body.find("</function>").unwrap_or(body.len());
        let params = &body[..end];
        let obj = parse_xml_parameters(params);
        if !obj.is_empty() {
            return Some(serde_json::Value::Object(obj));
        }
        rest = &body[end..];
        // 跳过收尾标签，免得同一个块被反复命中
        rest = rest.strip_prefix("</function>").unwrap_or(rest);
    }
    None
}

/// `<parameter=K>V</parameter>` 序列 → 对象。V 的解析见 [`parse_xml_param_value`]。
fn parse_xml_parameters(params: &str) -> serde_json::Map<String, serde_json::Value> {
    const OPEN: &str = "<parameter=";
    const CLOSE: &str = "</parameter>";
    let mut obj = serde_json::Map::new();
    let mut cursor = params;
    while let Some(p) = cursor.find(OPEN) {
        let after_key = &cursor[p + OPEN.len()..];
        let Some(gt) = after_key.find('>') else { break };
        let key = after_key[..gt].trim().to_string();
        let value_text = &after_key[gt + 1..];
        let (raw, next) = match value_text.find(CLOSE) {
            Some(c) => (&value_text[..c], &value_text[c + CLOSE.len()..]),
            None => (value_text, ""),
        };
        if !key.is_empty() {
            obj.insert(key, parse_xml_param_value(raw));
        }
        cursor = next;
        if cursor.is_empty() {
            break;
        }
    }
    obj
}

/// 参数值的形态判断：JSON 优先，解析不了当字符串。
///
/// 两头空白去掉——XML 文本里的换行是排版，不是内容的一部分。
fn parse_xml_param_value(raw: &str) -> serde_json::Value {
    let text = raw.trim();
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(v) => v,
        Err(_) => serde_json::Value::String(text.to_string()),
    }
}

/// 救援被 max_tokens 掐断的 JSON（run59 实证：EOF while parsing at column 1697）。
///
/// 只救「尾部被截断」这一种形态：截断处缺的只是收尾的引号与括号，内容本身没有矛盾。
/// 从截断点逐字符往回退，找**最长**的可解析前缀——按该前缀的扫描状态补齐未闭合的
/// 字符串与括号栈后能解析成**非空对象**即返回。退到只剩空对象说明原参数根本不是
/// JSON（如 `{not json`），返回 `None`，调用方照旧快速失败——非截断型的坏 JSON
/// 不在救援范围。
pub fn rescue_truncated_json(raw: &str) -> Option<serde_json::Value> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) {
        return Some(value);
    }
    let base = raw.trim_end();
    // 逐字符回退（UTF-8 边界上），最长可解析前缀优先。
    let mut cut = base.len();
    while cut > 0 {
        let prefix = &base[..cut];
        let (stack, in_string, ends_with_escape) = scan_json_tail(prefix);
        let mut candidate = prefix.to_string();
        if in_string {
            if ends_with_escape {
                candidate.pop(); // 悬空的尾反斜杠会把补上的引号吃掉
            }
            candidate.push('"');
        }
        for open in stack.iter().rev() {
            candidate.push(if *open == b'{' { '}' } else { ']' });
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&candidate) {
            if value.as_object().is_some_and(|o| !o.is_empty()) {
                return Some(value);
            }
        }
        let prev = base[..cut]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
        cut = prev;
    }
    None
}

/// 扫描前缀的截断态：未闭合的括号栈、是否落在字符串里、字符串尾是否悬空反斜杠。
fn scan_json_tail(prefix: &str) -> (Vec<u8>, bool, bool) {
    let mut stack: Vec<u8> = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    for &b in prefix.as_bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' | b'[' => stack.push(b),
            b'}' | b']' => {
                stack.pop();
            }
            _ => {}
        }
    }
    (stack, in_string, in_string && escaped)
}

/// 把提取到的 JSON 反序列化为阶段结构体（schema 与脚本编译期同源，决策 38）。
pub fn parse_metadata<T: DeserializeOwned>(value: &serde_json::Value) -> Result<T> {
    serde_json::from_value(value.clone())
        .map_err(|e| crate::Error::Validation(format!("元数据校验失败：{e}")))
}

/// 「参数被上游截断」这类失败的判据词（票 01②）。
///
/// 一处定义、两处用：`model_invoke` 用它生成诊断，[`retry_prompt`] 用它把回灌文案
/// 换成「参数过长被截断」的说法——否则模型只会把同一坨正文原样重发一遍，再截断一次。
pub const TRUNCATED_ARGUMENTS_MARKER: &str = "工具参数被截断";

/// 类型化校验失败时的最终诊断（票 01②）。
///
/// `arguments_truncated` 为真（转录里最后一次 `submit_metadata` 的参数不是合法 JSON，
/// 见 [`has_broken_submit_metadata_arguments`]）时改用截断的说法，**且不掺进原始的
/// schema 报错**：那句「missing field `passed`」是截断的假象，把它回灌给模型，模型
/// 只会换着法子重发同一坨正文——本次事故 `validate_output` 连挂三次就是这么来的。
/// 原始诊断不丢，它由调用点写进日志（现场排查看日志，模型看这句人话）。
pub fn validation_failure_error(err: crate::Error, arguments_truncated: bool) -> crate::Error {
    if !arguments_truncated {
        return err;
    }
    crate::Error::Validation(format!(
        "{marker}（不是漏了字段）：本轮 submit_metadata 的参数只到一半就被上游切断了。\
         请把正文精简后再提交，必要时先只提交必填字段。",
        marker = TRUNCATED_ARGUMENTS_MARKER
    ))
}

/// 校验失败后的重试反馈（决策 33「错误回填」；决策 278 定形为**错误 turn**）。
///
/// 决策 278 起，agent 节点的整体失败重试不再空对话起步：上一轮转录**原样保留**，
/// 本函数的返回值作为一条 user 消息追加在转录末尾——原 prompt 不再拼接（转录本身
/// 就是上下文），「错误回填」从拼进 prompt 改成追加进对话。
///
/// **截断型的失败要换一套说法**（票 01②）：原话「未按输出契约提交」会把模型引向
/// 「换个字段名再发一次」，而它真正该做的是把正文缩短。判据见
/// [`TRUNCATED_ARGUMENTS_MARKER`]。
pub fn retry_prompt(error: &str) -> String {
    if error.contains(TRUNCATED_ARGUMENTS_MARKER) {
        return format!(
            "上一轮的输出未按输出契约提交、已判废（{error}）。\n\
             注意：这不是你漏了字段，而是参数在传输中被上游截断（工具调用只传过去一半）。\n\
             请把正文里最长的部分（feedback / 说明类字段）**精简后再提交**，\
             必要时先只提交必填字段，最后仍要调用 submit_metadata。"
        );
    }
    format!(
        "上一轮的输出未按输出契约提交、已判废（{error}）。\n\
         本轮的最终动作必须是调用 submit_metadata 提交结构化元数据。"
    )
}

/// 转录里最后一次 `submit_metadata` 调用的参数是否**不是合法 JSON**。
///
/// 用于把下游的 schema 报错**如实改写**：参数被上游腰斩时，serde 只会说
/// 「缺 `passed`」——那句话会把人和模型都引向错误的方向（"换个字段名再发一次"），
/// 而真正该做的是精简正文或分次提交（票 01）。
///
/// 只看助手消息：`tool_calls` 是模型发起的调用，`tool` 结果里的文本不算。
pub fn has_broken_submit_metadata_arguments(messages: &[super::client::Message]) -> bool {
    for m in messages.iter().rev() {
        if !matches!(m.role, super::client::Role::Assistant) {
            continue;
        }
        for tc in m.tool_calls.iter().rev() {
            if tc.name == "submit_metadata" {
                return serde_json::from_str::<serde_json::Value>(&tc.arguments).is_err();
            }
        }
    }
    false
}

/// 这一轮响应里有没有**参数不是合法 JSON** 的工具调用（票 01③）。
///
/// 空串不算坏：部分工具调用本来就不带参数，`""` 与「还没收到」在这条判据上分不开，
/// 而把后者也算坏会让每一次无参调用都落成非 ok。
pub fn any_tool_arguments_broken(calls: &[super::client::ToolCall]) -> bool {
    calls.iter().any(|tc| {
        !tc.arguments.trim().is_empty()
            && serde_json::from_str::<serde_json::Value>(&tc.arguments).is_err()
    })
}

/// 「参数不是合法 JSON」时的收场说明（票 01③）；参数完整时给 `None`。
///
/// **带上上游的收尾原因**：`length`（OpenAI）/ `max_tokens`（Anthropic）是「上游按上限
/// 切断了输出」的直接证据，排查时它比任何猜测都硬。
pub fn broken_arguments_note(response: &super::client::AgentResponse) -> Option<String> {
    if !any_tool_arguments_broken(&response.tool_calls) {
        return None;
    }
    let mut note =
        String::from("工具参数不是合法 JSON：这一轮的流被上游截断（参数被腰斩的现场形状）");
    if let Some(reason) = response.finish_reason.as_deref() {
        note.push_str(&format!("；上游收尾原因：{reason}"));
    }
    Some(note)
}

/// 提取 ```json ... ``` 或 ``` ... ``` 围栏中的第一个 JSON 对象。
fn find_fenced_json(content: &str) -> Option<String> {
    let mut rest = content;
    while let Some(start) = rest.find("```") {
        let after = &rest[start + 3..];
        // 允许 ```json / ```JSON / ```
        let body_start = after.find('\n').map(|i| i + 1).unwrap_or(0);
        let body = &after[body_start..];
        let end = body.find("```")?;
        let candidate = body[..end].trim();
        if candidate.starts_with('{') && candidate.ends_with('}') {
            return Some(candidate.to_string());
        }
        rest = &body[end + 3..];
    }
    None
}

/// 找文本中**最后一个**平衡的 `{...}`（跳过字符串字面量里的花括号）。
pub fn find_last_balanced_json(content: &str) -> Option<String> {
    let bytes = content.as_bytes();
    let mut depth = 0usize;
    let mut start: Option<usize> = None;
    let mut in_string = false;
    let mut escaped = false;
    let mut last: Option<String> = None;

    for (i, &b) in bytes.iter().enumerate() {
        let c = b as char;
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            '}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    if let Some(s) = start {
                        last = Some(content[s..=i].to_string());
                    }
                }
            }
            _ => {}
        }
    }
    last
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::client::ToolCall;
    use crate::types::ArchitectExecuteMetadata;

    fn response_with_tool(name: &str, args: &str) -> AgentResponse {
        AgentResponse {
            content: None,
            tool_calls: vec![ToolCall {
                id: "call_1".into(),
                name: name.into(),
                arguments: args.into(),
            }],
            prompt_tokens: 10,
            completion_tokens: 5,
            ..Default::default()
        }
    }

    #[test]
    fn level1_tool_call_arguments_win() {
        let resp = response_with_tool("submit_metadata", r#"{"readiness":true}"#);
        let got = extract_metadata(&resp);
        assert_eq!(got.source, Some(MetadataSource::ToolCall));
        assert_eq!(got.value.unwrap()["readiness"], true);
    }

    #[test]
    fn level2_fenced_json_block_from_text() {
        let resp = AgentResponse {
            content: Some("分析完成。\n```json\n{\"readiness\": false, \"blockers\": [\"缺约束\"]}\n```\n以上。".into()),
            tool_calls: vec![],
            ..Default::default()
        };
        let got = extract_metadata(&resp);
        assert_eq!(got.source, Some(MetadataSource::FencedJsonBlock));
        assert_eq!(got.value.unwrap()["readiness"], false);
    }

    #[test]
    fn level2_fence_without_language_tag() {
        let resp = AgentResponse {
            content: Some("```\n{\"readiness\": true}\n```".into()),
            ..Default::default()
        };
        assert_eq!(
            extract_metadata(&resp).source,
            Some(MetadataSource::FencedJsonBlock)
        );
    }

    #[test]
    fn level3_last_balanced_json_object() {
        let resp = AgentResponse {
            content: Some(
                "先说明 {不是 JSON 的部分}，然后我给结果：\n{\"readiness\": true, \"blockers\": []}\n完毕。"
                    .into(),
            ),
            ..Default::default()
        };
        let got = extract_metadata(&resp);
        assert_eq!(got.source, Some(MetadataSource::BalancedJson));
        assert_eq!(got.value.unwrap()["readiness"], true);
    }

    #[test]
    fn level3_picks_the_last_balanced_object() {
        let resp = AgentResponse {
            content: Some(r#"{"a":1} 中间 {"b":{"c":2}}"#.into()),
            ..Default::default()
        };
        let got = extract_metadata(&resp);
        assert_eq!(got.value.unwrap()["b"]["c"], 2);
    }

    #[test]
    fn level3_ignores_braces_inside_strings() {
        let resp = AgentResponse {
            content: Some(r#"{"msg":"包含 } 和 { 的字符串","readiness":true}"#.into()),
            ..Default::default()
        };
        let got = extract_metadata(&resp);
        assert_eq!(got.value.unwrap()["readiness"], true);
    }

    #[test]
    fn failure_when_nothing_extractable() {
        let resp = AgentResponse {
            content: Some("我不知道该说什么。".into()),
            ..Default::default()
        };
        let got = extract_metadata(&resp);
        assert!(!got.is_ok());
        assert_eq!(got.error.as_deref(), Some("未找到结构化元数据"));
    }

    #[test]
    fn broken_tool_arguments_fail_fast_without_falling_back() {
        // agent 明确调用了 submit_metadata，参数坏掉时不得静默改用文本
        let resp = AgentResponse {
            content: Some(r#"{"readiness":true}"#.into()),
            tool_calls: vec![ToolCall {
                id: "c".into(),
                name: "submit_metadata".into(),
                arguments: "{not json".into(),
            }],
            ..Default::default()
        };
        let got = extract_metadata(&resp);
        assert!(!got.is_ok());
        assert!(got.error.unwrap().contains("工具参数 JSON 解析失败"));
    }

    #[test]
    fn rescue_closes_truncated_object_mid_value() {
        // run59 同型：尾部 token 被掐断，缺的只是收尾括号
        let raw = r#"{"readiness": true, "affected_files": ["src/a.rs"], "new_sym"#;
        let v = rescue_truncated_json(raw).unwrap();
        assert_eq!(v["readiness"], true);
        assert_eq!(v["affected_files"][0], "src/a.rs");
    }

    #[test]
    fn rescue_closes_truncated_string_inside_array() {
        let raw = r#"{"readiness": false, "blockers": ["缺约束，建议"#;
        let v = rescue_truncated_json(raw).unwrap();
        assert_eq!(v["blockers"][0], "缺约束，建议");
    }

    #[test]
    fn rescue_refuses_non_truncated_garbage() {
        // 非截断型坏 JSON 照旧快速失败——救援只对 EOF 型负责
        assert!(rescue_truncated_json("{not json").is_none());
        assert!(rescue_truncated_json("").is_none());
    }

    #[test]
    fn rescue_truncated_arguments_flow_through_extraction() {
        let resp = response_with_tool(
            "submit_metadata",
            r#"{"readiness": true, "affected_files": ["src/a.rs"], "new_symb"#,
        );
        let got = extract_metadata(&resp);
        assert!(got.is_ok(), "截断的工具参数应被救援：{:?}", got.error);
        // 救援过这件事**必须从来源读得出来**（票 01）：下游据此把「缺字段」如实报成
        // 「参数被截断」，而不是把残值当合法元数据往下传。
        assert_eq!(got.source, Some(MetadataSource::ToolCallRescued));
        assert_eq!(got.value.unwrap()["readiness"], true);
    }

    #[test]
    fn other_tool_calls_do_not_count_as_metadata() {
        let resp = response_with_tool("write_file", r#"{"path":"design.md"}"#);
        assert!(!extract_metadata(&resp).is_ok());
    }

    #[test]
    fn typed_validation_rejects_missing_required_field() {
        // §12.12 第 3 级：schema 校验失败 → 回填重试（决策 33）
        let value = serde_json::json!({"affected_files": []});
        let err = parse_metadata::<ArchitectExecuteMetadata>(&value).unwrap_err();
        assert!(err.to_string().contains("元数据校验失败"));

        let good = serde_json::json!({
            "readiness": true,
            "affected_files": ["src/a.rs"],
            "new_symbols": [],
            "conflict_warnings": [],
            "acceptance_criteria": [{"id": "AC-1", "description": "能登录"}]
        });
        let parsed: ArchitectExecuteMetadata = parse_metadata(&good).unwrap();
        assert!(parsed.readiness);
        assert_eq!(parsed.acceptance_criteria[0].id, "AC-1");
    }

    #[test]
    fn retry_prompt_is_the_error_turn() {
        // 决策 278：retry_prompt 是续接转录末尾那条错误 turn 的内容来源。
        // 关键指令放在第一行——L3 压缩摘要只保留首行（context.rs::summarize_message）。
        let p = retry_prompt("未找到结构化元数据");
        assert!(p.starts_with("上一轮的输出未按输出契约提交、已判废"));
        assert!(p.contains("未找到结构化元数据"), "原始诊断要给模型");
        assert!(p.contains("本轮的最终动作必须是调用 submit_metadata"));
    }

    #[test]
    fn truncated_arguments_get_their_own_diagnosis_and_retry_wording() {
        // 票 01②：截断型失败不许说成「缺字段」——那句诊断会把模型引向改字段名重发，
        // 而它真正该做的是精简正文（本次事故连挂三次的成因）。
        let schema_err =
            crate::Error::Validation("元数据校验失败：missing field `readiness`".into());
        let truncated = validation_failure_error(schema_err, true).to_string();
        assert!(
            truncated.contains(TRUNCATED_ARGUMENTS_MARKER),
            "{truncated}"
        );
        assert!(!truncated.contains("missing field"), "{truncated}");

        // 判据词与回灌文案同源：改了措辞而忘了这条，这里就红
        let p = retry_prompt(&truncated);
        assert!(p.contains("上游截断"), "{p}");
        assert!(p.contains("精简"), "{p}");

        // 非截断型：schema 诊断原样透出，回灌文案也不该换成截断的说法
        let plain_err =
            crate::Error::Validation("元数据校验失败：missing field `readiness`".into());
        let plain = validation_failure_error(plain_err, false).to_string();
        assert!(plain.contains("missing field"), "{plain}");
        let p = retry_prompt(&plain);
        assert!(!p.contains("精简"), "{p}");
        assert!(
            p.contains("本轮的最终动作必须是调用 submit_metadata"),
            "{p}"
        );
    }

    // ── 票 01：文本形态（XML）的工具调用与截断残片 ──────────────────────────────
    //
    // 下面两段 XML 正文按 106 现场的真实样本**结构逐字**重写（字段名、值形态、
    // 多行正文、JSON 数组值都照原样）：
    // · `ARCH_XML` ← `kanban_node_conversations` id 98 第 110 条消息（2517 字符，
    //   八个字段齐全）；
    // · `TEST_DESIGN_XML` ← id 88（6567 字符，含完整 `test_scenarios` 数组）。
    // 落库的 `content` 原文可在 106 的库里取到；这里的长度做了裁剪，**结构不动**。

    /// architect-design.execute 的完整文本形态（截自 id 98）。
    const ARCH_XML: &str = r#"<tool_call><function=submit_metadata><parameter=readiness>true</parameter><parameter=design_doc_path>/root/.agentpipeline/tasks/01M3QW8CKS07R3MWG9XM4FNYER/design.md</parameter><parameter=affected_files>["frontend/src/router.svelte.ts", "frontend/src/lib/hashLink.ts"]</parameter><parameter=new_symbols>[{"name": "hashLinkPath", "kind": "function", "module_path": "frontend/src/lib/hashLink.ts", "file_path": "frontend/src/lib/hashLink.ts"}]</parameter><parameter=conflict_warnings>[]</parameter><parameter=duplicate_risk>low，无文件/符号冲突警告（仅改 frontend/，无其他活跃任务交集）</parameter><parameter=acceptance_criteria>[{"id": "AC-1", "description": "探针在未修代码上确定性为红，修复后四条全绿。"}, {"id": "AC-2", "description": "同步收敛用例不 await 即断言，未修代码上为红。"}]</parameter><parameter=blockers>[]</parameter></function></tool_call>"#;

    /// test-design.execute 的完整文本形态（截自 id 88）：`test_scenarios` 的值
    /// **本身就是一段 JSON 数组**——这条是「先试 JSON 再退回字符串」的判据来源。
    const TEST_DESIGN_XML: &str = r#"<tool_call><function=submit_metadata><parameter=readiness>true</parameter><parameter=test_scenarios_path>/root/.agentpipeline/tasks/01M3QW8CKS07R3MWG9XM4FNYER/test-scenarios.md</parameter><parameter=test_scenarios>[{"name": "切换目的地时不得出现旧正文帧", "steps": ["挂起目的地请求", "点击链接", "逐帧断言"], "expected_result": "窗口内不出现旧正文帧", "priority": "high", "design_refs": ["AC-1", "AC-2"]}]</parameter></function></tool_call>"#;

    #[test]
    fn xml_form_recovers_the_full_metadata_from_assistant_text() {
        // 现场形态：结构化 tool_calls 被腰斩成 29 字符，正文那份却是齐的。
        let resp = AgentResponse {
            content: Some(format!("检查完成。\n{ARCH_XML}")),
            tool_calls: vec![ToolCall {
                id: "call_1".into(),
                name: "submit_metadata".into(),
                // 逐字取自 id 99/92 的落库值
                arguments: r#"{"blockers": [], "feedback": "#.into(),
            }],
            ..Default::default()
        };
        let got = extract_metadata(&resp);
        assert_eq!(
            got.source,
            Some(MetadataSource::XmlToolCall),
            "截断的工具参数旁边有完整正文时，应取正文那一份：{:?}",
            got.error
        );
        let v = got.value.unwrap();
        assert_eq!(v["readiness"], true);
        assert_eq!(
            v["acceptance_criteria"][1]["id"], "AC-2",
            "被截断的那个字段必须真的回来了"
        );
        assert_eq!(v["affected_files"][1], "frontend/src/lib/hashLink.ts");
        assert_eq!(v["new_symbols"][0]["name"], "hashLinkPath");
        assert_eq!(v["conflict_warnings"], serde_json::json!([]));
    }

    #[test]
    fn xml_form_wins_over_the_rescued_prefix() {
        // 顺序判据：正文 XML 是模型真实写下的完整那一份，比"救援出来的前缀"优先。
        let resp = AgentResponse {
            content: Some(ARCH_XML.into()),
            tool_calls: vec![ToolCall {
                id: "call_1".into(),
                name: "submit_metadata".into(),
                // 可被救援（残值里 readiness 在），但 accept/affected 全丢
                arguments: r#"{"readiness": true, "affected_files": ["src/a.rs"], "new_symb"#
                    .into(),
            }],
            ..Default::default()
        };
        let got = extract_metadata(&resp);
        assert_eq!(got.source, Some(MetadataSource::XmlToolCall));
        assert!(got.value.unwrap()["acceptance_criteria"].is_array());
    }

    #[test]
    fn xml_form_is_used_when_there_is_no_tool_call_at_all() {
        // id 88 形态：模型只把调用写进了正文（结构化 tool_calls 缺席）。
        let resp = AgentResponse {
            content: Some(TEST_DESIGN_XML.into()),
            tool_calls: vec![],
            ..Default::default()
        };
        let got = extract_metadata(&resp);
        assert_eq!(got.source, Some(MetadataSource::XmlToolCall));
        let v = got.value.unwrap();
        assert_eq!(v["readiness"], true);
        assert_eq!(v["test_scenarios"][0]["priority"], "high");
        assert_eq!(
            v["test_scenarios"][0]["design_refs"][1], "AC-2",
            "JSON 数组形态的值必须原样成为数组，不能退化成字符串"
        );
        assert_eq!(
            v["test_scenarios_path"],
            "/root/.agentpipeline/tasks/01M3QW8CKS07R3MWG9XM4FNYER/test-scenarios.md",
            "裸路径不构成 JSON，必须按字符串原样留下"
        );
    }

    #[test]
    fn xml_form_beats_fenced_and_balanced_json_fallbacks() {
        // 正文里同时有围栏 JSON 与文本形态工具调用：后者是显式调用，优先。
        let resp = AgentResponse {
            content: Some(format!(
                "先给一版草稿：\n```json\n{{\"readiness\": false}}\n```\n最终提交：\n{ARCH_XML}"
            )),
            tool_calls: vec![],
            ..Default::default()
        };
        let got = extract_metadata(&resp);
        assert_eq!(got.source, Some(MetadataSource::XmlToolCall));
        assert_eq!(got.value.unwrap()["readiness"], true);
    }

    #[test]
    fn xml_form_matches_only_the_exact_tool_name() {
        assert!(find_xml_tool_call(
            "<tool_call><function=write_file><parameter=path>design.md</parameter></function></tool_call>",
            "submit_metadata"
        )
        .is_none());
        // 前缀相同但名字更长的不算命中（`<function=submit_metadata_x>`）
        assert!(find_xml_tool_call(
            "<tool_call><function=submit_metadata_x><parameter=a>1</parameter></function></tool_call>",
            "submit_metadata"
        )
        .is_none());
    }

    #[test]
    fn xml_form_without_closing_tag_still_yields_the_gathered_parameters() {
        // 半截文本：参数本身齐了，缺的只是尾巴——照收（比整段丢掉强）。
        let content = "<tool_call><function=submit_metadata><parameter=readiness>true</parameter>";
        let v = find_xml_tool_call(content, "submit_metadata").unwrap();
        assert_eq!(v["readiness"], true);
    }

    #[test]
    fn xml_param_value_prefers_json_but_keeps_prose_as_text() {
        assert_eq!(parse_xml_param_value("[]"), serde_json::json!([]));
        assert_eq!(parse_xml_param_value(" true "), serde_json::json!(true));
        assert_eq!(parse_xml_param_value(" 42 "), serde_json::json!(42));
        // 中文散文、裸路径都不是 JSON，原样当字符串
        assert_eq!(
            parse_xml_param_value("\n第一行\n第二行\n"),
            serde_json::json!("第一行\n第二行")
        );
        assert_eq!(
            parse_xml_param_value("/root/.agentpipeline/tasks/x/design.md"),
            serde_json::json!("/root/.agentpipeline/tasks/x/design.md")
        );
        // `low，无文件/符号冲突警告` 以中文标点续写，也不是 JSON
        assert_eq!(
            parse_xml_param_value("low，无冲突"),
            serde_json::json!("low，无冲突")
        );
    }
}
