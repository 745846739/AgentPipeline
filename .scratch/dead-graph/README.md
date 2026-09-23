# 死镜像退场：`pipeline/graph.rs` 与 petgraph（dead-graph）

**状态（2026-09-23）：票 01 已落地（`Status: done`），`make check` 绿。** 本目录来自一次架构评审
（`improve-codebase-architecture`，2026-09-22）的**候选 3**，2026-09-23 经**一轮拷问五问「全部同意」**
定稿。评审报告是临时的，不入库；设计结论以本文件与**决策 248** 为准。

**出处**：决策 245⑤ 显式留口「不删 `petgraph` 与 `pipeline/graph.rs`——**另一个候选**」，
`.scratch/cursor-advance/issues/03`「明确不做」第四项同指本票。本票闭合那个口。

## 一、问题

`pipeline/graph.rs`（202 行）是流水线拓扑的**第二份描述**，而真正的驱动是
`landing.rs`（落点表）+ `routes.rs`（条件边）。删除测试给出裁决：

- 全仓引用它的地方 = `mod.rs` 的 re-export（`pub mod graph` + `pub use graph::{…}`）
  + **它自己的 5 个测试**。生产调用方：**零**。删掉生产行为零变化。
- `petgraph` 是**运行时**依赖（workspace `Cargo.toml:23` + `crates/core/Cargo.toml:14`），
  但使用它的 `.rs` 只有 `graph.rs` 一个文件。
- 它可以悄悄与 `landing.rs` 漂移——没有任何断言要求 `successors()` 等于 `next_stages`。

## 二、5 个测试逐条处置：**一处不搬，全删**

| 测试 | 断言的东西 | 处置与理由 |
|---|---|---|
| `graph_contains_every_stage_node` | 图里有 `nodes_for_stage` 的每个节点 | **同义反复**——图就是从 `nodes_for_stage` 构造的，删 |
| `parallel_branches_converge_on_join` | 两个设计阶段的末节点有 `Next → (SyncCheck, Execute)` | 与 `landing.rs::next_stages_full_table` + `only_parallel_branches_point_at_join` 重叠，删 |
| `architect_design_splits_into_two_branches` | architect 后继含 develop-design / test-design | 与 `next_stages_full_table`（逐行钉住全部跨阶段后继）重叠，删 |
| `merge_has_all_three_outcomes` | merge 后继含 Done / Develop / Test 三种边 | 由 `routes.rs::route_merge_*`（NoOp ×5、Next、KickbackDevelop、GotoTest、Pending 全分支，testing.md §5 点名）**行为断言**覆盖，删 |
| `retry_edge_exists_on_every_validate_output_stage` | 每个有 validate_output 的阶段有 Retry 回边 | 由 `route_after_validate_output` 的 Retry 分支 + 决策 245 收口后的 `advance` 一笔事务覆盖，删 |

**唯一没有别处覆盖的**是 `Review.ValidateOutput→Develop.Execute` 那条 KickbackDevelop
**拓扑边**——但生产里它**不是 EdgeKind**：review 打回走 `decisions` 端点的 goto（`routes.rs:221`
自注「返回修改由 merge/decision 端点直接置游标，不经路由」，路由返回 `Pending`），
`graph.rs:79` 自注「只是 goto 的合法落点，不是 `route()` 的返回值」。删它是**去掉一幅误导的图**，
不是丢覆盖。决策 39（显式 Rust 路由函数）与 80 / 107（游标与 join 屏障）**不依赖 petgraph**，
一条边语义不动。

## 三、七处机制陈述同批回填（全是机制句，删除后逐字变假话）

| # | 文件:行 | 原句要害 |
|---|---|---|
| 1 | `docs/glossary.md:15` | Pipeline 词条「基于 petgraph 构建」 |
| 2 | `docs/overview.md:88` | G15 特征「独立的 petgraph DAG 图」 |
| 3 | `docs/operations.md:728` | 「kanban 使用独立的 petgraph DAG」 |
| 4 | `docs/implementation.md:11` | §11.1 架构框图头「petgraph DAG（状态流转）」 |
| 5 | `docs/implementation.md:50/52/55-75` | §11.2 标题、导语、整段 `graph.rs` 示例（示例本身与真代码就对不上） |
| 6 | `docs/implementation.md:151` | advance 签名注释里「`pipeline/graph.rs` 是……死镜像（评审候选 3）」 |
| 7 | `docs/README.md:15` + `design/frontend-design.md:502` | 索引行「petgraph DAG + executor」；PipelineRail「来自 petgraph 的静态形状」 |

机制统一换成「`landing.rs` 落点表 + `routes.rs` 条件边」的静态拓扑表口径。

## 四、决策 248 三件事（Q3 定稿）

1. **退场事实与理由**：死镜像删除测试即证据；5 测试逐条处置（上表）；两处 Cargo.toml + lock 随行。
2. **修订决策 5**：「petgraph 只负责调度」成假——生产里它**从未被调度过**（graph.rs 零调用方）；
   现调度 = `routes.rs` 条件边 + 决策 245 的 `advance` 落点门；「LLM 直接调 rig」那半句不动。
3. **接决策 245⑤ 的口**：「不删……（另一个候选）」到此闭合（行内加「已由决策 248 执行」标注，
   照 246 标注 179 的笔法——只加标注、不改旧文）。

## 五、票

| 票 | 内容 | 依赖 |
|---|---|---|
| [01](issues/01-remove-graph-mirror.md) | 删代码与依赖 + 七处回填 + 决策 248 + 过 `make check` | — |

单票（Q4）：约 12 个文件但每处都是小改，拆两票是过度分票。

## 附：顺带修的一处既存瑕疵（非本批引入）

`docs/decisions.md` 第 259 行有一个**空行**把 246 与 247 隔开——GFM 表到此断开，
247（及本票的 248）会渲染成不带表头的裸行。决策 247 批（`189d3cc`）引入。
本票删掉这一行，让 247 与 248 都回到表内。
