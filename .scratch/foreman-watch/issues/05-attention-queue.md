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

**Status:** done

- [x] 新迁移 `0017_foreman_attention.sql`：待办一行一事件
      （`id` / `task_id` / `kind` / `detail_json` / `created_at` / `consumed_at`，
      `consumed_at IS NULL` 即未处理）
- [x] 调度器 tick 里的发现器改为「**先写待办，再（可选）记日志**」——
      日志不是通道，表才是
- [x] 待办表接进每小时维护作业做年龄清理（与 `conversation_retention_days` 同口径）
- [x] **修掉既有缺陷**：`TickReport.reminded` 从「唯一的提醒出口」降级为「本 tick 的日志」，
      提醒的事实落表；并补 SSE 事件使前端能知道
- [x] 唤醒频率相关阈值可配（`PipelineSettings`）：`OWNER_STUCK_MINUTES`（owner 持有超时判据）、
      `REPEATED_PENDING_WINDOW`（30 分钟那次计数）；
      配置是 `deny_unknown_fields` 的（`crates/core/src/config.rs:449-459`），加字段要同步文档
- [x] 新增用例：造一个「run 终态而游标 active」的任务，断言 tick 后待办表里出现
      `kind = 调度器处置未生效` 的行
- [x] 新增用例：一 tick 内多个事件**只写多行、只唤醒一次**（唤醒在票 06）
- [x] 反向用例：节点成功 / 心跳 / 工具调用**不产生待办**（这是 §2.1 那条纪律的牙齿）

**实施收尾（2026-09-18）:**

- **迁移号取 0019**（0017 = prompt 原文、0018 = run 的 step，都是票 02 / 04 落的）。
- **表多了一列 `occurred_at`**（票面字段列表之外）：事件的**发生时刻**，且是去重键的一半
  （`UNIQUE(task_id, kind, occurred_at)`）。没有它，`created_at` 会把两件完全不同的事混成
  一个数——「同一次 pending 被每一 tick 重复写」与「同一任务 30 分钟内真的 pending 了两次」；
  前者让唤醒被自己的重试刷屏，后者恰是「自动修复没治好」的判据（§4.9）。这一条写进了迁移注释。
- **两个阈值**：`watch_event_window_minutes`（30，事件的新鲜窗口，**同时**是票面那个
  `REPEATED_PENDING_WINDOW`——「多久之内算同一件事」本来就是一个数）与
  `watch_owner_stuck_minutes`（10，票面的 `OWNER_STUCK_MINUTES`，也用作
  「处置未生效」的宽限）。已同步 `docs/overview.md` §3 的参数表。
- **多了一个类别 `task_stale`**：停滞提醒（>24h 的 pending）**必须**能唤醒值守轮，而它恰好
  会被「只报新鲜事」的窗口漏掉（事件发生在 24h 前）。它在提醒那一刻以 `now` 为 `occurred_at`
  落表，与那条 `stalled` SSE 一起走。
- **`slow_run` 只播报不唤醒**：类别上带了 `wakes()`，慢跑是唯一返回 `false` 的那个
  （决策 66 的自适应告警没有可操作的动作）。§2.1 那条纪律因此有了牙齿，而不是一句注释。
- **闸门失败读的是同一处 `merge_result`**：develop 闸门失败也会落到那条记录上，故一个读数
  覆盖票面点名的两处。
- **顺手修掉的既有缺陷**（票面点名）：提醒现在真的发 `SseEvent::Stalled` 了——`docs/operations.md`
  一直写着「提醒与高亮均通过 SSE 推送」，而代码里从来没有发过；文档那句在本次一并订正。
- **五条用例**：终态 run + active 游标 → `scheduler_no_effect` 一行；宽限期内**不报**；
  owner 心跳停跳 → `owner_stuck`；健康在跑 → 零待办；两次 tick 同一件事**不重复写行**；
  停滞提醒 → SSE + `task_stale` 行。

## 备注

**别把「发现」和「唤醒」混在一票里**：发现是确定性的、便宜的、每一 tick 都跑的；
唤醒是要花 token 的。混在一起之后就没法单独测「不该唤醒时是否真的没唤醒」。

## 闸门跑通后补记（2026-09-18）

**`owner_stuck` 有一条成立前提，票面没写、用例原先也没摆对**：它要求「该节点的空闲超时
**长于** `watch_owner_stuck_minutes`」。空闲超时若先到（缺省 300s < 停跳线 10 分钟），
`check_timeouts` 会把 run 标终态，于是看到的是「处置未生效」那一类；`owner_stuck` 管的是
另一种局面——清扫暂时不会来（长跑节点把空闲超时按小时配），而主已经不在了。用例改成按长跑
节点配（`node_idle_timeout_sec = 7200`）并在注释里写清这条前提，否则它测的是一个到不了的态。

用例侧还有一处**此前一直假绿**的写法：`Harness::running_run` / `finished_run` 把 run 的
`(stage, node)` 写死成 `develop.execute`，而 `seed_task` 建的游标停在 `init.execute`。
超时清扫按 run 自己那一行判，所以那些用例照过；而按 `(task, stage, node)` 找 run 的两个读法
（本票的 `owner_stuck` / `scheduler_no_effect`）会一条都找不到——判据永远返回「没卡住」。
两个 helper 改成从游标取 `(stage, node)`，需要 run 落在 develop.execute 的用例（超时分层、
自适应分位数）先把游标推过去（`Harness::advance_to_develop`），因为那些读数**本来就是**
按节点取配置与样本的。
