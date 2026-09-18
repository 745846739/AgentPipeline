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

**Status:** done

- [x] 定下走哪条路（补齐菜单语义 / 降级），并说明理由
- [ ] Escape 一定关得掉；点击面板外部一定关得掉
- [ ] 关闭后焦点回到触发钮
- [ ] 若保留 `role="menu"`：方向键可在项间移动、Home/End、Enter 激活
- [ ] 触发钮有 `aria-haspopup` 与 `aria-controls`
- [ ] e2e：打开 → Escape → 断言关闭；再打开 → 点空白 → 断言关闭；
      再打开 → ArrowDown → 断言焦点进入第一项
- [ ] e2e：断言触发钮的 `aria-haspopup` / `aria-controls` 指向真实元素

**边界.** 下拉里每一项仍是能跳任务详情的可点项——别为了菜单语义把导航改成非链接语义。

## 实施记录（2026-09-18）

**走的是降级那条路。理由**：这一栏里每一行都是「跳到某个任务详情」的**链接**，链接不是菜单项。
补齐 `role="menu"` 的契约要求把行播报成 `menuitem`——那正好丢掉链接语义，而本票边界明令
不能为了菜单语义改掉导航语义；而这一栏本来就只是「有哪些待办 + 点进去」的跳转列表。
行从 `<button>` 换成 `<a href="#/task/…">`（链接语义坐实）。

**两处与票面清单的有意偏差**（都记在这里，免得下一轮当成欠账）：

1. **不加 `aria-haspopup`**。`aria-haspopup="true"` 在 ARIA 1.2 里等同 `"menu"`——加回去就是
   换了个说法继续声称「这是个菜单」。触发现有的 `aria-expanded` + 新增的 `aria-controls`
   正是 disclosure 模式给的那一对。e2e 断言的是 `aria-controls` 指向**真实存在**的元素。
2. **面板常驻 DOM、靠 `hidden` 开合**。`aria-controls` 的目标必须真的在文档里（IDREF 悬空
   是另一条会烂掉的账）；代价是待办行在没有打开时也渲染着，但那是一份很轻的列表。

**键盘**：方向键 / Home / End 都在 `window` 上收（触发钮与面板都不挂 `onkeydown`——
静态元素挂交互处理器与「把按钮语义扯歪」都是 a11y 检查里的红灯）。判据收紧到
「面板开着，且焦点在触发钮或面板里」；Escape **不看焦点位置**，一律关得掉，焦点若在这两处
就顺带还给触发钮。

**e2e**：`frontend/e2e/ux2-semantics.spec.ts` 的最后一条（进 `make check-e2e`）；
组件级十条在 `src/components/layout/TopBar.test.ts`。
