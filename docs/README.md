# AgentPipeline 设计文档

Kanban 式流水线驱动的本地多 agent 开发管线：init → architect-design → develop-design / test-design（并行）→ sync-check → develop → review → test → merge → done。

> 设计定稿于 2026-09-11，经六轮评审（g1–g6）；g7（2026-09-12）落防错误放大改进点（review 设计对照、异族复判、验收标准 traceability、lint 闸门等，决策 133–139）。文档由原单文件 agent-pipeline.md 拆分而来，章节编号与决策编号保持拆分前不变。

## 文档地图

| 文件 | 内容 | 原章节 | 什么时候读 |
|---|---|---|---|
| [overview.md](overview.md) | 流程图、全局约束（G1–G15）、全局配置参数表 | §1–3 | 第一次接触系统 / 查全局参数 |
| [data-model.md](data-model.md) | 任务主结构、阶段输入输出（submit_metadata 权威定义）、Pending→Resume 映射 | §4–5 | 对接数据结构与状态机字段 |
| [pipeline-spec.md](pipeline-spec.md) | 各节点详细流程、并发分支同步、幂等性、异常处理汇总 | §6–9 | 推敲流转语义、实现 executor |
| [agents.md](agents.md) | Tool 定义、各节点 Prompt 模板、节点级重试、System Prompt 管理、阶段级 Agent 配置 | §10 | 调优 prompt、改 agent 行为（高频变更区） |
| [implementation.md](implementation.md) | 流水线拓扑（落点表与条件边）、executor、KanbanScheduler、数据库表、进程中断恢复、关键接口 | §11 | 写实现代码 |
| [operations.md](operations.md) | worktree 隔离、成本、取消、可观测性、人机协作、依赖、通知、锁、上下文管理、**工具层出口控制与残余风险**、**配对令牌与局域网态势**等 16 项横切设计 | §12 | 处理横切关注点 |
| [testing.md](testing.md) | 测试设计（t1 新增，决策 140–152）：风险优先级、可测试性接缝、FakeAgent/testkit 基建、单元/集成/API/E2E 用例目录、质量闸门与决策↔测试映射 | — | 写实现代码前定接缝、写测试时查用例 |
| [decisions.md](decisions.md) | 已确认设计决策 #1–#249（只追加，修订关系显式标注） | §13 | 查"为什么这样定" |
| [glossary.md](glossary.md) | 领域术语表 | 附录 A | 遇到不认识的术语 |
| [backlog-v2.md](backlog-v2.md) | v2 预留（MCP、对话 agent、离线通知等）+ §B.8 界面整备的待改进项（隐喻词表落地缺口、依赖任务 ID 完整选择器、共享基元缺失等） | 附录 B | 规划 v2 |

## 引用约定

- **章节编号**：`§4.2` 这类引用按上表"原章节"列定位到对应文件（如 §4.2 → data-model.md）。
- **决策编号**：`决策 85` 一律指 [decisions.md](decisions.md) 的 #85，编号只增不改、被修订时在行内标注。
- **前端规格**：交互骨架与页面元素定稿在 [design/frontend-design.md](../design/frontend-design.md)（§4–§7）；视觉方向以**主题六「像素机房 · 夜班流水线」**为准（[theme-6-pixel.md](../design/theme-6-pixel.md)，决策 169）。operations.md §12.11 是前端与流水线之间的契约侧（流式、组件复用）。
- **主题方案**：选型已定为主题六「像素机房 · 夜班流水线」——深色「夜班靛」/ 浅色「掌机背光」× 桌面/移动四款原型（[theme-6-pixel.md](../design/theme-6-pixel.md)、[prototype-pixel.html](../design/prototype-pixel.html)、[prototype-pixel-light.html](../design/prototype-pixel-light.html)、[prototype-pixel-mobile.html](../design/prototype-pixel-mobile.html)、[prototype-pixel-mobile-light.html](../design/prototype-pixel-mobile-light.html)）；主题三「终端 · 调度电报」与其余四款候选（夜间调度台 / 日间时刻表 / 蓝图「晒图房」/ 车间「工单板」）均已归档至 [design/deprecated/](../design/deprecated/README.md)。
- **公开范围（票 11）**：上面引用的两份设计规格（[frontend-design.md](../design/frontend-design.md)、[theme-6-pixel.md](../design/theme-6-pixel.md)）、四款主题六原型（`design/prototype-pixel*.html`）与决策日志 [decisions.md](decisions.md) 都**随仓库公开**——界面文案里的规格 / 决策引用对公开读者可达。（历史方案 `design/deprecated/` 同样公开；`.scratch/` 是工作区，其下截图不入库。）
- **许可（票 11）**：本仓库以 **MIT** 许可公开，见仓库根 [LICENSE](../LICENSE)，与 `Cargo.toml` 的 `license = "MIT"` 同口径。
