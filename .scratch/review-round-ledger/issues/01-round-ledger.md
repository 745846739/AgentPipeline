# 01: 评审报告加轮间两栏 + 轮数落库 + 面板显示

**来源:** 任务 `01M4CD59Y977ZQ0GMY9MPSFFMX`（文案纪律扩面）两轮 `approved: false`，
第二轮打回的是**第一轮修复引入的回归**（`8bee901` 重构 `findHalfMixed` 扫描顺序时把
`FILENAME_TOKEN` 提到 `STAGE_ID` 之前，5 枚阶段 id 只剩 `sync-check` 能命中）。判断
「在收敛还是发散」只能人工 diff 两份报告。决策 387 那张票只做了反馈注入落点、**明确
排除报告格式**；「评审轮次 / 轮间核对」在票与 `docs/decisions.md` 全目录零命中。

**What to build:** 三层，可分批交付（L1 独立可用，L2 是 L3 的前提）：

- [ ] **L1 报告两栏**：`REVIEW_EX_SYSTEM` 报告格式加「上一轮 required_changes 逐条核对
      （改完 / 未改 / 部分）」与「本轮新增（上轮不存在的）」两栏；首轮显式写「首轮，
      无上一轮」；两栏进 `submit_metadata` 结构化字段，不解析报告正文
- [ ] **L2 落库**：`ReviewResult` 扩 `round` / `new_findings`，`serde(default)` 向后兼容
      （旧输出无此字段不炸）
- [ ] **L3 面板**：前端评审卡片显示「第 N 轮 · 上轮 M 条已改 k · 本轮新增 j」
- [ ] 各层用例：L1 口径用例（首轮不脑补、第二轮两栏都在）、L2 序列化兼容用例（缺字段
      走默认值）、L3 组件断言

**Blocked by:** None（可立即开工）

**Status:** ready-for-agent

**边界.** **不设自动放行、不设轮数上限**——review 不通过本来就走 `Pending(UserDecision)`
（`routes.rs:143`，决策 2/131），停在人手里是对的，本票只做可见性、不改变谁拍板；
不解析 `review-report.md` 正文（决策 387 边界照旧）；不改 review 的静态评审定位与
`review_mode=human` 通路。

## Comments

### 2026-10-09 · triage 裁决 → ready-for-agent（无开放选型）

三层结构与硬边界已在 spec 收口（不自动放行、不设轮数上限、不解析报告正文），按
**L1 → L2 → L3** 分批交付：L1 独立可用，L2 是 L3 的前提，每层独立过闸。
**开工约束**：`templates.rs` / `routes.rs` / `model_request.rs` 与并行会话在飞改动重叠——
独立 worktree 或等其收口；三票建议序 03 → 01 → 02（本票不阻塞任何事，可最后）。
