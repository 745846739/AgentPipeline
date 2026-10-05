# 11: 决策 240 入口归一：wordmark 去链、看板回根路由

**叠:** A（不动规格）

**来源:** B 漂移面（决策 240，2026-09-21）

**出处:** `frontend/src/components/layout/TopBar.svelte:46,57-66,151-154,227`；`[r3] ④.4`

**严重度:** 低（入口归一已落，无缺陷）

**What to see:**
真页面走查 `#/` 的顶栏（`[r3] ④.4`）：

```
.wordmark  tag = SPAN   isLink = false   href = null   text = "AGENTPIPELINE"
顶栏页面导航行 navItems = ["对讲台", "看板", "指标", "设置"]   ← 恰四项
```

与决策 240 逐条对上：`frontend/src/components/layout/TopBar.svelte:154` 是
`<span class="wordmark">AGENTPIPELINE</span>`——**铭牌，不是入口**（`:151-153` 头注写明
「它原先是 `href="#/"` 的看板入口……留字、去链」）。看板已收进顶栏导航行
（`:227` `<nav class="navbar" aria-label="页面导航">`），`NAV` 表（`:46` 起）恰四项：
`/talk` 对讲台、`/` 看板、`/metrics` 指标、`/settings` 设置——与决策 240「修订决策 198
（导航行三项 → 四项）」一致。设置子页（foreman / notify / tools）也在 `settings` 那一枚的
`routes` 集里（`:57-66`），从落地页点进去设置页签不会当场灭掉。

结论：**已修**——决策 240 的入口归一在真页面上成立：wordmark 是 `<span>` 不可导航、
页面导航行恰四项且含看板。无缺陷。

**证据等级:** 实测（`[r3] ④.4`）+ 代码（`TopBar.svelte:46,57-66,151-154,227`）

**与前轮关联:** 新开（B 漂移面；决策 240 本身是第一轮票 21「设置 IA / 顶栏」的后续收敛）

**建议:** 无。

**边界:** 审计票，不实现；不判定 `.railnav` 信号灯跳段（决策 242① 已随铭牌行退场作废，
那是窄档版面，见票 12）。
