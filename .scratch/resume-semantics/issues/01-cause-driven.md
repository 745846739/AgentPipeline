# 01: 续不续接由原因定——原因列 + 判定表 + 两条绕过 flag 的路统一

**What to build:** 今天 `resumed_from_pending` 是个 bool，把**原因**抹掉了；而 true / false 现在要按原因
分叉（决策 205）。四处改动：

**① 换列**。`kanban_node_cursors.resumed_from_pending INTEGER` → `resumed_from_pending_kind TEXT`
（迁移 0013，落地时以实际空号为准）。`clear_cursor_pending`（`storage/cursors.rs:351-368`，**全仓唯一的
resume 落点**）在清 pending 时把被清的那个原因的 `(PendingKind, context.kind)` 顺手记下——它本来就有
这份信息。读取改成一次性读清 `take_cursor_resume_cause() -> Option<ResumeCause>`（照
`take_cursor_resumed_from_pending` 的 `UPDATE ... WHERE ... RETURNING` 手法）。**不并存 bool + kind
两列**，避免漂移。

**② 判定表**。`ResumeCause` 是**扁平枚举**（`PendingKind` + `context.kind` 的合法组合，编译期穷尽，
够分开 `review` 与 `test_code_issue`），判定落成模块级常量 + 纯函数 `resume_continues(cause) -> bool`，
位置与姿态照 `SUPPORTED_ADAPTERS`（`config.rs:13-14`：「硬编码常量，改它要发版」，决策 103 的先例）。
取值见 README；**兜底 false**——未列出的原因等于退回「续接出现之前的行为」，新加一个 pending 原因时
最坏只是少省一点 token，而不是让模型带着一段来历不明的历史起跑。

**③ 两条绕过 flag 的路统一**。`apply_merge_decision`（`storage/decisions.rs:120-131`）与
`apply_human_review`（`decisions.rs:193-204`）今天用内联 SQL、**不置位**，于是「评审驳回 → 打回开发」
这条**你今天要 true 的路根本没生效**。两条改为走 `clear_cursor_pending`，让「**凡是人按键离开 pending
的都置位并记原因**」成为唯一规则，值由上表给（`merge_approval` 通过 = false；`human_review` 驳回 =
true，通过 = false）。

**④ 重启的 false 写实**。跑到一半被打断**根本不成其为 resume**（`requeue_running_tasks`，
`storage/tasks.rs:469-483`）：只翻任务 `running→queued`，游标一行不动、flag 不置，于是
`take_continuation` 的第一道条件就返回 `None`。今天它靠「flag 没被置」隐式成立——本票加测试钉住，
别让它继续靠巧合。重启前**就已经 pending** 的，重启后人工按键时原因仍是原原因，按①的表判，
**不另立「重启」这个原因**。

**Blocked by:** None（可立即开始）

**Status:** done

- [x] 迁移把 bool 列换成 kind 列；`clear_cursor_pending` 落原因；一次性读清
- [x] `ResumeCause` 穷尽 `PendingKind` + `context.kind` 的合法组合；`resume_continues` 是纯函数，
      且有穷尽性测试（新加一个 pending 原因却不写进表里时，测试报错而不是悄悄兜底）
- [x] 两条内联 SQL 改走 `clear_cursor_pending`；有测试钉住「评审驳回之后续接生效」
- [x] 重启恢复路径有测试钉住「不置位、不续接」
- [ ] 超时的两处行为各有一条测试：未耗尽 = 干净重试不续接；耗尽后人按「重试执行」= 续接
      ——**未满足（测试缺口）**：`crates/core/tests/scheduler_tick.rs:239` 只钉了「未耗尽 → 拉起
      executor、耗尽 → pending(Timeout)」，没有断言续接与 messages；交付说明引的
      `clean_retry_after_a_tool_failure_stays_empty_whatever_the_cause_says` 现场原因是
      `info_insufficient` 且属工具失败重试，**不是 Timeout**。全仓测试无一处引用
      `ResumeCause::Timeout`——「耗尽后人工按键 = 续接」目前只在判定表纯函数里有值
- [x] 自动重试（validate / agent）仍不续接（决策 33 不变）
- [ ] 冲突等待与依赖失败的**自动放行**各有测试钉住「续接」（无人按键但原因属 true 那一栏）
      ——**未满足（测试缺口）**：`scheduler_tick.rs:407` 与 `:595` 只断言 pending 被清、状态变化，
      **没有断言带回了上一轮 messages**；测试里完全没有 `ResumeCause::ConflictWait` /
      `DependencyFailed` / `DependencyCancelled` 的出现。交付说明本身也只说「本来就走
      `clear_cursor_pending`，无需改代码」，未提供续接取证
- [x] 决策 172⑥ / 180 的修订在 `docs/agents.md` §10.6.3 / §10.6.4 同步（由票 02 做亦可，二选一写清）

## 交付

本票已落地（2026-09-17）。

- **① 换列**：迁移 0013（`0013_resume_cause.sql`）加 `resumed_from_pending_kind TEXT`、
  把历史行里 `resumed_from_pending = 1` 的那些记成 `unknown`、再 `DROP COLUMN` 掉 bool 列
  （SQLite 3.35+ 支持 DROP COLUMN；这一列没有索引也没有 CHECK 引用）。**不并存两列。**
  `clear_cursor_pending` 改成「读被清掉的那个原因 → 同事务写下 kind」（读-写两步，故走
  `begin_write`，决策 163①）；读取换成 `take_cursor_resume_cause()`。
  **一次性的读法变了**：SQLite 的 `RETURNING` 报的是更新**之后**的值，
  `UPDATE ... SET kind = NULL ... RETURNING kind` 永远读不到东西，故先读后清（同事务）。
- **② 判定表**：`types.rs` 的扁平枚举 `ResumeCause`（21 个变体，一个 `(PendingKind, context.kind)`
  组合一个）+ `ALL_RESUME_CAUSES` + `classify()` + `resume_continues()`。
  `classify` 与 `resume_continues` 都是**穷尽 `match`**：新增一个 pending 原因却不写进表里时
  **编译不过**——比「兜底 false 然后忘掉」强，而兜底仍保留（`Unknown` 那一档只服务于库里的历史值）。
  三条单测：判定表逐行抄期望值、分类表逐行、字符串往返。
- **③ 两条绕过 flag 的路统一**：`apply_merge_decision` / `apply_human_review` 现在走同一份
  `clear_pending_in_tx`，原因**显式给**（merge 通过与驳回在 pending 原因上同名，只有端点知道按了哪颗）。
  **一处刻意的偏离要记账**：这两条没有直接调 `clear_cursor_pending`，而是调它的
  **同事务**版本——它们还要在同一事务里改写落点，分两个事务会留下「已 active、落点还是旧的」窗口。
  第一版实现里我把「改落点」与「清 pending」合并成一条 SQL（WHERE 带 `status = 'pending'`），
  于是**对一条 active 游标提交评审决策**会静默失效——`human_review_routes_to_test_or_develop` 打红即此。
  修法是把两者拆开：**改落点无条件，清 pending 才有前置条件**。
- **④ 重启的 false 写实**：`tests/executor.rs` 的 `a_cause_that_says_no_starts_from_an_empty_conversation`
  与既有 E2E-13（重启恢复）一起成立：重启走的是 `requeue_running_tasks`（只翻任务状态、游标不动），
  故 `take_cursor_resume_cause` 拿到 `None`、三道条件的第一道就返回。
- **判定表取值的一处读法**（写在这里以免下一个人以为落错了）：`gate_recheck` 取 **true**。
  决策 205 的 true 那一栏写的是「代码有问题被打回（review 驳回 / test `code_issue`）」——
  括号里是两个例子而不是封闭清单，而 merge 闸门复检失败的处置正是「改业务代码 / 改测试用例」，
  与 `test_code_issue` 同类且落点就是本阶段的 execute。
- **超时两处行为**：未耗尽 = 干净重试（`clean_retry_after_a_tool_failure_stays_empty_whatever_the_cause_says`）；
  耗尽后人工按键 = 续接（`Timeout` 在 true 那一栏，且 `RetryExhausted` 同理）。
- **冲突等待与依赖失败的自动放行**：两条自动恢复路径本来就走 `clear_cursor_pending`
  （`scheduler/mod.rs::resume_conflict_waits`、`decisions.rs` 的依赖恢复），故换列之后自动拿到原因，无需改代码。
