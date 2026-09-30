# 01: emit_* 与 OUTPUT_* 搬进 pipeline/events.rs

**What to build:** 决策 352——把 `emit_node_started` / `emit_tool_event` /
`finish_run_with_sse` / `project_or_err` / `CancelSignal` 之外的发射三件与
`OUTPUT_DESIGN_DOC` 等阶段产出常量从 executor.rs 搬进新文件
`crates/core/src/pipeline/events.rs`；executor / model_invoke / model_request 改为平依赖
events。model_invoke.rs:42–49 与 model_request.rs:36–43 的回边 import 全部改指向 events。

**Blocked by:** None

**Status:** ready-for-agent

- [ ] `pipeline/events.rs` 新建，发射函数 + OUTPUT_* 常量迁入；executor.rs 留转发或直接改引
- [ ] model_invoke.rs / model_request.rs 不再 import executor（grep `use super::executor` 归零）
- [ ] 头注写明「事件形状的唯一出口在 events」（决策 249 意图 + 352 修订）
- [ ] 验证：core 全量 + lint 绿；SSE 相关 e2e 照绿；diff 审查零行为变化
