# 02: 回合分类与排序抽成 `lib/talkTurns.ts`——失败轮的判据不再重写第二遍

**What to build:** `Talk.svelte:403-513` 的 `turns` 派生读四个响应式输入（`session` /
`pendingText` / `sending` / `stream`）加 `needsPairing`，同时干三件事：把台账行分类
（`mine` / `system` / `failed` / `console` / `fm` / `proactive`）、把提议并进来、
把流式中的三种在飞态插进来，最后**排序**（`409` 的 `rank` + `436`/`455` 的 `stamped.sort`）。
本票把这三件事收进 `lib/talkTurns.ts`，一个 `buildTurns(input, now)` 吃一个平凡输入类型、
返回 `TurnView[]`；Talk 只把四个值传进去。

**为什么整体搬而不是只抽每行的谓词**：真正咬过人的 bug 在**合并与排序**里——`506` 那条
（配对链接只挂在其中一处）与「台账那一行更全，故本地那行退场」的归属判定，
都写在行的**相互关系**上，逐行谓词那条路**测不到**。`lib/proposals.ts` 也是这个形状：
判断归模块，模板只渲染。

**同时收掉一处重复**：`Talk.svelte:412` 内联写着
`m.role === 'system' && m.content.startsWith(FAILED_TURN_MARK)`，而**同一条判据**在
`realtime/foreman.ts` 已存在两次（`failedLedgerRowIds():320`、`ledgerOwnsTheFailure():338`）。
本票让 Talk **调 `failedLedgerRowIds`**，不重写。

> **⚠️ 与决策 252 的重叠（本票写完后发现的并行工作，必须先读这条）**
>
> 另一批工作（`.scratch/mirror-contract/`，决策 252）已经裁定：**「这一行是什么」由后端给字段**——
> `message_wire` 增 `kind`（`"mine"` / `"console"` / `"failed"` / `"fm"`）与 `proactive`（布尔），
> 前端不再解析正文哨兵；`realtime/foreman.ts:320,338` 的两处 `startsWith` 筛法与
> `Talk.svelte:409-414` 的 `role` 三元式、`:427` 的 `proactive` **都归它改**。
> 那份 ticket 是 `**Status:** ready-for-agent`、`Blocked by: None`，**尚未实施**。
>
> **故本票的范围必须收窄**（否则两票会同时改 `Talk.svelte:409-414,427` 与
> `realtime/foreman.ts:320,338` 并互相打架）：
>
> - **行分类不再由本票负责**——① 里那半（`role` + 哨兵 → kind、`assistant + WATCH_MARK → proactive）
>   **让给决策 252**；本票的 `buildTurns` **读 252 给出的 `m.kind` / `m.proactive`**，
>   不再自己 `startsWith`。
> - 本票**保留**的是 252 不碰的那些：**排序**（`rank` + `stamped.sort`）、**提议合流**（`:437-452`）、
>   **在飞三态**（`:457-511`，含 `partial` 谓词）、`thinking` 的 `trim() → null`、
>   `attribution` 的 `?? null` 透传。这些没有一处与 252 相交。
> - 上面「同时收掉一处重复」那条与 ① 里的哨兵半句，**改由决策 252 兑现**；`failedLedgerRowIds`
>   / `ledgerOwnsTheFailure` 在 252 之后改读 `kind`，本票不再动它们。
> - **执行顺序**：**252 先落，本票后落**。本票的 `buildTurns` 输入类型按 `m.kind` / `m.proactive`
>   定型，落笔前先确认 252 已改完 `api/types.ts` 的 `ForemanMessage`。
>
> 这条不是理想的排布（两票拆开了一个本来完整的判断），但 252 已落表、且它的三个字段是
> **后端形状**（前端无从替代），故让路方向是单向的。若两票由同一人连着做，可以合成一次改动。

**Blocked by:** 决策 252（`.scratch/mirror-contract/issues/01-foreman-turn-kind.md`）——
它给出 `kind` / `proactive` 两个字段，本票的输入类型要吃它们；252 未落前本票写不了输入类型

**Status:** done（2026-09-23 实现；决策 252 已先落，阻塞解除——闸门与 e2e 见交付说明）

- [x]新建 `frontend/src/lib/talkTurns.ts`：导出 `buildTurns(input, now)`（或等价签名）与它需要的输入类型；模块头 docblock 照本仓惯例点明决策号与它防的失败模式
- [x]搬进去的四件事（**行分类那半已让给决策 252**，见上方警示）：① 读 `m.kind` / `m.proactive` 定型（**不自己 `startsWith`**）、`thinking` 的 `trim() → null`、`attribution` 的 `?? null` 透传**保持不映射**（理由照抄 `:428-430`）② 提议合流（`:437-452`）③ 在飞三态（`pending` / `live` / `send-error`，含 `partial: !stream.streaming && stream.text.length > 0`）④ **排序**
- [x]排序也搬：同刻 `rank`（人的话 0 / 提议 1 / 值班长 2）与 `stamped.sort` 的稳定次序，**行为逐字保持**（`:400-402` 的注释说「排序必须是确定的」——这正是要测的地方）
- [x]**不碰** `Talk.svelte:412` 的 `failedLedgerRowIds` 调用与 `:427` 的 proactive 判定——它们归决策 252（252 之后这两处已读 `m.kind` / `m.proactive`）
- [x]`:521-523 needsPairing` **本票不搬**——它是票 04 的主题（后端 403 的 `kind` 裁决未定前，搬只是把同一处耦合换个位置）
- [x]`needsPairing(stream.error)` 的调用点（`:506`）随输入类型一并传入，**行为不变**
- [x]新增 `frontend/src/lib/talkTurns.test.ts`：分类各一条（含 `failed` **和** `console` 的判据边界）、提议合流一条、在飞三态各一条、`partial` 边界各一条、**排序两条**（同刻三类的次序确定；不同刻按时间）
- [x]**静态扫描守卫**（若 01 / 03 已落地，则本票是最后一票，守卫落在这里；否则留待最后一票）：照 `lib/talkLayout.test.ts` 先例（`@vitest-environment node` + `readFileSync(resolve(process.cwd(), 'src/routes/Talk.svelte'))` + 正则），断言 Talk **不再内联** `startsWith(FOREMAN_FAILED_TURN_MARK)` / `startsWith(FOREMAN_WATCH_MARK)`，且**确实 import 了** `talkTurns`
- [x]`design/frontend-design.md` §12.3 的 `:793`（失败轮渲染）与 `:795`（值守播报名牌）两行**加上 `frontend/src/lib/talkTurns.ts`**——注意顺序：**先让新 module 落盘，再改表行**，因为 `lib/behavior-map.test.ts:142-159` 会断言被引路径**存在**（`PENDING` 名单是空的，引了不存在的路径当场红）；表由它守着，但它只断言路径存在、不判断内容，故必须手改。备注列的决策号仍写原来的（决策 211④ / 209④），本票加的是实现位置
- [x]`make check` 绿（`cd frontend && npm test && npm run check && npm run build`）

**范围纪律**：模板不拆（不把时间线 / 急停区 / `crew` 拆成子组件）——理由与实测代价见
`README.md` 第三节。`turns` 之外的派生（`markersFor` / `receiptIsOpen` / `timelineEmpty` /
`placeholder` / `pendingKey`）**本票不动**：它们要么已经委派给有测试的模块，要么读的状态太碎。

## 交付说明（2026-09-23）

落地形状与票面的差异，逐条记：

1. **签名裁定为单参 `buildTurns(input)`**（票面写 `buildTurns(input, now)`，但括注允许「等价签名」）：
   现实现**没有时钟消费者**——排序按 `created_at` 的 RFC3339 字符串比较，提议过期态另有
   `lib/proposals.ts` 按 `now` 算。加一个没人读的 `now` 是死参，理由写进模块头 docblock 的
   「③ 不读时钟」。
2. **`needsPairing` 谓词留在 Talk、以回调传入**（按票面执行）：票面没提到的**第二处消费**证实了这个
   裁定——`Talk.svelte` 的 loadError 分支也调它，谓词搬进模块反而要把 loadError 的判定一起拖进来。
   扫描守卫钉死「判定形状 `.includes('还没配对')` 只在 Talk、module 只调回调」。
3. **`TurnView` 接口随 `buildTurns` 迁出**：Talk 删掉本地 interface 与已无消费者的
   `type ForemanLiveTool` 导入（`ForemanBriefing` / `ForemanTrace` / `ForemanProposal` 仍被
   回执与提议渲染用着，留）。
4. **排序算法逐字照搬**，测试按**实际行为**钉期望：同刻中档（console / failed / 提议）的次序是
   「消息先入、提议后压 + 稳定排序」——初版测试把提议当消息传进 `sessionOf` 第一参、且期望值
   写反了中档次序，红了 4 条后修正的是**测试**，模块一个字没动（它与原组件是同一段代码）。
5. **静态守卫按票面落在本票**（01 / 03 已先落地）：`lib/delegation-scan.test.ts` 新增 4 条——
   Talk 从 lib 取 `buildTurns`、不再内联 `startsWith(FOREMAN_…_MARK)` 哨兵、`stamped` / rank
   三元式归 module、配对谓词仍留 Talk。**变异验证**：摘掉回调参数 + 把 rank `0` 改 `3` →
   4 条红（2 扫描 + 2 单元），回滚后复绿。
6. **§12.3 两行（失败轮 :793 / 值守名牌 :795）在模块落盘后**加了 `frontend/src/lib/talkTurns.ts`
   （`behavior-map.test.ts` 的存在性断言不红）；备注列决策号按票面**保持原样**（211④ / 209④）。
7. **决策 251 的落地字段**同步：「`talkTurns.ts` 待票 02」→ 已落地。

闸门：单元 **63 文件 / 762 条**全绿、`svelte-check` **0 错误**、`npm run build` 绿、
`e2e-artifacts.sh` 新鲜度守卫过后 **talk e2e 30/30 全绿**（时间线真界面走的已是 `buildTurns`）。
