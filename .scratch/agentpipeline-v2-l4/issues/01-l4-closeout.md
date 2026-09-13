# 01: L4 收口（删死代码 + 文档对齐）

**What to build:** 落实决策 154——删除 L4 永不触达的第二级（`L4Action::BatchByNode` /
`SpawnSubAgents`）及其整个承载结构，把 v1 的 L4 语义统一为「强制压缩 → `pending(context_overflow)`」两级；
同步六处文档口径，使"分批与子代理拆分不做"这件事在代码、术语表、规格、测试文档、决策日志中读到的是同一件事。

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

**背景（决策 154）：** `plan_l4` 的返回值在生产代码里**唯一**的消费点是决定要不要打一句 warn
（`executor.rs:1853`）；`force_keep_recent_rounds`（=2）无消费者，`l4_pending_kind` 仅测试引用。
两个非 pending 变体永不可达，其中 `BatchByNode` 更是在 `plan_l4` 明确返回后立即被否决。
这不是"未实现"，是"写在那里但从不可能发生"。

- [ ] `L4Action` / `L4Plan` / `plan_l4` / `l4_pending_kind` 整组删除；`pending(context_overflow)`
      由 `executor.rs` 在压缩后仍超硬限时直接构造（行为不变：pending kind / reason 文案 / 游标不推进）
- [ ] `executor.rs` 中针对 `plan.action != PendingContextOverflow` 的 warn 与 `spawn_sub_agent`
      声明探测一并删除（`declared_tools.iter().any(...)` 那两行不再需要）
- [ ] `context.rs` 的单元测试块中依赖 `plan_l4` / `L4Action` 的断言删除，**保留**对
      「压缩后仍超限 → `pending(context_overflow)`」这一行为的等价断言（不许因删测试而丢覆盖）
- [ ] `operations.md` §12.13.3 阶梯改标为「v1 两级 / v2 三级」，并保留 v1 注记但改写为指向决策 154
- [ ] `operations.md` 子代理支持段（约 755–767）与 `agents.md:136` 的 `spawn_sub_agent` 说明改口径：
      不再是"开启后可用"的扩展工具，而是「v2 候选能力，v1 无实现，声明亦不生效」
- [ ] `glossary.md:24` Tool 条目删去「扩展工具：`spawn_sub_agent`（默认关闭）」
- [ ] `testing.md` §11 把 `context_overflow` 从"已关闭缺口"移回"有意保留的能力缺口"，并注明重开条件见决策 154
- [ ] `testing.md` §3 表的两处相关行（⑤ 超长工具结果注入 / ⑦ 子代理不脚本化）措辞与 §11 一致
- [ ] `decisions.md` 决策 26 / 45 / 105 / 148⑦ 四行补修订注记（AGENTS.md 要求冲突显式标号）
- [ ] 全量质量闸门绿：`cargo fmt --check`、`clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`
- [ ] 该票**不夹带行为变更**：`pending` 的触发条件、kind、动作集、E2E-22 结果均不变
