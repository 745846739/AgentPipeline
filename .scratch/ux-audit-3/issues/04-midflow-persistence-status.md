# 04: 中流状态持久化余三件（详情页 `?tab=` / 看板 `?filter=` / 草稿 `talk_draft`）

**叠:** A（不动规格——规格由 ux-audit-2 票 [16](../ux-audit-2/issues/16-persistence-deeplink.md) / 决策 217 定）

**来源:** A/ux-audit-2 票 [22](../ux-audit-2/issues/22-midflow-persistence-impl.md)
（**wontfix**：余三件经 2026-10-01 复核由用户裁决收掉；已落的 router 底座与
班次两件照旧，决策 217①⑤）

**What to see:**
三处「刷新是否恢复」在真页面上逐条实测（`[r3] ③.1–③.3`）：

| 项 | 刷新前 | 刷新后 | 存活 |
|---|---|---|---|
| 详情页签 | `Diff` | `时间线`（默认） | ✗ |
| 看板过滤 | `已完成`（`aria-pressed=true`） | `全部` | ✗ |
| 对讲台草稿 | `r3-半句草稿` | `""`（空） | ✗ |

`location.hash` 在切页签/切过滤前后**不变**（`#/task/…` / `#/`），`localStorage` 键清单
刷新前后都只有 `["agentpipeline.theme","agentpipeline.talk_seen"]`——三件余项要的
`?tab=` / `?filter=` / `agentpipeline.talk_draft` **都不在地址、也不在本地留存**。

源码（本次实地读到，与 wontfix 的「余三件未落」一致）：
- 详情页签是组件局部态：`frontend/src/routes/TaskDetail.svelte:40` `let tab = $state<Tab>('timeline');`
- 看板过滤是本地态：`frontend/src/stores/board.svelte.ts:37` `filter = $state<StatusFilter>('all');`、`:168 setFilter`
- 草稿是局部态：`frontend/src/routes/Talk.svelte:1089` `let qdraft = $state('');`
- **已落的两处底座仍在**：`frontend/src/router.svelte.ts:177 readQuery` / `:193 writeQuery`；
  `frontend/src/lib/talkSessions.ts:21 TALK_SESSION_KEY`（班次进地址 + 兜底）——与票 22
  文末复核记载的 `partial` 状态完全对得上。

结论：**未修（wontfix 余三件维持，已落两处底座未回退，无回归）**。

**证据等级:** 实测（`[r3] ③.1–③.3` + 截图 `r3-tab-after-reload.png` /
`r3-board-filter-after-reload.png` / `r3-talk-draft-after-reload.png`）+ 代码
（`TaskDetail.svelte:40`、`board.svelte.ts:37,168`、`Talk.svelte:1089`；
底座 `router.svelte.ts:177,193`、`talkSessions.ts:21`）

**与前轮关联:** 现状核实（=前轮 22，wontfix；余三件原样未落，已落两件仍在）

**建议:** 维持 wontfix。若重开，余三件照票 22 清单逐条（`?tab=` push/replace 分档、
`?filter=` + `agentpipeline.board_filter` 兜底、`agentpipeline.talk_draft` 的
恢复/清零/7 天过期）。本轮**只记现状**。

**边界:** 审计票，不实现。
