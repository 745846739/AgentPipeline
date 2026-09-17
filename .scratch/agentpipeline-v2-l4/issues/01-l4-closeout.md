# 01: L4 收口（删死代码 + 文档对齐）

**What to build:** 落实决策 154——删除 L4 永不触达的第二级（`L4Action::BatchByNode` /
`SpawnSubAgents`）及其整个承载结构，把 v1 的 L4 语义统一为「强制压缩 → `pending(context_overflow)`」两级；
同步六处文档口径，使"分批与子代理拆分不做"这件事在代码、术语表、规格、测试文档、决策日志中读到的是同一件事。

**Blocked by:** None (can start immediately)

**Status:** done（2026-09-17）

**背景（决策 154）：** `plan_l4` 的返回值在生产代码里**唯一**的消费点是决定要不要打一句 warn
（`executor.rs:1853`）；`force_keep_recent_rounds`（=2）无消费者，`l4_pending_kind` 仅测试引用。
两个非 pending 变体永不可达，其中 `BatchByNode` 更是在 `plan_l4` 明确返回后立即被否决。
这不是"未实现"，是"写在那里但从不可能发生"。

- [x] `L4Action` / `L4Plan` / `plan_l4` / `l4_pending_kind` 整组删除；`pending(context_overflow)`
      由 `executor.rs` 在压缩后仍超硬限时直接构造（行为不变：pending kind / reason 文案 / 游标不推进）
- [x] `executor.rs` 中针对 `plan.action != PendingContextOverflow` 的 warn 与 `spawn_sub_agent`
      声明探测一并删除（`declared_tools.iter().any(...)` 那两行不再需要）——该参数随之从
      `enforce_context_budget` 的签名里删掉（唯一用处就是那次探测）
- [x] `context.rs` 的单元测试块中依赖 `plan_l4` / `L4Action` 的断言删除，**保留**对
      「压缩后仍超限 → `pending(context_overflow)`」这一行为的等价断言（不许因删测试而丢覆盖）
- [x] `operations.md` §12.13.3 阶梯改标为「v1 两级 / v2 三级」，并保留 v1 注记但改写为指向决策 154
- [x] `operations.md` 子代理支持段（约 755–767）与 `agents.md:136` 的 `spawn_sub_agent` 说明改口径：
      不再是"开启后可用"的扩展工具，而是「v2 候选能力，v1 无实现，声明亦不生效」
- [x] `glossary.md:24` Tool 条目删去「扩展工具：`spawn_sub_agent`（默认关闭）」
- [x] `testing.md` §11 把 `context_overflow` 从"已关闭缺口"移回"有意保留的能力缺口"，并注明重开条件见决策 154
- [x] `testing.md` §3 表的两处相关行（⑤ 超长工具结果注入 / ⑦ 子代理不脚本化）措辞与 §11 一致
- [x] `decisions.md` 决策 26 / 45 / 105 / 148⑦ 四行补修订注记（AGENTS.md 要求冲突显式标号）
- [x] 全量质量闸门绿：`cargo fmt --check`、`clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`
- [x] 该票**不夹带行为变更**：`pending` 的触发条件、kind、动作集、E2E-22 结果均不变

## 交付（2026-09-17 实现）

**代码：** `crates/core/src/agent/context.rs` 删掉整个 L4 计划组（含 `Stage` / `Node` 两个随之失效的
import）；`crates/core/src/pipeline/executor.rs::enforce_context_budget` 直接构造
`PendingReason::new(PendingKind::ContextOverflow, …)`，warn 与探测分支删除；`subagent.rs`
模块头里那句「`agent/context.rs` 的 `plan_l4` 一行未动」改成现状。

**等价覆盖（不丢的那条断言）：** 落在 L2（**这两条用例是既有的，本票没有新增**——
本票交付的是「删掉三条 L1 断言之后覆盖仍在」的核实，而不是新写的替代用例）——`crates/core/tests/executor.rs::context_overflow_ctx`
造成真超限现场（provider 窗口 1000 → 硬限 900，一次大块元数据越过），两条用例分别钉住
pending 的 kind（`…_starts_from_an_empty_conversation`）与「退出路径补写会话行」
（`context_overflow_path_writes_a_conversation_row`）。`context.rs` 里保留谓词层断言
（`compact_and_hard_limit_predicates`），并在原位留了一条指向 L2 的注释——避免下一个人以为
「L4 没有用例了」。

**与票面不一致的两处（就地更正，未按票面字面执行）：**

1. **票面第 5 / 6 条已被决策 172③ 推翻**：票写于 2026-09-13，当时 154② 裁定口径为
   「`spawn_sub_agent` = v2 候选能力，v1 无实现，声明亦不生效」。**决策 172③ 重开了这一条**：
   子代理已实现为**只读子代理**（工具集固定 `read_file` / `list_dir`、不继承阶段声明工具、
   不再派发），`spawn_sub_agent` 是**真的可用**的扩展工具（需阶段显式声明）。故：
   - `glossary.md` Tool 条目**不删**它的行，而是写清「只读子代理（决策 172③），需阶段显式声明，
     不在 `BUILTIN_TOOLS` 里」（原本的「默认关闭」措辞已由 172③ 落地时改掉）；
   - `agents.md` 与 `operations.md` 的子代理段**已经是** 172③ 的口径（不是「v1 无实现」），
     本次只补「它不参与 L4 判定、executor 里已无任何『声明了子代理吗』的探测」这一句。
   - 决策 26 / 45 两行在 172③ 落地时就已补注记，本次不再重复改写；**154 行**补了交付注记
     （见下）。
2. **第 1 条里的「强制压缩（`force_keep_recent_rounds` 降为 2）」其实从来没有实现**：全仓只有
   一处压缩调用（`executor.rs` 的 `compact_messages_from`，`keep_recent_rounds` 取设置值），
   `force_keep_recent_rounds` 只被测试引用过。故 §12.13.3 的阶梯按**如实**写法标注：
   v2 三级（设计台阶）vs v1 两级（**按轮压缩 → pending**），并点明第 1、2 级在 v1 均无实现
   （第 2 级是决策 154 明确不做，第 1 级随本次死代码清理一并消失）。决策 154 行同时记录了这一核实。

**决策日志：** 105 行补「L4 的两级补救 v1 均不实现」（与它的 `split_task` 留白同源）；148⑦ 行标注
「已由 172③ 重开为**可**脚本化，而 `pending(context_overflow)` 的收口不变」；154 行补交付注记
（含上面第 2 点的核实结论）。26 / 45 行已有 172③ 注记，未改。

**文档改动位置：** `docs/operations.md` §12.13.3（阶梯 + 两条注记）、`docs/testing.md`
§3.2 ⑤⑦ / §5 上下文行 / §6 L2 行 / §11 缺口清单、`docs/decisions.md` 105 / 148 / 154。
