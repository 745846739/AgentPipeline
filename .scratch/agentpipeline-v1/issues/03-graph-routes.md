# 03: 图、落点表与路由函数

**What to build:** 声明式流转层：petgraph 十阶段 DAG（含 architect-design 分裂、merge/review/sync-check 回边）、每阶段节点集例外表、entry_node / skip 落点表、集中路由函数（readiness 按阶段定 pending 类型、validate_output 重试/耗尽、merge 路由全语义、code gate 分流）。

**Blocked by:** None（can start immediately）

**Status:** done（已实现）

- [x] 图拓扑与 §1.1/§1.2 一致，节点集例外（init/sync-check/merge/done 单节点；develop/review/test 跳过 validate_input）
- [x] entry_node 查表（决策 69）与 skip 落点表（决策 93，含并行分支不越过 join）
- [x] route_merge：pending 不推进、gate 缺省 NoOp、耗尽收口、lint→develop / test→test 分流、None|Returned→NoOp（决策 85/95/108/121/139）
- [x] readiness 按阶段定 pending 类型（决策 94）；code gate 的 test_issue/code_issue 分流（决策 62）
- [x] 异族复判判定函数 resolve_validate_output + JudgeDisagreement 语义 + cross_family_judge fail-fast（决策 134/135）——判定逻辑完成，执行调用点属票 16
