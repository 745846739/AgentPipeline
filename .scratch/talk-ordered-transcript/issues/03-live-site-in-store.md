# 03: 切页面不丢本轮已经收到的输出（在飞现场进 store）

**What to build:** 一轮动辄十几分钟，而在飞一轮的现场（乐观轮 / 正在产的步骤 / 跟一轮的锚点）与那条
`/foreman/stream` 连接此前都住在 `Talk.svelte` 的组件作用域里——切一下界面再回来，组件被销毁、现场随之不见，
而 SSE 没有回放，重连补得回的只是此后的增量。

**Blocked by:** None（已实现）

**Status:** done（决策 275）

- [x] `frontend/src/stores/talk.svelte.ts`：现场 + 连接 + 落地哨进 store，`init()`/`dispose()` 随 `App.svelte`
- [x] 页面只认班与接线：`talk.watch(id)`、`talk.bindRecalibrate(fn)`；换班次仍然清现场（决策 204③ / 220⑤ 口径不变）；「从没有班次到有班次」不算换班
- [x] 落地哨进 store（与「跟一轮」同寿），三支判据仍是纯函数 `resolveFollowOutcome`
- [x] **台账代次**（`ledgerEpoch`）：收尾可能发生在页面之外（切走之后那一趟 POST 才回来），store 据此请**在屏的那一页**重读一次台账——`settleTurn()` / `markLedgerStale()` 两声
- [x] 守卫：`delegation-scan.test.ts` 那条扫描面跟着判据换到 store（**换文件不放松**：三支、留字、清字禁令一字不改）
- [x] 单测：`stores/talk.test.ts` 10 条（切页不丢 / 换班清 / 闸门 / 三支收口 / 收尾两声）
- [x] e2e：`talk.spec.ts` 的「切走再回来」——三截滴出来，切到看板再切回来，
      断言「切之前收到的」与「切页面期间到达的」两截都还在（旧行为下回来只剩一句占位话）
