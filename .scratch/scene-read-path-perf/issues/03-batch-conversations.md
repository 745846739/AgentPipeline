# 03: 会话批量读法（`include_messages`）+ 现场页签渐进填充

**What to build:** 现场页签现在是 **N+1**：`loadAllConversations`（`frontend/src/stores/
taskDetail.svelte.ts:241-253`）对每个未缓存的 run 各发一次 `GET /tasks/{id}/conversations/{run_id}`，
每次返回整段 `messages_json`；服务端没开 h2，浏览器 HTTP/1.1 单源约 6 并发，N 个请求要排
若干波。本机库最坏 48 轮 / 2.1 MB，106 现场那个任务 23 轮。

① **加批量读法**：给 `GET /tasks/{id}/conversations` 加**加性参数** `?include_messages=true`，
命中时一次返回该任务全部轮的完整会话（含 `messages_json`）。加性口径与决策 312 给 messages
定的「500 缺省 + `before_id` 向上游标」不冲突——**不改会话列表的既有分页立场**。

② **顺带修一处既有浪费**：`include_messages=false`（默认，摘要态）时，`list_conversations`
现在仍 SELECT 全文列（`messages_json` / `system_prompt` / `user_prompt` / `reasoning`，
`crates/core/src/storage/observability.rs:1006-1013`）并对**每一行**做 `serde_json::from_str`
（`:1187`），而路由层随即丢弃它们只留摘要（`crates/app/src/routes/tasks.rs:854-871`）。
给摘要态换一条**只取摘要列**的投影（`id, run_id, stage, node, attempt, agent_type,
parent_run_id, prompt_tokens, completion_tokens, created_at, archived_at`）。

③ **渐进填充**：首屏先出轮名牌（摘要已经很便宜、先到），正文到位再填进各轮
（`SceneTurn.loaded` / `SceneTimeline.svelte` 的「正在读取会话…」占位已有，`:207`）。
让「有数据」的等待从「全部会话到位」降到「摘要到位」。

**Blocked by:** None

**Status:** done（2026-10-01，决策 361）

## 落点

- `crates/app/src/routes/tasks.rs`：`ConversationListQuery` 加 `include_messages`
  （`#[serde(default)]`）；两个分支分别走摘要投影与全量读法。
- `crates/core/src/storage/observability.rs`：新增/改造「只取摘要列」的读法；保留既有
  `list_conversations` 供需要全量的调用方（`conversation_archive.rs` 等测试用它）。
- `frontend/src/api/client.ts`：`getConversations` 加参数。
- `frontend/src/stores/taskDetail.svelte.ts`：`loadAllConversations` 改为**一次**请求；
  缓存键仍按 `run_id`（`conversationsFull`）以免与单条读法打架；失败降级到逐条（保留旧路径
  作为兜底）。
- 前端类型 `ConversationSummary` 顺带补上**服务端已发但类型漏了的 `archived_at`**
  （`frontend/src/api/types.ts:211-222`）——小错位，改一行。

## 验收

- [ ] 进现场页签只发 **1 次**会话请求（不再是 N 次）；`make api` 契约测试覆盖新参数的两个分支
- [ ] 轮名牌先于正文出现（挂住批量会话响应时，名牌仍渲染——与票 04 的设施共用）
- [ ] 摘要态不再读全文列：L2 断言（`make integration TESTS=conversation_archive`）
      钉住摘要读法不做 JSON 解析、且不包括重列
- [ ] 批量响应与逐条读法**内容等价**（同一 run_id 两种读法给出相同会话）
- [ ] 单条读法 `GET /conversations/{run_id}` 仍在（深链、档案盒跳转继续用它）
- [ ] `npm test` / `npm run check` / `make api` 全绿

**明确不做**：通用分页 / 会话列表游标（决策 319 的边界；106 只有 23 轮，收益为零）；
把批量端点做成 POST（GET + 加性参数已足够，且能吃到浏览器与中间层的缓存语义）。

**来源：** `.scratch/scene-read-path-perf/spec.md` 决议 3；N+1 见
`frontend/src/stores/taskDetail.svelte.ts:241-253`；摘要态浪费见
`crates/core/src/storage/observability.rs:1006-1013`、`crates/app/src/routes/tasks.rs:854-871`。

## 落地

- `crates/core/src/storage/observability.rs`：新增 `list_conversation_summaries()` +
  `ConversationSummary` / `ConversationSummaryRow`（只 SELECT 摘要列；返回类型里没有大列，
  「不读全文」是编译期保证而不是约定）。
- `crates/app/src/routes/tasks.rs`：`ConversationListQuery` 加 `include_messages`（`#[serde(default)]`）；
  两个分支分别走摘要投影与**复用 `list_conversations`** 的全量读法（「两种读法内容等价」因此是
  同一份实现的结果，不是两条要各自维护的约定）。
- 前端：`getConversations` 加 `ConversationListParams`（重载给出两种返回类型）、
  `loadAllConversations` 改为**一次**请求（失败降级到逐条，老路径仍留着当兜底）、
  `ConversationSummary` 补上此前漏掉的 `archived_at`。
- 测试：`conversation_archive.rs::summary_read_never_parses_the_message_payload`
  （**把 `messages_json` 改成非法 JSON**：摘要读法照常、全文读法必须失败——行为级证据，
  比「grep SQL 里没有那几个列名」结实）、
  `api_contract.rs::conversations_include_messages_batches_full_payloads`（两个分支 + 与单条读法逐字段等价 +
  缺省摘要态不含大列）、`frontend/src/stores/taskDetail.test.ts` 三条、
  `first-paint-budget.spec.ts` 的「挂住批量会话，轮名牌先于正文出现」。
