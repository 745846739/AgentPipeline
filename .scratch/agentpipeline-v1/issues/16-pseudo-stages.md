# 16: 伪阶段执行（conflict_check / validator_cross_check / project_analysis 摘要）

**What to build:** 三个不占游标的 LLM 伪阶段调用点：architect 产出后的语义冲突第二层比对（命中进 conflict_wait）；agent 型 validate_output 首判不合格时的异族复判（复判合格 → pending(user_decision, judge_disagreement)）；项目分析的 LLM 摘要部分（当前只有纯代码探测）。

**Blocked by:** 13（需要真实 LLM 适配器）、14（复判发生在并行设计阶段）

**Status:** ready-for-agent

- [ ] conflict_check 语义层：命中 → conflict_wait，context 记全部冲突任务 id（决策 60/67/102；第一层与恢复循环已实现）
- [ ] validator_cross_check 复判：与首判不一致 → pending(judge_disagreement)（决策 134/135；路由与 fail-fast 已实现，缺调用点）
- [ ] project_analysis LLM 摘要并入 analyze 结果（决策 48/130）
- [ ] 伪阶段 run 落库：agent_type/system 归属、cursor 归属按决策 113
