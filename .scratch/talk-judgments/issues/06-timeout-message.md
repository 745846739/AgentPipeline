# 06: 超时判定也收成 `kind`——`startsWith('请求超时')` 是票 04 同一反模式的兄弟

**What to build:** `realtime/foreman.ts::isTimeoutMessage(message)` 用
`message.startsWith('请求超时')` 认「本地等不到回包」，而那句话由
`api/client.ts::mapRequestError` 构造（`请求超时（N 秒没有回应）。`）。判据两边隔着模块、
靠**字符串逐字同步**——`api/client.ts:154-156` 明写「按 `kind` 分支，不按状态码、
**更不按 `message` 里的字样**」，这是全仓已知的第二处违反（第一处配对判据已由票 04 /
决策 259 清偿）。

**与票 04 完全同构，通道问题也在**：`failureNotice(message)` 与 `isTimeoutMessage` 吃的都是
**降级成字符串之后**的错误——`kind` 若不随错误一起走到构造点，就得像票 04 那样把判据提到
`ApiError` 还在手的那层。可行形状（照票 04 抄）：

- `mapRequestError` 里 `new ApiError(0, 超时那句)` **带 `kind: "request_timeout"`**（后端不参与
  ——超时是前端本地 `AbortSignal.timeout` 的产物，没有 HTTP 应答体）；
- `Talk` 发话 catch 里与 `sendPairingNeeded` 并排判一枚 `sendTimeoutNeeded`，或更收拢一点：
  把「失败轮附不附『仍在继续』那句」改成布尔输入（`failureNotice` 的字符串拼接留在下游，
  **判**上移）；
- `foreman.ts::failureNotice` / `isTimeoutMessage` 随之退役或改为吃布尔。

**Blocked by:** None（票 04 已落地，通道形状可直接照抄；注意 `sendPairingNeeded` /
`loadErrorPairing` 两枚布尔旁边会再多一枚——三枚并排若显臃肿，可顺手收拢成一个小的
失败分类对象，那属于本票的实现裁量）

**Status:** done（2026-09-24 实现；决策 259 的延伸、无新号——见交付说明）

- [x] `mapRequestError` 的超时分支带 `kind`（常量，照 `KIND_PAIRING_REQUIRED` 的命名姿态）
- [x] 判据上移到 `ApiError` 还在手的 catch 层；`isTimeoutMessage` 的 `startsWith` 退役
- [x] 补测：超时 → 附「仍在继续」句；网络不通（`kind` 不是超时）→ 不附；
      **字样 spoof**（报文正文以「请求超时」开头但 kind 不是）→ 不附
- [x] `delegation-scan` 守卫：`realtime/foreman.ts` 无 `startsWith('请求超时')`
- [x] 若产出新裁决，`docs/decisions.md` 追加（否则在交付说明里记明「决策 259 的延伸、无新号」）

**来源：** 票 04 交付时的裁决项（用户 2026-09-23 选「另立票 06」）；反模式点名见票 04 票面
「无论选哪个」那一条。

## 交付说明（2026-09-24）

**无新决策号**：本票是**决策 259 的延伸**——259 已立「按 `kind` 分支、不按报文字样」这条规则
并点名 `isTimeoutMessage` 是同一反模式的兄弟（用户裁决另立票），本票照它落地，不产生新裁决。

实现要点：

1. **生产侧**：`api/client.ts` 新增导出常量 `KIND_REQUEST_TIMEOUT = 'request_timeout'`，
   `mapRequestError` 的超时分支把它作为 `ApiError` 第三参带上（后端不参与——超时是前端本地
   `AbortSignal.timeout` 的产物，没有 HTTP 应答体）。网络不通那支不带 kind（测试钉住
   `kind === undefined`）。
2. **判据**：票面两枚布尔（`sendTimeoutNeeded`）**没加**，取票面「更收拢一点」的那条路——
   `realtime/foreman.ts` 新增 `isRequestTimeout(err)`（`instanceof ApiError && kind` 姿态，
   照 `sharePairing.ts::isPairingRequired`），`failureNotice(message)` 改为
   `failureNotice(message, timedOut)`：**判上移（趁 ApiError 在手）、字符串拼接留下游**。
   `isTimeoutMessage` 整个退役。全仓 `failureNotice` 只有 Talk 发话 catch 一处消费者，
   故没有出现票面预警的「三枚并排」。
3. **Talk**：发话 catch 里 `failureNotice((err as Error).message, isRequestTimeout(err))`，
   与 `sendPairingNeeded` 并排、同在 `ApiError` 还在手的那一层；注释同步为「配对与超时两枚判据」。
4. **测试**：`foreman.test.ts` 的超时用例重写——走 `mapRequestError` 真构造链（钉「两端共用
   同一枚 kind」，不手写字面量），三条即票面要求的超时→附句 / 网络不通→不附 / **字样 spoof→不附**；
   `client.test.ts` 两条补 kind 断言；`delegation-scan` 守卫两断言（foreman.ts 剥块注释后无
   `startsWith('请求超时')`、无 `isTimeoutMessage`、判据钉 kind；Talk 调用形状钉
   `failureNotice(msg, isRequestTimeout(err))` + import 指向 realtime/foreman）。
   守卫首跑就拦下了自己 docblock 里对旧形状的叙述引用——剥注释再扫，与配对那条同款处理。
5. **行为映射表**：`design/frontend-design.md` §12.3 的超时那一行（原 :794）备注从
   `isTimeoutMessage` 换成 `isRequestTimeout`、加 `frontend/src/api/client.ts` 一列。
6. **文档**：`docs/decisions.md` 不追加（无新号）；`AGENTS.md` / `docs/README.md` 的
   `#1–259` 计数不变。
