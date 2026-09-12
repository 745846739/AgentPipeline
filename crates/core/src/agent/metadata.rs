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
    //    注意：工具参数存在但 JSON 坏掉时**立即失败**，不再降级——这与文档一致
    //    （agent 明确调用了 submit_metadata，此时静默改用文本反而危险）。
    for tc in &response.tool_calls {
        if tc.name == "submit_metadata" {
            return match serde_json::from_str::<serde_json::Value>(&tc.arguments) {
                Ok(value) => MetadataExtraction::ok(value, MetadataSource::ToolCall),
                Err(e) => MetadataExtraction::err(format!("工具参数 JSON 解析失败：{e}")),
            };
        }
    }

    let Some(content) = response.content.as_deref() else {
        return MetadataExtraction::err("未找到结构化元数据");
    };

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

/// 把提取到的 JSON 反序列化为阶段结构体（schema 与脚本编译期同源，决策 38）。
pub fn parse_metadata<T: DeserializeOwned>(value: &serde_json::Value) -> Result<T> {
    serde_json::from_value(value.clone())
        .map_err(|e| crate::Error::Validation(format!("元数据校验失败：{e}")))
}

/// 校验失败后的重试 prompt（决策 33：错误回填，要求重新调用 submit_metadata）。
///
/// 首轮为空不渲染的规则只适用于 feedback 段（决策 126 / 138）；重试段必然非空。
pub fn retry_prompt(original: &str, error: &str) -> String {
    format!("{original}\n\n上次调用失败：{error}\n请重新调用 submit_metadata。")
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
    fn retry_prompt_appends_error() {
        let p = retry_prompt("原始 prompt", "缺少 readiness 字段");
        assert!(p.starts_with("原始 prompt"));
        assert!(p.contains("上次调用失败：缺少 readiness 字段"));
        assert!(p.contains("请重新调用 submit_metadata。"));
    }
}
