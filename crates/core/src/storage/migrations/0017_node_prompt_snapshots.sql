-- 决策 211② / 票 02：组装后的 prompt **原文**落库。
--
-- 为什么必须落（三件今天断掉的事）：
--   * 落库的 messages 里**没有**系统段与用户段——它们只在适配器组装 HTTP body 时才被前置
--     （providers/openai.rs、anthropic.rs），从不回写。故 messages[0] 既不是系统段也不是
--     用户段，「这是 prompt 问题」这句话今天没有可核对的证据；
--   * 用户段**连哈希都没有**：`prompt_template_hash` 只对系统段、且只有 SHA-256 前 16 位；
--   * PromptSegments 的五个片段（backtrack / user_input / gate_recheck /
--     required_changes / retry_feedback）是运行时拼的，磁盘上的模板反推不出当时那一份。
--
-- 三条口径（票 02 的三问，逐条钉死）：
--   1. **原文是权威，hash 降级为「快速比对」的索引**：两者同时写、同时读得到。hash 还在
--      `kanban_node_runs.prompt_template_hash`，谁都没被取代——它回答的是「两次跑的是不是
--      同一份系统段」，原文回答的是「当时到底注入了什么」。
--   2. **留存期不另设**：落在这张表的行上，就跟着 `conversation_retention_days`（默认 30 天）
--      与整行的归档 / 清理一起走。另起一张表就要另立一套保留期，那是本批明确要避免的。
--   3. **字符账是同一本**：这两列与 `messages_json` 共享 `conversation_max_chars`
--      （默认 20 万字符）——原文先占（它是诊断的根据），余量给 messages；两侧各自截断且
--      **都留标记**，不许静默截短（静默截短会让读者把残缺的文本当成当时的原文）。
--
-- 可空：历史行与不走 LLM 的伪阶段 / system 节点没有这两段，空即「这一票之前落的」。

ALTER TABLE kanban_node_conversations ADD COLUMN system_prompt TEXT;
ALTER TABLE kanban_node_conversations ADD COLUMN user_prompt TEXT;
