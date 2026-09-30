# 对讲台瘦身：台账进 store + 折叠态搬运机退场（talk-store-ledger）

**Status:** ready-for-agent

> **来源**：2026-09-30 架构体检 ②+④ 号卡 + 拷问定案。决策落 `docs/decisions.md` 354。
> 「判据在 lib/*、组件只接线」的纪律真执行了——本批处理的是纪律跑完剩下的：
> 台账与发送编排还住在组件里。

## Problem Statement

`Talk.svelte` 4051 行、script 1763 行，五个子系统同住：台账生命周期、发送状态机编排、
三套滚动跟随、pending 详情去重、watch 双人格。三处疼：

1. **串台守卫**是「每个 await 后手比对 sessionId」的口头纪律——五种手写、三种锚点拼法，
   漏一处即历史根因「非本次会话内容」。
2. **epoch 协议**：store 因「台账住在页面」被迫用 `bindRecalibrate` + `ledgerEpoch` 朝页面喊话。
3. **折叠态搬运机**：在飞轮键 `'live'` 与落库键 `m<id>` 不一致，养出 carryLiveStepOpen /
   carryLiveTurnOpen / settlingTurn / 组件 effect 四件组成的两段提交机。

## Solution

- 台账生命周期（reload / loadEarlier / switchTo / 归档坠落 / seen 标记）搬进
  `stores/talk.svelte.ts`
- 发送编排收成 `talk.sendTurn()`；串台守卫变 `talk.claim()`（捕获 sessionId + 台账代际，
  返回闭包）；epoch 协议、页面队列排水 effect、`markLedgerStale` 整段删除
- **行为修正被接受**：队列进 store 后页面关着也照排照发（排水节奏照旧）
- 在飞轮保留 `'live'` 临时键，**首条带 `ledger_id` 的事件到达即改用 `m<ledger_id>` 且此后
  恒定**（经事实核实：切换点在首条事件、彼时尚无任何可折叠内容）——四件搬运机制删除，
  与 mergeLive 路径键形归一
- 附注：`GET /foreman/session` 应答回显自己的 `page_limit`（additive 字段），
  前端 `SESSION_PAGE_LIMIT=500` 常量退场

## Constraints

- watch 双人格拆分不做（另一棵树）；三套滚动跟随只随台账搬家被动整理
- e2e ㉖ 回看拼接 / ㉗ 滚到顶加载 / ㉘ 归档翻回是验收基线（决策 312 / 301）

## 执行顺序

全盘点第四批。票序：01 → 02 → (03, 04)。
