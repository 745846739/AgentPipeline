# 02: 组装后的 prompt 原文落库

**What to build:** 把**当时实际注入模型的系统段与用户段原文**落库，而不只留一个哈希。

今天是这个样子：系统段由 `executor.rs:1189-1199` 拼（基线前言 + 工作目录 + AGENTS.md + persona +
技能 + 格式规则），用户段由 `:1203-1236` 拼（节点模板 + `PromptSegments` 的五个可选片段），
然后**只把系统段的 SHA-256 前 16 位十六进制**写进 `kanban_node_runs.prompt_template_hash`
（`:1237-1240` + `crates/core/src/agent/prompts.rs:226-231`）。**用户段连哈希都没有。**

而落库的 `messages` 里**没有**这两段——它们只在适配器组装 HTTP body 时才被前置
（`crates/core/src/agent/providers/openai.rs:30-37`、`anthropic.rs:37-56`），从不回写。
所以 `messages[0]` 不是系统段，也不是用户段。

**后果**：「这是 prompt 问题」这句话今天**没有任何可核对的证据**支撑。值班长能从磁盘上的
`prompts/` 与 persona 文件反推，而反推不等于当时那份——`PromptSegments` 的五个片段
（backtrack / user_input / gate_recheck / review_required_changes / retry_feedback）
是运行时拼的，只有其中一部分另有文件形态。

**Blocked by:** None

**Status:** done

- [x] 落库两段原文（同一行的两列，或 `kanban_node_conversations` 的两列），**不另起一张表、不另设保留期**
- [x] **先定权威**：`prompt_template_hash` 降级为「快速比对」的索引，原文是权威；两者同时写
- [x] 留存期与 `conversation_retention_days`（默认 30 天）同口径
- [x] 与 20 万字符账对齐：明确超限时的截断策略，且**截断要留标记**（不许静默截短）
- [x] 值班长侧可读（诊断包工具里带出来，见票 03）
- [x] 新增用例：跑完一个节点后，从库里取回的原文与当次 `LlmRequest` 的两段**逐字相等**
      （打在两段字符串上，不是打在长度上——长度相等而内容不同正是哈希看不见的那种漂移）

**实施收尾（2026-09-18）:**

- **落点选了会话行的两列**（`system_prompt` / `user_prompt`，迁移 0017），而不是 run 行：
  留存期就**不必另立**——两列跟着那一行走 `conversation_retention_days` 与归档 / 清理。
  落在 run 行则要回答「run 表谁清、清不清」，那是本批明确要避免的第二种保留期。
- **账的口径写死在 `truncate_conversation` 里**：三段（系统段 → 用户段 → messages）共吃
  `conversation_max_chars` 一本账，顺序固定。理由不是偏好，是证据的不可替代性——两段原文
  是「这是不是 prompt 问题」的唯一根据，而 messages 下一轮就重建了。
  **代价如实记**：messages 的可用预算因此变小（原文吃掉的量级是几 k～几十 k）。
- **既有的 `conversations_are_truncated_to_max_chars` 顺带对齐**：它原来用 120 字符的阈值，
  在新账下会被两段原文吃光、测不到「丢最旧轮次」这件事，故改成 40k 阈值 + 60k 的
  `write_file` 参数（撑破余量但原文装得下），并补一条整行不超阈值的断言。
- **截断标记算在预算里**：`truncate_text` 先扣掉标记长度再截正文，否则「截断后的长度」会
  随标记长度偷偷超出阈值，账就不闭合了。
- **五处写会话的路径全都带上原文**：主 agent（成功 / 上下文溢出 / 失败三条出口，失败那条
  最要紧——「为什么失败」的 prompt 证据正在那里）、伪阶段、子代理。子代理顺带把任务正文
  存进 session（`user_prompt`），它同时是 `user_prompt` 与首条 user message。
- **`insert_conversation` 加一个参数而不是重构签名**：函数上本来就有
  `#[allow(clippy::too_many_arguments)]`，加 `Option<PromptSnapshot>` 的影响面是 9 处调用点
  各加一个 `None` / `Some(...)`；换成结构体参数会把这 9 处一起改写，收益只有「少一个参数」。

## 备注

这一条**不是补丁，是一个决定**（它撑大字符账、要定留存期、要回答「谁跟谁是权威」），
所以单列成票，不许顺手做掉。

