# 02: 人不被打扰——调度器两处豁免与托管收手

**What to build:** 手动暂停落成 pending 之后，**没有别的自动行为该去碰它**：调度器不该把
「人自己按住的任务」报成停滞 / 落进待办表，托管的自动集也不该替人松开。裁决落
`docs/decisions.md` 276 的 ⑥。

**Blocked by:** 01（暂停得先落成 `pending(user_paused)`）

**Status:** done（2026-09-25）

**要点：**

- 判据落在**游标**上（决策 82 的同一姿态）：任务状态只是投影，一个分支被人按住、另一分支
  在跑的任务，不该因为这条投影被当成「等人管」。
- 两档谓词各有用途：`is_human_hold`（这一条是）与 `all_pending_are_human_holds`（这个任务
  全部的 pending 都是）——提醒与待办要的是后者。

- [x] `pipeline/cursor.rs`：两个谓词
- [x] `scheduler/mod.rs`：`remind_pending_tasks` 跳过（不挂 `stalled`、不发提醒、不落
      `TaskStale`）；`note_discoveries` 的 ① 整块跳过（含 ② 的重复计数）
- [x] `agent/tools.rs`：`steward_grant` 第四条闸——**人按下的暂停，机器不许替他松开**
      （照常生成提议、按键仍是人的）
- [x] 用例：§6 `tests/integration/pause.rs::a_held_task_is_not_reported_as_stale`、
      §6 `tests/foreman.rs::a_held_task_is_never_auto_released_by_stewardship`
