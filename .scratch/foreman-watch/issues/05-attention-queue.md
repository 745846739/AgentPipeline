# 05: 值班长待办表 + 调度器接入

**What to build:** 把调度器 tick 里**已经算出来的发现**写进一张待办表，供值守轮消费。

今天这些发现只写日志。`TickReport.reminded` 是内存里的 `HashSet`
（`crates/core/src/scheduler/mod.rs:486-488`）——**重启即失、从不推 SSE**，前端永远不知道。
换句话说：任务停滞超过 `pending_reminder_hours`（默认 24h）今天**没有任何人会知道**。
这是一个独立的既有缺陷，本票顺手修掉。

**事件清单**（判据见 spec §2.2）：

| 事件 | 判据 |
|---|---|
| 任务转 pending | `kanban_tasks.pending_reason_json` 由空变非空 |
| 重试耗尽 | `PendingKind::RetryExhausted` |
| 上下文溢出 | `PendingKind::ContextOverflow` |
| 闸门失败 | develop 闸门 / `MergeResult.gate_failure_kind` |
| 同一任务 30 分钟内转 pending 超过一次 | 需新计数 |
| **调度器处置未生效** | run 已终态（`timeout`/`failed`）而游标仍 `active`、任务仍 `running` |
| **owner 持有超时** | 任务 `running` 且 `executor_owner` 非空、超过 N 分钟无 run 心跳 |
| 任务转 done | `SseEvent::TaskDone` |
| 慢跑（3×P90） | `metrics::should_alert_slow`——**只播报不唤醒** |

**「调度器处置未生效」是这一票的重点**，它是 2026-09-17 实测倒出来的：任务的 run 已被标 `timeout`、
超时处理也写了「干净对话重试」的 transition，但**没有 attempt-2 的 run 行**，任务从此停在
`running`。`check_timeouts` 只看 `active_runs()`（run 已是终态就不再被扫），
`remind_pending_tasks` 的 stalled 判据（`has_pending_cursor && !has_runnable_cursor`，
`scheduler/mod.rs:455-456`）也不成立。**既有调度器与既有台账之间的这条缝，今天零信号。**

**Blocked by:** None

**Status:** ready-for-agent

- [ ] 新迁移 `0017_foreman_attention.sql`：待办一行一事件
      （`id` / `task_id` / `kind` / `detail_json` / `created_at` / `consumed_at`，
      `consumed_at IS NULL` 即未处理）
- [ ] 调度器 tick 里的发现器改为「**先写待办，再（可选）记日志**」——
      日志不是通道，表才是
- [ ] 待办表接进每小时维护作业做年龄清理（与 `conversation_retention_days` 同口径）
- [ ] **修掉既有缺陷**：`TickReport.reminded` 从「唯一的提醒出口」降级为「本 tick 的日志」，
      提醒的事实落表；并补 SSE 事件使前端能知道
- [ ] 唤醒频率相关阈值可配（`PipelineSettings`）：`OWNER_STUCK_MINUTES`（owner 持有超时判据）、
      `REPEATED_PENDING_WINDOW`（30 分钟那次计数）；
      配置是 `deny_unknown_fields` 的（`crates/core/src/config.rs:449-459`），加字段要同步文档
- [ ] 新增用例：造一个「run 终态而游标 active」的任务，断言 tick 后待办表里出现
      `kind = 调度器处置未生效` 的行
- [ ] 新增用例：一 tick 内多个事件**只写多行、只唤醒一次**（唤醒在票 06）
- [ ] 反向用例：节点成功 / 心跳 / 工具调用**不产生待办**（这是 §2.1 那条纪律的牙齿）

## 备注

**别把「发现」和「唤醒」混在一票里**：发现是确定性的、便宜的、每一 tick 都跑的；
唤醒是要花 token 的。混在一起之后就没法单独测「不该唤醒时是否真的没唤醒」。
