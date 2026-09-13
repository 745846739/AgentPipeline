# 12: 设计类阶段 retry_exhausted 的「重试执行」是死按钮（主流程 e2e 暴露的真实缺陷）

**Status:** done（2026-09-13，被票 03 的恢复路径用例暴露并修复）

## 缺陷

决策 130 的 `retry_exhausted` 动作表把「重试执行」硬编码为 `goto(reason.stage, Execute)`
（`crates/core/src/actions.rs`），而 resume 端点按决策 69 校验「goto 落点必须是
`entry_node(stage)`」。两表对照：

| 阶段 | 动作表给的落点 | entry_node（决策 69） | 结果 |
|---|---|---|---|
| merge / develop / review / test | Execute | Execute | ✓ |
| architect-design / develop-design / test-design | Execute | **ValidateInput** | ✗ 必然 400 |

实测证据（真实二进制 + 探针用例，2026-09-13）：provider 配错 → 任务挂在
`architect-design.validate_input`（**主流程第一步**）→ 面板点「重试执行」→
`POST /tasks/{id}/resume` 返回
`400 {"error":"goto 落点必须是 architect-design 的入口节点 validate_input（决策 69）"}`。

雪上加霜的是前端 `TaskDetail.handleAction` 用 `.catch(() => undefined)` 吞掉提交错误，
且 store 层的 `actionError` **全 UI 无消费**——用户视角就是按钮点了没反应，面板成为死路。

## 修复

1. **动作表**：兜底行落点改为 `entry_node(reason.stage)`；钉住用例
   `actions.rs::retry_exhausted_goto_lands_on_stage_entry_for_every_stage`（全阶段断言）。
2. **可见性**：TaskDetail / Board 补「动作提交失败」横幅，消费 `actionError`。
3. **决策 159** 已追加（修订决策 130 的表行，显式标注）。

## 验收

- [x] 探针复现：修复前 resume 400，修复后任务推进到 merge_approval
- [x] `provider-misconfig.spec.ts` 全过（含恢复路径）
- [x] 全量闸门绿（490 Rust 测试 / 85 vitest / 4 条 playwright）
- [x] 决策 159 已追加（只追加，不修改既有行）
