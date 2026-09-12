# 08: KanbanScheduler 六职责 tick

**What to build:** 图外定时职责：超时检测（空闲 + 绝对上限，分层 node > stage > global，杀进程组后干净重试或 pending(timeout)）、conflict_wait 恢复（全终态 + 重跑第一层比对）、依赖启动与 dependency_failed 恢复、并发准入（名额 = running + pending）、pending 提醒与 stalled 标记（按游标判定谓词）、小时级 maintenance。

**Blocked by:** None（can start immediately）

**Status:** done（已实现；⚠️ 核心库完成但未接进应用进程——接线归票 17）

- [x] 六职责 tick + maintenance 全部实现，scheduler_tick 套件 18 例锁定
- [x] 谓词按游标判定：has_runnable_cursor / has_pending_cursor（决策 92/98）
- [x] 准入名额 = status ∈ {running, pending}（决策 117）
- [x] 超时时钟取 started_at，心跳刷新 last_activity_at（决策 64/66）
- [x] conflict 恢复：全终态 → 重查交集 → 清 pending 或更新 context（决策 102）
