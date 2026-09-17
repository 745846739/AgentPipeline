# 04: 缺陷——`context_overflow` 的三条出路里有一条不解除 pending，按了没反应

**What to build:** `context_overflow`（`pipeline/executor.rs:2038-2063`，另有早退那处 `1261-1283`）
给出三个动作：`split_task` / `model_override` / `cancel`（`actions.rs:266-271`）。其中
`POST /tasks/{id}/model-override`（`routes/tasks.rs:553-582`）**只写** `kanban_tasks.model_override`、
**不清游标 pending**——于是人按下「更换长上下文模型」，界面上什么都没变，任务仍卡在同一个 pending 上。
`split` 终止原任务、`cancel` 取消，那两条说得通；只有这一条按了没反应，是**给了出口但门没开**。

修法二选一（落地时定，写进交付说明）：

- **首选**：`model_override` 之后按「重试执行」的语义清 pending 并让任务回队列（等价于替人按一次
  resume）。模型换取一次是明确的意图，多一次点击没有信息增益。
- 备选：把这个动作从 `context_overflow` 的 `allowed_actions` 里摘掉，只留「先改模型，再按重试执行」
  的两步走。

若选首选，**清 pending 走 `clear_cursor_pending`**，因此会带上原因——`context_overflow` 在决策 205
的判定表里取什么值，由 `.scratch/resume-semantics/issues/01-cause-driven.md` 一并定（建议该原因给
**false**：上下文溢出的人为处置之后重开一段更干净）。

**Blocked by:** None（可立即开始）

**Status:** done

- [ ] 按下去之后 pending 真的解除：`GET /tasks/{id}` 的 `allowed_actions` 随之变化（测试钉住）
      ——**未满足（测试缺口）**：行为已实现（`crates/core/src/storage/tasks.rs:237-276` 的
      `apply_model_override` 清 pending、任务回 `queued`），但取证停在 Store 层
      （`crates/core/tests/executor.rs:3094` 断言 `cleared == 1`、游标不再 pending、任务 `Queued`）；
      契约侧的 `api_contract.rs:927` 只查 `model_override` 字段，**全仓无任何用例断言
      `GET /tasks/{id}` 的 `allowed_actions` 在覆盖后变化**
- [x] 若选首选：走 `clear_cursor_pending`，原因与判定表的取值一起定
- [x] 补测试：`context_overflow` 下按 `model_override` 之后游标不再 pending、任务回到可被调度器
      准入的状态（`try_admit` 只认 `queued`）
- [x] 交付说明里写清选了哪条、以及为什么

## 交付

本票已落地（2026-09-17），选的是**首选**方案（按下就解除 pending）。

- 新方法 `Store::apply_model_override(task_id, provider_id) -> Result<usize>`：三件事**一个事务**
  ——写 `model_override` / 清 `context_overflow` 的 pending（走 `clear_pending_in_tx`，因此记原因）/
  任务回 `queued` 且清掉任务行的 `pending_reason_json`（不一起做的话界面会继续显示一条已经不存在的待办）。
  分开做会留下「游标已 active、任务还是 pending」的中间态，而那种状态下没有任何东西会再推它一把。
- **只对 `context_overflow` 解除**：这个端点在任何时候都可被调用，而合入审批 / 人工评审等的是**另一种决定**，
  顺手替人清掉会让他永远等不到那个决定。返回值是解除掉的游标数，端点如实回报（`resumed` 字段）。
- 为什么选首选而不是备选（摘掉那颗按钮）：`context_overflow` 的动作集里就有「更换长上下文模型」
  （决策 105），人按下它表达的正是「换一次再试」——备选方案等于承认一颗按钮没有用，
  而它本来可以有用。
- **原因取值：`false`**（决策 205 未列 → 兜底 false；票 01 的表里也是 false）。
  于是溢出处置之后重开一段更干净，正好对得上「换个窗口再跑」的语义。
- 测试：`a_cause_that_says_no_starts_from_an_empty_conversation`（`tests/executor.rs`）——
  按下之后游标不再 pending、任务回 `queued`、重入那一轮起点为空；
  契约侧 `api_contract.rs` 的 model-override 用例照旧绿。
