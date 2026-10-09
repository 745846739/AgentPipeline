# 评审轮间台账：让「反复」变成可见的收敛曲线

## 背景（事实）

任务 `01M4CD59Y977ZQ0GMY9MPSFFMX` 两轮 `approved: false`，**第二轮打回的是第一轮修复
引入的回归**。要判断「在收敛还是发散」，当时唯一的办法是人工 diff 两份 `review-report.md`——
因为报告里没有「上轮改没改完 / 本轮新增」这两栏。

既有票 `.scratch/review-rework-feedback/`（决策 387）只做了**反馈注入落点**，并明确
**排除报告格式**（「不解析 review-report.md 摘录」，票 01 边界段）。全目录 + `docs/decisions.md`
搜「评审轮次 / 轮间 / 复审」**零命中**——轮间核对是空白地带。

## 三层（同一张票，可分批交付）

**L1 · 报告两栏**（改 `crates/core/src/agent/templates.rs::REVIEW_EX_SYSTEM` 的报告格式）
- 「上一轮 required_changes 逐条核对」：每条给 `改完 / 未改 / 部分`，未改的写清差在哪
- 「本轮新增」：本轮发现、上一轮不存在的问题，单独一栏
- 第 1 轮显式写「首轮，无上一轮」，不让模型自己脑补
- 两栏由评审 execute **结构化产出**（进 `submit_metadata`），不解析报告 markdown 正文

**L2 · 落库**（`ReviewResult` 扩字段）
- 轮数 `round` 与新增数 `new_findings` 入 stage metadata，旧输出 `serde(default)` 向后兼容
- 轮数可从 task 上的 review 次数推导，不依赖报告正文

**L3 · 面板**（前端评审卡片）
- 显示「第 N 轮 · 上轮 M 条已改 k · 本轮新增 j」
- 人裁决前一眼看得出是在收敛还是发散

## 硬边界

**不设自动放行、不设轮数上限。** review 不通过本来就走 `Pending(UserDecision)`
（`crates/core/src/pipeline/routes.rs:143`，决策 2/131），**停在人手里是对的**——
本票只做**可见性**，不做任何改变谁拍板的机制。已另行约定的止损判据（第三轮阻塞项若
不属本单 AC 范围 → 立新票 + 这单走通过）是**人工约定**，不进代码。

**不解析 `review-report.md` 正文**（决策 387 的边界照旧：markdown 无 schema，脆弱；
两栏由结构化字段承载）。
