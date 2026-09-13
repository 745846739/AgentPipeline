# 01: 共享 golden fixture 双向断言 + 补 project_analysis 路由

**What to build:** 用一份**提交进仓库**的 JSON fixture 把两份 mock 与 Rust 适配器钉在一起：
Node 侧断言自己的 SSE 构造产出与 fixture 逐字段一致，Rust 侧读同一份 fixture 喂给
`parse_chunk` + 聚合、断言解析出的结构符合预期。任一侧漂移即有一侧断言变红，**强制显式同步**。
同时补上 Rust mock 缺失的 `pseudo:project_analysis` 路由，关闭覆盖缺口。

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

**为什么是 fixture 而非收敛：** 抽 Rust helper 二进制需约 590 行改写 + 新二进制 + playwright
生命周期管理，换一个当前尚未发生的风险不划算（决策 151 修订已裁决）。而 fixture 方案
**不需要生成步骤、不需要新二进制、不需要生命周期管理**——只是一份 JSON 加两条各约十行的测试。
fixture 手工维护正是设计意图：两侧任一漂移都会红。

- [ ] 新增跨语言共享 fixture（如 `tests/fixtures/e2e_mock_sse.json`），内容 = 各步骤形态
      （tool call / text / submit）对应的**期望 SSE 文本**，来源是当前两侧实际产出的字节
- [ ] Rust 侧：in-crate 测试读该 fixture，逐条喂 `parse_chunk` + 既有聚合路径，断言得到的
      `StreamChunk` 序列结构（tool 名 / arguments / text / usage 字段）符合预期
- [ ] Node 侧：vitest 测试导入 harness 的 SSE 构造函数，断言产出与 fixture 逐字段一致
      （需把 `sseTool` / `sseText` 从模块私有改为可导出，`frontend/e2e/harness.ts:93-129`）
- [ ] 断言必须比较**可解析的字节契约**而非意图：至少覆盖 `data: ` 前缀、`choices[0].delta.tool_calls[]`
      的 `index`/`id`/`function.name`/`function.arguments`、顶层 `usage.prompt_tokens` /
      `completion_tokens`、`[DONE]` 终止——这些是 Rust 适配器实际消费的字段
      （`crates/core/src/agent/providers/openai.rs:58-122`、`mod.rs:210-215`）
- [ ] 补 `PSEUDO_MARKERS` 的第三条 `pseudo:project_analysis`
      （`crates/testkit/src/mock_llm.rs:204-207`），使 Rust 脚本化测试可覆盖该伪阶段
- [ ] **实现时验证**：该 marker 是 `system.contains()` 字面匹配，而 `project_analysis` 的 persona
      可经 `persona_path` 覆盖（`executor.rs:1453-1457`、决策 87）；须确认内置 persona
      （`pseudo.rs:56-58`）走 `build_system_prompt` 后首句仍包含该 marker，若 `persona_path`
      分支无法覆盖则在票面记录为已知限制（不得静默假设）
- [ ] `docs/testing.md` §9 的偏差注记更新：漂移风险从「由注记显式承担」改为「由 fixture 双向断言
      机械守住」，并注明 fixture 路径与更新方式
- [ ] 全量质量闸门绿：`cargo fmt --check`、`clippy --workspace --all-targets -- -D warnings`、
      `cargo test --workspace`；前端 vitest / svelte-check / build；`just frontend-e2e` 两条仍全过
- [ ] 该票**不改变** mock 的行为与 playwright 的两条冒烟场景；**不删除** Node mock
