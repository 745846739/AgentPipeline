# 04: 按停 / 判超时的中止请求落一条日志

**What to build:** `request_cancel` / `request_hold` 发出中止请求时，日志流里能直接看到
「谁在什么时候请求中止了哪个任务、有没有叫到执行体」——不必再去翻库。

**为什么**：2026-10-08 那次按停的**时刻**在日志里没有任何痕迹（`pause` 只在执行体
**看到**信号时留一句「本轮已有『人按停』的中止请求」，那是延迟 1 分 43 秒之后的事）。
本监控用 `kanban_node_cursors.updated_at` 反推出来的按停时刻——排障不该靠翻库。

**形状**：

- 落点在唯一的发出函数 `executor::request_cancel_with`（两个发出方共用），字段：
  `task` / `origin`（`Timeout` / `Hold` 的人话）/ `notified`（登记里当时确实有执行体吗）。
- INFO 级：这不是异常，是「有人按了钮 / 判超时出手了」——两种来路都要留。
- 措辞与既有日志同族（中文、一句话说清事实）。

**Blocked by:** None

**Status:** done

- [x] 用例：请求中止 → 日志里有一条带 `task` / `origin` / `notified` 的记录；按停与
  判超时两条来路各一条（`executor.rs::a_cancel_request_without_an_executor_is_still_logged`
  / `a_cancel_request_reaching_an_executor_is_logged_with_its_origin`）
- [x] `notified=false`（登记里没有执行体）照样留痕——那正是「按了没反应」要排查的形态
- [x] 顺带：日志捕获抽到 `testkit::log_capture`（一份定义），既有 tools.rs 两条用例改吃它
