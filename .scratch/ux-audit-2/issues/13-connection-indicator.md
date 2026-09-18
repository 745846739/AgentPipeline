# 13: 详情页补实时断线指示

**叠:** A（不动规格）

**来源:** R2-15（代码）

**What to build:** 看板有断线横幅（`Board.svelte:171-173` 渲染「实时流已断开，正在重连…」），
**任务详情页没有**：`stores/taskDetail.svelte.ts:70-75` 构造 StreamManager 时只给
`onEvent` / `onRecalibrate`，**没给 `onStatus`**（对比 `board.svelte.ts:48-50`）。

后果：服务器重启或网络断掉后，详情的命令、会话增量、状态轨道**静静停止更新，页面看起来一切健康**。
更棘手的是 `runAllowedAction` 成功后要等 SSE 回执、30s 兜底（`taskDetail.svelte.ts:273-276`）——
流死时点「合入」会像什么都没发生一样等 30 秒，然后按钮悄悄复活。

**Blocked by:** None（can start immediately）

**Status:** done

- [x] `taskDetail` 的 StreamManager 接上 `onStatus`，详情页有可见的断线/重连指示
- [x] 断线时「等回执」的动作有明确反馈（不要静默等 30s）
- [x] 重连成功后指示消失，内容恢复（`onRecalibrate` 已有重载路径）
- [x] e2e：打开运行中的任务 → 停掉后端（或 `page.route` 掐断 `/events`）→
      断言出现断线指示；恢复后断言指示消失
- [x] e2e：断线时点一个拍板动作 → 断言有「流未连通」类反馈，而不是静默 30s

**边界.** 指示的措辞与看板保持一致（同一件事别两套说法）。

## 实施记录（2026-09-18）

**落点**

| 处 | 改动 |
|---|---|
| `src/stores/taskDetail.svelte.ts` | StreamManager 接上 `onStatus`，暴露 `connectionState`；新增 `actionNote`（成功后、流未连通时要说的话） |
| `src/routes/TaskDetail.svelte` | 断线时渲染 `.banner[role=status]`「实时流已断开，正在重连…（这期间本页不会自动更新）」——**与看板同一句话**，括号里只说本页的差别；`actionNote` 走同一条 banner 通道 |

**「不要静默等 30s」怎么落的**：`runAllowedAction` 成功之后本来就等 SSE 回执（30s 兜底），
现在若 `connectionState !== 'open'`，**立刻**把 `actionNote` 写上
（「实时流未连通：这次动作已经发出，界面要等重连之后才会更新（最长等 30 秒）」）——
用户按下之后马上有话说，而不是看着按钮转 30 秒。

**证据**：`frontend/e2e/ux2-resilience.spec.ts` ④——`page.route` 掐断
`**/tasks/*/stream`，断言断线指示出现；此时点「合入」→ 断言「实时流未连通」那句出现；
放行后断言指示**消失**，**并且**（评审补的）断言重连后真的走了一次全量对齐
（`onRecalibrate` → `GET /tasks/{id}` 的计数上涨）、正文还在——「横幅消失」不等于「内容回来了」，
票面验收写的是后者。
