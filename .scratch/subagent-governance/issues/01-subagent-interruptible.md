# 01: 子代理可被中止（三个观察点）

**What to build:** 父节点收到中止请求（人按停 / 判超时）后，在飞的子代理**随即收口**，
不再跑满自己的轮数预算；它那一行 run 记 `cancelled` + 来路，回给父代理的回执照实写
「被中止（未完成）」。

**为什么**：2026-10-08 实测——08:18:47 按停，子代理 #3 又跑满 80 秒（08:19:10→08:20:30），
执行体直到轮边界才让出执行权，按停延迟 1 分 43 秒。机制上：`pause` 只置信号，
执行体在**轮边界**看一眼（`executor.rs` 的 `held_by_human_signal`），而子代理这一侧
**完全不看**——`SubAgentRunnerConfig` 里没有任何中止句柄，`run_rounds` 也没有打断点，
它的模型调用是裸的 `llm.complete()`（连决策 226 那个「模型调用可被打断」的观察点都不走）。
最长尾部是子代理的模型调用挂死：`idle_timeout_sec: None` + 持续给父 run 打心跳，
只有 `max_duration`（缺省 1800s）能收住。

**形状**：

- `SubAgentRunnerConfig` 带一份**与父节点同源**的中止句柄（`CancelSignal` 是
  `Arc<AtomicBool/U8/Notify>`，可克隆）；`model_invoke.rs` 构造处把手里那份传进去。
- 三个观察点：① `run_rounds` 每轮开头（同 `model_invoke` 的姿势）；② 一轮内部
  **每个工具调用之间**（子代理工具全是只读，批内打断安全）；③ 子代理的**模型调用**
  用 `select!` 等信号（同决策 226 的姿势）。
- 收口形状：新增类型化的收场（`SubAgentEnd`：完成 / 未收口 / 超时 / 被中止）——
  今天四个终局挤成一个 `String`，类别丢失。被中止时：run 行 `NodeStatus::Cancelled` +
  `cancel_origin`（`hold` / `timeout`），error 写「子代理被中止（第 N 轮）：<来路>」，
  回给父代理的文本标「未完成」。
- 父代理那一侧**不动**：它的工具批保持原子（不在批内打断父轮），轮边界照旧让位。

**Blocked by:** None

**Status:** done

- [x] 用例：按停 → 在飞的子代理在**下一个观察点**收口（不跑满轮数），run 行
  `cancelled` + `cancel_origin=hold`，父代理拿到「未完成」回执
  （`executor.rs::a_hold_stops_a_stalled_subagent_at_the_model_call`）
- [x] 用例：判超时（`CancelOrigin::Timeout`）同一条通道，`cancel_origin=timeout`
  （`executor.rs::a_timeout_cancel_stops_a_stalled_subagent_with_its_own_origin`）
- [x] 用例：子代理的模型调用挂住时，按停能在**不动 max_duration** 的前提下把它叫停
  （同上两条：`Step::Stall` 停在第 2 轮，`max_duration` 吃缺省 1800s）
- [x] 用例：无事发生时子代理行为逐字不变（正常摘要 / 未收口 / 超时三条既有语义不漂
  ——既有七条 L2 用例一字未改仍绿；四终局的回执 / 台账文本另由
  `tools.rs::sub_agent_end_renders_each_ending_distinctly` 逐句钉住）
