# 对讲台多会话（foreman-sessions）

**状态（2026-09-17）：三张票全部落地，代码已动。** 迁移 0012 与四个端点、班次 chip 行、
增量的会话身份与两处守卫都已实现并有测试（`crates/core/tests/foreman_sessions_migration.rs`、
`crates/core/tests/foreman.rs` 的多会话段、`frontend/e2e/talk.spec.ts` 的 ⑭⑮）。


裁决（决策 204）：一条长会话改成可管理的一排「班次」。四件事：**新建 / 切换 / 重命名 / 归档**——
不做物理删除（沿用 `archived_at` 只归档不删的语义）、**不做分叉**（不从某一轮另起一段并带上前文）。

| 票 | 内容 | 依赖 |
|---|---|---|
| [01](issues/01-sessions-data.md) | 会话数据与迁移 0012（含命令日志的会话归属）+ 存储层 + 端点 | — |
| [02](issues/02-talk-sessions-ui.md) | 对讲台的班次 chip 行 + 新建 / 重命名 / 归档 | 01 |
| [03](issues/03-sse-session-scope.md) | 事件流带会话身份 + 切换守卫 | 01、02 |

**这条反转了两处现状**：`crates/app/src/routes/foreman.rs:28-33` 的 out-of-scope 声明（原话
「一条长会话就是整晚的值班台账（out of scope 里明确不做多会话 / 会话列表）」）与决策 182④ 的
「一条长会话即整晚台账」读法。台账从此跨会话，**会话只是对话的容器**。

**隔离的是什么，别搞混**：会话隔离**上下文**（喂给模型的 transcript 与页面的 token 合计按会话过滤），
**不隔离权限**——`read_task` / `read_conversation` 照旧全局可查，态势快照（`build_briefing`）照旧全局、
不随会话变。「换会话 ≠ 换看板」。

**先做的理由**：`foreman-capabilities` 的提议要挂在会话上（提议表的 `session_id` 来自这里），
而本次迁移同时给命令日志加了会话归属列——值班长的命令没有 task_id 可挂（见票 01 的说明）。
