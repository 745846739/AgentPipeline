//! 伪阶段（decision 48 / 60 / 67 / 134）。
//!
//! 伪阶段是正式节点内**同步**发起的 LLM 调用，**不占游标行**（decision 107 的精神）：
//! `conflict_check`（architect 产出后的语义第二层）、`validator_cross_check`
//! （agent 型 validate_output 首判不合格后的异族复判）、`project_analysis`
//! （项目分析摘要）。三者都落独立的 run + 会话行（`agent_type = "pseudo:*"`，
//! `parent_run_id` 指向父 run，`cursor_id` 继承父游标，decision 100 / 113），
//! 心跳写父 run 的 `last_activity_at`（decision 88）。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::DuplicateRisk;

/// 伪阶段种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PseudoStage {
    /// 语义第二层冲突比对（decision 60 / 67）。
    ConflictCheck,
    /// agent 型 validate_output 的异族复判（decision 134）。
    ValidatorCrossCheck,
    /// 项目分析摘要（decision 48 / 130）。
    ProjectAnalysis,
}

impl PseudoStage {
    /// `stage_configs.stage` 键（decision 67 / 87：可被阶段配置覆盖档位与 persona）。
    pub fn stage_key(self) -> &'static str {
        match self {
            PseudoStage::ConflictCheck => "conflict_check",
            PseudoStage::ValidatorCrossCheck => "validator_cross_check",
            PseudoStage::ProjectAnalysis => "project_analysis",
        }
    }

    /// run / 会话行的 `agent_type`（decision 100）。
    pub fn agent_type(self) -> &'static str {
        match self {
            PseudoStage::ConflictCheck => "pseudo:conflict_check",
            PseudoStage::ValidatorCrossCheck => "pseudo:validator_cross_check",
            PseudoStage::ProjectAnalysis => "pseudo:project_analysis",
        }
    }

    /// 内嵌 persona（decision 7：可被 `persona_path` / `persona_append` 覆盖）。
    ///
    /// `project_analysis` 的 persona 允许为空（decision 87）——省略时只展示事实清单。
    pub fn embedded_persona(self) -> &'static str {
        match self {
            PseudoStage::ConflictCheck => {
                "你是设计语义冲突比对 agent。只比较两份设计的意图与新增符号是否重复实现"
            }
            PseudoStage::ValidatorCrossCheck => {
                "你是独立复核 agent。对上游 validate_output 的「不合格」结论做异族复判，只依据产出物本身"
            }
            PseudoStage::ProjectAnalysis => {
                "你是项目分析 agent。基于确定性探测事实清单写人读摘要并标注可疑项"
            }
        }
    }

    /// `submit_metadata` 的 schema 工具（与结果结构体同源，decision 38）。
    pub fn submit_tool(self) -> crate::agent::client::ToolDef {
        match self {
            PseudoStage::ConflictCheck => crate::agent::client::submit_metadata_tool::<
                ConflictCheckResult,
            >("提交语义冲突比对结论"),
            PseudoStage::ValidatorCrossCheck => {
                crate::agent::client::submit_metadata_tool::<CrossCheckResult>("提交异族复判结论")
            }
            PseudoStage::ProjectAnalysis => crate::agent::client::submit_metadata_tool::<
                ProjectAnalysisResult,
            >("提交项目分析摘要"),
        }
    }
}

/// conflict_check 的结论（decision 60：`duplicate_risk = high` 才上交用户）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ConflictCheckResult {
    pub duplicate_risk: DuplicateRisk,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// validator_cross_check 的结论（decision 134 / 135）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CrossCheckResult {
    pub passed: bool,
    #[serde(default)]
    pub blockers: Vec<String>,
}

/// project_analysis 的摘要（decision 78 / 130：探测事实由代码给出，LLM 只写摘要）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProjectAnalysisResult {
    pub summary: String,
    #[serde(default)]
    pub suspicious: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pseudo_stage_keys_and_agent_types_match_docs() {
        assert_eq!(PseudoStage::ConflictCheck.stage_key(), "conflict_check");
        assert_eq!(
            PseudoStage::ValidatorCrossCheck.stage_key(),
            "validator_cross_check"
        );
        assert_eq!(PseudoStage::ProjectAnalysis.stage_key(), "project_analysis");
        assert_eq!(
            PseudoStage::ConflictCheck.agent_type(),
            "pseudo:conflict_check"
        );
        assert_eq!(
            PseudoStage::ValidatorCrossCheck.agent_type(),
            "pseudo:validator_cross_check"
        );
        assert_eq!(
            PseudoStage::ProjectAnalysis.agent_type(),
            "pseudo:project_analysis"
        );
    }

    #[test]
    fn conflict_check_persona_is_non_empty_but_project_analysis_may_be_empty() {
        // decision 87：conflict_check 强制非空；project_analysis 内容可为空（此处仍给默认）
        assert!(!PseudoStage::ConflictCheck
            .embedded_persona()
            .trim()
            .is_empty());
        assert!(!PseudoStage::ValidatorCrossCheck
            .embedded_persona()
            .trim()
            .is_empty());
    }

    #[test]
    fn result_structs_round_trip() {
        let conflict = ConflictCheckResult {
            duplicate_risk: DuplicateRisk::High,
            reason: Some("两边都在实现登录".into()),
        };
        let v = serde_json::to_value(&conflict).unwrap();
        assert_eq!(v["duplicate_risk"], "high");
        let back: ConflictCheckResult = serde_json::from_value(v).unwrap();
        assert_eq!(back, conflict);

        let cross: CrossCheckResult = serde_json::from_value(serde_json::json!({
            "passed": true,
            "blockers": []
        }))
        .unwrap();
        assert!(cross.passed);
    }

    #[test]
    fn submit_tools_use_result_schema() {
        let tool = PseudoStage::ConflictCheck.submit_tool();
        assert_eq!(tool.name, "submit_metadata");
        assert!(tool
            .parameters
            .get("properties")
            .unwrap()
            .get("duplicate_risk")
            .is_some());
        let tool = PseudoStage::ValidatorCrossCheck.submit_tool();
        assert!(tool
            .parameters
            .get("properties")
            .unwrap()
            .get("passed")
            .is_some());
    }
}
