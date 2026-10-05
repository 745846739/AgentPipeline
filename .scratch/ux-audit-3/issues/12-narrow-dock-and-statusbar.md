# 12: 决策 243/300 窄档底栏与状态条（≤479px 状态条整条退场）

**叠:** A（不动规格）

**来源:** B 漂移面（决策 243 底部页签栏 + 决策 300 状态条整条退场）

**What to see:**
真页面 430 视口走查看板路由 `#/`（`[r3] ④.5`）：

```
footer.statusline  display = none   h = 0
底部页签栏 navbar   display = flex   h = 58   aria-label = "页面导航"
--topbar-h = 52px
--sbar-h   = calc(58px + 0px)
```

与决策 300（修订决策 243 的 ②④）逐条对上：
- **状态条整条退场**：`frontend/src/components/layout/StatusLine.svelte:164-166` 的
  `@media (max-width: 479px) { .statusline { display: none } }`——实测 `display:none`、`h:0`。
  `:156-163` 头注写明三个读数的去处（待处理/执行中→道具栏、token→指标页、深浅切换→
  `#/settings` 页头）与「`display:none` 而不是卸载」的理由（桌面档同一份 DOM 还要用）。
- **底部页签栏在场**：`TopBar.svelte:604` 起 `@media (max-width: 479px) { .navbar { position: fixed; … } }`，
  实测 `display:flex`、`h:58`、保留 `aria-label="页面导航"` 契约（决策 243「不改 role/aria-current/恰四项」）。
- **让位账本**：`app.css:662-663` 窄档 `--nav-h: calc(58px + var(--safeb))`、
  `--sbar-h: var(--nav-h)`——实测 `--sbar-h = calc(58px + 0px)`（safeb 为 0），即
  「只剩页签栏一层」，决策 300④ 记的「改值不改名、五个消费点零改动」成立。
- `--topbar-h = 52px`：看板路由窄档顶栏（`TopBar.svelte:533-561`，道具栏行自带下框
  44 触控 + 留白），与决策 242③ 的「看板 52px」吻合；非看板路由那一档应为 0（见边界）。

结论：**已修**——决策 243 的底栏与决策 300 的状态条退场在真页面上成立，且底部让位账本
`--sbar-h` 已按决策 300④ 收成 `--nav-h` 单层。无缺陷。

**证据等级:** 实测（`[r3] ④.5` + 截图 `r3-narrow-bottom-430.png`）+ 代码
（`StatusLine.svelte:156-166`；`TopBar.svelte:604` 起；`app.css:22-28,651-672`）

**与前轮关联:** 新开（B 漂移面；决策 243/300 在第二轮收口之后，前两轮未覆盖）

**建议:** 无。

**边界:** 审计票，不实现。`--topbar-h` 的「非看板路由取 0 那一档」（决策 243③ / 票 09）
本轮探针只走了看板路由 `#/`，故非看板路由的 0 值**未在真页面上量到**——标「未验证」，
不改结论（该值由顶栏自己 `bind:offsetHeight` 回写，决策 243⑤ 已放开 0 值）。
