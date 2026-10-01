# 22: 中流状态的留存与深链

**叠:** A（不动规格——规格由 [16](16-persistence-deeplink.md) / 决策 217 定）

**来源:** R2-21；由 [16](16-persistence-deeplink.md)（决策 217）开出

**What to build:** 详情页签、看板过滤、对讲台班次与输入草稿全是局部 `$state`：F5 一律打回
默认（页签回时间线、过滤回「全部」、班次回最近且草稿丢失），前进/后退也恢复不了；
全站 URL 里只有 `?task=` 与 `?project=&analyze=1`，localStorage 只有
`agentpipeline.project_id` / `.theme` / `.pairing` 三把 key。

**Blocked by:** 16（哪一项去哪儿、谁写地址、刷新与后退的语义由它定，实现照抄）

**Status:** wontfix（余三件——详情页 ?tab= / 看板 ?filter= / 输入草稿 talk_draft——经 2026-10-01 复核后由用户裁决收掉；已落地的 router 底座与班次两件（决策 217①⑤）照旧，复核记录见文末）

- [ ] 路由层支持读取 query（现只有 `?task=` / `?project=&analyze=1` 两个就地读取的点）：
      统一一个 `readQuery()` / `writeQuery(patch, {replace})` 口子，避免四页各写一遍
- [ ] 详情页签 `?tab=`：用户点页签 `pushState`、程序改页签（触发节点直达 / 打开产出文件）
      `replaceState`；缺省 `timeline` **不写进地址**；非法值回落缺省
- [ ] 看板过滤 `?filter=` + `agentpipeline.board_filter` 兜底（URL 权威，变化时两处都写；
      URL 里没有时用 localStorage；非法即删键）
- [ ] 对讲台班次 `?session=` + `agentpipeline.talk_session` 兜底（同上）
- [ ] 对讲台输入草稿 `agentpipeline.talk_draft`（`{sessionId, text, at}`）：装载恢复、
      发送成功立刻清零、`at` 早于 7 天装载时清零
- [ ] 会话页签里选中的 run **两处都不进**（它是页签内部的滚动位置类状态，且自动联动会改它）
- [ ] 单测：query 读写（缺省不写、非法回落、`replace` vs `push`）、草稿的恢复/清零/7 天过期
- [ ] e2e：切到 Diff 页签 → F5 → 仍在 Diff；切过滤 → F5 → 仍是该过滤；写半句草稿 → F5 →
      草稿还在 → 发送成功后 F5 → 草稿没了；切页签 → 后退 → 回到上一个页签；
      触发节点直达**不**新增历史条目（`history.length` 不变）

**边界.** 地址里只允许决策 217 列的短枚举参数（连同既有 `task=` / `project=` / `analyze=`）；
参数总数 ≥5 或出现自由文本时停下来重新裁决，不自行扩张。不新增「清空本地状态」界面。

## Comments

- **2026-10-01 复核**：本票不是全开。已落地：router 底座（pushState/replaceState 语义，`frontend/src/router.svelte.ts`，注释即按决策 217 写的）与对讲台班次两件（`lib/talkSessions.ts`：`?session=` 进地址 + `agentpipeline.talk_session` 兜底、`agentpipeline.talk_seen` 看过时刻表——决策 217①⑤）。仍未落地三件：① 详情页签 `?tab=`（`tab` 仍是 `TaskDetail.svelte` 组件局部 `$state`）；② 看板过滤 `?filter=` + localStorage 兜底（`Board.svelte` 只有 `board.filter` 本地态）；③ 对讲台输入草稿 `agentpipeline.talk_draft`（`qdraft` 仍是局部态、发送后无清零键）。Status 相应 open → partial。
