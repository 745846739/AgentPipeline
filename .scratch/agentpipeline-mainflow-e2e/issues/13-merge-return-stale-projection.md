# 13: 决策落库后任务投影不同步——「返回修改 / 人工评审」后 UI 显示陈旧状态（主流程 e2e 暴露的真实缺陷）

**Status:** done（2026-09-13，被票 06 的「返回修改」用例暴露并修复）

## 缺陷

`apply_merge_decision`（`crates/core/src/storage/decisions.rs`，决策 119）把游标打回
develop/execute、清掉 pending，但**不调 `sync_task_projection`**——`kanban_tasks` 行的
`current_stage / current_node / pending 投影` 要等执行器下一次写库才翻转。窗口期内
`GET /tasks/{id}`（与看板、任务详情）继续报告「pending(merge_approval)」。

实测后果（真实二进制 + playwright，2026-09-13）：

| 步骤 | 现象 |
|---|---|
| 点「返回修改」→ 200 | 游标已回 develop，第二遍流水线在跑 |
| 轮询 `GET /tasks/{id}` | **立刻**又「看到」merge_approval（陈旧投影）|
| 对陈旧状态点「合入」 | `404 任务 … 没有 merge 阶段的活跃游标` |
| 轮询方认为合入失败 | 实际任务正在正常推进——观测完全失真 |

同一缺陷在 `apply_human_review`（决策 2 / 124）同构存在：reject 后 `GET /tasks/{id}`
仍旧显示 human_review，用户会对已打回的评审再点一次「通过」。

修复前证据：返回修改后的流转时间线停在 `user_resume → develop`，而 API 报告
`pending_reason = merge_approval`——两者互相矛盾，即投影滞后。

## 修复

`apply_merge_decision` 与 `apply_human_review` 在事务提交后立刻
`sync_task_projection(task_id)`（与 `apply_resume` 路径既有做法一致）。

钉住用例：`crates/core/tests/executor.rs::merge_return_updates_task_projection_immediately`
（修复前红：投影仍为 `Merge`；修复后绿）。

## 与决策的关系

- 不改决策 119 / 2 / 124 的语义，只补「写侧同步投影」的实现缺口；
- 发现路径：主流程端到端票 06——返回修改用例的中间态轮询与决策端点互相矛盾，
  追出投影不同步（此前无任何测试走过 merge return）。
