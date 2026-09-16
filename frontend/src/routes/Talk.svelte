<script lang="ts">
  import { onMount } from 'svelte';
  import type {
    AllowedAction,
    BranchCursor,
    ForemanBriefing,
    ForemanSession,
    ForemanTrace,
    SseEvent,
    Stage,
    TaskListItem,
  } from '../api/types';
  import type { SpriteName } from '../theme/contract';
  import { getForemanSession, getTask, sendForemanMessage } from '../api/client';
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
  import { router } from '../router.svelte';

  /**
   * 对讲台（theme-6-pixel.md §3.3；决策 174 / 182 / 183）。版面**三分区**（票 04）：
   * 状态区（当前急停）／对话时间线（值班长的话、值班员的话、工位回执）／输入坞。
   * 值班板是**独立的一块**——桌面右栏、窄屏收成时间线之上的横向灯条，不在状态区里。
   *
   * **整页钉在视口内，时间线是唯一会滚的区域**：一个两小时前挂起的急停被对话顶出视野
   * 是本页最不能出的错，故不靠 sticky 逐段救，而是把「会长的部分」与「不能动的部分」
   * 放在两个不同的滚动容器里。
   *
   * **但状态区自己也有上限**（桌面 46vh / 窄屏 38vh，超高时区内滚），而一张急停轮内联着
   * 后端下发的动作集，最高的形状（带补充输入的 resume）约 330px——13″ 上可用只有约
   * 344px，**展开一张就已经占满整个区**。故急停轮折叠（决策 183）：**只有一张时不动**
   * （单急停版面与折叠前一致），两张以上**一张都不展开**、全部收成一行约 36px 的摘要条；
   * 人点「展开恢复动作」才展开那一张，且同时只展开一张（判据在 `lib/talkStops.ts`）。
   *
   * **对面是真的会说话的值班长**（`/foreman/session` + `/foreman/messages` + `/foreman/stream`）：
   * 对话不依赖任务——空看板（无项目无任务）也照样能问它话。
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

  let input = $state('');
  let sending = $state(false);
  /** 正在发的那句话（台账里还没有它的回话，故先以乐观轮显示）。 */
  let pendingText = $state<string | null>(null);
  let stream = $state<ForemanStreamState>(emptyForemanStream());
  let streamStatus = $state<StreamStatus>('idle');

  /** 每个 pending 任务的详情（allowed_actions 只在详情里下发，决策 101）。 */
  let details = $state<Record<string, { actions: AllowedAction[]; cursors: BranchCursor[] }>>({});

  const pending = $derived(board.pendingTasks);
  /** 空看板：装载完成之后一个任务都没有（装载中不算——那会把「还没读到」说成「空」）。 */
  const emptyBoard = $derived(!board.loading && board.tasks.length === 0);

  /**
   * 展开的那张急停（决策 183）。`undefined` = 还没选过（跟随默认：**只有一张时展开它，
   * 两张以上一张都不展开**）、`null` = 人显式收起、否则是那张的 id。三态的判据在
   * `lib/talkStops.ts`，此处只持状态。
   */
  let chosenStop = $state<string | null | undefined>(undefined);
  const stopIds = $derived(pending.map((t) => t.id));
  const foldable = $derived(isFoldable(stopIds));
  const openStop = $derived(resolveOpenStop(stopIds, chosenStop));

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

  async function reload(): Promise<boolean> {
    try {
      session = await getForemanSession();
      loadError = null;
      return true;
    } catch (err) {
      loadError = (err as Error).message;
      return false;
    } finally {
      loading = false;
    }
  }

  function onStreamEvent(_taskId: string, event: SseEvent) {
    // 只在等回话期间累积：收尾后到达的尾巴不得再造一轮（回话以台账为准）
    if (!sending) return;
    stream = appendForemanDelta(stream, event);
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
    document.addEventListener('visibilitychange', onVisible);
    return () => {
      document.removeEventListener('visibilitychange', onVisible);
      conn?.stop();
      conn = null;
    };
  });

  async function send() {
    const text = input.trim();
    if (!text || sending) return;
    sending = true;
    pendingText = text;
    stream = beginForemanStream();
    try {
      const res = await sendForemanMessage(text);
      // 回话是权威值：先收敛流式文本（重取台账期间不闪空），再以台账覆盖
      stream = settleForemanStream(stream, res.reply);
      input = '';
      if (await reload()) {
        pendingText = null;
        stream = emptyForemanStream();
      }
    } catch (err) {
      // 失败不改输入框内容：后端在叫模型之前已把 user 行落库，人改几个字就能重发
      // （决策 182㉓）。失败以时间线里的一轮呈现——不弹窗、不 toast。
      stream = failForemanStream(stream, (err as Error).message);
      // 重取成功才撤乐观轮：撤了之后这话由台账那一行承担，不靠重取失败时凭空消失
      if (await reload()) pendingText = null;
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
      <span>{session?.foreman.wired === false ? '值班长未接线' : '值班中'}</span>
      <span class="sep">▪</span>
      <span>夜班态势：8 工位</span>
      <span class="sep">▪</span>
      <span>本次会话 {session ? formatTokens(session.total_tokens) : '—'} tok</span>
      <span class="sep">▪</span>
      <a class="crumb" href="#/" onclick={() => router.navigate('/')}>看板</a>
    </div>
  </div>

  <!-- ── 状态区：当前急停。钉在第一屏，不随时间线滚动（票 04）。值班板不在本区
       ——桌面是右栏、窄屏是时间线之上的横向灯条 ── -->
  <section class="zone-status" aria-label="值班台">
    {#if loadError}
      <div class="blank error">
        <p>{loadError}</p>
        <!-- 配对入口在**两条**失败路径上都要给（票 07）：读会话与发话各自会撞 403，
             只在其中一处给链接，另一处的使用者就只看到一句「这台设备还没配对」而无处可去。 -->
        {#if needsPairing(loadError)}
          <p class="note">
            去看板顶栏的<a
              class="crumb"
              href="#/share"
              onclick={() => router.navigate('/share')}>手机访问</a
            >页，在已配对的设备上重扫一次二维码即可。
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
              onclick={() => (chosenStop = toggleOpenStop(openStop, task.id))}
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
            <span class="st">
              「{task.title}」{#if count !== null}· {count} 个动作{/if}
            </span>
            <button
              type="button"
              class="expander"
              aria-expanded={open}
              onclick={() => (chosenStop = toggleOpenStop(openStop, task.id))}
            >
              展开恢复动作 ▸
            </button>
          </div>
        {/if}
      </article>
    {/each}

    {#if pending.length === 0}
      <div class="no-stop">
        <p>
          当前没有急停。
          {board.projects.length === 0 ? '这台机器还没接入项目——先接一个，流水线才有货箱。' : ''}
        </p>
        <!-- 导航钮：页面固定，不进后端动作契约（票 04） -->
        {#if emptyBoard}
          <button type="button" class="btn" onclick={() => router.navigate('/')}>去看板新建任务</button>
        {/if}
      </div>
    {/if}
  </section>

  <!-- ── 对话时间线：值班长的话、值班员的话、工位回执。会滚、会长的那部分 ── -->
  <section class="timeline" aria-label="对话时间线">
    {#if loading && !session}
      <div class="quiet">正在读会话台账…</div>
    {:else if turns.length === 0}
      <div class="quiet">还没有对话。说一句，值班长就在对面——它与任务无关，空班也答得上。</div>
    {/if}

    {#each turns as turn (turn.key)}
      <article
        class="turn"
        class:fm={turn.kind === 'fm'}
        class:mine={turn.kind === 'mine'}
        class:failed={turn.kind === 'failed'}
      >
        <div class="dname">
          {turn.kind === 'failed' ? '发送失败' : turn.kind === 'mine' ? '值班员' : '值班长'}
        </div>
        <p class:streaming={turn.streaming}>{turn.content}</p>

        {#if turn.partial}
          <p class="dim note">流断了，上面是已经收到的部分；完整回话会在台账里补齐。</p>
        {/if}

        <!-- 配对入口（决策 182㉙，票 07）：非回环形态下缺令牌时后端回 403，报文里已经说清
             「这台设备还没配对」。这里补的是**动作**——报文让人知道发生了什么，链接让人知道
             下一步去哪。只在 403 且报文提到配对时出现，普通失败不挂这个出口。 -->
        {#if turn.needsPairing}
          <p class="note">
            去看板顶栏的<a
              class="crumb"
              href="#/share"
              onclick={() => router.navigate('/share')}>手机访问</a
            >页，在已配对的设备上重扫一次二维码即可。
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
    <div class="dname">值班员</div>
    <textarea
      class="input"
      rows="2"
      placeholder="对值班长说一句话（Enter 发送，Shift+Enter 换行）…"
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
        {/if}
      </span>
      <button type="submit" class="btn solid" disabled={sending || !input.trim()}>发送</button>
    </div>
  </form>

  <!-- ── 值班板：桌面右栏；窄屏收成对话之上的横向灯条（`crew`） ── -->
  <aside class="talk-side crew">
    <div class="reg">
      <div class="reg-head"><span>值班板</span><span class="n">8 工位</span></div>
      <ul class="brows">
        {#each crew as c (c.key)}
          <li class="brow {c.state === 'warn' ? 'pen' : c.state === 'run' ? 'hot' : ''}">
            <span class="blamp {c.state === 'warn' ? 'w' : c.state === 'run' ? 'c' : c.state === 'done' ? 'd' : ''}"></span>
            <span class="bnm">{c.label}</span>
            <span class="bc">{c.count}</span>
          </li>
        {/each}
      </ul>
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
  /* 值班员的话与值班长的话只差名牌停靠与明暗档：不换底色、不换圆角、不加箭头（§3.3 纪律 1） */
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
  .st {
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
  .dim {
    color: var(--text-4);
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
    color: var(--text-4);
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
  @media (max-width: 479px) {
    .talk {
      display: flex;
      flex-direction: column;
      gap: 14px;
      padding: 10px 12px 12px;
      /* 移动款顶栏 140px（138 + 2px 下框，e2e ⑩ 钉住这个定值）；底栏区同上。
         同样多留 4px 余量（理由见桌面款） */
      height: calc(100vh - 144px - 46px);
    }
    .talk-head {
      order: 1;
    }
    /* 值班板收成对话之上的横向灯条：横向滚动、不缩不折 */
    .talk-side {
      order: 2;
      position: static;
      /* 桌面那条 `align-self: start` 必须撤掉：灯条要靠父宽约束才会横向滚，
         否则 aside 取 max-content 宽度、把整页撑出横向滚动条 */
      align-self: stretch;
      max-height: none;
      overflow: visible;
    }
    .brows {
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
    .zone-status {
      order: 3;
      max-height: 38vh;
    }
    .timeline {
      order: 4;
      flex: 1;
      min-height: 0;
    }
    .typer {
      order: 5;
      flex: none;
    }
    .turn p {
      max-width: none;
    }
  }
</style>
