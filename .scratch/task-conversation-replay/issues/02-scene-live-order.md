# 02: 现场时间线的展示修正——直播流按到达序交织、思考自成一步、多次尝试分主次

**What to build:** 用户实机报障（2026-10-01）三件事：① 「工具执行一直在最上方」——工具回执
与增量各攒各的桶，所有 `liveTools` 整体排在正文之前；② 「思考没分段」——归约丢了
`conversation_delta.channel`（决策 244 加的声道），推理与回话按 role+agent 并成一整条；
③ 「子阶段多次重试展示不清晰且内容太长」——每个 attempt 一轮、名牌同名无次数、主次不分。
按**决策 359** 落地：归约发到达序 `seq`（增量与工具回执共用一只计数器）、现场归约按 seq
交织折步（对讲台 273 的步序规则）、start/end 合成一次调用、reasoning 折成默认收起的思考步
（ticker 复用 `thinkTicker`）、流式由 run 台账状态把关（跑完不亮光标）、同 `stage·node·agent`
的旧一代尝试整轮折起（同代并行子代理都是主）、名牌亮「第 N 次」。

**Blocked by:** None

**Status:** done（0e7e4a9，决策 359；2026-10-06 复核状态行订正）

## 落点

- `frontend/src/realtime/reduce.ts`：`LiveDelta.channel` / `LiveDelta.seq` / `LiveTool.seq`
  / `LiveTool.args` / `LiveTool.result` / `TaskDetailState.liveSeq`；`tool_event` 的
  start→end/error 按 run 合成。
- `frontend/src/lib/taskScene.ts`：`foldLiveStream`（交织折步）替换三只桶；`SceneStep.kind`
  增 `'thinking'`；`SceneTurn.attempt` / `SceneTurn.primary`；流式光标由 `status === 'running'`
  把关。
- `frontend/src/components/task/SceneTimeline.svelte`：思考折叠块、旧一代尝试的整轮折叠
  （snippet 共用轮体）、名牌次数章。

## 验收

- [x] 直播流按到达序交织：思考 → 工具 → 再思考 → 正文的发生序在步骤序里保真，
      工具回执不再整体堆顶（`taskScene.test.ts` 用 seq 交错 fixture 钉住）
- [x] reasoning 声道自成思考步，与正文分开折；连续思考并一条、换类另起；
      reasoning 收尾不摘收口话（思考不是回话）
- [x] 一次工具调用一条回执（start 与 end / error 合成，并行 run 不串台）；
      参数原文与结果进展开体（决策 301 的字段不再被现场丢掉）
- [x] run 落地（台账不再报 running）后光标下线，不再永远亮「正在说」
- [x] 多次尝试：旧一代整轮折起、内容一个字不删（点开全过程都在）；
      最新一代全幅；同代并行子代理（同 attempt）都是主；名牌带「第 N 次」
- [x] `npm test` 1102+ 全绿、`npm run check` 0 错误、`npm run build` 通过

**明确不做**（决策 359 的边界）：增量与落地消息的去重（在飞 run 的 `messages_json` 本就
到 attempt 结束才落库——落地接管与去重是本仓票 01 的地盘）；落地思考的回补（决策 244：
推理不落库）；现场轮按 branch 分组（决策 349 的任务级一叠维持）。

**来源：** 用户 2026-10-01 实机报障「看板任务阶段现场展示顺序有问题：工具执行一直在最上方，
思考没分段；子阶段多次重试的情况下展示不清晰且内容太长，需要分清主次展示」；裁决见
`docs/decisions.md` 决策 359。
