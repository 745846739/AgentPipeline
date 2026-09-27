# 01: 任务对话（流水线 run 会话）的在途可见性——同病，等 talk-replay 用出真需求再动

**What to build:** 流水线 run 的会话（任务详情里的对话，`kanban_node_conversations` /
attempt 回执）与对讲台同病：**attempt 结束才把回话整段写进 `messages_json`**，于是
跑一轮的中途刷新 / 切页之后，前半段同样看不见、进程被杀同样整段消失。把
`.scratch/talk-replay/spec.md` 的四条改动（在途边流边写 / 拼接 / 中断台账真相 /
历史边界）移植到这条通道上，**等真实使用先证明这病在这条通道也疼**（对讲台那侧是
用户报障报出来的；任务对话至今没人报——同病不等于同紧迫）。

**与 talk-replay 的两处不同**（移植时的手术点，spec Out of Scope 已记）：

1. **通道不同**：对讲台是消息逐行追加（`kanban_foreman_messages`，一行一 id，
   `before_id` 游标天然长在主键上）；任务对话是 **`messages_json` 整行重写**
   （一条会话一份 JSON）——「边流边写」在这条通道是**整段 JSON 反复重写**，
   节流与写放大要重新算账，不能照抄。
2. **表结构不同**：没有逐行的 `status` 列可挂 `in_flight` / `interrupted`，
   中断状态得另找落点（行内标记？外列？）——那是一次新裁决，不是搬运。

**先决**（不阻塞本票的存在，阻塞它的动工）：

- talk-replay 票 01–07 全部收口、跑过一段真实使用，确认四条改动在对讲台的
  形态是对的（照着一个还没验证过的形态移植，是把未定价的风险复制一份）；
- 用证据确认任务对话确实有「中途看不见」的痛点（issue / 实测），没有就继续躺着。

**明确不做**（继承 talk-replay 的口径，不重开）：SSE 回放、localStorage 在途回填
（决策 275）、中断轮续跑（已中断即终态）、检索。

**Blocked by:** talk-replay 01–07 收口后的真实使用反馈

**Status:** needs-triage

- [ ] 先决：talk-replay 落地后确认痛点在这条通道真实存在（拿证据来，不拿「同病」来）
- [ ] 裁决 `messages_json` 通道的「边流边写」形态（整段重写的节流与写放大账）
- [ ] 裁决中断状态在无逐行 `status` 时的落点
- [ ] 出 spec（本文件只是票面登记，设计定形走 `.scratch/<slug>/spec.md` 全流程）

**来源：** talk-replay spec Out of Scope「任务对话（流水线 run 会话）的在途可见性——
同病（attempt 结束才落库），但通道与表结构不同（messages_json 整行重写 vs 消息
逐行追加），记为后续票、不进本轮」，由 talk-replay 票 07 挂票面。
