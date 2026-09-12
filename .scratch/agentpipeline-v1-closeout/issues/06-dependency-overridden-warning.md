# 06: dependency_overridden 警告

**What to build:** `dependency_failed` 待办选「继续执行」时，决策 116 要求记为**忽略失败依赖**并落 `dependency_overridden` 警告。当前行为只清 pending、把任务置回 queued 交还准入——流转语义是对的（决策 130 ⑤），但警告无生产者，事后无法从审计面看出这个任务是踩着失败依赖上路的。补上警告后，风险行为在观测面可见。

**Blocked by:** None (can start immediately)

**Status:** done

- [ ] `dependency_failed` 的 continue 在流转记录（或等价观测面）留下 `dependency_overridden` 警告，含被忽略的依赖任务 id
- [ ] 任务置回 queued 交还准入、不直接 spawn 的既有语义不变（决策 130 ⑤）
- [ ] 按依赖终态裁剪的动作集不变（cancelled 无「等待依赖重试」）
- [ ] 用例覆盖：continue 后警告可查且内容正确；cancelled 分支同样落警告
