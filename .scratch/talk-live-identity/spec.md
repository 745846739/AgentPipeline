# Spec: 对讲台在飞状态机重整（身份统一 / 接上形态 / 折叠态 / 排队发送 / 操作台渲染）

2026-09-29 对五项使用问题的 grilling 收口决议（用户逐条拍板）。问题来源：①对讲台出现
非本次会话的内容；③切页返回后流式显示/折叠/锁定走样；②对讲台渲染格式但操作台不渲染。

## 根因（诊断结论，2026-09-29 实测）

- **串台**不是从 SSE 进来的（写侧身份 tagging 与前端两道班次守卫均核对无误），是从
  `Talk.svelte::reload()` 的**过期回包**进来的：两次 await 之后无「还在不该班次吗」比对，
  回包无条件整屏写入 + `talk.watch(landed)` + 改 URL。并发 reload 是常态——
  `ledgerEpoch` 住 store、只增不清零，而页面的 epoch effect 假设「首屏为 0」，
  于是每次挂载 onMount 的 reload（目标=URL）与 effect 的 reload（目标=`talk.sessionId`）
  必然并发；`/talk` 与 `/talk/watch` 共用 store 更使目标分叉。最后写者把整屏拖进另一班次。
- **接上路径走样**：决策 275 把现场数据搬进 store，但三根渲染支柱没跟着搬——
  组件本地 `generation` 把活过换页的 `send()` 回包误判成「切走了」（现场倒掉、输入框不清）；
  `stream.streaming` 全仓唯一点火点在 `send()`，`syncFollowing` 接手永不点亮 →
  假「流断了」、无光标、无贴底跟随；决策 312 半截行使渲染退化成「半截行落地式 + 尾巴成轮」，
  POST 在途时返回还会乐观轮与台账行重复；折叠三表组件本地 + 键空间不稳定（`live-s<i>` vs
  `m<id>-s<i>`），回来全合拢。
- **渲染分岔**：值班长回话（`kind='fm'`）走 `MarkdownView`，操作台各轮型（提议 summary /
  提问 question / 急停 message）全是纯文本插值——而这些字段的作者多半是模型。

## 决议（用户 2026-09-29 拍板）

1. **身份统一**（废组件 `generation`）：store 的 `sessionId` 是唯一身份；
   `reload()` 与 `send()` 同一条纪律——每个 await 之后比对「发起时的目标 ≠
   `talk.sessionId` 即丢弃」。守卫同源，少一份要同步的状态。
2. **接上路径与发送中不可区分**：`syncFollowing` 接手时点亮 `streaming`；
   快照里的 `in_flight` 半截行与 `seq > seq0` 尾巴**拼成一条 live 轮**渲染。
3. **折叠三表**（`receiptOpen` / `thinkingOpen` / `toolOpen`）搬进 talk store，
   键随轮稳定（锚定台账行 id 或轮语义键，不用下标）；跨页面存活、不跨刷新
   （决策 217 边界不动，不进 localStorage）。
4. **排队发送**（ZCode 式）：在飞时输入框解锁，发出即入队；**允许多条**；
   队列**可见、可编辑、可撤回**；队列住 store；当前轮收口自动发出；
   **死轮/中断扣住等确认**（不自动照发）；值守轮插进来不算收口、不触发下一句。
5. **操作台渲染**：提议 summary / 提问 question / 急停 message 等操作台轮型文本
   统一过 `MarkdownView`（「模型不写 markdown」是无法执行的纪律）。

## 明确不做

- 存量 22 条决策 286 前的【值守播报】行：不动（历史审计行）。
- `record_interrupted_turn` 落「最近活动的班次」语义：不动。
- 决策 260 / 275 / 274 / 217 的既有口径只做显式编号修订，不推翻。
