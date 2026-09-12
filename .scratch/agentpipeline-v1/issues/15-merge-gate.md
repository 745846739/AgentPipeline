# 15: merge 阶段执行与闸门

**What to build:** merge 的两阶段编排：阶段 A——rebase 到 default_branch（冲突打回 develop.execute）、执行 lint/test 闸门并把结果写进 merge_result.gate、产出 proposal + diff、进入 pending(merge_approval)；阶段 B——审批通过后执行合入。路由函数已完整（03 票），本票交付让 route_merge 的每个 EdgeKind 真正可达。

**Blocked by:** 11（执行器骨架）

**Status:** ready-for-agent（主体已随票 11 的 executor 落地并测试覆盖，剩三项收尾）

已随票 11 落地并有测试锁定：
- [x] 阶段 A：rebase（决策 74 冲突系统 abort + 打回 develop，冲突文件清单进流转原因）、diff 生成先于闸门（§4.2 契约：失败分支也引用真实 diff 文件）、空 diff 按闸门失败处理
- [x] lint 闸门（project lint_command，未配置跳过）与测试闸门（test_command 超时独立），写 gate / gate_failure_kind（决策 139）
- [x] gate_failures 跨重试累积、不因 merge_result upsert 重置（决策 108）
- [x] allow_dirty_worktree_merge=false 时脏目标进 pending(user_decision, dirty_worktree)（决策 61/132）
- [x] 阶段 B：approve 后经 /merge/decision 合入（ff/--no-ff + update-ref），返回修改直接置游标 develop.execute（决策 119/121）；基准前移 → approval 重置回阶段 A（决策 96/108）
- [x] 决策 139 分流端到端可达：lint 失败 → develop；测试失败 → test.execute

剩余（本票收尾）：
- [ ] rebase 冲突的**自动合并尝试**（pipeline-spec §6：可自动解决则记录后继续；当前直接 abort + 打回 develop，语义更保守）
- [ ] 闸门失败跳回 test.execute 的 `gate_recheck` prompt 注入（决策 85/109：完整日志 + 失败用例进 prompt；依赖票 12 的 prompt 组装到位）
- [ ] E2E 场景：闸门失败 → test 复检 → 重跑闸门的完整循环（票 19 矩阵）
