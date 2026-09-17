# AgentPipeline L4 收口执行计划

**Status:** done（2026-09-17，两票均交付）

来源：2026-09-13 针对「L4 兜底第二级（分批执行 / 子代理拆分）在 v1 未实现」的专访，
裁决落 **决策 154**。本目录承接该裁决的执行面：删除死代码、对齐六处文档、另立工具名校验票。

## 明确不立票的项

- **分批执行与子代理拆分的实现**：决策 154 裁决整体不做。重开条件见该决策末段（可核对：
  真实 `context_overflow` 落库 **且** 现有三动作救不回）。
- **`pending(context_overflow)` 的动作集与端点**：`split_task` / `model_override` / `cancel`
  三个端点均已接线（`crates/app/src/routes/tasks.rs:508`、`model-override`，L3 契约有用例），
  E2E-22 已走真实执行路径。非缺口。

## 依赖图

```
01 L4 收口（删死代码 + 六处文档）        02 未知工具名 fail fast
```

两票彼此独立，可任意顺序取用。

## 关键约束

- 每票验收须落在既有质量闸门内：`cargo fmt --check`、`clippy --workspace --all-targets -- -D warnings`、
  `cargo test --workspace`；涉及前端加跑 vitest / svelte-check / build。
- 改代码前先读 `docs/testing.md` §3 的可测试性接缝（决策 143，**现为五条**——决策 177 修订、194 改形状）与 §11 的已知缺口清单。
- 与决策冲突时必须显式标注决策编号（AGENTS.md）。本批次的权威裁决是决策 154。

## 交付（2026-09-17）

票 01 / 02 均 done，逐条核对与偏差写在各自的票据里。要点：

- 票 01：`L4Plan` / `L4Action` / `plan_l4` / `l4_pending_kind` 整组已删，pending 由 executor 直接构造；
  等价断言落在 L2（`context_overflow_ctx` 两条）。**票面第 5 / 6 条已被决策 172③ 推翻**（子代理是
  已实现的只读能力，不是「v1 无实现」），按 172③ 的口径落地；另核实阶梯第 1 级
  （`force_keep_recent_rounds = 2`）从来没有实现，文档已如实标注。
- 票 02：判据唯一（`client::is_known_tool_name` = 8 内置 + `spawn_sub_agent`），写入 400 / 启动
  fail fast（含存量配置），`tool_defs` 的静默丢弃改为报错；票面的「7 个内置」是 172③ 之前的数目。
