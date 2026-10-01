# 04: merge 状态机 —— A/B 相收成一片（决策 249 · 第四片，风险最大故收尾）

**What to build:** 把 merge 的状态机（Phase A rebase 与基准失配、内存合入与引用写回、
Phase B、proposal 生成与 `pending(merge_approval)`）从 executor.rs 搬进新 module。
排序放最后（②→③→⑤→④）：它只有 2 条全栈测试钉着真 git 链，是五片里最险的一片——
前三片把模式跑熟、把接缝调顺之后再动它。

**Blocked by:** 01, 02, 03

**Status:** done（已实现，决策 249：五片同批落地并过闸门）

## 一、搬什么

| 职责 | 行段 | 说明 |
|---|---|---|
| merge 状态机 | 655-957 | `merge_execute` 主循环（基准移动 → 审批失效 → 回 Phase A 的 `continue`）、`merge_phase_a_inner`（rebase：`rebase_onto_with_auto_resolve`）、内存合入与 `force` 引用写回、`merge_phase_b`（脏工作区 → Pending） |
| proposal 生成 | 同段 | diff 文件、DiffStats、审批态落 `merge_result.approval`（决策 72） |
| `parse_diff_stats` / `test_command_for` | 3962-4060 | 随迁（`repair.rs:257` 在用 `test_command_for`——**import 路径改一次，语义逐字不动**，让修复链与闸门命令同源这条既有性质保住） |

**闸门执行不搬**（决策 249 留守核裁定）：`develop_code_gate` / `test_code_gate` 的
命令跑法与日志落盘（`962-1048`、`3076-3181`）留留守核，本片经其调用——merge 闸门失败
→ kickback 的**路由判定**（`routes.rs` 的 gate=fail 分流）本就不在搬移面内，一并不动。

## 二、interface 要点

- 依赖显式化：本 module 接受 `(store, git, settings, clock…)` 子集；`Git` 今天是单元
  struct 直调——**不为本片新开 git trait**（testkit 真仓 fixture 是既定测法，开 trait
  即单 adapter 假想 seam）。
- 与 03 的接缝：节点循环吐 `NodeOutput`，merge 的 PhaseA/B 结果翻译成 Route/Edge/Pending
  仍照既有三通道（`NodeOutput::{Route, Edge, Pending}`）——**通道形状不动**，
  `route_merge_*` 的全 `EdgeKind` 分支表（testing §5 第一优先级）原样。
- 承重性质逐字保：基准移动 → 审批失效 → 重跑 Phase A 的循环语义；脏工作区是 Pending
  不是 Route；`Approval::Approved → None` 的中间态只在循环内。

## 三、验收

- **B 桶 3 条一条不删、断言不放宽**：`merge_return_updates_task_projection_immediately`、
  `merge_gate_failure_routes_to_test_recheck_then_reruns_gate`（有状态闸门三连跑，全栈必绿）、
  `diff_stat_summary_is_parsed`（纯函数）。e2e 的 merge 组（`happy_path`、
  `base_moved_invalidates_approval…`、rebase 冲突打回等 ≈14 条）全绿。
- **新增窄测试（只做加法）**：Phase A 的「基准失配 → 审批失效」判定可在不跑全节点
  循环的层直接测；`parse_diff_stats` 随迁后保持零依赖单测。
- `make check` 绿。

**明确不做**：不动 rebase/合入的 git 操作语义（决策 73/97，风险优先级②「每操作必测」）；
不设审批 TTL、不改 `approval` 落点；不把闸门执行搬进来（留守）；不动 `route()` 条件边
（事实源 `routes.rs` + `landing.rs`，245/248 之后）。

## Comments

- 2026-09-23 实现落地：`crates/core/src/pipeline/merge.rs`（`MergeFlow` 借用四件套
  store / settings / sse / clock；`execute` + `phase_a(_inner)` + `phase_b(_inner)` + 私有
  `PhaseA`/`PhaseB` 枚举逐字搬入）。**闸门执行留守**：`run_code_gate` / `run_system_command`
  从方法改成 executor 的自由函数（显式传 store/settings/clock），留守核的 develop 闸门与
  本片都经它调——不借 `&Executor`；`pend_reason` 同样下沉为自由函数（挂起出口单点，
  留守核两条路径 + 本片共用）；`begin_run_with_sse` 补齐与 `finish_run_with_sse` 对称的
  开立+事件单点。`test_command_for` / `parse_diff_stats` 随迁本片（repair.rs /
  model_request / 集成测试 import 改道，语义逐字）。新增窄测试 2 条：基准失配→审批失效
  **不跑全节点循环**直测（`reset_stale_approval` 抽层）+ parse_diff_stats 随迁单测。
  B3 三条 + e2e merge 组全绿；core 全量 502+328 绿；clippy / fmt 干净。executor → 1756 行。
