# long-run-budget：重读型长任务的预算治理（ux-audit-3 复盘）

**状态（2026-10-06）：设计经 grilling 收敛（六轮），两张票可并行开工。**

## 起因与实证账

任务 `01M450DK2GKZP4PJ4FVRAFAGXZ`（ux-audit-3 第三轮深度 UX 审计，纯只读走查）墙钟
**24 小时 47 分**，拆账（106 服务器 `kanban_node_runs` / `kanban_transitions` / journalctl 实测）：

| 去向 | 耗时 | 证据 |
|---|---|---|
| 真实执行（44 次 run 合计） | ≈13.2 h | runs 求和 |
| 被 max_duration 硬墙作废的工作量 | ≈7 h | review.execute 超时 4 次、test.execute 超时 6 次、develop 1 次 90 分钟——模型一直在干活（流式心跳正常），只是 30 分钟档跑不完一段 |
| 深夜停摆等人工 | 9.7 h（10-05 14:43 → 10-06 00:27） | 空白重跑也超时后任务进 pending；值守轮整夜零动作（日志静默），直到用户手动 resume |
| 服务重启打断 | 2 个在跑 run 被杀 | 10-05 12:23 / 12:43 两次部署重启（cancel_origin=restart） |

token 侧：全程 1.29 亿（512 次请求累计，**单次最大 8.9 万、中位数 5.3 万**，94% 是
cache read——续接重放转录有提示词缓存兜底，成本没有直觉的那么邪乎）；字符硬底
`char_floor`（20 万字符）压缩触发 **51 次**（develop 22 / test 15 / review 12 / 设计 2），
每次压缩作废缓存前缀并压掉一部分工作上下文。

## grilling 拍板记录（设计树结论）

- **超时机制不动**（决策 66 双闸保留），只把重读型阶段的预算配足 → 票 01。
- **超时梯子不动**（决策 320/205 原样；简报前移撤销——缓存证据推翻「续接重放很贵」的前提）。
- **pending 兜底不加机制**（值守轮现状保留；那晚静默不立案，知情接受）。
- **部署避让不做**；自适应预算（P50/P90 升格参与判定）本轮不做，想做时单独走决策。
- **压缩硬底 token 化**：触发判据改「软限 OR token 硬底 `conversation_max_tokens`=300k」，
  字符硬底从触发判据删除、`conversation_max_chars` 退回只管落库截断；显式推翻
  决策 376 票 03 的字符硬底及其后「不改 `conversation_max_chars` 缺省」条目 → 票 02。
- **设置界面**：新增「管线压缩」设置卡 + GET/PUT 端点，暴露 `conversation_max_tokens`
  与 `keep_recent_rounds`；`conversation_max_chars` 不进 UI → 票 02。

## 效果预期（如实说）

票 01 治「一直在干活却被杀」——90 分钟级节点不再循环作废；票 02 对审计级任务等于
关闭压缩（本任务单次最大 8.9 万 token，够不着 30 万线），上下文连续性拉满，代价是
单请求携带上下文变大、cache read 计费随之涨。**9.7 小时停摆问题这两张票都不治**。

## 票序与依赖

- [issues/01-stage-timeout-budget.md](issues/01-stage-timeout-budget.md) —— 无阻塞，可立即开工
- [issues/02-token-floor-and-settings.md](issues/02-token-floor-and-settings.md) —— 无阻塞，可立即开工

两票互不依赖（01 在 scheduler 超时判定侧，02 在预算/压缩侧），可并行。
