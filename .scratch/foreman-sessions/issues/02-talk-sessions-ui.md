# 02: 班次 chip 行——新建、切换、重命名、归档

**What to build:** 对讲台时间线顶部一条**班次 chip 行**：每个 chip 一个会话（当前那个高亮），末尾一颗
「+ 新班次」；重命名与归档走 `Modal.svelte`。

**几何纪律（三条硬约束，都不是偏好）**：

- **不新增 pinned 元素**：chip 行**非 sticky**、随手指滚——决策 192 的窄屏只有两个钉住物
  （急停摘要条 + 输入坞），`frontend/e2e/talk.spec.ts:532-560` 有断言盯着；
- **不动页头**：`.talk-head` 在 `row auto` 上，涨的 px 直接从 `minmax(0,1fr)` 的对话区扣，
  `Talk.svelte:464-473` 的注释（「页头因此从 38px 涨到 72px，直接吃掉对话区 34px」）与
  `talk.spec.ts` 的对话区 ≥320px 断言都盯着它；
- **不动顶栏**：决策 198 把页面导航行锁在**三项**且移动款高度冻结在 138px（窄屏 `top:138px` 的
  sticky 依赖它）。

**词汇复用**（决策 204）：`components/task/ConversationViewer.svelte` 的 `.runchip` / `.runrow`
——「选一条会话」在任务详情页已经有现成语汇，窄屏 `flex-wrap: nowrap; overflow-x: auto` 也是现成的。
**不得渲染成 `.turn`**：`talk.spec.ts:163` 断言时间线里没有 `.turn.warn`。

**切换时要重置的会话级状态**（`Talk.svelte` 的 runes）：`session`、`loading`、`loadError`、
`pendingText`、`stream`（→ `emptyForemanStream()`）、`input`。**不要重置** `details` / `chosenStop` /
`crew` / `pending`——它们派生自全局看板，不是会话。**发送中禁止切换会话**（最基本的自保；
彻底隔离在 03）。

**Blocked by:** 01

**Status:** done

- [ ] chip 行在时间线顶部、非 sticky、窄屏横滚
- [ ] 新建 / 切换 / 重命名 / 归档四个动作可用；新建后是空会话（空态文案走 `EmptyState`）
- [ ] 切换重置会话语状态、不动看板派生状态（把两份清单逐条对照写进交付说明）
- [ ] 发送中禁止切换（按钮禁用 + 说明）
- [ ] 窄屏几何断言仍绿：对话区 ≥320px、整页不空滚、两个钉住物各就各位、138px 顶栏
- [ ] 归档当前会话后自动切到最近活动的未归档会话；一个都没有时新建一个
- [ ] **不做分叉**（决策 204）：不从某一轮另起一段

## 交付

本票已落地（2026-09-17）。`frontend/src/routes/Talk.svelte`：

- **chip 行在时间线内部的顶部**（`.timeline .runrow`，词汇复用 `ConversationViewer.svelte` 的
  `.runchip`）：**非 sticky**（`position: static`，e2e 断言）、**不在页头**（e2e 断言
  `.talk-head .runrow` 计数为 0）、不动顶栏。窄屏 `flex-wrap: nowrap; overflow-x: auto`。
  放在时间线**内部**是这三条硬约束的直接结果：时间线自己的盒子高度不变，
  故「对话区 ≥320px」那条几何断言不受影响（e2e ⑮ 实测绿）。
- **四个动作**：`+ 新班次` / 切换（点 chip）/ 改名 / 归档（后两个走 `Modal.svelte`，
  复用同一套 Escape / 焦点 / 模态语义）。新建后是空会话，空态走 `EmptyState`。
- **切换时重置什么、不重置什么**（逐条对照，代码注释与 `resetSessionState()` 同处）：
  **重置** `session` / `loading` / `loadError` / `pendingText` / `stream`（→ `emptyForemanStream()`）/ `input`；
  **不重置** `details` / `chosenStop` / `crew` / `pending`——它们派生自全局看板，
  「换会话 ≠ 换看板」。e2e ⑭ 用「切换前后急停文本一字不差」把这条钉住。
- **发送中禁止切换**：四个动作的按钮一律 `disabled={sending || busy}`，并在 chip 行尾显示
  「回话中，先别换班次」——光把按钮变灰，人会以为界面卡了。
- **归档当前会话**：自动切到最近活动的未归档班次；一个都不剩时新开一个。
- **不做分叉**。
- **空态里的摆放**：chip 行是**控件**、空态文案是**内容**，故空态下 chip 行贴顶、
  只把空态文案居中（`justify-content: center` 会把整组居中，那就成了「控件浮在大片空白中间」，
  正是决策 202 治的毛病）。
