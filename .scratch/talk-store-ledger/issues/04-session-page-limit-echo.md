# 04: session 分页应答回显 page_limit

**What to build:** 决策 354 附注——`GET /foreman/session` 的应答载荷回显它自己的
`page_limit`（additive 字段，serde 缺省向后兼容）；前端 `SESSION_PAGE_LIMIT = 500`
常量（Talk.svelte:216，「与后端同一个数」的注释合同）退场，`hasMoreEarlier` 改从
`fresh.length >= payload.page_limit` 取值。分页语义（older.length >= limit、空页 = 到底）
在前端的两处复读（640 / 694）收敛到 store 一处。

**Blocked by:** 01

**Status:** done（已实现，决策 354④）

- [x] 后端：session 分页应答加 `page_limit` 字段（additive；Rust 侧常量同一出处）——
      `routes/foreman.rs::session_payload` 的**两条支路**（空班次与真班次）都回显
      `SESSION_PAGE_LIMIT` 这一个常量
- [x] 契约测试：`api_contract` 两条（空 home 那一支照样给、形状恒定；分页那一支断言
      回显 500 与这一段**实际取数上限**同源）
- [x] 前端：常量退场、`hasMoreEarlier` 从载荷取值、分页语义单点化——
      `stores/talk.svelte.ts` 新增私有 `markHasMore(got, payload)`，`reload` 最近一段
      与 `loadEarlier` 更早一段共用它；`api/types.ts` 的 `ForemanSession` 加 `page_limit`
- [x] e2e ㉗（滚到顶加载更早、接缝不重不漏、到头即止）照绿——判据换主人，行为不变

**注记（留给后来者）**：

- ① **`page_limit` 是加性字段**：老客户端不读它，行为逐字不变；前端与后端同仓同发
  （被测二进制编译期内嵌 `frontend/dist`），故 TS 侧写成必填 `number` 而不是可选
  ——写可选就等于留了一条「读不到就回落某个默认数」的路，那正是本票要拆掉的东西。
- ② **空班次那一支也给**（载荷形状恒定）：前端那条判据因此不必为「这台机器还没有班次」
  分岔；也顺带让契约测试能在最便宜的那个现场钉住这个字段。
- ③ **「空段即到底」没有单独写一条分支**：`markHasMore` 里 `0 >= page_limit` 自然为假；
  `loadEarlier` 里那处早退管的是**返回值**的语义（没接上新段），不是 `hasMoreEarlier`。
  两条判据各管各的，这一点写在 `markHasMore` 的注释里。
- ④ **顺手收敛的注释**：store 与 `api/client.ts` 里三处硬写「500 条」的注释一并改成
  「一页 / `page_limit` 条」——本票的判据是「这个数只有一个主人」，注释里留着第二个
  数字就还会有人照着它对齐。
- ⑤ **验证**（2026-09-30）：`crates/app/tests/integration/api_contract.rs` 的
  `foreman_session_is_available_on_an_empty_home` 与
  `foreman_session_messages_page_up_with_before_id` 两条绿；前端单测 `talk.test.ts`
  新增「分页尺从载荷取（决策 354④）」1 条（载荷说 2 就按 2 判——判据若还挂着前端那个
  常量即红）；全量前端 1087 绿 / 83 文件；`svelte-check` 0 错 0 警；
  `cargo fmt --check` / `clippy --workspace --all-targets -D warnings` 绿。
- ⑥ **本票不动的东西**：`before_id` 游标的语义与段内升序（票 05）一字未动；
  滚动几何仍留在页面（`Talk.svelte` 只记高度、段接上后补 `scrollTop`）。
