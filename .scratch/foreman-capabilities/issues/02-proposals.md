# 02: 提议接缝——存储、过期、执行端点（**不含任何新工具**）

**What to build:** 整批的承重墙。先把这条接缝单独跑通，一个写工具都不加。

- **表**（迁移 0015，落地时以实际空号为准）：`kanban_foreman_proposals(id, session_id, tool,
  args_json, summary, created_at, expires_at, status, resolved_at)`，
  `status ∈ {pending, executed, rejected, expired}`。`session_id` 来自 `foreman-sessions` 01。
- **拦截点**：在执行点（`ToolExecutor::execute` 的第三道闸，形态由 `run-command-permissions/02` 给）
  把该层的写工具调用转成提议。**提议 ≠ 动作**——库里存的是「打算做什么」。
- **端点**：`GET /foreman/proposals`（按会话取未决）、`POST /foreman/proposals/{id}/execute`、
  `POST /foreman/proposals/{id}/reject`。落在 `/foreman/` 前缀下，自动继承配对护（决策 182⑦）。
- **执行 = 走后端既有的那条路**（同一套校验、同一套闸门：依赖循环、worktree 准入、写入门、fail fast）。
  **不得**为值班长开一条绕过校验的捷径——否则「LLM 接进状态机」换个形式又回来了。
- **参数**（决策 207）：TTL **10 分钟**；**重启后保留**（未过期 + `status` 仍 `pending` + 态势未变即可
  执行，不另立「重启即作废」规则）；**过期只让按钮变灰、那一轮留在时间线**（审计价值）；
  **不做去重**（连着提两次就是两条，人各按一次）；过期清理接进既有每小时维护作业
  （`runtime.rs:31` 的 `MAINTENANCE_INTERVAL`）。
- **拒执语义**：态势已经变化（任务状态 / `allowed_actions` 变了）**拒绝执行**并报「现在的情况已经不是
  它当时说的那样」；**一次一按**、不可重放。
- **新事件**：提议到达 / 作废用一个**只读的新 SSE 事件**，不混进 `ConversationDelta`（混进去会让前端的
  时间线归约变复杂）。

**Blocked by:** 01、`foreman-sessions` 01（`session_id`）

**Status:** ready-for-agent

- [ ] 表 + 四个状态 + 过期清理接进维护作业
- [ ] 三个端点，均在配对护下
- [ ] 执行走既有端点那条路（测试：提议里的参数过不了校验时执行失败，且 `status` **不**变成
      `executed`）
- [ ] TTL 到期后 execute 被拒并标 `expired`；**那一轮仍在时间线**（前端断言在票 03）
- [ ] 态势变化的拒执有测试
- [ ] 一次一按：同一提议执行两次只生效一次
- [ ] **本票不含任何新工具**（写工具在票 04 / 05 / 06）——本票交付后值班长的工具集**没变**
