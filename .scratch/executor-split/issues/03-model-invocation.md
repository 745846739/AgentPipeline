# 03: 模型调用编排 —— agent 节点循环 + 伪阶段并成一片（决策 249 · 第三片）

**What to build:** 把 agent 节点循环（重试、工具循环、元数据提取、异族复判、会话落库）
与**并入的伪阶段**（conflict_check / validator_cross_check / 语义冲突 / project_analysis——
同构：一次 LLM 调用 + 一条 run 行）从 executor.rs 搬进新 module。这是最大也最承重的一片，
排在 ①② 之后正是为了让它的抽取变薄：请求拼装走 01 的 interface、记账走 02 的 interface。

**Blocked by:** 01, 02

**Status: ready-for-agent**

## 一、搬什么

| 职责 | 行段 | 说明 |
|---|---|---|
| agent 节点循环 | 1054-1843 | 按 attempt 重试、工具往返轮、`submit_metadata` 提取与三级降级、异族复判触发、会话行落库 |
| 伪阶段 | 1851-2187 | conflict_check、validator_cross_check、语义冲突 LLM、`project_analysis`（**公开方法 `Executor::project_analysis` 触点冻结**：内部转发到本 module，签名与语义不动） |
| `AgentNodeKind::post_process` | 3420-3670 | 随循环走；**去掉 `&Executor`**——只接真正要的参数（store / settings / 已拼好的 interface 产物），170 行 9 臂 match 的混合职责就地按臂拆清 |

**外部 6 触点里的 `try_run` / `run` 编排外壳留守**（`run_inner` 是留守核的派发）；
SSE 发射留守（本片经既有 emit 函数调用，不自建事件面）。

## 二、interface 要点

- 依赖**显式化**（决策 249 Q2）：本 module 接受 `(store, settings, llm, killer, clock…)`
  的子集，**不伸手拿 `&Executor`**——拆完后全仓 `&Executor` 参数归零（今天只有
  `post_process` 一个）。
- 与 01 的接缝：每 attempt 一次 `assemble`，每轮 `check_budget` + `request`；
  Overflow 翻译（先落快照与会话行，再 `NodeOutput::Pending(context_overflow)`）在**本片**，
  因为构造落点是编排的职责——决策 245「门吃落点不吃原因」在 01 检测、03 翻译。
- 与 02 的接缝：`begin / mark_step / finish / record_usage` 全经 RunLedger，
  承重顺序四条（票 02 §一）在本片的调用侧兑现。

## 三、验收

- **C 桶 10 条 + I 桶 5 条一条不删、断言不放宽**；5 条 subagent 用例（只读集、
  不继承声明工具、深度一层）与 4 条伪阶段用例原样绿。
- **新增窄测试（只做加法）**：`post_process` 的 9 臂在依赖显式化后可逐臂直接测
  （冲突打回、judge continue/goto 的 attempts 语义）；伪阶段的 run 行形状可不跑全循环测。
- `make check` 绿。

**明确不做**：不动 LLM 适配器与 ToolExecutor；不动 `submit_metadata` schema 与降级表；
不把 merge 的 PhaseA/B 带走（票 04）；不改伪阶段与真阶段的配置复用口径（决策 60/67/134/135 语义逐字）；
`project_analysis` 公开入口的语义不改（app 路由在调）。

## Comments

- 2026-09-23 实现落地：`crates/core/src/pipeline/model_invoke.rs`（`ModelInvoke` 只拿
  store / settings / llm / killer / sse / clock 六件，字段廉价克隆、不借 `&Executor`——
  全仓 `&Executor` 参数归零，post_process 9 臂签名改 `inv: &ModelInvoke`）。搬入：agent
  节点循环（重试/工具往返/元数据/异族复判/会话落库）+ 四个伪阶段（含
  `project_analysis` 实现——公开触点冻结，executor 留转发壳）+ `semantic_conflict_check`
  + `context_overflow_exit`（Overflow 翻译随编排走，01 检测 03 翻译）+ AgentNodeKind +
  AttemptTrace/AttemptFailure/failure_metadata/args_summary/module_overlaps。
  SSE 留守核：`emit_tool_event` / `emit_node_started` / `finish_run_with_sse` 三个出口
  函数留 executor、形状单点，编排片经既有 emit 函数调用。executor 4223 → 2088 行。
  既有 C10+I5+F8 全绿；新增 3 条窄测试（post_process 两臂直测 + 伪阶段 run 行形状
  不跑全循环，桩 LlmClient）；core 全量 500+328 绿；clippy / fmt 干净。
