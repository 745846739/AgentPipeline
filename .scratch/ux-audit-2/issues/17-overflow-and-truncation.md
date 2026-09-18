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

**Status:** open

- [ ] MetadataCard 的值列能收缩（`min-width:0` + 正确的换行策略），长值不再撑破卡片与页面
- [ ] 三处截断值可用悬停看全（`title`），或改成可展开/可复制
- [ ] toast：hover/focus 时暂停计时；有关闭的键盘路径；必要时取消 `nowrap`
- [ ] toast 与移动动作坞不再重叠（或 toast 抬到坞之上并保证坞可点）
- [ ] 夜间/节流丢弃通知的行为要么保留但**可被用户发现**，要么调整
- [ ] e2e：塞一个长 metadata 值 → 断言会话栏没有横向溢出（`scrollWidth <= clientWidth`）
- [ ] e2e：截断元素带 `title`；toast 在 hover 时不会消失（时间旅行或加长 TTL 断言）
- [ ] e2e（430 宽）：toast 出现时动作坞的按钮仍可点（`elementFromPoint` 命中坞按钮）

**边界.** 别为了这几个值引入新的通用布局组件；就地修。
