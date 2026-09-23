<script lang="ts">
  import { onMount, tick } from 'svelte';
  import type {
    AllowedAction,
    BranchCursor,
    ForemanBriefing,
    ForemanProposal,
    ForemanSession,
    ForemanSessionMeta,
    ForemanTrace,
    SseEvent,
    Stage,
    TaskListItem,
  } from '../api/types';
  import type { SpriteName } from '../theme/contract';
  import {
    archiveForemanSession,
    createForemanSession,
    executeForemanProposal,
    getForemanSession,
    getForemanSessions,
    getTask,
    rejectForemanProposal,
    renameForemanSession,
    sendForemanMessage,
  } from '../api/client';
  import { board } from '../stores/board.svelte';
  import {
    BOARD_COLUMNS,
    COLUMN_SPRITES,
    formatDuration,
    formatTokens,
    pendingLabel,
    taskDuration,
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
  import { labelFor, loadToolLabels } from '../lib/toolLabels';
  import { formatDateTime } from '../lib/format';
  import {
    TALK_FOLD_QUERY,
    TALK_MOBILE_QUERY,
  } from '../lib/talkLayout';
  import {
    loadSeen,
    loadSessionId,
    markSeen,
    pruneSeen,
    saveSeen,
    saveSessionId,
    seedBaselineIfFirstRun,
    sessionMark,
    type SeenAt,
    type SessionMark,
  } from '../lib/talkSessions';
  import { TaskStream, type StreamStatus } from '../realtime/connection';
  import {
    appendForemanDelta,
    appendForemanTool,
    beginForemanStream,
    emptyForeignActive,
    emptyForemanStream,
    failForemanStream,
    failureNotice,
    failedLedgerRowIds,
    foreignIsReplying,
    forgetForeignActive,
    ledgerOwnsTheFailure,
    noteForeignDelta,
    pruneForeignActive,
    settleForemanStream,
    type ForemanLiveTool,
    type ForemanStreamState,
    type ForeignActive,
  } from '../realtime/foreman';
  import Sprite from '../components/render/Sprite.svelte';
  import Gauge from '../components/render/Gauge.svelte';
  import PendingActions from '../components/board/PendingActions.svelte';
  import DiffReviewPanel from '../components/task/DiffReviewPanel.svelte';
  import EmptyState from '../components/ui/EmptyState.svelte';
  import Modal from '../components/ui/Modal.svelte';
  import { router, writeQuery } from '../router.svelte';

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
   * 自保（前两层是 `appendForemanDelta` 的班次守卫与 `send()` 里的 `generation` 比对）。
   * 「那一轮回话去哪了」改由班次列表里的两枚标记说：**正在回话**与**有新动静**
   * （判据在 `lib/talkSessions.ts` 与 `realtime/foreman.ts`）。
   *
   * **值班长的回复里永远没有按钮**：写动作只在状态区的急停轮里渲染（后端下发的
   * `allowed_actions`，决策 101）。同一个动作在两处各渲染一颗钮，会让「哪个是真的」
   * 变成使用者必须思考的问题。（空态原先还有一颗页面固定的导航钮「去看板新建任务」，
   * 决策 240 随看板页签一并摘除——它不进动作契约，去掉也不动这条边界。）
   *
   * **全站唯一的响仍只在急停一处**：值班长一律收在 `--pane`，不占琥珀档；发送失败走
   * 时间线里的一轮（红），不弹窗、不 toast。
   *
   * **不造聊天组件**：一列对话 = 一叠操作台对话框（`.turn` 即对话框本体，压在框沿上的
   * 名牌 tab = 发言者），故不另加 who 行，也不做左右交替气泡。
   */

  let loading = $state(true);
  let loadError = $state<string | null>(null);
  /** 会话台账（时间线的权威内容；每次回话后重取，不自攒一份账）。 */
  let session = $state<ForemanSession | null>(null);

  /** 未归档的班次（chip 行的数据源），按最近活动倒序。 */
  let sessionList = $state<ForemanSessionMeta[]>([]);
  /** 当前班次 id。**同步更新**，不从 `session` 派生——发送期间要靠它比对在途回包。 */
  let currentId = $state<string | null>(null);
  /**
   * 世代记号：**每次「屏幕上换了班次」就 +1**（切换 / 新建 / 重读时发现服务端换了班）。
   * 在途的 send 结果比对这个记号，不符即丢弃（决策 204⑥）——手机与电脑同时连着时，
   * 别班的回话不得落进这一班的活动轮。
   */
  let generation = $state(0);
  /** 班次操作（改名 / 归档）对话框；`null` = 关着。 */
  let dialog = $state<'rename' | 'archive' | null>(null);
  /** 改名输入框的草稿。 */
  let titleDraft = $state('');
  /** 班次操作的进行中标记（按钮转圈 + 禁用）。 */
  let busy = $state(false);
  /** 班次操作的失败说明（对话框里就地显示，不弹窗）。 */
  let dialogError = $state<string | null>(null);

  let input = $state('');
  let sending = $state(false);
  /** 正在发的那句话（台账里还没有它的回话，故先以乐观轮显示）。 */
  let pendingText = $state<string | null>(null);
  let stream = $state<ForemanStreamState>(emptyForemanStream());
  let streamStatus = $state<StreamStatus>('idle');
  /**
   * 本机刚发出去、还没落地的那一班（决策 220③ 的「正在回话」前半支）。
   *
   * 记 id 而不是一个布尔：切走之后那一轮照旧在路上，标记要落在**它**那一行上，
   * 而不是「你现在看的这一班」。
   */
  let sendingSid = $state<string | null>(null);

  /** 「哪一条我看过」的时刻表（决策 220③ 的「有新动静」判据）。 */
  let seen = $state<SeenAt>({});
  /** 这份表建过基线没有：本机第一次读到列表时把当下当基线（否则第一屏每条都带「有新动静」）。 */
  let seenSeeded = $state(false);
  /** 「别的班次正在回话」：SSE 广播里的 `session_id` 归位（纯前端、刷新即空）。 */
  let foreignActive = $state<ForeignActive>(emptyForeignActive());
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
   */
  let receiptOpen = $state<Record<string, boolean>>({});
  /** 工具回执标签（`GET /foreman/tools`，取数一次缓存，决策 247⑤）。空表 = 还没回来，原样显示工具名。 */
  let toolLabels = $state<Record<string, string>>({});
  const receiptIsOpen = (key: string) => receiptOpen[key] ?? !folded;

  /**
   * 「它想了什么」折叠块的展开态，按 `turn.key` 记（决策 244）。
   *
   * **与工位回执分开一份、且两边默认值相反**：回执是这一轮结论的出处（桌面默认展开），
   * 而思考是**过程的草稿**——长会话里它往往比回话本身长一个量级，默认展开会把时间线
   * 冲垮。故它**两档都默认收起**，人想看再点开。
   *
   * 受控的理由与 `receiptOpen` 逐字相同：`<details open>` 交给浏览器管的话，
   * 流式增量反复重渲染同一轮时会把使用者手动展开的那一块打回收起。
   */
  let thinkingOpen = $state<Record<string, boolean>>({});
  const thinkingIsOpen = (key: string) => thinkingOpen[key] ?? false;

  function toggleThinking(e: MouseEvent, key: string) {
    // 阻止默认的 `open` 翻转，改由状态说了算（与 `toggleReceipt` 同一手法）。
    e.preventDefault();
    thinkingOpen = { ...thinkingOpen, [key]: !thinkingIsOpen(key) };
  }

  /** 每个 pending 任务的详情（allowed_actions 只在详情里下发，决策 101）。 */
  let details = $state<Record<string, { actions: AllowedAction[]; cursors: BranchCursor[] }>>({});

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

  const pending = $derived(board.pendingTasks);

  /**
   * 地址里那一班（`?session=`，决策 217①：「我在哪」进 URL）。**它才是权威**——
   * 前进 / 后退因此能回到上一班，刷新也照地址恢复；没有它才轮到 localStorage 兜底
   * （跨页面回来时地址会丢参数，而「我一直在看这一班」不该因此被重置）。
   */
  const urlSession = $derived(
    router.route.name === 'talk' ? (router.route.query.session ?? null) : null,
  );

  /** 折行档里班次列表的那些行（当前那一班在下一条里单独渲染成身份行）。 */
  const otherSessions = $derived(sessionList.filter((s) => s.id !== currentId));

  /**
   * 展开的那张急停（决策 183）。`undefined` = 还没选过（跟随默认：**宽屏只有一张时展开它，
   * 两张以上一张都不展开；窄屏恒不展开**）、`null` = 人显式收起、否则是那张的 id。
   * 三态的判据在 `lib/talkStops.ts`，此处只持状态。
   */
  let chosenStop = $state<string | null | undefined>(undefined);
  const stopIds = $derived(pending.map((t) => t.id));
  const foldable = $derived(isFoldable(stopIds, folded));
  const openStop = $derived(resolveOpenStop(stopIds, chosenStop, folded));

  /** 8 工位的值班灯：按列聚合，与看板列头同一套词表与 sprite（`BOARD_COLUMNS`）。 */
  const crew = $derived(
    BOARD_COLUMNS.map((col) => {
      const tasks = board.tasks.filter((t) => col.stages.includes(t.current_stage));
      const pen = tasks.some((t) => t.status === 'pending');
      const live = tasks.some((t) => t.status === 'running');
      const don = tasks.length > 0 && tasks.every((t) => t.status === 'done');
      return {
        key: col.key,
        label: col.label,
        sprite: COLUMN_SPRITES[col.key],
        count: tasks.length,
        state: pen ? 'warn' : live ? 'run' : don ? 'done' : 'idle',
      };
    }),
  );

  interface TurnView {
    key: string;
    /**
     * 发言者。`console` = **操作台记的一轮**（`role === 'system'`：提议的执行结果，决策 207）。
     *
     * 它必须与 `fm`（值班长的话）分开：那一行的内容是「提议已执行：…」，而**动手的是按下
     * 那颗钮的人**——挂在值班长的名牌下等于替它认领了它没做的事。三种角色、两种说话的立场，
     * 操作台是第三种。
     */
    kind: 'fm' | 'mine' | 'failed' | 'proposal' | 'console';
    content: string;
    /** 排进时间线的时刻（RFC3339）。三种在途轮（乐观轮 / 流式轮 / 失败轮）没有它，恒在末尾。 */
    at: string;
    /** 流式尾随方块光标（既有 `.streaming`，不新增动画位）。 */
    streaming: boolean;
    /** 断流/出错：这一轮只有已收到的部分。 */
    partial: boolean;
    /** 该轮工具痕迹（台账查读），空数组 = 这一轮没翻台账。 */
    traces: ForemanTrace[];
    /**
     * 这一轮的**推理 / 思考**原文（决策 244）。`null` = 这一轮没产推理（多数模型如此）。
     *
     * 两种来源：在途轮来自实时流（`stream.thinking`），落地轮来自台账那一行的
     * `thinking` 列——**同一份内容的两个时态**，界面只渲染它，不关心哪来的。
     */
    thinking: string | null;
    /**
     * 这一轮**正在发生**的工具调用（决策 244，只在途轮非空）。
     *
     * 与 `traces`（落库那一份）分工：`traces` 说「这一轮查过什么」（轮次结束后才有），
     * 这里说「此刻在查什么」。落地之后这一栏就空了——那时 `traces` 已经把同一件事说完。
     */
    liveTools: ForemanLiveTool[];
    briefing: ForemanBriefing | null;
    /** 失败原因是「这台设备还没配对」（票 07）：只有它会挂出配对入口。 */
    needsPairing: boolean;
    /** 提议轮带的那条提议（其余轮为 `null`）。 */
    proposal: ForemanProposal | null;
    /**
     * 这一轮是**主动播报**（值守轮自己醒来说的话，票 06）。
     *
     * 与「回话」分开渲染的理由不是好看：回话是有人问的，播报是它自己说的——
     * 混成一种轮会让「它是不是在跟我说话」变成读不出来的一件事。
     */
    proactive: boolean;
    /**
     * 这一轮的归因类别词（决策 235①）：四类之一，由**后端解析**后随消息下来。
     *
     * `null` = 未定位或非助理轮——**不编一个假的类别**（决策 230 把「没有类别」也
     * 当成一项判据）。界面只渲染这一个词，不显示稳定标识、也不显示原因（那是排查面）。
     */
    attribution: string | null;
  }

  /**
   * 时间线归约（票 03 加一种轮：**提议轮**）。
   *
   * 提议与消息**按时刻合并排序**，不是把提议另起一段：提议是那一轮里发生的事
   * （模型调用 → 落成提议 → 它把话说完），先后次序本身是信息。三段的时刻天然分得开——
   * 值班经理的话先落库，提议在工具调用时落库，值班长的回话最后落库。
   *
   * 同刻的兜底次序按 `kind`：人的话 → 提议 → 值班长的话。时钟是同一台机器的，
   * 同刻基本只出现在 `ManualClock` 的用例里，但排序必须是确定的（否则每次渲染都可能换位）。
   */
  const turns = $derived.by<TurnView[]>(() => {
    const stamped: { view: TurnView; rank: number }[] = (session?.messages ?? []).map((m) => ({
      // 排序与分类**同一处判定**（决策 252）：`kind` 是后端给的，界面不再各判一遍
      // （此前这里用 `role` 三元式、下面用 `startsWith`，两个判定点迟早不一致）。
      rank: m.kind === 'mine' ? 0 : m.kind === 'fm' ? 2 : 1,
      view: {
        key: `m${m.id}`,
        kind: m.kind,
        content: m.content,
        at: m.created_at,
        streaming: false,
        partial: false,
        traces: m.traces ?? [],
        // 推理留痕（决策 244）：空串与 null 都当作「这一轮没产推理」，界面不渲染那一块。
        thinking: m.thinking?.trim() ? m.thinking : null,
        liveTools: [],
        briefing: m.briefing,
        needsPairing: false,
        proposal: null,
        // 值守播报（决策 209④）：与 `kind` 正交的那个布尔（决策 252③），也由后端判。
        proactive: m.proactive,
        // 归因类别（决策 235① / 238）：**用后端解析并翻好的那一份**（`attribution_label`），
        // 界面不自己从稳定标识再映射一遍——两份映射迟早给出两个词，而「四类各一个词」
        // 是同一件事。未定位时后端给 null，界面就不显示（不编一个假的类别）。
        attribution: m.attribution_label ?? null,
      },
    }));
    for (const p of session?.proposals ?? []) {
      stamped.push({
        rank: 1,
        view: {
          key: `p${p.id}`,
          kind: 'proposal',
          content: p.summary,
          at: p.created_at,
          streaming: false,
          partial: false,
          traces: [],
          thinking: null,
          liveTools: [],
          briefing: null,
          needsPairing: false,
          proposal: p,
          proactive: false,
          attribution: null,
        },
      });
    }
    stamped.sort((a, b) => (a.view.at === b.view.at ? a.rank - b.rank : a.view.at < b.view.at ? -1 : 1));
    const out: TurnView[] = stamped.map((s) => s.view);
    if (pendingText) {
      out.push({
        key: 'pending',
        kind: 'mine',
        content: pendingText,
        at: '',
        streaming: false,
        partial: false,
        traces: [],
        thinking: null,
        liveTools: [],
        briefing: null,
        needsPairing: false,
        proposal: null,
        proactive: false,
        attribution: null,
      });
    }
    if (sending || stream.text) {
      out.push({
        key: 'live',
        kind: 'fm',
        // 还没收到第一个增量时不摆空白：给一句"对面在动"的实情，光标说明还在流
        content: stream.text || '值班长正在查台账…',
        at: '',
        streaming: stream.streaming,
        partial: !stream.streaming && stream.text.length > 0,
        traces: [],
        thinking: stream.thinking.trim() ? stream.thinking : null,
        liveTools: stream.tools,
        briefing: null,
        needsPairing: false,
        proposal: null,
        proactive: false,
        attribution: null,
      });
    }
    if (stream.error) {
      out.push({
        key: 'send-error',
        kind: 'failed',
        content: `发送失败：${stream.error}`,
        at: '',
        streaming: false,
        partial: false,
        traces: [],
        thinking: null,
        liveTools: [],
        briefing: null,
        needsPairing: needsPairing(stream.error),
        proposal: null,
        proactive: false,
        attribution: null,
      });
    }
    return out;
  });

  /**
   * 失败原因是「这台设备还没配对」吗（票 07）。
   *
   * 判据是后端的 403 报文原文，而不是 HTTP 状态码：403 在本应用里还被跨源防护用着
   * （决策 128），只看状态码会把「Origin 不对」也挂上配对入口——那是一条走不通的指引。
   */
  function needsPairing(message: string): boolean {
    return message.includes('还没配对');
  }

  /** 对话时间线是空的（且不是「还没读到」）：空态要居中，见 CSS 的 `.timeline.empty`。 */
  const timelineEmpty = $derived(!(loading && !session) && turns.length === 0);

  /**
   * 重读班次列表与某个班次的台账。
   *
   * `want` 三态：不给 = 接着看当前这一班；给 id = 切过去；给 `null` = 回到服务端默认
   * （最近活动的未归档班次）。指定的班次不在了（别的设备归档了它、或这个 id 本来就不存在）
   * 时**回落到默认**而不是报错——切班次的地方没有出错这一说，只有「去最近有人说话的那一班」。
   *
   * 落点与原先不同时 `generation + 1`：在途的 send 结果就此作废（决策 204⑥）。
   * 这一条同时覆盖了「另一台设备把当前班次归档了」那条被动路径——它不在「发送中禁止切换」
   * 那把 UI 锁的覆盖范围内。
   */
  async function reload(want?: string | null): Promise<boolean> {
    try {
      const list = await getForemanSessions();
      const target = want === undefined ? currentId : want;
      const known = !!target && list.sessions.some((s) => s.id === target);
      const payload = await getForemanSession(known ? target : null);
      sessionList = list.sessions;
      session = payload;
      const landed = payload.session?.id ?? null;
      if (landed !== currentId) generation += 1;
      currentId = landed;
      rememberLanding(landed, payload.session);
      loadError = null;
      return true;
    } catch (err) {
      loadError = (err as Error).message;
      return false;
    } finally {
      loading = false;
    }
  }

  /**
   * 落点收口（决策 217①④ / 220③）：**看过表、兜底文件、地址**三处一起跟上这一班。
   *
   * 三件事各有各的理由，且都必须在这里做：
   *   - 「我看过它了」记的是**它此刻的 `last_active_at`**（不是本机的当下）——两边同一座钟，
   *     机器一慢一快才不会读出假标记；
   *   - 兜底文件写**落点**而不是「请求的那一班」：指定的班次不在了（别的设备归档了它）时
   *     落回默认，此时该记住的是默认那一班；
   *   - 地址用 `replaceState`（决策 217③：程序改地址一律 replace），否则装载时的规范化
   *     会在历史里多塞一条，后退就不再是「回到上一页」。
   */
  function rememberLanding(landed: string | null, meta: ForemanSessionMeta | null) {
    let next = seen;
    if (!seenSeeded) {
      // 立基线只在**本机一条记录都没有**时做（判据在 `seedBaselineIfFirstRun`）：
      // 少了它，第一屏每一条都带「有新动静」——而它们只是刚被列出来；写成「每次装载都
      // 拿当下的列表立基线」则相反：**关机期间别处发生的动静会被记成「看过了」**，
      // 而那正是这枚标记最该说话的场合（实测：手机在别处开了新班次、说了话，回到这台
      // 电脑打开对讲台，菜单里那条不该是安静的）。
      next = seedBaselineIfFirstRun(next, sessionList);
      seenSeeded = true;
    }
    if (meta) next = markSeen(next, meta.id, meta.last_active_at);
    next = pruneSeen(
      next,
      sessionList.map((s) => s.id),
    );
    if (next !== seen) {
      seen = next;
      saveSeen(next);
    }
    // 落地即熄灭（决策 220③）：这一班的回话已经在台账里了，它不再是「此刻在说话」
    if (landed) foreignActive = forgetForeignActive(foreignActive, landed);
    saveSessionId(landed);
    writeQuery({ session: landed }, { replace: true });
  }

  /**
   * 切换时要**重置**的会话级状态（决策 204③的那份清单，逐条）。
   *
   * 只有这些是「属于某一班」的：这一屏读到的台账、读的加载态与错误、正在发的那句乐观轮、
   * 流式增量、以及还没发出去的输入。**不重置**的是 `details` / `chosenStop` / `crew` /
   * `pending`——它们派生自全局看板，换会话不等于换看板（同一条决策的第②条裁决）。
   */
  function resetSessionState() {
    session = null;
    loading = true;
    loadError = null;
    pendingText = null;
    stream = emptyForemanStream();
    input = '';
  }

  /**
   * 切到另一班。
   *
   * **回话中也可以切**（决策 220②）：那把 `sending || busy` 的锁撤掉了。它只是「别让你把
   * 正在等的那句回话弄丢」的**第三层**自保——前两层是 `appendForemanDelta` 的班次守卫
   * （增量串台）与 `send()` 里每个 await 之后的 `generation` 比对（回包串台），那两层一步没动。
   * 切走之后「那一轮回话去哪了」改由班次列表里的两枚标记说清楚（决策 220③）。
   *
   * 切走时那一轮从视野里撤下（`resetSessionState` + `send()` 的 `gen !== generation` 分支），
   * 但**回话照旧落台账**，回来就能看到完整的（决策 220⑤；切进一条正在回话的班次会先看到
   * 回话的后半截，落地后 `reload()` 补齐——这一条也别当 bug 修）。
   *
   * `write`：用户点的切换把班次写进地址（`pushState`——后退回到上一班是想要的，决策 217③）；
   * 从地址来的切换（后退 / 前进）不写，否则自己触发的装载会再写一次地址。
   */
  async function switchTo(id: string, opts: { write?: boolean } = {}) {
    if (id === currentId) return;
    generation += 1;
    // 先认下这件事再写地址：地址一变，下面那个 `$effect` 会拿新值来比——认下了才不重复装载
    currentId = id;
    if (opts.write) writeQuery({ session: id });
    resetSessionState();
    await reload(id);
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

  /**
   * 开一个新班次并切过去（空班是合法状态：第一句话说出来时它才得名）。
   *
   * **不带 busy 守卫**：调用方已经持有它。归档最后一个班次那条路就是这样调的
   * ——那时的 busy 必然是 true，若这里再守一次，归档完最后一个班次会静默什么都不做，
   * 页面停在一片空白上（「一个班次都没有」且没有当前班次）。
   */
  async function openFreshSession(opts: { push?: boolean } = {}) {
    const created = await createForemanSession();
    sessionList = [created.session, ...sessionList];
    generation += 1;
    resetSessionState();
    // 同 `switchTo`：先把落点认下来，再写地址（用户按的那一颗 push，归档后的自动开新班不写——
    // 地址的落点由随后的 `reload` 用 replaceState 规范化）
    currentId = created.session.id;
    if (opts.push) writeQuery({ session: created.session.id });
    await reload(created.session.id);
  }

  /** 从界面按下「+ 新班次」（⋯ 菜单的第一项）。用户按的 = 一次换班，故 push 进地址。 */
  async function newSession() {
    if (sending || busy) return;
    busy = true;
    try {
      await openFreshSession({ push: true });
    } catch (err) {
      loadError = (err as Error).message;
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
      sessionList = sessionList.map((s) => (s.id === id ? updated.session : s));
      if (session?.session) session = { ...session, session: updated.session };
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
      await archiveForemanSession(id);
      dialog = null;
      generation += 1;
      resetSessionState();
      const list = await getForemanSessions();
      sessionList = list.sessions;
      if (list.sessions.length > 0) {
        await reload(list.sessions[0].id);
      } else {
        // 一个不剩：新开一班（走不带守卫的那条，见 `openFreshSession`）
        await openFreshSession();
      }
    } catch (err) {
      dialogError = (err as Error).message;
    } finally {
      busy = false;
    }
  }

  function onStreamEvent(_taskId: string, event: SseEvent) {
    // 「别的班次正在回话」（决策 220③）：`/foreman/stream` 把**全部**工头增量广播给所有
    // 订阅者，而这个字段此前只被用来「丢掉不匹配的」——等于把「另一班在说话」白扔了。
    // **不判 `sending`**：别的班次说话时本机可能什么都没发，而那正是需要告知的时候。
    foreignActive = noteForeignDelta(foreignActive, event, currentId, Date.now());
    // 只在等回话期间累积：收尾后到达的尾巴不得再造一轮（回话以台账为准）
    if (!sending) return;
    // 班次守卫：不是当前这一班的增量一律丢弃（决策 204⑥，判据在 realtime/foreman.ts）。
    // 两条声道各归各的：回话进 `text`、思考进 `thinking`、工具调用进 `tools`（决策 244），
    // 判据（类型 / 身份 / 班次）三处同源，故都收在这一个出口。
    stream = appendForemanDelta(stream, event, currentId);
    stream = appendForemanTool(stream, event, currentId);
  }

  let conn: TaskStream | null = null;

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
      if (pruned !== foreignActive) foreignActive = pruned;
    }, 5_000);
    return () => clearInterval(t);
  });

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
      await reload();
    } catch (e) {
      proposalError = { id, text: e instanceof Error ? e.message : String(e) };
      // 失败也要重读：过期 / 态势变化这两种失败**改了库里的状态**（标 expired / 保持 pending
      // 并落一条说明），不重读的话界面显示的仍是按键之前那一份。
      await reload();
    } finally {
      proposalBusy = null;
    }
  }

  function onVisible() {
    if (document.visibilityState !== 'visible') return;
    void reload();
    conn?.reconnectNow();
  }

  onMount(() => {
    // 地址权威、localStorage 兜底（决策 217④）：「我一直在看这一班」不该因为从看板点回来而重置
    seen = loadSeen();
    // 回执标签取一次（模块级缓存，之后别的页签再挂载不再发第二跳）。失败**不打断对话**：
    // 缓存不记失败（下一次挂载会重试），期间回执原样显示工具名——英文原名好过一个错词。
    void loadToolLabels()
      .then((l) => (toolLabels = l))
      .catch(() => {});
    void reload(urlSession ?? loadSessionId() ?? undefined);
    // 复用任务流的分帧 / 退避 / 主动重连（票 03）：工头流只是换了一条路径
    conn = new TaskStream(
      '',
      {
        onEvent: onStreamEvent,
        onStatus: (_id, status) => {
          streamStatus = status;
          // 断开即熄灭（决策 220③）：「别的班次在回话」是一份**描述此刻**的映射，
          // 连接不在的时候它说的就不再是此刻——留着只会变成一个撤不掉的假标记。
          if (status !== 'open') foreignActive = emptyForeignActive();
        },
      },
      { path: '/foreman/stream' },
    );
    conn.start();
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
      conn?.stop();
      conn = null;
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
   * 只在**轮数**变化时滚：流式增量改的是某一轮的内容，不是轮数，故流式期间视口不乱动；
   * 而发送（乐观轮 + 值班长那一轮进来）与回话落地（台账覆盖）都是轮数变化。
   */
  $effect(() => {
    const n = turns.length;
    if (n === 0) return;
    void tick().then(scrollToNewest);
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

  /* ───────────── ⋯ 班次菜单（折行档）：键盘与关闭（票 04，照顶栏「待处理」那一套） ─────────────
   *
   * 交互语汇**照抄**顶栏那个下拉（票 04 / R2-04 补的三条出口），不新造第三套：`aria-expanded`
   * + `aria-controls`、Escape 关得掉（**焦点没进过面板时也算**）、点面板外面关、上下方向键走项、
   * `Home` / `End`、面板**常驻 DOM** 用 `hidden` 开合。键盘一律在 `window` 上收——给静态元素
   * 挂交互处理器是 a11y 检查里的红灯。
   */

  /** 菜单里可聚焦的项（禁用的不行——它们在这一档读得到理由，但按不动）。 */
  function menuItems(): HTMLElement[] {
    return menuPanel
      ? [...menuPanel.querySelectorAll<HTMLElement>('button[data-menu-item]:not([disabled])')]
      : [];
  }

  function focusMenuItem(index: number): void {
    const list = menuItems();
    if (list.length === 0) return;
    const n = list.length;
    list[((index % n) + n) % n].focus();
  }

  function closeMenu(returnFocus: boolean): void {
    menuOpen = false;
    if (returnFocus) menuTrigger?.focus();
  }

  function toggleMenu(): void {
    if (menuOpen) {
      closeMenu(false);
      return;
    }
    menuOpen = true;
    void tick().then(() => focusMenuItem(0));
  }

  function onWindowKey(e: KeyboardEvent): void {
    const active = document.activeElement as HTMLElement | null;
    const onTrigger = !!menuTrigger && active === menuTrigger;
    const inPanel = !!active && !!menuPanel && menuPanel.contains(active);

    if (e.key === 'ArrowDown' && onTrigger && !menuOpen) {
      e.preventDefault();
      menuOpen = true;
      void tick().then(() => focusMenuItem(0));
      return;
    }
    if (!menuOpen) return;

    if (e.key === 'Escape') {
      closeMenu(onTrigger || inPanel);
      return;
    }
    if (!onTrigger && !inPanel) return;

    const list = menuItems();
    if (list.length === 0) return;
    const current = list.indexOf(active as HTMLElement);
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      focusMenuItem(current + 1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      if (current <= 0) menuTrigger?.focus();
      else focusMenuItem(current - 1);
    } else if (e.key === 'Home') {
      e.preventDefault();
      focusMenuItem(0);
    } else if (e.key === 'End') {
      e.preventDefault();
      focusMenuItem(list.length - 1);
    }
  }

  /** 点面板外面关掉（含「本来就开着、用户去点别处」那一档）。 */
  function onWindowClick(e: MouseEvent): void {
    if (!menuOpen) return;
    const target = e.target as Node | null;
    if (target && menuWrap?.contains(target)) return;
    menuOpen = false;
  }

  /**
   * 工位回执的展开／收起（受控，见 `receiptOpen`）。
   *
   * `preventDefault` 掉 `summary` 的默认行为：让浏览器自己翻 `open`，状态就在 Svelte 之外，
   * 流式增量重渲染那一轮时会与模板里的 `open` 打架。人点一下 = 改一次我们自己的状态。
   */
  function toggleReceipt(event: MouseEvent, key: string) {
    event.preventDefault();
    receiptOpen = { ...receiptOpen, [key]: !receiptIsOpen(key) };
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
   * 说一句话。
   *
   * **先确定班次，再发话**：这台机器上一个班次都没有时（首启空 home 的第一次说话），
   * 客户端自己先开一个——若让它落到服务端的缺省逻辑上，回话的流式增量带的班次 id
   * 是回来之后才知道的，而此刻增量已经在路上了，会被班次守卫挡掉（字还在，只是白流一场）。
   *
   * 每个 await 之后都比对 `generation`：这一班的回包不落到另一班的屏幕上（决策 204⑥）。
   * 比对不通过时**连乐观轮一起撤**——它属于已经不显示的那一班。
   */
  async function send() {
    const text = input.trim();
    if (!text || sending) return;
    sending = true;
    pendingText = text;
    stream = beginForemanStream();
    // 这一趟之前台账里已有的失败轮 id：失败回来后靠它分辨「这次新出现的那一条」
    // （判据在 realtime/foreman.ts；不记的话，早先的失败会让真正的断网静默下来）
    const failuresBefore = failedLedgerRowIds(session?.messages ?? []);
    let gen = generation;
    let sid = currentId;
    try {
      if (!sid) {
        const created = await createForemanSession();
        if (gen !== generation) return;
        sid = created.session.id;
        currentId = sid;
        sessionList = [created.session, ...sessionList];
        // 这是**我们自己**开的班，不算「换班」：重取记号，免得下面每一步都判成过期。
        gen = generation;
      }
      // 「本机发出且未落地」（决策 220③）：切走之后这枚标记要落在**它**那一行上
      sendingSid = sid;
      const res = await sendForemanMessage(text, sid);
      if (gen !== generation) {
        // 切走了：这一轮从这一屏撤下（决策 220⑤），但回话已经落地——列表要跟上，
        // 否则「原班次有新动静」永远等不到（这一支在放开切换之后是**常态路径**）。
        // 「落地即熄灭」在这里同样要办：这一班的增量此前被记进了「别的班次在回话」那张映射
        // （切走之后它的 `session_id` 就不再是「当前这一班」了），不清掉那枚
        // 「正在回话」会一直亮到静默超时——而它说的已经不是实话。
        pendingText = null;
        stream = emptyForemanStream();
        foreignActive = forgetForeignActive(foreignActive, sid);
        void refreshSessionList();
        return;
      }
      // 回话是权威值：先收敛流式文本（重取台账期间不闪空），再以台账覆盖
      stream = settleForemanStream(stream, res.reply);
      input = '';
      // 重取之后**无条件收掉这两样本地状态**：它们是「这一轮」的东西，而重取可能发现
      // 服务端已经把我们换到了另一班（另一台设备归档了它）。那种情况下留着乐观轮，
      // 它就会挂在**另一班的**时间线上——正是决策 204⑥ 要挡的串台。
      if (await reload(sid)) {
        pendingText = null;
        stream = emptyForemanStream();
      }
    } catch (err) {
      // 失败不改输入框内容：后端在叫模型之前已把 user 行落库，人改几个字就能重发
      // （决策 182㉓）。失败以时间线里的一轮呈现——不弹窗、不 toast。
      if (gen !== generation) {
        pendingText = null;
        stream = emptyForemanStream();
        void refreshSessionList();
        return;
      }
      // 本地超时**不等于**这一轮失败：服务端那一轮不随这次请求一起死（决策 223），
      // 故超时那一类由 `failureNotice` 补上「它仍在继续」的实情——否则人会重发一句，
      // 而那一轮很可能正在把话答完。
      stream = failForemanStream(stream, failureNotice((err as Error).message));
      // 重取成功才撤乐观轮：撤了之后这话由台账那一行承担，不靠重取失败时凭空消失
      if (await reload(sid)) {
        pendingText = null;
        // 后端**已经**把这一轮为什么没跑起来落了账（决策 211④ / 票 04）：那一行就是这次的
        // 失败轮，而且比本地这条传输报文更全（带归因、刷新后还在）。此时撤掉本地的 error，
        // 免得同一个失败在时间线里摆成两轮。台账里没有新失败行时才用它兜底——请求根本没
        // 到后端（网络断了、代理 502、配对 403 发生在进 handler 之前）时，本地是唯一信号。
        if (ledgerOwnsTheFailure(session?.messages ?? [], failuresBefore)) {
          stream = { ...stream, error: null };
        }
      }
    } finally {
      sending = false;
      sendingSid = null;
    }
  }

  /**
   * 只重读班次列表（不碰这一屏的台账）。
   *
   * 已经落地的回话（含切走之后落地的那些）更新的是 `last_active_at`——「有新动静」那枚
   * 标记的判据。这一屏读的是哪一班由 `reload` 管，本函数只管那一列元信息。
   */
  async function refreshSessionList() {
    try {
      const list = await getForemanSessions();
      sessionList = list.sessions;
      const next = pruneSeen(
        seen,
        list.sessions.map((s) => s.id),
      );
      if (next !== seen) {
        seen = next;
        saveSeen(next);
      }
    } catch {
      // 列表读不到不影响这一屏：标记晚一步出现而已，台账是权威
    }
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

  /** 工位名 → sprite：快照里的 stage 是后端字符串，未登记的值不猜（退回台账箱）。 */
  function stageSprite(stage: string): SpriteName {
    const col = BOARD_COLUMNS.find((c) => c.stages.includes(stage as Stage));
    return col ? COLUMN_SPRITES[col.key] : 'chest';
  }

  /**
   * 工具名 → 中文词（实时那一栏用；回执那一栏在 {@link receipt} 里并列拿同一张表）。
   *
   * 词来自后端清单（`GET /foreman/tools`，onMount 取一次缓存，决策 247⑤）——**前端不再
   * 手抄一张表**（那张 18 键的 `TOOL_LABELS` 缺 4 个词、还带着死键，已删）。未登记的值
   * **原样显示工具名**，不兜底成「台账查读」——那个兜底会把「值班长调了个界面还不认识的
   * 新工具」说成一件它没做的事（决策 200 的平实口径）；取数还没回来时同理（英文原名好过
   * 一个猜出来的中文词）。
   */
  function toolLabel(tool: string): string {
    return labelFor(toolLabels, tool);
  }

  /**
   * 工具痕迹 → 回执行（§3.3 纪律 3）。
   *
   * 能对上该轮快照里的任务就标出来源**工位**：像素本身不可考，靠 sprite + 工位名双编码。
   * 对不上（例如查了一次就没了的任务）就只说这是台账查读，不假装知道出处。
   */
  function receipt(trace: ForemanTrace, briefing: ForemanBriefing | null): {
    sprite: SpriteName;
    workshop: string;
    label: string;
  } {
    const known = [
      ...(briefing?.pending ?? []),
      ...(briefing?.running ?? []),
      ...(briefing?.failed ?? []),
    ].find((b) => trace.args_summary.includes(b.task_id));
    return {
      sprite: known ? stageSprite(known.stage) : 'chest',
      workshop: known?.stage ?? '台账',
      label: labelFor(toolLabels, trace.tool),
    };
  }

  /**
   * 拉取每个 pending 任务的详情（`allowed_actions` 只在详情下发，决策 101）。
   *
   * **必须随 pending 集合变化重拉，不能只在 onMount 拉一次**：`board.init()` 是异步的
   * （App.svelte 的 onMount 发起），本页 onMount 时 `board.tasks` 往往还是空的——
   * 只拉一次的话 `pending` 为空、`details` 永远为空，页面会安静地退化成「打开任务详情」
   * 按钮，把后端下发的恢复动作整片吞掉（e2e ⑩ 打红即此）。
   */
  async function loadDetails(tasks: TaskListItem[]) {
    const next: Record<string, { actions: AllowedAction[]; cursors: BranchCursor[] }> = {};
    await Promise.all(
      tasks.map(async (t) => {
        try {
          const detail = await getTask(t.id);
          next[t.id] = { actions: detail.allowed_actions, cursors: detail.cursors };
        } catch {
          // 单个任务失败不影响整页（与看板补详情同一姿态）
        }
      }),
    );
    details = next;
  }

  /**
   * 待办集合的指纹：只含 id 与 pending 类型。用它驱动重拉——
   * 集合不变时不重拉（避免 $effect 自激），集合一变（board 装载完成 / resume 后状态翻转）
   * 才重取。类型也进来是因为 `pending_updated` 会换理由而 id 不变。
   */
  const pendingKey = $derived(
    pending
      .map((t) => `${t.id}:${t.pending_reason?.type ?? ''}:${t.current_stage}`)
      .sort()
      .join('|'),
  );

  let lastKey = $state<string | null>(null);
  $effect(() => {
    const key = pendingKey;
    if (key === lastKey) return;
    lastKey = key;
    // 空集合无需拉取：直接清空上一次的读数
    if (!key) {
      details = {};
      return;
    }
    void loadDetails(pending);
  });

  async function handleAction(
    taskId: string,
    action: AllowedAction,
    opts: { cursorId?: string; input?: string },
  ) {
    // board 重载后 pending 集合会变，$effect 里的指纹驱动重拉详情；
    // 这里显式再拉一次是为了动作回执后立刻反映（不等下次轮询）。
    await board.handleTaskAction(taskId, action, opts);
    await loadDetails(board.pendingTasks);
  }
</script>

<main class="talk">
  <div class="talk-head">
    <h1 class="tt">对讲台</h1>
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
      <span>{session?.foreman.wired === false ? '值班长未接线' : '值班中'}</span>
      <span class="sep">▪</span>
      <!-- `工位` 的译文只在这一行（`.stat-wide` 窄屏收起）：窄屏上这个词不再出现
           （值班板的「8 工位」与那两行说明都收进了桌面款），故窄屏没有漏译。 -->
      <span class="stat-wide">夜班态势：8 工位（流水线的阶段）</span>
      <span class="sep stat-wide">▪</span>
      <span>本次会话 {session ? formatTokens(session.total_tokens) : '—'} tok</span>
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
        {#each sessionList as s (s.id)}
          <!-- 切换**不因 `sending || busy` 禁用**（决策 220②）：回话中也可以换班次，
               「那一轮回话去哪了」由 ⋯ 列表里的两枚标记说。 -->
          <button
            type="button"
            class="runchip"
            class:now={s.id === currentId}
            aria-pressed={s.id === currentId}
            title={s.title}
            onclick={() => void switchTo(s.id, { write: true })}
          >
            {s.title}
          </button>
        {/each}
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
          <button type="button" class="runchip act" disabled={sending || busy} onclick={openRename}
            >改名</button
          >
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
      </div>
    {:else}
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
              closeMenu(false);
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
                 `aria-current` 让它既可见又播报（票 04）。 -->
            <div class="mi now" aria-current="true">
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
              data-menu-item
              onclick={() => {
                closeMenu(false);
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
              closeMenu(false);
              openRename();
            }}>改名</button
          >
          <button
            type="button"
            class="mi act"
            data-menu-item
            disabled={sending || busy || !currentId}
            onclick={() => {
              closeMenu(false);
              dialogError = null;
              dialog = 'archive';
            }}>归档</button
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
  {#if !folded || pending.length > 0 || loadError}
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
        {#if needsPairing(loadError)}
          <p class="note">
            配对码只在那台跑服务的电脑本机生成：在那台电脑上（桌面应用窗口，或浏览器里的
            127.0.0.1）打开<a
              class="crumb"
              href="#/share"
              onclick={() => router.navigate('/share')}>手机访问</a
            >页扫码即可——手机上打开这一页是拿不到配对码的。若已添加到主屏幕，<b
              >换过令牌后要重新添加一次</b
            >（图标里记的是当时那条带令牌的地址）。
          </p>
        {/if}
      </div>
    {/if}

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
            「{task.title}」走到 {task.current_stage}，{task.pending_reason?.message ?? '需要你决定'}。
          </p>
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
              isBusy={(a) => board.actionBusy === `${task.id}:${a.action}`}
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
    aria-label="对话时间线"
  >
    {#if loading && !session}
      <div class="quiet">正在读会话台账…</div>
    {:else if turns.length === 0}
      <!-- 空态：状态 → 下一步（票 13 / parallel-brief §三.3）。`值班长` 的**首现平实说法**
           落在这里（决策 200）：页头那一行是标题栏、不承载翻译，而这一句正是第一次进这一页的
           人读到这个词的地方——本页其余地方（名牌、状态区）保持原词，同一页面内不重复。 -->
      <EmptyState
        state="还没有对话。"
        next="说一句，值班长（跟我对话的 AI）就在对面——它与任务无关，空班也答得上。"
      />
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
          <p>{p.summary}</p>
          {#if isRepairProposal(p)}
            <!-- 修复提议（票 12）：人要看的是那份补丁，不是参数摘要。
                 闸门读数先说「有没有补丁」——没过闸门时**根本没有** diff（决策 210④），
                 不说清会被当成加载失败。diff 可展开、可复制，不做语法高亮。 -->
            {@const gateLabel = repairGateLabel(p)}
            {#if gateLabel}
              <p class="dim note" class:ferr={!p.payload?.gate_passed}>{gateLabel}</p>
            {/if}
            {#if p.payload?.diff}
              <details class="rcpts">
                <summary class="rcpts-sum">
                  补丁 <span class="dim">{p.payload.diff_stat ?? ''}▸</span>
                </summary>
                <pre class="diff-body" data-repair-diff={p.id}>{p.payload.diff}</pre>
              </details>
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
      {:else}
        <article
          class="turn"
          class:fm={turn.kind === 'fm'}
          class:mine={turn.kind === 'mine'}
          class:failed={turn.kind === 'failed'}
          class:console={turn.kind === 'console'}
        >
          <div class="dname">
            {turn.kind === 'failed'
              ? '发送失败'
              : turn.kind === 'mine'
                ? '值班经理'
                : turn.kind === 'console'
                  ? '操作台'
                  : turn.proactive
                    ? '值班长 · 值守'
                    : '值班长'}
          </div>
          <p class:streaming={turn.streaming}>{turn.content}</p>

          <!-- 正在发生的工具调用（决策 244）：**实时**，不是等这一轮落库。
               形态照工位回执（左缘亮度阶 + 无框 = 转述不是发言），但它是**此刻**的东西，
               故左缘跟相位走：正在查是静的 --pane，查完点亮 --go，没查到用 --stop。
               它与下面的「工位回执」是同一件事的两个时态：这里是进行中，落地后由回执接管
               （所以落地轮的 `liveTools` 恒空，不会两处都画）。 -->
          {#if turn.liveTools.length > 0}
            <div class="livetools" data-live-tools={turn.liveTools.length}>
              {#each turn.liveTools as t, i (`${turn.key}-lt${i}`)}
                <div
                  class="rcpt live"
                  class:pending={t.phase === 'start'}
                  class:done={t.phase === 'end'}
                  class:bad={t.phase === 'error'}
                >
                  <div class="rcpt-head">
                    <span class="dim">{toolLabel(t.tool)}</span>
                    <span class="dim args">{t.args_summary}</span>
                    <span class="rs" class:bad={t.phase === 'error'}>
                      {t.phase === 'start' ? '正在查…' : t.phase === 'error' ? '没查到' : '已读'}
                    </span>
                  </div>
                </div>
              {/each}
            </div>
          {/if}

          <!-- 推理 / 思考（决策 244）：**默认收起**，两档都是——它常常比回话本身长一个量级，
               展开着摆在时间线上会把对话冲垮。零新增视觉语言：复用回执那套
               （左缘亮度阶 + 无框 + 详情块），只在措辞上把「它想的过程」与「它查的台账」分开。
               摘要在流式期间就说「正在想…」，收口后带字数——人不用点开就知道里面有没有东西。 -->
          {#if turn.thinking}
            <details class="rcpts think" open={thinkingIsOpen(turn.key)}>
              <summary class="rcpts-sum" onclick={(e) => toggleThinking(e, turn.key)}>
                {turn.streaming ? '正在想…' : `思考过程 ${turn.thinking.length} 字`} ▸
              </summary>
              <pre class="think-body">{turn.thinking}</pre>
            </details>
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
              >页扫码即可——手机上打开这一页是拿不到配对码的。若已添加到主屏幕，<b
                >换过令牌后要重新添加一次</b
              >（图标里记的是当时那条带令牌的地址）。
            </p>
          {/if}

          <!-- 工位回执：转述不是发言（左缘亮度阶 + 无框，形状上就与发言不同）。
               **默认态分档**（决策 218 ②/Q11）：桌面照旧展开（回执是这一轮结论的出处，
               「可追溯性不因对话而丢失」是四条纪律之一）；折行档默认收起——每轮 30–60px
               是长会话里最大的隐性纵向开销，而这一档的纵向空间是拿钉住物之间的残渣换的。
               **内容一个字不删**：出处按一下就在，只是不再默认占屏（摘要行永远带条数）。
               展开态**受控**（`receiptOpen`，见那边的注释）：让浏览器自己翻 `open` 的话，
               流式增量反复重渲染同一轮时会把人手动展开的那一轮打回收起。 -->
          {#if turn.traces.length > 0}
            <details class="rcpts" open={receiptIsOpen(turn.key)}>
              <summary class="rcpts-sum" onclick={(e) => toggleReceipt(e, turn.key)}>
                工位回执 <span class="dim">{turn.traces.length} 次台账查读 ▸</span>
              </summary>
              {#each turn.traces as trace, i (`${turn.key}-t${i}`)}
                {@const r = receipt(trace, turn.briefing)}
                <div class="rcpt">
                  <div class="rcpt-head">
                    <Sprite name={r.sprite} size={10} />
                    <span class="nm">{r.workshop}</span>
                    <span class="dim">{r.label}</span>
                    <span class="dim args">{trace.args_summary}</span>
                    <span class="rs" class:bad={!trace.ok}>{trace.ok ? '已读' : '未读到'}</span>
                  </div>
                </div>
              {/each}
            </details>
          {/if}
        </article>
      {/if}
    {/each}
  </section>

  <!-- ── 输入坞：钉底。Enter 发送 / Shift+Enter 换行；发送中禁用 ── -->
  <form
    class="typer"
    class:warn={streamStatus === 'error'}
    onsubmit={(e) => {
      e.preventDefault();
      void send();
    }}
  >
    <div class="dname">值班经理</div>
    <textarea
      class="input"
      rows="2"
      {placeholder}
      bind:value={input}
      disabled={sending}
      onkeydown={onKeydown}
      oncompositionstart={() => composing.start()}
      oncompositionend={() => composing.end()}
    ></textarea>
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
      <button type="submit" class="btn solid" disabled={sending || !input.trim()}>发送</button>
    </div>
  </form>

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
    <p class="dim note">
      说的话不会被删，但照旧按保留期（默认 30 天，可配）到期清理——归档是收起来，不是永久保存。
      归档后自动切到最近有说话的班次；如果这是最后一个，就新开一班。
    </p>
    {#if dialogError}
      <p class="ferr note">{dialogError}</p>
    {/if}
  </Modal>

  <!-- ── 值班板：**桌面右栏**（折行档整块不渲染，CSS 那条 `display: none` 管着；
       同一份读数在看板 8 列与顶栏灯带上各有一份，这一条是第三份——决策 218 ②） ── -->
  <aside class="talk-side crew">
    <div class="reg">
      <div class="reg-head"><span>值班板</span><span class="n">8 工位</span></div>
      <ul class="brows no-scrollbar">
        {#each crew as c (c.key)}
          <li class="brow {c.state === 'warn' ? 'pen' : c.state === 'run' ? 'hot' : ''}">
            <span class="blamp {c.state === 'warn' ? 'w' : c.state === 'run' ? 'c' : c.state === 'done' ? 'd' : ''}"></span>
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
</main>

<!-- ⋯ 班次菜单的三条出口（票 04）：Escape 关得掉（焦点没进过面板时也算）、点面板外面关、
     上下方向键走项。与顶栏那个下拉同一姿态——键盘一律在 `window` 上收。 -->
<svelte:window onclick={onWindowClick} onkeydown={onWindowKey} />

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

  /* ── 工位回执：转述不是发言（左缘 4px 亮度阶 + 无框，与命令输出同一手法） ── */
  .rcpts {
    margin-top: 8px;
  }
  .rcpts-sum {
    color: var(--text-3);
    cursor: pointer;
    list-style: none;
  }
  .rcpts-sum::-webkit-details-marker {
    display: none;
  }
  .rcpt {
    border-left: 4px solid var(--pane);
    background: var(--panel);
    padding: 6px 10px;
    margin-top: 6px;
  }
  .rcpt-head {
    display: flex;
    align-items: center;
    gap: 7px;
    color: var(--text-3);
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

  /* ── 正在发生的工具调用（决策 244）：与回执同一形状，靠左缘的档位说相位 ──
     正在查是静的 --pane（还没结果可看），查完点亮 --go，没查到走 --stop。
     不用动画位：全站零新增动画位这条纪律不因「实时」破例（转的那一刻就说明在查）。 */
  .livetools {
    margin-top: 8px;
  }
  .rcpt.live {
    margin-top: 4px;
  }
  .rcpt.live.pending {
    border-left-color: var(--pane);
  }
  .rcpt.live.done {
    border-left-color: var(--go);
  }
  .rcpt.live.bad {
    border-left-color: var(--stop);
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
      /* 名牌 tab 悬出框沿 16px，给它留出上沿（也给对话留出与输入坞的分界） */
      margin-top: 40px;
      display: grid;
      grid-template-columns: minmax(0, 1fr) auto;
      grid-template-areas: 'field send';
      gap: 6px 10px;
    }
    .typer textarea {
      grid-area: field;
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
      /* 与输入框齐高（44px 是触控底线，2 行输入框在 16px 字号下约 63px） */
      align-self: stretch;
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
    /* 这一档 `app.css` 把 `body` 的底边距也换成了 `--sbar-h`（= 状态条 42px + 底部
       页签栏 `--nav-h`，决策 243），两个数从此相等——899 块里那条抵差额的负外边距
       因此归零（不改的话会白吃 4px）。 */
    .talk {
      margin-bottom: 0;
    }
  }
</style>
