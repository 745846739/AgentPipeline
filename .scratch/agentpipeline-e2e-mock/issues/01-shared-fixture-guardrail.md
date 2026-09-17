# 01: 共享 golden fixture 双向断言 + 补 project_analysis 路由

**What to build:** 用一份**提交进仓库**的 JSON fixture 把两份 mock 与 Rust 适配器钉在一起：
Node 侧断言自己的 SSE 构造产出与 fixture 逐字段一致，Rust 侧读同一份 fixture 喂给
`parse_chunk` + 聚合、断言解析出的结构符合预期。任一侧漂移即有一侧断言变红，**强制显式同步**。
同时补上 Rust mock 缺失的 `pseudo:project_analysis` 路由，关闭覆盖缺口。

**Blocked by:** None (can start immediately)

**Status:** done（2026-09-17）

**为什么是 fixture 而非收敛：** 抽 Rust helper 二进制需约 590 行改写 + 新二进制 + playwright
生命周期管理，换一个当前尚未发生的风险不划算（决策 151 修订已裁决）。而 fixture 方案
**不需要生成步骤、不需要新二进制、不需要生命周期管理**——只是一份 JSON 加两条各约十行的测试。
fixture 手工维护正是设计意图：两侧任一漂移都会红。

- [x] 新增跨语言共享 fixture（如 `tests/fixtures/e2e_mock_sse.json`），内容 = 各步骤形态
      （tool call / text / submit）对应的**期望 SSE 文本**，来源是当前两侧实际产出的字节
- [x] Rust 侧：in-crate 测试读该 fixture，逐条喂 `parse_chunk` + 既有聚合路径，断言得到的
      `StreamChunk` 序列结构（tool 名 / arguments / text / usage 字段）符合预期
      ——**分层说明**：in-crate 那条断言的是 `parse_chunk` 出来的**块序列**（驱动层聚合的输入契约）；
      驱动层那条**聚合路径**（分段 arguments 拼接、content 累加、usage 合并）仍由既有的 L2 用例
      `crates/core/tests/production_llm.rs::openai_stream_aggregates_content_tools_usage_and_cache`
      覆盖（真 HTTP mock + 真 `complete()`），本票不为它另建一条——**没有丢覆盖**，只是覆盖点不同层。
- [x] Node 侧：vitest 测试导入 harness 的 SSE 构造函数，断言产出与 fixture 逐字段一致
      （需把 `sseTool` / `sseText` 从模块私有改为可导出，`frontend/e2e/harness.ts:93-129`）
- [x] 断言必须比较**可解析的字节契约**而非意图：至少覆盖 `data: ` 前缀、`choices[0].delta.tool_calls[]`
      的 `index`/`id`/`function.name`/`function.arguments`、顶层 `usage.prompt_tokens` /
      `completion_tokens`、`[DONE]` 终止——这些是 Rust 适配器实际消费的字段
      （`crates/core/src/agent/providers/openai.rs:58-122`、`mod.rs:210-215`）
- [x] 补 `PSEUDO_MARKERS` 的第三条 `pseudo:project_analysis`
      （`crates/testkit/src/mock_llm.rs:204-207`），使 Rust 脚本化测试可覆盖该伪阶段
- [x] **实现时验证**：该 marker 是 `system.contains()` 字面匹配，而 `project_analysis` 的 persona
      可经 `persona_path` 覆盖（`executor.rs:1453-1457`、决策 87）；须确认内置 persona
      （`pseudo.rs:56-58`）走 `build_system_prompt` 后首句仍包含该 marker，若 `persona_path`
      分支无法覆盖则在票面记录为已知限制（不得静默假设）
- [x] `docs/testing.md` §9 的偏差注记更新：漂移风险从「由注记显式承担」改为「由 fixture 双向断言
      机械守住」，并注明 fixture 路径与更新方式
- [x] 全量质量闸门绿：`cargo fmt --check`、`clippy --workspace --all-targets -- -D warnings`、
      `cargo test --workspace`；前端 vitest / svelte-check / build；`make check-e2e` 全过
- [x] 该票**不改变** mock 的行为与 playwright 的两条冒烟场景；**不删除** Node mock
      ——**计数订正**：「两条」是 v1 期的数目（票 18 落地时只有两条冒烟）。如今前端
      E2E 已扩到二十一个 spec 文件，本次实跑 `make check-e2e` = **83 passed / 15 skipped**（全过）。
      验收按「全套 E2E 仍全过」执行——比原票面更严，不是更松。

## 交付（2026-09-17 实现）

**fixture：** `tests/fixtures/e2e_mock_sse.json`——三个用例 `tool_call` / `submit` / `text`，
每条含产出方（`producer`）、入参（`call`）与**期望 SSE 文本**（`sse`）。它是**手工维护**的
（无生成步骤，设计意图如此）：改 mock 形状时必须同时改 fixture 与三处断言。

**三处断言（任一侧漂移即变红）：**

1. Node 产出侧 `frontend/src/lib/e2e-mock-fixture.test.ts`（6 条）——import 构造函数产出后
   与 fixture 逐字段比对；
2. Rust 产出侧 `crates/testkit/src/mock_llm.rs::sse_helpers_match_the_shared_golden_fixture`
   ——`sse_tool` / `sse_text` 的产出与 fixture 一致（生产侧那一半）；
3. Rust 消费侧 `crates/core/src/agent/providers/openai.rs::shared_golden_fixture_parses_into_the_expected_chunk_sequence`
   ——逐条喂 `parse_chunk`，断言块序列（tool 名 / arguments / 文本 / usage / `[DONE]`）。

**比较口径（写进了 fixture 头注与两侧注释）：** 字节层只钉 **SSE 帧**（`data: ` 前缀 / 空行分帧 /
`[DONE]` 终止）与字段位置；**JSON 键序不是契约**（serde_json 按 BTreeMap 键序输出、JS 按插入序），
两侧都比对**解析后的结构**。工具调用 `id` 现场生成（Ulid / 时间戳随机数）→ 归一为
`call_fixture_1`；`function.arguments` 是「谁产出按谁键序」的不透明串且下游按 JSON 解析 → 比对
**解析后的对象**。

**实现时验证的三件事（都留了痕迹）：**

- 第一次跑 Rust 产出侧断言就红了，红在 `arguments` 的键序上（`serde_json` 的 `json!` 按 BTreeMap
  输出，fixture 是插入序）。这不是「测试写松一点」，而是它**证明了自己有牙齿**：真正该被钉住的
  是解析后的结构。故归一逻辑写进了两侧的 `frames()`，并在注释里说清为什么键序不算契约。
- 票面第 3 条「把 `sseTool` / `sseText` 从模块私有改为可导出」**就是加一个 `export`**：构造函数
  留在 `frontend/e2e/harness.ts` 原地，vitest 直接 import 它（跑在 **`@vitest-environment node`**
  下——默认的 jsdom 里 `import.meta.url` 是 http 形态，而 `harness.ts` 顶部就
  `fileURLToPath(new URL('.', import.meta.url))` 定位仓库根，会当场抛
  `TypeError: The URL must be of scheme file`；本用例不碰 DOM，切 node 环境零代价，照
  `frontend/src/lib/behavior-map.test.ts` 的先例）。fixture 定位同样走 `import.meta.url`。
  **代码审查改过一版**：最初把构造函数抽到新模块 `frontend/e2e/sse.ts`，评审指出 node 环境下
  直接 import harness 即可，已回退——少一个新模块，票面也就不用打偏离标记。
- 三处断言的**分层**（评审提的「既有聚合路径」那半句）：in-crate 那条钉的是 `parse_chunk`
  出来的块序列；驱动层聚合（分段 arguments 拼接 / content 累加 / usage 合并）由既有的 L2
  `production_llm.rs::openai_stream_aggregates_content_tools_usage_and_cache` 覆盖，未丢覆盖。

**`pseudo:project_analysis`：** `PSEUDO_MARKERS` 补第三条（与 Node 侧 `PERSONA_ROUTES` 的伪阶段
三条对齐）。**「实现时验证」项的结论**：走 `build_system_prompt` 后 marker 仍命中——新用例
`pseudo_markers_survive_prompt_assembly` 对三条 marker 各断言一次（用的是内嵌 persona 那条路）。
**已知限制（票面要求写明）：** 伪阶段配置写了 `persona_path` 时它覆盖内嵌 persona，marker
不在 system prompt 里，脚本化 mock **因此无法路由**该伪阶段；`persona_append` 不受影响（追加，
首句仍在）。这条限制写在 `PSEUDO_MARKERS` 的文档与 `docs/testing.md` §9 里。

**文档：** `docs/testing.md` §9 的偏差注记改口径（漂移风险由 fixture 机械守住 + fixture 路径与
更新方式 + `persona_path` 限制）；同时补进 §9 的契约行与 §11 的 testkit 行。

**一处附带修正（代码审查后）：** 让 vitest import `harness.ts` 的副作用是 **svelte-check 从此会
检查这个文件**（它此前不在 `tsconfig` 的 include 里，从没被类型检查过），而它藏着 6 处
`string | null` 收窄告警（`nodeKey`）。修法是**一行**：`key === null ? '' : …`——那些值的使用点
个个由 `key` 把关，空串读不到，行为不变（`make check-e2e` 全套复跑确认）。附带收益是
harness 的 SSE / 路由代码从此有类型门，而它正是本票要钉住的那份契约的产出方。

**未改动：** mock 的行为（字节形状一字未动）、playwright 的场景、Node mock 本身（未删除）。
