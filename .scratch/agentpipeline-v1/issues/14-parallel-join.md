# 14: 并行分支执行与 sync-check 汇聚

**What to build:** develop-design 与 test-design 双游标并发执行，以及汇聚节点：两分支互不阻塞、都到边界后 sync-check 恰好执行一次（纯代码，agent_type="system"，不占游标行），计算 SyncDecision 落库并按 proceed/backtrack 路由；消费 skipped_to_join 标志。

**Blocked by:** 11（执行器骨架）

**Status:** done（主体已随票 11 的 executor 落地；本次收尾补齐决策 83 / 126 的 backtrack 生产侧与 E2E 场景）

已随票 11 落地并有测试锁定：
- [x] buffer_unordered 并发驱动多游标；一条分支 pending 另一条照常跑完本阶段（决策 81/82/89；L2 `one_branch_pending_does_not_stop_the_other`）
- [x] advance_join：全部 waiting_join 且无 pending 才执行、恰好一次、事务内归档分支游标插回 main（决策 83/107/113）
- [x] sync-check 落 node_run（agent_type="system"，token 0，无会话行；决策 99/114）
- [x] skipped_to_join 消费：对应分支视 readiness=true（决策 93）；空产出降级语义已由票 12 的 §10.3 模板内嵌（develop/test system prompt 显式写明决策 115 降级，review 按决策 133）
- [x] branch 字段贯穿 SSE 与时间线（决策 84）；焦点投影（决策 92/130④）在 L1 有用例

本次收尾（commit 本票）：
- [x] SyncDecision backtrack 边真正可达（G5）+ 决策 83 落地：backtrack 时 `dev-plan.md` / `test-scenarios.md` 标过期（`kanban_stage_outputs.stale` 列，migration 0002；与游标归档同一事务，upsert 覆盖写入时清除，文件保留供回溯）
- [x] 决策 126 落地：architect-design 重入（validate_input / execute）的 user prompt 注入 `backtrack-feedback.md`（`PromptSegments.backtrack_feedback` 生产侧接线；首轮无文件不渲染）
- [x] E2E 场景（`tests/e2e/tests/join_and_skip.rs`，5 例）：E2E-02 backtrack 全链断言（归档 / 标过期 / 反馈文件 / 重入 prompt / attempts 归零）；E2E-11 矩阵（architect skip → 分裂；develop-design skip → `skipped_to_join` + 不伪造产出元数据 + 下游降级；test-design skip 对称）；E2E-12 尾段（resume → join 恰一次）

注记：决策 136 的 design_refs 校验本体已实现于 `compute_sync_decision`（E2E 间接覆盖 proceed/warning 侧），E2E-16 高悬空场景归票 19。
