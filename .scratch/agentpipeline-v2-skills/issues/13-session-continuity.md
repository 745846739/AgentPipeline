# 13: 会话续接参数化

**What to build:** 新增阶段级 / 节点级参数（**默认 false**：每次 attempt 干净对话，与今天逐字相同）。
开启后，pending → resume 重入时从 `kanban_node_conversations.messages_json` 读回上一 attempt 的
messages 作为起点——信息补充型 pending 不必让 agent 从零重读一遍仓库。

会话**已经落库**（每次 attempt 结束时写入），`info_insufficient` 这条 pending 在写库**之后**才返回，
所以上一轮对话在 resume 时可读。本票只差「读回来 + 三项必要条件」。

**Blocked by:** 14（修正 `attempt` 语义——续接靠「读 attempt N-1 的会话」，而 `attempt` 今天会被伪阶段 run 行虚增，不过滤 `agent_type` 会读到伪阶段的会话）

**Status:** done

- [x] 参数默认 **false**，默认路径与今天逐字相同（现有测试全绿）
- [x] 开启后从会话行读回上一 attempt 的 messages 作为起点
- [x] **必要条件一：`context_overflow` 退出路径补写会话行**——该 pending 在会话写入之前返回，
      今天没有行可读，否则会出现「参数开了却静默无效」的路径
- [x] **必要条件二：token 双算防护**——任务总量今天对全部 run 行盲求和，续接会把历史对话的输入
      token 在新 run 里再报一遍；须给续接的 run 打标记并在汇总时排除被续接的历史
- [x] **必要条件三：压缩锚点修正**——`compact_messages` 保留「第一条 user 消息」，载入历史后那条是
      **上一轮**的提问，会占掉 keep 预算；载入的 messages 不得被当作本轮锚点
- [x] wire 顺序不变：system 恒为 `messages[0]`、user 恒为 `messages[1]`（有既有测试钉住）
- [x] **补一条锁住当前行为的断言**：今天没有任何测试钉住「每次 attempt 对话为空」——改动前先补，
      否则续接开关会改掉既有语义而无安全网
- [x] `agent_retry_max` 的干净对话重试语义（决策 33）**保持不变**；续接只作用于 pending → resume 边界
- [x] 用例：默认关时对话为空 / 开启后第二轮 messages 含上一轮 / 三项必要条件各有断言 / token 不双算

**Notes（实现提示）:**
- 最近先例是 E2E-21（`info_insufficient` 后重入的 prompt 内容断言），续接只需再加「第二轮 messages
  是否含上一轮」的断言——复用同一测试骨架。
- 与票 14 的关系（**硬前置**）：`next_attempt` 今天会把伪阶段 run 行算进去而虚增 `attempt`，续接若
  依赖「读 attempt N-1 的会话」会读到伪阶段会话。故票 14 必须先合入。
