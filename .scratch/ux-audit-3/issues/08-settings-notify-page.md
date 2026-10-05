# 08: 离线通知设置页（漂移面）可达性与语义 —— 兼「字面星号」同型新开

**叠:** A（不动规格）

**来源:** B/第二轮收口之后的代码漂移面（`SettingsNotify` 页新增）；
附带复核第二轮票 [15](../ux-audit-2/issues/15-copy-and-format.md) ③ 的「字面 Markdown 星号」

**出处:** `frontend/src/routes/SettingsNotify.svelte:408,433,458,490,492,498,499,577`；`frontend/src/routes/SettingsTools.svelte:160`；`[r3] ④.1/④.2/⑤.2`；截图 `r3-settings-notify.png` / `r3-literal-asterisks.png`

**严重度:** 中（面向用户的字面 `**` 星号原样渲染，文案破相且与「强调」意图相悖）

**What to see:**
真页面走查 `#/settings/notify`（`[r3] ④.1` / `④.2`）：

```
main=1  hScroll=0（无横向溢出）
h1: 设置 · 离线通知
h2: 总开关 / 现在生效的 / 通道 / 订阅 / 礼貌
通道四态：radiogroup=1，role=radio + aria-checked
  [{通用 webhook, checked=true}, {飞书机器人, false}, {iMessage（Blu…, false}, {浏览器推送, false}]
表单字段标签：webhook 地址（含 token） / 节流（秒） / 免打扰开始（整点） / 结束（整点）
```

可达性与语义**成立**：单一 `<main>`、恰一个 `<h1>`（`SettingsNotify.svelte:408`）、
五个 `<h2 class="sec-title">`（`:433,458,490,587,685`）；通道那一格是**真 `radiogroup`**
（`:492 role="radiogroup" aria-label="通道类型"` + `:498 role="radio"` + `:499 aria-checked`），
键盘/读屏语义齐。

**回归点（本票的重点）**：第二轮票 15 ③ 同型的「字面 `**` 星号」在设置页上**新开**
（`[r3] ⑤.2`）——真缺陷只有 **1 处**（初稿把有意设计的 `***` 掩码误计为缺陷，已按评审订正）：

```
已存时显示 ***；掩码或留空 = 沿用已存值。            （掩码 `***` 是有意设计，不计缺陷）
保存的是**整体覆盖**：四件要么全来自界面、要么全来自配置文件，不允许混。  （SettingsNotify.svelte:577，Svelte 不解析 Markdown）
```

这一处即票 15 ③ 同型的缺陷（模板纯文本里的 `**整体覆盖**` 不会加粗，原样显示）。同批还量到
`#/settings/tools` 的 `:160` 也有一处 `**不改写**`（`[r3] ⑤.2`，count=1）。
票 15 ③ 当年只在 `Talk.svelte` 上记录（且配对态未在页面复现、标「代码」级）；
**本轮把同型缺陷在设置页上实测到了**。

结论：**新开（同型）**（票 15 ③ 的验收点——`Talk.svelte` 无字面 `**`——至今成立，`:1576,:2087` 均已是 `<b>`；设置页是同型缺陷的新址，不构成回归）
页面本身可达、语义完整。

**证据等级:** 实测（`[r3] ④.1/④.2/⑤.2` + 截图 `r3-settings-notify.png` /
`r3-literal-asterisks.png`）+ 代码（`SettingsNotify.svelte:408,433,458,490,492,498,499,577`）

**与前轮关联:** 新开（同型；票 15 ③ 已在 `Talk.svelte` 落 `<b>` 修掉，本页是新址）

**建议:** 把面向用户的 `**…**` 改成真的强调元素（`<b>`），或走一处 Markdown 渲染管道。
本轮**只记现状**，不实现。

**边界:** 审计票，不改代码。
