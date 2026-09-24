# 01: 删掉 `conflict_overlap_threshold`——一个零读者的设置连同三处描述它的文档（决策 256 落档）

**What to build:** `conflict_overlap_threshold` 有字段、有默认值、有覆盖层项、在 `set!` 宏清单里，
**却没有任何生产读者**。真实的冲突判定是 `pipeline/model_invoke.rs:958-971` 的裸
`mine.files.iter().any(|f| theirs.files.contains(f))`——**没有阈值**。而它出现在**三处文档**里，
都写成「最小重叠文件数」。本票把字段与那三处描述一并收口；默认值 `0`（任一交集即冲突）**恰是代码
实际在做的**，故**行为零变化**。

**为什么删而不是补实现**：那个字段唯一有意义的取值是 `> 0`，而至今没人需要它。留着它等于留一个
「看起来能调、调了没用」的旋钮——**那比没有这个旋钮更坏**（用户会以为调它能改冲突判定，而放宽
冲突判定本身还要连带重审决策 71③ 的环消除口径）。判据见词条「**惰性设置**」。

**Blocked by:** None（可立即开始）

**Status:** done（已实现）

- [x] 删 `crates/core/src/config.rs:41` 的 `Settings::conflict_overlap_threshold` 字段
- [x] 删 `config.rs:117` 的 `Default` 项
- [x] 删 `config.rs:157` 的 `PipelineOverrides` 项
- [x] 删 `config.rs:204` 的 `set!` 宏清单项
- [x] 删 `config.rs:1246` 的 `assert_eq!(s.conflict_overlap_threshold, 0)` 断言行
      （`defaults_match_design_table` 里那一行——它断言的正是被删字段）
- [x] `docs/overview.md` §3 表：**删掉那一行**（不是改成「预留」——见词条「惰性设置」的判据）
- [x] `docs/pipeline-spec.md:36`：把「路径集合交集（≥ `conflict_overlap_threshold`）」改成
      「任一交集即命中，无阈值可调——`conflict_overlap_threshold` 已由决策 256 删除」
- [x] `docs/decisions.md:65` 决策 53：**行内标注**「**已由决策 256 修订**」+ 一句说明，**原文保留**
      （照决策 10 / 26 的先例）
- [x] `docs/testing.md` L1 用例目录：不需要加行（本票零测试面，行为无变化）

**零行为变化**（删的是零读者的字段与三处失实描述）。**验收**：`make check` 绿；全仓 grep
`conflict_overlap_threshold` **只剩决策日志里的历史记录**（决策 53 修订标注 + 决策 256 正文），
代码与其余文档零命中。

**明确不做**：不补 `conflict_overlap_threshold` 的实现；不动 `model_invoke.rs:958-971` 的判定本身；
不动决策 71③ 的环消除口径；不动 `tool_timeout_sec`（它是活的——见票 03 的姊妹条，只订正文档）。

---

## 实施收尾（2026-09-23）

`make check` 全绿（`check-lint` 0 / `check-test` 178 passed / `check-frontend` / `check-e2e` 120 passed）。
`cargo test -p agentpipeline-core --lib config::` 53 passed。

**残留 grep 只有历史记录**：`conflict_overlap_threshold` 现在只出现在
`docs/decisions.md:65`（决策 53 的行内修订标注）与 `:268`（决策 256 正文）——
**代码与其余文档零命中**，即本票的验收判据成立。
