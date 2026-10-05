# 05: toast 的键盘关闭路径（票 17 明写「没做」的子项）

**叠:** A（不动规格）

**来源:** A/ux-audit-2 票 [17](../ux-audit-2/issues/17-overflow-and-truncation.md) 的未勾子项
（该票 Status: done，但清单末条 `- [ ] toast：关闭的键盘路径——这一条没做`；
实施记录末条写「本轮没有加关闭钮……留待票 21 一并裁决」）

**出处:** `frontend/src/components/layout/ToastStack.svelte:32-36`（关闭钮）；`frontend/src/stores/notifications.svelte.ts:87`（`dismiss`）；Escape 缺失为全文件 0 命中的负向证据；`[r3] ⑤.1`

**严重度:** 低（toast 已可点关闭钮关闭，缺的是 Escape 快捷路径）

**What to see:**
源码（本次实地读到）——**关闭钮现在存在了**（票 17 当轮没有，之后某次改动补上了）：

```
ToastStack.svelte:32-36  <button type="button" class="close" aria-label="关闭"
                                 onclick={() => notifications.dismiss(toast.id)}>×</button>
```

`dismiss(id)` 在 `stores/notifications.svelte.ts:87`。因为它是原生 `<button>`，键盘可达：
Tab 聚焦 + Enter/Space 触发——即**「有关闭钮且键盘可触发」这条已成立**。

但**没有 `Escape` 快捷关闭**：全文件 `grep Escape` = 0 命中（`ToastStack.svelte` 无任何
`keydown` 监听；全局 Escape 监听只存在于 `Modal.svelte:98,129`、`TopBar.svelte:145`、
`Talk.svelte` 的菜单里，均不覆盖 toast）。而这正是票 17 实施记录里**有意留待票 21 裁决**
的那半句——票 21 于 2026-10-01 以 wontfix 收掉，故这个 Escape 口径至今**没有裁决、也没实现**。

真页面侧：第三轮的 toast 探针 `[r3] ⑤.1 {"count":0,"closeBtns":0}`——harness 在这一刻
**没有弹出任何 toast**，故「关闭钮 / Escape」在运行时**未量到**，源码结论未经页面复核。

结论：**部分已修**——「可关闭」成立（关闭钮 + Tab/Enter 键盘可达），「Escape 关 toast」
未做且其口径随票 21 wontfix 悬置。这是票 17 明写未做子项的真实收尾状态。

**证据等级:** 代码（`ToastStack.svelte:32-36`、`notifications.svelte.ts:87`；
Escape 缺失为全文件 0 命中的负向证据）。运行时一处标 **未验证**
（`[r3] ⑤.1` 未造出 toast，count=0）。

**与前轮关联:** 现状核实（=前轮 17 的未勾子项；从「连关闭钮都没有」推进到
「有关闭钮、缺 Escape」，非回归）

**建议:** 若要补 Escape，需与票 21 / 决策 216 的「确认步键盘口径」一起裁决
（票 17 实施记录已指出 Escape 会与对话框/下拉的 Escape 语义打架）。本轮**只记现状**。

**边界:** 审计票，不实现。
