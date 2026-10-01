# 02: 超时梯子不再被自己产生的中止行清零 + 重启恢复给任务级 run 收终态

**What to build:** 两处独立缺陷，都让"超时梯子"失效，一起修。

① **`trailing_timeout_streak` 的口径 + `handle_timeout` 会掐掉"活着的那一轮"**。
决策 320 的梯子（`crates/core/src/pipeline/retry.rs:68-100`）本该是
连续超时 1–2 次自动续接 → 第 3 次空白重跑 → 第 4 次挂起交人工；实测**从未升档**：
transitions 82/83/85 记的是「连续第 **0** 次」（那时该节点已超时 3/4/7 次），
只有 2026-10-01 那次记的是「连续第 1 次」。

**机制（由 run 行时序反推，比"自己产生的中止行"更具体）**：重启后留下的**陈旧 run**
被 idle 判死时，它的 `handle_timeout` 会掐掉**当前活着的那一轮**——

| run | attempt | started | finished | 状态 |
| --- | --- | --- | --- | --- |
| 104 | 1 | 00:48:17 | **01:48:23.851** | timeout（进程 01:48:03 已重启，这条是尸检） |
| 105 | 2 | **01:48:13.851** | 01:48:23.853 | cancelled「已按节点超时中止」 |
| 106 | 3 | 01:48:23.865 | 02:04:09 | timeout（continued_from=105） |

attempt 2 **比 attempt 1 的判死早 10 秒起跑**——即重启后新执行体正常跑着 run 105，
而调度器在 01:48:23 对**陈旧的 run 104** 判超时，`request_cancel(task_id)`
按任务键掐掉了**正在跑的 105**。于是 105 落一条 `cancelled`，其 **id 更大**，
而 `trailing_timeout_streak`（`storage/observability.rs:464`）从最新往回撞到第一条非 timeout
就清零 —— **自己造出来的中止行把刚记上的超时清零**，梯子永远停在第一档。

所以要修两处（缺一不可）：
- `handle_timeout` 判死一条 run 时，**只有它才是当前在跑的那一轮**才发 `request_cancel`；
  否则不发（陈旧 run 的尸检不该掐掉活着的 attempt）。判据用「进程内执行体登记的那一轮 run id」
  与「被判定 run 的 id」是否一致，别用时间戳猜。
- `trailing_timeout_streak` 别把**由超时中止产生的** `cancelled` 行当"非超时收场"：
  它必须与"人按停/重跑产生的 cancelled"和"真正的成功/失败"区分开。
  `finish_cancelled` 的来路在 `run_ledger.rs`（`CancelOrigin::Timeout` vs `CancelOrigin::Hold`，
  `pipeline/executor.rs:93-120` 有现成的枚举），落库时把 origin 记下来、或按 run 链
  （`continued_from_run_id`）归并，二选一，**取那条不依赖新列的**。

② **重启恢复给任务级遗留 run 收终态**。
2026-09-30 服务被干净重启 13 次，每次都 requeue 任务、起新 attempt，而
`requeue_running_tasks`（`crates/core/src/storage/tasks.rs:595`）只翻**任务行**，
`abandon_stale_project_runs`（`observability.rs:389`）显式限定 `task_id IS NULL`
—— **任务自己的遗留 `running` run 没人收**。它们 5 分钟后被 idle 超时判死，
`duration_ms` 记的是 since-start（决策 226「时长照实记」），读起来像"跑了这么久才超时"，
实为尸检；每判死一条又制造一个 `cancelled` 行，把 ① 的梯子搅乱。
改法：恢复序列（`crates/app/src/serve.rs:575-650`）补一条——把上一进程遗留的、
**属于任务的** `running` run 收成终态（语义上它们是"进程退出时还在跑"，
不是"节点超时"，文案要如实写，别冒充超时）。

**Blocked by:** None

**Status:** ready-for-agent（2026-10-01；批次一，与票 01 并行）

## 落点

- `crates/core/src/storage/observability.rs:464` `trailing_timeout_streak` 的 SQL/遍历口径。
- `crates/core/src/pipeline/executor.rs:93-120` `CancelOrigin`（已有 `Timeout` / `Hold` 两档）；
  `crates/core/src/pipeline/run_ledger.rs` 的 `finish_cancelled` 落库路径。
- `crates/app/src/serve.rs:598-650` 恢复序列调用点；
  `crates/core/src/pipeline/foreman_actions.rs:234-243` `run_recovery_sequence`（三处共用的唯一实现，
  决策 255——**新的一条要加在这个函数里**，不是只加在 serve.rs 的调用点）。
- 若需要新列或新查询，注意 `kanban_node_runs` 已有 `parent_run_id` / `continued_from_run_id`。

## 验收

- [x] 单测：造 `timeout → cancelled(由超时中止) → timeout` 的 run 序列，
      `streak` 必须是 **2**（现在是 1，因为尾部那条 cancelled 直接清零）
- [x] 单测：`timeout → cancelled(人按停) → timeout` 的 `streak` 必须是 **1**（人介入照旧清零，
      这条语义不许被改掉）
- [x] 单测：**陈旧 run** 被判超时时**不**掐掉当前在跑的那一轮（造 `run A（无心跳，模拟重启前遗留）
      + run B（活着的当前轮）`，判 A 超时后 B 必须仍是 running）
- [x] 单测：第 4 次连续超时走到 `TimeoutRetry::Pending`（现在永远到不了）
- [x] 单测：重启恢复把任务的遗留 `running` run 收成终态，且**不冒充** `节点超时` 文案
- [x] 回归：既有 `scheduler_tick` / `executor` 超时用例全绿（尤其
      `crates/core/tests/integration/executor.rs:6046` 那条"连续 1–2 次自动续接"）
- [ ] `make check` 全绿（本地按决策 331 跑 `make check-lint` + 窄跑，全量在 CI）

**明确不做**：不改梯子的档位与阈值（`TIMEOUT_AUTO_CONTINUES_MAX=2`）；不改 run 行已落库的
`duration_ms`/token/status（历史事实）；**不调大** `architect-design.max_duration_sec`。

**来源：** `.scratch/silent-degradation/spec.md` 缺陷 2；证据 transitions 82/83/85/102、
run 104/106/107/108/109/110/111、`journalctl` 2026-09-30 的 13 次重启。

## 落地记录（2026-10-01）

### ① 梯子计数口径
- **新列 `cancel_origin`**（迁移 `0039_run_cancel_origin.sql`）：`timeout` / `hold` / `restart`
  三档、可空。票面原话是「落库时把 origin 记下来、或按 run 链（`continued_from_run_id`）归并，
  二选一，**取那条不依赖新列的**」——**那条做不到，如实记在这里**：run 链只能证明「这一轮是被
  上一轮续接出来的」，证明不了「这条 `cancelled` 是判超时顺手造的，还是人按停造的」；而按报
  文字样判又正撞决策 259（判据按字段不按文案）。两条路都不成立，才落到新列。
- `RunOutcome.cancel_origin` + `finish_run` 的 `cancel_origin = COALESCE(?, cancel_origin)`
  （只有真的带值时才覆盖）；`CancelOrigin::as_slug`（`pipeline/executor.rs`）是落库取值的唯一处。
- `trailing_timeout_streak` 改成 `SELECT status, cancel_origin`：`timeout` 计数；`cancelled`
  且 origin ∈ {`timeout`, `restart`} → **跳过（不清零）**；其余（含 `hold`、成功、失败、`running`）→ 停。
  `restart` 也在跳过之列：一次重启既不是"这个节点这次没超时"的证据，也不是人介入——不跳过的话，
  2026-09-30 那 13 次干净重启会把刚爬上去的梯子连抹 13 次。

### ② 陈旧 run 不掐活轮
- `scheduler/mod.rs::check_timeouts` 先取一次 `active_runs()`，按 `cursor_id` 建
  `newest_per_cursor: HashMap<String, i64>`；判一条 run 超时前先问「它是不是该游标下 id 最大的
  那一轮」，不是 → 只收终态（`Timeout` + 如实时长 + 「陈旧 run」文案）并**不发** `request_cancel`。
  判据是「同 cursor_id 下有没有更大的 run id」，不是时间戳——重试复用同一 cursor，用时间戳猜
  会把正常并发误判成陈旧。
- `TickReport.stale_timeout_runs` 把这类收尾单独列出（观测面能看出"这一条是尸检，不是活轮"）。

### ③ 重启恢复收任务级遗留 run
- 新 `Store::abandon_stale_task_runs()`：把 `active_runs()` 里 `task_id.is_some()` 的行收成
  `Cancelled` + `cancel_origin = 'restart'` + 如实时长（`duration_ms` 从 `started_at` 起算，
  决策 226 照实记——0 会把"跑了 40 分钟被重启打断"读成"刚起来就没了"），文案如实写
  「进程重启：这一轮在上一进程退出时还在跑」，**不冒充**「节点超时」。
- 加在 `run_recovery_sequence` 里（不是只加在 `serve.rs` 的调用点）——三处共用同一个实现
  （决策 255），步骤从 3 变 4+1；`RecoveryReadings.abandoned_task_runs` 随之进读数与启动日志。

### 测试
`crates/core/tests/integration/scheduler_tick.rs`：
`the_ladder_still_climbs_across_a_timeout_originated_cancel`（含"第 4 次走到 Pending"）、
`a_human_cancel_still_resets_the_ladder`、`a_stale_timeout_only_closes_the_row_and_leaves_the_live_attempt_alone`、
`a_restart_closes_leftover_task_runs_without_impersonating_a_timeout`。

### 现场验收（2026-10-02，106 真实数据）——**这一条没被打到，如实记**

事故任务续跑后到 03:01 CST 为止，**一次节点超时都没发生**：develop 阶段的两次失败都是上游断流
（`LLM 调用失败：流在没有 [DONE] / finish_reason 的情况下结束`，收到 32327 / 2247803 字节后断开），
重试按 attempt 递增（143 → 2 → 3）继续走，没有出现事故里那条
「超时 → 自动续接 → 被自己的中止行清零」的循环。

也就是说：**梯子的现场证据仍然欠缺**。本票现有的正面证据只有
`crates/core/tests/integration/scheduler_tick.rs` 那四条集成用例，106 上只拿到「没有再犯」的旁证
——旁证不是证成：没被打到等于没有反例，不等于有正例。将来手上有超时场景的真实数据时，
这是第一个该复看的地方。详见 `.scratch/silent-degradation/spec.md` 的「现场验收」。
