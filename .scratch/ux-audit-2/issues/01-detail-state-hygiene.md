# 01: 详情页加载失败时清掉上一个任务，并给一条重试的路

**叠:** A（不动规格）

**来源:** R2-01（实测）+ 7b（代码）

**What to build:** 打开任务 A，再切到一个不存在的 id：页面顶部一条红字「任务不存在：…」，
**红字底下仍是 A 的完整界面**——标题、hero 轨道、页签、档案盒，连 6 颗拍板按钮都在，
而它们会提交到那个坏 id。修两件事：① `load()` 失败时把上一个任务的状态清干净，
让加载/空分支真的可达；② 失败态给一颗「重新加载」（含首次加载失败就永久死页那一种：
流没接上，服务器恢复也不会自愈）。

**Blocked by:** None（can start immediately）

**Status:** done

- [x] `stores/taskDetail.svelte.ts:110` 的 `catch` 清掉 `task` / `cursors` / `allowedActions` /
      `transitions` / `conversations` / `commands`（`{#if task}` 不能再短路加载分支）
- [x] 失败态有可点的「重新加载」，点了真的重跑 `load()`（首次加载失败也算）
- [x] 正常 A→B 切换时不会先闪 A 的正文（`loading` 分支可达）
- [x] e2e：打开有效任务 → 切到不存在的 id → 断言页面上**没有**上一个任务的标题、
      **没有**动作按钮、**有**重试钮；点重试（恢复后端）后内容回来
- [x] e2e：`page.route` 让 `GET /tasks/{id}` 首次 500 → 恢复 → 点重试 → 内容回来

**边界.** 不要动档案盒的动作行本身（六个 e2e 文件依赖它的定位）。

## 实施记录（2026-09-18）

落点：`frontend/src/stores/taskDetail.svelte.ts` 的 `load()` 与新增的 `resetTaskContent()`；
`frontend/src/routes/TaskDetail.svelte` 的失败空态。

**三处口径，比票面多写清两条：**

1. **清在「用户可见的那一次装载」的开头**，不是只在 `catch` 里：换 id 时先清再发请求，
   否则 B 的地址下仍会渲染一帧 A 的正文（票面第 3 条要的就是这个）。同一次 `load` 的
   `catch` 里再清一次——`getTask` 成功、随后 `getFlow` / `getConversations` / `getCommands`
   里有一个失败时，半截的新任务会与上一份的 transitions 混在一起。
2. **静默 refetch 失败不清内容**（只挂横幅 + 一颗「重试」）。那是 SSE 事件触发的后台对齐，
   服务器重启期间必然失败几次；把正在看的页面清空比留着更坏——这条没写进票面，是实施时定的。
3. **旧任务的流一并收**（`streamManager.sync([])`）：事件回调不按 id 过滤，留着就会把 A 的
   事件记到 B 的头上。

**404 与「没读到」分开说**：`errorStatus` 是 404 时空态写「任务不存在：<id>」，其余写
「任务没能打开：<id>」并把原始原因摆出来（同时也进 live region，见票 02）。

**e2e**：`frontend/e2e/ux2-failure-paths.spec.ts` 的 `UX2 ①`（三条，进 `make check-e2e`）。
