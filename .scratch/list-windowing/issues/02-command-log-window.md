# 02: 命令页签接线——切片（显尾部）+ 关键词 / 退出码过滤

**What to build:** `frontend/src/components/task/CommandLog.svelte:77` 的全量平铺
改为原语接线（默认尾部 50 条），并加两个过滤：命令行关键词、退出码（全部 / 非零 / 零）。
单条输出的 360px 滚动既有行为不动。

**Blocked by:** 01

**Status:** ready-for-agent

- [x] 切片接线 + 显尾部提示；过滤与切片可叠加（先过滤后切）
- [x] 过滤状态不进 URL（与折叠态同一口径，决策 217 类比）
- [x] 单测：过滤纯函数；既有 CommandLog 测试更新后全绿
