# 15: 文案与格式一致性：时间、绑定来源叫法、字面星号

**叠:** A（不动规格）

**来源:** R2-18（代码；星号一处本轮未在页面复现）

**What to build:** 三笔小账：

**① 时间格式分叉.** 市场用裸 `t.toLocaleString()`（`SettingsMarket.svelte:238-241`），
全站其余用 `formatDateTime` → `toLocaleString('zh-CN', {hour12:false})`
（`lib/format.ts:39-44`，如 `SettingsStages.svelte:194`）。非 zh-CN 浏览器上同一页会同时出现
`2026/9/18 15:04:05` 与 `9/18/2026, 3:04:05 PM`。

**② 同一字段两个叫法.** `bind_source === 'settings'` 在 `Share.svelte:237` 叫「界面设置」，
在 `:360-366` 叫「界面上的选择」（`startup` / 配置回落两处措辞一致，只有这一处分叉）。

**③ 字面 Markdown 星号.** `Talk.svelte:913-914` 与 `:1228-1229` 的
`**换过令牌后要重新添加一次**` 是纯模板文本——Svelte 不解析 Markdown、也没走 `MarkdownView`，
所以 `**` 会原样显示。只在未配对/局域网那一态可见（本轮取证 spec 在配对态
`asteriskLines:[]`，未在页面上复现，结论来自源码 + 确认无 Markdown 管道）。

**Blocked by:** None（can start immediately）

**Status:** done

- [x] 市场的时间戳改用 `lib/format.ts` 的既有口径（全站一处）
- [x] 手机访问页同一 `bind_source` 值只用一个说法（两处合一）
- [x] Talk 两处去掉字面 `**`：要么走 `MarkdownView`，要么直接用真实样式（`<b>`/class）
- [x] 单测：`formatDateTime` 的单一出处（若加了新用法，断言不再出现裸 `toLocaleString`）
- [x] 单测 / 静态扫描：`Share.svelte` 里同一个 `bind_source` 分支的标签只出现一份定义
- [x] e2e：非配对态触发配对提示，断言页面上没有 `**`（可用 `page.route` 造 403 / 非回环来源）
- [x] e2e：市场的时间戳格式与阶段页一致

**边界.** 第 ③ 条要能在测试里造出未配对态，否则就把它降级成「静态断言页面上无 `**` 字面量」。

## 实施记录（2026-09-18）

**落点**

| 处 | 改动 |
|---|---|
| `src/lib/format.ts` | 新增 `formatClockAt(Date)`（**唯一**碰 `toLocaleTimeString` 的地方），`formatClock` 改为调它；`StatusLine.svelte` 的时钟从裸 `toLocaleTimeString` 改走它 |
| `src/routes/SettingsMarket.svelte` | 时间戳改用 `formatDateTime`（原先裸 `toLocaleString()`） |
| `src/lib/sharePairing.ts` | 新增 `bindSourceLabel(source)`：`startup` → 启动参数、`settings` → **界面上的选择**、其余 → 配置文件 |
| `src/routes/Share.svelte` | 两处各写一份的三元表达式收成一次调用（同一页面上同一字段两个叫法 → 合一） |
| `src/routes/Talk.svelte` | 两处字面 `**换过令牌后要重新添加一次**` → `<b>换过令牌后要重新添加一次</b>`（不走 Markdown 管道，直接用真实样式） |

**「界面设置」还是「界面上的选择」**：取后者（它把「这是界面上选出来的」说得更完整，
且与「启动参数 / 配置文件」并列时读得通）。这一处是**统一**，不是新增说法。

**证据**：
- 单测 `src/lib/format.test.ts`：格式化函数的行为 + **三条静态扫描**——① `toLocaleTimeString` /
  `toLocaleDateString` 只许出现在 `lib/format.ts`；② 不许裸 `toLocaleString()`（随浏览器 locale 变）；
  ③ **拿 `toLocaleString` 当日期用的也归一处**（选项里出现 `hour` / `year` / `timeZone` 这类键
  就判为时间格式化）。第 ③ 条是评审当场补的：`toLocaleString('en-US', { hour: '2-digit' })`
  **正是 R2-18 那处分叉**，却因为「显式给了 locale」从前两条里漏过去。数字的千分位分组
  （`toLocaleString('en-US')`）仍不在此列，但也必须显式给 locale。
- 单测 `src/lib/sharePairing.test.ts`：三态取值。
- e2e `frontend/e2e/ux2-flows-and-copy.spec.ts` ④：**非回环来源**造出未配对态，
  断言页面上没有字面 `**`（票面说的「若造不出就降级成静态断言」，实测**造得出**）。
