# AgentPipeline v1 收尾执行计划

来源：将 `docs/testing.md` §11「尚未实现 / 代码评审记录的偏差与遗留」与 `.scratch/agentpipeline-v1/issues/19-e2e-matrix.md` 文末「生产 gap」逐条对照 `crates/` 实现现状核对后拆分（2026-09-12）。`agentpipeline-v1` 的票 01–22 已全部 done，本目录承接其中**登记在案但尚未成票**的收尾项。

## 明确不立票的项

- **`task_failed` 终态**：决策 70 已裁决 `failed` 变体保留但 v1 无生产者，用户主动终止走 cancel → `cancelled`，系统级失败终态留待有真实生产者时启用。非缺口。
- **`run_command` 的周期性心跳**：决策 100 的心跳半边已随票 13 落地（票 14 只补逐行输出推送）。

## 依赖图

```
01 e2e 脚手架回灌（预重构）        03 结构性清理（预重构）
02 serve 沉入 lib + 端口 0 回读 ── 18 playwright 双冒烟接线执行
04 上下文管理接线 + L4 兜底          05 pending 补 context.kind
06 dependency_overridden 警告        07 develop 重入注入 review 修改项
08 architect 重入反馈注入            09 gate_recheck 完整命令日志
10 project_analysis 独立观测行       11 retry 会话归档
12 会话消息端点                      13 review_diff 产出与下发
14 命令输出流式 SSE                  15 环境变量值脱敏
16 配置面接线                        17 自适应超时 P50/P90 告警
```

除 **18 依赖 02** 外，各票彼此独立，可按票号任意顺序取用。01 / 03 是预重构，先做能让后续票少走弯路，但不阻塞任何票。

## 关键约束

- 每票验收须落在既有质量闸门内：`cargo fmt --check`、`clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`；涉及前端加跑 vitest / svelte-check / build。
- 改代码前先读 `docs/testing.md` §3 的四条可测试性接缝（决策 143）与 §11 的已知缺口清单。
- 与决策冲突时必须显式标注决策编号（AGENTS.md）。

## 完成状态（2026-09-13）

票 01–18 全部 done。质量闸门全绿：`cargo fmt --check`、`clippy --workspace --all-targets -- -D warnings`、
`cargo test --workspace` = 476 全过；前端 vitest 85 + svelte-check 0/0 + build 成功 +
playwright 双冒烟 2 条全过（`just frontend-e2e`）。

与票面的一处口径修正见票 05：E2E-06b 走的是 merge 测试闸门打回后的**复检**路径，
`context.kind` 应为 `gate_recheck`（非 `test_code_issue`）；非复检的直接 code_issue 路径
另立用例 `e2e_06b_direct_code_issue_pends_with_test_code_issue_kind` 覆盖。
