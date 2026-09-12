# 01: 数据模型与类型系统

**What to build:** 全量领域数据模型与 Pending→Resume 动作映射：任务主结构、活跃游标、九类 pending 原因、merge_result（gate 与 approval 正交）、各阶段 StageIO 结构；以及代码级动作表——由 (pending type, context kind) 生成 allowed_actions，side_effect 动作与端点一一配对。

**Blocked by:** None（can start immediately）

**Status:** done（已实现）

- [x] types.rs 覆盖 data-model.md §4 全部结构（cursors/depends_on/blocks 在 API 层组合，不落库）
- [x] 九类 pending 原因 + PendingContext（决策 82/94）
- [x] allowed_actions 权威表 + endpoint_for 配对，静态测试保证每个 side_effect 都有端点（决策 49/69/101/119/130/132/138）
- [x] Gate 无 Default、缺省绝不视为通过（决策 95）
