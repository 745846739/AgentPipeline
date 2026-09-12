# 10: project_analysis 独立观测行

**What to build:** `project_analysis` 伪阶段目前传空任务 id 与 run_id 0、不落 run 与会话行，决策 100 的「伪阶段独立观测」对它是空的，也不计入 `total_calls`（决策 130 ②）。task 内伪阶段（conflict_check / validator_cross_check）已按决策 100 / 113 合规落库，本票把项目级这条补平。需先定项目级 run 行的落库口径（无任务、无游标时外键与归属如何表达），再实现。

**Blocked by:** None (can start immediately)

**Status:** done

- [ ] 项目级伪阶段的 run / 会话行落库口径在 `docs/data-model.md` 写明（含无任务场景下的归属与外键处理）
- [ ] 落库路径落地，`total_calls` 口径与决策 130 ② 一致（含或不含，需明确并有用例钉住）
- [ ] LLM 不可用时的既有降级（保留确定事实 + 记摘要错误）不回退
- [ ] 用例覆盖：调用项目分析后可查到该次 run / 会话行
