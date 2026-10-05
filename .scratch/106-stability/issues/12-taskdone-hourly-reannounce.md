# 12: 小时级维护把 done 任务的「刚完成」洗成现在——PWA「任务完成」每小时重发

**What to build:** 指标聚合是**读数**，不是**事件**——小时级 `maintenance`
（`aggregate_node_metrics` → `refresh_task_totals`）不再无条件把每个 done 任务的
`updated_at` 写成现在。于是「刚完成」的新鲜度窗口在真实完成那一刻起算，`TaskDone`
待办只插一条，PWA「任务完成」推送不再跟着维护循环每小时重发一条。token / 调用数
真的变了（run 多了、用量补记了）才刷新时间戳——那时它确实是新事件。

**Blocked by:** None (can start immediately)

**Status:** done（2026-10-04）

- [x] 诊断定案：链条上一环都不是嫌疑，唯一无条件复位的正是「读数刷新」。
      `refresh_task_totals`（`crates/core/src/storage/tasks.rs`）原先**无条件**
      `UPDATE ... SET total_tokens=?, total_calls=?, updated_at=now`；小时级维护
      `aggregate_node_metrics` 对**每一张**任务卡（`include_archived: true`）
      逐个调它。而 `updated_at` 在 done 任务上是 `note_discoveries` ④ 的
      「新鲜事」判据（`now - task.updated_at <= watch_event_window_minutes`，
      缺省 **30 分钟**），`occurred_at` 又取 `task.updated_at` 作
      `note_attention(TaskDone, …)` 的入参——待办去重键是
      `(task_id, kind, occurred_at)`（`ON CONFLICT(task_id, kind, occurred_at) DO NOTHING`），
      时间戳一被维护洗成新的整点，去重键就变了，必插新行；新行 `kind.wakes()`
      为真即经 `notifier.notify` 走出去（`storage/attention.rs::note_attention`）。
      维护每小时一趟 → 每小时一条新 `TaskDone` → PWA「任务完成」每小时一条，
      永不收场。
- [x] 修复落地：`refresh_task_totals` 的写入收成**一条条件 `UPDATE`**——
      `WHERE id = ? AND (total_tokens <> ? OR total_calls <> ? OR status = 'running')`。
      读数没变**且**任务不在跑时 `WHERE` 不成立，一行都不动（`updated_at` 不被改写）；
      在跑的任务照刷；读与写之间不留竞态窗口。`updated_at` 的语义由此明确为
      「**事件**时刻，不是读数时刻」，写进方法文档。
      **在跑的任务为什么放行**：run 行的 token 到节点收口才落库（轮内只走
      `touch_run_heartbeat`，只写 `last_activity_at`），长节点在库面上的读数本就不变——
      判据若也管在跑的任务，就等于静默回退决策 375（详情页 `updated_at` 再次冻结、
      监控把活跃任务误判卡死）。
- [x] 调用点核对：`refresh_task_totals` 的九处调用方（`model_invoke` 六处收口/滚动、
      `scheduler` 维护、`subagent`、`executor` 终态）在用量变化的路径上都伴随 run 行
      变更，读数必变、写入不受影响；读 `task.updated_at` 的只有闸门失败（③）与完成
      播报（④）两条「新鲜事」判据，卡住/停滞判据读的是 `cursor.updated_at` 与 run
      心跳，不依赖这次改写。
- [x] 回归验证：两条集成用例。①
      `scheduler_tick::maintenance_with_unchanged_totals_does_not_reannounce_done_tasks`
      ——两次维护间推进一小时，断言读数没变时 `updated_at` 一字不动、`tick` 的
      `attention_noted == 0` 且 `TaskDone` 仍只有一条、补一条新 run 后写入恢复
      （`total_tokens == 375`）。**牙齿已验**：把条件去掉、回到无条件 `UPDATE`，第一条
      断言先红（实测 `left=00:00 right=01:00`）。②
      `scheduler_tick::running_task_totals_refresh_still_unfreezes_updated_at`
      ——在跑的任务读数没变也刷 `updated_at`（钉住决策 375 不被回退），收口后同口径
      不再刷；**牙齿**：把 `OR status = 'running'` 拿掉，该用例先红。

> **证据分级（诚实记账）**：本票**可完全自证**的是代码链——去重键含 `occurred_at`、
> 维护无条件写 `updated_at`、完成播报窗口 30 分钟、`note_attention` 新行即 `notify`，
> 四段均在源码里可逐行复核，且集成用例在撤掉修复后即红。**现场读数**（2026-10-04
> 报的「PWA 每小时一条任务完成」）由本次开工会话记录，未随仓留存脱敏样本——与决策
> 384/385 的 106 库取数不同，本票没有可回放的 106 台账切片，故此处**不编造**具体
> 条数与时刻。
