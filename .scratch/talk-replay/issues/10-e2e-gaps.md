# 10: 两条 e2e 缺口——在飞时切走再切回、重启后中断轮带标记出现在时间线

**What to build:** talk-replay 票 08② 的收口。补两条 L4 用例：

1. **在飞时切走再切回**：一轮跑到一半切到看板再切回，按 `ledger_id` / `seq` 重新拼接，
   接缝不重不漏——写进 `frontend/e2e/talk.spec.ts`。这是票 02 的 checklist 唯一未勾项
   （`issues/02-snapshot-splice.md:15`）。
   **注意既有用例不是它**：`talk.spec.ts:2249` 那条「切走再回来（决策 275）」测的是
   **本机跨页不丢**，不是在飞时按 `seq0` 重拼。
2. **重启后中断轮带标记出现在时间线**：真重启后，中断轮以带标记的形态出现在时间线
   （Testing Decisions 的 L4 格，`.scratch/talk-replay/spec.md:149`）。
   该格目前靠三点覆盖、**端到端那一格空着**：L2 的 `restart_recovery.rs`（真 SIGKILL，
   `:454` `kill_9_mid_foreman_turn_marks_the_hanging_row_interrupted_on_restart`）+
   `frontend/src/lib/delegation-scan.test.ts`（中断标记单一来源）+ `frontend/src/realtime/foreman.test.ts`
   （中断行落 settled 支）。

**Status:** done（已实现，决策 363②，2026-10-01；用例二按票面预案记为**已接受的 L4 缺口**）

- [x] 用例一：在飞时切走再切回（`talk.spec.ts`，`对讲台 · 在飞时切走再切回：接缝两侧不重不漏（票 10）`）
      ——**必补已补**：切走时 A 已到、B 在页面之外到达，切回来接缝两侧**各恰好一次**、顺序保真、
      收口后仍只有这一轮（与决策 275 那条的区别写在用例头的注释里：那条只证字还在屏上，
      分不开「store 留着」与「快照重取」两个来源）
- [x] 用例二：重启后中断轮带标记出现在时间线 —— **记为已接受的 L4 缺口**（见下条）
- [x] 真重启在 playwright 里不可行：`harness.ts::startApp` 每用例独占一套临时 home + 子进程，
      **没有「同一 home 上的真重启」形态**（`stop()` 还会回收临时目录），为它改整套 harness 不划算。
      该格现由 **L2/L3** 承担：`crates/app/tests/integration/restart_recovery.rs::
      kill_9_mid_foreman_turn_marks_the_hanging_row_interrupted_on_restart`（真二进制 + 真 SIGKILL +
      同 home 重启，经 HTTP 观测）；渲染那半边由 `lib/delegation-scan.test.ts`（中断标记单一来源）
      + `realtime/foreman.test.ts`（中断行落 settled 支、在飞轮退场）两点覆盖。缺口与理由已写进
      `.scratch/talk-replay/spec.md` 的 L4 格与 `docs/testing.md` ㉙
