# 15: merge 阶段执行与闸门

**What to build:** merge 的两阶段编排：阶段 A——rebase 到 default_branch（冲突打回 develop.execute）、执行 lint/test 闸门并把结果写进 merge_result.gate、产出 proposal + diff、进入 pending(merge_approval)；阶段 B——审批通过后执行合入。路由函数已完整（03 票），本票交付让 route_merge 的每个 EdgeKind 真正可达。

**Blocked by:** 11（执行器骨架）

**Status:** done（rebase 冲突自动合并尝试 + `gate_recheck` prompt 注入落地；E2E-06a/06b 归票 19；闸门循环已补 L2 用例）

已随票 11 落地并有测试锁定：
- [x] 阶段 A：rebase（决策 74 冲突系统 abort + 打回 develop，冲突文件清单进流转原因）、diff 生成先于闸门（§4.2 契约：失败分支也引用真实 diff 文件）、空 diff 按闸门失败处理
- [x] lint 闸门（project lint_command，未配置跳过）与测试闸门（test_command 超时独立），写 gate / gate_failure_kind（决策 139）
- [x] gate_failures 跨重试累积、不因 merge_result upsert 重置（决策 108）
- [x] allow_dirty_worktree_merge=false 时脏目标进 pending(user_decision, dirty_worktree)（决策 61/132）
- [x] 阶段 B：approve 后经 /merge/decision 合入（ff/--no-ff + update-ref），返回修改直接置游标 develop.execute（决策 119/121）；基准前移 → approval 重置回阶段 A（决策 96/108）
- [x] 决策 139 分流端到端可达：lint 失败 → develop；测试失败 → test.execute

本票收尾已落地：
- [x] rebase 冲突的**自动合并尝试**：`rebase_onto_with_auto_resolve`（pipeline-spec §6）——可机械判定（某侧等于 merge-base / 两侧同内容，含同名同内容新增）时自动解决并把被解决文件记入 `merge_result.conflict_files`；不可判定时 abort + 打回 develop（决策 74 不变）。libgit2 对同名同内容补丁本就可能自行 Applied 通过，机械解析器是其未覆盖情形的安全网；硬冲突回归用例保留。
- [x] 闸门失败跳回 test.execute 的 `gate_recheck` 注入（决策 85/109）：`apply_edge(GotoTest)` 后，test.execute 的 prompt 注入「## 合入闸门失败复检上下文」= 闸门命令完整 stdout/stderr 预览 + 上一轮失败用例；系统同时置 `test_result.gate_recheck = true`；首轮为空不渲染。L2 用例 `merge_gate_failure_routes_to_test_recheck_then_reruns_gate` 覆盖 闸门失败 → test 复检 → 重跑闸门通过。
- [x] E2E 场景（票 19 矩阵）——不在本票，L2 已覆盖循环。
