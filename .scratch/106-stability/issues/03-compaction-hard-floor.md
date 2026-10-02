# 03: O4-A 压缩硬底——转录体量兜底触发,不看 provider 窗口脸色

**来源:** 同 01 的监控实录。ux-audit-3 任务(纯只读走查)在 develop 节点四连超时
(~90min/次),单 run prompt_tokens 高达 1490 万——每轮 `plan.request(&trace.messages)`
整卷转录重发,轮累计 O(n²)。L3 压缩本该拦,但三个松口叠加让它基本不触发:
容量判据看 provider 登记的 `context_window`(登记值大→软限 0.6× 也高)、
`keep_recent_rounds=5` 把大工具输出全保在保留区、撞窗自校准只上调
(`model_invoke.rs:837-858`)。

**Blocked by:** None

**Status:** done(2026-10-02,决策 378)

- [x] 转录字符量超过 `conversation_max_chars`(现有常量,20 万字符)时**强制**
      触发 L3 压缩,与现有软限判据取「或」——provider 窗口登记失真不再能跳过压缩
- [x] `keep_recent_rounds` 提为配置项(缺省仍 5),走查型任务可调小
      ——**落地时核实发现已存在**(`Settings` + `PipelineOverrides`,缺省 5),无需再动
- [x] 撞窗自校准允许下调(校准值只反映「provider 真实能收多少」,双向都认)
- [x] 回归测试:转录超 20 万字符的 agent 节点在 FakeAgent 上走完且压缩真实发生
- [x] 续接回归:压缩后的转录被下一 attempt 续接时,锚点规则
      (`agent/context.rs:320-330`)仍成立、不空转

**验收.** 用本轮事故的形状做负载:一个只跑工具往返的节点跑满 90 分钟,
断言 run 的 prompt_tokens 总量被压在「无压缩理想值」的可解释倍数内,
且节点仍能正常产出。

**验收实录.** 集成测试 `a_long_tool_round_trip_node_is_capped_by_the_char_floor`
(FakeAgent,60 轮 × 8 千字符写文件往返 = 转录 49 万字符、两度撞硬底,即 90 分钟
工具往返节点按同一常数等比缩放的形状):节点照常收口(Success);压缩真实发生
(转录出现 `[摘要]`);每个请求压在「硬底 + 可解释余量」内;请求总量低于
无压缩理想值(O(n²))的一半。测试写出来当场抓到一个真 bug:压缩摘要本身是
User 角色,下一轮压缩把它当「本轮第一条 user」锚点原样保留——旧摘要永不回收、
新摘要每次追加,长会话里空转累积;随锚点规则一起修掉(锚点候选跳过
`SUMMARY_PREFIX` 合成消息,决策 378 裁决③)。

**边界.** 不做 provider 前缀缓存(那是票 05);不改 `offload_threshold_tokens`
与 L1 裁剪口径;不动决策 278「转录不折叠不省略」的**收口**语义——压缩是
运行期行为,落库的会话行仍一字不删。
