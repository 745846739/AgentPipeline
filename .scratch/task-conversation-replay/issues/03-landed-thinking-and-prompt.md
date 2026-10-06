# 03: 流水线会话的思考落库 + 现场页签摆出阶段 prompt 与落地思考

**What to build:** 用户复看决策 359 成果（2026-10-01）追报三件事：① 「没有把思考过程展示
出来，至少需要把折叠的思考过程展示出来」——359 只把**直播里**的 `reasoning` 声道折成
思考步，run 落地或刷新那一刻增量被台账接管，思考整段消失；流水线的 `AgentResponse.reasoning`
在 `model_invoke.rs` 攒完即弃，连第二处落点都没有（值班长那侧早有迁移 0025 的
`thinking` 列先例）。② 「工具调用也要默认折叠」——复核为已实现（直播与落地同一形状的
`<details>` 默认收起），用户所见展开形态最可能是旧 bundle；本轮把「默认收起」用组件
测试钉死。③ 「阶段的 prompt 也要展示而不是只展示 llm 的回答」——prompt 快照两列
（决策 211②）库里一直有、API 也整行透传，但前端类型没接、归约不画；`messages_json`
只存工具环转录，所以「只有 llm 的回答」是真的。按**决策 360** 落地：会话表新增
`reasoning` 列（迁移 0038，多调用按到达序空行相连、四条落库路径都带上、字符账独立成册），
现场轮首插「阶段 PROMPT（两段原文，默认收起）→ 落地思考（默认收起）」两步，落地思考
在场时直播的 reasoning 声道不再折步。

**Blocked by:** None

**Status:** done（84f29b5，决策 360；2026-10-06 复核状态行订正）

## 落点

- `crates/core/src/storage/migrations/0038_conversation_reasoning.sql`：`reasoning` 列。
- `crates/core/src/storage/observability.rs`：`insert_conversation` /
  `insert_project_conversation` 带 `reasoning`（独立字符账、截断留标记）；行映射带上。
- `crates/core/src/types.rs`：`NodeConversation.reasoning`。
- `crates/core/src/pipeline/model_invoke.rs`：`AttemptTrace.reasoning` 攒账；成功 / 失败 /
  超窗 / 伪阶段四条落库路径带上。
- `crates/core/src/pipeline/subagent.rs`：`SubAgentSession.reasoning`，随子代理会话行落地。
- `crates/testkit/src/script.rs`：`.thinking(...)` / `push_subagent_thinking(...)` 脚本位
  （与步骤同队弹出，贴到对应响应的 `reasoning` 上）。
- `frontend/src/api/types.ts`：`NodeConversation` 补 `system_prompt` / `user_prompt` /
  `reasoning`。
- `frontend/src/lib/taskScene.ts`：`SceneStep.kind = 'prompt'`；轮首 prompt → 落地思考 →
  转录的步序；落地思考在场时过滤直播 reasoning 声道；过滤收 prompt 原文。
- `frontend/src/components/task/SceneTimeline.svelte`：阶段 PROMPT 折叠块（展开体两段
  分开摆）、落地思考复用既有思考折叠形状。

## 验收

- [x] 一次 run 的多次调用思考按到达序空行相连落库；不进 `messages_json`（决策 244
      红线：绝不回灌）；失败路径同样带（`executor.rs` 集成用例钉住）
- [x] 子代理的思考随**它自己的**会话行落地，不串进父会话
- [x] 完整会话端点透传 `reasoning` / `system_prompt` / `user_prompt`（`api_contract.rs`）
- [x] 现场轮首两步：阶段 PROMPT（摘要行「系统 N 字 · 用户 M 字」，展开两段分开摆）
      → 落地思考（「思考过程 N 字」）；两段快照全空、reasoning 为 null 的历史行不加空步
- [x] 落地思考在场时直播的 reasoning 声道不再折步（同一份思考只摆一遍）；
      直播中的 run 照旧折思考步（决策 244 的直播口径不动）
- [x] 轮级过滤命中 prompt 两段原文即留下
- [x] 工具回执默认收起用组件测试钉死（直播与落地同形状）
- [x] `cargo test`（core integration 472 / app integration 224）、`cargo clippy -D warnings`、
      `npx vitest run` 1111 全绿、`svelte-check` 0 错误

**明确不做**（决策 360 的边界）：思考的逐调用分步（与 assistant 消息的对位账是另一笔
工程）；直播增量与落地转录的正文去重（决策 359 留给本仓票 01 的地盘不动）。

**来源：** 用户 2026-10-01 复看决策 359 成果后的追加报障「上一轮修改了看板任务现场功能，
但是没有把思考过程展示出来，至少需要把折叠的思考过程展示出来，工具调用也要默认折叠，
还有阶段的 prompt 也要展示而不是只展示 llm 的回答」；裁决见 `docs/decisions.md` 决策 360。
