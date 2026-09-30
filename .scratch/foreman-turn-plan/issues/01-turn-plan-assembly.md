# 01: TurnPlan 组装裁定抽纯

**What to build:** 决策 356——新文件 `crates/core/src/pipeline/foreman/turn_plan.rs`：
给定（会话、cfg、env_mode 档位、转录、预算计数器快照）→ LlmRequest + 工具广告集 +
压缩/截断/预算判定。`respond_inner` 的组装段（档位解析、简报构建、工具广告集、
取史 + trim + 锚点注入、内联压缩判定、预算判界）改经 TurnPlan；respond_inner 保留
编排、SSE、落库、LLM 循环推进。头注照 `RequestPlan` 同款：不变量 1–N 写进接口。

**Blocked by:** foreman-split/issues/01

**Status:** done（已实现，决策 356）

- [x] `turn_plan.rs`：组装裁定纯计算，预算计数器快照进、裁定出（压缩与否 / 截断到哪 /
      是否超预算），时钟从快照取不经侧门——`TurnPlan::assemble(TurnFacts) -> TurnPlan`
      是纯函数（不读库、不读盘、不调模型）；两道门 `check_window_budget` / `compact_forced`
      与成本门 `cost_verdict` 同址，13 条单测（见注记 ①）
- [x] `respond_inner` 只剩：调 TurnPlan → 推进循环 → SSE → 落库；函数体显著缩身——
      组装段 212 行 → 33 行（组装调用 + 取转录 + 建 tooling），删掉五个搬家方法
      （`turn_capacity` / `compact_inline_if_over_budget` / `compact_inline_forced` /
      `round_start_of` / `tool_defs` / `system_prompt` / `power_discipline`），
      `turn_capacity` 收成一个只取数的 `turn_window`
- [x] 单测：预算 / 压缩 / 截断裁定的边界（锚点注入后仍稳定前缀、超窗强制压缩、
      预算撞线）；复演决策 288 的墙钟场景（组装裁定可独立断言）——
      `injections_keep_the_history_prefix_byte_identical`（稳定前缀逐字相同）、
      `an_over_window_transcript_is_compacted_into_an_anchor_round`（超窗即压）、
      `the_cost_line_stops_a_watch_turn_and_only_warns_a_human_one`（预算撞线两档）、
      `a_window_the_provider_never_registered_skips_both_budget_gates`（不臆造窗口）
- [x] 验证：foreman e2e 全族照绿（FakeAgent 消费方式不动）；core 全量 + lint 绿——
      foreman 集成 **136 条过**（FakeAgent 那条整轮脚本一字未动）；`cargo test --workspace`
      与 clippy 见提交闸门

**注记（留给后来者）**：

- ① **两处刻意的「不改」**：组装要 `Settings`（容量算术的两个比例 + `keep_recent_rounds`）
  ——与 [`RequestPlan`] 借 `ctx.settings` 同一姿态，不算侧门；取数（简报 / 历史 / 人格
  读盘 / provider 窗口 / 摘要调用）全留在 `respond_inner`。「不管推进」（LLM 循环、
  工具往返、SSE、落库、`LiveTurn`、停钮）是决策 356 明文的不做项。
- ② **容量算术为什么只能在计划里算**：`estimate_context_capacity` 要 `system_prompt`
  （系统段的 token 预留），而 `system_prompt` 是组装本身的产物。故 `respond_inner`
  只取 `model_window`（provider 行的 `context_window`），容量与 80% 触发线由
  `TurnPlan::assemble` 派生——这也让「超窗强制压缩」第一次能被纯函数断言。
- ③ **一次调用换了个位置，语义没换**：原 `turn_capacity` 里 `context_window == 0 → None`，
  现在收成 `turn_window` 的 `(provider.context_window > 0).then_some(...)`，同一个判据。
- ④ **`turn_window` 里仍带 fallback**（首个启用 provider），而请求那条 `provider_id` 不带
  ——这是原样保留的不对称：窗口要找到**行**才读得到，请求那条由适配器兜底回落。
  两处都还在，没有合并（合并会改行为）。

## Comments

- 2026-09-30 实施完毕（决策 356）。两轴评审各一轮，改动如下：
  - **Mysterious Name（评审指出的最强一条）**：`TurnPlan.question` 装的是**合并轮全文**
    （快照 + 问题），与 `TurnFacts.question`（裸问题）同名不同义——搬运时恰好把原
    `respond_inner` 里那句告警（「别名成 question 会名不副实」）丢掉了。已改名
    `merged_round`，`TurnFacts.question` 保留原名，两者语义在字段注里写明。
  - **公开面收窄**：`env_mode` / `available_tools` 改私有（只有 `assemble` 自己调，编排侧
    吃的是 `plan.env_mode` / `plan.available` 这两个字段）；`FOREMAN_INLOOP_COMPACT_RATIO`
    改私有常量。对外只剩 `TurnPlan` / `TurnFacts` / `CostVerdict` / `cost_verdict`。
  - **Ordering 一处回退**：人格读盘原本排进 `compact_history`（会调摘要）之后，坏
    `persona_path` 会先烧掉一次摘要调用才失败；已挪回 provider 解析之后、取史之前，
    与从前的失败时机一致。
  - 头注补了一节「这是一次搬家，不是改口径」。
- **评审的 Spec 轴一条未落实项（如实记）**：票面 build 行写「取史 + trim + 锚点注入
  改经 TurnPlan」，实际落地是「**trim 的结果与锚点正文**经 `TurnFacts` 进计划，注入的
  位置 / 标记 / 顺序由计划裁定，`trim_history` 与摘要调用（`compact_history`）留在编排
  侧」——理由见注记 ①（取数不做，裁定做）。若后来者认为 `compact_history` 也该搬，
  那是**宽边界**，与决策 356 的明文不做项冲突，得先改决策。
