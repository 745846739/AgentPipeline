# Spec: 直播增量的有界化（条数上限 + 落地即清）

2026-10-01 grilling 决议（用户逐轮「同意」）。缘起是用户报障「看板任务阶段现场慢」，
排查 `.scratch/scene-read-path-perf/` 时**顺带发现的另一条病**：任务详情与现场页签的
实时增量在 reducer 里只增不减，长 run 跑到后段每个 token 都触发一次全量重归约，前端追不上。
落点：决策 362，票 `issues/01`。

本票独立于 `scene-read-path-perf` 那批（那批治首屏 1.33 MB 载荷），但共享同一组实测底稿。

## 事实与诊断

### 无界累积的两处

`frontend/src/realtime/reduce.ts`：

- `conversation_delta` 每次无条件 `liveDeltas: [...state.liveDeltas, {...}]`（`:384`），逐 chunk
  追加、绝不合并（同 channel / role 也不合并）；
- `tool_event` 同理（`:404-441`），只在 start→end / start→error 时**原地替换**那一条（沿用
  start 的 `seq`），其余情况追加。

**除 `emptyTaskDetailState()`（`:217-238`，换任务 / 首次加载）外，没有任何分支重置、截断
或设上限**。静默 refetch 的 `load(id, true)` 还**有意保留**增量
（`frontend/src/stores/taskDetail.svelte.ts:114-122`，注释原文「保留流式增量（重载会话前不丢
当前 run 的实时文本）」）。

### 成本模型（关键事实）

`buildTaskScene`（`frontend/src/lib/taskScene.ts:357-451`）是 `SceneTimeline.svelte:61-71`
的 `$derived`——**每个到达的 delta 都从零重跑一遍全量归约**：

- `foldLiveStream`（`:258-300`）把**全量** deltas 与 tools 拼成 `items` 再 `.sort`（`:259-262`），
  主项是 O(N) 两趟 map + O(N log N) 排序；
- `buildTaskScene` 进入折叠前还按 `run_id` 分桶遍历全量（`:359-370`）。

**归约成本由「原始增量条数 N」驱动，与折出多少步无关**（本机复刻：N=36000 单步 53.9 ms
vs 500 步 51.4 ms；N=100000 单步 103.7 vs 多步 113.8）。步数只决定「每 token 要刷新多大的
DOM 文本节点」，**不改变 O(N log N) 的归约成本**。

实测（本机）：1 万条 ~4 ms、10 万条 54 ms、20 万条 ~103 ms。事件以 ~50/s 到达——服务端按
**provider chunk 粒度逐块 emit、不合流**（`crates/core/src/agent/providers/mod.rs:493-495`、
`:499-504`，`emit_delta` `:590-622` 无合并无节流；唯一的 250 ms 节拍是**落库**不是广播）。
按真实单请求 36k completion tokens 估算，尾部可到 130 ms+。

### 落地之后没有清除 → 双渲染

run 落地（attempt 结束、`messages_json` 整段入库）后，live 增量**仍在被消费**：
`taskScene.ts:418` 无条件 `foldLiveStream`、`:446-451` 的 `drafts` 无条件 `...stream`——只有
`reasoning` 声道在 `full.reasoning` 在场时被过滤（`:414-417`，决策 360）。也就是说，
**已落地 `msgSteps` 与 live `stream` 会同时摆进时间线**。

**热路径上的 markdown 缝**：`SceneTimeline.svelte:255-256` 对非流式 assistant 步走
`<MarkdownView>`，而 `streaming` 由 run 台账是否 `running` 把关（`taskScene.ts:421`）——一个
run 已停、但增量未被清时，其折叠出的 assistant 步就是 `streaming=false`，于是任何其他
delta 触发的重归约都会对**全量文本**重跑一次 `renderMarkdown`（`frontend/src/lib/markdown.ts`）。
「落地即清」正是关掉这条热路径的手段。

### 落地接管的匹配键

`messages_json` 里的元素（`ChatMessage`，`frontend/src/api/types.ts:230-236`）**不带 `run_id`**，
所以匹配键只能落在**会话行 / 摘要的 `run_id`** 上（`NodeConversation.run_id` `:240`、
`ConversationSummary.run_id` `:212`）。

- `TaskDetailState`（`reduce.ts:190-215`）**没有 `messages` 字段**，只有
  `conversations: ConversationSummary[]`；完整正文住在 store 的
  `conversationsFull: Record<number, NodeConversation>`（`frontend/src/stores/taskDetail.svelte.ts:52`）。
- 故**清空点只能在 store**，不在 reducer。
- 严格落地信号是 `conversationsFull[runId] !== undefined`（即 `conversationFor(runId)` 有值）。
  **不能只看摘要 `run_id` 出现**——票 03 的渐进填充会先到摘要、后到正文，只看摘要会在正文
  到手前把流式文本清空。

live 侧的 `run_id` 来源一致：`LiveDelta.run_id`（`reduce.ts:387`）、`LiveTool.run_id`（`:430`）
都来自 SSE 事件的 `run_id`（`types.ts:432-434`、`:461-463`），与 `insert_conversation` 的
`run_id`（`model_invoke.rs:989-1013`）同为 `runs.id`，1:1。

## 决议

1. **上限（票 01 前半）**：`liveDeltas` 与 `liveTools` **各设上限 N = 10000 条**，方向为
   **环形缓冲丢最早**（直播只关心尾部；「到顶拒收」会让流停在半截）。
   **当前正在长的那一步不豁免**——一豁免，36k tokens 那次超长单步（正是本票要修的形态）
   就绕过上限，等于没修。
2. **落地即清 + 去重（票 01 后半）**：按 `run_id` 逐条清——把 `liveDeltas` / `liveTools`
   过滤掉 `run_id ∈ conversationsFull` 的条目；触发点在 **store**，用严格信号
   `conversationsFull[runId] !== undefined`；只清已落地的 run，在飞的另一轮（决策 260 的双轮）
   不受影响，也不动 reducer 的 `emptyTaskDetailState()`。这条同时修掉上面的双渲染。
   **去重归属迁移**：决策 359 / 360 的「明确不做②」把「落地接管与去重」记在
   `task-conversation-replay/issues/01`，现迁入本票（该票无证据、已转 `needs-info`）；359 / 360
   的**展示口径**与「不做落地思考回补」等裁定不动。
3. **界面交代**：截断处显式一行「**更早的增量已省略**」，**非交互**——数据未落库、取不回来，
   不能复用 `MoreRow` 的「加载更多」按钮（那会撒谎）；措辞与决策 319 的「已省略前 N 条」
   对齐，落点在折叠出的步序列最前。
4. **`seq` 口径**：丢弃后**保留原 `seq` 值（前面留洞），不重编号**——`seq` 只用于排序
   （`taskScene.ts:259-262`），留洞无害；重编号要给每次截断加一趟 O(N) 减法，还把决策 359 的
   「`seq` 单调」偷换成「窗口内单调」，无收益。
5. **判据（可测）**：「**累积到 N 条后，单次 `buildTaskScene` 在本机 < 10 ms，且不再随累积
   上升**」；另有四条行为断言（见下）。

## 验收线

- 计数判据：累积到 N 条（含 N 以上）后单次 `buildTaskScene` 本机 < 10 ms，且不随累积上升。
  「N 以上」那一半由**构造**保证（进不了归约）；N 整档由 `frontend/src/lib/taskScene.bench.ts`
  的「在飞增量到窗口上限」一档记录（本机 10000 条 mean ≈ 4.8 ms）——照仓里口径
  **只记录、不设闸**（毫秒在 CI 上必然抖动，做成断言就是引进一条 flaky 门，`docs/testing.md` §1 / §10）。
- 行为单测：①累积超上限仍**丢最早**（尾部保留）；②截断处**省略行在场**且不可交互；
  ③某 run **落地后其增量消失**，同时**在飞的另一 run 不受影响**；④截断后步序仍**按 `seq`
  交织**（「先想 → 查 → 再想」在窗口内保真）。四条各有用例：①`reduce.test.ts`、
  ②`taskScene.test.ts` + `SceneTimeline.test.ts`、③`taskDetail.test.ts`、④`taskScene.test.ts`。

## 与既有决策的关系

- **决策 359 / 360**：展示口径（直播按到达序交织折步、思考自成一步、多次尝试分主次、
  落地思考接管直播声道）**不动**；只把「落地接管与去重」的落点从 task-conversation-replay 01
  迁到本票（决策 362 记录）。
- **决策 319**（长列表窗口化）：省略措辞沿用其「已省略前 N 条」口径，但**不用**其「加载更多」
  按钮——本票丢掉的增量**不可取回**。
- **决策 312 / 275**：`seq` 只做去重、禁 SSE 回放与 localStorage 回填——本票不碰。
- **决策 260**：同一班可并行两轮（值守轮 + 人的轮）——「落地即清只清已落地的 run」据此。

## 明确不做

- **当前步豁免上限**：会放过超长单步，正是本票要修的形态。
- **步粒度上限 / 增量折步重构**：事实已证步粒度不降归约成本，且要把归约改成增量折步，
  收益不抵改动面。
- **虚拟滚动、把 `DEFAULT_PAGE` 降更小**：决策 319 的边界，与决策 349 / 359 的「整条时间线
  摆得开、直播贴底」语义冲突大。
- **SSE 回放 / localStorage 在途回填**：决策 275 原样保持。
- **修订 359 / 360 的现场展示口径**：本票只做清理与上限，不改任何归约判断的形状。

## 票

- `issues/01-unbounded-live-deltas.md` —— 上游计数 + 落地即清 + 省略行 + `seq` 口径
  （`Status: done`，已实现，决策 362）
