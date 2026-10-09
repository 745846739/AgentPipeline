//! 条件边路由（决策 39 集中定义；docs/implementation.md §11.2）。
//!
//! 路由输入是**游标**而不是任务（决策 80）：attempts 与 pending 都是游标级的。
//! 所有 `EdgeKind` 的判定都在这里，executor 只负责执行返回的边。

use crate::types::{
    Approval, EdgeKind, FailureCause, Gate, GateFailureKind, MergeResult, Node, NodeCursor,
    PendingKind, Stage, SyncDecision, SyncDecisionKind, TestFailure, TestResult,
};

/// 路由的元数据视图（各阶段 submit_metadata 的归一化投影）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MetadataView {
    /// validate_input 的充分性（architect / develop-design / test-design）。
    pub readiness: bool,
    /// validate_input 不充分时提交的问题清单（决策 277④）：`info_insufficient`
    /// 的 pending 消息要把它带给用户，路由侧才不用回头翻会话行。
    pub blockers: Vec<String>,
    /// agent 型 validate_output 的判定。
    pub passed: bool,
    /// test.validate_output 的根因分类（决策 62 / 85）。
    pub failures: Vec<TestFailure>,
    /// develop.execute 显式申报了「本任务零变更」（决策 391）：develop.validate_output
    /// 据此挂 pending(user_decision) 交用户确认收尾，不再走常规放行/重试。
    pub zero_changes: bool,
    /// develop / test 的代码门判定**任务工作区不干净**（决策 416 A）：产出没落提交就
    /// 交了元数据。它必须短路在 `route_code_gate` **之前**——脏工作区不是「用例 vs
    /// 业务代码」的争议，落到那条分支会误挂 `pending(user_decision, test_code_issue)`，
    /// 把一条本该打回 execute 自己提交的确定性问题推给用户拍板。
    pub worktree_dirty: bool,
}

impl MetadataView {
    pub fn readiness(readiness: bool) -> Self {
        MetadataView {
            readiness,
            ..Default::default()
        }
    }

    /// validate_input 的完整投影（决策 277④）：readiness 连同它提交的问题清单。
    pub fn readiness_with_blockers(readiness: bool, blockers: Vec<String>) -> Self {
        MetadataView {
            readiness,
            blockers,
            ..Default::default()
        }
    }

    pub fn passed(passed: bool) -> Self {
        MetadataView {
            passed,
            ..Default::default()
        }
    }

    pub fn from_test_result(t: &TestResult) -> Self {
        MetadataView {
            passed: t.passed,
            failures: t.failures.clone(),
            ..Default::default()
        }
    }

    /// sync-check 判定投影到 [`MetadataView::passed`]（`proceed` 即"放行"，决策 83）。
    ///
    /// sync-check **不占游标行**（决策 107），回溯由 [`crate::pipeline`] 的 `advance_join`
    /// 按 [`SyncDecisionKind`] 落库，不经 [`route`]；本方法只提供字段投影。
    pub fn from_sync_decision(d: &SyncDecision) -> Self {
        MetadataView {
            passed: d.decision == SyncDecisionKind::Proceed,
            ..Default::default()
        }
    }

    /// 全部失败用例都是用例自身的问题（decision 62：全部 test_issue → 自己修用例）。
    pub fn all_failures_are_test_issues(&self) -> bool {
        !self.failures.is_empty()
            && self
                .failures
                .iter()
                .all(|f| f.failure_cause == FailureCause::TestIssue)
    }
}

/// 路由上下文。
///
/// 不派生 `Default`：`MergeResult` 无 Default（闸门未跑 ≠ 通过），
/// 构造时必须显式给出 merge 视图。
#[derive(Debug, Clone)]
pub struct RouteContext {
    pub validate_retry_max: u32,
    pub metadata: MetadataView,
    pub merge: MergeResult,
}

/// cross_family_judge 下的 validate_output 结论（决策 134 / 135）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidateOutcome {
    Passed,
    /// 两侧一致不合格 → 走路由打回 execute。
    Failed,
    /// 首判不合格但异族复判合格 → pending(user_decision)，**不经路由**（决策 135）。
    JudgeDisagreement,
}

/// 合并首判与异族复判（决策 134 / 135）。
///
/// 复判合格时分歧上交用户终审：这不是 `EdgeKind`，因此永远不会进 [`route`]——
/// 节点内直接置 pending，路由层看不到这条路径。
pub fn resolve_validate_output(
    cross_family_judge: bool,
    first_passed: bool,
    cross_passed: Option<bool>,
) -> ValidateOutcome {
    if first_passed {
        return ValidateOutcome::Passed;
    }
    if !cross_family_judge {
        return ValidateOutcome::Failed;
    }
    match cross_passed {
        Some(true) => ValidateOutcome::JudgeDisagreement,
        // 复判也不合格 / 复判缺失 → 维持原路径
        _ => ValidateOutcome::Failed,
    }
}

/// 路由入口（key 为 `(stage, node)`）。
pub fn route(cursor: &NodeCursor, ctx: &RouteContext) -> EdgeKind {
    match (cursor.stage, cursor.node) {
        // develop / test 的 validate_output 是纯代码闸门（决策 62）
        (Stage::Develop, Node::ValidateOutput) => {
            // 决策 391：execute 显式申报了零变更 → 不走常规放行/重试，
            // 挂 pending(user_decision) 交用户确认（确认收尾 / 打回继续）。
            // 这一条排在工作区守卫**之前**：申报零变更的收尾语义独立于工作区状态，
            // 把它挪后只会给决策 391 已有的用例凭空多一条分支。
            if ctx.metadata.zero_changes {
                EdgeKind::Pending(PendingKind::UserDecision)
            // 决策 416 A：产出没落提交 → 打回 execute 自己提交，不进用户裁决。
            } else if ctx.metadata.worktree_dirty {
                route_after_validate_output(cursor.validate_attempts, ctx.validate_retry_max)
            } else {
                route_code_gate(cursor, ctx)
            }
        }
        (Stage::Test, Node::ValidateOutput) => {
            // 决策 416 A：必须短路在 `route_code_gate` 之前——脏工作区不是「用例 vs
            // 业务代码」的争议，落进那条分支会因 `all_failures_are_test_issues()` 对
            // 空失败表恒假而误挂 `pending(user_decision, test_code_issue)`。
            if ctx.metadata.worktree_dirty {
                route_after_validate_output(cursor.validate_attempts, ctx.validate_retry_max)
            } else {
                route_code_gate(cursor, ctx)
            }
        }
        (Stage::Merge, Node::Execute) => route_merge(ctx),
        // sync-check 不占游标行（决策 107）：execute_node 直接报错拦截，
        // 汇聚与回溯由 advance_join 经 SyncDecisionKind 落库，永远到不了这里。
        // review 的 validate_output 是纯代码判定（approved）。不通过不是"重试"，
        // 而是上交用户裁决：pending(user_decision, context.kind = "review")（决策 2 / 131）。
        (Stage::Review, Node::ValidateOutput) => {
            if ctx.metadata.passed {
                EdgeKind::Next
            } else {
                EdgeKind::Pending(PendingKind::UserDecision)
            }
        }
        (_, Node::ValidateInput) => route_by_readiness(cursor.stage, ctx.metadata.readiness),
        (_, Node::ValidateOutput) => {
            // 通过 → 直接进入下一阶段；不通过才走重试 / pending 判定。
            if ctx.metadata.passed {
                EdgeKind::Next
            } else {
                route_after_validate_output(cursor.validate_attempts, ctx.validate_retry_max)
            }
        }
        _ => EdgeKind::Next,
    }
}

/// agent 型 validate_output 判定不通过后的路由（决策 82）。
///
/// 按**游标自身**的 attempts 判定，不受其他分支影响。能进到这里说明两侧一致不合格
/// （决策 134）——分歧路径在节点内直接进 pending，不经过本函数。
pub fn route_after_validate_output(attempts: u32, validate_retry_max: u32) -> EdgeKind {
    if attempts >= validate_retry_max {
        EdgeKind::Pending(PendingKind::RetryExhausted)
    } else {
        // 重试 execute
        EdgeKind::Retry
    }
}

/// validate_input 的 pending 类型按阶段定死（决策 94）。
pub fn route_by_readiness(stage: Stage, readiness: bool) -> EdgeKind {
    if readiness {
        return EdgeKind::Next;
    }
    match stage {
        Stage::ArchitectDesign => EdgeKind::Pending(PendingKind::InfoInsufficient),
        Stage::DevelopDesign | Stage::TestDesign => EdgeKind::Pending(PendingKind::UserDecision),
        _ => EdgeKind::Pending(PendingKind::UserDecision),
    }
}

/// develop / test 的纯代码 validate_output（决策 62 / 85 / 139）。
///
/// **通过即放行**：attempts 不参与判定（与 agent 型 validate_output 同一条规则，
/// 否则最后一次通过反而进 pending）。
pub fn route_code_gate(cursor: &NodeCursor, ctx: &RouteContext) -> EdgeKind {
    if ctx.metadata.passed {
        return EdgeKind::Next;
    }
    if cursor.validate_attempts >= ctx.validate_retry_max {
        return EdgeKind::Pending(PendingKind::RetryExhausted);
    }
    match cursor.stage {
        // test：全部失败是 test_issue → 自己修用例；存在 code_issue → 交用户决定
        Stage::Test if !ctx.metadata.all_failures_are_test_issues() => {
            EdgeKind::Pending(PendingKind::UserDecision)
        }
        // develop：lint / 单元测试 / 环境类失败 → 直接打回 execute（同属「确定性失败
        // 不绕 test」那一族，决策 139 / 391 / 392；develop 本就没有 test 复检这一步）
        _ => EdgeKind::Retry,
    }
}

/// merge.execute 的流转判定（决策 85 / 86 / 95 / 108 / 121 / 139）。
///
/// 闸门结果与审批状态**正交**：先看 gate，再看 approval。
pub fn route_merge(ctx: &RouteContext) -> EdgeKind {
    let merge = &ctx.merge;

    // ① 仍在等审批：不推进、不重入（决策 95）。原先的 `=> Next` 会把未审批的 merge 送进 done。
    if merge.approval == Approval::Pending {
        return EdgeKind::NoOp;
    }

    // ② 闸门未跑：阶段 A 中途 / 元数据残缺。不推进，**绝不当作通过**（Gate 无 Default）。
    let Some(gate) = merge.gate else {
        return EdgeKind::NoOp;
    };

    // ③ 闸门失败次数耗尽 → pending(retry_exhausted)（决策 85 / 108）。
    //    没有这个收口，merge ↔ test / develop 会无限循环。
    if gate == Gate::Fail && merge.gate_failures >= ctx.validate_retry_max {
        return EdgeKind::Pending(PendingKind::RetryExhausted);
    }

    // ④ 闸门失败分流（决策 139）：lint 失败确定性，直接打回 develop；
    //    空分支同样是确定性失败（决策 391：修用例造不出提交，test 复检必然空转）；
    //    环境类失败同理（决策 392：用例侧造不出工具链）——三者都直接打回 develop，
    //    决策 85 的 test 复检只留给真正的用例争议。
    //    测试失败需要 agent 分辨 test_issue / code_issue，跳回 test.execute（决策 85）。
    if gate == Gate::Fail {
        return match merge.gate_failure_kind {
            Some(GateFailureKind::Lint)
            | Some(GateFailureKind::EmptyBranch)
            | Some(GateFailureKind::Environment) => EdgeKind::KickbackDevelop,
            Some(GateFailureKind::Test) | None => EdgeKind::GotoTest,
        };
    }

    match merge.approval {
        // 批准 → 阶段 B 已在 execute 内完成合入 → done
        Approval::Approved => EdgeKind::Next,
        // None / Returned 均不经过路由（决策 121）：None 的唯一出口是阶段 A 末尾的
        // pending(merge_approval)，"返回修改"由 merge/decision 端点直接置游标。
        Approval::None | Approval::Returned => EdgeKind::NoOp,
        Approval::Pending => unreachable!("上面已提前返回"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    fn cursor(stage: Stage, node: Node, attempts: u32) -> NodeCursor {
        let now = chrono::Utc::now();
        NodeCursor {
            cursor_id: "c1".into(),
            task_id: "t1".into(),
            branch: NodeCursor::BRANCH_MAIN.into(),
            stage,
            node,
            status: CursorStatus::Active,
            validate_attempts: attempts,
            skipped_to_join: false,
            pending_reason: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn ctx_with(merge: MergeResult) -> RouteContext {
        RouteContext {
            validate_retry_max: 3,
            metadata: MetadataView::default(),
            merge,
        }
    }

    /// 文档必填字段齐全的"空"merge metadata（gate=None = 闸门未跑）。
    fn plain_merge() -> MergeResult {
        MergeResult {
            diff_path: "merge-proposal.diff".into(),
            diff_stats: DiffStats {
                files_changed: 0,
                insertions: 0,
                deletions: 0,
                file_details: Vec::new(),
            },
            base_commit: "basesha".into(),
            gate: None,
            gate_failure_kind: None,
            gate_failures: 0,
            gate_failure_output: None,
            conflict_files: Vec::new(),
            approval: Approval::None,
            status: MergeStatus::PendingApproval,
            push_after_merge: false,
        }
    }

    fn merge(approval: Approval, gate: Option<Gate>, kind: Option<GateFailureKind>) -> MergeResult {
        MergeResult {
            approval,
            gate,
            gate_failure_kind: kind,
            ..plain_merge()
        }
    }

    // ── route_merge：全 EdgeKind 逐分支（testing.md §5）──

    #[test]
    fn route_merge_approval_pending_is_noop() {
        let ctx = ctx_with(merge(Approval::Pending, Some(Gate::Pass), None));
        assert_eq!(route_merge(&ctx), EdgeKind::NoOp);
    }

    #[test]
    fn route_merge_approval_none_is_noop_not_kickback() {
        // 决策 121 的回归：首次进入 merge（approval=none）不得被打回 develop。
        let ctx = ctx_with(merge(Approval::None, Some(Gate::Pass), None));
        assert_eq!(route_merge(&ctx), EdgeKind::NoOp);
    }

    #[test]
    fn route_merge_approval_returned_is_noop() {
        let ctx = ctx_with(merge(Approval::Returned, Some(Gate::Pass), None));
        assert_eq!(route_merge(&ctx), EdgeKind::NoOp);
    }

    #[test]
    fn route_merge_approved_goes_next() {
        let ctx = ctx_with(merge(Approval::Approved, Some(Gate::Pass), None));
        assert_eq!(route_merge(&ctx), EdgeKind::Next);
    }

    #[test]
    fn route_merge_gate_fail_lint_kicks_back_develop() {
        // 决策 139：lint 失败不经 test.execute
        let ctx = ctx_with(merge(
            Approval::Approved,
            Some(Gate::Fail),
            Some(GateFailureKind::Lint),
        ));
        assert_eq!(route_merge(&ctx), EdgeKind::KickbackDevelop);
    }

    #[test]
    fn route_merge_gate_fail_empty_branch_kicks_back_develop_not_test() {
        // 决策 391：空分支是确定性失败（修用例造不出分支提交），直接回 develop.execute，
        // 与决策 139 的 lint 同款——不走决策 85 的 test 复检。
        let ctx = ctx_with(merge(
            Approval::None,
            Some(Gate::Fail),
            Some(GateFailureKind::EmptyBranch),
        ));
        assert_eq!(route_merge(&ctx), EdgeKind::KickbackDevelop);
    }

    #[test]
    fn route_merge_gate_fail_test_goes_to_test() {
        let ctx = ctx_with(merge(
            Approval::None,
            Some(Gate::Fail),
            Some(GateFailureKind::Test),
        ));
        assert_eq!(route_merge(&ctx), EdgeKind::GotoTest);
    }

    #[test]
    fn route_merge_gate_fail_environment_kicks_back_develop_not_test() {
        // 决策 392：环境类失败是确定性失败（用例侧造不出工具链），直接回 develop.execute——
        // 与 lint / 空分支同款，不走决策 85 的 test 复检（那会让 test 侧永远报绿空转）。
        let ctx = ctx_with(merge(
            Approval::None,
            Some(Gate::Fail),
            Some(GateFailureKind::Environment),
        ));
        assert_eq!(route_merge(&ctx), EdgeKind::KickbackDevelop);
    }

    #[test]
    fn gate_failure_kind_round_trips_through_as_str() {
        // 落库键与读回同一个单点（决策 392：别各写一份拼法）
        for kind in [
            GateFailureKind::Lint,
            GateFailureKind::Test,
            GateFailureKind::EmptyBranch,
            GateFailureKind::Environment,
        ] {
            assert_eq!(GateFailureKind::from_str_opt(kind.as_str()), Some(kind));
        }
        assert_eq!(GateFailureKind::from_str_opt("who_knows"), None);
    }

    #[test]
    fn route_merge_gate_fail_without_kind_defaults_to_test() {
        let ctx = ctx_with(merge(Approval::None, Some(Gate::Fail), None));
        assert_eq!(route_merge(&ctx), EdgeKind::GotoTest);
    }

    #[test]
    fn route_merge_gate_and_approval_are_orthogonal() {
        // 决策 95：闸门失败优先于审批状态——"已批准但闸门失败"仍打回。
        let ctx = ctx_with(merge(
            Approval::Approved,
            Some(Gate::Fail),
            Some(GateFailureKind::Test),
        ));
        assert_eq!(route_merge(&ctx), EdgeKind::GotoTest);
    }

    #[test]
    fn route_merge_pending_approval_beats_gate_fail() {
        // approval=pending 时不得重入，即使 gate=fail 也保持 NoOp（等用户审批）
        let ctx = ctx_with(merge(
            Approval::Pending,
            Some(Gate::Fail),
            Some(GateFailureKind::Test),
        ));
        assert_eq!(route_merge(&ctx), EdgeKind::NoOp);
    }

    #[test]
    fn route_merge_gate_unset_never_advances() {
        // Gate 无 Default：闸门没跑 ≠ 闸门通过（否则残缺 metadata 会被当成已过闸门）。
        let ctx = ctx_with(merge(Approval::None, None, None));
        assert_eq!(route_merge(&ctx), EdgeKind::NoOp);

        // 即使"已批准"，只要闸门没跑过就不许进 done。
        let ctx = ctx_with(merge(Approval::Approved, None, None));
        assert_eq!(route_merge(&ctx), EdgeKind::NoOp);
    }

    #[test]
    fn route_merge_gate_failure_exhaustion_pends() {
        // 决策 85 / 108：gate_failures 累到上限必须收口，否则 merge ↔ test 无限循环。
        let mut m = merge(
            Approval::None,
            Some(Gate::Fail),
            Some(GateFailureKind::Test),
        );
        m.gate_failures = 3;
        assert_eq!(
            route_merge(&ctx_with(m.clone())),
            EdgeKind::Pending(PendingKind::RetryExhausted)
        );
        // 未到上限仍按失败类型分流
        m.gate_failures = 2;
        assert_eq!(route_merge(&ctx_with(m)), EdgeKind::GotoTest);
    }

    // ── review 判定 ──

    #[test]
    fn review_failure_pends_for_user_not_retry() {
        // 决策 2 / 131：review 不通过 → user_decision(kind=review)，不是节点重试，也不看 attempts。
        let c = cursor(Stage::Review, Node::ValidateOutput, 3);
        let ctx = RouteContext {
            validate_retry_max: 3,
            metadata: MetadataView::passed(false),
            merge: plain_merge(),
        };
        assert_eq!(
            route(&c, &ctx),
            EdgeKind::Pending(PendingKind::UserDecision)
        );
    }

    // ── route_after_validate_output：attempts 边界 ──

    #[test]
    fn attempts_boundary_is_inclusive_at_max() {
        assert_eq!(route_after_validate_output(0, 3), EdgeKind::Retry);
        assert_eq!(route_after_validate_output(1, 3), EdgeKind::Retry);
        assert_eq!(route_after_validate_output(2, 3), EdgeKind::Retry);
        assert_eq!(
            route_after_validate_output(3, 3),
            EdgeKind::Pending(PendingKind::RetryExhausted)
        );
        assert_eq!(
            route_after_validate_output(4, 3),
            EdgeKind::Pending(PendingKind::RetryExhausted)
        );
    }

    #[test]
    fn passed_validate_output_goes_next_even_at_attempt_limit() {
        // 通过即放行，attempts 不参与判定
        let c = cursor(Stage::ArchitectDesign, Node::ValidateOutput, 3);
        let ctx = RouteContext {
            validate_retry_max: 3,
            metadata: MetadataView::passed(true),
            merge: plain_merge(),
        };
        assert_eq!(route(&c, &ctx), EdgeKind::Next);
    }

    #[test]
    fn failed_validate_output_retries_then_pends() {
        let mut c = cursor(Stage::ArchitectDesign, Node::ValidateOutput, 1);
        let ctx = RouteContext {
            validate_retry_max: 3,
            metadata: MetadataView::passed(false),
            merge: plain_merge(),
        };
        assert_eq!(route(&c, &ctx), EdgeKind::Retry);
        c.validate_attempts = 3;
        assert_eq!(
            route(&c, &ctx),
            EdgeKind::Pending(PendingKind::RetryExhausted)
        );
    }

    #[test]
    fn cross_family_disagreement_never_enters_route() {
        // 决策 134 / 135 的注释性断言：分歧路径在节点内直接置 pending，不是 EdgeKind。
        assert_eq!(
            resolve_validate_output(true, false, Some(true)),
            ValidateOutcome::JudgeDisagreement
        );
        // 一致不合格 → 正常走路由
        assert_eq!(
            resolve_validate_output(true, false, Some(false)),
            ValidateOutcome::Failed
        );
        // 开关关闭 → 不看复判
        assert_eq!(
            resolve_validate_output(false, false, Some(true)),
            ValidateOutcome::Failed
        );
        // 通过 → 直接放行
        assert_eq!(
            resolve_validate_output(true, true, None),
            ValidateOutcome::Passed
        );
    }

    // ── route_by_readiness：决策 94 ──

    #[test]
    fn readiness_routes_by_stage() {
        assert_eq!(
            route_by_readiness(Stage::ArchitectDesign, true),
            EdgeKind::Next
        );
        assert_eq!(
            route_by_readiness(Stage::ArchitectDesign, false),
            EdgeKind::Pending(PendingKind::InfoInsufficient)
        );
        assert_eq!(
            route_by_readiness(Stage::DevelopDesign, false),
            EdgeKind::Pending(PendingKind::UserDecision)
        );
        assert_eq!(
            route_by_readiness(Stage::TestDesign, false),
            EdgeKind::Pending(PendingKind::UserDecision)
        );
        // 其余阶段兜底为 user_decision（路由不返回裸 Pending）
        assert_eq!(
            route_by_readiness(Stage::Develop, false),
            EdgeKind::Pending(PendingKind::UserDecision)
        );
    }

    // ── route_code_gate：决策 62 / 85 ──

    #[test]
    fn test_code_gate_all_test_issue_retries() {
        let c = cursor(Stage::Test, Node::ValidateOutput, 0);
        let ctx = RouteContext {
            validate_retry_max: 3,
            metadata: MetadataView {
                passed: false,
                failures: vec![TestFailure {
                    test_name: "t".into(),
                    error_message: "e".into(),
                    failure_cause: FailureCause::TestIssue,
                }],
                ..Default::default()
            },
            merge: plain_merge(),
        };
        assert_eq!(route_code_gate(&c, &ctx), EdgeKind::Retry);
    }

    #[test]
    fn test_code_gate_code_issue_pends_for_user() {
        let c = cursor(Stage::Test, Node::ValidateOutput, 0);
        let ctx = RouteContext {
            validate_retry_max: 3,
            metadata: MetadataView {
                passed: false,
                failures: vec![TestFailure {
                    test_name: "t".into(),
                    error_message: "e".into(),
                    failure_cause: FailureCause::CodeIssue,
                }],
                ..Default::default()
            },
            merge: plain_merge(),
        };
        assert_eq!(
            route_code_gate(&c, &ctx),
            EdgeKind::Pending(PendingKind::UserDecision)
        );
    }

    #[test]
    fn test_code_gate_exhausted_pends_regardless_of_cause() {
        let c = cursor(Stage::Test, Node::ValidateOutput, 3);
        let ctx = RouteContext {
            validate_retry_max: 3,
            metadata: MetadataView::passed(false),
            merge: plain_merge(),
        };
        assert_eq!(
            route_code_gate(&c, &ctx),
            EdgeKind::Pending(PendingKind::RetryExhausted)
        );
    }

    #[test]
    fn code_gate_passed_at_attempt_limit_goes_next() {
        // 与 agent 型路径同规则：通过即放行，attempts 不参与判定。
        let c = cursor(Stage::Develop, Node::ValidateOutput, 3);
        let ctx = RouteContext {
            validate_retry_max: 3,
            metadata: MetadataView::passed(true),
            merge: plain_merge(),
        };
        assert_eq!(route_code_gate(&c, &ctx), EdgeKind::Next);
    }

    #[test]
    fn develop_code_gate_passes_or_retries() {
        let c = cursor(Stage::Develop, Node::ValidateOutput, 0);
        let mut ctx = RouteContext {
            validate_retry_max: 3,
            metadata: MetadataView::passed(true),
            merge: plain_merge(),
        };
        assert_eq!(route_code_gate(&c, &ctx), EdgeKind::Next);
        // lint / 单元测试失败 → 重试 execute（无 user_decision 分支）
        ctx.metadata = MetadataView::passed(false);
        assert_eq!(route_code_gate(&c, &ctx), EdgeKind::Retry);
    }

    #[test]
    fn develop_validate_output_declared_zero_changes_pends_for_user() {
        // 决策 391：execute 申报零变更 → 不走常规放行/重试，挂 user_decision 交用户确认。
        let c = cursor(Stage::Develop, Node::ValidateOutput, 0);
        let mut ctx = RouteContext {
            validate_retry_max: 3,
            metadata: MetadataView::passed(true),
            merge: plain_merge(),
        };
        ctx.metadata.zero_changes = true;
        assert_eq!(
            route(&c, &ctx),
            EdgeKind::Pending(PendingKind::UserDecision),
            "申报零变更即便闸门放行也交用户确认，不直接 Next"
        );
        // 零变更申报优先于闸门失败：即便 passed=false 也不进 Retry（交用户裁量）
        ctx.metadata.passed = false;
        assert_eq!(
            route(&c, &ctx),
            EdgeKind::Pending(PendingKind::UserDecision)
        );
    }

    #[test]
    fn worktree_dirty_kicks_back_to_execute_not_user_decision() {
        // 决策 416 A：工作区守卫打回是**确定性 Retry**，两个代码门都不得落进
        // user_decision——test 侧 failures 为空时 `all_failures_are_test_issues()` 恒假，
        // 没有这条短路就会误挂 pending(user_decision, test_code_issue) 让用户拍板。
        for stage in [Stage::Develop, Stage::Test] {
            let c = cursor(stage, Node::ValidateOutput, 0);
            let mut ctx = RouteContext {
                validate_retry_max: 3,
                metadata: MetadataView::passed(false),
                merge: plain_merge(),
            };
            ctx.metadata.worktree_dirty = true;
            assert_eq!(
                route(&c, &ctx),
                EdgeKind::Retry,
                "{stage}: 脏工作区必须打回 execute，不得 pending"
            );
        }

        // 打回同样吃 attempts 上限：耗尽后收口 retry_exhausted，不无限打回。
        let c = cursor(Stage::Test, Node::ValidateOutput, 3);
        let mut ctx = RouteContext {
            validate_retry_max: 3,
            metadata: MetadataView::passed(false),
            merge: plain_merge(),
        };
        ctx.metadata.worktree_dirty = true;
        assert_eq!(
            route(&c, &ctx),
            EdgeKind::Pending(PendingKind::RetryExhausted)
        );

        // 零变更申报优先于工作区守卫（决策 391 的用例不受 413 扰动）：申报成立时
        // 收尾语义独立于工作区状态，仍交用户确认。
        let c = cursor(Stage::Develop, Node::ValidateOutput, 0);
        let mut ctx = RouteContext {
            validate_retry_max: 3,
            metadata: MetadataView::passed(false),
            merge: plain_merge(),
        };
        ctx.metadata.worktree_dirty = true;
        ctx.metadata.zero_changes = true;
        assert_eq!(
            route(&c, &ctx),
            EdgeKind::Pending(PendingKind::UserDecision)
        );
    }

    #[test]
    fn route_dispatches_by_stage_and_node() {
        // merge.execute 走 route_merge
        let c = cursor(Stage::Merge, Node::Execute, 0);
        let ctx = ctx_with(merge(Approval::Approved, Some(Gate::Pass), None));
        assert_eq!(route(&c, &ctx), EdgeKind::Next);

        // 普通 execute 节点继续流转
        let c = cursor(Stage::Develop, Node::Execute, 0);
        assert_eq!(route(&c, &ctx), EdgeKind::Next);

        // validate_input 走 readiness
        let c = cursor(Stage::ArchitectDesign, Node::ValidateInput, 0);
        let mut ctx2 = ctx.clone();
        ctx2.metadata = MetadataView::readiness(true);
        assert_eq!(route(&c, &ctx2), EdgeKind::Next);
    }
}
