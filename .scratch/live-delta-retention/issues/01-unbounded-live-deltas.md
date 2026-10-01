# 01: `liveDeltas` / `liveTools` 只增不减——长任务把归约拖垮

**What to build:** 任务详情与现场页签的实时增量在 reducer 里**无界累积**：

- `conversation_delta` 每次 `[...state.liveDeltas, {...}]`（`frontend/src/realtime/reduce.ts:384`）；
- `tool_event` 同理（`:409-441`）；

没有任何上限，也没有 event 分支会重置——只有 `emptyTaskDetailState()` 会清（即换任务），
而静默 refetch（`load(id, true)`）**保留**增量（`frontend/src/stores/taskDetail.svelte.ts:126-135`
那个 `...this.state` 展开就是有意为之）。

代价随累积量近似线性上升（实测，本机）：1 万条 ~4 ms、10 万条 54 ms、20 万条 ~103 ms
（外推），而事件以约 **50/s** 到达（按 provider chunk 粒度 emit，见
`crates/core/src/agent/providers/mod.rs:494`，服务端不合流）。也就是说长 run 跑到后段，
每个到达的 token 都要触发一次几十到上百 ms 的全量重归约，前端追不上。附加代价：reducer
的数组复制本身也随 n 增长（10 万条 1.6 ms）。

**这不是本批（`.scratch/scene-read-path-perf/`）的痛点**——用户报的是首屏 3~4 秒，已归因
为未压缩的 commands 载荷。本票是**在同一轮排查里顺带发现的另一条病**，故另立。

**先决（已裁，决策 362）：** 丢弃语义由一轮 grill 裁决（2026-10-01），五问本为修法前提，
逐条答复如下：

- 丢最早的（环形缓冲）还是到顶就不再接收？
- 界线按条数、按字节，还是按 run？
- 与决策 359（直播按到达序 `seq` 交织折步）怎么对齐——丢弃后 `seq` 是否留洞、「先想 → 查
  → 再想」的序在截断处还保不保真？
- 与决策 312 的落库口径怎么对齐（落地接管后这些增量就没用了，是不是「run 落地即清」就够）？
- 丢掉的增量在界面上怎么交代（是静默截断还是显式留一行「更早的增量已省略」）？

→ **以上五点已逐条裁定**：丢最早（环形缓冲）、界线取条数（N = 10000）、`seq` 留洞不重编号且
窗口内序保真、「落地即清」按 `run_id` 且并入本票、界面显式非交互省略行。答案见
[spec.md](../spec.md)「决议」与决策 362。

**Blocked by:** 无——丢弃语义已由决策 362（2026-10-01）裁定，设计定形见
[spec.md](../spec.md)。

**Status:** done（已实现，决策 362，2026-10-01；设计定形见 [spec.md](../spec.md)）

- [x] 先决：grill 裁决丢弃语义——**已裁**（决策 362）：上限 **N = 10000 条**、`liveDeltas` /
      `liveTools` 各一、**环形缓冲丢最早**、**当前步不豁免**；丢弃后 `seq` 保留原值（留洞）
      不重编号；截断处**非交互**省略行；落地即清按 `run_id`、**落点在 store**
- [x] 出一组可测的判据——见 [spec.md](../spec.md)「验收线」（计数判据 + 四条行为断言）
- [x] 出 spec 后转 ready-for-agent——spec 已出
- [x] 实现：上限（`reduce.ts::LIVE_WINDOW_LIMIT` 环形缓冲 + `liveDroppedRuns` 标记）+
      落地即清（`taskDetail.svelte.ts::clearLandedLive`，按 `run_id`、严格信号
      `conversationsFull[runId] !== undefined`）+ 省略行（`SceneTimeline.svelte` 的非交互
      `.omitted` 行）；判据见 spec，单测见 `reduce.test.ts` / `taskDetail.test.ts` /
      `taskScene.test.ts` / `SceneTimeline.test.ts`

**来源：** `.scratch/scene-read-path-perf/spec.md`「另立票」一节；实测数字见该 spec 的
「客户端侧的实测与推断」。
