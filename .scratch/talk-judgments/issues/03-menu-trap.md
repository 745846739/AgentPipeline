# 03: 键盘陷阱收成 `lib/menuTrap.ts`——两处同义实现合成一处

**What to build:** `Talk.svelte:923-1003`（⋯ 班次菜单）与 `components/layout/TopBar.svelte:103-185`
（待处理下拉）是**同一套控制流换了标识符**。Talk 那段注释自己承认是照抄
（`:925`「交互语汇**照抄**顶栏那个下拉（票 04 / R2-04 补的三条出口），**不新造第三套**」），
两条注释点名同一组出口：`aria-expanded` + `aria-controls`、Escape 关得掉
（**焦点没进过面板时也算**）、点面板外面关、上下方向键走项、`Home` / `End`、
面板常驻 DOM 用 `hidden` 开合、键盘一律在 `window` 上收。

本票把**纯判定**与一个 `attachMenuTrap(node, opts)` **普通 helper** 抽进 `lib/menuTrap.ts`，
两处都从 effect 里调。

**为什么不是只抽纯判定**：88 行里绝大部分是接线（查项、聚焦、挂 `window` 监听、
`preventDefault`），只抽判定等于把大头留在两处。

**为什么不是 Svelte `use:` action**：**全仓一个 `use:` 都没有**（`grep -rn 'use:' frontend/src/`
零命中，也没有任何文档表态）。不在一次去重里首次引入一种团队从未用过、也从未写下的模式。

**Blocked by:** None（可立即开始）

**Status:** done（2026-09-23 实现；`TopBar.test.ts` 一条不改全绿；2026-09-24 收口：`make check` 四段全绿含 e2e 121 通过 0 失败——见交付说明）

- [x] 新建 `frontend/src/lib/menuTrap.ts`，导出：① 纯判定（下一个 / 上一个索引、绕回、
  `ArrowUp` 从 0 回触发钮、`Home` / `End` 落点、Escape 该不该关、该不该还焦点）②
  `attachMenuTrap(node, opts)`（返回 detach），`opts` 收：开合状态读写、触发钮 / 面板的引用、
  项选择器、`returnFocus` 策略
- [x] 接入 `Talk.svelte:923-1003`：`menuItems` / `focusMenuItem` / `closeMenu` / `toggleMenu` /
  `onWindowKey` / `onWindowClick` 六个函数由 helper 承担，`menuOpen` 作为**局部** `$state` 传入
  （Talk 这份今天用的是局部 `menuOpen`，不是 store）
- [x] 接入 `TopBar.svelte:103-185`：同上，但开合态走 **store** `board.pendingOpen`
  （`stores/board.svelte.ts:40`），且开合还带一个副作用 `board.togglePendingDropdown()`
  （`:262-276` 的 `refreshPendingInto()` 会发一次 `listTasks`）——**该副作用留在 TopBar**，
  不搬进 helper
- [x] **禁用项跳过**的行为差**如实保留**：Talk 的选择器是 `button[data-menu-item]:not([disabled])`、
  TopBar 是 `a.dd-item`（全是链接，无禁用态）。**不强行统一**成一条选择器——把选择器作为 `opts` 传入
- [x] 新增 `frontend/src/lib/menuTrap.test.ts`：纯判定逐条（绕回边界、`ArrowUp` 从 0、`Home`/`End`、
  空列表是 no-op）
- [x] **`TopBar.test.ts:85-135` 必须原样通过、不准改**——它是黑盒（渲染组件、`fireEvent.keyDown(window, …)`、
  断言 `document.activeElement`），抽对了它不动。四条硬约束：① 监听留在 **`window`**
  （挂到 `document` 或元素上会让 jsdom 里的 window 事件收不到）② `id="pending-dropdown"` /
  `a.dd-item` / `aria-expanded` / `hidden` 四个 DOM 契约不许挪 ③「Escape 在焦点没进过面板时也关得掉」
  必须保持 ④ ArrowDown-on-trigger 打开并送焦点进第一项
- [x] Talk 那份陷阱**补单测**：它今天零单测，只有 `e2e/talk.spec.ts` 的 4 条
  （覆盖不到 ArrowUp 回触发钮 / Home / End / 绕回）。**首选**把 TopBar 那组断言在 Talk 上重放一遍
  （同姿态、同断言），若挂载成本过高（Talk 是路由件，见 `README.md` 第三节的实测代价），
  退一步：靠 `menuTrap.test.ts` 的纯判定覆盖 + 一条 e2e 补 ArrowUp / Home / End
- [x] **静态扫描守卫**（若本票是 01–03 里最后一票，守卫落在这里）：照 `lib/talkLayout.test.ts` 先例，
  断言 `Talk.svelte` 与 `TopBar.svelte` **都不再**各自定义 `onWindowKey` / `onWindowClick`，
  且**都 import 了** `menuTrap`
- [x] `design/frontend-design.md` §12.3 的 `:783`（对讲台 ⋯ 菜单键盘出口）与 `:819`
  （顶栏「待处理」下拉键盘出口）两行加上 `frontend/src/lib/menuTrap.ts`——**先让新 module
  落盘，再改表行**：`lib/behavior-map.test.ts:142-159` 断言被引路径**存在**（`PENDING`
  名单是空的，引了不存在的路径当场红）
- [x] `make check` 绿（2026-09-24 分段取证：fmt+clippy / `cargo test --workspace` / vitest 778 + svelte-check 0 错 + build / e2e 121 通过 0 失败）


## 交付说明（2026-09-23）

**闸门**：`npm test` 绿（本票新增 `lib/menuTrap.test.ts` **20 条**）；
**`TopBar.test.ts` 11 条一条未改、全绿**——它就是决策 251⑤ 那四条硬约束的黑盒闸门
（监听留 `window` / `id="pending-dropdown"`·`a.dd-item`·`aria-expanded`·`hidden` 不挪 /
Escape 焦点没进过面板也关得掉 / ArrowDown-on-trigger 送焦点进第一项）。`npm run build` 绿。
**e2e 未跑**（`cargo` 卡在并发会话在飞的 `serve.rs:524`，本批不动 Rust）。

**一处形状偏差（已回写进决策 251⑤）**：本票与 README §三 原定
`attachMenuTrap(node, opts)` **返回 detach、由 effect 调**；实现期改形为
**`createMenuTrap(opts)` 产 handler、交模板的 `<svelte:window onclick onkeydown>` 收**。
理由：两处本来就用 `<svelte:window>` 在模板里收（那是「键盘一律在 `window` 上收」这条
a11y 立场的可见落点），改成 effect 挂 / 卸会**每次开关态变化都重挂一遍监听**，还会把那条
立场从模板挪进脚本。**不引入 Svelte `use:` action** 这一条不变。

**判据与接线分家**：`decideMenuKey` / `wrapIndex` / `closeOnOutsideClick` 是纯函数，
单测钉**陷阱本身**；接线由 `createMenuTrap` 承担，`TopBar.test.ts` 黑盒验。
§12.3 的 `:783`（⋯ 班次菜单）与 `:820`（待处理下拉）两行都已加 `frontend/src/lib/menuTrap.ts`。

**`make check` 绿** 那一条当时未勾：`cargo test --workspace` 与 e2e 卡在并发会话的
Rust 编译上（见上）。**2026-09-24 已补跑并勾上**：四段分段取证全绿（fmt+clippy /
cargo test --workspace / vitest 778 + svelte-check 0 错 + build / e2e 121 通过 0 失败）。
