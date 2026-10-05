# 04: 中流状态持久化：详情页 `?tab=` / 看板 `?filter=` / 草稿 `talk_draft` 三件余项

**出处:** 待填：`frontend/src/routes/TaskDetail.svelte`（`let tab = $state`）、
`frontend/src/stores/board.svelte.ts`（`filter = $state`）、`frontend/src/routes/Talk.svelte`（`qdraft`）
的行号 + 实测（刷新后状态是否还在）
**严重度:** 待填
**与前轮关联:** 待填（前轮票 [ux-audit-2/22](../../ux-audit-2/issues/22-midflow-persistence-impl.md)，partial）
**证据等级:** 待填

**证据占位:** 待填：详情页切 Diff 页签 → `reload()` → 读当前页签；看板设过滤 → 刷新 → 读过滤；
对讲台输入半句 → 刷新 → 读输入框内容；顺带记 `location.hash` 与 `localStorage` 的
`agentpipeline.*` 键清单。
