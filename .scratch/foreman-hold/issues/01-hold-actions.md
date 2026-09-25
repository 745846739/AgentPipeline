# 01: 三颗手动钮的状态机与两个端点

**What to build:** 非 pending 的任务此前**一颗钮都没有**（`running` 只能看）：把「暂停 /
续跑 / 重跑本阶段」做成可执行的动作——暂停落 `pending(user_paused)`、续跑复用既有 resume
的 `continue`、重跑本阶段落回阶段入口。裁决落 `docs/decisions.md` 276 的 ①–⑤。

**Blocked by:** None

**Status:** done（2026-09-25）

**要点（实现即规格）：**

- **暂停不新造 `TaskStatus`**：pending 本来就是「停着等人」那一格。只对被准入的任务生效
  （`running` / `pending`），按住之后照旧占名额（决策 117）。
- **中止请求带来路**（`CancelOrigin::{Timeout, Hold}`）：人按停的请求一旦发出，执行体本轮
  结论一个字都不许写台账；判超时那条路行为不变。
- **中止路径自收口**：`RunLedger::finish_cancelled` 在没人判过终态时收成
  `NodeStatus::Cancelled`（判超时先写的那条退化成补用量）。
- **重跑 = `Landing::Rerun`**：与 `Entry` 同落点，只在离开 pending 时显式写 `user_rerun`。

- [x] `types.rs`：`PendingKind::UserPaused`、`ResumeCause::{UserPaused, UserRerun}`（判定表
      23 行）、`NodeStatus::Cancelled`；镜像契约两侧同步（`tests/fixtures/enum_members.json`
      + `frontend/src/api/types.ts`）
- [x] `actions.rs`：`user_paused` 那一行 = `continue` + `goto`（落点 `entry_node(stage)`）+ `cancel`
- [x] `pipeline/pause.rs`：`pause()` / `rerun()`（前提、先请求中止再落账、SSE、投影、重派）
- [x] `pipeline/executor.rs`：`request_hold`、`held_by_human` 护栏（`run_inner` 里认它）
- [x] `pipeline/run_ledger.rs`：`finish_cancelled`（`model_invoke` 的中止分支改为调它）
- [x] `pipeline/{advance,resume}.rs`：`Landing::Rerun`；`apply_action` 的 goto 在
      `user_paused` 上走它（同一行两颗出口键，只有按键的那一方知道按的是哪一颗）
- [x] `crates/app/src/routes/tasks.rs` + `lib.rs`：`POST /tasks/{id}/pause` 与 `/rerun`
- [x] 用例：§6 `tests/integration/pause.rs` 8 条、§5 `executor.rs` 单测 1 条、§7
      `api_contract.rs::pause_and_rerun_hold_a_running_task`

**实现者记事（2026-09-25）：**

- 票面草案里「重跑」只有端点一条入口，落地时发现**已经停住的任务**那颗 `goto` 必须也能重跑
  （它就在 `allowed_actions` 里，人按的就是它）——于是 `Landing::Rerun` 有两条调用者，端点
  那条反过来学会拒绝并指路（报文说「走 resume」而不是含糊地说「不能重跑」）。
- 端点的重跑前提一度写成「本阶段有 run 行」就够，漏了「游标得是在跑的那种」——挂着的游标
  不进 `active_cursors`，于是「停着的任务」会撞上一句「本阶段还没跑过」的错报。两条分开报。
