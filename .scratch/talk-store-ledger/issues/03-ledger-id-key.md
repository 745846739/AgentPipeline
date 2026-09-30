# 03: 在飞轮改键 ledger_id，折叠态搬运机整段删除

**What to build:** 决策 354③——在飞轮保留 `'live'` 临时键（乐观段，尚无任何可折叠内容），
**首条带 `ledger_id` 的事件到达即改用 `m<ledger_id>` 且此后恒定到收口**。经事实核实
（2026-09-30）：切换点在首条事件，彼时思考体尚未渲染、不存在用户折叠态可丢；
mergeLive 路径（talkTurns.ts:482）本就用 `m<base.id>`，两路键形归一。
`carryLiveStepOpen` / `carryLiveTurnOpen` / `settlingTurn` / 组件搬运 effect 四件删除。

**Blocked by:** 01, 02

**Status:** ready-for-agent

- [ ] `talkTurns.ts`：live 轮在首条带 ledger_id 事件后即以 `m<ledger_id>` 为键；
      乐观段（无事件）保持 `'live'`
- [ ] 删除：carryLiveStepOpen / carryLiveTurnOpen / settlingTurn / Talk.svelte
      1208–1235 一带的搬运 effect 与 liveSeen / liveSid 暂存
- [ ] 单测（01/02 新建的测试面）：首事件即换键、换键瞬间无折叠态、
      收口后键不变（「收口后不被自动打回」，决策 301）
- [ ] e2e ㉖ / ㉗ 照绿；收口拼接路径（决策 312）无第二个依赖旧键的读者
