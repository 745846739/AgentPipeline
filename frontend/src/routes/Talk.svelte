<script lang="ts">
  import { onMount, tick } from 'svelte';
  import type {
    AllowedAction,
    BranchCursor,
    ForemanBriefing,
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
    getForemanSession,
    getForemanSessions,
    getTask,
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
  import { CompositionGuard, shouldSubmitOnEnter } from '../lib/enterToSend';
  import { TaskStream, type StreamStatus } from '../realtime/connection';
  import {
    appendForemanDelta,
    beginForemanStream,
    emptyForemanStream,
    failForemanStream,
    settleForemanStream,
    type ForemanStreamState,
  } from '../realtime/foreman';
  import Sprite from '../components/render/Sprite.svelte';
  import Gauge from '../components/render/Gauge.svelte';
  import PendingActions from '../components/board/PendingActions.svelte';
  import DiffReviewPanel from '../components/task/DiffReviewPanel.svelte';
  import EmptyState from '../components/ui/EmptyState.svelte';
  import Modal from '../components/ui/Modal.svelte';
  import { router } from '../router.svelte';

  /**
   * 对讲台（theme-6-pixel.md §3.3；决策 174 / 182 / 183 / 192）。版面**三分区**（票 04）：
   * 状态区（当前急停）／对话时间线（值班长的话、值班经理的话、工位回执）／输入坞。
   * 值班板是**独立的一块**——桌面右栏、窄屏收成时间线之上的横向灯条，不在状态区里。
   *
   * **宽屏整页钉在视口内，时间线是唯一会滚的区域**：一个两小时前挂起的急停被对话顶出视野
   * 是本页最不能出的错，故不靠 sticky 逐段救，而是把「会长的部分」与「不能动的部分」
   * 放在两个不同的滚动容器里。
   *
   * **窄屏（≤479px）反过来：整页随手指滚，只有两样东西钉住**——急停摘要条钉在顶栏下沿、
   * 输入坞钉在底栏上沿（决策 192，§5 转写 4）。同一个不变式（急停不滚出视野）在手机上由
   * 「钉住」完成：钉住的东西必须便宜，故**窄屏连单张急停也折成一行摘要条**（`forceFold`），
   * 一张 376px 的展开轮钉在 900px 的屏幕上等于把整块屏幕钉死。对话区因此拿到的是
   * 「整块屏幕减去两条钉住物」，而不是钉住物之间的残渣——这是本次改版的全部要点。
   *
   * **但状态区自己也有上限**（桌面 46vh，超高时区内滚），而一张急停轮内联着
   * 后端下发的动作集，最高的形状（带补充输入的 resume）约 330px——13″ 上可用只有约
   * 344px，**展开一张就已经占满整个区**。故急停轮折叠（决策 183）：**只有一张时不动**
   * （单急停版面与折叠前一致），两张以上**一张都不展开**、全部收成一行约 36px 的摘要条；
   * 人点「展开恢复动作」才展开那一张，且同时只展开一张（判据在 `lib/talkStops.ts`）。
   * 窄屏这一档不另写判据，只把 `forceFold` 传成真（决策 192）。
   *
   * **对面是真的会说话的值班长**（`/foreman/session` + `/foreman/messages` + `/foreman/stream`）：
   * 对话不依赖任务——空看板（无项目无任务）也照样能问它话。
   *
   * **一条长会话改成一排班次**（决策 204）：时间线顶部一条 chip 行——新建 / 切换 /
   * 重命名 / 归档。三条硬约束都在版面里兑现：chip 行**非 sticky**（窄屏只有两个钉住物，
   * 决策 192）、**不在页头**（`.talk-head` 在 `row auto` 上，涨的 px 直接吃对话区）、
   * 不动顶栏（决策 198 的三项与 138px）。会话隔离的是**对话上下文**（这一屏读什么、合计多少），
   * **不是权限、也不是态势快照**——「换会话 ≠ 换看板」。
   *
   * **值班长的回复里永远没有按钮**：写动作只在状态区的急停轮里渲染（后端下发的
   * `allowed_actions`，决策 101）。同一个动作在两处各渲染一颗钮，会让「哪个是真的」
   * 变成使用者必须思考的问题。空态的「去看板新建任务」是页面固定的**导航钮**，不进动作契约。
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

  /** 每个 pending 任务的详情（allowed_actions 只在详情里下发，决策 101）。 */
  let details = $state<Record<string, { actions: AllowedAction[]; cursors: BranchCursor[] }>>({});

  /** 窄屏（≤479px，§5 移动款）：整页滚 + 两条钉住物，见文件头。断点与 app.css 同一档。 */
  let narrow = $state(
    typeof window !== 'undefined' && window.matchMedia('(max-width: 479px)').matches,
  );
  let narrowMq: MediaQueryList | null = null;
  let onNarrowChange: ((e: MediaQueryListEvent) => void) | null = null;

  /** 滚动容器（`scrollToNewest` 用）：窄屏是整页，故只绑桌面那个时间线。 */
  let timelineEl = $state<HTMLElement | undefined>();
  /** 状态区（展开一张时把它拉回视野，见 `toggleStop`）。 */
  let zoneEl = $state<HTMLElement | undefined>();

  const pending = $derived(board.pendingTasks);
  /** 空看板：装载完成之后一个任务都没有（装载中不算——那会把「还没读到」说成「空」）。 */
  const emptyBoard = $derived(!board.loading && board.tasks.length === 0);

  /**
   * 展开的那张急停（决策 183）。`undefined` = 还没选过（跟随默认：**宽屏只有一张时展开它，
   * 两张以上一张都不展开；窄屏恒不展开**）、`null` = 人显式收起、否则是那张的 id。
   * 三态的判据在 `lib/talkStops.ts`，此处只持状态。
   */
  let chosenStop = $state<string | null | undefined>(undefined);
  const stopIds = $derived(pending.map((t) => t.id));
  const foldable = $derived(isFoldable(stopIds, narrow));
  const openStop = $derived(resolveOpenStop(stopIds, chosenStop, narrow));

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
    kind: 'fm' | 'mine' | 'failed';
    content: string;
    /** 流式尾随方块光标（既有 `.streaming`，不新增动画位）。 */
    streaming: boolean;
    /** 断流/出错：这一轮只有已收到的部分。 */
    partial: boolean;
    /** 该轮工具痕迹（台账查读），空数组 = 这一轮没翻台账。 */
    traces: ForemanTrace[];
    briefing: ForemanBriefing | null;
    /** 失败原因是「这台设备还没配对」（票 07）：只有它会挂出配对入口。 */
    needsPairing: boolean;
  }

  const turns = $derived.by<TurnView[]>(() => {
    const out: TurnView[] = (session?.messages ?? []).map((m) => ({
      key: `m${m.id}`,
      kind: m.role === 'user' ? 'mine' : 'fm',
      content: m.content,
      streaming: false,
      partial: false,
      traces: m.traces ?? [],
      briefing: m.briefing,
      needsPairing: false,
    }));
    if (pendingText) {
      out.push({
        key: 'pending',
        kind: 'mine',
        content: pendingText,
        streaming: false,
        partial: false,
        traces: [],
        briefing: null,
        needsPairing: false,
      });
    }
    if (sending || stream.text) {
      out.push({
        key: 'live',
        kind: 'fm',
        // 还没收到第一个增量时不摆空白：给一句"对面在动"的实情，光标说明还在流
        content: stream.text || '值班长正在查台账…',
        streaming: stream.streaming,
        partial: !stream.streaming && stream.text.length > 0,
        traces: [],
        briefing: null,
        needsPairing: false,
      });
    }
    if (stream.error) {
      out.push({
        key: 'send-error',
        kind: 'failed',
        content: `发送失败：${stream.error}`,
        streaming: false,
        partial: false,
        traces: [],
        briefing: null,
        needsPairing: needsPairing(stream.error),
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
   * **发送中禁止切换**：这是最基本的自保——回包落地时得有一个确定的「这一轮属于谁」。
   * 但它不是彻底隔离：两台设备各说各话不受任何 UI 锁保护，那一层靠上面的 `generation`
   * 与 `appendForemanDelta` 的班次守卫（决策 204⑥）。
   */
  async function switchTo(id: string) {
    if (sending || busy || id === currentId) return;
    generation += 1;
    resetSessionState();
    await reload(id);
  }

  /**
   * 开一个新班次并切过去（空班是合法状态：第一句话说出来时它才得名）。
   *
   * **不带 busy 守卫**：调用方已经持有它。归档最后一个班次那条路就是这样调的
   * ——那时的 busy 必然是 true，若这里再守一次，归档完最后一个班次会静默什么都不做，
   * 页面停在一片空白上（「一个班次都没有」且没有当前班次）。
   */
  async function openFreshSession() {
    const created = await createForemanSession();
    sessionList = [created.session, ...sessionList];
    generation += 1;
    resetSessionState();
    await reload(created.session.id);
  }

  /** 从界面按下「+ 新班次」。 */
  async function newSession() {
    if (sending || busy) return;
    busy = true;
    try {
      await openFreshSession();
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
    // 只在等回话期间累积：收尾后到达的尾巴不得再造一轮（回话以台账为准）
    if (!sending) return;
    // 班次守卫：不是当前这一班的增量一律丢弃（决策 204⑥，判据在 realtime/foreman.ts）
    stream = appendForemanDelta(stream, event, currentId);
  }

  let conn: TaskStream | null = null;

  function onVisible() {
    if (document.visibilityState !== 'visible') return;
    void reload();
    conn?.reconnectNow();
  }

  onMount(() => {
    void reload();
    // 复用任务流的分帧 / 退避 / 主动重连（票 03）：工头流只是换了一条路径
    conn = new TaskStream(
      '',
      {
        onEvent: onStreamEvent,
        onStatus: (_id, status) => (streamStatus = status),
      },
      { path: '/foreman/stream' },
    );
    conn.start();
    // 窄屏断点与 app.css 同一档（479px）：装置顺序与 TaskDetail 的 mobileMq 一致
    narrowMq = window.matchMedia('(max-width: 479px)');
    narrow = narrowMq.matches;
    onNarrowChange = (e: MediaQueryListEvent) => (narrow = e.matches);
    narrowMq.addEventListener('change', onNarrowChange);
    document.addEventListener('visibilitychange', onVisible);
    return () => {
      document.removeEventListener('visibilitychange', onVisible);
      if (narrowMq && onNarrowChange) narrowMq.removeEventListener('change', onNarrowChange);
      conn?.stop();
      conn = null;
    };
  });

  /**
   * 新的一轮落地后把视口带到它那里。
   *
   * **为什么必须有**：窄屏的输入坞钉在底栏上沿，回话落在文档末尾（在屏幕之外）——
   * 不跟过去的话，人发完话只看得到一个空白的对话区，得自己往下拨。宽屏同理，只是滚的是
   * 时间线自己（它才是那个滚动容器）。滚动一律瞬时（§1 原则 4：无缓动），故直接写 scrollTop
   * ——`app.css` 没有 `scroll-behavior: smooth`，赋值即到位。
   */
  function scrollToNewest() {
    if (narrow) {
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
      const res = await sendForemanMessage(text, sid);
      if (gen !== generation) {
        pendingText = null;
        stream = emptyForemanStream();
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
        return;
      }
      stream = failForemanStream(stream, (err as Error).message);
      // 重取成功才撤乐观轮：撤了之后这话由台账那一行承担，不靠重取失败时凭空消失
      if (await reload(sid)) pendingText = null;
    } finally {
      sending = false;
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

  /** 值班长的两个只读工具（`FOREMAN_TOOLS`）；未登记的照原样显示。 */
  const TOOL_LABELS: Record<string, string> = {
    read_task: '读任务台账',
    read_conversation: '读工位会话',
  };

  /** 工位名 → sprite：快照里的 stage 是后端字符串，未登记的值不猜（退回台账箱）。 */
  function stageSprite(stage: string): SpriteName {
    const col = BOARD_COLUMNS.find((c) => c.stages.includes(stage as Stage));
    return col ? COLUMN_SPRITES[col.key] : 'chest';
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
      label: TOOL_LABELS[trace.tool] ?? trace.tool,
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

<div class="talk">
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
      <span>{session?.foreman.wired === false ? '值班长未接线' : '值班中'}</span>
      <span class="sep">▪</span>
      <!-- `工位` 的译文只在这一行（`.stat-wide` 窄屏收起）：窄屏上这个词不再出现
           （值班板的「8 工位」与那两行说明都收进了桌面款），故窄屏没有漏译。 -->
      <span class="stat-wide">夜班态势：8 工位（流水线的阶段）</span>
      <span class="sep stat-wide">▪</span>
      <span>本次会话 {session ? formatTokens(session.total_tokens) : '—'} tok</span>
      <span class="sep">▪</span>
      <a class="crumb" href="#/" onclick={() => router.navigate('/')}>看板</a>
    </div>
  </div>

  <!-- ── 状态区：当前急停。宽屏钉在第一屏，不随时间线滚动；窄屏默认收成摘要条并钉在
       顶栏下沿（`.stops` / `.stop-open` 两个类只在 §5 的移动块里有规则，决策 192）。
       值班板不在本区 ——桌面是右栏、窄屏是时间线之上的横向灯条 ── -->
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
            >页扫码即可——手机上打开这一页是拿不到配对码的。若已添加到主屏幕，**换过令牌后要
            重新添加一次**（图标里记的是当时那条带令牌的地址）。
          </p>
        {/if}
      </div>
    {/if}

    {#each pending as task (task.id)}
      {@const detail = details[task.id]}
      {@const count = stopActionCount(detail)}
      {@const open = isStopOpen(foldable, openStop, task.id)}
      <!-- 急停轮：全站唯一"响"的一处（琥珀框 + ▼ + 恢复动作）。折叠只收动作区，不收身份：
           折叠行的框色 / 硬投影 / ▼ 与展开行完全相同（决策 183） -->
      <article class="turn warn" class:folded={!open}>
        <!-- 名牌是发言者：这一轮是操作台在报"卡住了、要你按键"，不是值班长在说话
             （值班长的话一律没有按钮，见 §3.3 的四条纪律） -->
        <div class="dname">操作台</div>

        {#if open}
          <div class="dtag">⏸ 等你拍板 · {pendingLabel(task.pending_reason)}</div>
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
            <span class="dtag">⏸ 等你拍板 · {pendingLabel(task.pending_reason)}</span>
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

    {#if pending.length === 0}
      <!-- 空态与其余六处同一套语汇（票 13 / parallel-brief §三.3）：状态 → 下一步 → 可选入口，
           凡是提到另一个页面都可点。入口是页面固定的**导航入口**（纯前端路由，不进后端动作契约，
           票 04）——`EmptyState` 的 href 就是它，不再是裸的 `<button>`。
           `货箱` 的译文就在这一句里（本页首现处，同一页面内不重复）。 -->
      <div class="no-stop">
        <EmptyState
          state="当前没有急停（等你拍板的阻塞）。"
          next={board.projects.length === 0
            ? '这台机器还没接入项目——先接一个，流水线才有货箱（一张任务卡）。'
            : '有任务需要你拍板时，它会挂在这里，动作就在那一轮里。'}
          href={emptyBoard ? '#/' : undefined}
          linkLabel="去看板新建任务"
        />
      </div>
    {/if}
  </section>

  <!-- ── 对话时间线：值班长的话、值班经理的话、工位回执。宽屏它是那个会滚、会长的地方；
       窄屏整页去滚，它就是页面本身（§5 移动款，决策 192） ── -->
  <section
    class="timeline"
    class:empty={timelineEmpty}
    bind:this={timelineEl}
    aria-label="对话时间线"
  >
    <!-- ── 班次 chip 行（决策 204③）：时间线顶部、**非 sticky**（随手指滚）。
         词汇复用任务详情页的 `.runchip`——「选一条会话」在那里已经有现成语汇；
         窄屏 `nowrap + 横滚` 也是现成的。**不得渲染成 `.turn`**：时间线里那些是发言，
         而这是一排控件（`talk.spec.ts` 断言时间线里没有 `.turn.warn`）。
         不动页头、不动顶栏：它长在这条**会滚的**时间线里，故 `.talk-head` 的高度
         与对话区的高度都不因它变。 -->
    <div class="runrow no-scrollbar" role="group" aria-label="班次">
      {#each sessionList as s (s.id)}
        <button
          type="button"
          class="runchip"
          class:now={s.id === currentId}
          disabled={sending || busy}
          title={s.title}
          onclick={() => void switchTo(s.id)}
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
        <button
          type="button"
          class="runchip act"
          disabled={sending || busy}
          onclick={openRename}>改名</button
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
      <!-- 发送中为什么点不动（决策 204③的「最基本的自保」）：光把按钮变灰，
           人会以为界面卡了。回话落地得有一个确定的「这一轮属于谁」。 -->
      {#if sending}
        <span class="dim note rwhy">回话中，先别换班次</span>
      {/if}
    </div>

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
      <article
        class="turn"
        class:fm={turn.kind === 'fm'}
        class:mine={turn.kind === 'mine'}
        class:failed={turn.kind === 'failed'}
      >
        <div class="dname">
          {turn.kind === 'failed' ? '发送失败' : turn.kind === 'mine' ? '值班经理' : '值班长'}
        </div>
        <p class:streaming={turn.streaming}>{turn.content}</p>

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
            >页扫码即可——手机上打开这一页是拿不到配对码的。若已添加到主屏幕，**换过令牌后要
            重新添加一次**（图标里记的是当时那条带令牌的地址）。
          </p>
        {/if}

        <!-- 工位回执：转述不是发言（左缘亮度阶 + 无框，形状上就与发言不同）。
             默认展开：回执是这一轮结论的出处，「可追溯性不因对话而丢失」是四条纪律之一 -->
        {#if turn.traces.length > 0}
          <details class="rcpts" open>
            <summary class="rcpts-sum">
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
    {/each}
  </section>

  <!-- ── 输入坞：钉底。Enter 发送 / Shift+Enter 换行；发送中禁用 ── -->
  <form class="typer" onsubmit={(e) => { e.preventDefault(); void send(); }}>
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
      <span class="dim hint">
        {#if sending}
          值班长正在回话…
        {:else if streamStatus === 'error'}
          流断了：回话仍会以台账为准补上。
        {:else if narrow}
          <!-- 窄屏这一行是**常驻**的（留了高度，见 CSS）：空着就是一条 19px 的死白，
               拿它说 §3.3 纪律 4 的那件事（说的每句话都进审计）比留白有用。
               宽屏不写：那里这一行本来就与发送钮同行，不占地方也不缺话说。 -->
          说的每句话都会记进审计
        {/if}
      </span>
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

  <!-- ── 值班板：桌面右栏；窄屏收成对话之上的横向灯条（`crew`） ── -->
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
</div>

<style>
  /* 三分区（票 04）：状态区 / 时间线 / 输入坞自上而下。整页钉在视口内，故时间线是
     唯一会滚的区域——两小时前挂起的急停不会被它顶出视野。 */
  .talk {
    max-width: var(--split-max, 1240px);
    margin: 0 auto;
    padding: 14px 20px 12px;
    display: grid;
    grid-template-columns: minmax(0, 1fr) var(--dossier-w, 340px);
    grid-template-rows: auto auto minmax(0, 1fr) auto;
    gap: 20px 18px;
    /* 视口 = 顶栏 + 整页 + 底栏区 46px（body 下边距）。顶栏实测约 78–81px
       （46px 铭牌行 + 约 33px 页面导航行 + 2px 下框；项目选择器在场时取上限）。
       **多减一点是刻意的**：宁可让页面矮几像素，也不能让它能滚——整页一旦能滚，
       「急停钉在第一屏」就只剩口头保证。 */
    height: calc(100vh - 88px - 46px);
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
  .crumb {
    color: var(--text-3);
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
    max-height: 46vh;
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
  /* 空态里那两样东西**不是一类**：班次 chip 行是控件，空态文案是内容。
     让 flex 把整组居中会让控件浮在大片空白的中间（决策 202 治的就是「内容浮着」），
     故 chip 行照旧贴顶、只把空态文案推到中间。 */
  .timeline.empty > .runrow {
    flex: 0 0 auto;
  }
  .timeline.empty > :global(.empty) {
    margin-top: auto;
    margin-bottom: auto;
  }

  /* ── 班次 chip 行（决策 204③）：词汇照抄任务详情页的 `.runchip` ──
     「选一条会话」在那里已经有现成形状，本页不另造一套药丸。 */
  .runrow {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
    margin-bottom: 16px;
  }
  .runchip {
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
  .rwhy {
    align-self: center;
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

  /* 移动款（§5 转写 4）：页头 → 值班灯条 → 状态区 → 竖排对话 → 钉底输入坞 */
  /* ── 窄屏（§5 移动款，决策 192）：**对话是页面本身** ──
     桌面款把整页钉在视口里、让时间线做唯一的滚动容器；窄屏反过来——整页随手指滚，
     只有两样东西钉住：急停摘要条（钉在顶栏下沿）与输入坞（钉在底栏上沿）。
     两个滚动容器在 430px 宽、740–930px 高的屏上是零和的：钉住的部分每多 100px，
     对话区就少 100px，而对话区正是这一页在手机上唯一的用处。
     钉住的两样各有代价：急停摘要条实测约 66px/张（宽屏是 376px 的展开轮，见 `forceFold`），
     输入坞约 113px（宽屏那种「正文一行、提示与按钮又一行」在这里要吃掉 145px）。 */
  @media (max-width: 479px) {
    .talk {
      display: flex;
      flex-direction: column;
      gap: 14px;
      padding: 10px 12px 0;
      /* 桌面那条 `height` 必须撤掉：两处一起定高，窄屏就没有「长出去」的余地 */
      height: auto;
      /* 视口 = 顶栏 138 + 整页 + 底盘底边距（`app.css` 窄屏把底盘 padding-bottom
         定成了底栏高度 `--sbar-h`，故这里减的是同一个值）。`min-height` 而非 `height`：
         内容长了就长出去（整页滚），短了就撑满余下的屏幕——输入坞于是总在底边。
         底内边距为 0 是**算过**的：让输入坞的底边正好落在底栏顶边（钉住时同一个位置，
         于是「对话短时悬空 16px、长了又贴上去」那种一跳没有了）。
         用 `dvh` 而不是 `vh`：手机上 `vh` 取的是地址栏收起时的高度，地址栏在场时
         整块版面会高出一截，把钉底的输入坞推到屏幕外（`vh` 那行是给不认 `dvh` 的旧内核的）。 */
      min-height: calc(100vh - 138px - var(--sbar-h));
      min-height: calc(100dvh - 138px - var(--sbar-h));
    }
    .talk-head {
      order: 1;
      flex: none;
    }
    /* 班次 chip 行在窄屏**不折行、横向滚**（与任务详情页的 `.runrow` 同一形状）：
       430px 上折行会把三四个班次铺成两三行，而这一档的纵向空间是拿「两只钉住物之间的
       残渣」换来的，不能喂给一排控件。 */
    .runrow {
      flex-wrap: nowrap;
      overflow-x: auto;
    }
    .runchip {
      flex: none;
    }
    /* 标题与元信息同一行：手机上「对讲台」顶栏的页签已经在说，页内不必再铺两行。
       「夜班态势：8 工位」收进宽屏——它说的就是正下方那条 8 工位的灯条。 */
    .stat-wide {
      display: none;
    }
    /* 值班板收成对话之上的横向灯条：横向滚动、不缩不折。
       台账盒的框与头行在窄屏没有意义（它会随手指滚走），只留「值班板」当行首标签。 */
    .talk-side {
      order: 2;
      flex: none;
      position: static;
      /* 桌面那条 `align-self: start` 必须撤掉：灯条要靠父宽约束才会横向滚，
         否则 aside 取 max-content 宽度、把整页撑出横向滚动条 */
      align-self: stretch;
      max-height: none;
      overflow: visible;
    }
    .talk-side .reg {
      display: flex;
      align-items: center;
      gap: 10px;
      border: 0;
      background: none;
    }
    .talk-side .reg-head {
      flex: none;
      padding: 0;
      border-bottom: 0;
      background: none;
    }
    /* 「8 工位」不写：下面 8 枚灯自己说得更清楚（与页头收掉的那句是同一件事） */
    .talk-side .reg-head .n {
      display: none;
    }
    .brows {
      flex: 1;
      min-width: 0;
      display: flex;
      overflow-x: auto;
    }
    .brow {
      flex: none;
      border-bottom: 0;
      border-right: 2px solid var(--wash);
    }
    .brow:last-child {
      border-right: 0;
    }
    .boks {
      display: none;
    }
    /* ── 状态区：钉在顶栏下沿的一条（`.stops`） ──
       桌面那条 38vh 上限（区内滚动）在窄屏撤掉：整页去滚，状态区不再有自己的滚动条。
       钉住只在**收起时**成立——展开的那一张回到普通文档流（`.stop-open`），否则一张
       376px 的轮钉在 900px 的屏上就钉死了整块屏幕；展开那一张要看得见由 `toggleStop`
       滚回它负责。不钉住的是「没急停」的形态：一句「当前没有急停」不值得占一行屏幕。 */
    .zone-status {
      order: 3;
      flex: none;
      padding: 20px 6px 6px 2px;
      max-height: none;
      overflow: visible;
    }
    .zone-status:not(.stops) {
      padding-top: 0;
    }
    .zone-status.stops:not(.stop-open) {
      position: sticky;
      top: 138px; /* = 窄屏顶栏高度（e2e ⑩ 钉住 138）；下边框把灯条与对话分开 */
      z-index: 15;
      background: var(--bg);
      border-bottom: 2px solid var(--hairline);
      /* `scrollIntoView` 停的位置（§5 定值：顶栏 138 + 10） */
      scroll-margin-top: 148px;
      /* 兜底上限：一张摘要条实测约 **66px**（窄屏两行）+ 26px 间距，五张在 900px 屏上
         就是 460px——钉住的东西不能没有上界，否则「对话区太小」会以另一种形状回来。
         到顶之后这条带子自己滚（与桌面那一档同一手法），代价如实写在 §3.3 的残留里：
         被滚出去的那张不再「一直看得见」。2px 右内边距已在基线上留过（见 `.zone-status`），
         故纵向一滚不会连带长出横向滚动条。 */
      max-height: 45vh;
      overflow-y: auto;
    }
    /* 摘要条在窄屏**显式两行**：标签一行，标题与展开钮一行。
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
    /* ── 对话时间线：窄屏它就是页面（没有自己的滚动条，只剩名牌 tab 的上留白） ── */
    .timeline {
      order: 4;
      /* 不缩：内容多高就多高——去滚的是整页，不是它；余量归它，输入坞于是贴底 */
      flex: 1 0 auto;
      /* 桌面那条 `overflow-y: auto` 也要撤掉：窄屏它一旦成了滚动容器，会长出来的
         是它自己而不是整页（滚轮到底也翻不过去），与这一档的版面整个相反 */
      overflow-y: visible;
      padding: 20px 2px 6px;
    }
    .turn p {
      max-width: none;
    }
    /* 手机上每行字数少（约 31 个汉字），行距跟上走：1.6 是按桌面约 76 字符一行配的 */
    .timeline .turn p {
      line-height: 1.8;
    }
    /* ── 输入坞：钉在底栏上沿 ──
       一行制：[空心底的输入框][实心发送] 同一行，提示语挪到下一行并**常驻**（出现时才占位
       会把这颗钉底的框顶得上下跳）。`display: contents` 把桌面的 `.typer-foot` 拆开，
       两个子元素各归各格——桌面款那一套排版因此逐像素不变，不必改标记。 */
    .typer {
      order: 5;
      flex: none;
      position: sticky;
      bottom: var(--sbar-h);
      z-index: 25;
      /* 名牌 tab 悬出框沿 16px，给它留出上沿（也给对话留出与输入坞的分界） */
      margin-top: 26px;
      display: grid;
      grid-template-columns: minmax(0, 1fr) auto;
      grid-template-areas:
        'field send'
        'hint hint';
      gap: 6px 10px;
    }
    .typer textarea {
      grid-area: field;
    }
    .typer-foot {
      display: contents;
    }
    .typer-foot .hint {
      grid-area: hint;
      min-height: 1.6em;
    }
    .typer-foot .btn {
      grid-area: send;
      /* 与输入框齐高（44px 是触控底线，2 行输入框在 16px 字号下约 63px） */
      align-self: stretch;
    }
  }
</style>
