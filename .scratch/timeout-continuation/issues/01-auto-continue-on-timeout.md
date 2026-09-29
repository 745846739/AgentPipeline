# 01: 超时类重试改自动续接——上限 2 次，降级空白 1 次，再挂起

**What to build:** 节点超时（idle / max_duration）后的自动重试从**空白重跑**改为
**自动续接**（带 `continued_from_run_id`，与手动「继续」同路，`crates/core/src/pipeline/
resume.rs` 的既有机制）：自动续接最多 2 次 → 降级回空白重跑 1 次 → 再超时挂起交回人工
（`pending_reason`）。次数**写死不配**（少一份组合），数字进决策日志；与托管止损
「满 2 次即停」（决策 210）同一量级。错误类别分流（决策 298）的既有语义不回退——
只改超时那一类的重试形态。

**Blocked by:** None

**Status:** ready-for-agent

- [ ] 超时判定的重试支（`scheduler/mod.rs::handle_timeout` → advance 重试流转）带
      `continued_from_run_id`，续接计数记在游标 / run 链上
- [ ] 第 3 次续接尝试改空白一次，第 4 次超时挂起 pending
- [ ] 非超时类（传输 / 配置 / 校验）重试形态不变（决策 278 / 298 用例照旧绿）
- [ ] L2 集成：模拟两次超时 → 第三次 run 带 continued_from_run_id；第四次空白；第五次挂起
- [ ] 决策日志追加：显式修订决策 298 的超时支（重跑 → 续接）与决策 226 的收场说明
