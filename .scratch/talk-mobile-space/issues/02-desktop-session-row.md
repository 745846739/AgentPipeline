# 02: 桌面：班次行移出时间线滚动容器

**叠:** A（不动规格；决策 218 已把口径定死）

**来源:** 决策 218 ①/②/Q15；用户「桌面端也需要固定住新建对话这些而不是随着对话上移」

**What to build:** 桌面上整页钉在视口里、只有 `.timeline` 在滚，而班次行 `Talk.svelte:1047-1092` **长在那个滚动容器里面**——所以它随对话上移。把它搬出来挂到页头那一行右端，它就恒在。

**Blocked by:** 01（决策已落，可立即开工）。与 03–06 互不阻塞，可并行。

**Status:** done（桌面班次行落点已落地；`talk.spec.ts` 桌面组全绿）

- [ ] 把 `.runrow` 的标记从 `<section class="timeline">` 内**移出**，作为 `.talk-head`（`grid-column: 1 / -1`，桌面全宽）的第三个 flex 子项，排在 `.ts` 之后；`role="group"` 与 `aria-label="班次"` 原样保留
- [ ] `≥900px` 才渲染（`≤899` 由票 04 的 ⋯ 取代）。判据用一个新断点，**与决策 215 的 `<900` 同源**，不得各写各的（现有 `narrow` 是 `max-width: 479px`，见 `Talk.svelte:156/595`，**不是**这一档的判据）
- [ ] `.runrow` 在页头里的样式：`flex: 1 1 auto; min-width: 0; flex-wrap: nowrap; overflow-x: auto; justify-content: flex-end; margin-bottom: 0`（容器内横滚、滚动条不占高——与 `.slots` / `.navbar` / 窄屏 chip 行同一手法，`no-scrollbar` 已在标记上）
- [ ] 收掉两条死代码规则：`.timeline.empty > .runrow { flex: 0 0 auto }`（`Talk.svelte:1467-1469`）与随之无主的空态居中写法（`.timeline.empty > :global(.empty)` 保留），空态改为整格居中。**这两条只由本票删**（票 04 不得重复删）
- [ ] **不改** `--talk-chrome: 386px` 与 `--timeline-floor: 160px`（`Talk.svelte:1386-1392`）。若实测发现页头被撑高导致时间线掉到 160px 之下，说明落点选错了，**回来重裁**而不是调常量
- [ ] e2e（1280×720，长对话）：时间线滚到底后 `.runrow` 仍在视口内、整页仍不滚（`scrollHeight - innerHeight ≤ 0`）、`.timeline` 高度不比改动前少
- [ ] e2e：`.timeline` 的后代里**不再有** `.runrow`；`.talk-head` 里有一颗可点班次芯片且当前项仍带 `aria-pressed`
- [ ] 回归：`expectProposalReachable` 四条断言（1280×720）与 `talk.spec.ts` 桌面两张急停那一组全绿；时间线滚到底时急停摘要条仍在第一屏（`toBeInViewport`）

**边界.** 不动芯片的形状、词汇与 `.runchip` 类；不动桌面三分区几何与右栏；不动 `Modal`（改名 / 归档照旧）；不给桌面加 ⋯（决策 218 Q14）。

## 实现记录（2026-09-18）

- 桌面班次行已搬出时间线滚动容器，挂到 `.talk-head` 右端（`flex: 1 1 auto; min-width: 0; flex-wrap: nowrap; overflow-x: auto` + 逐块删掉 `.timeline.empty > .runrow` / `.rwhy` 两条死规则）。
- **回归**：时间线滚到底时 `.talk-head .runrow` 与 `.runchip.plus` 都在视口内、`.timeline .runrow` 计数为 0；`expectProposalReachable` 四条（1280×720）全绿；桌面两张急停那一组全绿。
- **偏离（如实记）**：桌面的班次芯片**回话中不禁用**（决策 220② 撤的是那把锁），只有改名 / 归档在 `sending || busy` 时仍禁用；`.talk-head` 的高度断言改成「≤ `<h1>` 行盒 + 1px」（原先写死 38，见决策 221）。
