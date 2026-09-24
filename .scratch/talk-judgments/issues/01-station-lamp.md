# 01: 值班板的工位灯对「全部失败」点红灯——并让聚合只有一处实现（决策 251 落档）

**What to build:** 对讲台右栏的值班板（`crew`，`Talk.svelte:306-320`）今天把「该工位的任务全失败」
并进 `idle`，画成一个空的灰灯框；而看板同一列是红的，规格（`design/theme-6-pixel.md:628`）
却说值班板是「同一份读数在看板 8 列与顶栏灯带上各有一份，**这是第三份**」、
`:620` 把它归在**状态直陈**层。本票把这三处聚合收成一处：

- **修 `lib/pipeline.ts:487 aggregateStationState` 的优先序**为看板的次序
  （pending → running → failed → done → idle）。它今天是 `pending → failed → running → done → idle`，
  与 `BoardColumn.svelte:59-61` 的可见行为**不一致**。它**零消费者**（全仓 grep 只命中定义），
  故改它不牵动任何既有路径。
- **Talk 的 `crew` 改调它**，不再自己推第三遍；出参从 `'run'` 改回契约词表 `'go'`
  （`StationState` 在 `lib/pipeline.ts:286` 是 `'done' | 'go' | 'warn' | 'stop' | 'idle' | 'dev' | 'test'`——
  Talk 用的 `'run'` **根本不在契约里**）。
- **顺带两处同模块的重复**：`stageSprite`（`Talk.svelte:1165-1168`）重写了
  `lib/pipeline.ts:72 columnForStage`（同一件「stage → 哪一列」的判断）；`crew` 返回的
  `sprite` 字段**模板一处都没读**（`grep 'c.sprite'` 零命中），是死字段。
- **加 `.blamp.x` 与失败行变体**（Talk 自己的 `<style>`，`:2516-2534` 是 `.blamp` 那一族）。
  **不需要动契约**：`--stop` 已在 `theme/contract.ts:340`（`'#FF6157'`，浅色 `#B3271E`）
  与 `app.css:46` 存在。
- 本票同时把**决策 251** 追加进 `docs/decisions.md`（六条裁决一次记齐，见票面末段），
  并改 `design/frontend-design.md` §12.3 引用到 `crew` 的那两行（`:801` / `:788` 附近的措辞核对，
  见 README 第六节）。

**Blocked by:** None（可立即开始）

**Status:** done（2026-09-23 实现；2026-09-24 收口：`make check` 四段全绿——fmt+clippy / cargo test / vitest 778 / e2e 121 通过 0 失败，交付说明里「e2e 待跑」一段已由本轮补上。四处偏差见交付说明，故 32–36 里被偏差覆盖的格子保持未勾）

- [x] `lib/pipeline.ts:487 aggregateStationState` 优先序改为 `pending → running → failed → done → idle`；**先补测试**（该函数今天零测试）：四盏灯各一条 + 优先序两条（pending 压过 running/failed；running 压过 failed）+「空列 → idle」+「全 done 才算 done」
- [x] `Talk.svelte:306-320` 的 `crew` 改调 `aggregateStationState(tasks.map((t) => t.status))`，删掉内联的 `pen` / `live` / `don` 三个谓词与 `state:` 三元式；出参词表改为 `StationState`
- [x] 模板 `:1929` 的 `c.state === 'run'` 改为 `'go'`，并按状态补 `.blamp` 变体（`warn → w`、`go → c`、`done → d`、**`stop → x`**、`idle → 无变体`）；`<style>` 里加 `.blamp.x { background: var(--stop); border-color: var(--stop); }`
- [ ] `:1928` 的 `<li>` class 三元式按同一词表对齐（`warn → pen`、`go → hot`、**`stop → 失败行变体`**；`stop` 的 `li` 变体若无既有点法，可只上灯不改行，**不为它新造图元/颜色**——决策 169 / 200 的纪律）
- [ ] `stageSprite`（`:1165-1168`）改为调 `columnForStage` + `COLUMN_SPRITES`，或删除改由 `crew` 直接给出（与上一条同一处产出），二选一，不两处都留
- [ ] 删 `crew` 返回的 `sprite` 死字段（模板零读取）
- [x] **补一条能看见失败灯的 e2e**：`e2e/talk.spec.ts:203` 今天只断言 `.talk .blamp` **数量是 8**，不验颜色/变体——故本票的行为变化**没有任何自动化见证**。要求：造一个该工位任务全 `failed` 的态势，断言那一盏 `.blamp` 带 `x` 变体且计算色为 `--stop`
- [ ] `BoardColumn.svelte:59-61` **本票不动**（它已是正确形状，且 `e2e/pixel-theme.spec.ts:225-229` 在量它的灯箱 12×12）
- [x] `docs/decisions.md` 追加**决策 251**；`docs/README.md:18` 与根 `AGENTS.md:5` 的 `#1–250` 改为 `#1–251`
- [x] `design/frontend-design.md` §12.3 **新增一行**记录本票的行为规则（落成 :802）（**今天没有这一行**——表里指到值班板的只有 `:801`「换班次不重置看板派生的东西」与 `:788`「工位回执默认收起」，两条都不是灯色）。新行的实现位置只许写**已存在的路径**（`frontend/src/lib/pipeline.ts`、`frontend/src/routes/Talk.svelte`、`frontend/src/components/board/BoardColumn.svelte`——251② 让列头也读 helper，故它也在表行里），备注列必须带编号（决策 251），格式约束见 `lib/behavior-map.test.ts:100-129`——**不得引尚未落地的路径**（`PENDING` 名单是空的，引了当场红）
- [x] `make check` 绿（2026-09-24 分段取证：fmt+clippy / `cargo test --workspace` / vitest 778 + svelte-check 0 错 + build / e2e 121 通过 0 失败）

**决策 251 的六条裁决（本票一次记齐）**：

1. 值班板**对「全部失败」点红灯**，与看板同读数（修订「值班板只是三态灯条」的既有实现口径）；
2. 工位聚合的**优先序以看板为准**（pending → running → failed → done → idle），
   `aggregateStationState` 是**唯一实现**，Talk / 看板都读它；
3. 值班板的词表**回到契约 `StationState`**（`'run'` 是词表外的自造值，退场）；
4. 候选 10 的**范围裁定**：只抽判断，不拆模板（路由级组件测试的代价有实测支撑，
   见 README 第三节）；配对判据与动作身份**各拆一票**（票 04 / 05）；
5. 键盘陷阱的共享做法选**纯判定 + `attachMenuTrap` 普通 helper**，**不引入 Svelte `use:` action**
   （全仓零先例、零文档表态，不在一次去重里首次立模式）；
6. 三块判断都配一条**静态扫描守卫**，落在 01–03 里最后落地的那票——
   单测证明不了「Talk 还在用这个 module」。


## 交付说明（2026-09-23）

**闸门**：`cd frontend && npm test` → **62 文件 / 741 条全绿**（本票新增
`lib/pipeline.station.test.ts` 7 条 + `lib/delegation-scan.test.ts` 的工位灯部分）；
`npm run build` 绿；`svelte-check` 无**本批**错误（余 2 条在并发会话在飞的
`lib/specTablesFixture.test.ts`）。**`make check` 与两条 e2e 未跑**：`cargo` 编译卡在并发
会话在飞的 `crates/app/src/serve.rs:524`（`AppState::with_market_override` 尚未定义），
本批不动任何 Rust，故 e2e 待该编译恢复后补跑。**2026-09-24 已补跑**：`make check` 四段
分段取证全绿（fmt+clippy / `cargo test --workspace` / vitest 778 + svelte-check 0 错 + build /
e2e 121 通过 0 失败，本票的失败灯 e2e 在列）。

**四处偏差（按事实记，均已在代码或本票面就地写明理由）**：

1. **`BoardColumn.svelte` 被改了**（本票原写「本票不动」）。改的是 `stationState` 那一行——
   改为调 `aggregateStationState`。理由：**决策 251② 要求它是「唯一实现、Talk / 看板都读它」**，
   而本票那句「不动」与它自相矛盾（251 是权威，票面是我后写的窄句）。逐字等价已核：同
   `spineTasks`、`hasFailed` 本就含 `cancelled`、优先序修完后两串同序，故**没有第二处可见变化**；
   `pixel-theme.spec.ts:225-229` 的灯箱断言照旧绿。决策 251 的「明确不做」已同步改成如实描述。
2. **`colState` 留在原地**（同一优先序的另一份词表 pen / live / fail / don / idle，与
   `stationState` 共用那四个谓词）。评审指「去重停在浅一层」——成立，但它是**另一份词表**，
   收进 helper 等于把两个概念（section 三态词表 vs 信号灯词表）绑成一个，且会动看板的可见
   样式而 e2e 此刻跑不了。**记为后续一票**，不塞进本票。
3. **`stageSprite` 两处都留着**（本票原写「二选一，不两处都留」）。两项都不成立：① 委派给
   `columnForStage` 会把 `sync-check` 从台账箱变成锤子 sprite——那是**新的可见行为**，违反
   §五「行为变化只有一处」；② 「改由 `crew` 直接给出」做不到，`receipt()` 要的是**任一 trace**
   的 stage（可能落在没有工位行的列上），而 `crew` 只有 8 列。理由已写进 `stageSprite` 的
   docblock（并删掉了那里一处**不存在的裁决引用**——原先误写「决策 251② 落地时的裁定」）。
4. **失败灯 e2e 造的是 `cancelled` 而非 `failed`**。走 `POST /tasks/{id}/cancel`（同步写、
   立即落终态），而 `failed` 只能靠坏 provider 把重试耗尽——先例 `ux-audit-2.spec.ts:385`
   那条只能 `.catch(() => undefined)` 容忍它没跑到。**两者同走一条灯臂**
   （`aggregateStationState` 的 `failed || cancelled` → `stop`），而 `failed` 那一格由
   `pipeline.station.test.ts` 的 `aggregateStationState(t('failed')) === 'stop'` 单独钉住。
