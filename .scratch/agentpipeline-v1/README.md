# AgentPipeline v1 执行计划

来源：docs/ 设计文档（2026-09-12 定稿）对照 crates/ 实现现状逐条核对后拆分。
**22 个全部已实现（01–22，状态 done）**。票 19 的 E2E 矩阵补全同时暴露并修复了三个生产缺陷（见 [docs/testing.md](../../docs/testing.md) §11）。

## 依赖图

```
已完成 01–10（数据模型 / 存储层 / 图与路由 / Git 链 / 工具 / 上下文元数据 / HTTP API / Scheduler / 配置与进程治理 / 测试基建）
已完成 11（执行器骨架）─ 12（Prompt 模板与阶段配置）─ 13（生产 LLM 适配器）─ 14（并行分支与汇聚）
已完成 15（merge 收尾：rebase 自动合并 / gate_recheck 注入）─ 16（伪阶段：conflict_check / validator_cross_check / project_analysis）
已完成 17（生产接线：run_loop + maintenance + resume_hook 真 spawn + 进程组 pgid）─ 18（崩溃恢复端到端，决策 152 in-process）
已完成 20–22（前端：看板与实时流 / 任务详情与 resume / 配置与指标，另补 stage_configs CRUD 端点）

已完成 19（E2E 矩阵补全，依赖 14/15/16/17）
```

## 关键结论

- 声明式层全部落地且测试锁定：图拓扑、路由语义（决策 94/95/121/134/135/139）、游标状态机、Scheduler 六职责、§11.7 全部端点（含 stage_configs CRUD）。
- 执行器：FakeAgent 驱动完整流水线（happy path / join_and_skip / crash_recovery）、并行分支、sync-check 汇聚、skip 落点、merge 阶段 A/B 与闸门复检、三个伪阶段（决策 60/67/102/134/135/48/130）均有测试锁定。
- 生产侧已「活」：真实二进制启动 tick 循环 + 小时级维护，resume/review/merge-decision 真正驱动 executor，`run_command` 以独立进程组启动并回填真实 pgid。
- 测试面：testing.md §8 的 25 场景矩阵除 E2E-13（归票 18）外全部落地；两个由矩阵暴露的生产缺陷已修复并强化用例钉住。
- 已知行为级缺口（文档已定义、当前无生产者）见 [docs/testing.md](../../docs/testing.md) §11「尚未实现」。

