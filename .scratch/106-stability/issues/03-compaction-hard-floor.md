# 03: O4-A 压缩硬底——转录体量兜底触发,不看 provider 窗口脸色

**来源:** 同 01 的监控实录。ux-audit-3 任务(纯只读走查)在 develop 节点四连超时
(~90min/次),单 run prompt_tokens 高达 1490 万——每轮 `plan.request(&trace.messages)`
整卷转录重发,轮累计 O(n²)。L3 压缩本该拦,但三个松口叠加让它基本不触发:
容量判据看 provider 登记的 `context_window`(登记值大→软限 0.6× 也高)、
`keep_recent_rounds=5` 把大工具输出全保在保留区、撞窗自校准只上调
(`model_invoke.rs:837-858`)。

**Blocked by:** None

**Status:** todo

- [ ] 转录字符量超过 `conversation_max_chars`(现有常量,20 万字符)时**强制**
      触发 L3 压缩,与现有软限判据取「或」——provider 窗口登记失真不再能跳过压缩
- [ ] `keep_recent_rounds` 提为配置项(缺省仍 5),走查型任务可调小
- [ ] 撞窗自校准允许下调(校准值只反映「provider 真实能收多少」,双向都认)
- [ ] 回归测试:转录超 20 万字符的 agent 节点在 FakeAgent 上走完且压缩真实发生
- [ ] 续接回归:压缩后的转录被下一 attempt 续接时,锚点规则
      (`agent/context.rs:320-330`)仍成立、不空转

**验收.** 用本轮事故的形状做负载:一个只跑工具往返的节点跑满 90 分钟,
断言 run 的 prompt_tokens 总量被压在「无压缩理想值」的可解释倍数内,
且节点仍能正常产出。

**边界.** 不做 provider 前缀缓存(那是票 05);不改 `offload_threshold_tokens`
与 L1 裁剪口径;不动决策 278「转录不折叠不省略」的**收口**语义——压缩是
运行期行为,落库的会话行仍一字不删。

**Blocked by:** None(可立即开工;票 04/08 等它)
