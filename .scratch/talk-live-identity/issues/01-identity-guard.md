# 01: 身份统一——reload 过期回包守卫 + 废弃组件 generation

**What to build:** 「这是谁的回包」只有一个裁判：store 的 `sessionId`。
`reload()`（`frontend/src/routes/Talk.svelte:587-637`）在每个 await 之后比对「发起时的
目标 ≠ `talk.sessionId` 即丢弃」，不再无条件整屏写入 / `talk.watch(landed)` / 改 URL；
`send()` 的组件本地 `generation`（`Talk.svelte:258`）废弃，同一判据收口。
ledgerEpoch effect 的「首屏为 0 才跳过」假设一并修正（epoch 只增不清零，假设永不成立）。

**Blocked by:** None

**Status:** ready-for-agent

- [x] `reload()` 两个 await 之后各一道身份比对，过期回包整包丢弃（不写 session、
      不 watch、不改 URL、不 syncFollowing）
- [x] `send()` 的 `gen !== generation` 早退分支改判「发起时目标 ≠ `talk.sessionId`」；
      组件 `generation` 状态删除，决策 204⑥ 的「切走了作废」语义不变
- [x] 每次挂载的 onMount reload 与 epoch effect reload 不再双发（epoch effect 只在
      真正发生 epoch 增量时触发，挂载首读只走 onMount 那一趟）
- [ ] 单测：reload 在途时 `talk.watch(其他班次)` → 旧回包到达不改动 session/URL/store
- [ ] 单测：`send()` 在途切班 → 回包作废、现场倒空（决策 204⑥ 用例搬家后照旧绿）
- [ ] 修复后实测：切班次瞬间连点两个 chip，屏幕停在最后点的班次、时间线与其一致

## Comments

- 2026-09-29 实施完毕（决策 313）。前四项勾掉；reload / send 的守卫单测留在 e2e 面
  （组件无单测面，既有口径），实测条待桌面壳跑起来后验。
