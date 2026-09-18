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

**Status:** open

- [ ] `taskDetail` 的 StreamManager 接上 `onStatus`，详情页有可见的断线/重连指示
- [ ] 断线时「等回执」的动作有明确反馈（不要静默等 30s）
- [ ] 重连成功后指示消失，内容恢复（`onRecalibrate` 已有重载路径）
- [ ] e2e：打开运行中的任务 → 停掉后端（或 `page.route` 掐断 `/events`）→
      断言出现断线指示；恢复后断言指示消失
- [ ] e2e：断线时点一个拍板动作 → 断言有「流未连通」类反馈，而不是静默 30s

**边界.** 指示的措辞与看板保持一致（同一件事别两套说法）。
