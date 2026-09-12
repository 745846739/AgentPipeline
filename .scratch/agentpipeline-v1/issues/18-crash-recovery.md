# 18: 崩溃恢复端到端

**What to build:** 进程中断恢复的全链路验收：kill -9 后重启，从游标 checkpoint 恢复到各分支中断节点继续执行；并行任务两条游标独立恢复；waiting_join 保留等待；SIGINT 在节点边界退出。

**Blocked by:** 17（生产接线）

**Status:** done

完成记录：新增 `tests/e2e/tests/crash_recovery.rs`（E2E-13，全 in-process，决策 152）：① `interrupted_node_resumes_after_restart_and_owner_cleanup`（develop.execute 中途 abort → 游标停中断节点 → `clear_executor_owners` 后可再 claim → 续跑至 done，上游节点不重跑、design_doc 产出唯一）；② `parallel_branches_recover_independently_and_join_survives_restart`（两分支各自中断/恢复，join 恰一次）；③ `waiting_join_survives_restart_and_join_advances_when_sibling_ready`（waiting_join 跨重启保留，另一分支就位后 join 推进）。遗留：SIGINT 信号本身按决策 152 仍不自动化（留手动）。

- [ ] kill -9 注入测试（testkit killer 思路扩展到进程级）：重启后任务从各游标中断节点续跑至终态
- [ ] 并行任务：两游标独立恢复，互不影响（决策 80）
- [ ] waiting_join 状态跨重启保留，另一分支就位后 join 正常推进
- [ ] executor_owner 残留清理 + 重新准入（决策 127）在真实重启路径验证
- [ ] 节点级幂等重跑不产生重复产出（G8/G9）
