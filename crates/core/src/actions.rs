//! allowed_actions 权威总表（决策 130 + 132 收官 + 138 + 122 + 86）。
//!
//! 动作集由后端按 `(pending_reason.type, context.kind)` 下发，前端纯渲染。
//! **每个 `side_effect` 动作必须有配对端点**（决策 101 / 119），
//! 否则前端会出现点不动的按钮——[`endpoint_for`] 是这条约束的单一事实来源。

use serde::{Deserialize, Serialize};

use crate::pipeline::landing::entry_node;
use crate::types::{PendingKind, PendingReason, Stage};

/// 动作分层（决策 69）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    /// 走 `POST /tasks/{id}/resume`。
    Resume,
    /// 走各自的专用 API。
    SideEffect,
    /// 纯等待（无系统变更、无端点）——前端渲染为禁用按钮。
    ///
    /// §5 权威表中 `dependency_failed` 的"等待依赖重试"是唯一一项。
    Wait,
}

/// `goto` 的目标。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionTarget {
    pub stage: Stage,
    pub node: crate::types::Node,
    /// 备用节点（网关失败复检等场景可能有多个可选落点）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_kind: Option<String>,
}

/// 下发到前端的动作项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowedAction {
    pub action: String,
    pub kind: ActionKind,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_id: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub requires_input: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<ActionTarget>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl AllowedAction {
    fn resume(action: &str, label: &str) -> Self {
        AllowedAction {
            action: action.to_string(),
            kind: ActionKind::Resume,
            label: label.to_string(),
            cursor_id: None,
            requires_input: false,
            target: None,
        }
    }

    fn side_effect(action: &str, label: &str) -> Self {
        AllowedAction {
            action: action.to_string(),
            kind: ActionKind::SideEffect,
            label: label.to_string(),
            cursor_id: None,
            requires_input: false,
            target: None,
        }
    }

    fn wait(action: &str, label: &str) -> Self {
        AllowedAction {
            action: action.to_string(),
            kind: ActionKind::Wait,
            label: label.to_string(),
            cursor_id: None,
            requires_input: false,
            target: None,
        }
    }

    fn goto(label: &str, stage: Stage, node: crate::types::Node) -> Self {
        let mut a = Self::resume("goto", label);
        a.target = Some(ActionTarget {
            stage,
            node,
            node_kind: None,
        });
        a
    }

    fn with_cursor(mut self, cursor_id: Option<&str>) -> Self {
        self.cursor_id = cursor_id.map(str::to_string);
        self
    }

    fn requiring_input(mut self) -> Self {
        self.requires_input = true;
        self
    }
}

/// context.kind 常量（避免字符串散落）。
pub mod kinds {
    pub const DUPLICATE_RISK: &str = "duplicate_risk";
    pub const DEVELOP_DESIGN_INPUT_INSUFFICIENT: &str = "develop_design_input_insufficient";
    pub const TEST_DESIGN_INPUT_INSUFFICIENT: &str = "test_design_input_insufficient";
    pub const JUDGE_DISAGREEMENT: &str = "judge_disagreement";
    pub const REVIEW: &str = "review";
    pub const TEST_CODE_ISSUE: &str = "test_code_issue";
    pub const GATE_RECHECK: &str = "gate_recheck";
    pub const DIRTY_WORKTREE: &str = "dirty_worktree";
    pub const DEPENDENCY_FAILED: &str = "dependency_failed";
    pub const DEPENDENCY_CANCELLED: &str = "dependency_cancelled";
}

/// 动作 → 配对端点（`None` = resume / wait 类，无需端点）。
///
/// 配对按 `(type, context.kind)` **行内**解析：同一动作名可出现在不同行且端点不同
/// （§5 权威表中 human_review 与 merge_approval 都有 `approve`，分别指向
/// `/review` 与 `/merge/decision`）。只列权威总表能产出的 `side_effect` 动作；
/// `/retry`、`/archive` 等旁路端点不属于 `allowed_actions` 驱动，不在此表内。
pub fn endpoint_for(reason_kind: PendingKind, action: &str) -> Option<&'static str> {
    match (reason_kind, action) {
        (_, "cancel") => Some("POST /tasks/{id}/cancel"),
        (_, "split_task") => Some("POST /tasks/{id}/split"),
        (_, "model_override") => Some("POST /tasks/{id}/model-override"),
        (PendingKind::MergeApproval, "approve" | "return") => {
            Some("POST /tasks/{id}/merge/decision")
        }
        (PendingKind::HumanReview, "approve" | "reject") => Some("POST /tasks/{id}/review"),
        _ => None,
    }
}

/// 全部 side_effect 动作名（端点配对静态检查用）。
pub const SIDE_EFFECT_ACTIONS: [&str; 6] = [
    "cancel",
    "split_task",
    "model_override",
    "approve",
    "return",
    "reject",
];

/// 按 `(type, context.kind)` 下发动作集（§5 权威总表逐行实现）。
///
/// `cursor_id` 在并行区间用于区分两个分支（决策 82）。
pub fn allowed_actions(reason: &PendingReason, cursor_id: Option<&str>) -> Vec<AllowedAction> {
    use crate::types::Node;
    let kind = reason.context.as_ref().and_then(|c| c.kind.as_deref());

    let actions: Vec<AllowedAction> = match (reason.kind, kind) {
        // ── info_insufficient：唯一带自由输入的动作（决策 79）──
        (PendingKind::InfoInsufficient, _) => vec![
            AllowedAction::resume("continue", "补充信息并继续").requiring_input(),
            AllowedAction::side_effect("cancel", "取消任务"),
        ],

        // ── conflict_wait：无用户恢复动作，自动恢复（决策 102）──
        (PendingKind::ConflictWait, _) => {
            vec![AllowedAction::side_effect("cancel", "取消任务")]
        }

        // ── user_decision ──
        (PendingKind::UserDecision, Some(kinds::DUPLICATE_RISK)) => vec![
            // 决策 132：「合并任务」已移出动作集（v1 无端点）
            AllowedAction::goto("回到开发", Stage::Develop, Node::Execute),
            AllowedAction::side_effect("cancel", "取消本任务（其一）"),
        ],
        (PendingKind::UserDecision, Some(kinds::DEVELOP_DESIGN_INPUT_INSUFFICIENT))
        | (PendingKind::UserDecision, Some(kinds::TEST_DESIGN_INPUT_INSUFFICIENT)) => vec![
            AllowedAction::goto(
                "回退到架构设计",
                Stage::ArchitectDesign,
                Node::ValidateInput,
            ),
            AllowedAction::resume("skip", "跳过本设计阶段"),
        ],
        (PendingKind::UserDecision, Some(kinds::JUDGE_DISAGREEMENT)) => vec![
            // 决策 135：continue = 裁决合格 → 特判直接放行 next_stage
            AllowedAction::resume("continue", "裁决合格，继续"),
            // goto = 裁决不合格，打回本阶段的 execute（attempts +1）
            AllowedAction::goto("裁决不合格，打回修复", reason.stage, Node::Execute),
        ],
        (PendingKind::UserDecision, Some(kinds::REVIEW)) => vec![
            AllowedAction::goto("打回开发修复", Stage::Develop, Node::Execute),
            AllowedAction::resume("skip", "强制通过评审"),
        ],
        (PendingKind::UserDecision, Some(kinds::TEST_CODE_ISSUE))
        | (PendingKind::UserDecision, Some(kinds::GATE_RECHECK)) => vec![
            AllowedAction::goto("修改测试用例", Stage::Test, Node::Execute),
            AllowedAction::goto("修改业务代码", Stage::Develop, Node::Execute),
        ],
        (PendingKind::UserDecision, Some(kinds::DIRTY_WORKTREE)) => vec![
            // 决策 132：「放弃合入」已移出动作集（无端点）
            AllowedAction::resume("continue", "我已手动处理，继续合入"),
            AllowedAction::side_effect("cancel", "取消任务"),
        ],
        (PendingKind::UserDecision, _) => vec![
            AllowedAction::resume("skip", "跳过当前阶段"),
            AllowedAction::side_effect("cancel", "取消任务"),
        ],

        // ── retry_exhausted（merge / develop / test 三套动作集）──
        (PendingKind::RetryExhausted, _) if reason.stage == Stage::Merge => vec![
            // 决策 86 / 122：merge 无 skip
            AllowedAction::goto("重试合并", Stage::Merge, Node::Execute),
            AllowedAction::side_effect("cancel", "终止任务"),
        ],
        (PendingKind::RetryExhausted, _)
            if matches!(reason.stage, Stage::Develop | Stage::Test) =>
        {
            vec![
                AllowedAction::goto("重试执行", reason.stage, Node::Execute),
                AllowedAction::resume("skip", "强制进入下一阶段"),
                // 决策 138：带失败摘要回架构设计修订
                AllowedAction::goto(
                    "带失败摘要回架构设计修订",
                    Stage::ArchitectDesign,
                    Node::ValidateInput,
                ),
                AllowedAction::side_effect("cancel", "终止任务"),
            ]
        }
        (PendingKind::RetryExhausted, _) => vec![
            // 落点必须是阶段入口节点（决策 69 / 159），否则 resume 端点 400、按钮点不动：
            // merge / develop / test 的入口恰好是 Execute，设计类阶段是 ValidateInput——
            // 此前硬编码 Execute，设计类阶段的「重试执行」在端点上必然被拒（决策 159）。
            AllowedAction::goto("重试执行", reason.stage, entry_node(reason.stage)),
            AllowedAction::resume("skip", "强制进入下一阶段"),
            AllowedAction::side_effect("cancel", "终止任务"),
        ],

        // ── human_review ──
        (PendingKind::HumanReview, _) => vec![
            // §5 权威表用词：approve / reject。与 merge_approval 的 approve 同名不同行，
            // 端点由 endpoint_for 按行解析（决策 101）。
            AllowedAction::side_effect("approve", "评审通过"),
            AllowedAction::side_effect("reject", "评审不通过，打回开发"),
        ],

        // ── merge_approval（决策 119）──
        (PendingKind::MergeApproval, _) => vec![
            AllowedAction::side_effect("approve", "合入"),
            AllowedAction::side_effect("return", "返回修改"),
        ],

        // ── timeout（merge 例外同决策 122）──
        (PendingKind::Timeout, _) if reason.stage == Stage::Merge => vec![
            AllowedAction::goto("重试合并", Stage::Merge, Node::Execute),
            AllowedAction::side_effect("cancel", "终止任务"),
        ],
        (PendingKind::Timeout, _) => vec![
            AllowedAction::goto("重试执行", reason.stage, Node::Execute),
            AllowedAction::resume("skip", "强制进入下一阶段"),
            AllowedAction::side_effect("cancel", "终止任务"),
        ],

        // ── context_overflow（决策 105）──
        (PendingKind::ContextOverflow, _) => vec![
            AllowedAction::side_effect("split_task", "拆分任务"),
            AllowedAction::side_effect("model_override", "更换长上下文模型"),
            AllowedAction::side_effect("cancel", "取消任务"),
        ],

        // ── dependency_failed（决策 116：按失败依赖终态裁剪）──
        (PendingKind::DependencyFailed, Some(kinds::DEPENDENCY_CANCELLED)) => vec![
            AllowedAction::resume("continue", "忽略失败依赖，继续执行"),
            AllowedAction::side_effect("cancel", "取消任务"),
        ],
        (PendingKind::DependencyFailed, _) => vec![
            AllowedAction::resume("continue", "忽略失败依赖，继续执行"),
            AllowedAction::side_effect("cancel", "取消任务"),
            AllowedAction::wait("wait_dependency_retry", "等待依赖重试"),
        ],
    };

    actions
        .into_iter()
        .map(|a| a.with_cursor(cursor_id))
        .collect()
}

/// 校验动作是否在允许集合内（`POST /resume` 用，决策 49）。
pub fn is_action_allowed(reason: &PendingReason, action: &str) -> bool {
    allowed_actions(reason, None)
        .iter()
        .any(|a| a.action == action)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Node, PendingContext, Stage};

    fn reason(kind: PendingKind, stage: Stage, ctx_kind: Option<&str>) -> PendingReason {
        let mut r = PendingReason::new(kind, stage, Node::Execute, "m");
        if let Some(k) = ctx_kind {
            r = r.with_context(PendingContext::with_kind(k));
        }
        r
    }

    fn actions_of(reason: &PendingReason) -> Vec<String> {
        allowed_actions(reason, None)
            .into_iter()
            .map(|a| a.action)
            .collect()
    }

    // ── 权威总表逐行 ──

    #[test]
    fn info_insufficient_only_free_input_action() {
        let r = reason(PendingKind::InfoInsufficient, Stage::ArchitectDesign, None);
        let acts = allowed_actions(&r, None);
        assert_eq!(
            acts.iter().map(|a| a.action.as_str()).collect::<Vec<_>>(),
            vec!["continue", "cancel"]
        );
        assert!(acts[0].requires_input);
        assert_eq!(acts[0].kind, ActionKind::Resume);
        assert_eq!(acts[1].kind, ActionKind::SideEffect);
    }

    #[test]
    fn conflict_wait_has_no_resume_actions() {
        let r = reason(PendingKind::ConflictWait, Stage::ArchitectDesign, None);
        let acts = allowed_actions(&r, None);
        assert_eq!(acts.len(), 1);
        assert_eq!(acts[0].action, "cancel");
        assert!(acts.iter().all(|a| a.kind != ActionKind::Resume));
    }

    #[test]
    fn duplicate_risk_has_no_merge_task_action() {
        let r = reason(
            PendingKind::UserDecision,
            Stage::ArchitectDesign,
            Some(kinds::DUPLICATE_RISK),
        );
        let acts = actions_of(&r);
        assert_eq!(acts, vec!["goto", "cancel"]);
        assert!(!acts.iter().any(|a| a.contains("merge")));
    }

    #[test]
    fn design_input_insufficient_goes_back_to_architect_or_skip() {
        for k in [
            kinds::DEVELOP_DESIGN_INPUT_INSUFFICIENT,
            kinds::TEST_DESIGN_INPUT_INSUFFICIENT,
        ] {
            let r = reason(PendingKind::UserDecision, Stage::DevelopDesign, Some(k));
            assert_eq!(actions_of(&r), vec!["goto", "skip"]);
        }
    }

    #[test]
    fn judge_disagreement_allows_continue_and_goto_execute() {
        let r = reason(
            PendingKind::UserDecision,
            Stage::ArchitectDesign,
            Some(kinds::JUDGE_DISAGREEMENT),
        );
        let acts = allowed_actions(&r, None);
        assert_eq!(acts.len(), 2);
        assert_eq!(acts[0].action, "continue");
        assert_eq!(acts[0].kind, ActionKind::Resume);
        assert_eq!(acts[1].action, "goto");
        // 裁决不合格 → 打回**本阶段**的 execute
        let target = acts[1].target.as_ref().unwrap();
        assert_eq!(target.stage, Stage::ArchitectDesign);
        assert_eq!(target.node, Node::Execute);
    }

    #[test]
    fn review_rejection_goes_to_develop_or_skip() {
        let r = reason(
            PendingKind::UserDecision,
            Stage::Review,
            Some(kinds::REVIEW),
        );
        assert_eq!(actions_of(&r), vec!["goto", "skip"]);
    }

    #[test]
    fn retry_exhausted_default_has_skip_and_cancel() {
        let r = reason(PendingKind::RetryExhausted, Stage::ArchitectDesign, None);
        assert_eq!(actions_of(&r), vec!["goto", "skip", "cancel"]);
    }

    #[test]
    fn retry_exhausted_goto_lands_on_stage_entry_for_every_stage() {
        // 决策 69：goto 落点必须是入口节点，否则 resume 端点 400、按钮点不动。
        // 回归（主流程票 03）：设计类阶段入口是 ValidateInput，此前硬编码 Execute
        // 导致 architect-design 的「重试执行」在端点上必然被拒。
        for stage in [
            Stage::Init,
            Stage::ArchitectDesign,
            Stage::DevelopDesign,
            Stage::TestDesign,
            Stage::Develop,
            Stage::Review,
            Stage::Test,
            Stage::Merge,
        ] {
            let r = reason(PendingKind::RetryExhausted, stage, None);
            let goto = allowed_actions(&r, None)
                .into_iter()
                .find(|a| a.action == "goto")
                .unwrap_or_else(|| panic!("{stage}: retry_exhausted 缺 goto"));
            let target = goto.target.as_ref().unwrap();
            assert_eq!(target.stage, stage, "{stage}: 重试应落回本阶段");
            assert_eq!(
                target.node,
                entry_node(stage),
                "{stage}: goto 落点必须是本阶段入口节点（决策 69）"
            );
        }
    }

    #[test]
    fn retry_exhausted_merge_has_no_skip() {
        // 决策 86：merge 的 skip 等于越过测试闸门直接合入
        let r = reason(PendingKind::RetryExhausted, Stage::Merge, None);
        let acts = allowed_actions(&r, None);
        assert_eq!(
            acts.iter().map(|a| a.action.as_str()).collect::<Vec<_>>(),
            vec!["goto", "cancel"]
        );
        assert!(!acts.iter().any(|a| a.action == "skip"));
    }

    #[test]
    fn retry_exhausted_develop_and_test_add_architect_escape() {
        for stage in [Stage::Develop, Stage::Test] {
            let r = reason(PendingKind::RetryExhausted, stage, None);
            let acts = allowed_actions(&r, None);
            let back = acts
                .iter()
                .find(|a| {
                    a.action == "goto" && a.target.as_ref().unwrap().stage == Stage::ArchitectDesign
                })
                .expect("应有回架构设计的出口（决策 138）");
            assert_eq!(back.target.as_ref().unwrap().node, Node::ValidateInput);
            assert_eq!(acts.len(), 4);
        }
    }

    #[test]
    fn test_code_issue_offers_both_fix_paths() {
        for k in [kinds::TEST_CODE_ISSUE, kinds::GATE_RECHECK] {
            let r = reason(PendingKind::UserDecision, Stage::Test, Some(k));
            let acts = allowed_actions(&r, None);
            let stages: Vec<Stage> = acts
                .iter()
                .filter_map(|a| a.target.as_ref().map(|t| t.stage))
                .collect();
            assert!(stages.contains(&Stage::Test));
            assert!(stages.contains(&Stage::Develop));
        }
    }

    #[test]
    fn human_review_actions_have_review_endpoint() {
        let r = reason(PendingKind::HumanReview, Stage::Review, None);
        let acts = allowed_actions(&r, None);
        for a in &acts {
            assert_eq!(
                endpoint_for(PendingKind::HumanReview, &a.action),
                Some("POST /tasks/{id}/review")
            );
        }
    }

    #[test]
    fn merge_approval_actions_use_decision_endpoint() {
        let r = reason(PendingKind::MergeApproval, Stage::Merge, None);
        let acts = allowed_actions(&r, None);
        assert_eq!(acts.len(), 2);
        assert_eq!(
            endpoint_for(PendingKind::MergeApproval, &acts[0].action),
            Some("POST /tasks/{id}/merge/decision")
        );
        assert_eq!(
            endpoint_for(PendingKind::MergeApproval, &acts[1].action),
            Some("POST /tasks/{id}/merge/decision")
        );
    }

    #[test]
    fn dirty_worktree_has_no_abandon_action() {
        let r = reason(
            PendingKind::UserDecision,
            Stage::Merge,
            Some(kinds::DIRTY_WORKTREE),
        );
        let acts = actions_of(&r);
        assert_eq!(acts, vec!["continue", "cancel"]);
        assert!(!acts.iter().any(|a| a.contains("abandon")));
    }

    #[test]
    fn timeout_follows_retry_matrix_with_merge_exception() {
        let merge = reason(PendingKind::Timeout, Stage::Merge, None);
        assert_eq!(actions_of(&merge), vec!["goto", "cancel"]);
        let develop = reason(PendingKind::Timeout, Stage::Develop, None);
        assert_eq!(actions_of(&develop), vec!["goto", "skip", "cancel"]);
    }

    #[test]
    fn context_overflow_actions_all_have_endpoints() {
        let r = reason(PendingKind::ContextOverflow, Stage::Develop, None);
        let acts = allowed_actions(&r, None);
        assert_eq!(acts.len(), 3);
        for a in &acts {
            assert_eq!(a.kind, ActionKind::SideEffect);
            assert!(
                endpoint_for(PendingKind::ContextOverflow, &a.action).is_some(),
                "{} 缺端点",
                a.action
            );
        }
    }

    #[test]
    fn dependency_failed_trims_wait_action_when_dep_cancelled() {
        let failed = reason(
            PendingKind::DependencyFailed,
            Stage::Init,
            Some(kinds::DEPENDENCY_FAILED),
        );
        let acts = allowed_actions(&failed, None);
        assert!(acts.iter().any(|a| a.action == "wait_dependency_retry"));
        assert!(acts.iter().any(|a| a.action == "continue"));
        // continue = 忽略失败依赖置回 queued（决策 116）
        assert_eq!(acts[0].kind, ActionKind::Resume);

        let cancelled = reason(
            PendingKind::DependencyFailed,
            Stage::Init,
            Some(kinds::DEPENDENCY_CANCELLED),
        );
        let acts = allowed_actions(&cancelled, None);
        assert!(!acts.iter().any(|a| a.action == "wait_dependency_retry"));
    }

    // ── 端点配对静态检查（决策 101 / 119）──

    #[test]
    fn every_side_effect_action_has_paired_endpoint() {
        // 收集权威表可能产出的全部动作
        let mut all_kinds: Vec<(PendingKind, Option<&str>, Stage)> = vec![
            (PendingKind::InfoInsufficient, None, Stage::ArchitectDesign),
            (PendingKind::ConflictWait, None, Stage::ArchitectDesign),
            (
                PendingKind::UserDecision,
                Some(kinds::DUPLICATE_RISK),
                Stage::ArchitectDesign,
            ),
            (
                PendingKind::UserDecision,
                Some(kinds::DEVELOP_DESIGN_INPUT_INSUFFICIENT),
                Stage::DevelopDesign,
            ),
            (
                PendingKind::UserDecision,
                Some(kinds::TEST_DESIGN_INPUT_INSUFFICIENT),
                Stage::TestDesign,
            ),
            (
                PendingKind::UserDecision,
                Some(kinds::JUDGE_DISAGREEMENT),
                Stage::ArchitectDesign,
            ),
            (
                PendingKind::UserDecision,
                Some(kinds::REVIEW),
                Stage::Review,
            ),
            (
                PendingKind::UserDecision,
                Some(kinds::TEST_CODE_ISSUE),
                Stage::Test,
            ),
            (
                PendingKind::UserDecision,
                Some(kinds::GATE_RECHECK),
                Stage::Test,
            ),
            (
                PendingKind::UserDecision,
                Some(kinds::DIRTY_WORKTREE),
                Stage::Merge,
            ),
            (PendingKind::UserDecision, None, Stage::Review),
            (PendingKind::RetryExhausted, None, Stage::ArchitectDesign),
            (PendingKind::RetryExhausted, None, Stage::Merge),
            (PendingKind::RetryExhausted, None, Stage::Develop),
            (PendingKind::RetryExhausted, None, Stage::Test),
            (PendingKind::HumanReview, None, Stage::Review),
            (PendingKind::MergeApproval, None, Stage::Merge),
            (PendingKind::Timeout, None, Stage::Develop),
            (PendingKind::Timeout, None, Stage::Merge),
            (PendingKind::ContextOverflow, None, Stage::Develop),
            (
                PendingKind::DependencyFailed,
                Some(kinds::DEPENDENCY_FAILED),
                Stage::Init,
            ),
            (
                PendingKind::DependencyFailed,
                Some(kinds::DEPENDENCY_CANCELLED),
                Stage::Init,
            ),
        ];
        all_kinds.sort_by_key(|(k, c, s)| (k.as_str(), c.map(str::to_string), s.as_str()));

        let mut seen_side_effects = std::collections::BTreeSet::new();
        for (k, c, stage) in all_kinds {
            let r = reason(k, stage, c);
            for a in allowed_actions(&r, None) {
                if a.kind == ActionKind::SideEffect {
                    seen_side_effects.insert(a.action.clone());
                    assert!(
                        endpoint_for(k, &a.action).is_some(),
                        "side_effect 动作 {} 没有配对端点（决策 101）",
                        a.action
                    );
                } else if a.kind == ActionKind::Resume {
                    assert!(
                        endpoint_for(k, &a.action).is_none(),
                        "resume 动作 {} 不应有专用端点",
                        a.action
                    );
                }
            }
        }
        // 声明的 side_effect 集合与实际产出一致
        let declared: std::collections::BTreeSet<String> =
            SIDE_EFFECT_ACTIONS.iter().map(|s| s.to_string()).collect();
        let missing: Vec<&String> = declared.difference(&seen_side_effects).collect();
        assert!(
            missing.is_empty(),
            "声明了 side_effect 动作但权威表从不产出：{missing:?}"
        );
    }

    #[test]
    fn cursor_id_is_attached_to_every_action() {
        let r = reason(PendingKind::Timeout, Stage::DevelopDesign, None);
        let acts = allowed_actions(&r, Some("cursor-42"));
        assert!(acts
            .iter()
            .all(|a| a.cursor_id.as_deref() == Some("cursor-42")));
    }

    #[test]
    fn is_action_allowed_rejects_unknown() {
        let r = reason(PendingKind::MergeApproval, Stage::Merge, None);
        assert!(is_action_allowed(&r, "approve"));
        assert!(is_action_allowed(&r, "return"));
        assert!(!is_action_allowed(&r, "skip"), "merge_approval 不含 skip");
        assert!(!is_action_allowed(&r, "goto"));
    }

    #[test]
    fn actions_serialize_with_kind_and_optional_fields() {
        let r = reason(PendingKind::InfoInsufficient, Stage::ArchitectDesign, None);
        let acts = allowed_actions(&r, Some("c1"));
        let json = serde_json::to_value(&acts[0]).unwrap();
        assert_eq!(json["action"], "continue");
        assert_eq!(json["kind"], "resume");
        assert_eq!(json["requires_input"], true);
        assert_eq!(json["cursor_id"], "c1");
        assert!(json.get("target").is_none());

        let acts2 = allowed_actions(
            &reason(
                PendingKind::UserDecision,
                Stage::ArchitectDesign,
                Some(kinds::DUPLICATE_RISK),
            ),
            None,
        );
        let json2 = serde_json::to_value(&acts2[0]).unwrap();
        assert_eq!(json2["target"]["stage"], "develop");
        assert_eq!(json2["target"]["node"], "execute");
    }
}
