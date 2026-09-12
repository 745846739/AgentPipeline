# 16: 伪阶段执行（conflict_check / validator_cross_check / project_analysis 摘要）

**What to build:** 三个不占游标的 LLM 伪阶段调用点：architect 产出后的语义冲突第二层比对（命中进 conflict_wait）；agent 型 validate_output 首判不合格时的异族复判（复判合格 → pending(user_decision, judge_disagreement)）；项目分析的 LLM 摘要部分（当前只有纯代码探测）。

**Blocked by:** 13（需要真实 LLM 适配器）、14（复判发生在并行设计阶段）

**Status:** done（三个伪阶段调用点全部落地；`project_analysis` 的 app 路由接线已由编排侧补齐——`POST /projects/analyze` 经注入的 `Executor` 调用摘要并合并进探测结果，L3 用例 `analyze_merges_project_analysis_llm_summary_into_result`）

- [x] conflict_check 语义层：命中 → pending(user_decision, context.kind `duplicate_risk`)，context.conflict_task_ids 记**全部**冲突任务 id（决策 60/67/102/132；按 pipeline-spec §6 第二层与 E2E-18 的既定语义，`duplicate_risk=high` 才上交用户。ticket 文案写的 conflict_wait 与文档不符，以文档为准；第一层 conflict_wait 已随票 11 存在）
- [x] validator_cross_check 复判：首判不合格 + 复判合格 → pending(user_decision, judge_disagreement)（决策 134/135）；garbage/pass 两态由 `resolve_validate_output` 判定
- [x] 决策 135 continue/goto 特判：continue = 用户裁决合格 → 直接放行下一阶段（architect 分裂 / 并行分支到 join 边界 / 串行到入口），不重跑校验；goto execute → 同阶段 execute 且 `validate_attempts + 1`（`storage/decisions.rs::apply_resume`）
- [x] project_analysis LLM 摘要：`Executor::project_analysis(project, facts)` 返回合并 `summary` / `suspicious` 的结果（决策 48/78/130）；app 路由接线已补齐（`analyze` 路由注入执行器；LLM 不可用时保留纯代码事实并记 `summary_error` 降级）
- [x] 伪阶段 run 落库：task 内伪阶段（conflict_check / validator_cross_check）落独立 run + 会话行，`agent_type = pseudo:*`、`parent_run_id` → 父 run、`cursor_id` 继承父游标、心跳写父 run（决策 88/100/113）；project_analysis 为项目级无游标，故不落 run

**测试：** testkit `Script::for_pseudo` / `push_pseudo`（按 `run.agent_type` 路由，testing.md §3.2 ⑥）；L2 `architect_semantic_conflict_check_pends_with_duplicate_risk`、`validator_cross_check_disagreement_pends_and_continue_advances_stage`、`validator_cross_check_disagreement_goto_execute_increments_attempts`、`project_analysis_merges_llm_summary_into_facts`。
