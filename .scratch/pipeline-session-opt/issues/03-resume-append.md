# 03: 补充输入改为对话末尾 user turn（决策 279，限定 79）

**What to build:** 用户在「信息不足」补充输入框里说的话，在续接的会话里以一条真实的 user 消息出现在**对话末尾**——归档转录里能看到用户说了什么（现状：这句话只活在重渲染的开场白里，转录中查无此人）；续接请求的开场白与上一轮逐字一致，prompt 缓存不再整段打穿（实测 run40：25,800 prompt 只命中 126）。

**Blocked by:** None（can start immediately）

**Status:** done（2026-09-25）

- [x] info_insufficient 续接：补充输入作为 user turn 追加到携带转录的末尾（续接装配之后、round 0 seeding 之前）（决策 279）
- [x] validate_input 的续接场景停用 architect_reentry_segment（不再把补充输入拼进首条 user 消息）；execute 节点（节点间无转录可续）仍走 segment（决策 79「看板自由输入仅限 info_insufficient 补充说明」照旧）
- [x] user-input.md 照旧落盘留痕（决策 79 / 138 的落盘纪律不动）
- [x] 单测：续接请求首条消息与上一轮逐字一致 + user turn 在末尾；app L3 契约
- [x] 四门全绿

## Comments

- 来源：同一会话复盘——resume 走「user-input.md → segment 重渲染首条消息」导致前缀全变、缓存全失（126/25,800），且转录保真破损（run40 的 43 条消息与 run39 逐字相同，用户发言不可见）。

- **评审修复（2026-09-25）**：决策 79 的落盘纪律「空输入不落文件」但不清旧档——同一 info_insufficient 第二次「继续」且不带新输入时，旧补充已在转录里，直接追加会重放。已改为：转录里已有同文 user turn 就不再追加（回归用例 `a_second_resume_without_new_input_does_not_replay_the_old_supplement`）。
