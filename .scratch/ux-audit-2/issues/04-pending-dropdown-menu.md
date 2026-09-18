# 04: 待处理下拉：补齐菜单的键盘语义，或者别叫它 menu

**叠:** A（不动规格）

**来源:** R2-04（实测）

**What to build:** 「待处理」下拉播报自己是 `role="menu"` / `menuitem`，但没有任何菜单行为。
实测（`①.2`）：打开后按 Escape **仍然开着**、点面板外面的空白**仍然开着**、
焦点在触发钮上按 ArrowDown 焦点不动；触发钮还缺 `aria-haspopup` / `aria-controls`。

`role="menu"` 是有契约的。两条路选一条：**补齐**（Escape 关、方向键走、点外关、
`aria-haspopup="menu"` + `aria-controls`、关闭后焦点回触发钮），或者**降级**成普通弹层
（去掉 `role=menu/menuitem`，用一对 `aria-expanded` + 可聚焦的列表）。

**Blocked by:** None（can start immediately）

**Status:** open

- [ ] 定下走哪条路（补齐菜单语义 / 降级），并说明理由
- [ ] Escape 一定关得掉；点击面板外部一定关得掉
- [ ] 关闭后焦点回到触发钮
- [ ] 若保留 `role="menu"`：方向键可在项间移动、Home/End、Enter 激活
- [ ] 触发钮有 `aria-haspopup` 与 `aria-controls`
- [ ] e2e：打开 → Escape → 断言关闭；再打开 → 点空白 → 断言关闭；
      再打开 → ArrowDown → 断言焦点进入第一项
- [ ] e2e：断言触发钮的 `aria-haspopup` / `aria-controls` 指向真实元素

**边界.** 下拉里每一项仍是能跳任务详情的可点项——别为了菜单语义把导航改成非链接语义。
