# e2e mock 契约：共享 golden fixture 双向断言 + 补 project_analysis 路由

来源：2026-09-13 针对决策 151 mock 偏差的专访（裁决已写入 `decisions.md` 决策 151 行的
**2026-09-13 修订**）。本目录承接该裁决的执行面。

## 背景

决策 151 要求「复用 E2E harness、不维护独立 mock server」。票 18 未达成——playwright 进程（Node）
无法直接调用 Rust 的 `testkit::MockLlm`，于是在 `frontend/e2e/harness.ts` 自写了 Node 侧
OpenAI 兼容 SSE mock。偏差已由 `docs/testing.md` §9 的注记显式记录并承担风险。

**风险的真实形态（专访中核实）：** 两份 mock **都不靠身份字段路由**——`build_body`
（`crates/core/src/agent/providers/openai.rs:30-56`）只发 `model` / `stream` / `messages` /
`tools` / `temperature` / `max_tokens`，`agent_type` 存在于 `RunContext` 但**从不上线**（仅落库记录）。
两侧都靠**子串匹配 system prompt 的 persona 首句**反查节点。因此漂移源是两边各自维护的
「persona 首句 → 节点」字面表，而这份表**没有任何自动检查**：改一处 prompt 模板而不同步另一侧，
Node mock 会静默回「脚本已结束」文本、任务卡在断言前的超时（响，但不指根因）；usage 字段漂移则
**完全静默**（token 归零，无断言）。

**另有一处功能面不对称：** Rust `from_script` 的 `PSEUDO_MARKERS`
（`crates/testkit/src/mock_llm.rs:204-207`）只有 `conflict_check` / `validator_cross_check` 两条，
而 Node 侧 `PERSONA_ROUTES`（`frontend/e2e/harness.ts:68-84`）有第三条 `pseudo:project_analysis`。
即 Rust mock 存在**覆盖缺口**：基于脚本的 Rust 测试无法覆盖 `project_analysis`
（收尾票 10 的产出）。

## 依赖图

单票。

## 关键约束

- 验收须落在既有质量闸门内：`cargo fmt --check`、`clippy --workspace --all-targets -- -D warnings`、
  `cargo test --workspace`；前端加跑 vitest / svelte-check / build，以及 `just frontend-e2e`。
- 与决策冲突时必须显式标注决策编号（AGENTS.md）。权威裁决是决策 151 的 2026-09-13 修订。
