# 值班长能力面·边界内叠（决策 262①）

> 起源：2026-09-24 triage（决策 262）判「**该补**」的三项。与破边界叠
> （`.scratch/foreman-capability-gaps/`，票 01/02 已随决策 265/266 done）不同，
> 这三项**不修订任何既有裁决**——落点全在现有边界之内（B 层只读、attention
> 信号面、对话上下文预算），故开票即可做；裁决只裁**实现形状**。
> 立项顺序即实现顺序（决策 262① 的陈述顺序）。

| 票 | 状态 | 关键接缝 |
|---|---|---|
| [01 内容搜索](issues/01-content-search.md) | done（2026-09-24，决策 267） | `FOREMAN_TOOL_SPECS`（23→24）或 `READONLY_COMMANDS`；域校验 + 台账 |
| [02 离线通知渠道](issues/02-offline-notification.md) | done（2026-09-24，决策 268） | attention 生产点 / `NotificationPolicy` 移端；`[notify]` 配置段 |
| [03 上下文管理](issues/03-context-compaction.md) | done（2026-09-24，决策 269） | `trim_history`（`pipeline/foreman.rs`）+ `llm.complete` 复用；两条既有钉子不能破 |
