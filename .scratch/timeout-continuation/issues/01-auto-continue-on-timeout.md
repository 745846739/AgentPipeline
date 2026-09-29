# 01: 超时类重试改自动续接——上限 2 次，降级空白 1 次，再挂起

**What to build:** 节点超时（idle / max_duration）后的自动重试从**空白重跑**改为
**自动续接**（带 `continued_from_run_id`，与手动「继续」同路，`crates/core/src/pipeline/
resume.rs` 的既有机制）：自动续接最多 2 次 → 降级回空白重跑 1 次 → 再超时挂起交回人工
（`pending_reason`）。次数**写死不配**（少一份组合），数字进决策日志；与托管止损
「满 2 次即停」（决策 210）同一量级。错误类别分流（决策 298）的既有语义不回退——
只改超时那一类的重试形态。

**Blocked by:** None

**Status:** done（2026-09-29 实现，决策 320）

- [x] 超时判定的重试支（`scheduler/mod.rs::handle_timeout` → advance 重试流转）带
      `continued_from_run_id`，续接计数记在游标 / run 链上
- [x] 第 3 次续接尝试改空白一次，第 4 次超时挂起 pending
- [x] 非超时类（传输 / 配置 / 校验）重试形态不变（决策 278 / 298 用例照旧绿）
- [x] L2 集成：模拟两次超时 → 第三次 run 带 continued_from_run_id；第四次空白；第五次挂起
- [x] 决策日志追加：显式修订决策 298 的超时支（重跑 → 续接）与决策 226 的收场说明

## Comments

### 2026-09-29 实现（决策 320，编号 318 已由并行的存储维护工作线预占）

- **计数口改了**：续接计数不记在游标上，而是「该节点**从最新 run 往回连续是 timeout** 的
  条数」（`Store::trailing_timeout_streak`）——空白重跑不带续接链接，按链接链回数会在
  空白轮后归零、梯子失忆；按连续超时 run 数，空白轮之后的超时照样累进（第 4 次挂起）。
  口径三限：只数节点自身 run（`NODE_OWNING_AGENT_TYPES_SQL` 白名单）、撞到非 timeout
  终态即停（人工介入重新起算）、running 也算撞墙。
- **置位口**：`Store::mark_cursor_continuation` 直接写 `resumed_from_pending_kind` 列
  （决策 205 原因列的第二把合法钥匙）——超时路径的游标从未 pending 过，
  `clear_cursor_pending` 那把钥匙够不着。**只对 agent 节点置位**（纯代码节点没有转录，
  置了没人取走会悬在列上）；`AgentNodeKind::of` 为此改 `pub(crate)`。
- **转录可得性**：被超时杀掉的 run 的转录由既有的失败落库路径补写
  （`record_failed_attempt` 对含 Cancelled 在内的一切 attempt 失败都落会话行）——
  L2 实测卡死在**第一次**模型调用的 run 转录为空，`take_continuation` 的「空转录不续」
  正确回退成干净起跑（第一轮本来就该空跑）。生产形状（先干了活再卡）续接的是真实进度。
- **读排在写之后（合入前实测补的顺序保证）**：上面那条「可得性」漏了一个竞态——
  `release_ownership` 摘登记后 resume 起的新执行体，可能在旧执行体还没写完会话行时
  就进 `take_continuation` 读，读到空就回退，且原因列读-清一次、白白烧掉一格梯子。
  修法：`handle_timeout` 改成「先只发中止请求 → `executor::await_teardown` 有界等旧
  执行体自然退出（上限 2s 写死）→ 再摘登记」；等不到的（卡在不返回的同步调用里，
  303 现场）到点照旧兜底，代价不新增。L2 同时补了测试侧的确定性：判超时前先等
  `ToolThenStall` 的计数走到 `2 * round`（call B 已进场），保证转录非空——原先约
  1/3 概率中止请求抢在第一次工具轮之前落地，第 2 轮断言偶发红。
- **顺带修了一个承重缺口（执行体观察点持有制）**：`force_release` 摘走去重登记后，
  旧执行体再按名字取取消信号就取不到——正走在两轮之间的执行体丢了中止请求，
  停在下一个不返回的调用里再也出不来（转录也不落库），续接就续了个寂寞。修法：
  `ExecutorGuard` 携带自己那一格的 `CancelSignal`，`run_inner → execute_node →
  agent_node → agent_attempt → agent_attempt_inner` 一路传参，不再按名字重取；
  `held_by_human` 判据收进 `held_by_human_signal(&signal)`。登记本身只剩
  「请求中止」与「去重」两职。
- **L2**：`executor::timeout_retry_ladder_continues_twice_then_blank_then_pending`
  全程真执行体 + ToolThenStall（每轮先真实执行一次工具、再停在一次不返回的调用上）：
  run3/run4 带 `continued_from_run_id`（指向前一条超时 run）、run5 无链接（空白）、
  run5 超时后游标 pending(timeout) 且不再自动起 run。调度器侧梯子在
  `scheduler_tick::timeout_kills_process_group_and_retries_per_ladder`（杀进程组 +
  标记置位/空白不置位 + 挂起全链）。
- **实测留待**：桌面壳跑起来观察一次真实的超时续接（对讲台台账里看
  `continued_from_run_id` 链与「连续第 N 次超时」的 transition 文案）。
