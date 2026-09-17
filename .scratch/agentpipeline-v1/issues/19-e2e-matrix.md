# 19: E2E 用例矩阵补全

**What to build:** testing.md §8 的 25 场景矩阵目前只覆盖 4 个（happy path、E2E-08 retry 半段、E2E-09）；补齐其余 22 个 FakeAgent 驱动场景，并补 testkit 缺口（fail_tool_n「第 N 次某工具失败」、超长工具结果注入）。执行器循环自身的 §6 用例（pending 出 runnable、join 恰一次、单游标失败隔离）一并落地。

**Blocked by:** 14, 15, 16, 17（场景依赖并行/闸门/伪阶段/接线的真实路径）

**Status:** done（E2E-13 归票 18；本票暴露的两个生产 bug 已由编排侧修复并强化用例钉住，见文末「生产 gap」——4/5 已修复，其余为文档已定义的既有缺口）

## 场景清单

- [x] E2E-01 happy path —— 已有（`tests/e2e/tests/happy_path.rs`，不改）
- [x] E2E-02 sync-check backtrack —— 已有（`join_and_skip.rs`，不改）
- [~] E2E-03 review 打回循环 —— `reviews.rs::e2e_03_review_rejection_pends_then_goto_develop_and_re_review_passes`
      部分：pending → goto develop → re-review 通过已断言；`required_changes` 注入 develop prompt 无生产者（gap 1）
- [~] E2E-04 human review —— `reviews.rs::e2e_04_human_review_pends_then_reject_goes_to_develop` / `..._approve_goes_to_test`
      部分：pending / approve / reject 路由 / comments 进流转原因已断言；`review-diff.diff` 无生产者（gap 2）
- [x] E2E-05 merge 冲突打回 —— `conflicts.rs::e2e_05_merge_rebase_conflict_kicks_back_develop_with_conflict_files`
- [x] E2E-06a 闸门测试失败 → 全 test_issue —— `gates.rs::e2e_06a_merge_test_gate_failure_rechecks_via_test_then_passes`
- [x] E2E-06b 闸门失败 → code_issue —— `gates.rs::e2e_06b_gate_failure_code_issue_pends_for_user_then_goto_develop`
      （注：pending 无 `context.kind=test_code_issue`，见 gap 3）
- [x] E2E-07 闸门 lint 失败 —— `gates.rs::e2e_07_merge_lint_gate_failure_kicks_back_develop_without_test`
- [x] E2E-08 耗尽 → failed → retry —— `gates.rs::e2e_08_exhausts_gate_failures_then_retry_resets_worktree_and_readmits`
- [x] E2E-09 基准前移 —— 已有（`happy_path.rs`，不改）
- [x] E2E-10 脏工作区合入 —— `conflicts.rs::e2e_10_dirty_worktree_pends_for_user_and_blocks_merge`
      pending/动作集/不合入 + **挂起后游标仍停 merge.execute** + **continue → 工作区恢复干净后完成合入到 done**
- [x] E2E-11 skip 落点矩阵 —— 已有（`join_and_skip.rs`，不改）
- [x] E2E-12 并行互不阻塞 —— 已有（`join_and_skip.rs` + core `one_branch_pending_does_not_stop_the_other`）
- [x] E2E-13 中断恢复 —— **归票 18**（`tests/e2e/tests/crash_recovery.rs`），本票不碰
- [x] E2E-14 超时链 —— `timeouts.rs::e2e_14_timeout_chain_kills_retries_then_pends_and_merge_has_no_skip` / `..._long_system_command_heartbeat_survives_idle_timeout`
- [x] E2E-15 judge_disagreement —— `pending.rs::e2e_15_judge_disagreement_pends_then_continue_advances_without_rerun` / `..._goto_execute_increments_attempts`
- [x] E2E-16 design_refs 完整性 —— `pending.rs::e2e_16_high_dangling_design_ref_is_blocker_and_backtracks` / `..._medium_..._only_a_warning`
- [x] E2E-17 conflict_wait —— `conflicts.rs::e2e_17_conflict_wait_yields_then_auto_recovers_when_all_terminal` + `e2e_17_pure_name_overlap_is_only_a_warning`
      存储层 warning + **执行器不因纯 name 重合挂 conflict_wait（architect.execute 不停住）**
- [x] E2E-18 duplicate_risk —— `conflicts.rs::e2e_18_semantic_duplicate_risk_pends_with_goto_and_cancel`
- [~] E2E-19 依赖三态 —— `deps.rs::e2e_19_*`（3 条）
      部分：waiting → queued → running / dep failed / dep retry 已断言；`continue` 不记 `dependency_overridden`（gap 6）
- [x] E2E-20 并发准入 —— `deps.rs::e2e_20_concurrent_admission_respects_max_and_slot_release`
- [~] E2E-21 info_insufficient —— `pending.rs::e2e_21_info_insufficient_requires_input_then_reruns_validate_input`
      部分：requires_input 动作 + 重跑 validate_input 已断言；补充输入未注入重入 prompt（gap 7）
- [~] E2E-22 context_overflow —— `pending.rs::e2e_22_context_overflow_actions_all_have_paired_endpoints`
      动作集/端点配对已断言；L4 无生产者（gap 8），用例直接构造 pending
- [~] E2E-23 取消传播 —— core 部分 `deps.rs::e2e_23_cancel_propagates_dependency_failed_to_unstarted_dependents`；
      worktree 清理 / 分支删除 / SSE `task_cancelled` 在 app 层（`crates/app/src/routes/tasks.rs`），由 L3 API 契约覆盖
- [x] E2E-24 resume 防连点 —— `timeouts.rs::e2e_24_resume_cooldown_detected_and_single_executor_guard`
      （注：cooldown 409 在 app 层，见 gap 9）

## testkit 缺口

- [x] `Script::fail_tool_n`（第 N 次某工具失败）+ `NodeScript::fail_tool_n`；
      测试 `crates/testkit/src/script.rs::fail_tool_n_fails_only_the_nth_call_of_that_tool` / `..._is_scoped_to_stage_node_and_tool`
- [x] `NodeScript::long_tool_result`（超长结果注入，真实跑 `seq 1 N`）；测试 `...::long_tool_result_emits_real_seq_command`
- [x] 集成验证：core `executor.rs::tool_failure_within_budget_does_not_retry_the_node`（G13）、`long_tool_result_triggers_l1_trim`、`long_tool_result_triggers_l2_offload_to_disk`
- [x] testkit 新增 `backdate_run`（超时链假时钟回拨），`crates/testkit/src/assertions.rs`

## §6 执行器循环

- [x] pending 不阻塞他分支 —— 已有 `crates/core/tests/executor.rs::one_branch_pending_does_not_stop_the_other`
- [x] 单游标节点失败隔离（决策 89） —— `executor.rs::single_branch_node_failure_is_isolated_to_that_cursor`
- [x] join 恰执行一次（决策 107） —— `executor.rs::advance_join_runs_exactly_once_even_across_repeated_runs`

## 配置 fail-fast

- [x] `cross_family_judge=true` 无 provider → 拒绝启动（决策 134） —— `executor.rs::cross_family_judge_without_provider_refuses_startup`
- [x] 引用不存在的 skill → 拒绝启动（决策 47） —— `executor.rs::referenced_missing_skill_refuses_startup`

## 生产 gap（报告，不修）

**本票暴露、已修复（编排侧，2026-09-12）：**

4. ✅ **脏工作区 pending 后游标仍被推进到 `done.execute`**：`merge_phase_b_inner` 置 pending 后返回 `Ok(())`，
   `merge_execute` 当作 Route(default)，`route_merge` 见 approval=approved 返回 Next。修复：改为返回 `PhaseB::DirtyWorktree(reason)` 并由 `merge_phase_b` 出口 `NodeOutput::Pending`，挂起统一经 `advance_cursor`（不推进游标）；用例补 `continue → 合入成功到 done`。
5. ✅ `first_layer_conflicts` 把纯 name 重合的 Low warning 放进返回 Vec，executor 用 `!is_empty()` 判定 → conflict_wait（与决策 71②/120 相悖）。修复：executor 只对 `duplicate_risk == Some(High)` 挂 conflict_wait，Low 仅 `tracing::warn!`；与 scheduler `recheck_first_layer_overlap` 的精确 (module,name) 判定一致。

**既有缺口（文档已定义、当前无生产者，属后续票）：**

1. review 打回后 `required_changes` / 重试摘要未注入 develop prompt（`PromptSegments.retry_feedback` 无生产者）。
2. `review-diff.diff`（决策 124）无生产者，human review 不生成 diff 文件。
3. review / test code_issue 的 pending 由 `EdgeKind::Pending(kind)` 构造，丢 `context.kind`（`review` / `test_code_issue`），
   `allowed_actions` 落到通用 `(user_decision, _)` 行。
6. dependency_failed 的 `continue` 未记 `dependency_overridden` 警告（决策 116）。
7. info_insufficient 的补充输入只落流转原因，未注入 validate_input 重入 prompt（决策 79）。
8. `context_overflow`（L4 兜底）executor 无生产者（testing.md §11.2 已知缺口）。
9. resume cooldown 409 与 cancel 的 worktree/分支清理、SSE `task_cancelled` 只在 app 层（由 L3 契约覆盖）。
