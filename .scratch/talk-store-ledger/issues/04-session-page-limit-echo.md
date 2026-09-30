# 04: session 分页应答回显 page_limit

**What to build:** 决策 354 附注——`GET /foreman/session` 的应答载荷回显它自己的
`page_limit`（additive 字段，serde 缺省向后兼容）；前端 `SESSION_PAGE_LIMIT = 500`
常量（Talk.svelte:216，「与后端同一个数」的注释合同）退场，`hasMoreEarlier` 改从
`fresh.length >= payload.page_limit` 取值。分页语义（older.length >= limit、空页 = 到底）
在前端的两处复读（640 / 694）收敛到 store 一处。

**Blocked by:** 01

**Status:** ready-for-agent

- [ ] 后端：session 分页应答加 `page_limit` 字段（additive；Rust 侧常量同一出处）
- [ ] 契约测试：`api_contract` / `tests/integration/foreman.rs` 钉新字段
- [ ] 前端：常量退场、`hasMoreEarlier` 从载荷取值、分页语义单点化
- [ ] e2e ㉗（滚到顶加载更早、接缝不重不漏、到头即止）照绿
