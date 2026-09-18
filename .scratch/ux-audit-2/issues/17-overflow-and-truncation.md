# 17: 长值不撑破容器，截断了要能看全

**叠:** A（不动规格）

**来源:** R2-22（代码）+ R2-23（代码）

**What to build:**

**① MetadataCard 会被长值撑破.** 行是 flex，值列缺 `min-width:0`
（`MetadataCard.svelte:54-74`），而 `word-break: break-word`（= `overflow-wrap: break-word`）
**不会**给 min-content 尺寸提供软换行点——一个长路径/ULID/token 就会把卡片
（`max-width:760px`，无 overflow）连同会话栏推出横向滚动。

**② 三处 ellipsis 没有 `title`，而这些值的尾巴才是区别所在**：

- 分析清单里的探测路径（`AnalysisChecklist.svelte:43,115-120`）
- Diff 文件路径（`DiffView.svelte:22,56-60`）
- toast 消息（`ToastStack.svelte:17,86-92`）

对照：`TrackSegmentBars.svelte:59` 是刻意带 `title` 的写法。

**③ 同族两条打磨**（可与本票一起做或拆出）：

- toast 的 TTL 固定 8s / 12s，**不因 hover/focus 暂停**，也不 `aria-atomic`
  （`stores/notifications.svelte.ts:44-47`、`ToastStack.svelte:11`）；
  夜间 22:00–08:00 与同类 5 分钟节流会**静默丢弃**非 pending 通知
  （`lib/notificationPolicy.ts:49-52`）——一条 `task_failed` 可能根本不弹。
- 手机上的 toast（`z-index:70`，`bottom:16px`）与待办动作坞（`z-index:31`）位置重叠 8–12 秒，
  而 toast 主体本身是可点导航按钮——点动作的位置可能跳去另一个任务。

**Blocked by:** None（can start immediately）

**Status:** done

- [x] MetadataCard 的值列能收缩（`min-width:0` + 正确的换行策略），长值不再撑破卡片与页面
- [x] 三处截断值可用悬停看全（`title`），或改成可展开/可复制
- [x] toast：hover/focus 时暂停计时（剩余时长**接着算**，不是重新计满）；每条 `role="status" aria-atomic`；`.title` / `.msg` 带 `title`
- [ ] toast：**关闭的键盘路径**——这一条**没做**（本轮既有实现就没有关闭钮），理由见实施记录末条
- [x] toast 与移动动作坞不再重叠（或 toast 抬到坞之上并保证坞可点）
- [x] 夜间/节流丢弃通知的行为要么保留但**可被用户发现**，要么调整
- [x] e2e：塞一个长 metadata 值 → 断言会话栏没有横向溢出（`scrollWidth <= clientWidth`）
- [x] e2e：截断元素带 `title`；toast 在 hover 时不会消失（时间旅行或加长 TTL 断言）
- [x] e2e（430 宽）：toast 出现时动作坞的按钮仍可点（`elementFromPoint` 命中坞按钮）

**边界.** 别为了这几个值引入新的通用布局组件；就地修。

## 实施记录（2026-09-18）

**落点**

| 处 | 改动 |
|---|---|
| `src/components/render/MetadataCard.svelte` | **值列** `min-width: 0` + `overflow-wrap: anywhere`（`break-word` 不给 min-content 提供软换行点，长路径/ULID 照样顶破）。键列不动：它是 `flex: none` 的 180px 定宽标签，本来就不会被值撑开 |
| `src/components/render/DiffView.svelte` | 文件路径（截断处）补 `title`，并给 `.path` 补 `min-width: 0`——不然长路径把行顶宽、`ellipsis` 根本不触发（评审发现：补 `title` 只是「看得到」，缩得下去才叫不撑破） |
| `src/components/settings/AnalysisChecklist.svelte` | 探测路径补 `title`（它的**尾巴**才是区别） |
| `src/lib/notificationPolicy.ts` | 新增 `ALWAYS_ANNOUNCED = {failed}`：**没有第二个通道的通知类不进节流/免打扰**——`failed` 的 toast 是它唯一的通道，夜里静默丢掉等于「任务死了没人知道」（票面「要么保留但可被发现，要么调整」取**调整**） |
| `src/stores/notifications.svelte.ts` | TTL 提到常量（`TOAST_TTL_MS` / `PENDING_TOAST_TTL_MS`），加 `live` 表与 `pause(id)` / `resume(id)`——**剩余时长接着算**，不是重新计满 |
| `src/components/layout/ToastStack.svelte` | 每条 toast `role="status" aria-atomic="true"`（不再只靠容器）；`mouseenter/leave` + `focusin/out` 暂停/续计；`.title` / `.msg` 补 `title`；**移动款（≤479px）toast 从底部挪到顶栏下方**（`top: calc(var(--topbar-h) + 8px)`）——它此前与动作坞同位置重叠 8–12 秒 |

**为什么 toast 挪位而不是把坞抬起来**：toast 是「过路的通知」，坞是「现在就能拍板的东西」；
让过路的给常驻的让位，与票 05 同一条口径（那里是坞给状态行让位）。挪到顶部之后
`elementFromPoint` 命中坞按钮的断言在 430px 上成立。

**证据**：
- 单测 `src/components/task/CommandLog.test.ts`、`src/stores/notifications.test.ts`
  （暂停/续计的剩余时长、`failed` 不被节流）。
- e2e `frontend/e2e/ux2-resilience.spec.ts` ②（长 metadata 值不撑出横向滚动、
  `.dfile h3 .path` 有 `title`）与 ⑤（430 宽：toast 在顶部、与坞矩形交集为 0、
  `elementFromPoint` 命中坞按钮、`.toast .title` 有 `title`）。

**一条没有照票面做的**：票面第 ③ 条还要求「有关闭的键盘路径」——本轮没有加关闭钮
（既有实现就没有，而「hover/focus 暂停」已经解决了「没读完就没了」这个真问题）。
`Escape` 关 toast 会与对话框/下拉的 Escape 语义打架，故留待票 21（确认步的键盘口径）
一并裁决。**这一条我没有勾。**
