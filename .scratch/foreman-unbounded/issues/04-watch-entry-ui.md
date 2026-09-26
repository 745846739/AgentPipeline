# 04: 前端：值守独立入口与只读账

**What to build:** 值守时间线有自己的入口与列表，是**只读的一本账**（看、翻、点开详情，不出输入坞、不出发送态）；每条播报带一个「转去对话」动作（把这件事带进人的时间线并聚焦输入坞）。人的时间线不再渲染播报（**存量旧行照旧渲染**，配合裁决 12）。

**Blocked by:** 01

**Status:** done（已实现，决策 290）

- [x] 入口与列表：`Talk.svelte`(会话列表 :1208-1338)、`frontend/src/stores/talk.svelte.ts`、`frontend/src/lib/talkSessions.ts` —— 两条时间线各自的列表与「正在回话」标记（`talkSessions.ts:165-179`），值守的标记读它自己的 `turn_in_flight`
- [x] 呈现：`frontend/src/lib/talkTurns.ts`(:379-392 的名牌) 在值守账里不必再靠 `proactive` 猜（类型由 `kind` 带出，:90/:278 那条链路改成按班次类型）；播报条目上给「转去对话」
- [x] 只读约束：值守账**不渲染输入坞与发送态**；`turn_in_flight` 只用来显示「值守正在跑」
- [x] 实时分流：`appendForemanEvent`(realtime/foreman.ts:104-125) 按班次类型把增量送进对应时间线（现在只按 agent_type + session_id 过滤，播报会并进人的那一屏）
- [x] `frontend/src/api/client.ts` / `types.ts`：会话与消息带 `kind`
- [x] vitest（turns 构建与分流、值守账无输入坞）+ e2e 一条（切到值守账 → 点「转去对话」→ 人的时间线聚焦）
- [x] §12.3 行为表补行 + 四门 + 决策落号

## Comments

- 来源：拷问 Q2 / Q12(a) 与裁决 2、11。今天前端对值守的唯一专属呈现是一句名牌文案，`proactive` 不参与任何 CSS——所以这不是「把已有区分用起来」，是真的新做一条时间线。
