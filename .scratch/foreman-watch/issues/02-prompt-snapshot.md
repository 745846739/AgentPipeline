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

**Status:** ready-for-agent

- [ ] 落库两段原文（同一行的两列，或 `kanban_node_conversations` 的两列），**不另起一张表、不另设保留期**
- [ ] **先定权威**：`prompt_template_hash` 降级为「快速比对」的索引，原文是权威；两者同时写
- [ ] 留存期与 `conversation_retention_days`（默认 30 天）同口径
- [ ] 与 20 万字符账对齐：明确超限时的截断策略，且**截断要留标记**（不许静默截短）
- [ ] 值班长侧可读（诊断包工具里带出来，见票 03）
- [ ] 新增用例：跑完一个节点后，从库里取回的原文与当次 `LlmRequest` 的两段**逐字相等**
      （打在两段字符串上，不是打在长度上——长度相等而内容不同正是哈希看不见的那种漂移）

## 备注

这一条**不是补丁，是一个决定**（它撑大字符账、要定留存期、要回答「谁跟谁是权威」），
所以单列成票，不许顺手做掉。
