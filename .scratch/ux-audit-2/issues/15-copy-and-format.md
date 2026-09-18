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

**Status:** open

- [ ] 市场的时间戳改用 `lib/format.ts` 的既有口径（全站一处）
- [ ] 手机访问页同一 `bind_source` 值只用一个说法（两处合一）
- [ ] Talk 两处去掉字面 `**`：要么走 `MarkdownView`，要么直接用真实样式（`<b>`/class）
- [ ] 单测：`formatDateTime` 的单一出处（若加了新用法，断言不再出现裸 `toLocaleString`）
- [ ] 单测 / 静态扫描：`Share.svelte` 里同一个 `bind_source` 分支的标签只出现一份定义
- [ ] e2e：非配对态触发配对提示，断言页面上没有 `**`（可用 `page.route` 造 403 / 非回环来源）
- [ ] e2e：市场的时间戳格式与阶段页一致

**边界.** 第 ③ 条要能在测试里造出未配对态，否则就把它降级成「静态断言页面上无 `**` 字面量」。
