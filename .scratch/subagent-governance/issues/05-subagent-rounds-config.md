# 05: 子代理轮数上限改配置（全局，默认 200）

**What to build:** `SUB_AGENT_MAX_ROUNDS = 12` 这个编译期常量退役，换成全局配置
`sub_agent_max_rounds`（config.toml `[pipeline]`，缺省 **200**）。

**为什么**：12 轮是照着「防御模型读一个文件→再读一个的空转」定的，但它在真实任务里
直接把三个子代理全部打死（2026-10-08，见 README 实证账）。用户裁决：上限该是**配置**，
不是常量——12 与其他任何数字一样是拍的，让运维面能调。时间界另有 `max_duration`
（节点级，缺省 1800s）兜着，轮数与墙钟两个界各管一段。

**形状**：

- `Settings.sub_agent_max_rounds: usize`，缺省 200；与 `node_max_duration_sec` /
  `agent_retry_max` 并列（同一层：全局数值界住 config.toml）。
- 取用点：`SubAgentRunnerConfig` 构造处（`model_invoke.rs`）读一次进 cfg，
  `run_rounds` 从 cfg 读——**不再有编译期常量**。
- 非法值（0）不得变成「无限」：按 1 兜底（或启动即拒），不许出现「0 = 无上限」。
- 失败文案里的数字跟着配置走（不再写死 12）。

**Blocked by:** None

**Status:** ready-for-agent

- [ ] 用例：缺省 = 200（钉住数字）；配置成 N 时子代理最多跑 N 轮
- [ ] 用例：0 / 非法值不产生「无上限」行为
- [ ] 既有「打满即报错」用例跟着常量走，不写死 12（照决策 292 那条的姿势）
