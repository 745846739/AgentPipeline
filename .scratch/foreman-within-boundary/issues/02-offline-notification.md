# 02: 离线通知渠道——叫不醒人的值守轮等于没有

**What to build:**（先裁决、后动手——渠道与触发面见裁决问）值守轮的终点是叫醒人，
但今天的出口只有 SSE toast：**夜里没人盯浏览器，attention 落库就石沉大海**。
自 `backlog-v2.md §B.3` 提前（决策 262①：「值守轮的终点是叫醒人，叫不醒的值守轮
等于没有」），`NotificationPolicy` 的 cooldown/quiet_hours **结构**已在前端留好。

**接缝事实（探查取证，2026-09-24）**：
- `NotificationPolicy` 今天是**前端独有**（`frontend/src/lib/notificationPolicy.ts`）——
  决策 130③ 明言「cooldown / quiet_hours 只作用于前端 toast」，后端零实现、零 webhook 桩；
- attention 是唯一「叫人」信号面：`AttentionKind` 12 变体、`wakes()` 11 个（仅 `SlowRun` 不醒），
  生产点 = scheduler 十处 + `cancel_task`（决策 234）——**提议没有 attention 变体**（提议在
  `kanban_foreman_proposals` 自己的表）；
- 值守轮已有一套「礼貌机制」：debounce → 每任务 cooldown → 每小时上限
  （`foreman.rs:1526-1581`），但它的出口仍是浏览器 SSE，且**依赖 LLM 轮成功**才落消息；
- 配置面：`Config` struct `deny_unknown_fields`（未知键拒），加 `[notify]` 段 = 新 struct +
  字段；`reqwest` 已在 core（`web_fetch` 同批引入，出站零新依赖）。

**分支草案（拷问输入，不是结论）**：

- **渠道**：(a) 通用 webhook（POST JSON 到配置 URL——飞书/企业微信/ntfy 机器人皆可转发，
  零新依赖）；(b) SMTP 邮件（要凭据与新依赖）；(c) 绑死一家的专用格式。
- **触发面**：(a) attention 落库且 `wakes()`——与值守轮同一信号面，产出点直发、
  LLM 挂了也发得出（代价：cooldown/quiet_hours 要移端重做一遍语义）；
  (b) 挂在值守轮成功播报之后——白蹭既有礼貌机制，但 LLM 连挂/被每小时上限掐掉时就哑；
  (c) (a) 再加「新提议落库」（等按键也是叫人）。

**边界草案（拷问输入）**：通知是 **best-effort**——发失败只记日志，绝不影响流水线与
值守轮本体；webhook URL 是含 token 的秘密 → 只进 `config.toml`（`data/` 密钥面同款
存放纪律），**不进台账、不进日志明文**；cooldown/quiet_hours 若移端，语义必须与前端
`notificationPolicy.ts` 同一张表（或显式写清差异，别两套开关说两件事——179 同姿态）。

**Blocked by:** None（裁决落 `docs/decisions.md` 后实现）

**Status:** done（2026-09-24）

- [x] 渠道裁决取 **通用 webhook**、触发面取 **attention 落库且 `wakes()`**（四问，决策 268）
- [x] 实现 + 用例落齐（决策 268）：`[notify]` 配置段（缺省关死）、`note_attention` 单一漏斗触发、
      礼貌语义镜像 `notificationPolicy.ts` 同一张表（`tests/fixtures/notification_policy.json` 双端 22 行 fixture）、
      payload 六字段通用 JSON（body 只带归因白名单字段、detail 原文不出网——268「不发正文/日志原文」）、投递 best-effort（日志 `without_url()`）；
      断言：Rust 表测试 + 前端 `notificationPolicyFixture.test.ts` + L2 `tests/integration/notify.rs` **6 条**
      + `config.rs` 3 条；backlog §B.3 随本票结掉
