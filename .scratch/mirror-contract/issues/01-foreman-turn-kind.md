# 01: 值班长消息「这一行是什么」由后端给字段，前端不再解析正文哨兵（决策 252 落档）

**What to build:** 前端今天靠**正文前缀**判断一条值班长台账行是什么，而有三个前缀是后端写进正文的。
本票把「这一行是什么」改成后端给的字段，前端不再解析正文。

**三处哨兵，两种处置：**

1. **`FOREMAN_ATTRIBUTION_MARK`（`【归因】`）——直接删，连带那条假测试。**
   前端生产代码**从不读它**：`message_wire`（`crates/app/src/routes/foreman.rs:1296-1307`）早已把
   前缀剥掉、解析成 `attribution` / `attribution_label` / `attribution_reason` 三个字段发出来了
   （决策 235 / 238）。前端那份（`frontend/src/realtime/foreman.ts:303`）只出现在自己的定义与
   `frontend/src/realtime/foreman.test.ts:373` 那条断言里。**那条断言是
   `expect(FOREMAN_ATTRIBUTION_MARK).toBe('【归因】')`——前端字面量与前端字面量自比**，Rust 改了
   它照样绿，而用例标题写着「哨兵与后端同源（原样镜像）」。常量与用例一起删，**零行为变化**。

2. **`FOREMAN_WATCH_MARK`（`【值守播报】`）与 `FOREMAN_FAILED_TURN_MARK`（`【没跑起来】`）——
   改成后端给的字段。**
   前者由后端拼进正文（`crates/core/src/pipeline/foreman.rs:1349`），前端 `Talk.svelte:337,427`
   靠 `startsWith` 认它才能把名牌渲染成「值班长 · 值守」；后者同形，`realtime/foreman.ts:320,338`
   与 `Talk.svelte:330,412` 靠它把一条 `system` 行分成「操作台记的一轮」与「没跑起来的那一轮」。

**落地形状：`message_wire` 增加两个字段。**

- `kind`：`"mine"` / `"console"` / `"failed"` / `"fm"`，承载「这一行是什么」。
  取代 `Talk.svelte:409-414` 的 `role` 三元式加失败哨兵判断，以及 `realtime/foreman.ts:320,338`
  的两处筛法。
- `proactive`：布尔，承载「是不是值班长自己醒来说的」。
  取代 `Talk.svelte:427` 的 `m.content.startsWith(WATCH_MARK)`。

**为什么是两个字段而不是一个五值枚举**：两件事**正交**——「自发的轮失败了」（值守轮失败）是可能
的组合，今天它算成 `failed` 且 `proactive === false`（因为 proactive 要求 `role === 'assistant'`）；
压成一个枚举会让这个组合**从形状上不可能**。分开写，将来要显示「值班长自己醒来说的那一轮失败了」
时不必再动接口。

**`role` 字段保留**：存储与查询仍在用，且它是观测类字段（`crates/core/src/storage/foreman.rs:185`
明写非法值不打断查询）。

**`content` 里的哨兵前缀不能删**——它是给模型看的（`foreman.rs:887` 的值守简报模板、`:1349` 的
播报写入），也是人翻台账时认得出「这条是系统写的」的标记。本票只改**前端怎么认**，不改
**后端写什么**。

**Blocked by:** None（可立即开始）

**Status:** done（2026-09-23 实现）

- [x] `crates/app/src/routes/foreman.rs::message_wire`（`:1286-1307`）增加两个字段：`kind`
      （按 `role` + `FOREMAN_FAILED_TURN_MARK` 前缀判定：`user → "mine"`；`system` 且带失败前缀 →
      `"failed"`；其余 `system → "console"`；`assistant → "fm"`）与 `proactive`
      （`role === FOREMAN_ROLE_ASSISTANT && content.starts_with(FOREMAN_WATCH_MARK)`）。
      **判定逻辑住在后端**——前端不再各判一遍
- [x] `frontend/src/api/types.ts` 的 `ForemanMessage` 加 `kind: 'mine' | 'console' | 'failed' | 'fm'`
      与 `proactive: boolean` 两个字段
- [x] `frontend/src/realtime/foreman.ts`：删 `FOREMAN_ATTRIBUTION_MARK`（`:296-303`）；
      删 `FOREMAN_FAILED_TURN_MARK`（`:283-286`）与它在 `:320` / `:338` 的两处 `startsWith` 筛法，
      改用 `m.kind === 'failed'`
- [x] `frontend/src/routes/Talk.svelte`：删 `FOREMAN_FAILED_TURN_MARK` / `FOREMAN_WATCH_MARK` 的
      导入（`:91-92`）与 `FAILED_TURN_MARK`（`:330`）/ `WATCH_MARK`（`:337`）两个常量；
      `:409-414` 的 `kind` 三元式改读 `m.kind`；`:427` 的 `proactive` 改读 `m.proactive`
- [x] `frontend/src/realtime/foreman.test.ts`：删 `:371-374` 那条自比自的用例；
      把 `:191` / `:193` 那两处用 `FOREMAN_FAILED_TURN_MARK` 造的 fixture 改成直接给
      `kind: 'failed'` / `kind: 'mine'`；**补用例**断言 `failed` 与 `proactive` 按字段取值
      （含「assistant 且带失败前缀 → `kind: 'failed'` 且 `proactive: false`」这个今天分不开的组合）
- [x] `crates/app/tests/integration/api_contract.rs` 加断言：按构造的四个场景（人的话 / 操作台 /
      没跑起来的一轮 / 值班长的话）各取一条消息，断言 `kind` 取值正确、`proactive` 只在播报轮为
      `true`；**并断言 `content` 里仍带那个哨兵前缀**（前缀不删，只是不再由前端解释）
- [x] `docs/decisions.md` 追加**决策 252**（本批已落，核对即可）；
      `AGENTS.md:5` 与 `docs/README.md:18` 的计数改到 `#1–254`
- [x] `docs/glossary.md` 加词条「机器可读的种类」（本批已落，核对即可）
- [x] `make check` 绿

## 实现者记事（2026-09-23）

**票面自身有一处矛盾，已按落地形状那一版处置，记在此处**（票 02 同款要求：暴露了什么就
就地记在票面）。上面那条测试要点写的是「含『assistant 且带失败前缀 → `kind: 'failed'` 且
`proactive: false`』这个今天分不开的组合」，而**落地形状那一栏的判定顺序**写的是
`user → mine`；`system` 且带失败前缀 → `failed`；其余 `system → console`；`assistant → fm`
——**助理轮无条件落在 `fm`**。两者只有一条能对。

按**落地形状**那一版实现，两条理由：

1. **另一条是实现不出来的行为变化，不是本票的范围**。今天那条三元式（`Talk.svelte:409-414`）
   的失败判定整个住在 `system` 分支里，助理轮永远走不到它；照测试要点实现等于顺手改掉一档
   归属，而本票的口径是「唯一的行为变化是线上多两个字段」。
2. **照测试要点实现会开一个伪造面**。失败账一律由后端以 `role = system` 写
   （`record_failed_turn` / `record_interrupted_turn`），而**助理轮的正文来自模型**。让模型的
   措辞能把自己那一行染成红色失败轮——「发送失败」的名牌加一个编出来的原因——正是要防的事。

**那个组合今天确实分不开，而两个字段照样为它留了位**：实测是「值守轮失败」——后端写入路径
今天写成 `system`，故落成 `failed` 且 `proactive = false`，与一条普通失败轮形状相同。
决策 252③ 的说法是「分开写，**将来要显示时不必再改线**」（留位，不是留值），契约测试照此
断言：播报轮 `fm` + `proactive`，它失败的那一轮 `failed` + `false`，并另立一条断言钉住
「助理轮不因正文前缀被判成失败轮」。

**唯一的行为变化**：消息线上形状多 `kind` / `proactive` 两个字段，前端不再解析正文。
除此之外**行为中性**。

**已知缺口（本票未补，如实记下）**：`proactive` 的**消费端**没有单元见证——线上取值由
`api_contract.rs::the_session_wire_says_what_each_row_is` 钉住，而「名字牌渲染成
『值班长 · 值守』」那一支只有 `design/frontend-design.md` §12.3 的行为映射行（它只断言引用
的路径存在，不断言值）。**这不是本票造成的缺口**：改动前那一支是
`m.content.startsWith(WATCH_MARK)`，同样没有测试。本仓对路由级组件的代价有实测口径
（决策 251④：拆模板不增可测性，`e2e/talk.spec.ts` 29 条驱动真界面），故补这一条应当走 e2e、
另立一票，不塞进本票。

**明确不做**：不改存储层 `role` 列（`storage/foreman.rs:24` 的三个取值原样）；不给 `proposal`
造 `kind`（它来自 `session.proposals`、不是消息行，`Talk.svelte:432-...` 的合流不动）；不删
`content` 里的哨兵前缀；不给配对 403 加 `kind`（那是决策 251 票 04 的范围，同一条口径但不同票）。
