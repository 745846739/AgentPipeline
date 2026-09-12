# 19: E2E 用例矩阵补全

**What to build:** testing.md §8 的 25 场景矩阵目前只覆盖 4 个（happy path、E2E-08 retry 半段、E2E-09）；补齐其余 22 个 FakeAgent 驱动场景，并补 testkit 缺口（fail_tool_n「第 N 次某工具失败」、超长工具结果注入）。执行器循环自身的 §6 用例（pending 出 runnable、join 恰一次、单游标失败隔离）一并落地。

**Blocked by:** 14, 15, 16, 17（场景依赖并行/闸门/伪阶段/接线的真实路径）

**Status:** ready-for-agent

- [ ] 22 个缺失 E2E 场景落地（backtrack、review 打回、gate 循环、conflict_wait、超时链、cancel 级联、split 等）
- [ ] testkit：Script::fail_tool_n（第 N 次失败）、超长工具结果注入 helper
- [ ] §6 执行器循环用例：pending 不阻塞他分支、advance_join 恰好一次、单游标失败隔离（决策 89/107）
- [ ] 配置 fail-fast 用例：cross_family_judge 无 provider、skill 不存在（决策 47/134）
