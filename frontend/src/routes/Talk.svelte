<script lang="ts">
  import { onMount, tick, untrack } from 'svelte';
  import type {
    AllowedAction,
    ForemanBriefing,
    ForemanProposal,
    ForemanSessionMeta,
    Stage,
  } from '../api/types';
  import type { SpriteName } from '../theme/contract';
  import { createMenuTrap } from '../lib/menuTrap';
  import { chipRow } from '../lib/sessionChips';
  import {
    cancelForemanTurn,
    executeForemanProposal,
    rejectForemanProposal,
    renameForemanSession,
  } from '../api/client';
  import { board } from '../stores/board.svelte';
  import {
    BOARD_COLUMNS,
    COLUMN_SPRITES,
    aggregateStationState,
    formatDuration,
    formatTokens,
    pendingLabel,
    taskDuration,
    type StationState,
  } from '../lib/pipeline';
  import {
    isFoldable,
    isStopOpen,
    resolveOpenStop,
    stopActionCount,
    toggleOpenStop,
  } from '../lib/talkStops';
  import {
    isRepairProposal,
    proposalActionable,
    proposalPointerOnly,
    repairActionLabel,
    repairGateLabel,
    proposalRemainingLabel,
    proposalShortLabel,
    proposalState,
    proposalStateLabel,
    proposalTaskId,
    proposalToolLabel,
  } from '../lib/proposals';
  import { CompositionGuard, shouldSubmitOnEnter } from '../lib/enterToSend';
  import { growTextarea } from '../lib/talkDock';
  import { stopButtonLabel, stopButtonState } from '../lib/stopButton';
  import { labelFor, loadToolLabels } from '../lib/toolLabels';
  import { attentionKindSummary, visibleAttention } from '../lib/attentionKind';
  import { formatDateTime } from '../lib/format';
  import {
    TALK_FOLD_QUERY,
    TALK_MOBILE_QUERY,
  } from '../lib/talkLayout';
  import {
    loadSessionId,
    sessionMark,
    type SessionMark,
  } from '../lib/talkSessions';
  import {
    buildTurns,
    prettyArgs,
    thinkTicker,
    turnName,
    watchDraftExcerpt,
    type TurnStep,
    type TurnView,
  } from '../lib/talkTurns';
  import { isPairingRequired } from '../lib/sharePairing';
  import { maskPairValue, pairingLaunchNote } from '../lib/pairingLaunch';
  import { getPairingToken } from '../api/config';
  import { actionKey } from '../lib/actions';
  import {
    foreignIsReplying,
    ledgerOwnsTheFailure,
    pruneForeignActive,
  } from '../realtime/foreman';
  import { talk } from '../stores/talk.svelte';
  import Sprite from '../components/render/Sprite.svelte';
  import Gauge from '../components/render/Gauge.svelte';
  import MarkdownView from '../components/render/MarkdownView.svelte';
  import PendingActions from '../components/board/PendingActions.svelte';
  import DiffReviewPanel from '../components/task/DiffReviewPanel.svelte';
  import EmptyState from '../components/ui/EmptyState.svelte';
  import Fold from '../components/ui/Fold.svelte';
  import Modal from '../components/ui/Modal.svelte';
  import { router } from '../router.svelte';

  /**
   * 值守台账档（票 04 / 决策 286）：`<Talk watch />` 渲染**只读的一本账**——同一套
   * 时间线，取数换到 `?kind=watch`，坞与一切「说话 / 动手」的口全部收起。prop 名刻意
   * 与路由同名（`/talk/watch`）：它们是同一件事的两半，不是两套配置。
   *
   * 值守账上**唯一**的动作是「转去对话」：它不在这本账里动手，去处（写口）在人的对讲台。
   */
  let { watch: watchMode = false }: { watch?: boolean } = $props();

  /**
   * 对讲台（theme-6-pixel.md §3.3；决策 174 / 182 / 183 / 192）。版面**三分区**（票 04）：
   * 状态区（当前急停）／对话时间线（值班长的话、值班经理的话、工位回执）／输入坞。
   * 值班板是**独立的一块**——桌面右栏、窄屏收成时间线之上的横向灯条，不在状态区里。
   *
   * **宽屏整页钉在视口内，时间线是唯一会滚的区域**：一个两小时前挂起的急停被对话顶出视野
   * 是本页最不能出的错，故不靠 sticky 逐段救，而是把「会长的部分」与「不能动的部分」
   * 放在两个不同的滚动容器里。
   *
   * **窄档（≤899px）反过来：整页随手指滚，钉住的是三样**（决策 192，由决策 218 修订 ④
   * 把两只改成三只）——页头那一行（46px 带子，`top: var(--topbar-h)`）、急停摘要条（钉在它
   * 下沿）、输入坞（钉在底栏上沿）。同一个不变式（急停不滚出视野）在手机上由「钉住」完成：
   * 钉住的东西必须便宜，故**折行档连单张急停也折成一行摘要条**（`forceFold`），
   * 一张 376px 的展开轮钉在 900px 的屏幕上等于把整块屏幕钉死。对话区因此拿到的是
   * 「整块屏幕减去三条钉住物」，而不是钉住物之间的残渣。
   *
   * **为什么页头也要钉**（决策 218 当日修订 ④）：班次动作（含开新对话）收进页头右端的 ⋯
   * 之后，**页头是它们唯一的家**——不钉的话，长会话里要滚回顶部才够得着，那正是用户最初
   * 诉求指的毛病。代价是对话区 46px，换来全菜单恒在手边。
   *
   * **档位的分界是 899**（`lib/talkLayout.ts` 一处定义、三处用：折行版面 / ⋯ 与 `forceFold` /
   * 页头形态）。桌面（≥900）仍是三分区 + 单张展开，两侧的行为差异是刻意的（两种滚动模型）。
   *
   * **但状态区自己也有上限**（桌面 46vh 与「先留给时间线的那一份」两项取小，
   * 超高时区内滚——后者是决策 208：矮窗口里不让它把时间线压成一条缝），而一张急停轮内联着
   * 后端下发的动作集，最高的形状（带补充输入的 resume）约 330px——13″ 上可用只有约
   * 202px（决策 208 之前是 344px），**展开一张就已经占满整个区**。故急停轮折叠（决策 183）：**只有一张时不动**
   * （单急停版面与折叠前一致），两张以上**一张都不展开**、全部收成一行约 36px 的摘要条；
   * 人点「展开恢复动作」才展开那一张，且同时只展开一张（判据在 `lib/talkStops.ts`）。
   * 窄屏这一档不另写判据，只把 `forceFold` 传成真（决策 192）。
   *
   * **对面是真的会说话的值班长**（`/foreman/session` + `/foreman/messages` + `/foreman/stream`）：
   * 对话不依赖任务——空看板（无项目无任务）也照样能问它话。
   *
   * **一条长会话改成一排班次**（决策 204）。落点由决策 218 ② 改了两处：**桌面**把那一排
   * 挂在**页头右端**（`nowrap` + 容器内横滚）——它原先长在 `.timeline` 这个滚动容器**里面**，
   * 于是随对话上移（实测滚到底时它在屏幕上方 320.2px，这正是「新建对话往上翻很久」的病根）；
   * **折行档**整排收进页头右端的 **⋯ 菜单**（含「+ 新班次」/ 切换 / 改名 / 归档），
   * 桌面上可见的那一排因此有了第三处约束：页头高度与 `--talk-chrome` 都不许因它变
   * （挂在既有那一行的右端、不新增行，桌面因此白得 36px）。
   * 会话隔离的是**对话上下文**（这一屏读什么、合计多少），**不是权限、也不是态势快照**
   * ——「换会话 ≠ 换看板」。班次进地址（`?session=` + `agentpipeline.talk_session` 兜底，
   * 决策 217①）。
   *
   * **回话中允许换班次**（决策 220②）：那把 `sending || busy` 的 UI 锁撤掉了——它只是第三层
   * 自保（前两层是 `appendForemanEvent` 的班次守卫与 `send()` 里的 `talk.sessionId` 比对，票 01）。
   * 「那一轮回话去哪了」改由班次列表里的两枚标记说：**正在回话**与**有新动静**
   * （判据在 `lib/talkSessions.ts` 与 `realtime/foreman.ts`）。
   *
   * **值班长的回复里永远没有按钮**：按钮有三个落点、都不在回复里——状态区的急停轮
   * （后端下发的 `allowed_actions`，决策 101）、提议轮里操作台内联的确认钮（决策 207③）
   * 与提问轮的选项钮（决策 265）。
   * 同一个动作在两处各渲染一颗钮，会让「哪个是真的」
   * 变成使用者必须思考的问题。（空态原先还有一颗页面固定的导航钮「去看板新建任务」，
   * 决策 240 随看板页签一并摘除——它不进动作契约，去掉也不动这条边界。）
   *
   * **全站唯一的响仍只在急停一处**：值班长一律收在 `--pane`，不占琥珀档；发送失败走
   * 时间线里的一轮（红），不弹窗、不 toast。
   *
   * **不造聊天组件**：一列对话 = 一叠操作台对话框（`.turn` 即对话框本体，压在框沿上的
   * 名牌 tab = 发言者），故不另加 who 行，也不做左右交替气泡。
   */

  /**
   * 台账生命周期（决策 354①）：`session` / `sessionList` / `loading` / `loadError` /
   * `hasMoreEarlier` 与 reload / loadEarlier / switchTo / 归档坠落 / seen 标记全住在
   * `stores/talk.svelte.ts`——「某一班的事发生在页面之外」（收尾、归档、切走之后落地的
   * 回话）正是 store 存在的理由。这里只是**读别名**：写走 store 的方法。
   */
  const loading = $derived(talk.loading);
  const loadError = $derived(talk.loadError);
  /** `loadError` 是不是**配对缺失**（按后端给的 `kind` 判，决策 259）——挂不挂配对入口读它，不读报文字样。 */
  const loadErrorPairing = $derived(talk.loadErrorPairing);
  /**
   * 这个组件**装载那一刻**的地址（决策 285）。standalone 窗口没有地址栏，而主屏图标的
   * 启动地址决定令牌能不能递进来（191）——「图标没带上参数」这句话在界面上本该有处可读。
   * 取装载时那一刻而不是渲染时：渲染时地址可能已经被页内导航改过，而要看的是启动那条。
   */
  const launchHref = typeof window === 'undefined' ? '' : window.location.href;
  /** 会话台账（时间线的权威内容；每次回话后重取，不自攒一份账）。 */
  const session = $derived(talk.session);
  /** 未归档的班次（chip 行的数据源），按最近活动倒序。 */
  const sessionList = $derived(talk.sessionList);
  /**
   * 未消费待办的**只读**读数（决策 307，票 executor-never-returns 06）。
   *
   * `null` = 还没读到 / 读不到——那时**不渲染**页头那枚读数（未接线、离线都走这一支）。
   * 它与 `session.turn_in_flight` 无关，这正是它存在的一半理由：值守轮排队时（决策 289）
   * 那条「值守台账 · 正在跑」的 crumb 根本不出现，而系统此刻正在报警。
   * 与班次同一次重读刷一遍（`reload` 尾部，决策 354① 起在 store）。
   */
  const attention = $derived(talk.attention);
  /**
   * 页头那枚读数的渲染条件：**有未消费待办才渲染**（0 条不占窄档空间——这是一条决策，
   * 不是顺手写的缺省值）。判据本体在 `visibleAttention` 里，由 `attentionKind.test.ts` 钉住。
   */
  const openAttention = $derived(visibleAttention(attention));
  /** 类别摘要进 `title`（一行几何不许被撑开）：条数是读数，明细是悬停时看的。 */
  const attentionTitle = $derived.by(() => {
    if (!openAttention) return '';
    const kinds = attentionKindSummary(openAttention.by_kind);
    const stuck = openAttention.blocked_reads.stuck_now;
    return [kinds, stuck > 0 ? `有 ${stuck} 个文件读卡着` : ''].filter(Boolean).join('；');
  });
  /**
   * 「显示已归档」开关（票 06）：关 = 现状（chip 行只见活跃班次）。只换**列表给谁看**，
   * 不动当前在读的那一班；不持久化——它是一次浏览动作，不是身份（与「地址记住落点」
   * 那套不是一回事），下次进本页回到关（onMount 拨回，store 寿命比页面长）。
   */
  const showArchived = $derived(talk.showArchived);
  /**
   * 当前班次 id。**住在 `stores/talk.svelte.ts` 里**（决策 275）：在飞一轮的现场与那条
   * `/foreman/stream` 连接都挂在它上面，而它们必须活得比这个组件长——否则切一下界面再回来，
   * 本轮已经收到的输出整段不见（那一轮还在服务端跑，SSE 没有回放，补不回来）。
   * 这里只是一个**读别名**：写走 `talk.watch(id)`（它同时管换班要清的那几样）。
   */
  const currentId = $derived(talk.sessionId);
  /**
   * 「在途回包是谁的」不再有组件本地记号（票 01 of talk-live-identity，显式修订决策
   * 204⑥ 的实现载体的那一半，语义不变）：原先的 `generation` 随组件销毁重建，而**在飞的
   * `send()` 活得过换页**——回来后 generation 归零，回包被误判成「切走了」，现场整段
   * 倒掉、已发的话永远留在输入框里。现在裁判只有一位：store 的 `sessionId`。每个 await
   * 之后比对「发起时认下的目标 ≠ `talk.sessionId` 即丢弃」，`reload()` 与 `send()` 同一条纪律。
   */
  /** 班次操作（改名 / 归档）对话框；`null` = 关着。 */
  let dialog = $state<'rename' | 'archive' | null>(null);
  /** 改名输入框的草稿。 */
  let titleDraft = $state('');
  /** 班次操作的进行中标记（按钮转圈 + 禁用）。 */
  let busy = $state(false);
  /** 班次操作的失败说明（对话框里就地显示，不弹窗）。 */
  let dialogError = $state<string | null>(null);

  let input = $state('');
  /** 发送在途 / 乐观轮 / 在飞步骤 / 跟一轮的锚点：全在 store 里（决策 275，见 `currentId`）。 */
  const sending = $derived(talk.sending);
  const pendingText = $derived(talk.pendingText);
  const stream = $derived(talk.stream);
  const followingSince = $derived(talk.followingSince);
  /** 当前这条流错误是不是**配对缺失**（按 `kind` 判，决策 259）：与 `stream.error` 同处设置。 */
  const sendPairingNeeded = $derived(talk.pairingNeeded);
  const streamStatus = $derived(talk.status);
  /**
   * **这一趟发送之前**台账里已有的失败行 id（决策 337）。
   *
   * 判「这次失败是不是已经由台账那一行接管了」要的正是这个差集：不带它的话，一次早先的失败
   * 会让此后每一次真实断网（请求根本没到后端、台账不会多出任何行）都静默下来——而那种情况
   * 恰恰是本地那条失败轮存在的理由。自决策 354② 起由 store 在**发出去的那一刻**记
   * （`sendTurn` 开头）：记的地方得跟着发送走，而发送已经搬进了 store。这里只是读别名。
   */
  const failuresBefore = $derived(talk.failureBaseline);
  /**
   * 这次失败已被台账那一行接管（决策 337）——**判在渲染上**，不是收尾那一刻判一次。
   *
   * 两条来源（后端当场落的台账行、本地那条传输层报文）说的是一件事，谁先到都合理；只在收尾时
   * 判一次会给出一条真实的共存窗：重取回包一落地，时间线上同时摆着两轮「发送失败」，等下一次
   * 判据跑过才收掉（2026-09-30 实测：e2e 20 次里红 2 次撞的就是这个窗）。判据挂在渲染的输入上，
   * 窗从形状上不存在。
   */
  const ledgerOwnsFailure = $derived(
    ledgerOwnsTheFailure(session?.messages ?? [], failuresBefore),
  );
  /**
   * 本机刚发出去、还没落地的那一班（决策 220③ 的「正在回话」前半支）——住在 store
   * （决策 354②）：发送编排搬走之后，「刚发出去的是哪一班」活得过页面（队列在关着页的
   * 时候也会发出去）。这里只是读别名。
   */
  const sendingSid = $derived(talk.sendingSid);

  /**
   * 本机已经按过停、那一轮还没收口（决策 294 / 票 09）。
   *
   * 它只喂停钮自己那一个「正在停…」态：按下之后那一轮还要把结论与半成品落库（协作式收口，
   * 决策 226 的同一句话），人在等的那几秒里该看得见「按下去了」。**在飞一结束它就失去意义**
   * ——态由 `stopButtonState` 的 `inFlight` 归零（`session.turn_in_flight` 翻假 / 本机这一趟
   * 的回包到了），故这里不做显式重置：多一处重置就多一处忘记重置的地方。
   */
  let stopAsked = $state(false);
  /** 上一次按停**没送到**的原因（就地显示在坞里；成功那一趟与一轮收口后都清空）。 */
  let stopError = $state<string | null>(null);
  /**
   * 这一班此刻有一轮在飞吗（决策 260 的读数 + 本机这一趟）。
   *
   * 两个来源都要：`sending` 管「我刚发出去的那一趟」（服务端登记之前有一段空隙），
   * `turn_in_flight` 管「刷新 / 换设备之后它仍在跑」——只认前者的话刷新页面就没法停。
   * 值守轮不在此列：它在**另一本条账**上（`watchMode` 下整个输入坞都不渲染）。
   */
  const turnRunning = $derived(sending || Boolean(session?.turn_in_flight));
  const stopState = $derived(stopButtonState({ inFlight: turnRunning, asked: stopAsked }));
  // 这一轮收口之后把停钮那两样一起归零：`stopAsked` 的失效判据本来就在 `stopButtonState`
  // 里（`inFlight` 一假就 hidden，故它不影响下一个态），但 `stopError` 没有那样的判据——
  // 不在这儿清，它会在**下一轮**一开始就浮出来，说一件与这一轮无关的事。
  $effect(() => {
    if (!turnRunning) {
      stopAsked = false;
      stopError = null;
    }
  });

  /** 「哪一条我看过」的时刻表（决策 220③ 的「有新动静」判据）——住在 store（决策 354①）。 */
  const seen = $derived(talk.seen);
  /** 「别的班次正在回话」（store 持有，决策 275——它说的是「此刻」，与页面在不在无关）。 */
  const foreignActive = $derived(talk.foreign);
  /** 标记的时钟：只在真有「别的班次在回话」时走（静默超时熄灭用）。 */
  let markerNow = $state(Date.now());

  /** ⋯ 班次菜单（折行档；桌面保留可见的班次行，决策 218 Q14）。 */
  let menuOpen = $state(false);
  let menuWrap = $state<HTMLDivElement | null>(null);
  let menuTrigger = $state<HTMLButtonElement | null>(null);
  let menuPanel = $state<HTMLDivElement | null>(null);

  /**
   * 工位回执的展开态，按 `turn.key` 记（票 06）。
   *
   * **必须受控**：`<details open>` 让浏览器自己管的话，流式增量反复重渲染同一轮时，
   * 使用者手动展开的那一轮会被打回收起——这正是决策 218② 这条最容易写坏的地方。
   * 没记过的 key 走默认：折行档收起、桌面照旧展开（决策 182 的纪律只在折行档反转）。
   * 跨刷新不记忆（与决策 217 的折叠态口径一致：折叠态不进 URL / localStorage）。
   *
   * **三张表自票 03 of talk-live-identity 起住在 store**（`talk.receiptOpen` 等）：
   * 展开态描述的是**那一轮**、不是这一屏——组件作用域的话，切页回来全被重置成默认态
   * （「回来方式就不对」的另一半）。键随轮稳定：台账轮锚 `m<id>`；在飞轮拼到半截行时
   * 渲染键就是行 id，而本机发送那一趟自决策 354③ 起在首条带 `ledger_id` 的事件到达后
   * 同样换成行 id——收口后台账那一行同键接管，不需要搬运。跨页面存活、不跨刷新。
   */
  /** 工具回执标签（`GET /foreman/tools`，取数一次缓存，决策 247⑤）。空表 = 还没回来，原样显示工具名。 */
  let toolLabels = $state<Record<string, string>>({});
  const receiptIsOpen = (key: string) => talk.receiptOpen[key] ?? !folded;
  const thinkingIsOpen = (key: string) => talk.thinkingOpen[key] ?? false;
  const toolIsOpen = (key: string) => talk.toolOpen[key] ?? false;

  function toggleThinking(e: MouseEvent, key: string) {
    // 阻止默认的 `open` 翻转，改由状态说了算（与 `toggleReceipt` 同一手法）。
    e.preventDefault();
    talk.thinkingOpen = { ...talk.thinkingOpen, [key]: !thinkingIsOpen(key) };
  }

  function toggleTool(e: MouseEvent, key: string) {
    // 与 `toggleThinking` / `toggleReceipt` 同一手法：默认的 `open` 翻转由状态接管。
    e.preventDefault();
    talk.toolOpen = { ...talk.toolOpen, [key]: !toolIsOpen(key) };
  }

  /**
   * 修复提议那块补丁（票 12）的展开态：**默认收起**，故不走 `receiptIsOpen` 那张表
   * （它的缺省是「桌面展开、折行档收起」，那是工位回执的规矩）。受控的理由与其余折叠
   * 逐字相同（决策 218② / 301）：提议轮会随流式反复重渲染，人不该被自己点开的那一块
   * 打回。键是提议 id——它在这一屏里唯一。
   */
  let patchOpen = $state<Record<string, boolean>>({});

  function togglePatch(e: MouseEvent, id: string) {
    e.preventDefault();
    patchOpen = { ...patchOpen, [id]: !(patchOpen[id] ?? false) };
  }

  /** 每个 pending 任务的详情（allowed_actions 只在详情里下发，决策 101）——住在 store（决策 354①）。 */
  const details = $derived(talk.details);

  /**
   * 折行档（≤899，决策 218 修订 ⑥）：`.talk` 折成一列、页头收成一行 + ⋯、班次行与值班板
   * 灯条不渲染、工位回执默认收起、`forceFold` 传真。**这是版面判据的唯一一处**
   * （`lib/talkLayout.ts` 里那个常量，与 CSS 那条 `@media` 由 `talkLayout.test.ts` 逐字对齐）。
   */
  let folded = $state(
    typeof window !== 'undefined' && window.matchMedia(TALK_FOLD_QUERY).matches,
  );
  let foldedMq: MediaQueryList | null = null;
  let onFoldChange: ((e: MediaQueryListEvent) => void) | null = null;

  /**
   * 移动款（≤479px，与 `app.css` 的窄屏基线同档）。**只剩输入框的占位语用它**：
   * 「Enter 发送，Shift+Enter 换行」这句在手机上是一句空话（没有 Shift 键），而版面判据
   * 一律走上面的 899——两件事的理由不同，故不是同一个断点。
   */
  let narrow = $state(
    typeof window !== 'undefined' && window.matchMedia(TALK_MOBILE_QUERY).matches,
  );
  let narrowMq: MediaQueryList | null = null;
  let onNarrowChange: ((e: MediaQueryListEvent) => void) | null = null;

  /** 滚动容器（`scrollToNewest` 用）：窄屏是整页，故只绑桌面那个时间线。 */
  let timelineEl = $state<HTMLElement | undefined>();
  /** 状态区（展开一张时把它拉回视野，见 `toggleStop`）。 */
  let zoneEl = $state<HTMLElement | undefined>();
  /** 输入坞的 textarea（折行档自长的接线点，见下面那个 `$effect`）。 */
  let typerField = $state<HTMLTextAreaElement | undefined>();

  /**
   * 贴底跟随（决策 301）：视口还在最新那一头吗。
   *
   * 三件事共用这一枚状态——流式增量要不要把视口带下去、悬浮的回底钮出不出现、
   * 「新轮落地」那一拍滚不滚。初始为**真**：打开这一页当然是在看最新那一头；
   * 人上滑超过容差即为假，滚回底部又为真（判据在下面挂着的那个 scroll 监听里）。
   */
  let following = $state(true);
  /**
   * 贴底容差（沿用 ZCode 那一档）：48px 之内视作「还在底上」——一两像素的抖动、
   * 一轮落地时那点高度差都不该把跟随关掉，而真上滑一屏必然超过它。
   */
  const FOLLOW_BOTTOM_PX = 48;

  /**
   * 这一档的滚动容器（决策 301）：折行档**整页随手指滚**，故是 `document.scrollingElement`；
   * 桌面款滚的是时间线自己（它是唯一会滚的区域）。
   *
   * `scrollToNewest` 与贴底判据共用这一处——档位判别只此一份，两处各写各的迟早分叉。
   */
  function scrollContainer(): HTMLElement | Element | null {
    return folded ? document.scrollingElement : (timelineEl ?? null);
  }

  /** 视口在不在底（差 FOLLOW_BOTTOM_PX 以内算在）：没量到容器时按「在」——不误判成人在读历史。 */
  function atBottom(): boolean {
    const el = scrollContainer();
    return el === null ? true : el.scrollHeight - el.scrollTop - el.clientHeight <= FOLLOW_BOTTOM_PX;
  }

  const pending = $derived(board.pendingTasks);

  /**
   * 地址里那一班（`?session=`，决策 217①：「我在哪」进 URL）。**它才是权威**——
   * 前进 / 后退因此能回到上一班，刷新也照地址恢复；没有它才轮到 localStorage 兜底
   * （跨页面回来时地址会丢参数，而「我一直在看这一班」不该因此被重置）。
   * 两条时间线各自带地址（票 04）：`/talk` 与 `/talk/watch` 都认这个参数。
   */
  const urlSession = $derived(
    router.route.name === 'talk' || router.route.name === 'talk-watch'
      ? (router.route.query.session ?? null)
      : null,
  );

  /**
   * chip 行与 ⋯ 菜单共用的那一份（票 06：归档开关的取舍住在 `lib/sessionChips.ts`，
   * 两处各写各的迟早分叉）。当前所在的归档班次永远在行里——见那里的判据说明。
   */
  const chips = $derived(chipRow(sessionList, showArchived));
  /** 折行档里班次列表的那些行（当前那一班在下一条里单独渲染成身份行）。 */
  const otherSessions = $derived(chips.filter((s) => s.id !== currentId));
  /**
   * 当前开着的是归档班次（票 06）：只读时间线——输入坞整块不渲染、不给「恢复归档」
   * 的口子（Q6：不做额外管理），消息内容照旧完整可读。
   */
  const archivedOpen = $derived(session?.session?.archived_at != null);

  /**
   * 展开的那张急停（决策 183）。`undefined` = 还没选过（跟随默认：**宽屏只有一张时展开它，
   * 两张以上一张都不展开；窄屏恒不展开**）、`null` = 人显式收起、否则是那张的 id。
   * 三态的判据在 `lib/talkStops.ts`，此处只持状态。
   */
  let chosenStop = $state<string | null | undefined>(undefined);
  const stopIds = $derived(pending.map((t) => t.id));
  const foldable = $derived(isFoldable(stopIds, folded));
  const openStop = $derived(resolveOpenStop(stopIds, chosenStop, folded));

  /**
   * 8 工位的值班灯：按列聚合，与看板列头**读同一个答案**（决策 251②）。
   *
   * 判断落在 `aggregateStationState` 一处——规格 `theme-6-pixel.md:628` 说值班板是
   * 「同一份读数在看板 8 列与顶栏灯带上各有一份，**这是第三份**」，三份不许各推一遍：
   * 此前这里是就地推的，且**少了失败分支**（全失败的工位画成空灯框，与看板的红灯打架）。
   */
  const crew = $derived(
    BOARD_COLUMNS.map((col) => {
      const tasks = board.tasks.filter((t) => col.stages.includes(t.current_stage));
      return {
        key: col.key,
        label: col.label,
        count: tasks.length,
        state: aggregateStationState(tasks.map((t) => t.status)),
      };
    }),
  );

  /**
   * 值班板的状态 → **灯**的变体（决策 251①③）。词表是契约的 `StationState`：
   * 灯取 `--go` / `--pending` / `--done` / `--stop`，`idle` 不亮灯（空灯框）。
   * `stop` 这一档（`x`）是本批新增的——改之前全失败的工位掉进 `idle`、画的是空灯框。
   */
  function crewLampClass(state: StationState): string {
    switch (state) {
      case 'warn':
        return 'w';
      case 'go':
        return 'c';
      case 'done':
        return 'd';
      case 'stop':
        return 'x';
      default:
        return '';
    }
  }

  /**
   * 值班板的状态 → **行**class（决策 251①③）。三档 + 一档留白，与改之前逐字同一份：
   * 急停琥珀（`pen`）> 在跑提亮（`hot`，只把工位名提到 `--text-hi`）> 失败红
   * （`fail`，本批新增的一档）> 其余保持次级灰。`idle` / `done` 不上行 class——
   * 它们的语义全在灯上。**不给它新造图元或颜色**（决策 169 / 200 的纪律）。
   */
  function crewRowClass(state: StationState): string {
    if (state === 'warn') return 'pen';
    if (state === 'go') return 'hot';
    if (state === 'stop') return 'fail';
    return '';
  }

  /**
   * 时间线：台账行、提议与三种在飞轮归约成一列 {@link TurnView}（票 02；判断在
   * `lib/talkTurns.ts`——合并与排序的坑都长在行与行的关系上，组件只把四个响应式输入
   * 加一枚判好的布尔传进去；行分类读决策 252 给的 `kind` / `proactive`，配对与否读
   * 决策 259 的 `kind`——正文哨兵不再由界面解析）。
   *
   * `ledgerKind`（票 04）把**这本账是谁的**一并交给归约：值守账里名牌按班次类型写
   * （不靠 `proactive` 逐行猜）、在飞占位句说「值守正在跑」。
   */
  const ledgerKind = $derived(watchMode ? 'watch' : 'talk');
  const turns = $derived.by<TurnView[]>(() =>
    buildTurns({
      session,
      pendingText,
      sending,
      following: followingSince !== null,
      stream,
      pairingNeeded: sendPairingNeeded,
      ledgerKind,
      ledgerOwnsFailure,
    }),
  );

  /** 对话时间线是空的（且不是「还没读到」）：空态要居中，见 CSS 的 `.timeline.empty`。 */
  const timelineEmpty = $derived(!(loading && !session) && turns.length === 0);

  /* reload（重读班次列表与某个班次的台账，含过期回包守卫与落点收口）自决策 354① 起住在
   * `stores/talk.svelte.ts`：台账是 store 的，重读也是 store 的——本页与 store 内部的
   * 收尾路径（落地哨、重连校准）调的是同一个 `talk.reload()`。 */

  /**
   * 滚到顶加载更早的消息（票 05：`before_id` 向上游标）。
   *
   * **阅读位置不跳**：接上去之前记下滚动高度与位置，段接到头部、`tick()` 等 DOM 长高
   * 之后把 scrollTop 补上长高的那一截——眼睛看着的那一行 stays put。账的判据
   * （游标、守卫、到头即止）在 store 的 `talk.loadEarlier()`（决策 354①），它返回
   * 「有没有接上新段」，这里只管这一屏的滚动几何——**能不能翻也只有 store 判**（它自己
   * 那两道早退），页面在实参上再判一遍就是第二个判据点。
   */
  async function loadEarlier(): Promise<void> {
    const el = timelineEl;
    const prevHeight = el?.scrollHeight ?? 0;
    const prevTop = el?.scrollTop ?? 0;
    const appended = await talk.loadEarlier();
    if (!appended) return;
    await tick();
    if (el) el.scrollTop = prevTop + (el.scrollHeight - prevHeight);
  }

  /** 滚到顶触发向上加载（票 05）：留一小段余量，免得要滚得严丝合缝才触发。 */
  function onTimelineScroll(): void {
    const el = timelineEl;
    if (el && el.scrollTop < 160) void loadEarlier();
  }

  /* 落点收口（seen 看过表 / 兜底文件 / 地址，决策 217①④ / 220③）自决策 354① 起随台账
   * 住在 store 的 `reload` 尾部（`rememberLanding`）：它是「落地」那件事的一半，
   * 而落地发生在页面之外是常态。 */

  /**
   * 切到另一班（账在 store 的 `talk.switchTo`，决策 354①）。
   *
   * 页面只剩两样接线：**草稿的清空时机**（输入框是页面私有——重置成功才清，store 在
   * `resetLedger` 那一拍回调 `onReset`）与同一班的早退（chip 点了自己：草稿不动）。
   */
  async function switchTo(id: string, opts: { write?: boolean } = {}) {
    if (id === currentId) return;
    input = '';
    await talk.switchTo(id, opts);
  }

  /**
   * 地址里的班次变了就跟着走（后退 / 前进，决策 217④ 的恢复语义）。
   *
   * 只在**装载完成之后**跟：首次装载由 `onMount` 负责（它还要兼顾 localStorage 兜底），
   * 两边都动手会装载两遍。`switchTo` 先把 `currentId` 认下来，故由它自己写地址那一趟
   * 不会被这里再拦一次。
   */
  $effect(() => {
    const want = urlSession;
    if (loading || !want || want === currentId) return;
    void switchTo(want, { write: false });
  });

  /* 开新班次（`openFreshSession`）与归档坠落（`archiveAndFall`）自决策 354① 起住在 store：
   * 建班 / 归档之后「落点在哪、台账怎么落」都是账的事。页面只递 `onReset`（清输入框草稿）
   * 与 `onArchived`（关对话框）两个回调，时点与改前逐拍一致。 */

  /** 从界面按下「+ 新班次」（⋯ 菜单的第一项）。用户按的 = 一次换班，故 push 进地址。 */
  async function newSession() {
    if (sending || busy) return;
    busy = true;
    try {
      await talk.openFreshSession({ push: true, onReset: () => (input = '') });
    } catch (err) {
      talk.loadError = (err as Error).message;
      talk.loadErrorPairing = isPairingRequired(err);
    } finally {
      busy = false;
    }
  }

  function openRename() {
    if (!session?.session) return;
    titleDraft = session.session.title;
    dialogError = null;
    dialog = 'rename';
  }

  async function submitRename(event: SubmitEvent) {
    event.preventDefault();
    const id = currentId;
    const title = titleDraft.trim();
    if (!id || !title || busy) return;
    busy = true;
    try {
      const updated = await renameForemanSession(id, title);
      talk.sessionList = talk.sessionList.map((s) => (s.id === id ? updated.session : s));
      if (talk.session?.session) talk.session = { ...talk.session, session: updated.session };
      dialog = null;
    } catch (err) {
      dialogError = (err as Error).message;
    } finally {
      busy = false;
    }
  }

  /**
   * 归档当前班并**自动切到最近活动的未归档班次**；一个都不剩时新开一个（决策 204）。
   *
   * 不切的话屏幕上会留着一个已经不在列表里的班次，人接着说话才发现「这一班已经归档了」
   * ——把一件必然要做的善后交给使用者做，是界面偷懒。
   */
  async function submitArchive(event: SubmitEvent) {
    event.preventDefault();
    const id = currentId;
    if (!id || busy) return;
    busy = true;
    try {
      await talk.archiveAndFall(id, {
        onArchived: () => (dialog = null),
        onReset: () => (input = ''),
      });
    } catch (err) {
      dialogError = (err as Error).message;
    } finally {
      busy = false;
    }
  }

  /**
   * 现在几点（提议的过期按它算，决策 207）。
   *
   * **只有在有未决提议时才走**（`$effect` 里判）：这条 10s 的心跳是给「还剩 N 分钟」
   * 与到点变灰用的——没提议时它一次状态更新都不该产生，否则整页每 10 秒重算一遍。
   * 到期那一刻不等后端把它标成 `expired`：清理挂在每小时的维护作业上，等它会让按钮
   * 在过期之后还亮着（判据见 `lib/proposals.ts`）。
   */
  let now = $state(Date.now());
  $effect(() => {
    if (!(session?.proposals ?? []).some((p) => p.status === 'pending')) return;
    const t = setInterval(() => (now = Date.now()), 10_000);
    return () => clearInterval(t);
  });

  /**
   * 标记的时钟（决策 220③：**超时**与**落地**两条收口）。
   *
   * **只在真有别的班次在回话时才走**（与上面那条 10s 心跳同一姿态：没有东西要看的时候，
   * 一次状态更新都不该产生）。查得比心跳密一点——这是一枚说「此刻」的标记，
   * 慢半拍地撤掉比不显示更坏。传 `sessionList` 进去是为了第二条判据：那一班的
   * `last_active_at` 一旦比我们记下的时刻新，就说明这一轮的收尾已经落台账了（判据在
   * `pruneForeignActive`），此刻它不再「正在回话」。列表没刷新时兜底的是静默超时。
   */
  $effect(() => {
    if (Object.keys(foreignActive.bySession).length === 0) return;
    const t = setInterval(() => {
      markerNow = Date.now();
      const pruned = pruneForeignActive(foreignActive, sessionList, markerNow);
      if (pruned !== foreignActive) talk.foreign = pruned;
    }, 5_000);
    return () => clearInterval(t);
  });

  /* 「跟一轮」的**落地哨**自决策 275 起住在 store（`stores/talk.svelte.ts::pollFollow`）：
   * 它与「这一轮此刻在跑」这件事实同寿，而页面来去不该影响收口——页面切走之后那一趟
   * POST 才回来的情形，本页早已销毁，收口在那里发生的话只会写进一个死组件里
   * （实测：切去看板再回来，落地那一刻时间线空了一格）。收口之后的台账重读也归 store
   * （决策 354①）：`bindRecalibrate` 交棒那一跳随 epoch 喊话协议一并退场。 */

  /** 提议对应的那个任务的 `allowed_actions`（不指路时用不到，判据在 `lib/proposals.ts`）。 */
  function actionsFor(p: ForemanProposal): AllowedAction[] | undefined {
    const id = proposalTaskId(p);
    return id ? details[id]?.actions : undefined;
  }

  /** 正在执行 / 拒绝的那条提议 id（两颗钮一起禁用，理由与 `board.actionBusy` 同一姿态）。 */
  let proposalBusy = $state<string | null>(null);
  /**
   * 提议操作的失败说明（就地显示在**那一轮**里，不弹窗、不 toast）。
   *
   * 带 id 而不是一个裸字符串：同时挂着两条提议时，裸字符串会把同一条报错挂到两轮下面
   * ——人按的是第二条，却在第一条下面读到失败原因。
   */
  let proposalError = $state<{ id: string; text: string } | null>(null);

  /**
   * 按下确认钮（执行 / 拒绝）。
   *
   * **结果一律回灌成一轮对话**：后端把「执行了 / 拒绝了 / 执行失败」都落成一条系统轮，
   * 这里只需重读台账——不弹窗、不 toast，与 `send()` 的失败处理同一姿态。
   * 失败（400 / 409）就地挂在那一轮下面并保留提议原状：失败**不消耗**提议，
   * 人还能改主意去按「拒绝」。
   */
  async function actOnProposal(id: string, act: 'execute' | 'reject') {
    if (proposalBusy) return;
    proposalBusy = id;
    proposalError = null;
    try {
      if (act === 'execute') await executeForemanProposal(id);
      else await rejectForemanProposal(id);
      await talk.reload();
    } catch (e) {
      proposalError = { id, text: e instanceof Error ? e.message : String(e) };
      // 失败也要重读：过期 / 态势变化这两种失败**改了库里的状态**（标 expired / 保持 pending
      // 并落一条说明），不重读的话界面显示的仍是按键之前那一份。
      await talk.reload();
    } finally {
      proposalBusy = null;
    }
  }

  /**
   * 可见性恢复：重读一次台账。
   *
   * 流的重连不在这里——那归 store（决策 275，`talk.syncFollowing` 那条注释写着它的两条
   * 收口）；本页只管**这一屏读到的东西**跟上服务端。
   */
  function onVisible() {
    if (document.visibilityState !== 'visible') return;
    void talk.reload();
  }

  onMount(() => {
    // 两本账共用一个 store（决策 286 / 354①）：挂载时认下「这本账是谁的」——
    // reload 的 `?kind=` 与 seen 记账（只属于人的班次列表）都按它走。读的是上面那枚
    // `ledgerKind`（模板渲染的那一个）而不是就地再写一遍三元：这本账是谁的只有一个判据，
    // store 这一格只是它在**页面之外**（落地哨、重连校准）还记得的那份副本。
    talk.kind = ledgerKind;
    // 「显示已归档」是一次浏览动作，不是身份（票 06 的口径）——store 寿命比页面长，
    // 故挂载时拨回关，与「下次进本页回到关」逐字一致。
    talk.showArchived = false;
    // 「转去对话」的交接（票 04）：值守账把摘录放进 store，这里消费——预填是**草稿**
    // （人可以改、可以扔），故填进输入框即清，不做任何「未发送」的持久账。
    // 在人的对讲台上才消费：值守账自己没有坞。
    if (!watchMode && talk.watchDraft !== null) {
      input = talk.watchDraft;
      talk.watchDraft = null;
      void tick().then(() => typerField?.focus());
    }
    // 地址权威、localStorage 兜底（决策 217④）：「我一直在看这一班」不该因为从看板点回来而重置。
    // 装载目标在发请求**之前**就认进 store（票 01）：reload 的过期回包守卫拿它当锚，
    // 而 store 里的 `sessionId` 可能还是上一页留下的旧值——不先认下，首读就会被守卫误杀。
    // 值守账的落点（没有 ?session= 时）由 reload 落地后认下，这里只认地址/兜底给的那个。
    const initialWant = urlSession ?? loadSessionId() ?? undefined;
    if (initialWant !== undefined || talk.sessionId === null) talk.watch(initialWant ?? null);
    // 回执标签取一次（模块级缓存，之后别的页签再挂载不再发第二跳）。失败**不打断对话**：
    // 缓存不记失败（下一次挂载会重试），期间回执原样显示工具名——英文原名好过一个错词。
    void loadToolLabels()
      .then((l) => (toolLabels = l))
      .catch(() => {});
    void talk.reload(initialWant);
    // 流归 store（决策 275）：那条 `/foreman/stream` 连接与在飞一轮的现场都活在页面之外。
    // 重连成功后的全量校准（票 03，stream-self-heal）也归 store 自己 reload——台账住 store
    // （决策 354①）之后，`bindRecalibrate` 那一跳没有存在的理由了。
    // 两档断点都是 `lib/talkLayout.ts` 的常量（票 07：899 一处定义；479 只剩占位语用它）
    foldedMq = window.matchMedia(TALK_FOLD_QUERY);
    folded = foldedMq.matches;
    onFoldChange = (e: MediaQueryListEvent) => (folded = e.matches);
    foldedMq.addEventListener('change', onFoldChange);
    narrowMq = window.matchMedia(TALK_MOBILE_QUERY);
    narrow = narrowMq.matches;
    onNarrowChange = (e: MediaQueryListEvent) => (narrow = e.matches);
    narrowMq.addEventListener('change', onNarrowChange);
    document.addEventListener('visibilitychange', onVisible);
    return () => {
      document.removeEventListener('visibilitychange', onVisible);
      if (foldedMq && onFoldChange) foldedMq.removeEventListener('change', onFoldChange);
      if (narrowMq && onNarrowChange) narrowMq.removeEventListener('change', onNarrowChange);
    };
  });

  /**
   * 新的一轮落地后把视口带到它那里。
   *
   * **为什么必须有**：折行档的输入坞钉在底栏上沿，回话落在文档末尾（在屏幕之外）——
   * 不跟过去的话，人发完话只看得到一个空白的对话区，得自己往下拨。桌面同理，只是滚的是
   * 时间线自己（它才是那个滚动容器）。滚动一律瞬时（§1 原则 4：无缓动），故直接写 scrollTop
   * ——`app.css` 没有 `scroll-behavior: smooth`，赋值即到位。
   */
  function scrollToNewest() {
    if (folded) {
      const doc = document.scrollingElement;
      doc?.scrollTo({ top: doc.scrollHeight });
      return;
    }
    if (timelineEl) timelineEl.scrollTop = timelineEl.scrollHeight;
  }

  /**
   * 悬浮「滚动到底部」那颗钮的动作（决策 301）：回到底 + **恢复跟随**。
   *
   * 恢复跟随是它存在的理由：只滚不恢复的话，下一串流式增量又不会跟——人得反复按同一颗钮，
   * 而那正是他按第一次时想结束的状态。
   */
  function jumpToBottom() {
    following = true;
    scrollToNewest();
  }

  /**
   * 视口跟着最新那一头走（决策 301；决策 217 那一条「新轮落地滚过去」是它的前身）。
   *
   * 两件事一起管，判据都是「人还在底上吗」：
   *   - **轮数变化**（发送、回话落地）——贴底才跟，人在读历史就交给回底钮；
   *   - **在飞轮的内容在长**（逐字正文 + 步骤数）——这正是「流式跟随」，
   *     此前视口在整个流式期间一动不动，人得自己往下拨。
   *
   * `following` 用 `untrack` 读：它只做**闸门**，不该成为这条效果的依赖——订阅它的话，
   * 人按下回底钮（`following` 翻真）会让这条效果自己再跑一次，而那一刻滚不滚已经由
   * `jumpToBottom` 决定了。
   *
   * **向上补历史那一拍不滚**（票 05）：`loadEarlier` 把更早的段接进头部，轮数同样变多，
   * 但那不是「新轮落地」——滚到底会把刚用 scrollTop 保住的阅读位置当场掀翻（e2e 那条
   * 「不许挪窝」量的就是它）。故读在途标记时同样 `untrack`：订阅它的话，标记在收尾落回
   * `false` 会让这条效果再跑一次，照样滚到底。
   */
  $effect(() => {
    const n = turns.length;
    // 依赖：在飞轮的逐字正文长度 + 步数（内容每长一次就跟一次）
    let grow = 0;
    for (const t of turns) if (t.streaming) grow += t.content.length + t.steps.length;
    void grow;
    // 依赖：贴底容差用到的容器（换档时重判一次，免得旧容器的读数留着）
    void folded;
    if (n === 0) return;
    if (untrack(() => talk.loadingEarlier)) return;
    void tick().then(() => {
      if (untrack(() => following)) scrollToNewest();
    });
  });

  /**
   * 谁在滚都逃不过这一关：容器每滚一次就重算「人还在底上吗」（决策 301）。
   *
   * **不区分「程序化滚动」与「人的滚动」**：我们自己写的滚动只有一种落点——底部，
   * 而它代入判据本来就得出「在底上」。为它加一个标志位反而多一条会漂的状态。
   *
   * 容器随档位换（折行档整页、桌面时间线），故这条效果读 `folded` 与 `timelineEl`：
   * 换档时重挂，旧容器上的监听一并撤掉。`passive` 是因为监听体里只读几何、不阻止默认行为。
   */
  $effect(() => {
    const el = folded ? document.scrollingElement : timelineEl;
    if (!el) return;
    const onScroll = () => {
      following = atBottom();
    };
    el.addEventListener('scroll', onScroll, { passive: true });
    return () => el.removeEventListener('scroll', onScroll);
  });

  /* ───────────── 推理正文的贴底跟随（决策 301） ─────────────
   *
   * 展开着的推理正文在流式期间**自己往下滚**（ZCode 的口径）：不放的话，人看到的是这块
   * 滚动区冻在最初那几行，而它每毫秒都在长——「正在想」的现场感全在末尾那几行上。
   *
   * 暂停只看这一块的**内滚位置**（人上滑读历史即暂停），与整页那一套彼此独立：
   * 它们是两个滚动容器，混用一个「在底上吗」会在人读整页历史时把内滚也冻住。
   */
  /** 展开着的推理正文（按 `step.key`）。非响应式：它只被效果读，不驱动渲染。 */
  const thinkBodies = new Map<string, HTMLPreElement>();
  /** 人在某一块的内滚里上滑了——那一块暂停贴底，直到他自己滚回底部。 */
  const thinkPeeked = new Set<string>();

  function trackThinkBody(node: HTMLPreElement, key: string) {
    thinkBodies.set(key, node);
    return {
      update(next: string) {
        if (next !== key) {
          thinkBodies.delete(key);
          thinkBodies.set(next, node);
        }
      },
      destroy() {
        thinkBodies.delete(key);
        thinkPeeked.delete(key);
      },
    };
  }

  function onThinkScroll(e: Event, key: string) {
    const el = e.currentTarget as HTMLPreElement;
    if (el.scrollHeight - el.scrollTop - el.clientHeight > FOLLOW_BOTTOM_PX) thinkPeeked.add(key);
    else thinkPeeked.delete(key);
  }

  /**
   * 流式期间把展开着的推理正文钉在底部（决策 301）。
   *
   * 只跟**在飞轮**那些块：落地轮的正文是死的（不再增长），人正在读的那一段不该被别处的
   * 变化拽走。依赖取「在飞轮的字数总和 + 步数」——它每长一次这条效果就跑一次，
   * 正是要跟随的那个时刻。
   */
  $effect(() => {
    const live = new Set<string>();
    let grow = 0;
    for (const t of turns) {
      if (!t.streaming) continue;
      for (const s of t.steps) {
        live.add(s.key);
        grow += s.text.length;
      }
    }
    void grow;
    for (const [key, el] of thinkBodies) {
      if (!live.has(key) || thinkPeeked.has(key)) continue;
      el.scrollTop = el.scrollHeight;
    }
  });

  /**
   * 展开／收起一张急停（同时只展开一张，机制在 `lib/talkStops.ts` 的 `toggleOpenStop`）。
   *
   * 展开之后要**把人拉回那张轮**：窄屏的状态区此刻刚从「钉住」（贴在顶栏下沿）变回普通块，
   * 它原本在文档里的位置此刻已经在视口之上——不滚过去，人点了「展开恢复动作」却看不到
   * 任何动作（那正是本页最不能出的错：要拍板的东西不在眼前）。宽屏整页不滚，这句是空操作。
   */
  async function toggleStop(id: string) {
    const next = toggleOpenStop(openStop, id);
    chosenStop = next;
    if (next === null) return;
    await tick();
    zoneEl?.scrollIntoView({ block: 'start' });
  }

  /* ───────────── ⋯ 班次菜单（折行档）：键盘与关闭（票 04 / 决策 251⑤） ─────────────
   *
   * 交互语汇与顶栏「待处理」下拉**同一份**（票 04 / R2-04 补的三条出口），不新造第三套：
   * `aria-expanded` + `aria-controls`、Escape 关得掉（**焦点没进过面板时也算**）、点面板外面关、
   * 上下方向键走项、`Home` / `End`、面板**常驻 DOM** 用 `hidden` 开合。键盘一律在 `window`
   * 上收——给静态元素挂交互处理器是 a11y 检查里的红灯（那两条监听仍在模板的 `<svelte:window>`）。
   *
   * **判据不在这里**：它在 `lib/menuTrap`（决策 251⑤），两处弹层共用一份——此前这两段是
   * 同义的两份（各 80 多行、只换了标识符），而**这一份一条单测都没有**，抄漏一条出口
   * 没人会发现。这里只出**接线**：开关态是本地的 `menuOpen`（顶栏那份住在 `board.pendingOpen`）。
   */
  const menuTrap = createMenuTrap({
    isOpen: () => menuOpen,
    onOpen: () => {
      menuOpen = true;
    },
    onClose: () => {
      menuOpen = false;
    },
    trigger: () => menuTrigger,
    panel: () => menuPanel,
    wrap: () => menuWrap,
    itemSelector: 'button[data-menu-item]:not([disabled])',
  });

  /** 点触发钮开合。打开那一路与键盘打开**同一条路径**（都要等一次 DOM 刷新再送焦点），故走 `menuTrap.open`。 */
  function toggleMenu(): void {
    if (menuOpen) menuTrap.close(false);
    else menuTrap.open();
  }

  /**
   * 工位回执的展开／收起（受控，见 `receiptOpen`）。
   *
   * `preventDefault` 掉 `summary` 的默认行为：让浏览器自己翻 `open`，状态就在 Svelte 之外，
   * 流式增量重渲染那一轮时会与模板里的 `open` 打架。人点一下 = 改一次我们自己的状态。
   */
  function toggleReceipt(event: MouseEvent, key: string) {
    event.preventDefault();
    talk.receiptOpen = { ...talk.receiptOpen, [key]: !receiptIsOpen(key) };
  }

  /**
   * 一条班次该挂哪一枚标记（判据与优先关系全在 `lib/talkSessions.ts`，这里只把输入接上）。
   *
   * `markerNow` 是 `foreignIsReplying` 的时钟：读它才会在超时那一刻重算——那正是
   * 「标记说的是此刻」这条要求的时间那一半。
   */
  function markersFor(s: ForemanSessionMeta): SessionMark {
    return sessionMark(s, {
      currentId,
      sendingSid,
      foreignReplying: foreignIsReplying(foreignActive, s.id, markerNow),
      seen,
    });
  }

  /**
   * 说一句话（坞的入口）。**在飞就入队、空着就直接发**——那个判据自决策 354② 起住在
   * store（`talk.submit`）：页面只拿捏输入框里那句文本（trim、清空），发与排的取舍
   * 与「什么算在飞」由 store 一处说。
   *
   * 排队那条路见 `stores/talk.svelte.ts` 的 `queue`（按班次分列、住 store）；
   * 死轮 / 中断时扣住（`queueHeld`），等坞里的两个出口。
   */
  function send() {
    const text = input.trim();
    if (!text) return;
    input = '';
    talk.submit(text);
  }

  /**
   * 选项点选 = 把选项文本当作下一条 user 消息发回（决策 265⑤：既有写口、零新端点）。
   * 乐观轮、配对闸、落地哨全部走 `send()` 既有那套——这里只负责把 `input` 填上。
   * 已答 / 发送在途时按钮本身是禁用的，这里再拦一道（防连点）。
   */
  function answerAsk(option: string) {
    if (talk.sending) return;
    input = option;
    send();
  }

  /**
   * 停钮（决策 294 / 票 09）：请这一轮停下。
   *
   * **回包那一位是「有没有一轮可停」，不是「它停没停」**：`false` = 这一班此刻没有可停
   * 的一轮（本机这一趟还在路上、服务端尚未登记，或它刚收口）——把「正在停…」撤回去，
   * 人再按一次即可；留着那一格会让人以为按坏了，而**人的那一轮没有时间界**（决策 292），
   * 它可能一直跑下去。端点的文档就是这句话：「界面据此把那颗钮收回去」
   * （`crates/app/src/routes/foreman.rs` 的 `cancel_session`）。
   *
   * `true` 也只说请求送到了（收口是协作的，决策 226）：真停下的证据是那一轮自己落的那一行
   * （`【已停】`），本机这一趟的 POST 回包也会照常把台账重读一遍——两条路都不靠这里假装。
   */
  async function stopTurn() {
    if (stopState !== 'ready') return;
    const sid = sendingSid ?? currentId;
    if (!sid) return;
    stopAsked = true;
    stopError = null;
    try {
      const { cancelled } = await cancelForemanTurn(sid);
      if (!cancelled) stopAsked = false;
    } catch (err) {
      // 请求没送到（网络 / 配对 / 本机这条链路）：**如实撤态**回到可按的「停」，
      // 并把原因写在坞里——不弹窗、不落失败轮（按停不成，不改变「那一轮还在跑」这个事实，
      // 而它会在该收口时收口）。与「流断了」那一行同一姿态：只在异常态占高。
      stopAsked = false;
      stopError = err instanceof Error ? err.message : String(err);
    }
  }

  /* 队列的出队口自决策 354② 起住在 store（`talk.startQueueDrain`，由 `App.svelte` 的
   * `talk.init()` 起）：队列进 store 之后**页面挂载不再是排水的前提**——关着页排出去的
   * 话照发（决策 354② 明确接受的唯一行为修正，排水节奏照旧）。判据（在飞 / 扣住 /
   * 归档 / 值守账）随之下移到 store，那里有它要的每一格读数。 */

  /** 「上一轮没回来」之后人按了确认：放行队列（store 的排水环自会接手）。 */
  function resumeQueue() {
    if (currentId) talk.setQueueHeld(currentId, false);
  }

  /** 人放弃整条队列：清空 + 撤扣住记号。 */
  function clearQueue() {
    const sid = currentId;
    if (!sid) return;
    talk.queue = { ...talk.queue, [sid]: [] };
    talk.setQueueHeld(sid, false);
  }

  /** 排队条的就地编辑态（票 04）：正在改第几条 + 草稿。 */
  let qedit = $state<number | null>(null);
  let qdraft = $state('');

  /** 提交一条排队话的编辑：空文本视作撤回原样保留（真要删用「撤回」）。 */
  function commitQueueEdit(index: number) {
    const sid = currentId;
    if (sid && qedit === index) {
      const text = qdraft.trim();
      if (text) talk.editQueued(sid, index, text);
    }
    qedit = null;
  }

  /**
   * **发话就是一次回底**（决策 301）：人自己按下的那一句必须看得见——他可能正往上翻
   * 历史，若把这一轮也交给「贴在底上才跟」那条判据，他发完话屏幕上什么都没动，
   * 只会以为没发出去。故这一档与「回底钮」同级：无条件恢复跟随，再由上面那条效果滚过去。
   *
   * 判据挂在 `talk.sending` 的**上升沿**而不是写在 `send()` 里（决策 354②）：发送编排
   * 住进了 store，队列里的那一句是**排水环自己**发出去的（页面关着也发）——它同样是人打的
   * 话、同样该看得见。而「上升沿」也正是这条判据要的形状：真写成「在发就一直跟」的话，
   * 人在流式期间上滑读历史会被每一段增量拽回底部。
   */
  $effect(() => {
    if (talk.sending) following = true;
  });

  /**
   * **这次没送出去的那句话回填进输入框**（决策 182㉓「失败不改输入框」）。
   *
   * 框空着才填：人自己又打了字的话，他打的那句才是最新的。填不填都把这格清掉——它说的是
   * **某一次没送到的发送**，不是草稿。发送编排在 store（决策 354②），那句话先落在
   * `talk.unsentText` 上由这里消费：页面关着那一刻发出去的失败，回来这一拍照样回框里。
   */
  $effect(() => {
    const text = talk.unsentText;
    if (text === null) return;
    talk.unsentText = null;
    if (!input.trim()) input = text;
  });

  /* 只重读班次列表（不碰这一屏的台账）的 `refreshSessionList` 自决策 354① 起住在 store：
   * 已经落地的回话（含切走之后落地的那些）更新的是 `last_active_at`——「有新动静」那枚
   * 标记的判据。哪一班在屏由 `reload` 管，它只管那一列元信息。 */

  /**
   * 「显示已归档」开关（票 06）。只换**列表给谁看**，不动当前在读的那一班——
   * 关掉时归档的从行里**全部**退场（含正在读的这班：开关关着还挂一枚灰 chip
   * 就是在说谎），正在读这件事由时间线与只读提示自己说，选中错不到活跃班头上。
   */
  async function toggleShowArchived() {
    talk.showArchived = !talk.showArchived;
    await talk.refreshSessionList();
  }

  /**
   * 输入法组合态（决策 184）。**只查 `event.isComposing` 挡不住**：桌面壳是 WKWebView，
   * 它先发 `compositionend` 再发那次 `keydown`（`isComposing` 已是 false）——于是
   * 「在中文输入法里敲英文、按回车确认」会把半截话直接发出去。判据在
   * `lib/enterToSend.ts`（延迟一拍清标志），这里只接线。
   */
  const composing = new CompositionGuard();

  function onKeydown(event: KeyboardEvent) {
    // Enter 发送 / Shift+Enter 换行（判据见 `lib/enterToSend.ts`）
    if (!shouldSubmitOnEnter(event, composing.active())) return;
    event.preventDefault();
    void send();
  }

  /**
   * 输入框占位语。窄屏那一份短一截，理由不只是省地方：**手机上两个键位提示都是空话**
   * ——没有 Shift 键，也没有「Enter 发送」之外的选项（软键盘的回车键就是发送）。
   * 那句说明因此在窄屏挪到提示行去说一件真事（见标记处）。
   */
  const placeholder = $derived(
    narrow ? '对值班长说一句话…' : '对值班长说一句话（Enter 发送，Shift+Enter 换行）…',
  );

  /**
   * 坞的自长接线（决策 282 ③，修订 218④ 隐含的「rows=2 固定」）：折行档（≤899）
   * **1 行起步、随内容自长、封顶 6 行**——判据在 `lib/talkDock.ts`，这里只把
   * `rows` 拨到判据说的那一档。三条路都汇到这一处：`bind:value` 的每一次输入、
   * 发送后的清空（`send` 里 `input = ''` 那一趟不经过 input 事件）、跨 899 的档位
   * 切换（`folded` 翻转时按目标档重算——桌面档写回 2，逐像素不变，两侧差异是刻意的）。
   */
  $effect(() => {
    void input;
    const el = typerField;
    if (!el) return;
    if (folded) growTextarea(el);
    else el.rows = 2;
  });

  /**
   * 工位名 → sprite：快照里的 stage 是后端字符串，未登记的值不猜（退回台账箱）。
   *
   * **这里刻意不调 `columnForStage`**（实现期观察，**不是**决策 251 的裁决——251② 只裁灯的
   * 优先序与唯一实现）：那张表把 `sync-check` 也映到 design 列，而 `BOARD_COLUMNS` 按决策 107
   * **全站不展示** sync-check。两者答的是不同的问题——「这个 stage 属于哪一列」（含不展示的）
   * 与「它是这 8 列里的哪一列」。改成查 `columnForStage` 会把未列的 sync-check 从台账箱变成
   * 锤子 sprite，那是一处**新的可见行为**，而本批的口径是「行为变化只有一处」（决策 251 §五）。
   * 票 01 原写「二选一，不两处都留」，两项都不成立：① 委派 = 上述行为变化；② 「改由 `crew`
   * 直接给出」做不到——`receipt()` 要的是**任一** trace 的 stage（可能落在没有工位行的列上），
   * 而 `crew` 只有 8 列。故留两处并**在此写明理由**，偏差记在票面。
   */
  function stageSprite(stage: string): SpriteName {
    const col = BOARD_COLUMNS.find((c) => c.stages.includes(stage as Stage));
    return col ? COLUMN_SPRITES[col.key] : 'chest';
  }

  /**
   * 工具那一步 → 回执行（§3.3 纪律 3）。
   *
   * 能对上该轮快照里的任务就标出来源**工位**：像素本身不可考，靠 sprite + 工位名双编码。
   * 对不上（例如查了一次就没了的任务、或还在飞的那一步——那时这一行没有快照）就只说这是
   * 台账查读，不假装知道出处。
   *
   * **入参是两个原语而不是一条工具留痕**（决策 273）：实时那一步与落地那一步的形状不同
   * （一个带相位、一个带 `ok`），而这一行要画的东西是一样的——取值交给调用点，
   * 这里只管「怎么画」。
   */
  function receipt(
    tool: string,
    argsSummary: string,
    briefing: ForemanBriefing | null,
  ): {
    sprite: SpriteName;
    workshop: string;
    label: string;
  } {
    const known = [
      ...(briefing?.pending ?? []),
      ...(briefing?.running ?? []),
      ...(briefing?.failed ?? []),
    ].find((b) => argsSummary.includes(b.task_id));
    return {
      sprite: known ? stageSprite(known.stage) : 'chest',
      workshop: known?.stage ?? '台账',
      label: labelFor(toolLabels, tool),
    };
  }

  /**
   * 「过程」那一组的摘要（决策 273）：**永远带条数**——收起的是版面，不是信息
   * （决策 218 ② 的原话：「内容一个字不删」）。
   *
   * 词表照旧：工具那些步仍叫「N 次台账查读」（回执那一条的既有措辞，决策 247⑤），推理另计一道数。
   * 只有中途的话（既没有工具也没有推理）的那一组理论上不存在——落地段序里 `text` 步只在
   * 带工具调用的那次模型调用之后出现；真出现了就退回按步数说，不编一个词。
   */
  function stepsSummary(steps: TurnStep[]): string {
    const tools = steps.filter((s) => s.kind === 'tool').length;
    const thinks = steps.filter((s) => s.kind === 'thinking').length;
    const parts: string[] = [];
    if (thinks > 0) parts.push(`思考 ×${thinks}`);
    if (tools > 0) parts.push(`${tools} 次台账查读`);
    return parts.length > 0 ? parts.join(' · ') : `${steps.length} 步`;
  }

  /**
   * pending 详情的**指纹去重**拉取（决策 354① 起归 store 管：判据在
   * `talk.syncPendingDetails`，这里只把输入接上）。
   *
   * **必须随 pending 集合变化重拉，不能只在 onMount 拉一次**：`board.init()` 是异步的
   * （App.svelte 的 onMount 发起），本页 onMount 时 `board.tasks` 往往还是空的——
   * 只拉一次的话 `pending` 为空、`details` 永远为空，页面会安静地退化成「打开任务详情」
   * 按钮，把后端下发的恢复动作整片吞掉（e2e ⑩ 打红即此）。
   */
  $effect(() => {
    talk.syncPendingDetails(board.pendingTasks, watchMode);
  });

  async function handleAction(
    taskId: string,
    action: AllowedAction,
    opts: { cursorId?: string; input?: string; push?: boolean },
  ) {
    // board 重载后 pending 集合会变，指纹驱动重拉详情；
    // 这里显式再拉一次是为了动作回执后立刻反映（不等下次轮询）。
    await board.handleTaskAction(taskId, action, opts);
    await talk.loadDetails(board.pendingTasks);
  }

  /**
   * 「转去对话」（票 04）：把这条播报带进人的时间线。**动作本体在那边**——这里只
   * 把摘录交给 {@link talk.watchDraft}（预填草稿，消费即清），跳过去之后由人的对讲台
   * 装载时把它填进输入坞并聚焦。在人少的这一本账上，这是唯一的一颗钮。
   */
  function takeToTalk(content: string) {
    talk.watchDraft = watchDraftExcerpt(content);
    router.navigate('/talk');
  }

  /**
   * 值守台账这会儿「正在跑」吗（票 04：入口上的那一枚标记）。
   *
   * `/foreman/stream` 把全部工头增量广播给所有订阅者，每条都带 `session_id`——人的
   * 班次的增量都能对上这一页的列表（判据在 `noteForeignDelta`），对不上的那份就是
   * **人的班次之外**的工头在说话，而那只有值守轮（它住在本页列表之外的那本账里）。
   * 静默超时与落地清理由既有那套映射管着，这里只读。
   */
  const watchLedgerActive = $derived(
    Object.keys(talk.foreign.bySession).some(
      (id) => !sessionList.some((s) => s.id === id),
    ),
  );
</script>

<main class="talk" class:ledger={watchMode}>
  <div class="talk-head">
    <h1 class="tt">{watchMode ? '值守台账' : '对讲台'}</h1>
    <div class="ts">
      <!-- 车间隐喻的**首现平实说法**（决策 200 / design §12.2）：行内、全宽括号、紧跟词后，
           词与译文同字号同色档。口径**按页面**、同一页面内不重复——故本页每个词只译一次。
           `值班长` 的译文**不在这里**：这一行是页头（标题栏），而决策 200 裁决 ④ 明说
           「按钮与标题里不翻译」；更要紧的是**版面**——430px 上这一行本来就只够一行，
           加上「（跟我对话的 AI）」会把它挤成两行，页头因此从 38px 涨到 72px，
           直接吃掉对话区 34px（决策 192 把对话区从 26px 救回来的那件事，不允许这样退回去，
           `talk.spec.ts` 的窄屏几何用例会红）。译文落在本页正文里 `值班长` 首现的地方
           ——时间线空态那句「说一句，值班长（跟我对话的 AI）就在对面」，那是第一次进这一页
           的人真正读到这个词的地方。**名牌上的 `值班长` / `值班经理` 同样不翻译**
           ——那是发言者称谓，保持原词。 -->
      <!-- 当前班次名（票 03）：折行档把 chip 行收进页头右端的 ⋯ 之后，这是这一班在屏上
           唯一的署名，故补在这里。桌面款不显示（`.sess-name` 只在折行档可见）——那里班次行
           自己挂着每一班的名字，再说一遍是重复。截断宽度按 375px 算，不折行。 -->
      {#if session?.session}
        <span class="sess-name" title={session.session.title}>{session.session.title}</span>
        <span class="sep sess-sep">▪</span>
      {/if}
      <span>{session?.foreman.wired === false
        ? '值班长未接线'
        : watchMode
          ? '值守中'
          : '值班中'}</span>
      <span class="sep">▪</span>
      <!-- `工位` 的译文只在这一行（`.stat-wide` 窄屏收起）：窄屏上这个词不再出现
           （值班板的「8 工位」与那两行说明都收进了桌面款），故窄屏没有漏译。 -->
      <span class="stat-wide">夜班态势：8 工位（流水线的阶段）</span>
      <span class="sep stat-wide">▪</span>
      <span class="tok">本次会话 {session ? formatTokens(session.total_tokens) : '—'} tok</span>
      <!-- 未消费待办的读数（决策 307，票 06）：**独立一枚只读 span**，不并进下面那条
           「值守台账 · 正在跑」的 crumb——后者的判据是「值守轮在飞」，而本读数最该被看见的
           时候恰好是值守轮**正在排队**（决策 289），那一刻 `in_flight = false`、crumb
           根本不出现。条数为 0 时不渲染（省窄档空间）。
           窄档用「本次会话 N tok」腾位置（`.tok` 在 ≤479 收起）：决策 300 摘掉状态条时
           腾出的位置补的是**这一类读数**——不是决策 300 明说不做的 pending，两者语义不同
           （一个报「系统在报警」，一个报「看板上的待办列」，后者看板顶栏已有）。 -->
      {#if openAttention}
        <span class="sep">▪</span>
        <span class="attn" title={attentionTitle}>{`待办 ${openAttention.open} 条`}</span>
      {/if}
      {#if watchMode}
        <!-- 回对讲台（票 04）：只读账上唯一的出口，与人的对讲台页头末尾那条「值守台账」
             是同一对进法的两半。挂页头元信息行而不是班次行——值守台账**不是一班**，
             不进「选一条班次」的那一排。 -->
        <span class="sep">▪</span>
        <a class="crumb" href="#/talk" onclick={() => router.navigate('/talk')}>回对讲台</a>
      {:else}
        <!-- 值守台账的入口（票 04 / 决策 240 的「同一个目的地只摆一套进法」）：两条页头
             元信息行互指，不另造第三处。带「值守正在跑」的读数（票 04：`turn_in_flight`
             的用途）：它在说话时这一条亮一档，让人知道那本账此刻有动静。 -->
        <span class="sep">▪</span>
        <a
          class="crumb"
          href="#/talk/watch"
          title={watchLedgerActive ? '值守轮这会儿正在跑' : '值守轮自己的台账（只读）'}
          onclick={() => router.navigate('/talk/watch')}
        >{watchLedgerActive ? '值守台账 · 正在跑' : '值守台账'}</a
        >
      {/if}
      <!-- 页头末尾原先还有一颗「看板」面包屑（决策 240 摘除）：看板是顶栏那一行里的一枚页签，
           本页不再代它递入口。`router` 仍为本页其余跳转所用（见下）。 -->
    </div>

    <!-- ── 班次行（决策 204③；落点由决策 218 ②/Q15 改到**页头右端**）：桌面挂在这里，
         `nowrap` + 容器内横滚 + `min-width: 0`，故页头高度不因它变（`--talk-chrome` 因此
         也不必动）。它原先长在 `.timeline` 这个滚动容器**里面**，于是随对话上移——实测长
         会话滚到底时它在屏幕上方 320.2px，那正是「新建对话要往上翻很久」的病根。
         **不得渲染成 `.turn`**：时间线里那些是发言，而这是一排控件。 -->
    {#if !folded}
      <div class="runrow no-scrollbar" role="group" aria-label="班次">
        {#each chips as s (s.id)}
          <!-- 切换**不因 `sending || busy` 禁用**（决策 220②）：回话中也可以换班次，
               「那一轮回话去哪了」由 ⋯ 列表里的两枚标记说。灰 chip = 已归档（票 06）：
               判据在 `archived_at` 字段上，与活跃班次一眼可辨。 -->
          <button
            type="button"
            class="runchip"
            class:now={s.id === currentId}
            class:arch={s.archived_at != null}
            aria-pressed={s.id === currentId}
            title={s.archived_at != null ? `${s.title}（已归档，只读）` : s.title}
            onclick={() => void switchTo(s.id, { write: true })}
          >
            {s.title}
          </button>
        {/each}
        <!-- 「显示已归档」开关（票 06）：放在动作块**之外**——值守账同样能翻归档
             （只读账本来就只读，开关与它不冲突）。 -->
        <button
          type="button"
          class="runchip arch-toggle"
          class:on={showArchived}
          aria-pressed={showArchived}
          title="列表里显不显示已归档的班次"
          onclick={() => void toggleShowArchived()}
        >
          显示已归档
        </button>
        {#if !watchMode}
          <!-- 只读账上这些「说话 / 动手」的班次动作全部收起（票 04）：值守台账不是一班，
               不开新班、不改名、不归档。 -->
          <button
            type="button"
            class="runchip plus"
            disabled={sending || busy}
            title="开一个新班次"
            onclick={() => void newSession()}
          >
            + 新班次
          </button>
          {#if currentId}
            <button
              type="button"
              class="runchip act"
              disabled={sending || busy}
              onclick={openRename}>改名</button
            >
            {#if !archivedOpen}
              <!-- 归档按钮对已归档的班退场（票 06 的只读）：它已经归档了，再按一次
                   没有意义，而「恢复归档」这个口子本轮明确不做（Q6：不做额外管理）。 -->
              <button
                type="button"
                class="runchip act"
                disabled={sending || busy}
                onclick={() => {
                  dialogError = null;
                  dialog = 'archive';
                }}>归档</button
              >
            {/if}
          {/if}
        {/if}
      </div>
    {:else if !watchMode}
      <!-- ── ⋯ 班次菜单（折行档，决策 218 ②/Q10/Q12/Q13）。桌面**不渲染**它
           （Q14：桌面横向宽裕，把每一班的名字收进菜单是净丢信息）。
           面板常驻 DOM 用 `hidden` 开合——`aria-controls` 指过去的目标必须真的存在。 -->
      <div class="more-wrap" bind:this={menuWrap}>
        <button
          type="button"
          class="more"
          aria-label="班次菜单"
          aria-expanded={menuOpen}
          aria-controls="talk-session-menu"
          bind:this={menuTrigger}
          onclick={toggleMenu}
        ></button>
        <div class="smenu panel" id="talk-session-menu" hidden={!menuOpen} bind:this={menuPanel}>
          {#if sessionList.length === 0 && !session?.session}
            <div class="mi-empty">还没有班次。说一句话就会开一个。</div>
          {/if}
          <!-- 「+ 新班次」是第一项（这一页最常按的一颗），且**本页班次动作的唯一入口**
               （决策 218 当日修订 ①：坞里那一颗已撤）。回话中禁用（`sending || busy`）。 -->
          <button
            type="button"
            class="mi plus"
            data-menu-item
            disabled={sending || busy}
            onclick={() => {
              menuTrap.close(false);
              void newSession();
            }}>+ 新班次</button
          >
          {#if session?.session || otherSessions.length > 0}
            <div class="mi-sep"></div>
          {/if}
          {#if session?.session}
            {@const cur = session.session}
            {@const curMark = markersFor(cur)}
            <!-- 当前班次是**身份行**不是按钮：点了它没有去处（你已经在这一班）。
                 `aria-current` 让它既可见又播报（票 04）。灰 = 已归档（票 06）。 -->
            <div class="mi now" class:arch={cur.archived_at != null} aria-current="true">
              <span class="mi-nm">{cur.title}</span>
              <span class="mi-meta dim">{formatDateTime(cur.last_active_at)}</span>
              {#if curMark === 'replying'}<span class="mi-mark rep">正在回话</span>{/if}
            </div>
          {/if}
          {#each otherSessions as s (s.id)}
            {@const m = markersFor(s)}
            <!-- 切换**不因回话中禁用**（决策 220②/⑤）：点「正在回话」那一条就是切过去，
                 先看到回话的后半截、落地后 `reload()` 补齐——这不拦。 -->
            <button
              type="button"
              class="mi"
              class:arch={s.archived_at != null}
              data-menu-item
              onclick={() => {
                menuTrap.close(false);
                void switchTo(s.id, { write: true });
              }}
            >
              <span class="mi-nm">{s.title}</span>
              <span class="mi-meta dim">{formatDateTime(s.last_active_at)}</span>
              <!-- 标记只影响**读**：用词而不是纯色块（决策 195 的次级必读档门槛），
                   且进可访问名（这是按钮自己的子节点，不做 `aria-hidden` 装饰）。
                   「两条同时成立时哪条优先」不在这里判——`sessionMark()` 已经只回来一枚。 -->
              {#if m === 'replying'}<span class="mi-mark rep">正在回话</span>
              {:else if m === 'fresh'}<span class="mi-mark fresh">有新动静</span>{/if}
            </button>
          {/each}
          <div class="mi-sep"></div>
          <button
            type="button"
            class="mi act"
            data-menu-item
            disabled={sending || busy || !currentId}
            onclick={() => {
              menuTrap.close(false);
              openRename();
            }}>改名</button
          >
          {#if !archivedOpen}
            <button
              type="button"
              class="mi act"
              data-menu-item
              disabled={sending || busy || !currentId}
              onclick={() => {
                menuTrap.close(false);
                dialogError = null;
                dialog = 'archive';
              }}>归档</button
            >
          {/if}
          <!-- 「显示已归档」开关（票 06）：折行档的镜像位——班次行收进这颗菜单之后，
               开关也跟着进来（桌面在 `.runrow` 里，两处同一个动作）。 -->
          <button
            type="button"
            class="mi act"
            class:on={showArchived}
            data-menu-item
            aria-pressed={showArchived}
            onclick={() => {
              menuTrap.close(false);
              void toggleShowArchived();
            }}>显示已归档</button
          >
        </div>
      </div>
    {/if}
  </div>

  <!-- ── 状态区：当前急停。桌面钉在第一屏，不随时间线滚动；折行档默认收成摘要条并钉在
       页头那条带子的下沿（`.stops` / `.stop-open` 两个类只在折行档块里有规则）。
       值班板不在本区 ——桌面是右栏、折行档**整块不渲染**（同一份读数在看板 8 列与顶栏
       灯带上各有一份，这是第三份；决策 218 ②）。

       **一张急停都没有时整块退场**（折行档；决策 218 修订 ⑦b）——430px 上它今天约 65px。
       但 `loadError` 是另一回事：读不到台账时这里必须还说得出话，故它单独把关。
       桌面款照旧保留那个引导块。 ── -->
  {#if loadError || (!watchMode && (!folded || pending.length > 0))}
  <section
    class="zone-status"
    class:stops={pending.length > 0}
    class:stop-open={openStop !== null}
    bind:this={zoneEl}
    aria-label="值班台"
  >
    {#if loadError}
      <div class="blank error">
        <p>{loadError}</p>
        <!-- 配对入口在**两条**失败路径上都要给（票 07）：读会话与发话各自会撞 403，
             只在其中一处给链接，另一处的使用者就只看到一句「这台设备还没配对」而无处可去。
             **但这条指引必须点名「哪台机器」（决策 189）**：原先写的是「在已配对的设备上重扫
             一次」，而座机上的「手机访问」页在手机上打开是拿不到配对码的（配对令牌只允许回环
             来源读取），照着这句话做的人会一直停在这一页——指引把使用者的力气导向重复扫码，
             而不是去找那台电脑。 -->
        {#if loadErrorPairing}
          <p class="note">
            配对码只在那台跑服务的电脑本机生成：在那台电脑上（桌面应用窗口，或浏览器里的
            127.0.0.1）打开<a
              class="crumb"
              href="#/share"
              onclick={() => router.navigate('/share')}>手机访问</a
            >页扫码即可——手机上打开这一页是拿不到配对码的。若已添加到主屏幕，从图标打开即是<b
              >独立窗口</b
            >（没有地址栏与底栏）；<b>换过令牌后要重新添加一次</b
            >（图标里记的是当时那条带令牌的地址）。
          </p>
          <!-- 这次启动的地址摊开（决策 285）：standalone 没有地址栏，「图标到底带没带参数」
               在界面上本该有处可读——判混了就会把人送去重扫一个本来就对的码。令牌值打码。 -->
          <p class="note">
            本次启动地址：<code>{maskPairValue(launchHref)}</code>——{pairingLaunchNote(
              launchHref,
              getPairingToken(),
            )}
          </p>
        {/if}
      </div>
    {/if}

    {#if !watchMode}
      <!-- 急停轮与其空态只在人的对讲台上渲染（票 04）：值守台账是只读的一本账，
           拍板的动作在对讲台。loadError 在两种形态里都要有处可读（见上面的把关）。 -->
      {#each pending as task, i (task.id)}
      {@const detail = details[task.id]}
      {@const count = stopActionCount(detail)}
      {@const open = isStopOpen(foldable, openStop, task.id)}
      {@const first = i === 0}
      <!-- 急停轮：全站唯一"响"的一处（琥珀框 + ▼ + 恢复动作）。折叠只收动作区，不收身份：
           折叠行的框色 / 硬投影 / ▼ 与展开行完全相同（决策 183）。**折行档的折叠态例外**：
           摘要条不画名牌（决策 218 修订 ⑦a——那张 20px 的净空是给悬出框沿 14px 的名牌留的，
           而决策 183 裁决③给摘要条定的三块里本就没有名牌；身份由框色 / 硬投影 / ▼ 承担）。 -->
      <article class="turn warn" class:folded={!open}>
        <!-- 名牌是发言者：这一轮是操作台在报"卡住了、要你按键"，不是值班长在说话
             （值班长的话一律没有按钮，见 §3.3 的四条纪律） -->
        {#if open || !folded}
          <div class="dname">操作台</div>
        {/if}

        {#if open}
          <!-- 「急停」的首现平实说法落在**本页第一张**的琥珀标签上（决策 218 修订 ⑦b：
               原先在窄屏那条空态里，而那条空态在折行档整块退场了）。主判据是 `i === 0`，
               **不拘形态**——桌面只有一张时它是展开的，若按「只有摘要条才给括号」办，
               那一档整页就没有一句解释（口径按页面、同一页面内不重复是决策 200 的要求）。 -->
          <div class="dtag">
            ⏸ 急停{first ? '（等你拍板的阻塞）' : ''} · {pendingLabel(task.pending_reason)}
          </div>
          <p>
            「{task.title}」走到 {task.current_stage}：
          </p>
          <!-- 阻塞原因照回话待遇渲染（票 05 of talk-live-identity）——diagnose 类原因里
               带 run id / 反引号路径是常态。 -->
          <MarkdownView source={task.pending_reason?.message ?? '需要你决定。'} />
          <div class="ctx">
            状态：<b>{task.status}</b> ▪ 已跑 {formatDuration(taskDuration(task))} ▪
            <Gauge tokens={task.total_tokens} tone="warn" /> {formatTokens(task.total_tokens)} tok
          </div>

          <!-- 合并审批走 DiffReviewPanel 的**动作行**，与任务详情页（`PendingDossier` /
               Diff 页签）同一套渲染：后端把 approve / return 都归为 `side_effect`，
               交给 PendingActions 会两颗都落进「旁路动作」——全站最重要的一颗
               「合入」于是长成一行弱化灰字，看不出它才是主动作（原型 `.acts` 里它是
               实心绿钮 `btn solid`，`返回修改` 才是 `btn quiet`）。
               其余 pending 类型仍交 PendingActions：那里才有游标选择与 resume 的自由输入。 -->
          {#if detail && task.pending_reason?.type === 'merge_approval'}
            <DiffReviewPanel
              actionsOnly
              diff={null}
              raw={null}
              actions={detail.actions}
              cursors={detail.cursors}
              busy={board.actionBusy !== null}
              onaction={(a, opts) => handleAction(task.id, a, opts)}
            />
          {:else if detail}
            <PendingActions
              actions={detail.actions}
              cursors={detail.cursors}
              pendingType={task.pending_reason?.type}
              disabled={board.actionBusy !== null}
              isBusy={(a, cursorId) => board.actionBusy === actionKey(a, cursorId)}
              onaction={(a, opts) => handleAction(task.id, a, opts)}
            />
          {:else}
            <!-- 详情没到（拉取失败）：只给去看详情的路，不假装动作集是空的 -->
            <button type="button" class="btn" onclick={() => router.navigate(`/task/${task.id}`)}>
              打开任务详情
            </button>
          {/if}

          {#if board.actionError && board.actionBusy === null}
            <div class="ctx err">{board.actionError}</div>
          {/if}

          {#if foldable}
            <button
              type="button"
              class="expander"
              aria-expanded={open}
              onclick={() => void toggleStop(task.id)}
            >
              收起 ▾
            </button>
          {/if}
        {:else}
          <!-- 摘要条（约 36px）：两张以上急停时未展开的那些仍留在第一屏上——**默认态就是
               「都看得到」**。这一行三块：琥珀的「等你拍板 + 类型」、「标题 · N 个动作」、
               展开钮。动作个数是「动作集仍在后端下发、仍在这一轮里」的可见证据；详情没到
               时不报数（不把「还没读到」说成「没有」——决策 182①） -->
          <div class="srow">
            <!-- 「急停」这个词的**首现平实说法**落在这里（决策 218 修订 ⑦b：原先在窄屏那条
                 空态里，而那条空态在折行档整块退场了）。**只在本页第一张上给括号**，
                 同一页面内不重复——译文跟着词走：没有急停则词不在，也就没有没被翻译的词。 -->
            <span class="dtag"
              >⏸ 急停{first ? '（等你拍板的阻塞）' : ''} · {pendingLabel(task.pending_reason)}</span
            >
            <!-- 类名刻意不叫 `st`：那是 app.css 里的全局状态标记（`状态文字标记`，
                 `::before` 自带一枚 8px 方灯 + `white-space: nowrap`）。撞名的代价是
                 摘要条上凭空多一枚无意义的灯、且标题不换行——长标题会压到展开钮上
                 （430px 实测：`.st` 宽 122px、文字 160px，两者重叠 28px） -->
            <span class="stt">
              「{task.title}」{#if count !== null}· {count} 个动作{/if}
            </span>
            <button
              type="button"
              class="expander"
              aria-expanded={open}
              onclick={() => void toggleStop(task.id)}
            >
              展开恢复动作 ▸
            </button>
          </div>
        {/if}
      </article>
    {/each}

    {#if pending.length === 0 && !folded}
      <!-- 空态与其余六处同一套语汇（票 13 / parallel-brief §三.3）：状态 → 下一步（入口由
           决策 240 摘除——原先空看板时给一条「去看板新建任务」，而看板已经是顶栏的一枚页签；
           「去哪」交给那一行，本页不再自带一条）。
           `货箱` 的译文就在这一句里（本页首现处，同一页面内不重复）。
           **折行档不渲染它**（决策 218 修订 ⑦b：整块退场，约 65px）；「急停」的首现平实说法
           随之搬到摘要条的琥珀标签上（见上），故这里只留原词。 -->
      <div class="no-stop">
        <EmptyState
          state="当前没有急停。"
          next={board.projects.length === 0
            ? '这台机器还没接入项目——先接一个，流水线才有货箱（一张任务卡）。'
            : '有任务需要你拍板时，它会挂在这里，动作就在那一轮里。'}
        />
      </div>
    {/if}
    {/if}
  </section>
  {/if}

  <!-- ── 对话时间线：值班长的话、值班经理的话、工位回执。桌面它是那个会滚、会长的地方；
       折行档整页去滚，它就是页面本身（§5 移动款 / 决策 192；档位由决策 218 修订 ⑥ 扩到 ≤899）。
       班次行**不在这里**（决策 218 ②）：桌面在页头右端、折行档在页头的 ⋯ 里——它原先长在
       这个滚动容器里面，于是随对话上移。 ── -->
  <section
    class="timeline"
    class:empty={timelineEmpty}
    bind:this={timelineEl}
    onscroll={onTimelineScroll}
    aria-label="对话时间线"
  >
    {#if loading && !session}
      <div class="quiet">正在读会话台账…</div>
    {:else if turns.length === 0}
      {#if watchMode}
        <!-- 值守账的空态（票 04）：`值守轮` 的**首现平实说法**落在这里（决策 200，
             与人的账那一句同一手法）——本页其余地方保持原词，同一页面内不重复。 -->
        <EmptyState
          state="值守台账还是空的。"
          next="值守轮（不用人开口、自己定期醒来看一圈的那一轮）有该说的事时，账就记在这里。这本账只读——想跟它说话，回对讲台。"
        />
      {:else}
        <!-- 空态：状态 → 下一步（票 13 / parallel-brief §三.3）。`值班长` 的**首现平实说法**
             落在这里（决策 200）：页头那一行是标题栏、不承载翻译，而这一句正是第一次进这一页的
             人读到这个词的地方——本页其余地方（名牌、状态区）保持原词，同一页面内不重复。 -->
        <EmptyState
          state="还没有对话。"
          next="说一句，值班长（跟我对话的 AI）就在对面——它与任务无关，空班也答得上。"
        />
      {/if}
    {/if}

    {#each turns as turn (turn.key)}
      {#if turn.proposal}
        {@const p = turn.proposal}
        {@const st = proposalState(p, now)}
        {@const actionable = proposalActionable(p, now)}
        {@const pointer = proposalPointerOnly(p, actionsFor(p))}
        <!-- 提议轮（决策 188 / 207，票 03）：**内联在时间线的那一轮里**，不新开第三个按钮面。
             类名刻意**不叫** `.warn`（决策 203：全站唯一的「响」仍是急停轮）——它是操作台
             记的一笔账 + 一颗等人按的钮，不是一次告警。 -->
        <article class="turn prop" class:grey={st !== 'pending'} data-proposal={p.id}>
          <!-- 名牌是发言者：这一轮是操作台在报「有一件事等你按键」，不是值班长在说话
               （与急停轮同一条口径：值班长的话一律没有按钮）。 -->
          <div class="dname">操作台</div>
          <div class="dtag">
            {#if st === 'pending'}⏳{/if}
            {proposalShortLabel(p, now)} · {proposalToolLabel(p, toolLabels)}
            {#if st === 'pending'}· <span class="pleft">{proposalRemainingLabel(p, now)}</span>{/if}
          </div>
          <!-- 摘要与提问同值班长回话的待遇（票 05 of talk-live-identity）：作者是模型，
               「它不写 markdown」是无法执行的纪律——原样插值只会把 `**` 与反引号吐给人。 -->
          <MarkdownView source={p.summary} />
          {#if p.stopped_round}
            <!-- 来路（决策 294 / 票 09）：这一条提在一轮**被按停**的话里——那一轮的结论是
                 半截的，按之前值得多看一眼。与「过期只让按钮变灰」同一条口径：不改它能不能按，
                 只多一句读数。 -->
            <p class="dim note">这条提议来自一轮被你按停的话（那份结论没说完），按之前多看一眼。</p>
          {/if}
          {#if isRepairProposal(p)}
            <!-- 修复提议（票 12）：人要看的是那份补丁，不是参数摘要。
                 闸门读数先说「有没有补丁」——没过闸门时**根本没有** diff（决策 210④），
                 不说清会被当成加载失败。diff 可展开、可复制，不做语法高亮。 -->
            {@const gateLabel = repairGateLabel(p)}
            {#if gateLabel}
              <p class="dim note" class:ferr={!p.payload?.gate_passed}>{gateLabel}</p>
            {/if}
            {#if p.payload?.diff}
              {@const diff = p.payload.diff}
              <Fold
                chrome="bare"
                row="inline"
                class="rcpts"
                open={patchOpen[p.id] ?? false}
                ontoggle={(e) => togglePatch(e, p.id)}
              >
                {#snippet summary()}补丁 <span class="dim">{p.payload?.diff_stat ?? ''}</span>{/snippet}
                <pre class="diff-body" data-repair-diff={p.id}>{diff}</pre>
              </Fold>
            {/if}
          {:else}
            <!-- 参数原样可见：按键之前要看得出它到底要什么（后端生成的那句话是摘要，不是全部） -->
            <details class="pargs">
              <summary class="dim">参数 ▸</summary>
              <pre class="mono">{JSON.stringify(p.args, null, 2)}</pre>
            </details>
          {/if}

          {#if st === 'executed' || st === 'rejected'}
            <!-- 终态：两颗钮都收掉，这一轮仍在（审计）。**先判终态再判指路**——反过来的话，
                 一条已经执行过的提议只要那个动作还在动作集里，就会继续显示「去那里按」。 -->
            <p class="dim note">这条提议{proposalStateLabel(p, now)}，不再可按键。</p>
          {:else if watchMode}
            <!-- 只读账上的提议轮（票 04；防御性：值守轮的工具被 deny 清单挡着，提议
                 理论上不落在这本账）：不画钮，处理在对讲台。 -->
            <p class="dim note">这本账只读——要处理这条提议，去对讲台。</p>
          {:else if pointer}
            <!-- 同一个动作已经在状态区那张急停轮里有一颗钮 → **只指路，不画第二颗**
                 （决策 207③，保留决策 176④ 的原顾虑：两处各一颗钮会让「哪颗是真的」
                 变成使用者必须思考的问题，而它们是同一个端点）。 -->
            <p class="dim note">
              这件事的钮在状态区那张急停轮里——同一个动作不摆第二颗，去那里按。
            </p>
          {:else}
            <div class="pacts">
              <button
                type="button"
                class="btn solid"
                disabled={!actionable || proposalBusy !== null}
                onclick={() => void actOnProposal(p.id, 'execute')}
              >
                {proposalBusy === p.id ? '执行中…' : repairActionLabel(p)}
              </button>
              <button
                type="button"
                class="btn quiet"
                disabled={!actionable || proposalBusy !== null}
                onclick={() => void actOnProposal(p.id, 'reject')}
              >
                拒绝
              </button>
            </div>
          {/if}

          {#if st === 'expired'}
            <!-- 过期只让按钮变灰，**那一轮留在时间线**（审计：它当时提议过什么必须可追溯）。
                 这一句只说「为什么按不动、下一步怎么办」——完整说法在 `dtag` 上，两处都写全
                 会让同一句话在同一轮里读两遍。 -->
            <p class="dim note">
              按钮已灰：有效期过了——当时的情况未必还成立，要做得请值班长重新提一次。
            </p>
          {/if}

          {#if proposalError?.id === p.id}
            <p class="note ferr">按下没成：{proposalError.text}</p>
          {/if}
        </article>
      {:else if turn.ask}
        <!-- 提问轮（决策 265，第三种轮型）：**选项钮住在自己这一轮里**——回话轮零按钮的
             票 04 断言原样不动（`talk.spec.ts` 的 reply count 0）。时间线的钮位自此两处：
             提议确认钮（207③）与这里的选项钮（265）；名牌沿提议轮口径挂「操作台」——
             这一轮在等一个选择，选项钮不是值班长回话的一部分。 -->
        <article class="turn ask" class:grey={turn.askAnswered} data-ask>
          <div class="dname">操作台</div>
          <div class="dtag">{turn.askAnswered ? '已答' : '等你选'}</div>
          <!-- 模型的收口话照常显示（工具指示叫它别复述问题，但短收口是它的自由）；
               问题本身以**结构化字段**为准——两处不互相解析。 -->
          {#if turn.content.trim()}
            <MarkdownView source={turn.content} />
          {/if}
          <!-- 问题本体照回话待遇渲染（票 05 of talk-live-identity，同一条纪律）。 -->
          <div class="ask-q"><MarkdownView source={turn.ask.question} /></div>
          {#if !watchMode}
            <div class="aopts">
              {#each turn.ask.options as option}
                <button
                  type="button"
                  class="btn solid"
                  disabled={turn.askAnswered || sending}
                  onclick={() => answerAsk(option)}
                >
                  {option}
                </button>
              {/each}
            </div>
          {:else}
            <p class="dim note">这本账只读——想答这个问题，去对讲台。</p>
          {/if}
        </article>
      {:else}
        <article
          class="turn"
          class:fm={turn.kind === 'fm'}
          class:mine={turn.kind === 'mine'}
          class:failed={turn.kind === 'failed'}
          class:console={turn.kind === 'console'}
        >
          <!-- 名牌那五个词是一张文案规格，判据住在 `lib/talkTurns.ts::turnName`（决策 252 / 271）：
               值守轮的失败账（`failed` + `proactive`）不叫「发送失败」；值守账按**班次类型**
               写名牌（票 04），不靠 `proactive` 逐行猜。 -->
          <div class="dname">{turnName(turn, ledgerKind)}</div>

          {#if turn.interruptedAt}
            <!-- 已中断（票 03）：进程被杀留下的半截轮——与正常轮**只差这一行**，过程与
                 回话照旧按正常轮渲染（断在哪一步正是排查要的证据；修订决策 223）。时刻是
                 后端给的 `interrupted_at` 字段——界面不自己合成一条「中断轮」，台账那一行
                 就是唯一真相（退役决策 260 裁决③ 的本地合成）。 -->
            <div class="cut" data-interrupted="1">
              已中断 · {formatDateTime(turn.interruptedAt)}
            </div>
          {/if}

          <!-- ── 过程：这一轮**按发生顺序**的一步一步（决策 273）──
               值班长的一轮常态是「先想 → 查台账 → 再想 → 收口」，而此前三份留痕是三个
               各自累积的桶（回话在前、思考与回执在后），顺序整个丢了——用户报的
               「命令执行、思考过程没按实际顺序来」就是它。现在按段序画：推理、中途说出口的
               话、工具调用各是一行，谁先谁后由 `lib/talkTurns.ts` 归好的 `segments` 说了算。

               **整组收在一个受控折叠里**（决策 218 ② 的口径原样）：桌面默认展开
               （过程是这一轮结论的出处，「可追溯性不因对话而丢失」是四条纪律之一），
               折行档默认收起——每轮 30–60px 是长会话里最大的隐性纵向开销。内容一个字不删：
               出处按一下就在，摘要行永远带条数。展开态受控的理由与工位回执逐字相同
               （见 `receiptOpen`）：流式增量反复重渲染同一轮时，人手动展开的那一组不该被打回。 -->
          {#if turn.steps.length > 0}
            <Fold
              chrome="bare"
              row="inline"
              class="rcpts process"
              open={receiptIsOpen(turn.key)}
              ontoggle={(e) => toggleReceipt(e, turn.key)}
              data-steps={turn.steps.length}
            >
              {#snippet summary()}过程 <span class="dim">{stepsSummary(turn.steps)}</span>{/snippet}
              {#each turn.steps as step (step.key)}
                {#if step.kind === 'thinking'}
                  <!-- 推理（决策 244）：**默认收起**，两档都是——它常常比回话本身长一个量级，
                       展开着摆在时间线上会把对话冲垮。摘要在流式期间就说「正在想…」并带出
                       **最新一行原文**（ticker，决策 301：人不用点开就知道它想到哪了），
                       收口后带字数。 -->
                  <Fold
                    chrome="bare"
                    row="inline"
                    class="rcpts think"
                    space={4}
                    open={thinkingIsOpen(step.key)}
                    ontoggle={(e) => toggleThinking(e, step.key)}
                    data-step="thinking"
                  >
                    {#snippet summary()}
                      {#if step.live}
                        正在想… <span class="tick" data-think-tick>{thinkTicker(step.text)}</span>
                      {:else}
                        思考过程 {step.text.length} 字
                      {/if}
                    {/snippet}
                    <pre
                      class="think-body"
                      use:trackThinkBody={step.key}
                      onscroll={(e) => onThinkScroll(e, step.key)}>{step.text}</pre>
                  </Fold>
                {:else if step.tool}
                  {@const r = receipt(step.tool.name, step.tool.argsSummary, turn.briefing)}
                  {@const tool = step.tool}
                  <!-- 工具调用：与「正在发生的工具调用」同一形状（左缘亮度阶 + 无框 = 转述
                       不是发言），只是这里它是**已经发生**的那一步——左缘跟它成没成走：
                       正在查是静的 --pane，查完点亮 --go，没读到（含被拒的越权工具）用 --stop。
                       三种状态的词只有一份：**正在查… / 已读 / 未读到**——落地前后是同一个形状、
                       同一句话（此前实时那一栏说「没查到」、落地那一栏说「未读到」，同一件事两个词）。

                       **收起行就是上面那一行**（决策 301 加展开）：摘要把「谁、查什么、成没成」
                       说全了，完整参数与结果是排查时才要的深度。默认收起（ZCode 的口径）——
                       一次长查读摊开参数会直接盖住半屏对话。 -->
                  <Fold
                    chrome="box"
                    row="flex"
                    class="rcpt live"
                    tone={tool.state === 'running' ? 'pending' : tool.state === 'bad' ? 'bad' : 'go'}
                    space={4}
                    open={toolIsOpen(step.key)}
                    ontoggle={(e) => toggleTool(e, step.key)}
                    title={toolIsOpen(step.key) ? '收起工具详情' : '展开工具详情'}
                    data-step="tool"
                    data-tool={tool.name}
                  >
                    <!-- `title` 只说**动作**，不夹编号（文案纪律）；不给收起行挂
                         `aria-label` 是有意的——那会把「读到了什么、成没成」这句可读名整个换掉，
                         而展开态 `details` 自己就报给辅助技术了。 -->
                    {#snippet summary()}
                      <span class="rcpt-head">
                        <Sprite name={r.sprite} size={10} />
                        <span class="nm">{r.workshop}</span>
                        <span class="dim">{r.label}</span>
                        <span class="dim args">{tool.argsSummary}</span>
                        <span class="rs" class:bad={tool.state === 'bad'}>
                          {tool.state === 'running' ? '正在查…' : tool.state === 'bad' ? '未读到' : '已读'}
                        </span>
                      </span>
                    {/snippet}
                    {#if toolIsOpen(step.key)}
                      <!-- 展开体：参数与结果各一份，纯文本（决策 274：留痕不渲染 markdown——
                           它是模型的排版输出之外的东西，符号拿去做标题/强调会把一句实话画歪）。
                           正文只有展开时才进 DOM：收起行因此与从前逐字一致。 -->
                      <div class="rcpt-more">
                        <div class="rm-label dim">完整参数</div>
                        <pre class="rm-body mono" data-tool-args>{prettyArgs(step.tool.args)}</pre>
                        <div class="rm-label dim">结果</div>
                        {#if step.tool.state === 'running'}
                          <pre class="rm-body mono dim" data-tool-result>执行中…</pre>
                        {:else}
                          <pre class="rm-body mono" data-tool-result
                            >{step.tool.result || '（没有输出）'}</pre
                          >
                        {/if}
                      </div>
                    {/if}
                  </Fold>
                {:else}
                  <!-- 中途说出口的话（决策 273）：它在段序里有自己的位置，故不并进收口那一句。
                       按 markdown 渲染，与回话同一套（它也是值班长的话，只是没在那句上收口）。 -->
                  <div class="narr" data-step="text"><MarkdownView source={step.text} /></div>
                {/if}
              {/each}
            </Fold>
          {/if}

          <!-- ── 回话（收口那一句）──
               只在流式期间用等宽预换行 + 光标（那是**还在往外冒**的字，markdown 结构未必成立：
               半个代码栅栏会被渲染成一段乱码）；收口之后按 markdown 渲染（决策 274）——
               模型的回话本来就带标题 / 列表 / 粗体 / 行内代码，此前一律原样吐成纯文本，
               用户读到的是一堆 `**` 与 `-`。 -->
          {#if turn.streaming}
            <p class="streaming">{turn.content}</p>
          {:else if turn.kind === 'fm'}
            <MarkdownView source={turn.content} class="reply" />
          {:else if turn.kind === 'console' && turn.content}
            <!-- 操作台记的账照值班长的待遇渲染（票 05 of talk-live-identity）：它是
                 后端生成给人读的话，反引号 / 列表同样会出现。传输层的失败报文**不渲染**
                 ——那是协议原文，符号拿去做强调会把一句实话画歪（决策 274 的边界保留）。 -->
            <MarkdownView source={turn.content} />
          {:else if turn.content}
            <p>{turn.content}</p>
          {/if}

          <!-- 归因类别标记（决策 235① / 238）：四类各一个词，显示在那一轮的名牌行上。
               未定位时**什么都不显示**——不编一个假的类别（决策 230 把「没有类别」也算一项
               判据，而画一个「未定位」的标签会让人以为那也是一类）。 -->
          {#if turn.attribution}
            <p class="attr" data-attribution={turn.attribution}>
              <span class="dim">归因</span> · {turn.attribution}
            </p>
          {/if}

          {#if turn.partial}
            <p class="dim note">流断了，上面是已经收到的部分；完整回话会在台账里补齐。</p>
          {/if}

          {#if watchMode && turn.kind === 'fm' && !turn.streaming && turn.content.trim()}
            <!-- 转去对话（票 04）：每条播报的「把这件事带进人的时间线」。只读账上唯一的
                 钮，且它不在这本账里动手——摘录预填进**人的对讲台**的输入坞并聚焦，
                 写口在那边（`talk.watchDraft`，装载时消费）。在飞那一轮不挂（话还没说完）。 -->
            <div class="pacts">
              <button type="button" class="btn quiet" onclick={() => takeToTalk(turn.content)}>
                转去对话
              </button>
            </div>
          {/if}

          <!-- 配对入口（决策 182㉙，票 07）：非回环形态下缺令牌时后端回 403，报文里已经说清
               「这台设备还没配对」。这里补的是**动作**——报文让人知道发生了什么，链接让人知道
               下一步去哪。只在 403 且报文提到配对时出现，普通失败不挂这个出口。
               措辞点名「哪台机器」（决策 189）：见上方读失败路径的同一条注释。 -->
          {#if turn.needsPairing}
            <p class="note">
              配对码只在那台跑服务的电脑本机生成：在那台电脑上（桌面应用窗口，或浏览器里的
              127.0.0.1）打开<a
                class="crumb"
                href="#/share"
                onclick={() => router.navigate('/share')}>手机访问</a
              >页扫码即可——手机上打开这一页是拿不到配对码的。若已添加到主屏幕，从图标打开即是<b
                >独立窗口</b
              >（没有地址栏与底栏）；<b>换过令牌后要重新添加一次</b
              >（图标里记的是当时那条带令牌的地址）。
            </p>
            <!-- 同上方读失败那条：这次启动的地址在本条上也摊开（决策 285）。 -->
            <p class="note">
              本次启动地址：<code>{maskPairValue(launchHref)}</code>——{pairingLaunchNote(
                launchHref,
                getPairingToken(),
              )}
            </p>
          {/if}

          <!-- 工位回执那一块**已经并进上面的「过程」**（决策 273）：同一件事的两个时态此前
               分成两处画（实时的工具条 / 落地的回执表），顺序于是两边都对不上。现在工具调用
               就是段序里的一步，落地前后同一个形状——**内容一个字不删**（工具名 / 工位 /
               参数摘要 / 成没成，四样都在那一行上），只是不再有第二份聚合表。 -->
        </article>
      {/if}
    {/each}
  </section>

  <!-- ── 输入坞：钉底。Enter 发送 / Shift+换行；发送中禁用。
       只读账不渲染它（票 04）——值守台账不出输入坞、不出发送态，说话回对讲台。
       归档班次同样不渲染（票 06）：只读时间线，消息完整可读，但说不了话——归档的
       语义仍是「收起来」，不因为翻回来看一眼就变回活跃（也没有「恢复归档」的口子）。 ── -->
  {#if !watchMode && !archivedOpen}
    <form
      class="typer"
      class:warn={streamStatus === 'error'}
      onsubmit={(e) => {
        e.preventDefault();
        void send();
      }}
    >
      <div class="dname">值班经理</div>
      <!-- 排队发送（票 04 of talk-live-identity，显式修订决策 182㉓「一轮没落地就发不出
           第二句」）：一轮在飞时输入框**解锁**，回车 / 按钮把话送进队列——队列条在下面，
           可编辑可撤回；当前轮收口后自动发出。 -->
      <textarea
        class="input"
        rows="2"
        {placeholder}
        bind:value={input}
        bind:this={typerField}
        onkeydown={onKeydown}
        oncompositionstart={() => composing.start()}
        oncompositionend={() => composing.end()}
      ></textarea>
      {#if currentId && ((talk.queue[currentId]?.length ?? 0) > 0 || talk.queueHeld[currentId])}
        <!-- 排队条：按入队顺序，就地编辑、逐条撤回。扣住（上一轮没回来）时给两个出口，
             不自动照发——「它不会再来」要人看见（票 04 的决议）。 -->
        <ul class="qsend" data-queue={currentId}>
          {#if talk.queueHeld[currentId]}
            <li class="qrow held">
              <span class="dim">
                上一轮没有回来——排队的话等你确认再发（也可以清掉）。
              </span>
              <span class="qacts">
                <button type="button" class="btn solid" onclick={resumeQueue}>继续发送</button>
                <button type="button" class="btn quiet" onclick={clearQueue}>清空队列</button>
              </span>
            </li>
          {/if}
          {#each talk.queue[currentId] ?? [] as qtext, qi (qi)}
            <li class="qrow">
              {#if qedit === qi}
                <input
                  class="input qedit"
                  bind:value={qdraft}
                  maxlength={8000}
                  onkeydown={(e) => {
                    if (e.key === 'Enter' && !e.isComposing) {
                      e.preventDefault();
                      commitQueueEdit(qi);
                    } else if (e.key === 'Escape') {
                      qedit = null;
                    }
                  }}
                  onblur={() => commitQueueEdit(qi)}
                />
              {:else}
                <button
                  type="button"
                  class="qtext"
                  title="点击编辑这条排队的话"
                  onclick={() => {
                    qedit = qi;
                    qdraft = qtext;
                  }}
                >{qtext}</button>
              {/if}
              <span class="qacts">
                <span class="dim">{qi === 0 ? '下一句' : `第 ${qi + 1} 句`}</span>
                <button
                  type="button"
                  class="btn quiet"
                  aria-label={`撤回第 ${qi + 1} 句`}
                  onclick={() => currentId && talk.removeQueued(currentId, qi)}
                >撤回</button>
              </span>
            </li>
          {/each}
        </ul>
      {/if}
      <div class="typer-foot">
        <!-- 这一行**只剩传输层断线**这一件事（决策 218 修订 ⑤ / 220④）：
             - 「值班长正在回话…」**删掉**——流式尾随光标本就在说这件事（`.turn p.streaming`），
               用户裁决「光标跳动已经代表了正在回复，不需要这条提示了」；不新增动画位。
             - 「流断了」**删不得**：`streamStatus === 'error'` 是全页**唯一**的断线告知
               （这个状态在仓里只有这一个消费者），而「实时流断了不能静静不更新」是审计 R2-18
               的既有能力。时间线里那句「流断了，上面是已经收到的部分」是**另一件事**
               （那一轮只收到半截，`turn.partial`）。
             - 于是这一行**空闲时高度 0**：决策 192 当初是用 19.2px 的常驻死白换「坞不上下跳」
               （那句「说的每句话都会记进审计」），撤掉告知之后这笔买卖不划算——而且它只在
               异常态发生。**撤的是告知，不是纪律**：决策 182 §3.3 纪律 4 照旧，审计照旧全量落库。 -->
        {#if streamStatus === 'error'}
          <span class="dim hint">流断了：回话仍会以台账为准补上。</span>
        {/if}
        {#if stopError}
          <span class="dim hint ferr">按停没送到：{stopError}</span>
        {/if}
        {#if stopState !== 'hidden'}
          <!-- 停钮（决策 294 / 票 09）：这一轮在飞时它占发送钮的位置。排队发送（票 04）
               之后两颗钮**可以同时在场**——框里有字时旁边多一颗「排队发送」；
               次级样式（`.quiet`）：按停不是破坏性动作——部分结论、它提的提议、
               已经烧掉的 token 都留着（决策 294），故刻意**不吃** `.danger`
               （决策 195 那一档留给删除/覆盖这类真会丢东西的动作）。 -->
          <button
            type="button"
            class="btn quiet"
            data-stop="turn"
            disabled={stopState === 'stopping'}
            onclick={() => void stopTurn()}
          >{stopButtonLabel(stopState)}</button>
          {#if input.trim()}
            <button type="submit" class="btn solid" data-send="queue">排队发送</button>
          {/if}
        {:else}
          <button type="submit" class="btn solid" disabled={!input.trim()}>发送</button>
        {/if}
      </div>
      <!-- 回底钮**必须是坞的子元素**（决策 301）：它靠 `bottom: calc(100% + 8px)` 钉在坞的上沿，
           而要这样定位就得有一个**定位祖先**——坞正是那一个（`position: relative`，折行档是
           `sticky`，两者都为绝对定位子元素立定位盒）。放到坞外面写成它的兄弟节点，这个百分比
           就改成按**视口**算，按钮会飞到屏幕上方之外（实测 `bottom: 728px`、`y = -37.6`）。 -->
      {#if !following && turns.length > 0}{@render jumpToBottomButton(false)}{/if}
    </form>
  {:else if archivedOpen}
    <!-- 归档班次的坞位（票 06）：留一句实情，免得输入区的空白看起来像「坏了」——
         只读，且说清去处（对话一字不少，说话去活跃的班次）。 -->
    <div class="ro-note dim">已归档的班次只读——说的话都在上面；要接着聊，去没归档的班次。</div>
  {:else if !following && turns.length > 0}
    <!-- 只读账没有坞位，回底钮按**视口**固定（`floating`）：值守账照样在长，
         钉在坞位上的那条路在这里没有东西可钉。 -->
    {@render jumpToBottomButton(true)}
  {/if}

  <!-- ── 班次的重命名与归档（决策 204③：走 Modal，不另造第二套对话框） ── -->
  <Modal
    open={dialog === 'rename'}
    title="给这一班起个名字"
    submitLabel="改名"
    submitting={busy}
    submitDisabled={!titleDraft.trim()}
    onclose={() => (dialog = null)}
    onsubmit={submitRename}
    width={420}
  >
    <label class="field">
      <span>班次名</span>
      <input
        type="text"
        bind:value={titleDraft}
        maxlength={24}
        placeholder="例如：周三夜班"
      />
    </label>
    <p class="dim note">名字只是给这一班贴的标签，改它不动台账里的任何一句话。</p>
    {#if dialogError}
      <p class="ferr note">{dialogError}</p>
    {/if}
  </Modal>

  <Modal
    open={dialog === 'archive'}
    title="归档这一班"
    submitLabel="归档"
    submitting={busy}
    onclose={() => (dialog = null)}
    onsubmit={submitArchive}
    width={420}
  >
    <p>
      归档只是把「{session?.session?.title ?? ''}」从班次列表里收起来。
    </p>
    <!-- 票 04：消息表豁免保留期后，这句不再提「按保留期 30 天到期清理」——照旧那么说
         是与永久语义**相反**的困惑。显式修订决策 204⑦「归档不保护消息，照旧吃保留期」
         与 182④「同一把保留期尺」：归档且豁免之后消息永存。 -->
    <p class="dim note">
      说的话不会被删，也不会到期清理——历史对话永久保留。
      归档后自动切到最近有说话的班次；如果这是最后一个，就新开一班。
    </p>
    {#if dialogError}
      <p class="ferr note">{dialogError}</p>
    {/if}
  </Modal>

  <!-- ── 值班板：**桌面右栏**（折行档整块不渲染，CSS 那条 `display: none` 管着；
       同一份读数在看板 8 列与顶栏灯带上各有一份，这一条是第三份——决策 218 ②）。
       只读账不渲染（票 04）：那一页只剩账，读数在看板与顶栏灯带上都有。 ── -->
  {#if !watchMode}
    <aside class="talk-side crew">
    <div class="reg">
      <div class="reg-head"><span>值班板</span><span class="n">8 工位</span></div>
      <ul class="brows no-scrollbar">
        {#each crew as c (c.key)}
          <li class="brow {crewRowClass(c.state)}">
            <span class="blamp {crewLampClass(c.state)}"></span>
            <span class="bnm">{c.label}</span>
            <span class="bc">{c.count}</span>
          </li>
        {/each}
      </ul>
      <!-- `急停` 的译文在这一页只给一次（决策 200：同一页面内不重复），落点是上面状态区
           那条空态——「当前没有急停（等你拍板的阻塞）」正是这个词最需要被解释的时候。
           有急停挂在屏上时，那一轮自己的标签就是完整的平实读法（「⏸ 等你拍板 · …」），
           本块因此不再重复解释，只留原词。 -->
      <div class="boks">
        在跑的工位会自己往下走，不用追问。<br />
        急停的只能你来按键。
      </div>
    </div>
  </aside>
  {/if}
</main>

<!-- 悬浮回底钮（决策 301）：人上滑之后还源源不断有新内容时，**回底这条路必须摆在眼前**
     ——没有它，人只看得到「它不跟了」，看不到「怎么让它再跟」。两处引用同一份标记：
     坞在时钉在坞的上沿（`floating` 为假），只读账没有坞位、按视口固定（`floating` 为真）。 -->
{#snippet jumpToBottomButton(floating: boolean)}
  <button
    type="button"
    class="btn quiet jump"
    class:float={floating}
    data-jump-bottom
    onclick={jumpToBottom}
  >
    滚动到底部
  </button>
{/snippet}

<!-- ⋯ 班次菜单的三条出口（票 04）：Escape 关得掉（焦点没进过面板时也算）、点面板外面关、
     上下方向键走项。与顶栏那个下拉同一姿态——键盘一律在 `window` 上收。 -->
<svelte:window onclick={menuTrap.onClick} onkeydown={menuTrap.onKeydown} />

<style>
  /* 三分区（票 04）：状态区 / 时间线 / 输入坞自上而下。整页钉在视口内，故时间线是
     唯一会滚的区域——两小时前挂起的急停不会被它顶出视野。 */
  .talk {
    max-width: var(--split-max, 1240px);
    margin: 0 auto;
    padding: 14px 20px 12px;
    display: grid;
    /* 左栏的下限**写在这里**（决策 215）：`minmax(0, 1fr)` 正是「480px 上只剩 82px」那个洞
       的来源——1fr 能缩到 0，而对话列的下限是 420px（右栏 280 的下限配上一条算得出来的不等式：
       `900 − 40(页内边距) − 18(gap) − 280 = 562 ≥ 420`）。 */
    grid-template-columns: minmax(420px, 1fr) var(--dossier-w, 340px);
    grid-template-rows: auto auto minmax(0, 1fr) auto;
    gap: 20px 18px;
    /* 视口 = 顶栏 + 整页 + 底栏区 46px（body 下边距）。顶栏实测约 78–81px
       （46px 铭牌行 + 约 33px 页面导航行 + 2px 下框；项目选择器在场时取上限）。
       **多减一点是刻意的**：宁可让页面矮几像素，也不能让它能滚——整页一旦能滚，
       「急停钉在第一屏」就只剩口头保证。 */
    height: calc(100vh - 88px - 46px);
    /* ── 时间线的下限（决策 208）：三分区是零和的，得先给对话区留出它那一份 ──
       确认钮内联在时间线里（决策 207③），故这一格**必须**放得下一条提议轮的操作行
       （身份标签 + 参数摘要 + 一颗 30px 的钮）。2026-09-17 在 1280×720 实测：
       状态区吃满 46vh（331px）之后时间线只剩 **26px**——恰好是它自己的上下内边距，
       内容区为 0，那颗钮在缝里点不到（`elementFromPoint` 命中的是输入坞的名牌）。
       下面的 46vh 因此改成「46vh，但**先扣掉**留给时间线的这一份」。 */
    --timeline-floor: 160px;
    /* 那个「先扣掉」里除下限之外的其它一切，2026-09-17 在 1280×720 逐块量得：
       底栏区 46 + 顶栏 88（上限）+ `.talk` 内边距 26 + 页头 38 + 三条 20px 行距 60
       + 输入坞 124 = 382.6px（有几块是小数，量到 159.4 才知道）。取 **386**：与上面的
       `height` 公式同一手法——**宁可少给状态区几像素，也不能让对话区掉到下限之下**，
       故实测值向上取整、再让出几像素。估计偏大只会让时间线多十几像素，不会再回到 26px。 */
    --talk-chrome: 386px;
  }
  /* ── 值守台账档（票 04）：只读的一本账 —— 状态区、输入坞与值班板都不渲染，
     时间线独占整个主列。网格行相应收成两行（页头 + 时间线）：不收的话，缺席的两块
     会留下两条空的行与三道 20px 行距——对话区上下各白一条。折行档（≤899）是
     flex 列、缺席的块本来就不占位，无需另写。 ── */
  .talk.ledger {
    grid-template-columns: minmax(420px, 1fr);
    grid-template-rows: auto minmax(0, 1fr);
  }
  .talk.ledger .timeline {
    grid-row: 2;
  }
  .talk-head {
    grid-column: 1 / -1;
    grid-row: 1;
    display: flex;
    align-items: baseline;
    gap: 14px;
    flex-wrap: wrap;
  }
  .tt {
    font-size: 24px;
    font-weight: 400;
    color: var(--text-hi);
    line-height: 1.2;
  }
  .ts {
    display: flex;
    gap: 14px;
    color: var(--text-3);
    align-items: baseline;
    flex-wrap: wrap;
  }
  .sep {
    color: var(--text-4);
  }
  /* 当前班次名与它后面那个分隔符只属于折行档的页头那一行（票 03）：桌面款不显示
     ——那里班次行自己挂着每一班的名字，再说一遍是重复。折行档的规则在下面的 899 块里。 */
  .sess-name,
  .sess-sep {
    display: none;
  }
  /* `crumb` 是 app.css 里的**共用基元**（台账三页的「← 看板」面包屑，`.crumb` 带
     `display: inline-block; margin-bottom: 10px`）。本页只借它的颜色档：那一份 10px 下边距
     是给「面包屑独占一行、下面还有标题」的版面留的，落在这里一头扎进页头**那一行**——
     flex 行盒量的是**外边距盒**，于是 19.44px 的字撑出 29.44px 的行（决策 218 ④ 要一行
     19.2px，`talk.spec.ts` 的折行档页头用例钉着它）；散在正文里的两条（配对说明）也会被
     它顶高一个半行。故这里显式归零。 */
  .crumb {
    color: var(--text-3);
    margin-bottom: 0;
  }
  .crumb:hover {
    color: var(--text-hi);
  }
  /* 页头元信息行里的那两条 crumb（票 04）：与 `.ts > span` 同一口径——不参与折行、
     不被压窄（折行档那行是 nowrap + overflow hidden，被压窄的只有班次名）。 */
  .ts .crumb {
    flex: none;
    white-space: nowrap;
  }

  /* ── 状态区：钉在第一屏。上限留出时间线与输入坞的位置，超高时自己滚（急停永不消失） ── */
  .zone-status {
    grid-column: 1;
    grid-row: 2;
    /* 上留白给压在框沿上的名牌 tab；右留白给急停那轮的 4px 硬投影
       （纵向一滚，横向的可见溢出会退化成 auto，不留白就多一条横向滚动条） */
    padding: 20px 6px 4px 2px;
    /* **两个上限取其小**（决策 208）：46vh 是「状态区不能吃满整页」，而
       `100vh - 其它一切 - 时间线的下限` 是「吃满之前先给对话区留下它的那一份」。
       矮窗口（视口高 < 约 930px）里生效的是后者；高窗口里仍是 46vh，逐像素不变。
       代价是零和的，如实记在 §3.3：矮窗口里状态区自己滚得更多（那本来就是它让位的方式）。 */
    max-height: min(46vh, calc(100vh - var(--talk-chrome) - var(--timeline-floor)));
    overflow-y: auto;
  }
  .no-stop {
    color: var(--text-3);
    line-height: 1.8;
    display: flex;
    align-items: center;
    gap: 14px;
    flex-wrap: wrap;
  }

  /* ── 时间线：唯一会滚的区域 ── */
  .timeline {
    grid-column: 1;
    grid-row: 3;
    min-height: 0;
    overflow-y: auto;
    padding: 20px 2px 6px; /* 上留白同上（名牌 tab） */
  }
  /* 空态**居中**（票 13）：空的时候这一格照样占满余下的高度（grid 的 1fr / 窄屏的
     `flex: 1 0 auto`），内容若顶着上沿，就成了「内容浮在顶上、输入坞隔着大片空白」。
     空态是这一格里唯一的内容，居中即「不浮在顶上」。 */
  .timeline.empty {
    display: flex;
    flex-direction: column;
    justify-content: center;
  }
  /* 空态里那两样东西**不是一类**：班次行是控件，空态文案是内容——而班次行已随决策 218 Q15
     搬进页头，这一格里只剩空态文案，故 `justify-content: center` 直接就是「不浮在顶上」。 */
  .timeline.empty > :global(.empty) {
    margin-top: auto;
    margin-bottom: auto;
  }

  /* ── 班次行（决策 204③；决策 218 Q15 把它从时间线里搬到这里）──
     词汇照抄任务详情页的 `.runchip`：「选一条会话」在那里已经有现成形状，本页不另造一套药丸。
     **它现在挂在页头那一行的右端**，故：`flex: 1 1 auto` 吃掉标题与元信息之后的余量、
     `min-width: 0` 是能被压窄的前提（否则内容宽就是它的下限，页头会被撑高）、
     `nowrap` + `overflow-x: auto` 让多出来的班次在**容器内**横滚（与 `.slots` / `.navbar`
     同一手法，`no-scrollbar` 已在标记上）、`align-self: center` 保证它不参与基线对齐
     ——页头的高度仍由 `<h1>` 那一行决定（`--talk-chrome: 386px` 里那个「页头 38px」因此不动）。 */
  .runrow {
    display: flex;
    gap: 6px;
    flex: 1 1 auto;
    min-width: 0;
    flex-wrap: nowrap;
    overflow-x: auto;
    justify-content: flex-end;
    align-self: center;
    margin-bottom: 0;
  }
  .runchip {
    flex: none;
    font-size: 12px;
    padding: 2px 8px;
    border: 2px solid var(--pane);
    color: var(--text-3);
    background: none;
    max-width: 26ch;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .runchip:hover:not(:disabled) {
    color: var(--text);
  }
  .runchip.now {
    background: var(--wash);
    color: var(--text-hi);
    border-color: var(--text-2);
  }
  /* 「+ 新班次」是这里的主动作：亮一档，但仍不占信号色（全站唯一的响在急停一处） */
  .runchip.plus {
    color: var(--text-hi);
  }
  /* 改名 / 归档：同一排但不是「选一条班次」，故压暗一档（次级必读，决策 195） */
  .runchip.act {
    color: var(--text-3);
  }
  .runchip:disabled {
    opacity: 0.5;
  }
  /* 已归档的班（票 06）：灰一档、描边降一档——与活跃班次一眼可辨。选中时照样是灰的
     （「归档」说的是它的身份、「now」说的是选中，两件事正交：灰不许因为选中而退场，
     「打开归档班次不把选中项弄脏」的可见一半就在这里）。 */
  .runchip.arch {
    opacity: 0.55;
    border-color: var(--text-3);
  }
  .runchip.now.arch {
    opacity: 0.8;
    background: var(--wash);
    border-color: var(--text-2);
  }
  /* 「显示已归档」开关（票 06）：按下态借 `.now` 那档 wash 表示「开着」——
     它是开关不是班次，故只借底色，不借「选一条班次」的边框语义。 */
  .runchip.arch-toggle.on {
    background: var(--wash);
    color: var(--text-hi);
    border-color: var(--text-2);
  }
  .quiet {
    color: var(--text-3);
    line-height: 1.8;
    max-width: 78ch;
  }

  /* ── 单轮发言：操作台对话框本体（双线框 + 名牌 tab 即发言者） ──
     `.dname` 名牌在 PendingDossier 里是局部样式、未进 app.css，故本页自带一份，
     取值与 §3.3 / 原型逐字一致。 */
  .turn {
    position: relative;
    margin: 26px 0 0;
    padding: 10px 12px 11px;
    background: var(--bg);
    border: 2px solid var(--pane);
    box-shadow:
      inset 0 0 0 2px var(--bg),
      inset 0 0 0 4px var(--pane);
  }
  .turn:first-child {
    margin-top: 0;
  }
  .turn p {
    color: var(--text);
    margin-bottom: 7px;
    max-width: 76ch;
    overflow-wrap: anywhere;
  }
  /* 时间线里的话保留模型自己的换行（回话是排版好的文本，不是装饰） */
  .timeline .turn p {
    white-space: pre-wrap;
  }
  .turn p:last-child {
    margin-bottom: 0;
  }
  /* 归因类别（决策 235①）：一行小字，跟在正文之后。像素纪律照旧——只用既有 token，
     不加圆角、不加投影（决策 169 的基元表里没有「徽章」这一档，故它就是一行字）。 */
  .turn .attr {
    margin-top: 6px;
    font-size: 12px;
    letter-spacing: 0.04em;
    color: var(--text);
  }
  .dname {
    position: absolute;
    top: -16px;
    left: 6px;
    background: var(--bg);
    border: 2px solid var(--pane);
    color: var(--text-hi);
    padding: 0 8px;
    line-height: 1.5;
    white-space: nowrap;
  }
  /* 值班经理的话与值班长的话只差名牌停靠与明暗档：不换底色、不换圆角、不加箭头（§3.3 纪律 1） */
  .turn.mine .dname {
    left: auto;
    right: 6px;
    color: var(--text-2);
  }
  .turn.mine p {
    color: var(--text-2);
  }
  /* 操作台记的一轮（提议的执行结果，决策 207）：名牌与正文都收一档，形状与值班长的话相同
     ——它是同一张操作台在记账，不是第四种对话框。**不占琥珀、不加 ▼**（全站唯一的响在急停） */
  .turn.console .dname {
    color: var(--text-3);
  }
  .turn.console p {
    color: var(--text-2);
  }
  /* 发送失败：走失败红一档，仍不占琥珀（全站唯一的响只在急停） */
  .turn.failed {
    border-color: var(--stop);
  }
  .turn.failed .dname {
    border-color: var(--stop);
    color: var(--stop);
  }
  .turn.failed p {
    color: var(--stop);
  }
  /* 已中断（票 03）：与失败灯同一个 token——它同样是「这一轮没跑完」，不另造色。
     但**整轮不染红**：它与正常轮只差这一行标记（同构是票面的验收项），过程与回话
     照旧按正常轮读。 */
  .turn .cut {
    color: var(--stop);
    font-size: 12px;
    line-height: 1.6;
    margin: 2px 0 4px;
  }
  /* ── 提议轮（决策 188 / 207，票 03）：操作台记的一笔账 + 一颗等人按的钮。
     **不占琥珀、不加 ▼、不加硬投影**——决策 203 把「全站唯一的响」留给急停轮，
     一次「可以这么做吗」的询问不是告警；这里只把边框换成正文色，与普通发言轮分开。
     类名刻意不叫 `.warn`：那是急停轮的既有断言（`talk.spec.ts` 的 `.turn.warn` 计数）。 ── */
  .turn.prop {
    border-color: var(--text-3);
  }
  .turn.prop .dname {
    border-color: var(--text-3);
  }
  .turn.prop .dtag {
    color: var(--text-2);
  }
  /* 终态（执行过 / 被拒绝 / 已过期）整轮压暗一档：它仍在时间线上（审计），但不再是待办 */
  .turn.prop.grey p {
    color: var(--text-3);
  }
  .turn.prop.grey .dname {
    color: var(--text-3);
  }
  .pleft {
    color: var(--text-2);
  }
  .pacts {
    display: flex;
    gap: 8px;
    margin-top: 4px;
  }
  /* 参数原文（`<details>` 收起）：按键之前要看得出它到底要什么 */
  .pargs {
    margin-bottom: 7px;
  }
  .pargs summary {
    cursor: pointer;
  }
  .pargs pre {
    margin: 6px 0 0;
    padding: 6px 8px;
    background: var(--pane);
    color: var(--text-2);
    overflow-x: auto;
    white-space: pre;
  }
  /* 修复提议的补丁正文（票 12）：**看得全、能复制**——不要求语法高亮，
     但要求横向可滚、用户能选中全文。等宽是唯一的形式要求。
     字号随全站的像素纪律（12 的整数倍，`css-parity.test.ts` 钉着）——11px 那条
     是凭手感写的，被闸门挡下是它该做的事。 */
  .diff-body {
    margin: 0;
    max-height: 320px;
    overflow: auto;
    font-family: var(--font-mono, ui-monospace, SFMono-Regular, Menlo, monospace);
    font-size: 12px;
    line-height: 1.5;
    white-space: pre;
    user-select: text;
  }

  /* 提议操作失败的说明：走失败红，与 `send()` 的失败轮同一档 */
  .turn.prop .ferr {
    color: var(--stop);
  }

  /* ── 提问轮（决策 265）：操作台等一个选择，选项钮住在这一轮里。
     框色比提议轮再深一档（--text-2 vs --text-3）——两种轮型并排时分得开；
     同样**不占琥珀**（决策 203：全站唯一的「响」仍是急停轮），不加 ▼、不加硬投影。 ── */
  .turn.ask {
    border-color: var(--text-2);
  }
  .turn.ask .dname {
    border-color: var(--text-2);
  }
  .turn.ask .dtag {
    color: var(--text-2);
  }
  /* 已答 / 被取代：整轮压暗一档（与 `.turn.prop.grey` 同一条口径）——它仍在时间线上
     （审计：它当时问过什么），但选项不再是待办。 */
  .turn.ask.grey p {
    color: var(--text-3);
  }
  .turn.ask.grey .dname {
    color: var(--text-3);
  }
  .turn.ask.grey .aopts button {
    opacity: 0.6;
  }
  /* 问题句是这一轮的主体，比收口话重一档 */
  .ask-q {
    font-weight: 600;
  }
  /* 选项钮一行排不下就折行（2–4 个短语，窄屏同理） */
  .aopts {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 4px;
  }
  /* ▼ 光标默认不画：只有待拍板那一轮点亮（§3.3 纪律 2） */
  .turn::after {
    content: none;
  }
  .turn.warn {
    border-color: var(--pending);
    box-shadow:
      inset 0 0 0 2px var(--bg),
      inset 0 0 0 4px var(--pane),
      4px 4px 0 var(--ink);
  }
  .turn.warn .dname {
    border-color: var(--pending);
    color: var(--pending);
  }
  .turn.warn::after {
    content: '▼';
    position: absolute;
    right: 6px;
    bottom: 0;
    color: var(--pending);
    font-size: 12px;
    line-height: 1;
    animation: blink 1s steps(2) infinite;
  }
  /* ── 摘要条：两张以上急停时，未展开的那些收成一行（决策 183）。
     只收动作区（那块约 158px 是卡片高度的主要来源），**不收身份**——框色、4px 硬投影、
     ▼ 与展开行完全一致，故「全站唯一的响」在两种形态下是同一个东西，也不新增动画位 ── */
  .turn.warn.folded {
    padding: 6px 12px 7px;
  }
  .srow {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }
  /* 摘要条中间那块「「标题」· N 个动作」。**不叫 `st`**——app.css 的同名全局类会同时
     套上来（`::before` 塞一枚方灯 + `nowrap` 不换行），见标记处的注释。 */
  .stt {
    flex: 1;
    min-width: 0;
    color: var(--text-2);
    overflow-wrap: anywhere;
  }
  /* 展开/收起钮：与「查看产出文件 ▸」同一手法（`.linklike`）——正文色 + 下划线。
     刻意**不长成像素钮**：像素钮是给后端下发动作的，折叠是呈现，两者混同会让人
     以为折叠也在改状态（§3.3 边界） */
  .expander {
    color: var(--text-3);
    text-decoration: underline;
    text-underline-offset: 3px;
  }
  .srow .expander {
    flex: none;
  }
  /* 摘要条里的琥珀标签：与展开行同一个 `.dtag`，只去掉它作为块级首行时的那 8px 下边距
     （摘要条是**一行**，不是「标签一行 + 正文一行」） */
  .srow .dtag {
    flex: none;
    margin-bottom: 0;
  }
  .turn > .expander {
    display: block;
    margin-top: 9px;
  }
  .expander:hover {
    color: var(--pending);
  }
  .dtag {
    color: var(--pending);
    margin-bottom: 8px;
  }
  .ctx {
    color: var(--text-2);
    margin-bottom: 6px;
    line-height: 1.8;
    overflow-wrap: anywhere;
  }
  .ctx b {
    color: var(--go);
  }
  .ctx.err {
    color: var(--stop);
  }
  /* 班次对话框里的字段（与 NewTaskDialog 同一形状：标签在上、输入在下） */
  .field {
    display: block;
    margin-bottom: 10px;
  }
  .field > span {
    display: block;
    font-size: 12px;
    color: var(--text-3);
    letter-spacing: 0.04em;
    margin-bottom: 4px;
  }
  .field input {
    width: 100%;
  }
  .ferr {
    color: var(--stop);
  }

  /* 次级必读档（--text-3，决策 195 的门槛 4.5:1）：本页这一类字（断流后的补给说明、
     输入坞的提示行、收据里的工具名与参数）都属「读不到会挡住下一步」，不是纯装饰刻度，
     故不留 --text-4。纯装饰那几处（`.sep` 的 ▪、`.blamp` 的灯框）另按各自 token 走。 */
  .dim {
    color: var(--text-3);
  }
  .note {
    font-size: 12px;
  }

  /* ── 工位回执的收起行（左缘 4px 亮度阶 = 转述不是发言，与命令输出同一手法）──
     折叠壳（框沿 / 左缘档位 / 底板 / chevron / 清单符）归 `components/ui/Fold.svelte`
     （决策 366），这里只管字段本身。**这层包裹是调用方写的**：Svelte 的作用域够不到
     `Fold` 内部，包一层自己的元素正是让 `nm` / `args` / `rs` 那些子规则照旧可锚的那道缝。 */
  .rcpt-head {
    display: flex;
    align-items: center;
    gap: 7px;
    flex: 1;
    min-width: 0;
  }
  .rcpt-head .nm {
    color: var(--text-2);
    flex: none;
  }
  .rcpt-head .args {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* ── 工具展开详情（决策 301）：完整参数与结果各一份 ──
     上限 320px 与 `.think-body` / `.diff-body` 同一个数：展开的是**深度**，不是版面。
     纯文本、等宽、可选中（决策 274：留痕不渲染 markdown，也不做语法高亮）。 */
  .rcpt-more {
    margin-top: 4px;
  }
  .rm-label {
    font-size: 12px;
    color: var(--text-3);
  }
  .rm-body {
    margin: 2px 0 4px;
    padding: 6px 8px;
    background: var(--pane);
    color: var(--text-2);
    font-size: 12px;
    line-height: 1.5;
    white-space: pre-wrap;
    overflow-wrap: break-word;
    max-height: 320px;
    overflow-y: auto;
    user-select: text;
  }
  .rcpt-head .rs {
    margin-left: auto;
    flex: none;
    /* 「已读 / 未读到」是回执的结论（未读到意味着这条工具被拒），属次级必读：--text-4
       是纯装饰档，不得承载它（决策 195）。失败那一路仍走 --stop。 */
    color: var(--text-3);
  }
  .rcpt-head .rs.bad {
    color: var(--stop);
  }

  /* ── 过程那一组（决策 273）：这一轮按发生顺序的每一步都排在里面 ──
     工具那一步与思考那一步各自带着自己的样式，这里只管组内的间距；推理是嵌套的一层
     折叠块（形状照旧），中途说出口的话是一小段正文。 */
  .narr {
    margin-top: 6px;
    color: var(--text);
  }
  .narr :global(.md) {
    max-width: 76ch;
  }

  /* ── 收口那一句（决策 274）：markdown 渲染，宽度与时间线里的话对齐 ──
     作用域要写 `:global`：那些节点是 `{@html}` 出来的，拿不到本组件的 scoping class。 */
  .turn :global(.md.reply) {
    max-width: 76ch;
  }

  /* ── 思考过程（决策 244）：默认收起，展开后是一段等宽正文 ──
     用 .mono 那一套等宽 + 预换行（它是模型的草稿，markdown 结构未必成立，故不渲染 md）。 */
  .think-body {
    margin: 6px 0 0;
    padding: 6px 10px;
    border-left: 4px solid var(--pane);
    background: var(--panel);
    color: var(--text-2);
    font-family: var(--font-mono);
    font-size: 12px;
    line-height: 1.5;
    white-space: pre-wrap;
    overflow-wrap: break-word;
    max-height: 320px;
    overflow-y: auto;
  }

  /* ── 流式期的推理 ticker（决策 301）：跟在「正在想…」后面的一行原文 ──
     单行 + 溢出省略号，**不用左右渐隐遮罩**：全站禁平滑渐变（`theme/css-parity.test.ts`
     钉着 `linear-gradient` 那一条），而省略号本来就是本页 `.args` 那一栏的既有手法。 */
  .tick {
    display: inline-block;
    max-width: 46ch;
    vertical-align: bottom;
    color: var(--text-4);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* ── 悬浮回底钮（决策 301）──
     坞在时钉在坞的上沿（`bottom: 100%`，绝对定位相对 `.typer` 那个定位盒），
     只读账没有坞位时按视口固定（`.float`）。两处同款，只差锚点。 */
  .jump {
    position: absolute;
    right: 0;
    bottom: calc(100% + 8px);
    z-index: 6;
  }
  .jump.float {
    position: fixed;
    right: 20px;
    /* 桌面款：整页钉在视口里，坞位于底栏（46px 的 `body` 下边距）之上 */
    bottom: 58px;
    z-index: 26;
  }

  /* ── 输入坞：对话框形（双线框），描边 --pane、名牌收 --t3；主动作交给既有实心 ▶ 钮 ── */
  .typer {
    grid-column: 1;
    grid-row: 4;
    position: relative;
    padding: 10px 12px 11px;
    background: var(--bg);
    border: 2px solid var(--pane);
    box-shadow:
      inset 0 0 0 2px var(--bg),
      inset 0 0 0 4px var(--pane);
  }
  /* 归档班次的只读坞位（票 06）：占输入坞那一**格**——只换一句实情，
     版面骨架（网格三行两列）一个数都不动。 */
  .ro-note {
    grid-column: 1;
    grid-row: 4;
    padding: 10px 12px 11px;
    background: var(--bg);
    border: 2px solid var(--pane);
    font-size: 12px;
  }
  .typer .dname {
    color: var(--text-3);
  }
  .typer textarea {
    resize: none; /* 坞的高度是版面的一部分，不让人拖坏三分区 */
  }
  .typer-foot {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-top: 6px;
  }
  .typer-foot .hint {
    flex: 1;
    min-width: 0;
  }
  /* ── 排队条（票 04 of talk-live-identity）：ZCode 式排队发送的可见部分 ──
     入队顺序自上而下，最前面那条是收口后下一句。占位形态贴 `.turn` 的对话框语汇：
     细边框行、次级灰元信息，不新造颜色档（决策 169 / 200 的纪律）。 */
  .qsend {
    list-style: none;
    margin: 6px 0 0;
    padding: 0;
    display: grid;
    gap: 4px;
  }
  .qrow {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 8px;
    border: 2px solid var(--pane);
    font-size: 12px;
  }
  .qrow.held {
    border-color: var(--warn, var(--text-3));
  }
  .qrow .qtext {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-align: left;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-2);
    cursor: text;
  }
  .qrow .qedit {
    flex: 1;
    min-width: 0;
    font-size: 12px;
  }
  .qrow .qacts {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
  }
  .qrow .qacts .btn {
    padding: 2px 8px;
    font-size: 12px;
  }

  /* ── 值班板（复用 .reg 台账盒语汇） ── */
  .talk-side {
    grid-column: 2;
    grid-row: 2 / span 3;
    align-self: start;
    position: sticky;
    top: 48px;
    max-height: 100%;
    overflow: auto;
  }
  .brows {
    list-style: none;
  }
  .brow {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 5px 12px;
    border-bottom: 2px solid var(--wash);
    color: var(--text-3);
  }
  .brow:last-child {
    border-bottom: 0;
  }
  .blamp {
    flex: none;
    width: 8px;
    height: 8px;
    background: transparent;
    border: 2px solid var(--text-4);
  }
  .blamp.c {
    background: var(--go);
    border-color: var(--go);
  }
  .blamp.w {
    background: var(--pending);
    border-color: var(--pending);
  }
  .blamp.d {
    background: var(--done);
    border-color: var(--done);
  }
  /* 失败工位（决策 251①）：与看板同一列的失败红同源，取契约的 `--stop`。 */
  .blamp.x {
    background: var(--stop);
    border-color: var(--stop);
  }
  .brow .bnm {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .brow.hot .bnm {
    color: var(--text-hi);
  }
  .brow.pen .bnm {
    color: var(--pending);
  }
  /* 失败工位（决策 251①）：工位名与灯同色，与看板失败列的做法一致。 */
  .brow.fail .bnm {
    color: var(--stop);
  }
  .brow .bc {
    flex: none;
    color: var(--text-4);
    font-variant-numeric: tabular-nums;
  }
  .boks {
    padding: 10px 12px;
    color: var(--text-3);
    line-height: 1.9;
  }

  .blank {
    padding: 10px 12px;
    border: 2px solid var(--pane);
    color: var(--text-3);
    margin-bottom: 10px;
  }
  .blank.error {
    border-color: var(--stop);
    color: var(--stop);
  }

  /* ── 两栏那一档的右栏宽度（决策 215）：`900–1099` 收到 **280px**，`≥1100` 回到 340px ──
     「先缩右栏、缩到底再折」：右栏是「摘要 + 动作行」，280 仍放得下一行动作钮，而对话列
     因此拿回 60px。`≤899` 那一条也落在这个上界里，但折行档是 `display: flex`，
     `grid-template-columns` 在那里不生效（故不必写 `min-width: 900` ∧ `max-width: 1099`，
     那样只会让断点表多出一个数）。 */
  @media (max-width: 1099px) {
    .talk {
      grid-template-columns: minmax(420px, 1fr) 280px;
    }
  }

  /* ── 折行档（≤899px）：**对话是页面本身**（决策 192 的版面，由决策 218 修订 ⑥ 从 ≤479
     扩到 ≤899——折行档与窄档同源，「钉住的东西必须便宜」那条理由在那里同样成立，而一张
     376px 的展开轮钉在 768px 高的窗口上就是半块屏）──
     桌面款把整页钉在视口里、让时间线做唯一的滚动容器；这一档反过来——整页随手指滚，
     钉住的是**三样**（决策 218 修订 ④：页头 46px 带子 / 急停摘要条 / 输入坞）。
     钉住的部分每多 100px，对话区就少 100px，而对话区正是这一页在这一档唯一的用处。

     **顶栏高度分档且窄档还按路由两档**（≤479：导航行已移到屏幕底部（决策 243），看板只剩
     道具栏行 52px / 其余路由顶栏清零 0px，480–899 是桌面款 78–81px），故一切钉位读
     `--topbar-h`（顶栏自己量出来写回，票 09 / R2-11；0 也是合法值）
     而**不写死一个数**——写死的话换一档摘要条就钉在屏幕中间。 */
  @media (max-width: 899px) {
    .talk {
      display: flex;
      flex-direction: column;
      /* **间距改成逐块外边距**（决策 218 修订 ④）：页头与状态区之间那道通用行距要省掉
         ——钉住的带子自带 2px 下框，再叠一道 14px 是白给。其余几对仍是 14px，坞前那 40px
         （= 14 行距 + 26 外边距）原样保留。用 `gap: 0` + 各自外边距，而不是给页头加负边距：
         状态区在「一张急停都没有」时整块不渲染（决策 218 修订 ⑦b），负边距会把那一对的
         14px 一起吃掉。 */
      gap: 0;
      padding: 10px 12px 0;
      /* 桌面那条 `height` 必须撤掉：两处一起定高，这一档就没有「长出去」的余地 */
      height: auto;
      /* 视口 = 顶栏（实测）+ 整页 + **底盘底边距**。`min-height` 而非 `height`：
         内容长了就长出去（整页滚），短了就撑满余下的屏幕——输入坞于是总在底边。
         底内边距为 0 是**算过**的：让输入坞的底边正好落在底栏顶边（钉住时同一个位置，
         于是「对话短时悬空 16px、长了又贴上去」那种一跳没有了）。
         用 `dvh` 而不是 `vh`：手机上 `vh` 取的是地址栏收起时的高度，地址栏在场时
         整块版面会高出一截，把钉底的输入坞推到屏幕外（`vh` 那行是给不认 `dvh` 的旧内核的）。
         **底盘那一份要按「这一档实际是多少」算**，而 480–899 上一共有两个数：`app.css` 给
         `body` 的仍是桌面那份 **46px**（它到 ≤479 才换成 `--sbar-h`），而输入坞钉的是
         `bottom: var(--sbar-h)`（= 38px）。两者差 8px，各管一头——`min-height` 减 **38**
         才让坞的底边与底栏顶边齐平（实测差 10px 的正是这条），多出来的那 8px 底盘边距由下面
         那条负外边距就地抵掉，文档高度因此仍是视口高（静置态不空滚）。**不去改 `body`
         本身**：底盘边距是全站的（看板、详情页、台账页都在用），组件里一条 `:global(body)`
         会把 480–899 这一档**所有页面**的底边距一起改掉，那不是本页的地盘
         （决策 218 ⑧：其余页面不动；这一处是决策 222 的现场修正）。 */
      min-height: calc(100vh - var(--topbar-h) - var(--sbar-h));
      min-height: calc(100dvh - var(--topbar-h) - var(--sbar-h));
      margin-bottom: calc(var(--sbar-h) - 46px);
    }
    /* ── 页头那一行：**钉住的 46px 带子**（决策 218 修订 ④）──
       `height: 46px` 含那 2px 下框（全站 `box-sizing: border-box`），于是内容高 44px
       ——⋯ 的 44px 触控目标**直接落在行内**，不需要任何溢出技巧（旧 ④ 的「命中区故意溢出
       到留白里」与随之而来的「`.talk` 不得加 `overflow: hidden`」都作废了）。
       `top: var(--topbar-h)`：带子钉在顶栏下沿，急停摘要条再钉在它下沿（+46px）。 */
    .talk-head {
      order: 1;
      flex: none;
      position: sticky;
      top: var(--topbar-h);
      z-index: 15;
      height: 46px;
      align-items: center;
      flex-wrap: nowrap;
      gap: 10px;
      padding: 0 2px;
      background: var(--bg);
      border-bottom: 2px solid var(--hairline);
    }
    /* `<h1>` 转 visually-hidden（决策 218 Q6，先例是看板页）：页签已经在说「对讲台」，
       而这一行 19.2px 的高度要留给真正会变的东西。**语义要留**——每路由一个 h1（R2-20）。 */
    .talk-head .tt {
      position: absolute;
      width: 1px;
      height: 1px;
      overflow: hidden;
      clip: rect(0 0 0 0);
      white-space: nowrap;
    }
    .ts {
      flex: 1 1 auto;
      min-width: 0;
      flex-wrap: nowrap;
      overflow: hidden;
      gap: 8px;
      /* 这一档是**一行**，垂直居中即可。不参与基线对齐是必须的：`.sess-name` 带
         `overflow: hidden`，它的基线由盒子底边合成，`align-items: baseline` 会把整行撑到
         29.4px（决策 218 ④ 要的是 19.2px 的行——量出来的是行盒，不是字）。 */
      align-items: center;
    }
    /* 原先还有 `.ts > a`（页头末尾那颗「看板」面包屑），决策 240 摘除后这一行只剩 `<span>` */
    .ts > span {
      flex: none;
      white-space: nowrap;
    }
    .ts .sess-sep {
      display: inline;
    }
    /* 班次名是这一行里唯一可以被压窄的东西：其余几项都是短定值，压它们只会把字切掉 */
    .ts .sess-name {
      display: inline;
      flex: 0 1 auto;
      min-width: 0;
      max-width: 26ch;
      overflow: hidden;
      text-overflow: ellipsis;
    }
    /* 标题与元信息同一行：这一档「对讲台」的顶栏页签已经在说，页内不必再铺两行。
       「夜班态势：8 工位」也收进桌面款——它说的就是看板列头与顶栏灯带上已有的那份读数。 */
    .stat-wide {
      display: none;
    }
    /* ── ⋯ 与它的班次菜单（决策 218 ②/Q10/Q12/Q13）── */
    .more-wrap {
      position: relative;
      flex: none;
      align-self: stretch;
      display: flex;
      align-items: center;
    }
    .more {
      width: 44px;
      height: 44px;
      display: grid;
      place-items: center;
      background: none;
      border: 0;
      color: var(--text-2);
    }
    /* 三个点用 `::before` 画：装饰不进可访问名（票 06 / R2-19，与页签前缀三角同一手法），
       名字由 `aria-label` 给。 */
    .more::before {
      content: '⋯';
      font-size: 16px;
      line-height: 1;
    }
    .more:hover,
    .more[aria-expanded='true'] {
      color: var(--text-hi);
    }
    /* 面板锚在**钉住带子的下沿**：带子自己钉着，故这是个常量（顶栏实测高 + 46px 带子），
       不必现量。`fixed` + 左右各 12px 与顶栏「待处理」下拉在窄档同一手法（脱离横滚容器的
       裁剪，也让长标题有地方展开）。 */
    .smenu {
      position: fixed;
      left: 12px;
      right: 12px;
      top: calc(var(--topbar-h) + 46px);
      max-height: 60vh;
      overflow: auto;
      padding: 6px 0;
      z-index: 40;
      background: var(--bg);
      text-align: left;
    }
    .smenu[hidden] {
      display: none;
    }
    /* 行高按触控来（≥44px）：**不复用 20px 的 `.runchip` 尺寸**——那一档的芯片是在页头里
       横滚的一排，这里是一份要按得准的菜单。 */
    .mi {
      display: flex;
      align-items: center;
      gap: 8px;
      width: 100%;
      min-height: 44px;
      padding: 0 12px;
      background: none;
      border: 0;
      color: var(--text-2);
      text-align: left;
    }
    .mi:hover:not(:disabled) {
      background: var(--wash);
      color: var(--text-hi);
    }
    .mi:disabled {
      opacity: 0.5;
    }
    /* 「+ 新班次」是这一页最常按的一颗：亮一档，但仍不占信号色（全站唯一的响仍在急停一处） */
    .mi.plus {
      color: var(--text-hi);
    }
    .mi-nm {
      flex: 1;
      min-width: 0;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    .mi-meta,
    .mi-mark {
      flex: none;
      font-size: 12px;
    }
    /* 当前班次是**身份行**：它不是按钮（点了没有去处），选中态与页头那一行的名字是同一个人 */
    .mi.now {
      background: var(--wash);
      color: var(--text-hi);
    }
    /* 已归档的行（票 06）：灰一档——身份行与可点的行同一套灰（与 chip 行同判据）。 */
    .mi.arch {
      opacity: 0.55;
    }
    /* 「显示已归档」开关的按下态（票 06）：借 `.now` 那档 wash 表示「开着」。 */
    .mi.act.on {
      background: var(--wash);
      color: var(--text-hi);
    }
    /* 两枚标记（决策 220③）：**用词而不是纯色块**（决策 195 的次级必读档门槛），
       且不加动画位。「有新动静」比行右端的时间戳亮一档——它是叫你回去看一眼的那一句。 */
    .mi-mark.fresh {
      color: var(--text-2);
    }
    /* 「正在回话」占琥珀档的文字色，但**不加动画**（全站唯一的「响」仍只给急停，决策 220③） */
    .mi-mark.rep {
      color: var(--pending);
    }
    .mi-sep {
      height: 2px;
      margin: 6px 0;
      background: var(--hairline);
    }
    .mi-empty {
      padding: 10px 12px;
      color: var(--text-3);
      font-size: 12px;
    }
    /* ── 值班板：**整块不渲染**（决策 218 ②）──
       同一份读数在看板 8 列与顶栏灯带上各有一份，灯条是第三份；而这一档的纵向空间是零和的。
       桌面右栏 `.talk-side` 一字不动（下面那些 `.brows` / `.brow` / `.blamp` 规则照旧是它的）。 */
    .talk-side {
      display: none;
    }
    /* ── 状态区：钉在**页头那条带子的下沿**（`.stops`） ──
       桌面那条 46vh 上限（区内滚动）在这一档撤掉：整页去滚，状态区不再有自己的滚动条。
       钉住只在**收起时**成立——展开的那一张回到普通文档流（`.stop-open`），否则一张
       376px 的轮钉在 900px 的屏上就钉死了整块屏幕；展开那一张要看得见由 `toggleStop`
       滚回它负责（`scroll-margin-top` 要按带子的下沿算，见下）。 */
    .zone-status {
      order: 2;
      flex: none;
      /* 上内边距归 0（决策 218 修订 ⑦a）：那 20px 是给悬出框沿 14px 的名牌留的净空，
         而摘要条不再画名牌——只有展开那一张（`.stop-open`）才把它拿回来。 */
      padding: 0 6px 6px 2px;
      max-height: none;
      overflow: visible;
      /* `toggleStop` 的 `scrollIntoView` 停的位置：钉住的带子下沿再让 10px 的呼吸
         （= 顶栏实测高 + 46 + 10，决策 218 修订 ④ 把 148 挪过来的那一处）。
         **两种形态都要**——别放进下面 `.stops:not(.stop-open)` 那条规则里：展开那一张恰好
         退出钉住，滚动是唯一能把它带回眼前的东西，而丢了 `scroll-margin-top` 就会**多滚一截**
         （实测整页滚到底 36px、名牌因此钻到钉住带子底下：状态区 158、名牌上沿 164、
         带子下沿 184——正是「点了展开恢复动作，动作却看不见」那种错）。 */
      scroll-margin-top: calc(var(--topbar-h) + 56px);
    }
    /* 展开那一张：名牌回来、那 20px 也回来。展开态本来就退出钉住，是刻意的一刻。 */
    .zone-status.stop-open {
      padding-top: 20px;
    }
    .zone-status.stops:not(.stop-open) {
      position: sticky;
      /* 下沿 = 顶栏实测高 + 46px 带子（决策 218 修订 ④：这个钉位从「顶栏下沿」挪到了
         「带子下沿」）。下边框把这条带子与下面的对话分开。 */
      top: calc(var(--topbar-h) + 46px);
      z-index: 14;
      background: var(--bg);
      border-bottom: 2px solid var(--hairline);
      /* 兜底上限：一张摘要条实测约 **66px**（窄屏两行）+ 14px 间距，五张在 900px 屏上
         就是 400px——钉住的东西不能没有上界，否则「对话区太小」会以另一种形状回来。
         到顶之后这条带子自己滚（与桌面那一档同一手法），代价如实写在 §3.3 的残留里：
         被滚出去的那张不再「一直看得见」。2px 右内边距已在基线上留过（见 `.zone-status`），
         故纵向一滚不会连带长出横向滚动条。 */
      max-height: 45vh;
      overflow-y: auto;
    }
    /* 摘要条在这一档**显式两行**：标签一行，标题与展开钮一行。
       不显式分行、让 flex 自己折的话，430px 上量到的是「标题被压到 122px 宽、
       `· 2 个动作` 断在词中间」（标签 138 + 标题 160 + 钮 90 + 两道间距 = 408px > 370px），
       而标题恰好是这一行唯一要看的东西。 */
    .srow .dtag {
      flex: 0 0 100%;
    }
    .srow .stt {
      flex: 1 1 auto;
    }
    /* 折下来的那颗钮靠右站 */
    .srow .expander {
      margin-left: auto;
    }
    /* ── 对话时间线：这一档它就是页面（没有自己的滚动条，只剩名牌 tab 的上留白） ──
       `margin-top: 14px` 是**它自己带的那一道行距**（页头→状态区那一对被省掉了；
       状态区整块不渲染时，页头与它之间也正好是这一道——见 `.talk` 的 `gap: 0`）。 */
    .timeline {
      order: 3;
      /* 不缩：内容多高就多高——去滚的是整页，不是它；余量归它，输入坞于是贴底 */
      flex: 1 0 auto;
      /* 桌面那条 `overflow-y: auto` 也要撤掉：这一档它一旦成了滚动容器，会长出来的
         是它自己而不是整页（滚轮到底也翻不过去），与这一档的版面整个相反 */
      overflow-y: visible;
      padding: 20px 2px 6px;
      margin-top: 14px;
    }
    /* ── 输入坞：钉在底栏上沿 ──
       一行制：[空心底的输入框][实心发送] 同一行。那一行提示语**整行撤掉**（决策 218
       当日修订 ②）：它原先常驻是为了不上下跳，而现在它只在**断线**时出现——异常态才
       跳一次，比常驻 19.2px 的死白划算（`grid-template-areas` 因此只剩一行）。
       `display: contents` 把桌面的 `.typer-foot` 拆开，两个子元素各归各格——
       桌面款那一套排版因此逐像素不变，不必改标记。 */
    .typer {
      order: 4;
      flex: none;
      position: sticky;
      bottom: var(--sbar-h);
      z-index: 25;
      /* 名牌悬出净空（决策 282 ②，修订 218④ 的「40 = 14 + 26」为「24 = 14 + 10」）：
         悬出量按实测 14px 定（名牌 22px 高、`top: -16px` 相对 padding box，再让 2px
         描边——上一轮 14/16 两个口径之差就是那道边框），呼吸 10px 与 `toggleStop` 的
         `scroll-margin-top` 那道同量；时间线自带的 6px 下内边距计入后，最后一轮正文
         与名牌之间仍有 16px 视觉余量 */
      margin-top: 24px;
      display: grid;
      grid-template-columns: minmax(0, 1fr) auto;
      grid-template-areas: 'field send';
      gap: 6px 10px;
    }
    /* 只读坞位同档处理（票 06）：同序、同钉位、同呼吸——换的只是那一句，不是骨架。 */
    .ro-note {
      order: 4;
      flex: none;
      position: sticky;
      bottom: var(--sbar-h);
      z-index: 25;
      margin-top: 24px;
    }
    /* 悬浮回底钮这一档的锚点（决策 301）：底栏是页签栏（`--sbar-h`），
       dock 在它上面——没坞位时按钮就贴在它上沿，与坞在时同一条视觉基线。 */
    .jump.float {
      right: 12px;
      bottom: calc(var(--sbar-h) + 12px);
    }
    /* 这一档的摘要行比桌面窄得多，ticker 跟着收：它只是「它想到哪了」的一瞥，
       不是给人读推理的地方（要读就点开）。 */
    .tick {
      max-width: 24ch;
    }
    .typer textarea {
      grid-area: field;
      /* 自长 1 行起步（决策 282 ③）：`app.css` 的 `textarea.input` 基线是
         `min-height: 60px`——不归零，1 行仍占 60px，自长就白改。全局基线不动，
         只在这一档归零；行高的下限由发送钮的 44px 触控底线抬住（见下）。 */
      min-height: 0;
    }
    .typer-foot {
      display: contents;
    }
    /* 空闲时这一行**不占高**（决策 218 修订 ⑤ / 220④） */
    .typer-foot .hint {
      display: none;
      grid-area: hint;
    }
    .typer.warn {
      grid-template-areas:
        'field send'
        'hint hint';
    }
    .typer.warn .typer-foot .hint {
      display: block;
    }
    .typer-foot .btn {
      grid-area: send;
      /* 与输入框齐高；自长 1 行起步之后（决策 282 ③）stretch 只有 37.6px——
         触控底线 44px 由 min-height 抬住（网格行高随之抬到 44，坞与钮仍然齐平） */
      align-self: stretch;
      min-height: 44px;
    }
  }

  /* ── 移动款（≤479px，与 `app.css` 的窄屏基线同档）：**只剩排版细调** ──
     版面判据（页头收窄、钉住物、⋯ 菜单、回执默认态）一律走上面那条 899——两档同源
     （决策 218 修订 ⑥）。这里留下的两条都是「每行字数少」引起的，与页宽无关。 */
  @media (max-width: 479px) {
    .turn p {
      max-width: none;
    }
    /* 手机上每行字数少（约 31 个汉字），行距跟上走：1.6 是按桌面约 76 字符一行配的 */
    .timeline .turn p {
      line-height: 1.8;
    }
    /* 这一档 `app.css` 把 `body` 的底边距也换成了 `--sbar-h`（窄档 = 底部页签栏
       `--nav-h`；状态条那一层由决策 300 摘掉，此前是「状态条 42px + 页签栏」，
       决策 243），两个数从此相等——899 块里那条抵差额的负外边距因此归零
       （不改的话会白吃 4px）。 */
    .talk {
      margin-bottom: 0;
    }
    /* 窄档：把「本次会话 N tok」让给页头那枚待办读数（决策 307）。
       `textContent` 仍在 DOM 里（既有 e2e 那条 `toContainText(/本次会话 … tok/)` 不受影响），
       只是不再占这一行的宽度。 */
    .tok {
      display: none;
    }
  }
</style>
