# 从对讲台抽出「判断」，接线与版面留下（talk-judgments）

**状态（2026-09-24）：票 01–06 全部落地（`Status: done`）。** 票 04–05 是拷问中拆出去的独立项、
票 06 是票 04 交付时的裁决项（用户选「另立票」），均已随批实现。本目录来自一次架构评审
（`improve-codebase-architecture`）的候选 10，经两轮拷问定下范围与接缝。评审报告是临时的，
不入库；设计结论以本文件、决策 251 与决策 259（票 04 的裁决）为准。

同一轮评审的候选 1 / 2 / 3 / 4 / 5 / 7 已由决策 245–250 落地（`b7107bb`→`72bb715`），
**候选 7（值班长工具清单）在那批里已被决策 247 顺带解决**——`lib/toolLabels.ts` 从后端取标签，
Talk 那张 18 键的 `TOOL_LABELS` 已删。本目录只处理**剩下的候选 10**。

## 一、问题

`frontend/src/routes/Talk.svelte` 2929 行（`<script>` 1–1264、模板 1266–1945、样式 1951–2929），
是 `design/frontend-design.md` §12.3 行为映射表（85 行）里被引用**最多**的文件（**20 行**，
第二名 `TopBar.svelte` 与 `TaskDetail.svelte` 各 7 行），也是近 150 次提交里改了 12 次的那个。它同时住着**判断**（纯逻辑）、
**接线**（API / router / localStorage / SSE）与**版面**（978 行 CSS）。

判断侧已经有相当一部分抽出去了，而且抽出去的那些都有测试：

| 已经抽出的（有测试） | 落点 |
|---|---|
| `seedSeen` / `markSeen` / `pruneSeen` / `isFresh` / `sessionMark` / `load–saveSeen` | `lib/talkSessions.ts` |
| `isFoldable` / `openStop` / `toggleOpenStop` / `stopActionCount` | `lib/talkStops.ts` |
| 提议的全部判据（过期、可执行、只指路、状态词） | `lib/proposals.ts` |
| 流式归约（增量 / 工具 / 收尾 / 断流 / 失败归属） | `realtime/foreman.ts` |
| 回车提交护栏 | `lib/enterToSend.ts` |
| 工具标签查表 | `lib/toolLabels.ts` |

**本目录要处理的是剩下那三块没有模块、也没有测试的判断**，加上一处「已有模块但没人调」的
重复：

1. **回合分类与排序**（`turns` 派生，`403-513`）：把台账行分成 `mine` / `system` / `failed` /
   `console` / `fm`、把提议并进来、把流式中的三种在飞态插进来，最后**排序**（同刻按 `rank`
   兜底）。这三件事一件都没有单测。
2. **键盘 / 焦点陷阱**（`923-1003`）：与 `components/layout/TopBar.svelte:103-185` 是**同一套
   控制流换了标识符**。TopBar 那份有单测（`TopBar.test.ts:85-135`，通过渲染组件、在 `window`
   上派发事件、断言 `document.activeElement`），**Talk 这份一条单测都没有**——只有
   `e2e/talk.spec.ts` 里 4 条，且覆盖不到 ArrowUp 回触发钮 / Home / End / 绕回。
3. **工位灯聚合**（`crew` 派生，`306-320`）：`lib/pipeline.ts:487 aggregateStationState` 是这件事
   的现成 helper，**零消费者、零测试**；`BoardColumn.svelte:59-61` 又自己推了一遍；Talk 推的是
   **第三遍**。

## 二、三处真实缺口（都有现场证据）

### 1. 工位灯：三份实现、两套词表、两种优先序，且 Talk 少了失败分支

| 位置 | 优先序 | 有失败分支吗 |
|---|---|---|
| `BoardColumn.svelte:59-61`（看板列头，用户看得见、e2e 钉着） | pending → **running → failed** → done → idle | ✅ 红 |
| `lib/pipeline.ts:487-493 aggregateStationState` | pending → **failed → running** → done → idle | ✅ 红 |
| `Talk.svelte:306-320`（值班板） | pending → running → done → idle | ❌ **并进 idle** |

**两种优先序不同**（看板是「在跑优先于失败」，helper 是「失败优先于在跑」），而**词表也不同**：
契约 `StationState`（`lib/pipeline.ts:286`）是 `'done' | 'go' | 'warn' | 'stop' | 'idle' | 'dev' | 'test'`，
Talk 用的是 `'warn' | 'run' | 'done' | 'idle'`——**`'run'` 根本不在契约词表里**。

后果是用户可见的：**一个工位的任务全部失败时，值班板画的是空的灰灯框**（`.blamp` 不带变体），
而看板同一列是红的。而规格恰恰要求两者是同一份读数——
`design/theme-6-pixel.md:628` 明写值班板是「同一份读数在看板 8 列与顶栏灯带上各有一份，
**这是第三份**」，`:620` 把它归在**状态直陈**层（「后端确定性下发……也是最硬的信号」、
「不经 LLM 转手」）。

**不需要改契约**：`--stop` 已在 `theme/contract.ts:340`（`'--stop': '#FF6157'`，浅色 `#B3271E`）
与 `app.css:46` 存在，Talk 只需在自己 的 `<style>` 里加一条 `.blamp.x`。

### 2. 回合分类：失败轮的判据在 Talk 里被重写了一遍

`Talk.svelte:412` 内联写着 `m.role === 'system' && m.content.startsWith(FAILED_TURN_MARK)`，
而**同一条判据**在 `realtime/foreman.ts` 里已经存在两次（`failedLedgerRowIds():320`、
`ledgerOwnsTheFailure():338`）。这是同一份「这批轮次里哪些是失败轮」的判定散成三处。
`Talk.svelte:427` 的 `assistant + WATCH_MARK → proactive` 没有 lib 对应物。

排序这一块的风险最高、也最没测试：`409`（`rank`）与 `436`/`455`（`stamped.sort`）。
`:400-402` 的注释写明「同刻基本只出现在 `ManualClock` 的用例里，但排序必须是确定的
（否则每次渲染都可能换位）」——**这句话描述的正是它没有测试的那部分**。

### 3. 键盘陷阱：两份 88 行 / 84 行的同义实现

`Talk.svelte:923-1003` 的注释自己承认是照抄：「交互语汇**照抄**顶栏那个下拉（票 04 / R2-04
补的三条出口），**不新造第三套**」。两条注释都点名同一组出口：`aria-expanded` + `aria-controls`、
Escape 关得掉（**焦点没进过面板时也算**）、点面板外面关、上下方向键走项、`Home` / `End`、
面板常驻 DOM 用 `hidden` 开合、键盘一律在 `window` 上收。

陷阱逐条两边都有（Escape-未进焦点、ArrowUp 从 0 回触发钮、Home/End、绕回、点外关），
**唯一差异是禁用项跳过**：Talk 的选择器是 `button[data-menu-item]:not([disabled])`，
TopBar 是 `a.dd-item`（全是链接，没有禁用态）。这就是「照抄」的代价——一份实现携带的
陷阱会随另一份的演化而漂移，且没有任何测试能发现。

## 三、设计树（两轮拷问定稿）

**主目标**：以 **locality** 为纲——把一个判断收到一处；可测性是副产品。

**范围纪律（本批不做，各有自己的票）**：

- **只抽判断，不动模板**。不把 `crew` 面板 / 急停区 / 时间线拆成子组件。理由有实测支撑：
  本仓测**路由级组件**的代价很高——`routes/TaskDetail.test.ts:22-60` 需要 `vi.hoisted` 手搭
  整个 store 替身 + 两条 `vi.mock` + 约 50 行 fixture + jsdom polyfill。子组件只增组件面、
  不增可测性（`e2e/talk.spec.ts` 的 29 条已经在驱动真界面）。拆模板是**另一张票**。
- **配对判据不动**（票 04）。`needsPairing`（`521-523`）按报文字串 `'还没配对'` 分支，
  而 `:519-520` 的注释为这个选择辩护：403 在本应用里被**两处**用着（跨源防护，决策 128），
  只看状态码会把「Origin 不对」也挂上配对入口。查过生产者，`state.rs:400-407`
  的 `ApiError::forbidden` 设的是 `kind: None`——**今天确实只有报文可依**。要改得先给后端的
  配对 403 加机器可读的 `kind`，那是**后端裁决**，不该顺手塞进前端重构，更不该悄悄删掉那条注释。
- **动作身份不动**（票 05）。动作身份有**两种拼法**：`lib/actions.ts:115-123 actionKey()` 是
  四段规范形（`action:cursor:stage:node`），而 `stores/board.svelte.ts:114` 写的是
  `` `${taskId}:${action.action}` `` 两段、taskId 在前。消费者 `Talk.svelte:1530`、
  `TaskCard.svelte:62` 跟着生产者的拼法，`TaskCard.svelte:63` 还手工去桥接规范形。
  这是 store 契约，面比本批宽。

**接缝**：三块判断各建一个 `lib/` module，形状与既有兄弟一致（`lib/proposals.ts` 那样：
导出纯函数 + 一个「取数/查表」入口，没有类、没有 store）：

- `lib/talkTurns.ts` —— 回合分类与排序（票 02）
- `lib/menuTrap.ts` —— 键盘陷阱的纯判定 + 一个 `attachMenuTrap(node, opts)` 接线 helper（票 03）
- `lib/pipeline.ts` —— 工位灯聚合**不新建 module**，改 `aggregateStationState` 一处（票 01）

**键盘陷阱的做法（三选一里选 ii）**：把**纯判定**（下一个索引、绕回、Escape 该不该关、
该不该还焦点）**加上**一个 `attachMenuTrap(node, opts)` 普通 helper，两处都从 effect 里调。
选它而不是「只抽纯判定」：88 行里绝大部分是接线（查项、聚焦、挂 window 监听），只抽判定
等于把大头留在两处。选它而不是 Svelte `use:` action：**全仓一个 `use:` 都没有**
（`grep 'use:' frontend/src/` 零命中，也没有任何文档表态），本不该在一次去重里
首次引入一种团队从未用过、也从未写下的模式。

**对 TopBar 既有测试的硬约束**（`TopBar.test.ts:85-135` 是黑盒，抽对了它原样通过）：
① 监听必须留在 **`window`**（:91 就是 `fireEvent.keyDown(window, …)`，挂到 `document`
或元素上会让 jsdom 里的 window 事件收不到）；② `id="pending-dropdown"` / `a.dd-item` /
`aria-expanded` / `hidden` 四个 DOM 契约不许挪（:59-62 的 helper 与 :78-82 / :141-144 的断言
都钉着）；③「Escape 在焦点没进过面板时也关得掉」必须保持；④ ArrowDown-on-trigger 打开并送焦点
进第一项（:113 的 `waitFor` 对时序是宽容的）。

**为什么顺序是 01 → 02/03 且 01、03 无依赖**：三票动的是互不相交的代码（`crew` 派生、
`turns` 派生、两个陷阱块），**唯 02 有一处外部依赖**（见下）。**静态扫描守卫落在最后落地的那一票**
（见下），一次断言三处委派。

### 与本批之外的并行工作的重叠：决策 252（写完后发现）

另一批工作（`.scratch/mirror-contract/`，决策 **252–254**）裁定：值班长消息「这一行是什么」
**由后端给字段**——`message_wire` 增 `kind`（`"mine"` / `"console"` / `"failed"` / `"fm"`）
与 `proactive`，前端不再解析正文哨兵。**它改的正是本批票 02 的一半范围**
（`Talk.svelte:409-414,427` 与 `realtime/foreman.ts:320,338`）。

**让路方向是单向的**：252 给的是**后端形状**，前端无从替代，故 **252 先落、票 02 后落**，
票 02 的 `buildTurns` 输入类型吃 `m.kind` / `m.proactive`，不再自己 `startsWith`。
票 02 保留的是 252 不碰的**排序 / 提议合流 / 在飞三态**。详见票 02 顶部的警示框。

> 这条重叠是**评审报告的分栏造成的**：候选 8（前端手抄清单 / 类型面）与候选 10（对讲台判断）
> 都含「谁来判断这一行是什么」，而报告把它们分成了两张卡。若两票由同一人连着做，可合成一次改动。

## 四、五张票

| 票 | 内容 | 依赖 |
|---|---|---|
| [01](issues/01-station-lamp.md) | 工位灯：修 `aggregateStationState` 的优先序、Talk 改调它、补 `.blamp.x` + 失败行变体 | — |
| [02](issues/02-turn-kind.md) | 新建 `lib/talkTurns.ts`（排序 / 提议合流 / 在飞三态）；**行分类让给决策 252** | **决策 252**（`.scratch/mirror-contract/issues/01-foreman-turn-kind.md`） |
| [03](issues/03-menu-trap.md) | 新建 `lib/menuTrap.ts`，Talk 与 TopBar 两处接入 | — |
| [04](issues/04-pairing-kind.md) | 配对 403 加机器可读 `kind`，`needsPairing` 不再看报文字串（**后端裁决**） | — |
| [05](issues/05-action-identity.md) | 动作身份收成一种拼法（`board.svelte.ts` 的 2 段 vs `actionKey()` 的 4 段） | — |

**静态扫描守卫**（落在 01–03 里最后落地的那张票，Q3 的裁定）：照 `lib/talkLayout.test.ts`
的先例——`@vitest-environment node`、`readFileSync(resolve(process.cwd(), 'src/routes/Talk.svelte'))`、
正则扫描，断言 Talk 不再内联那三条判断、且确实引用了三个 module。
**没有这道守卫，单测只能证明新 module 对，证明不了 Talk 还在用它**——本仓已经被这个形状咬过：
`formatTokens` 至今在两处逐字重复（`lib/pipeline.ts:513` 与 `lib/format.ts:51`），
而 `lib/format.test.ts` 那趟「全站扫描重复的 locale 调用」没有覆盖到它。

## 五、行为变化只有一处

**票 01 是本批唯一改用户可见行为的一票**：值班板从此会对「全部失败」的工位点红灯
（今天画空灯框）。另有两处**行为中性**的一致性收敛：`aggregateStationState` 的优先序改为与
看板一致、Talk 的出参从 `'run'` 改回契约词表 `'go'`（模板里 `c.state === 'run'` 的
class 三元式同步改为 `'go'`）。

**这一处变化没有自动化见证**：`e2e/talk.spec.ts:203` 只断言 `.talk .blamp` 的**数量是 8**，
没断言任何一盏的颜色或变体。故票 01 要求**补一条能看见失败灯的用例**——否则这次改动的
唯一外部证据是手工目视。

**零风险的证据面**：`aggregateStationState` **零消费者**（全仓 grep 只命中定义），
所以改它的优先序不会牵动任何既有路径；`BoardColumn.svelte:59-61` **本票不动**（它已是
正确形状，且 e2e `pixel-theme.spec.ts:225-229` 在量它的灯箱尺寸）。

## 六、术语与文档

- **不需要新术语**：`StationState` 已在 `lib/pipeline.ts:286`，`--stop` 已在契约里。
  本批只把 Talk 拉回契约已有的词表，故 `docs/glossary.md` 不动。
- **`docs/decisions.md` 追加决策 251**（票 01 携带，本批一次记齐）。
- **`design/frontend-design.md` §12.3 的表行改动**（各票改自己那几行；**改表行前必须让新 module
  先落盘**——`lib/behavior-map.test.ts:142-159` 断言被引路径**存在**，而 `PENDING` 名单是空的）：
  - `:793`（失败轮渲染，现指 `Talk.svelte`）→ 加指 `frontend/src/lib/talkTurns.ts`（票 02）
  - `:795`（值守播报名牌，现指 `Talk.svelte`）→ 同上（票 02）
  - `:783`（折行档 ⋯ 菜单的键盘出口，现指 `Talk.svelte`）→ 加指 `frontend/src/lib/menuTrap.ts`（票 03）
  - `:819`（顶栏「待处理」下拉的键盘出口，现指 `TopBar.svelte`）→ 同上（票 03）
  - **新增一行**：值班板工位灯的聚合规则（**今天没有这一行**——表里指到值班板的只有 `:801`
    「换班次不重置看板派生的东西」与 `:788`「工位回执默认收起」，两条都不是灯色）（票 01）
  - 备注：§12.3 的表由 `lib/behavior-map.test.ts` 守着，但**它只断言被引的路径存在**
    （`behavior-map.test.ts:142-159`），不判断内容——故上面几行是**必须手改**的。
    行格式约束（`behavior-map.test.ts:100-129`）：相对仓库根、无反引号外的路径、无 glob / 锚点、
    备注列必须带编号。
- **两处计数器要一起改**：`docs/README.md:18` 与根 `AGENTS.md:5` 现在都写 `#1–250`，
  本批新增 251 后要写 `#1–251`。（`cursor-advance` 那批的 README 已记过这两处曾长期漂移。）
