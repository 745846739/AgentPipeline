# 手动暂停 / 续跑 / 重跑本阶段（决策 276）

> 起源：2026-09-25 用户诉求「给非 pending 状态的看板任务提供手动暂停、续跑、重跑功能；
> 给值班长提供相应能力并补充到 operate-pipeline 这个 skill 中」。裁决落
> `docs/decisions.md` **276**（七条裁决 + 明说的四件「不做」），本 effort 只做落地。

| 票 | 状态 | 关键接缝 |
|---|---|---|
| [01 三颗手动钮：core 状态机 + 两个端点](issues/01-hold-actions.md) | done（决策 276） | `pipeline/pause.rs`、`actions.rs` 的 `user_paused` 行、`Landing::Rerun`、`CancelOrigin`、`RunLedger::finish_cancelled`、`POST /tasks/{id}/pause|rerun` |
| [02 人不被打扰：调度器两处豁免 + 托管收手](issues/02-unwatched-hold.md) | done（决策 276） | `cursor::is_human_hold` / `all_pending_are_human_holds`、`remind_pending_tasks`、`note_discoveries`、`steward_grant` 第四条闸 |
| [03 界面两颗钮（在跑的任务）](issues/03-hold-buttons.md) | done（决策 276） | `TaskDetail.svelte` 的 `canHold` 分支、`api/client.ts` 的 `pauseTask` / `rerunTask`、`pendingLabel` 的「已暂停」 |
| [04 值班长与操作手册](issues/04-foreman-and-skill.md) | done（决策 276） | `task` 工具的 `pause` / `rerun` 两个动作（直接调端点 handler）、`operate-pipeline` 手册九个动作 + 新增一节 |

**为什么四张票而不是一张**：它们各自的**判据来源不同**——01 是状态机与执行体收口（Rust
L2/L3 用例），02 是调度器与托管的自动行为（L2 用例），03 是界面（vitest + 行为映射表），
04 是模型面对的工具面与手册（契约用例 + skill 正文）。合在一起会让「红了是哪一类」需要
读完整份改动。

**与既有 effort 的关系**：本 effort 不重开任何既有票，但**显式修订三处**——
决策 210② 的托管自动集（`user_paused` 上收手）、决策 226 的中止通道（加来路与「按住后
不许推进」一条护栏）、决策 205 的续接判定表（加 `user_paused` = true / `user_rerun` = false）。
三处都在决策 276 的行内点名。
