# 01: 取证——`EdgeKind::Backtrack` 是死代码，删掉并补一条反向不变量用例

**Status: done（已实现）**

**Blocked by:** None（可立即开始）

**What to build:** `EdgeKind::Backtrack` 今天**在生产里也不可达**（不是「可达但没测」）。本票把
证据钉成测试、把死代码清掉、把两处「已修」的声称订正。**这一步先于票 02**——它的结论直接改变
票 02 的接口（`apply_edge` 从 6 臂变 5 臂）。

## 证据链（三条，缺一不可）

1. **`route()` 全仓只有一个调用者**：`advance_cursor`（`pipeline/executor.rs:2796`）。
   `grep -rn "pipeline::route(" crates/ tests/` 只此一处（`app/src/assets.rs:40` 那个是 axum 的
   `.route()`，不相干）。
2. **`advance_cursor` 只对跑完节点的游标调用，而 sync-check 被 `execute_node` 拦截**：
   `executor.rs:521-526` 的 `(Stage::SyncCheck, Node::Execute)` 臂注释写着「join 由 `advance_join`
   统一执行（决策 107），**游标永远不该指向这里**」，直接返回 `Error::Validation`。
   而 `advance_cursor` 的调用点在节点结果循环里（`:367`），输入来自 `execute_node`。
3. **`advance_join`（`executor.rs:2450`）在游标循环之外执行 join**，落点是
   `Store::backtrack_cursors` / `merge_cursors_to_develop`，自己写一条
   `TransitionTrigger::AutoResume` + 「sync-check 判定回溯」的流转行（`:2521-2526`）。
   它**根本不经过 `route()`**。

**结论**：`routes.rs:116-121` 的分支与 `apply_edge:3023-3046` 的臂是**防御性死代码**。
`routes.rs:419` 那个 L1 用例（`assert_eq!(route(&c, &ctx), EdgeKind::Backtrack)`）是拿**手工造的**
sync-check 游标直接调 `route()`——它证明这分支**算得对**，不证明它**到得了**。测试面里没有一处
造出 `stage = sync-check` 的游标行（测试对 `Stage::SyncCheck` 的引用全在 `list_runs_at` /
`stage_output_metadata`，即 advanc_join 落的 run 与产出，不是游标）。

**来历（改的时候要知道）**：`docs/testing.md:481` 记着 2026-09-12 那次「文档-实现对齐 pass」，
把「sync-check backtrack 用独立 `EdgeKind::Backtrack`」列为**修好的四条路由之一**。那次对齐
「修好」了一条本来就到不了的边——本票订正这条声称。

## 要做的四处

**① 删分支与其臂**：`routes.rs:116-121` 的 `(Stage::SyncCheck, Node::Execute)` 分支
（连同 `:46` 的 doc 引用）、`executor.rs:3023-3046` 的 `EdgeKind::Backtrack` 臂。

**② 从枚举里拿掉**：`types.rs:259-276` 的 `EdgeKind::Backtrack` 变体。

> **别动错人**：`types.rs:824` 的 `SyncDecisionKind::Backtrack` 是**另一个枚举**，`advance_join`
> 在用（`executor.rs:2521`/`:2641`），**活的**。`glossary.md:79` 的「Backtrack」词条
> （「回退到 architect-design 重新设计。由 sync-check 触发」）描述的是**行为**而非这个枚举值，
> **词条保留**——sync-check 判定回溯这件事没变，变的是它走 `advance_join` 而非 `EdgeKind`。

**③ 清图片**：`pipeline/graph.rs:90`（`EdgeKind::Backtrack` 进 successsor 表）、`:175-182`
（那个断言「backtrack 的落点是 architect-design.validate_input」的用例）。注意 `graph.rs` 整个
module 是**候选 3 的死镜像**（生产零调用方）——本票**只清 Backtrack 相关**，不顺手删整个文件，
也不删 `petgraph`，那是另一张票。

**④ 补反向不变量用例**（本票的重点产出）：一条**不变量测试**，断言「没有任何路径能把游标停在
sync-check」。落在 `crates/core/tests/integration/join_and_skip.rs`（那里已有 join 的用例），
照 `advance_join_runs_exactly_once_even_across_repeated_runs`（`executor.rs:2665`）的骨架：跑到
join 边界之后，断言所有游标行的 `(stage, node) != (SyncCheck, Execute)`，且 sync-check 只以
**run 行**（`agent_type = "system"`）存在。

这条用例**今天为真、删掉死代码之后仍为真**——它把本票的发现钉住：将来若有人「照文档」把
sync-check 做成占游标行的节点，它会变红。

**⑤ 订正文档**：`docs/testing.md:481` 那句「sync-check backtrack 用独立 `EdgeKind::Backtrack`」
改为如实说法（sync-check 的回溯由 `advance_join` 经 `SyncDecisionKind` 判定、
`Store::backtrack_cursors` 落库；不占游标行，故不经 `route()`）。

## 验收


- [ ] `EdgeKind::Backtrack` 变体已删；`apply_edge` 只剩 5 臂；`cargo build` 无 unused 警告
- [ ] `routes.rs` 的 `(SyncCheck, Execute)` 分支与其 L1 用例已删（注意 `routes.rs:416` 那个用例
      用的是 `SyncDecisionKind`，**保留**——它是活的那一个）
- [ ] `graph.rs` 的 Backtrack 相关两处已清，`petgraph` 与 `graph.rs` 其余部分**未动**
- [ ] 不变量用例落地：断言 join 之后无游标停在 sync-check，且 sync-check 只以 system run 存在
- [ ] `docs/testing.md:481` 的声称已订正
- [ ] `SyncDecisionKind::Backtrack` 与 `glossary.md:79` 的「Backtrack」词条**未被误删**
      （跑一遍 `cargo test -p agentpipeline-core join_and_skip` 与含 `backtrack` 的全部用例，
      确认 sync-check 回溯这条行为仍然有测试覆盖）

## 交付说明（2026-09-23）

**验收文字与现实对不上的两处（票面笔误，按实际计）**：

1. 「`apply_edge` 只剩 **5** 臂」——`EdgeKind` 删 `Backtrack` 之前是 **7** 臂
   （`NoOp` / `Retry` / `Pending` / `Next` / `KickbackDevelop` / `GotoTest` / `Backtrack`），
   删后 **6** 臂。决策 245 里写的就是「7 → 6」，以它为准；README 的「6 臂 → 5 臂」同源笔误。
2. 「`routes.rs:416` 那个用例用的是 `SyncDecisionKind`，保留」——`:411–423` 正是被删的
   `sync_check_backtrack_is_routable`，`:416` 在其中，无法既删用例又留它。括注的本意是
   **别把 `SyncDecisionKind::Backtrack` 当成 `EdgeKind::Backtrack` 一起删**——那个枚举
   （`types.rs:824`）与 `glossary.md:79` 的「Backtrack」词条**都保留了**，回溯行为仍由
   `advance_join` 那条路的用例覆盖（`cursor_lifecycle` 两条 + E2E-02 全绿）。
   另：`MetadataView::from_sync_decision` 因此失去唯一调用方，但它描述的是
   「sync 判定 → `passed`」这条**字段投影**、不属本次删除范围，故保留并改了 doc。
