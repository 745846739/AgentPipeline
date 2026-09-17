# 01: 失败 run 的会话落库

**What to build:** 让**失败的那一轮**也把会话写进 `kanban_node_conversations`。

今天的落库点只有三处：成功路径（`crates/core/src/pipeline/executor.rs:1463-1479`）、局部上下文溢出
路径（`:1287-1302`）、伪阶段成功（`:1633-1652`）。**重试耗尽、LLM 分类错误（401 / 429 / 网络）、
工具重试耗尽**这些路径攒在内存里的 `messages` 直接丢掉，只剩 run 行的 `error` 字符串。

于是 `read_conversation` 会踩一个静默的错：它的 `run_id` 是可选的、默认取**最新那一条**
（`crates/core/src/agent/tools.rs:1121-1173`）——值班长去查一个刚失败的任务，读到的是上游某个
**成功**阶段的会话，然后一本正经地误诊。

**实证（2026-09-17）**：任务 `01M2QH0DHKGSGNVHC0WT2Q4CG0`（支持rtk）在 `architect-design`
失败于「校验错误：未找到结构化元数据」，而 `kanban_node_conversations` 里该任务 **0 行**。
它到底是没看懂要求、还是工具描述有歧义、还是 prompt 缺了一条约束——**无从查起**。

**Blocked by:** None

**Status:** ready-for-agent

- [ ] `agent_node` 的重试耗尽分支（`executor.rs:958-971`）在返回错误**之前**落库会话
- [ ] LLM 分类错误（`Error::LlmClassified`）与网络失败路径同样落库
- [ ] 工具重试耗尽（`:1386-1392`）同样落库
- [ ] 与决策 99「会话与 run 1:1 落库」对齐：一条 run 至多一条会话行，attempt 区分
- [ ] 失败会话的 `error` 同时写进那一行的上下文（否则读会话的人不知道它为什么停在那里）
- [ ] **先改断言**：确认既有没有「失败不落库」的隐含约定被哪个用例钉着，先改它再加落库
- [ ] 新增用例：一次必然失败的节点跑完后，`kanban_node_conversations` 有且仅有一条行，
      且 `messages_json` 里能看到失败前的最后一次工具调用
- [ ] 与 `conversation_max_chars`（默认 20 万字符）的账对齐：失败会话同样走 `truncate_messages_json`

## 备注

这条是整批的**根**：它不落地，票 03 的诊断包就只有一个空壳，「分析出代码问题 / prompt 问题 /
环境问题」里的后两类**在物理上不可诊断**。
