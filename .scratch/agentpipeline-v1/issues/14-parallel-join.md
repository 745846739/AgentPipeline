# 14: 并行分支执行与 sync-check 汇聚

**What to build:** develop-design 与 test-design 双游标并发执行，以及汇聚节点：两分支互不阻塞、都到边界后 sync-check 恰好执行一次（纯代码，agent_type="system"，不占游标行），计算 SyncDecision 落库并按 proceed/backtrack 路由；消费 skipped_to_join 标志。

**Blocked by:** 11（执行器骨架）

**Status:** ready-for-agent

- [ ] buffer_unordered 并发驱动多游标；一条分支 pending 另一条照常跑完本阶段（决策 81/82/89）
- [ ] advance_join：全部 waiting_join 且无 pending 才执行、恰好一次、事务内归档分支游标插回 main（决策 83/107/113）
- [ ] sync-check 落 node_run（agent_type="system"，token 0，无会话行；决策 99/114）
- [ ] SyncDecision 计算 + backtrack 边真正可达（G5）
- [ ] skipped_to_join 消费：对应分支视 readiness=true，空产出降级语义写进 develop/test 的 prompt（决策 93/115）
- [ ] branch 字段贯穿 SSE 与时间线（决策 84）；焦点投影正确反映并行区间
