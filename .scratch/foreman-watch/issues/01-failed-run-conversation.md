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

**Status:** done

- [x] `agent_node` 的重试耗尽分支（`executor.rs:958-971`）在返回错误**之前**落库会话
- [x] LLM 分类错误（`Error::LlmClassified`）与网络失败路径同样落库
- [x] 工具重试耗尽（`:1386-1392`）同样落库
- [x] 与决策 99「会话与 run 1:1 落库」对齐：一条 run 至多一条会话行，attempt 区分
- [x] 失败会话的 `error` 同时写进那一行的上下文（否则读会话的人不知道它为什么停在那里）
- [x] **先改断言**：确认既有没有「失败不落库」的隐含约定被哪个用例钉着，先改它再加落库
- [x] 新增用例：一次必然失败的节点跑完后，`kanban_node_conversations` 有且仅有一条行，
      且 `messages_json` 里能看到失败前的最后一次工具调用
- [x] 与 `conversation_max_chars`（默认 20 万字符）的账对齐：失败会话同样走 `truncate_messages_json`

**实施收尾（2026-09-18）:**

- **没有一条既有的断言钉着「失败不落库」**：`agent_metadata_failure_retries_then_pends` 只断言
  了 run 行（数量 = `agent_retry_max`、全部 `Failed`），对会话一个字都没说。故「先改断言」
  这一步落在**给它补断言**上（每个失败的 run 各有一条会话行、带错误上下文、messages 非空），
  补完全红 → 再动实现。
- **落库点集中在外框，不在三条分支上**：`agent_attempt` 拆成外框 + `agent_attempt_inner`，
  现场（`messages` / `tokens`）搬进 `AttemptTrace` 交给内里持有。理由是这道断链的**机制**：
  `?` 会把内存里的 `messages` 一起带走，逐条分支补调用等于把「哪些路径会失败」写成一份
  手工维护的清单——今天三条，明天新加一条又静默漏掉。外框一处兜住所有 `?` 出口。
- **`persisted` 标记保住 1:1**：成功落库（含上下文溢出的那条 pending 路径）都会置位；
  此后若在 `post_process` 上失败，外框走 `annotate_conversation_failure` 把错误上下文
  **并进**已写的那一行，不插第二行——决策 99 的口径不因失败路径而破。
- **失败原因落在 `metadata_json`**：`{"failed": true, "error": …, "classified": {kind, raw}}`。
  可归因的 LLM 失败额外带类别与原始诊断：`error` 回答「发生了什么」，`classified` 回答
  「该去改什么」，两者不是一回事（与 `Error::llm_classified` 的分工一致）。
- **落库失败不覆盖原错误**：只 `tracing::error!` 一条。调用方要带回去的是节点为什么失败，
  不是记账为什么失败。
- **testkit 加了 `Step::Fail`**（`.fail_llm(kind, message, raw)`）：此前没有任何办法让
  FakeAgent 报错，所以「LLM 报错那一轮留不留证据」在测试上根本无法表达。`mock_llm` 同步
  支持（真 HTTP 503），二进制冒烟那条路不会有 `Step::Fail` 的未覆盖分支。
- **本票不动 `read_conversation`**：它的 `run_id` 缺省取最新一条，此前会静默读到上游某个
  成功阶段的会话；失败行落库后，「最新一条」正是那个失败现场，误导自动消失。

**备注**

这条是整批的**根**：它不落地，票 03 的诊断包就只有一个空壳，「分析出代码问题 / prompt 问题 /
环境问题」里的后两类**在物理上不可诊断**。
