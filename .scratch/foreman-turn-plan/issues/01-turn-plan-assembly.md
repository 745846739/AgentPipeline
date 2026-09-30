# 01: TurnPlan 组装裁定抽纯

**What to build:** 决策 356——新文件 `crates/core/src/pipeline/foreman/turn_plan.rs`：
给定（会话、cfg、env_mode 档位、转录、预算计数器快照）→ LlmRequest + 工具广告集 +
压缩/截断/预算判定。`respond_inner` 的组装段（档位解析、简报构建、工具广告集、
取史 + trim + 锚点注入、内联压缩判定、预算判界）改经 TurnPlan；respond_inner 保留
编排、SSE、落库、LLM 循环推进。头注照 `RequestPlan` 同款：不变量 1–N 写进接口。

**Blocked by:** foreman-split/issues/01

**Status:** ready-for-agent

- [ ] `turn_plan.rs`：组装裁定纯计算，预算计数器快照进、裁定出（压缩与否 / 截断到哪 /
      是否超预算），时钟从快照取不经侧门
- [ ] `respond_inner` 只剩：调 TurnPlan → 推进循环 → SSE → 落库；函数体显著缩身
- [ ] 单测：预算 / 压缩 / 截断裁定的边界（锚点注入后仍稳定前缀、超窗强制压缩、
      预算撞线）；复演决策 288 的墙钟场景（组装裁定可独立断言）
- [ ] 验证：foreman e2e 全族照绿（FakeAgent 消费方式不动）；core 全量 + lint 绿
