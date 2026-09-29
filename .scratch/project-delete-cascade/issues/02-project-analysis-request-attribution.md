# 02: 项目分析的请求台账挂它自己的 run

**What to build:** 决策 329——`Executor::project_analysis` 把调用方手里的**项目级 run id**
透进这次 LLM 调用的 `RunContext`，于是 `kanban_model_requests` 里那几行不再三个归属键全空：
它们挂得上 run，既进得了决策 231 的在飞请求台账与 `model_requests_for_run` 读数，
也能被删项目时的级联带走（决策 328 的 `RUN` 谓词已经覆盖这条路）。

**Blocked by:** None（决策 249 的「要改语义单开票」由本票兑现）

**Status:** done（已实现，决策 329）

- [x] `model_invoke.rs::project_analysis` 收 `run_id: Option<i64>`，
      `RunContext.run_id = run_id.unwrap_or(0)`（0 是既有哨兵，`recording.rs` 归一成 NULL——
      **不能**直接写 `0` 之外的值而不归一，那会撞 `kanban_node_runs` 的外键）
- [x] `Executor::project_analysis` 同签名转发（决策 249 的外部 6 触点之一：
      签名确实动了，按那条决策「单开票」办——本票即那张票）
- [x] app 路由把 `insert_project_run` 已经拿到的 run id 传下去
      （落库失败时为 `None`，那几行保持无归属——没有 run 行可指，这是对的）
- [x] `recording.rs` 那句过时注释改掉（原文「值班长没有 run 行，决策 182⑨；**项目分析也没有**」
      自决策 100 / 迁移 0004 起就不成立了）
- [x] L2 `executor.rs`：`project_analysis` 的请求行能按项目级 run 读到
      （`model_requests_for_run` 一条、`agent_type` 对得上）；另有 `None` 那支的反向用例
      （无 run 行时照落账、不报外键错）
- [x] `fmt` / `clippy -D warnings` / 相关用例全绿

**判据**（决策 328 行里留的那条）：`POST /projects/analyze` 之后，
`model_requests_for_run(<项目级 run>)` 能读到这次分析的请求行；删项目时那几行随项目一起消失。

**量到它的现场**：线上库副本里 4 行 `agent_type = pseudo:project_analysis` 的台账
`run_id` / `session_id` / `task_id` 全为 NULL（每次分析 4 行，从不清理，删项目带不走）。
