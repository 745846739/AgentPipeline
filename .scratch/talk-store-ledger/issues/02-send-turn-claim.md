# 02: sendTurn 收编 + claim() 守卫 + 关页照排照发

**What to build:** 决策 354②——发送编排收成 `talk.sendTurn(text)`：begin → settle →
follow → 队列排水全进 store（store 已持有连接、哨兵定时器、队列）；串台守卫变
`talk.claim()`——捕获当前 sessionId + 台账代际，返回闭包，五种手写比对
（wanted / originSid / gen）全部换成一处调用。组件只留：输入框文本、失败时乐观回填
（「失败时不要清空输入框」）、渲染。**行为修正被接受**：页面关着队列也照排照发，
排水节奏照旧。

**Blocked by:** 01

**Status:** done（已实现，决策 354②）

- [x] `talk.sendTurn(text)`：含 sending 状态、pendingText、queue 排水、sentinel、
      follow——`syncFollowing` / `followAfterGiveUp` 本就是 store 的方法，本票把
      begin → POST → settle → finally 那一串搬了进来；「在飞就入队、空着就直接发」
      收成 `talk.submit()`（页面 `send()` 只剩 trim + 清空，见注记 ①）
- [x] `talk.claim()`：捕获 + 闭包比对；Talk.svelte 那五处手写比对
      （613 / 626 / 686 / 1456 / 1464 / 1486 一带）逐处替换——其中三处在票 01 已随
      `reload` / `loadEarlier` 进 store，本票把剩下三处（发送路径）连同 store 里
      那三处一并收成 `claim()`，`wanted` / `originSid` / `gen` 三个记号全部退场
      （守卫无参，见注记 ②）
- [x] 队列排水 effect 搬进 store，页面挂载不再排水前置条件——`startQueueDrain()`
      由 `init()` 起、`dispose()` 收，判据逐条对齐（见注记 ③）
- [x] 单测：关页排队照发、失败路径（传输失败 vs 真失败 vs 本地放弃）、串台守卫
      （claim 后换会话 → 丢弃）——`frontend/src/stores/talk.test.ts` 新增「发送编排
      （决策 354②）」一组 10 条（见注记 ⑤⑥⑦）
- [x] e2e 新增：关页排队、回来见到已发——`e2e/talk.spec.ts`「关着页排队的话照发」，
      判据落在**页面之外**（见注记 ⑧）；`talk.spec.ts` 全套 **43 条绿**（2.6m）

**注记（留给后来者）**：

- ① **`submit()` 是本票新增的入口**（票面只写了 `sendTurn`）：坞的入口要判「在飞就
  入队、空着就直接发」，而原先那条判据在页面里写一遍、排水 effect 里再写一遍。
  发送搬进 store 之后两处都在 store 了，索性收成一个入口 + 一个判据
  （`private get inFlight`：`sending` / `turn_in_flight` / `followingSince` 三项）。
  页面 `send()` 因此只剩 trim、清空、`talk.submit(text)` 三行。
- ② **`claim()` 无参，且显式修订决策 354② 的一处措辞**：354② 写的是「捕获 sessionId +
  **台账代际**」——后半句不成立。台账代际在票 01（354①）就随 `ledgerEpoch` 退场，而在途
  回包的裁判自决策 313 起只有 store 的 `sessionId` 一格（同一班的重读不换班次，按定义也
  不该作废这一趟）。故闭包只认这一格，**刻意不接收参数**：能传 id 就等于允许「认一个不是
  当前这一班的锚」。已在决策 354 那一行就地标注修订。
- ③ **排水环是 `$effect.root`**：`$effect` 不能在模块作用域裸调（store 单例在模块里构造），
  故用一个 effect root 包起来，句柄存 `drainRoot`，`startQueueDrain()` 幂等。
  判据与页面那个 effect **逐条对齐**，只是换成 store 自己的读数：`watchMode` →
  `this.kind === 'watch'`、`archivedOpen` → `this.session?.session?.archived_at != null`、
  `session?.turn_in_flight` → 同一格、`sending` / `followingSince` / `queueHeld[sid]` /
  队头——一条不增不减（`delegation-scan.test.ts` 钉住页面里既没有 `takeQueued(` 也没有
  那条 effect 的 `queueHeld[sid]` 判据）。
- ④ **单测为什么自己叫 `startQueueDrain()`**：`init()` 会去连真流（`TaskStream` 打真
  网络），测试里不能叫；而这个环是**唯一会自己往外发请求**的东西，若常开，别的用例留下的
  队列状态会在事后偷偷发一跳（`sendForemanMessage` 是 mock，但调用计数会脏）。故测试
  `reset()` 先 `stopQueueDrain()`，要验排水的用例自己起。生产上的两端仍是
  `init()` / `dispose()`。
- ⑤ **`sendingSid` 与 `failureBaseline` 跟着发送进 store**：两处都不是「这一屏」的事
  ——「刚发出去的是哪一班」（决策 220③ 的标记）与「这一趟之前台账里有哪些失败行」
  （决策 337 的差集）在页面关着那一刻发出去的发送里同样成立，页面本地留一份就必然对不上。
  页面改成读别名，判据（`ledgerOwnsFailure`）挂在渲染上不动。
  **一处随之而变的语义**：`failureBaseline` 不再随页面重挂清成空集——空集会把**任何**
  早先的失败行误当「这次新出现的那一条」从而压掉本地失败轮，正是决策 337 要挡的形状。
  也就是说这是朝既有口径收敛，不是新取舍。
- ⑥ **`unsentText`：失败那句话的载体**。失败那一刻只有 store 在场（页面可能关着、也可能是
  排水环自己发的那一趟），故这句话先落在 `talk.unsentText` 上，页面挂一枚效果消费它：
  框空着才填、填不填都清（它说的是「一次没送到的发送」，不是草稿）。时序与旧版逐字一致
  ——旧版也是在 catch 里**无条件回填**（早于串台比对），故「切走之后回话失败」仍回框里，
  这条口径没有变。
- ⑦ **「发话就是一次回底」改挂 `talk.sending` 的上升沿**（决策 301）：排水环自己发出去的
  那一句也是人打的字，同样该看得见，而页面已经不知道「谁发的」。两处刻意的形状：
  (a) 做成上升沿（`if (talk.sending) following = true;`）而**不是**「在发就一直跟」——
  后者会在流式期间把上滑读历史的人每一段增量都拽回底部；(b) 滚动跟随那条效果在
  `tick().then()` 里才读 `following`，故这一格与本效果的先后不影响结论（e2e
  「滚动跟随」那条「自己发话必回底」照绿）。
- ⑧ **e2e 的判据为什么必须在页面之外**：回页之后再断言「那句话发出去了」是分不出新旧
  行为的——旧行为等人回到这一页也会补发。故用例人还在看板上时就 `fetch` 后端台账要那一行
  （`ledgerHasUserLine`）：它出现在人回到对讲台**之前**，才证明排水发生在页面之外。
  这也是本用例决定性的理由——页面卸载期间没有任何别的代码路径会调 `sendTurn`
  （`takeQueued` 的唯一调用点是 store 的排水环）。
- ⑨ **扫描面跟着判据走**（`delegation-scan.test.ts`，新增一组「发送编排只有一处」）：
  两处旧断言从页面搬到 store（超时判据、落地哨收口入口 `this.syncFollowing(session)`），
  另加四颗新牙——`claim()` 有唯一定义 / 页面里不剩 `talk.sessionId !==` / 旧拼法
  （`sessionId !== wanted`、`gen !== this.sessionId`）已退场 / `inFlight` 有唯一定义且
  入队与出队都读它。**已知边界**：反向牙齿只钉住那一种拼法，更宽的
  `/sessionId\s*!==/` 会误伤票 03 那处 `liveSid !== untrack(() => talk.sessionId)`。
- ⑩ **文档**：`design/frontend-design.md` §12.3 两行改口径（过期回包那一行补上发送三处
  走 `claim()`；排队发送那一行把「在屏页面的出队效果」改成 store 的排水环 + 关页照发 +
  `submit` / `sendTurn` / `unsentText`）；决策 354 那一行补了本票的落地记录、② 的显式
  修订与两处搬家副作用。
- ⑪ **验证**（2026-09-30）：前端单测 **1085 绿 / 83 文件**（`talk.test.ts` 39 条，其中
  发送编排一组 10 条；`delegation-scan.test.ts` 27 条）；`npx svelte-check --tsconfig
  ./tsconfig.json` **0 错 0 警**；e2e `e2e/talk.spec.ts` 全套 **43 条绿**（新增「关页排队
  的话照发」，含既有验收线 ㉖㉗㉘ 与「滚动跟随 / 收口接力」两组）。代码审两轴
  （Standards / Spec）跑过，发现已逐条处理。
- ⑫ **本票不动的东西**：在飞轮渲染键 `'live'` → `m<ledger_id>` 与折叠态搬运机的四件
  （票 03，页面里 `liveSeen` / `liveSid` 那一对与它那段 effect 原样保留）、后端
  `page_limit` 回显（票 04）、watch 双人格拆分与三套滚动跟随的合并（决策 354 明确不做）。
  停钮（`stopAsked` / `stopError` / `stopState`）也留在页面：它是「人在按停」的状态机，
  不随发送搬走，只是改读 store 的 `sendingSid`。
