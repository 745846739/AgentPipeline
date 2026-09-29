# 04: 看板列——列内上限折叠（显头部）

**What to build:** `frontend/src/components/board/BoardColumn.svelte:97` 全量渲染
改为原语接线（显头部）：一列超过上限时显示前 N 张 + 「还有 M 张，加载更多」；
顶栏状态粗过滤（all/running/pending/…）既有行为不动。

**Blocked by:** 01

**Status:** done（2026-09-29 实现，决策 319）

- [x] 列内上限 + 加载更多；换过滤档时页游标重置
- [x] 卡内 PendingActions 加高不在本票范围（既有形态不动）
- [x] 单测：按列分组后切片正确；既有 Board 测试全绿
