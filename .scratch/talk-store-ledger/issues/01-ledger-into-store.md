# 01: 台账生命周期搬进 talk store，epoch 协议退场

**What to build:** 决策 354①——班次台账生命周期（reload / loadEarlier 游标翻页与滚动
位置保留 / switchTo / openFreshSession / submitArchive 归档坠落 / seen 标记
rememberLanding）从 Talk.svelte 搬进 `stores/talk.svelte.ts`。store 本就持有
sessionId / ledgerEpoch / 连接 / 哨兵定时器 / 队列，缺的只是台账这一块。
`bindRecalibrate` + `ledgerEpoch` 的 store→页面喊话协议、`markLedgerStale` 整段删除。

**Blocked by:** None

**Status:** done（已实现，决策 354①）

- [x] store 新增：session / sessionList / loading / loadError / hasMoreEarlier /
      reload / loadEarlier / switchTo / 归档坠落 / seen 标记——`loadError` 分两格
      （`loadError` / `loadErrorPairing`，与页面那两个别名一一对应），`attention`
      与 pending 详情指纹一并随台账归 store（见注记 ①⑧）
- [x] epoch/recalibrate 协议删除：`ledgerEpoch` 字段、`bindRecalibrate`、`markLedgerStale`
      三样在 store 与页面两侧全部退场，页面那枚 `seenEpoch` 效果整段删除——喊话两端都没了；
      `untrack` 那半句见注记 ⑥
- [x] pending 详情指纹去重的 `$effect` 随台账归 store 管——判据（指纹只含 id 与 pending
      类型 / 集合不变不重拉 / 值守账只读不拉）在 `talk.syncPendingDetails`，页面只剩
      `$effect(() => talk.syncPendingDetails(board.pendingTasks, watchMode))` 一行接线
- [x] 行为零变化确认：e2e ㉖ 回看拼接 / ㉗ 滚到顶加载 / ㉘ 归档翻回照绿
      （`e2e/talk.spec.ts` 全套 **42 条过**，2.3m）
- [x] store 单测补台账生命周期（reload 合并已分页旧消息、loadEarlier 不滚动、
      归档坠落到下一班次）——`frontend/src/stores/talk.test.ts` 新增「台账生命周期
      （决策 354①）」一组，文件共 29 条

**注记（留给后来者）**：

- ① **`attention` 随台账归 store**（本票自定的搬法）：它原本是页面的一枚 `$effect` 盯
  `sessionId` 重拉。台账搬走之后那枚效果的依赖成了「本次装载 / 切换」——正好就是
  `reload` 的触发面，故收进 `reload` 尾部的一跳（`loadAttention`，读失败退 `null`、
  不打断这一屏）。不这么收就得让页面再挂一枚盯 store 的效果，等于把刚搬走的判据请回来一半。
- ② **seen 的装载时机**：页面原先每次挂载 `loadSeen()` 重读一遍 localStorage，现在
  store 构造时读一次（`seen = $state(loadSeen())`）。数据上等价（写口只有本 store，
  没有第二方改这个键），差别只落在「同一寿命里被别处改过 localStorage」这种前提下——
  本仓没有这个前提。`seenSeeded` 的语义一字未动（基线仍只在**本机一条记录都没有**时立）。
- ③ **`showArchived` 住 store + 挂载时拨回关**：它是浏览动作不是身份（票 06 的口径），
  而 store 寿命比页面长——故挂载时显式拨回 `false`，与「下次进本页回到关」逐字一致。
- ④ **重挂载不再回骨架**：`loading` 是 store 的一格，首次装载完成之后就一直是假，
  切页再回来是「先显示手上的台账、同时重读」（stale-while-revalidate），不再是空白骨架。
  这是「台账住在 store」的自然结果——页面已经不再拥有「我这一屏还没读到」这个事实。
  与决策 275 给在飞现场的同一条口径（切页不丢、回来即见），e2e 未见反对。
- ⑤ **`markLedgerStale` 那一跳被删**：它喊的是「再读一次台账」，而它前面那句
  `if (await reload(sid))` 刚刚读成——那一跳本来就是一次冗余重读（页面侧会因此多发一枪）。
  喊话协议退场时它没有留下的理由；`reload` 的过期回包守卫仍保证「谁读谁认账」。
- ⑥ **`untrack` 的账**：本票能清的都清了——epoch 那枚 `$effect` 整段删除（它把
  `talk.ledgerEpoch` 当**依赖**读，本来就不带 `untrack`），滚动跟随里那处
  `untrack(() => loadingEarlier)` 随字段搬进 store（`untrack(() => talk.loadingEarlier)`，
  注释里「向上补历史那一拍不滚」那条纪律照旧成立）。**仍活着的两处**（`liveSeen` /
  `liveSid` 那一对，`Talk.svelte` 的收口搬运效果）属于**折叠态搬运机**——决策 354③ 明写
  四件机制整段删除，那是**票 03** 的地界；本票不碰同一段（两张票改同一处必然打架）。
- ⑦ **顺手收掉的一处错（显式记）**：`refreshSessionList` 里 prune 看过表那一跳加了
  `kind === 'talk'` 门。页面旧代码无条件 prune——而看过表说的是**人的账**（决策 220③），
  值守账的名单是另一本，拿它 prune 会把对讲台刚记下的看过时刻抹掉（在值守账上拨一下
  「显示已归档」就能撞见）。台账搬进 store 时判据就在手边，故一并修正；这是本票唯一的
  行为改动，方向是「让标记说实话」，对讲台那一侧零变化。
- ⑧ **`writeSessionAddress` 加路由门**：落点写地址原本只发生在页面里的切换 / 装载。
  搬进 store 之后**页面关着也会重读**（落地哨、重连校准）——那时地址栏是别的页的，
  写进去就是污染。故只在 `talk` / `talk-watch` 两条路由上写。
- ⑨ **「这本账是谁的」只有一个判据**：页面那枚 `ledgerKind`（`watchMode` 的模板渲染
  判据）是它，store 的 `kind` 只是它在页面之外还记得的那份副本——挂载时
  `talk.kind = ledgerKind`，不再就地重写一遍三元。
- ⑩ **文档与扫描面跟着判据走**：`delegation-scan.test.ts` 两处断言从页面挪到 store
  （落地哨入口、向上游标），并新增两条牙齿钉住「滚动几何留在页面 / store 的
  `loadEarlier` 不摸 `scrollTop`」；`design/frontend-design.md` §12.3 六行改指 store
  （班次落点 / 两枚标记 / 换班重置 / 重连校准 / 向上游标 / 过期回包），其中过期回包那行
  原先写着「无自动化见证」，现在有了（`talk.test.ts` 的「切班后过期的回包整包丢弃」）。
  另顺手修了 §12.3 里 4 处 `crates/core/src/pipeline/foreman.rs`——**决策 351 搬家遗留的
  主仓红**（`behavior-map.test.ts` 按路径存在性断言，那一票里前端测试没跑到）。
- ⑪ **验证**（2026-09-30）：前端单测 **1072 绿 / 83 文件**（`talk.test.ts` 29 条，
  其中台账一组 8 条）；`npx svelte-check --tsconfig ./tsconfig.json` **0 错 0 警**；
  e2e `e2e/talk.spec.ts` 全套 **42 条绿**（含 ㉖㉗㉘ 三条验收线，另含「切走再回来」
  「新建 / 切换 / 重命名 / 归档」两组）。代码审两轴（Standards / Spec）跑过。
- ⑫ **本票不动的东西**：发送编排与串台守卫（票 02 的 `sendTurn` / `claim`）、在飞轮
  渲染键与折叠态搬运机（票 03）、后端 `page_limit` 回显（票 04）、watch 双人格拆分与
  三套滚动跟随的合并（决策 354 的明确不做）。页面那一侧的重连校准入口原样保留
  （`talk.init()` 的 `onRecalibrate` 仍是 `reload`），只是登记时机从「页面挂载」
  改到「store 起收」——页面关着也该补这一跳。
