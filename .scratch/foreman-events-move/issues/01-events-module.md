# 01: emit_* 与 OUTPUT_* 搬进 pipeline/events.rs

**What to build:** 决策 352——把 `emit_node_started` / `emit_tool_event` /
`finish_run_with_sse` / `project_or_err` / `CancelSignal` 之外的发射三件与
`OUTPUT_DESIGN_DOC` 等阶段产出常量从 executor.rs 搬进新文件
`crates/core/src/pipeline/events.rs`；executor / model_invoke / model_request 改为平依赖
events。model_invoke.rs:42–49 与 model_request.rs:36–43 的回边 import 全部改指向 events。

**Blocked by:** None

**Status:** done

- [x] `pipeline/events.rs` 新建，发射函数 + OUTPUT_* 常量迁入；executor.rs 留转发或直接改引
- [x] model_invoke.rs / model_request.rs 不再 import executor（grep `use super::executor` 归零）
- [x] 头注写明「事件形状的唯一出口在 events」（决策 249 意图 + 352 修订）
- [x] 验证：core 全量 + lint 绿；SSE 相关 e2e 照绿；diff 审查零行为变化

**注记（收口口径）**：
- **grep 归零一项，model_request.rs 归零达成；model_invoke.rs 保留 `use super::executor::{project_or_err, CancelSignal, NodeOutput}`**——这三个是留守核与编排片的接线契约（取消信号 / 路由结论 / 项目行收口），不是事件形状；票面 What-to-build 本就写明搬的是「`project_or_err` / `CancelSignal` **之外**的发射三件」，checkbox 措辞过宽。两轴评审同判：非规格违背。搬运清单比票面多一项 `PIPELINE_AGENT_TYPE`——它是 emit 事件身份的同一处事实源（决策 244），留在 executor 反造新回边。
- **零行为变化核法**：被搬块与 `git show HEAD:executor.rs` 逐字节对照，仅两处有意差异——emit_tool_event 头注的宿主口径句、`finish_run_with_sse` 签名的 `RunLedger` 路径缩写（语义等价）；runner.rs:2492 的 `executor::emit_tool_event` 陈旧路径引用与 events.rs 一处「SSE 留在留守核」头注为评审发现，已随本票改口。
- 验证数：core lib 656 / core integration 463 / e2e 40 / app 224 全绿，fmt + clippy -D warnings 绿。
