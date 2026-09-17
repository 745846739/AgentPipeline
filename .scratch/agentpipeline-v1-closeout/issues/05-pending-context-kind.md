# 05: pending 补 context.kind

**What to build:** review 打回与 test 的 code_issue 两条待办目前经通用边构造，丢了 `context.kind`，于是 `allowed_actions` 落到「(user_decision, _) → 跳过当前阶段 / 取消任务」的通用兜底行——权威总表里为它们写好的专用行（决策 130 ①）形同虚设。补上生产者：review 打回带 `review`，test 闸门失败带 `test_code_issue` / `gate_recheck`。完成后 review 打回的任务卡片出现「打回开发修复 / 强制通过评审」，test 代码问题出现「修改测试用例 / 修改业务代码」，而不是无意义的跳过。

**Blocked by:** None (can start immediately)

**Status:** done

- [x] review 打回 pending 带 `context.kind = review`，动作集为「打回开发修复（goto develop.execute）/ 强制通过评审（skip）」
- [x] test 闸门 code_issue pending 带 `context.kind = test_code_issue` / `gate_recheck`，动作集为「修改测试用例 / 修改业务代码」
- [x] 两条路径不再落通用兜底行；权威表既有行不改语义
- [x] E2E-03 / E2E-06b 断言 `context.kind` 与动作集（复用票 01 的助手）
- [x] 前端纯渲染，无需改动；若发现渲染依赖通用行则一并核对
