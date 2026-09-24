# 07: 在途轮（决策 260）已随并行会话落地——剩悬空 doc 引用与两处计数器

**起因（2026-09-24 上午，票 06 收口时发现）**：`GET /foreman/session` 应答带了第五个位置
`turn_in_flight`，但立票那一刻前端零消费者、决策日志无对应条目——代码引用的决策号读不到
来历。立票后数小时内并行会话落地了全部主体并补上**决策 260**（用户报的毛病：「值班长回答
问题时刷新页面就看不到实时对话流」），故本票由 open 改 partial，收窄为两条收口。

**已落的（勿重做，全部由决策 260 记账）**：

- core：`FOREMAN_TURNS` **计数**登记（同一班可同时跑两轮，按格覆盖会让先退出的那轮摘掉
  另一轮）+ `begin_foreman_turn` 落在 `say` / `watch` 共用的唯一漏斗上 +
  `foreman_turn_in_flight()` 读数；`crates/core/tests/integration/foreman.rs` 集成用例。
- app：`session_payload` 第五个位置；`api_contract.rs::the_session_payload_says_whether_a_
  turn_is_running`（L3 断言——本票立票时的第 3 条余量，已由并行会话补上）。
- 前端：`maxLedgerId` / `turnLanded` 两枚纯判定 + `Talk.svelte` 的 `followingSince` 状态机
  （接手 / 增量闸门放宽 / 落地轮询）+ `buildTurns` 的 `following`；各带单测。
- 记账：**决策 260** 全行 + `glossary.md`「在途轮」词条 + `testing.md` 两行 +
  `frontend-design.md` §12.3 的 :805 行（立票时的第 2、4 条余量，已由并行会话补上）。
- e2e：`frontend/e2e/talk.spec.ts`「回话中刷新页面」（behavior-map 行声称；随 `make check-e2e` 验证）。

**顺手修的两处闸门竞态（本票收口期间，均在 `crates/app/tests/integration/api_contract.rs`）**：

1. `the_session_payload_says_whether_a_turn_is_running`：单次断言改成轮询（与同测试里
   落地那次对称）——用户行先落库、「在跑」的登记在随后起来的那一轮里，全量并跑时单次
   断言会输掉窗口（单独跑 3/3 绿、`make check-test` 整跑红过一次）。
2. `foreman_sessions_can_be_created_renamed_and_archived`（**HEAD 上的潜伏竞态**，非 260 引入）：
   两次新建之间把 `ManualClock` 拨 60 秒——钟是冻住的，不拨则两行 `last_active_at` 相等，
   排序落进 `id DESC` 的并列兜底，而 ULID **同毫秒随机尾缀**决定先后（实测全套整跑红过一次）。

**还缺的（本票余量，两条都是记账面——2026-09-24 已由本票收口时一并补上）**：

1. ~~**悬空 doc 引用**：`api/types.ts:999` 仍写「判据见 `realtime/foreman.ts::followForemanTurn`」~~
   → 已改指真判据（`maxLedgerId` / `turnLanded` + `Talk.svelte` 的 `followingSince`）。
2. ~~**两处计数器**：`AGENTS.md:5` 与 `docs/README.md:18` 仍写 `#1–259`~~ → 已改 `#1–260`。

**Blocked by:** None

**Status:** partial（2026-09-24：主体与记账已落成决策 260，两条余量与两处测试竞态已补；
**code-review 又查出一条决策 260 内部矛盾，见文末新增余量**）

- [x] core 登记 / 计数摘除集成用例
- [x] 前端判据 + 单测（`maxLedgerId` / `turnLanded` / `following`）
- [x] `Talk.svelte` 装载接线（`followingSince` 状态机 + 落地轮询 + SSE 守卫）
- [x] L3 契约断言（`turn_in_flight` 随一轮寿命翻转）
- [x] 决策记账：决策 260 全行 + 词条 + testing 行 + 行为映射行
- [x] 两处测试竞态收口（见上「顺手修的两处」）
- [x] `types.ts:999` 的悬空 `followForemanTurn` 引用改指真判据
- [x] `AGENTS.md` / `docs/README.md` 计数 → #1–260
- [ ] **裁决③的「留白」没落地**（2026-09-24 code-review 查出）：决策 260 裁决③与
  `turnLanded` 的 doc 都写「不换行而服务端也不再报『在跑』（进程被杀）时**保留已经收到的
  半截字比清空诚实**」，而 `Talk.svelte` 落地哨是 `landed = turnLanded(...) ||
  !payload.turn_in_flight`——恰恰那一类靠后半句为真进分支，随后
  `stream = emptyForemanStream()` **把半截字清掉了**，与裁决相反。修法要先裁决呈现方式
  （半截字留在时间线上以何种身份出现——`failForemanStream` 的失败轮姿态是现成候选），
  不只是把 `|| !turn_in_flight` 摘掉（摘掉会让死掉的那一轮永远跟下去）。

**来源：** 票 06 收口跑 `npm run check` 时发现（fixture 缺 `turn_in_flight` 字段而红——
vitest 不做类型检查，实现方当时的「全绿」没照到这一处）。按「交付时发现的缺口另立票」先例
（票 06 之于票 04）立此票；主体落地后收窄为两条记账余量。
