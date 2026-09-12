# 07: HTTP API 全表面

**What to build:** implementation.md §11.7 的全部约 30 个端点：任务 CRUD/旁路动作（resume/retry/cancel/split/model-override/archive/review/merge-decision）、查询（flow/metrics/conversations/commands/files）、项目管理（含异步 analyze 202+轮询）、providers CRUD（api_key 掩码）、全局指标；SSE 唯一事件通道与跨源写守卫。

**Blocked by:** None（can start immediately）

**Status:** done（已实现；注意：resume/review 的 spawn 侧在执行器落地前是空 hook，归票 11/17）

- [x] §11.7 全端点实现并有契约测试（35 例，含跨源矩阵、SSE branch 字段、files 路径逃逸 403、命令跨任务 404）
- [x] resume：唯一游标规则（多条无 cursor_id → 409）、动作白名单、冷却防连点（决策 91/101）
- [x] providers api_key 只回显 ***，patch 忽略 ***（决策 112）
- [x] analyze 异步 202 + 轮询（决策 130；LLM 摘要部分归票 16）
- [x] 指标端点：成功率/阶段聚合/逃逸率/首过率（决策 100/130/131/137）
