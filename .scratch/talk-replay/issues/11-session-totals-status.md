# 11: 页头合计加 `status IS NULL`——恢复「口径不动」的旧读数

**What to build:** talk-replay 票 08③ 的收口。`foreman_session_totals`
（`crates/core/src/storage/foreman.rs:788-799`）按 `role = assistant` 求和、**无 `status` 过滤**，
而在途行也是 assistant 且 `status = 'in_flight'`（`:564-585`）——于是页头「本次会话 N tok」
在一轮进行中途跟着涨、失败轮丢弃后再回落，与 spec / 决策 204⑤「口径不动」相悖。

**改法：** 求和与 `total_calls` 都加 `status IS NULL`（已收口正常行的 `status` 为 NULL，
常量注释 `:54-61`、收口 SQL `:629`），恢复旧口径。

**事实订正（避免沿用错前提）：** 对讲台页头只渲染 `total_tokens`
（`frontend/src/routes/Talk.svelte:1314`），「N 次调用」**不在对讲台**——全前端唯一渲染
`total_calls` 的是任务卡 `frontend/src/components/board/TaskCard.svelte:139`。若 `total_calls`
也要同口径，一并改；否则只改求和。

**Status:** done（已实现，决策 363③，2026-10-01）

- [x] SQL 加 `status IS NULL`（`total_tokens` 与 `total_calls` 两处，一条 WHERE 同时罩住）
- [x] 单测：`foreman::session_totals_ignore_in_flight_rows`——在途行不计入（中途刷写的读数
      一分不动）、收口写回同一行后计入（权威值）、没跑起来的那轮丢弃后不残留
- [x] 「实时在涨」不混进合计（决策 363③）：本轮没有另做在飞指示，页头合计恢复「只算已收口的正常行」
