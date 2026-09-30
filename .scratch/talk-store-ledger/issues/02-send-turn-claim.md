# 02: sendTurn 收编 + claim() 守卫 + 关页照排照发

**What to build:** 决策 354②——发送编排收成 `talk.sendTurn(text)`：begin → settle →
follow → 队列排水全进 store（store 已持有连接、哨兵定时器、队列）；串台守卫变
`talk.claim()`——捕获当前 sessionId + 台账代际，返回闭包，五种手写比对
（wanted / originSid / gen）全部换成一处调用。组件只留：输入框文本、失败时乐观回填
（「失败时不要清空输入框」）、渲染。**行为修正被接受**：页面关着队列也照排照发，
排水节奏照旧。

**Blocked by:** 01

**Status:** ready-for-agent

- [ ] `talk.sendTurn(text)`：含 sending 状态、pendingText、queue 排水、sentinel、
      follow（syncFollowing / followAfterGiveUp 收进 store 内部）
- [ ] `talk.claim()`：捕获 + 闭包比对；Talk.svelte 五处手写比对
      （613 / 626 / 686 / 1456 / 1464 / 1486 一带）逐处替换
- [ ] 队列排水 effect 搬进 store，页面挂载不再排水前置条件
- [ ] 单测：关页排队照发、失败路径（传输失败 vs 真失败）不清输入、
      串台守卫（claim 后换会话 → 丢弃）
- [ ] e2e 新增：关页排队、回来见到已发
