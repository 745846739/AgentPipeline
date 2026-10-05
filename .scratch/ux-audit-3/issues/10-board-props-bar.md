# 10: 看板顶部道具栏（过滤槽 / 待处理 / 新建）的语义与计数

**叠:** A（不动规格）

**来源:** B 漂移面（决策 201 过滤槽 + 决策 92 待办入口；第二轮收口后仍在，作独立候选复核）

**What to see:**
真页面走查 `#/`（看板路由）的道具栏（`[r3] ④.3`）：

```
slotCount = 7
slots = [{全部 1, pressed=true,   count="1"},
         {执行中 0, pressed=false, count="0"},
         {待处理,   pressed=false, count=null},   ← 第 3 槽不画数
         {等依赖 0, pressed=false, count="0"},
         {排队 0,   pressed=false, count="0"},
         {已完成 0, pressed=false, count="0"},
         {已结束 0, pressed=false, count="0"}]
pendingChip = "待处理 1"
```

语义完整、与决策 201 逐条对上：`frontend/src/components/layout/TopBar.svelte:164` 的
`<nav class="slots" aria-label="状态过滤">` 是**唯一**一个带「状态过滤」名的导航行；
每个槽是原生 `<button>`，`aria-pressed={board.filter === f}`（`:176`）、
`title={FILTER_LABELS[f]}` 与 `aria-label="{FILTER_LABELS[f]}（{board.countFor(f)}）"`
（`:172-175`）双编码，屏幕上的字（`.lbl`）+ 计数徽章（`.cb`）才是主层。
第 3 槽（`pending`）**不画数**（`:179-181` 的 `{#if f !== 'pending'}`，实测 `count=null`）——
它与紧邻的「待处理 N」芯片是同一个数，数由芯片**唯一**承载（决策 201 的去重）。
「待处理」芯片（`:190-197`）是 `aria-expanded={board.pendingOpen}` +
`aria-controls="pending-dropdown"` 的**披露型**按钮，面板常驻 DOM 靠 `hidden` 开合
（`:206-211`），IDREF 不悬空。「新建任务」是 `class="btn btn-new"` 的原生按钮（`:223`）。

计数来源可信：`frontend/src/stores/board.svelte.ts:256` 的 `countFor(filter)` 按
`StatusFilter` 七值（`:19`）各算各的，`ended` 一条单独把 `failed|cancelled` 计数（`:258-260`），
与实测「已完成 0 / 已结束 0」分开列出吻合。

结论：**新开（无缺陷）**——七个过滤槽语义齐全（`aria-pressed` + 图标 + 词 + 计数去重）、
待办芯片是可展开披露型按钮、新建任务在场。决策 201 的形态在漂移面之后仍在。

**证据等级:** 实测（`[r3] ④.3` 数字 + 截图 `r3-board-props-bar.png`）+ 代码
（`TopBar.svelte:163-183,190-211,223`；`board.svelte.ts:19,256-260`）

**与前轮关联:** 新开（B 漂移面；第一轮票 26/决策 201 的实现复核见本目录票 06，本条只看**语义与计数**）

**建议:** 无。

**边界:** 审计票，不实现；不判定窄档（≤479px）道具栏行「只跟看板相关」的按路由露出
（那是决策 242③ 的口径，落点见票 12）。
