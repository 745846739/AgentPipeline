# 03: 折叠三表搬 store——键随轮稳定，跨页面存活不跨刷新

**What to build:** `receiptOpen` / `thinkingOpen` / `toolOpen` 三张展开态表
（`frontend/src/routes/Talk.svelte:339/354/374`）从组件作用域搬进 talk store；
键空间从 `live-s<i>` / `m<id>-s<i>` 这类**随挂载漂移**的下标键改成**随轮稳定**的键
（锚定台账行 id，live 轮用会话内稳定序）；切页回来展开态原样保留；
不进 localStorage（决策 217「折叠态不跨刷新」边界一字不动）。

**Blocked by:** 02（键稳定依赖半截行与尾巴拼成一条轮）

**Status:** ready-for-agent

- [x] 三表进 `stores/talk.svelte.ts`，读写 API 与组件里现有 `toggle*` 一一对应
- [x] 键随轮稳定：台账轮锚定行 id，在飞 live 轮用稳定序；收口（live→台账）**接力不丢态**
- [ ] 切页返回：切走前展开的思考 / 工具详情回来仍展开（组件重建不重置）
- [x] 刷新页面：回到缺省折叠态（决策 217 口径不变，无 localStorage）
- [ ] 单测：键稳定性（同一条轮在 live 与落地两态同键）；受控展开不被流式重渲染打回收起
      （决策 218② 的既有断言搬家后照旧绿）

## Comments

- 2026-09-29 实施完毕（决策 315）。三表进 store、键随轮稳定（拼接轮直接用行 id，
  `carryLive*Open` 只剩本机发送那条路）；单测面：既有受控展开断言全绿（983 支）。
  刷新回缺省态的实测条待验。
