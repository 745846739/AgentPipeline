# 14: 修正 attempt 语义（`agent_type` 过滤）

**What to build:** `next_attempt` 统计 `(task, stage, node)` 的 run 行数时**不过滤 `agent_type`**，
而伪阶段（`conflict_check` / `validator_cross_check`）的 run 行复用父节点的 stage / node——
于是 `attempt` 会被伪阶段虚增。

这是一个**既存 bug**，不是本功能引入的问题；但它会让票 13 的续接读到伪阶段的会话，故必须先修。

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

- [ ] `next_attempt` 加 `agent_type = 'main'`（或等价）过滤，使 `attempt` 只统计真实 agent 尝试
- [ ] 子代理（`agent_type = "subagent"`，票 08）同样不计入父节点的 `attempt`
- [ ] 受影响指标的语义复核：重试率（`attempt > 1`）与一次通过率（`attempt == 1`）——
      修正后数值会变，确认这是期望的（伪阶段不该被算成一次重试）
- [ ] 用例：伪阶段触发后 `attempt` 不虚增 / 重试率指标反映真实重试次数
- [ ] 既有测试全绿（若有测试依赖旧的虚增数值，一并修正并说明）

**Notes（实现提示）:**
- 修正方向明确（过滤 `agent_type`），但须核对所有读 `attempt` 的地方：run 行、会话行、
  指标聚合三处都要一致，否则会出现「会话行的 attempt」与「run 行的 attempt」错位。
- 本票**独立可动**（不依赖任何前置），且是票 13 的隐含前置——票 13 在开工前应确认本票已合入，
  否则续接的「读上一轮」会读错行。
