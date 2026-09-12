# 18: 崩溃恢复端到端

**What to build:** 进程中断恢复的全链路验收：kill -9 后重启，从游标 checkpoint 恢复到各分支中断节点继续执行；并行任务两条游标独立恢复；waiting_join 保留等待；SIGINT 在节点边界退出。

**Blocked by:** 17（生产接线）

**Status:** ready-for-agent

- [ ] kill -9 注入测试（testkit killer 思路扩展到进程级）：重启后任务从各游标中断节点续跑至终态
- [ ] 并行任务：两游标独立恢复，互不影响（决策 80）
- [ ] waiting_join 状态跨重启保留，另一分支就位后 join 正常推进
- [ ] executor_owner 残留清理 + 重新准入（决策 127）在真实重启路径验证
- [ ] 节点级幂等重跑不产生重复产出（G8/G9）
