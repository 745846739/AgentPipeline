# 12: 两条并行在途行的呈现——按 `ledger_id` 分流、各自成轮

**What to build:** talk-replay 票 08④ 的收口。决策 260 允许同一班并行两轮（值守轮 + 人的轮），
而现判据会把另一条行的增量折进基准行：

- `inFlightBase`（`frontend/src/lib/talkTurns.ts:274-279`）取快照里**第一条**在途行当基准；
- `spliceAccepts`（`frontend/src/realtime/foreman.ts:89-96`）对 `ledger_id` 与基准行对不上的
  增量**放行** → 另一条在途行的事件被折进基准行那一轮渲染（`talkTurns.ts:412-417`），
  而那条行在自己的屏里已渲染过，**可能重字**。

**改法（决策 363④）：** 按 `ledger_id` **分流**——每条在途行各自一个基准、各自折进自己
那一轮（把 `inFlightBase` 从「第一条」改成按 `ledger_id` 的映射）；呈现规则：**两条都渲染，
各占一轮、按行的位置（id / seq）排在时间线里**；**无 `ledger_id` 的事件（流水线事件 /
老后端）归基准行**。

**Status:** done（已实现，决策 363④，2026-10-01）

- [x] `inFlightBase` 改成按 `ledger_id` 的映射（`talkTurns.ts::inFlightBases` 返回**一组**基准，
      逐行拼接；`acceptsForLine` 按 `ledger_id` 分流）
- [x] `spliceAccepts` / 折装路径按 `ledger_id` 分流（`foreman.ts` 的「对不上」从**放行改成拒**；
      拼好的在飞轮与台账行**同键同时刻**，各排在自己那一行的位置上）
- [x] 单测：两条并行在途行各自成轮、互不串台、无重字（`talkTurns.test.ts` 新增 4 条）；
      无 `ledger_id` 的事件归第一条在途行
- [x] 既有单测随判据更新：`foreman.test.ts` 的「ledger_id 对不上」一条改为**拒**，并补
      「不带 seq 也照拒」；`talkTurns.test.ts` 既有拼接用例原样通过（单基准时与从前逐字一致）

**实现注记（票面外，随本条一起定的事）：**

- **拼出来的在飞轮取那一行自己的时刻与 `proactive`**（`talkTurns.ts`）：上一版一律落在末尾、
  名牌恒按「值班长」写；两条并行时那样两条都顶着同一个名牌。现在 `at` 用行的 `created_at`
  （「排在它自己那一行的位置上」的字面落地），名牌读行的 `proactive`——值守那条写
  「值班长 · 值守」。
- **分流加一条兜底**：`ledger_id` **不在快照里**（新轮的第一批增量比快照先到）时归第一条
  在途行——«不认识就不改变行为»。少了这条，那个窗口里那一段直播会**静默消失**到下个快照
  （旧口径的放行正是为它留的）；「已在快照里却不是这一条」才拒。单测两条钉住。
