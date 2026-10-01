<script lang="ts">
  import { onDestroy, onMount, tick } from 'svelte';
  import { archiveTask, getForemanSessions, listProviders, pauseTask, rerunTask, retryTask, setStewardship } from '../api/client';
  import type { AllowedAction, Provider } from '../api/types';
  import PipelineRail from '../components/pipeline/PipelineRail.svelte';
  import DiffReviewPanel from '../components/task/DiffReviewPanel.svelte';
  import FileViewer from '../components/task/FileViewer.svelte';
  import ModelOverrideDialog from '../components/task/ModelOverrideDialog.svelte';
  import PendingDossier from '../components/task/PendingDossier.svelte';
  import SceneTimeline from '../components/task/SceneTimeline.svelte';
  import SplitDialog from '../components/task/SplitDialog.svelte';
  import TimelineView from '../components/task/TimelineView.svelte';
  import DiffView from '../components/render/DiffView.svelte';
  import EmptyState from '../components/ui/EmptyState.svelte';
  import { buildHeroStations, formatDuration, formatTokens, pendingLabel, statusCode } from '../lib/pipeline';
  import { stewardshipFace, toggleStewardship } from '../lib/stewardship';
  import { router, writeQuery } from '../router.svelte';
  import { taskDetail } from '../stores/taskDetail.svelte';

  interface Props {
    id: string;
  }
  let { id }: Props = $props();

  /**
   * 「现场」页签（决策 349）：会话与命令输出合并成的一条时间线，版式取对讲台那套
   * （一叠轮、名牌、过程步骤、命令回执）。旧「会话」「命令与输出」两个页签由此退场——
   * 两套版面说的是同一件事的两半，合在一处之后「只有命令没有会话的节点」也有了
   * 同一个形状（`lib/taskScene.ts` 把孤儿命令按 stage · node 分组成合成轮）。
   */
  type Tab = 'timeline' | 'scene' | 'files' | 'diff';
  /** 页签顺序（方向键与 Home/End 按它走）与各自的词（面板标题用它）。 */
  const TAB_ORDER: Tab[] = ['timeline', 'scene', 'files', 'diff'];
  const TAB_LABELS: Record<Tab, string> = {
    timeline: '时间线',
    scene: '现场',
    files: '产出文件',
    diff: 'Diff',
  };
  let tab = $state<Tab>('timeline');
  /**
   * 深链 / 跳转要带到眼前的那个 run（`?run=` 消费一次、档案盒的「去看对话」各写一次）；
   * `null` = 没有落点。旧「会话页签的选中 run」的变体：现场时间线不搞选中态，
   * 只把那一轮滚进视野并亮一下边框。
   */
  let highlightRun = $state<number | null>(null);
  let splitOpen = $state(false);
  let modelOpen = $state(false);
  let providers = $state<Provider[]>([]);
  let dialogError = $state<string | null>(null);
  let bypassBusy = $state<string | null>(null);
  /**
   * 值班长接线没有（决策 210① / 票 14）：未接线时托管开关无处可去，不摆。
   *
   * 读法用 `/foreman/sessions`（小载荷）而不是 `/foreman/session`（连消息一起回）——
   * 这里只要那个「接没接」的答案，而 503 就是答案。
   */
  let foremanWired = $state(false);
  let stewardBusy = $state(false);
  let stewardNote = $state<string | null>(null);
  let stewardError = $state<string | null>(null);
  /** 窄屏（<480px）：hero 轨道转纵向脊线（§5 移动款）。 */
  let isMobile = $state(
    typeof window !== 'undefined' && window.matchMedia('(max-width: 479px)').matches,
  );
  /** 底部动作坞实测高度：内容据此留出底边距（0 时回退到原型 122px）。 */
  let dockH = $state(0);

  const detail = $derived(taskDetail.state);
  const task = $derived(detail.task);
  const pendingReason = $derived(detail.pendingReason ?? task?.pending_reason ?? null);
  const isPending = $derived(task?.status === 'pending' && pendingReason !== null);
  const isTerminal = $derived(
    task?.status === 'done' || task?.status === 'failed' || task?.status === 'cancelled',
  );

  const heroStations = $derived(
    task
      ? buildHeroStations({
          status: task.status,
          current_stage: task.current_stage,
          stalled: task.stalled,
          branches: detail.cursors,
        })
      : [],
  );

  const focalCursor = $derived(
    detail.cursors.find((c) => c.status !== 'archived') ?? detail.cursors[0],
  );
  const pendingType = $derived(pendingReason?.type ?? null);
  const durationMs = $derived(
    task ? Math.max(0, Date.parse(task.updated_at) - Date.parse(task.created_at)) : 0,
  );
  const showDiffTab = $derived(
    pendingType === 'merge_approval' ||
      pendingType === 'human_review' ||
      task?.current_stage === 'merge' ||
      task?.current_stage === 'done',
  );
  /** 托管开关这一面（`null` = 不摆，见 `lib/stewardship.ts`）。 */
  const steward = $derived(task ? stewardshipFace(task, foremanWired) : null);
  /** 「这个 id 没有」与「没读到」是两回事（票 01）：404 说前者，其余说后者。 */
  const notFound = $derived(taskDetail.error !== null && taskDetail.errorStatus === 404);

  /**
   * 任务级入口的地址形状是 brief §二 末尾那张跨流接口表的**契约**（票 06 / 07），逐字照抄：
   * 指标页据 `query.task` 自动载入并高亮（消费方 Me）、项目页据 `query.project` + `analyze=1`
   * 自动就位并触发分析（消费方 S）。参数名不是自由发挥——写错一个字母，对面那页就不就位。
   * 放在 `$derived` 里而不是模板里拼：`&` 在模板属性里要走实体，容易写成一个字面 `&amp;`。
   */
  const metricsHref = $derived(task ? `#/metrics?task=${task.id}` : '');
  const analyzeHref = $derived(
    task ? `#/settings/projects?project=${task.project_id}&analyze=1` : '',
  );

  let mobileMq: MediaQueryList | null = null;
  let onMobileChange: ((e: MediaQueryListEvent) => void) | null = null;

  onMount(() => {
    mobileMq = window.matchMedia('(max-width: 479px)');
    isMobile = mobileMq.matches;
    onMobileChange = (e: MediaQueryListEvent) => (isMobile = e.matches);
    mobileMq.addEventListener('change', onMobileChange);
    void taskDetail.load(id);
    void listProviders()
      .then((list) => (providers = list))
      .catch(() => (providers = []));
    // 「值班长接线没有」的读法（见 `foremanWired` 的说明）：读得到就是接线的。
    void getForemanSessions()
      .then(() => (foremanWired = true))
      .catch(() => (foremanWired = false));
  });

  onDestroy(() => {
    if (mobileMq && onMobileChange) mobileMq.removeEventListener('change', onMobileChange);
  });

  // pending 变化时预取 dossier 所需材料
  $effect(() => {
    if (pendingType === 'merge_approval') {
      void taskDetail.loadDiff('merge-proposal.diff');
    } else if (pendingType === 'human_review') {
      void taskDetail.loadDiff('review-diff.diff');
      void taskDetail.loadFile('review-report.md');
      void taskDetail.loadFile('test-report.md');
    }
  });

  function handleAction(action: AllowedAction, opts: { cursorId?: string; input?: string }) {
    if (action.action === 'split_task') {
      dialogError = null;
      splitOpen = true;
      return;
    }
    if (action.action === 'model_override') {
      dialogError = null;
      modelOpen = true;
      return;
    }
    void taskDetail.runAllowedAction(action, opts).catch(() => undefined);
  }

  /**
   * 页签的方向键（票 06 / R2-19）：`role="tablist"` 的契约要求方向键能在页签间走
   * （左右循环、Home/End 到两端），并且**焦点跟着选中的页签走**（roving tabindex）——
   * 只在选中项上按左右键也能一路切过去。
   */
  function onTabKey(e: KeyboardEvent) {
    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(e.key)) return;
    const list = TAB_ORDER.filter((t) => t !== 'diff' || showDiffTab);
    if (list.length === 0) return;
    e.preventDefault();
    const i = list.indexOf(tab);
    const next =
      e.key === 'Home'
        ? list[0]
        : e.key === 'End'
          ? list[list.length - 1]
          : e.key === 'ArrowLeft'
            ? list[(i - 1 + list.length) % list.length]
            : list[(i + 1) % list.length];
    tab = next;
    void tick().then(() => document.getElementById(`tab-${next}`)?.focus());
  }

  /**
   * 档案盒的「去看对话」落点（决策 349）：跳到现场页签，把那一轮滚进视野。
   * 完整会话顺手装上（`loadConversation` 有缓存，重复点不重复发）。
   */
  function gotoconversation(stage: string, node: string) {
    const match = detail.conversations.find((c) => c.stage === stage && c.node === node);
    tab = 'scene';
    if (match) {
      highlightRun = match.run_id;
      void taskDetail.loadConversation(match.run_id);
    }
  }

  /**
   * 通知深链 `?run=<id>` 的消费（pwa-webpush 票 02/03，决策 323）。
   *
   * 浏览器推送落在锁屏上，点开后要**直接到那一次对话**——值班长回话的推送落在它的会话上，
   * 失败推送落在失败那次 run 上（地址由 `notify.rs::attention_deep_link` 拼出）。形状是
   * `#/task/<task_id>?run=<run_id>`，与 `#/metrics?task=` 同一族的查询串契约。
   *
   * 三段语义，缺一段都会出毛病：
   * ① **消费一次就抹掉**（`replace`，不进历史）：留着它，用户切到时间线后地址栏还在说
   *    「看第 42 次对话」，刷新又会把人拽回会话页签；抹掉之后地址始终描述屏幕上那一屏。
   *    用 `replace` 而不是 `push`：深链的落点不是「用户走过的一步」，后退应当离开本页。
   * ② **`consumedRun` 记忆**挡住重复消费：抹地址会改 hash，不记一笔就自己咬自己。
   *    它同时是「同一条推送点第二次」的修复路径——抹掉参数后地址真的变了，那次改址会把
   *    run 重新交给这里；`?run=` 一直挂着时第二次点开是同一地址，浏览器不发 `hashchange`，
   *    什么也不会发生。
   * ③ **非法值清掉不留**：`?run=abc` / `?run=0` / `?run=-1` 不是「定位失败」，是脏地址；
   *    留着它只会让人以为页面没反应。
   *
   * 不校验「这个 run 属不属于这个 task」——那是服务端的事（`GET /tasks/:id/conversations/:run`
   * 查不到自会报错），前端多一道白名单只会把合法的深链挡在门外。
   */
  let consumedRun: number | null = null;
  $effect(() => {
    const r = router.route;
    if (r.name !== 'task' || r.id !== id) return;
    const raw = r.query.run;
    if (raw === undefined) return;
    const runId = Number(raw);
    if (!Number.isInteger(runId) || runId <= 0) {
      writeQuery({ run: null }, { replace: true });
      return;
    }
    if (consumedRun === runId) return;
    consumedRun = runId;
    highlightRun = runId;
    void taskDetail.loadConversation(runId);
    tab = 'scene';
    writeQuery({ run: null }, { replace: true });
  });

  /**
   * 现场页签的批量装载（决策 349）：进页签就把每一轮的完整会话读齐（缓存挡住重复），
   * 时间线才摆得开——旧「会话页签」是选中哪轮读哪轮，合并版没有选中态可搭。
   *
   * 同时把「页签在屏」告诉 store（决策 365）：commands 只在它要的地方保鲜——进场当场
   * 补拉一次，在屏期间静默 refetch 也重拉（361 以为 SSE 会承担增量，服务端其实从不发
   * 那两类事件，于是开屏后跑的命令一直看不见）。
   */
  $effect(() => {
    taskDetail.setSceneVisible(tab === 'scene');
    if (tab === 'scene') void taskDetail.loadAllConversations();
  });

  function handleDockHeight(h: number) {
    dockH = h;
  }

  async function submitSplit(tasks: Parameters<typeof taskDetail.submitSplit>[0]) {
    dialogError = null;
    try {
      await taskDetail.submitSplit(tasks);
      splitOpen = false;
    } catch (err) {
      dialogError = (err as Error).message;
    }
  }

  async function submitModel(providerId: string) {
    dialogError = null;
    try {
      await taskDetail.submitModelOverride(providerId);
      modelOpen = false;
    } catch (err) {
      dialogError = (err as Error).message;
    }
  }

  async function bypass(kind: 'retry' | 'archive') {
    bypassBusy = kind;
    taskDetail.actionError = null;
    try {
      if (kind === 'retry') await retryTask(id);
      else await archiveTask(id);
      await taskDetail.load(id, true);
    } catch (err) {
      // 静默失败 = 用户点了「重试」而屏上什么都没发生（票 02 / R2-08）：按钮静静复活，
      // 人不知道出了事。走页面既有的动作错误位（`actionError`），不再让 rejection 落地。
      taskDetail.actionError = (err as Error).message;
    } finally {
      bypassBusy = null;
    }
  }

  /**
   * 拨托管开关（决策 210①，票 14）。
   *
   * 成功之后**重读任务**：托管那一列是端点的回读值，界面不自己拼一个「应该是这样」的
   * 本地态——开了没开以库里那一列为准（`lib/stewardship.ts` 的说明）。
   */
  async function toggleSteward() {
    if (!steward) return;
    stewardBusy = true;
    stewardError = null;
    stewardNote = null;
    const result = await toggleStewardship(id, !steward.enabled, { set: setStewardship });
    if (result.ok) {
      stewardNote = result.note;
      await taskDetail.load(id, true);
    } else {
      stewardError = result.message;
    }
    stewardBusy = false;
  }

  /**
   * 非 pending 任务的两颗手动钮（决策 276）：**暂停**与**重跑本阶段**。
   *
   * 只在 `running` 上摆：暂停的前提是「已被准入 + 有在跑的游标」，而那正是 `running`
   * 这一档；`queued` / `waiting` 还没开跑（后端会拒，报文说清为什么），终态有自己的
   * 两颗（`bypassActions`）。「续跑」不在这里——按住之后任务变 pending，那颗钮由后端的
   * `allowed_actions` 下发，与其余每一种待办同一套机制（界面不写第二份）。
   */
  const canHold = $derived(task?.status === 'running');

  /** 暂停：走 `bypassBusy` 那套忙态与错误位（与既有的旁路动作同一副面孔）。 */
  async function hold(kind: 'pause' | 'rerun') {
    bypassBusy = kind;
    taskDetail.actionError = null;
    taskDetail.actionNote = null;
    try {
      const result = kind === 'pause' ? await pauseTask(id) : await rerunTask(id);
      // 后端那句报文带**事实**（有没有真的通知到在跑的执行体）——照原样显示，不自己拼。
      taskDetail.actionNote = result.message;
      await taskDetail.load(id, true);
    } catch (err) {
      // 与 bypass 同一条纪律：吞掉 rejection = 点了钮而屏上什么都没发生。
      taskDetail.actionError = (err as Error).message;
    } finally {
      bypassBusy = null;
    }
  }
</script>

{#snippet bypassActions()}
  {#if isTerminal}
    <div class="bypass">
      {#if task?.status !== 'done'}
        <button type="button" class="btn danger" disabled={bypassBusy !== null} onclick={() => bypass('retry')}>
          重试（回到 init）
        </button>
      {/if}
      <button type="button" class="btn quiet" disabled={bypassBusy !== null} onclick={() => bypass('archive')}>
        归档
      </button>
    </div>
  {:else if canHold}
    <!-- 在跑的任务（决策 276）：按住 / 从本阶段入口重来。续跑那一步等按住之后再出现
         （任务转 pending，动作坞换成 allowed_actions 下发的那一组）。 -->
    <div class="bypass">
      <button type="button" class="btn" disabled={bypassBusy !== null} onclick={() => hold('pause')}>
        暂停
      </button>
      <button type="button" class="btn quiet" disabled={bypassBusy !== null} onclick={() => hold('rerun')}>
        重跑本阶段
      </button>
    </div>
  {/if}
{/snippet}

<!-- `--dock-h` 兜底 64px = 收起态手柄的估高（决策 281：坞默认收成一行）；实测值由
     PendingDossier 的 ondockheight 写回，只兜首帧那一格。 -->
<main
  class="detail"
  class:split={isPending}
  class:docked={isMobile && isPending}
  style="--dock-h: {dockH > 0 ? `${dockH}px` : '64px'}"
>
  <div class="main">
    <!-- 桌面档原先在这里挂一条「← 看板」面包屑（决策 240 摘除）：看板已是顶栏的一枚页签，
         详情不必再代它递一次入口。这一行因此直接进正文，`<h1>` 仍是本页唯一的标题。 -->
    {#if task && taskDetail.error}
      <!-- 后台对齐（refetch）失败：内容还在，只把「这次没刷新上」挂出来。
           没有 task 的那一态（进页面 / 换 id 失败）由下面的空分支自己说，
           否则同一件事会在屏上出现两遍（票 01）。这颗「重试」是用户看得见的那条出路
           ——不指望他等到下一次 10s 对齐或切一次标签页（票 02 / R2-07）。 -->
      <div class="banner error" role="alert">{taskDetail.error}</div>
      <div class="reload">
        <button type="button" class="btn" onclick={() => void taskDetail.load(id, true)}>重试</button>
      </div>
    {/if}

    {#if taskDetail.connectionState === 'error'}
      <!-- 与看板同一句话（票 13 / R2-15）：同一件事别两套说法；括号里说清**本页**的差别
           ——详情页没有看板那种 10s 对齐，流断了就是真的不更新了。 -->
      <div class="banner" role="status">实时流已断开，正在重连…（这期间本页不会自动更新）</div>
    {/if}

    {#if taskDetail.actionNote}
      <div class="banner" role="status">{taskDetail.actionNote}</div>
    {/if}

    {#if taskDetail.actionError}
      <!-- 动作提交失败必须可见（主流程票 03）：吞掉它 = 用户点「重试」毫无反应的死面板。
           `role=alert` 让它在读屏里也说一声（票 02 / R2-06）——红颜色只说给看得见的人。 -->
      <div class="banner error" role="alert">动作提交失败：{taskDetail.actionError}</div>
    {/if}

    {#if task}
      {#if isMobile}
        <div class="bar-row">
          <h1 class="d-title bt">{task.title}</h1>
          <!-- 标题行只放短码（原型 `.bar-row` 写 WAIT / RUN）：完整状态句在下一行
               `.dmeta` 里，两处都写全句会让同一件事在屏上出现两遍并挤掉标题 -->
          <span class="mark" class:pending={isPending} class:run={!isPending && !isTerminal}>
            {statusCode(task.status)}
          </span>
        </div>
        <div class="dmeta">
          <span class="status" class:pending={isPending} class:run={!isPending && !isTerminal}>
            {#if isPending}
              <span class="st wait">pending</span> ▪ {pendingLabel(pendingReason)}
            {:else}
              <span class="st {isTerminal ? 'dim' : 'run'}">{task.status}</span>
              {#if focalCursor && !isTerminal}· {focalCursor.branch}.{focalCursor.node}{/if}
            {/if}
          </span>
          <span>{formatDuration(durationMs)}</span>
          <span>{formatTokens(task.total_tokens)} tok · {task.total_calls} 次调用</span>
          <span class="mono">{task.id}</span>
        </div>
        {@render bypassActions()}
      {:else}
        <div class="d-head">
          <h1 class="d-title">{task.title}</h1>
          <div class="dmeta">
            <span class="status" class:pending={isPending} class:run={!isPending && !isTerminal}>
              {#if isPending}
                <span class="st wait">pending</span>
                <span class="sep">▪</span>{pendingLabel(pendingReason)}
              {:else}
                <span class="st {isTerminal ? 'dim' : 'run'}">{task.status}</span>
                {#if focalCursor && !isTerminal}· {focalCursor.branch}.{focalCursor.node}{/if}
              {/if}
            </span>
            <span class="big num">{formatDuration(durationMs)}</span>
            <span class="big num">{formatTokens(task.total_tokens)}<span class="unit">tok</span></span>
            <span>{task.total_calls} 次调用</span>
            <span class="mono">{task.id}</span>
            {#if task.branch_name}<span class="mono">{task.branch_name}</span>{/if}
            <span>评审：{task.review_mode}</span>
            {#if task.model_override}<span class="mono">model: {task.model_override}</span>{/if}
          </div>
          {@render bypassActions()}
        </div>
      {/if}

      <!-- 任务级入口（票 06 / 07）：从任务就能到「这个任务的指标」与「所属项目的分析」，
           不必手打 ULID、也不必先去设置里找项目。提到别的页面就给 **可点的入口**（票 13 §三.3）。
           两处都是 `?k=v` 深链，目标页据 query 自动就位——入口只是把地址递过去，不做预判。 -->
      <div class="entries">
        <a class="entry" href={metricsHref}>这个任务的指标 ▸</a>
        {#if task.project_id}
          <a class="entry" href={analyzeHref}>分析所属项目 ▸</a>
        {/if}
      </div>
      {#if steward}
        <!-- 托管开关（决策 210① / 票 08 的端点、票 14 的界面）：**任务级**授权，不是全局档位
             ——它随任务自限。说明那一行不能省：同一个钮开着时「它现在能免按键做什么」与关着时
             完全不同，而钮上只有两个字的差别。 -->
        <div class="steward">
          <span class="s-lbl">值班长</span>
          <button
            type="button"
            class="btn"
            class:on={steward.enabled}
            disabled={stewardBusy}
            aria-pressed={steward.enabled}
            onclick={toggleSteward}
          >
            {steward.label}
          </button>
          <span class="s-note" class:bad={stewardError !== null}>
            {stewardError ?? stewardNote ?? steward.note}
          </span>
        </div>
      {/if}
      <div class="hero-rail">
        <PipelineRail variant={isMobile ? 'vrail' : 'hero'} stations={heroStations} />
      </div>
      {#if !isMobile}
        <!-- 图例：灯即状态（不使用 ✓ ● ○ 字符；与 hero 同一套信号灯图元）。
             `急停` 是车间隐喻词在**本页**的首次出现处 → 给一次平实说法（决策 200 的定稿说法，
             逐字照抄；行内全宽括号、同字号同色档，本页不再重复解释）。 -->
        <div class="legend">
          <span class="lbl">灯</span>
          <i class="sw d"></i><span class="lg">已完成</span>
          <i class="sw c"></i><span class="lg">执行中</span>
          <i class="sw idle"></i><span class="lg">未开始</span>
          <i class="sw w"></i><span class="lg">急停（等你拍板的阻塞）</span>
          <i class="sw x"></i><span class="lg">失败</span>
          <span class="lg dash">↩ 已打回</span>
        </div>
      {/if}

      {#if detail.terminal}
        <div class="terminal {detail.terminal}">任务已{detail.terminal === 'done' ? '完成' : detail.terminal === 'failed' ? '失败' : '取消'}。</div>
      {/if}

      <!-- 真页签（票 06 / R2-19）：`role=tablist` + 每格 `role=tab` + `aria-selected`
           + `aria-controls` 指向面板；方向键在页签间走、焦点跟着选中项走（roving tabindex）。
           元素从 `<nav>` 换成 `<div>`：页签不是导航地标，`nav` + `tablist` 这个组合本身
           就是错的（Svelte 的 a11y 检查直接点名）。
           容器**不必可聚焦**：焦点住在那几格上（选中的那一格 `tabindex=0`）——这是 APG 的
           tablist 模式，Svelte 那条检查不认，故就地忽略并在此说明。 -->
      <!-- svelte-ignore a11y_interactive_supports_focus -->
      <div
        class="tabs"
        class:no-scrollbar={isMobile}
        role="tablist"
        aria-label="任务详情页签"
        onkeydown={onTabKey}
      >
        {#each TAB_ORDER as t (t)}
          {#if t === 'diff' && !showDiffTab}
            <button type="button" class="tab dis" role="tab" aria-selected="false" disabled>
              Diff ─ merge 后生成
            </button>
          {:else}
            <button
              type="button"
              class="tab"
              class:on={tab === t}
              id={`tab-${t}`}
              role="tab"
              aria-selected={tab === t}
              aria-controls="detail-pane"
              tabindex={tab === t ? 0 : -1}
              onclick={() => (tab = t)}
            >
              {TAB_LABELS[t]}{#if t === 'scene'}<span class="c">{detail.conversations.length + detail.commands.length}</span>{/if}
            </button>
          {/if}
        {/each}
      </div>

      <div class="pane" id="detail-pane" role="tabpanel" aria-labelledby={`tab-${tab}`}>
        <!-- 面板的标题（票 06 / R2-20）：详情页此前从 h1 直接跳到 Diff 里的 h4，
             中间整两级没人管——读屏按标题跳转时等于没有落点。视觉上不摆（页签已经写着）。 -->
        <h2 class="visually-hidden">{TAB_LABELS[tab]}</h2>
        {#if tab === 'timeline'}
          <TimelineView
            transitions={detail.transitions}
            currentBranch={focalCursor?.branch}
            currentStage={focalCursor?.stage}
            currentNode={focalCursor?.node}
          />
        {:else if tab === 'scene'}
          <SceneTimeline
            conversations={detail.conversations}
            conversationFor={(runId) => taskDetail.conversationsFull[runId]}
            commands={detail.commands}
            liveDeltas={detail.liveDeltas}
            liveTools={detail.liveTools}
            liveDroppedRuns={detail.liveDroppedRuns}
            commandOutputFor={(c) => taskDetail.outputFor(c)}
            commandErrorFor={(c) => taskDetail.commandOutputError[c.id] ?? null}
            onloadCommand={(cmdId) => taskDetail.loadCommandOutput(cmdId)}
            highlightRunId={highlightRun}
          />
        {:else if tab === 'files'}
          <FileViewer loaded={taskDetail.files} onload={(path) => void taskDetail.loadFile(path)} />
        {:else if tab === 'diff'}
          {#if pendingType === 'merge_approval'}
            <DiffReviewPanel
              diff={taskDetail.diff}
              raw={taskDetail.diffRaw}
              stale={taskDetail.diffStale}
              error={taskDetail.diffError}
              loading={taskDetail.diffError === null && taskDetail.diff === null}
              actions={detail.allowedActions}
              cursors={detail.cursors}
              busy={taskDetail.busyKey !== null}
              onaction={handleAction}
              onreload={() => void taskDetail.loadDiff('merge-proposal.diff')}
            />
          {:else}
            <DiffView parsed={taskDetail.diff} raw={taskDetail.diffRaw} />
          {/if}
        {/if}
      </div>
    {:else if taskDetail.loading}
      <div class="hint">正在加载任务…</div>
    {:else}
      <!-- 打不到任务也是一种空态（票 13）：状态 → 下一步（入口由决策 240 摘除——回看板走顶栏
           那枚页签，这里不再自带一条）。失败态再给一条「重新加载」（票 01 / R2-01）——首次加载
           失败在服务器恢复后不会自愈（流没接上，visibility 恢复遍历的是空连接表），没有这颗钮
           就是永久死页。「这个 id 没有」与「没读到」分开说：前者重试无意义，后者重试是唯一的
           出路。失败原因单独一行并进 live region（票 02 / R2-06）——读屏也要听得到。 -->
      <EmptyState
        state={notFound ? `任务不存在：${id}` : `任务没能打开：${id}`}
        next={notFound
          ? '这个 id 没有对应的任务。它可能已经被删掉，或者地址抄漏了一位。'
          : '没能读到这个任务。'}
      />
      {#if taskDetail.error}
        <div class="banner error" role="alert">{taskDetail.error}</div>
        <div class="reload">
          <button type="button" class="btn" onclick={() => void taskDetail.load(id)}>重新加载</button>
        </div>
      {/if}
    {/if}
  </div>

  {#if task && isPending && pendingReason}
    <!-- 票 08：把「用户此刻在哪个页签」告诉档案盒——停在 Diff 页签时右栏不再摆第二份 diff
         （动作行照旧在，那是红线）。不在 Diff 页签时它照旧内嵌 diff，行为一字未动。 -->
    <PendingDossier
      dock={isMobile}
      ondockheight={handleDockHeight}
      reason={pendingReason}
      cursors={detail.cursors}
      actions={detail.allowedActions}
      busy={taskDetail.busyKey !== null}
      onaction={handleAction}
      ongotoconversation={gotoconversation}
      onopenfiles={() => (tab = 'files')}
      diff={taskDetail.diff}
      rawDiff={taskDetail.diffRaw}
      diffStale={taskDetail.diffStale}
      diffError={taskDetail.diffError}
      diffLoading={taskDetail.diffError === null && taskDetail.diff === null && pendingType === 'merge_approval'}
      diffInPane={tab === 'diff'}
      onreloaddiff={() => void taskDetail.loadDiff(pendingType === 'human_review' ? 'review-diff.diff' : 'merge-proposal.diff')}
      reviewReport={taskDetail.getFile('review-report.md')?.content ?? null}
      unitTestReport={taskDetail.getFile('test-report.md')?.content ?? null}
      onsubmitreview={(approved, comments) => void taskDetail.submitReview(approved, comments).catch(() => undefined)}
    />
  {/if}
</main>

<SplitDialog
  open={splitOpen}
  submitting={taskDetail.busyKey !== null}
  error={dialogError}
  onclose={() => (splitOpen = false)}
  onsubmit={submitSplit}
/>
<ModelOverrideDialog
  open={modelOpen}
  {providers}
  submitting={taskDetail.busyKey !== null}
  error={dialogError}
  onclose={() => (modelOpen = false)}
  onsubmit={submitModel}
/>

<style>
  .detail {
    max-width: var(--detail-max);
    margin: 0 auto;
    padding: 18px 20px 40px;
  }
  .detail.split {
    max-width: 1240px;
    display: grid;
    grid-template-columns: minmax(0, 1fr) 320px;
    gap: 18px;
    align-items: start;
  }
  /* 左栏是**一个**网格项（不是 `display: contents` 把每个孩子各塞一行）。
     档案盒跨多行时，网格会把它的高度摊到它跨的那几行上——左栏于是被当成「行内居中」，
     标题与轨道 hero 之间空出约 300px 死区（实测行高 165/213/328 而内容只有 19/72/168），
     在 1100px 高的视口上把时间线整个顶到折叠线以下。收成一项后多余高度落在行**下边**，
     左栏照旧自上而下排。 */
  .detail.split .main {
    grid-column: 1;
    min-width: 0;
  }
  .crumb {
    display: inline-block;
    color: var(--text-3);
    font-size: 12px;
    margin-bottom: 10px;
  }
  .crumb:hover {
    color: var(--text-hi);
    text-decoration: none;
  }
  .d-head {
    display: flex;
    align-items: baseline;
    gap: 14px;
    flex-wrap: wrap;
    margin-bottom: 6px;
  }
  /* 标题 24px（字阶只取 12 的整数倍），像素字体无字重轴 */
  .d-title {
    font-size: 24px;
    color: var(--text-hi);
    line-height: 1.2;
  }
  .dmeta {
    display: flex;
    align-items: baseline;
    gap: 14px;
    font-size: 12px;
    color: var(--text-3);
    flex-wrap: wrap;
  }
  /* 大数字（时长 / token）24px 起步，配 12px 灰注（§2.2） */
  .dmeta .big {
    font-size: 24px;
    line-height: 1.2;
    color: var(--text-hi);
  }
  .dmeta .big .unit {
    font-size: 12px;
    color: var(--text-3);
    margin-left: 2px;
  }
  .dmeta .sep {
    color: var(--text-4);
    margin: 0 3px;
  }
  .dmeta .status.run {
    color: var(--text-hi);
  }
  .dmeta .status.pending {
    color: var(--pending);
  }
  .bypass {
    display: flex;
    gap: 8px;
  }
  .entries {
    display: flex;
    flex-wrap: wrap;
    gap: 8px 16px;
    margin-top: 8px;
  }
  /* 任务级入口 = 空态那套「可选入口」的同一副面孔（--text-hi + 2px 底缘，票 13 §三.3）：
     同一个语义在全站长一个样，用户不必学第二遍「哪里能点」。 */
  .entry {
    font-size: 12px;
    color: var(--text-hi);
    border-bottom: 2px solid var(--pane);
    padding-bottom: 2px;
  }
  .entry:hover {
    border-bottom-color: var(--text-hi);
    text-decoration: none;
  }
  /* 托管开关（决策 210① / 票 14）：与「任务级入口」同属任务级的那些事，故挨着它摆。
     开着的形态吃全站「选中」的那套语言（`.chip.on`：wash 底 + text-hi 描边 + ▶）。 */
  .steward {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 12px;
    font-size: 12px;
    color: var(--text-3);
  }
  .steward .btn.on {
    background: var(--wash);
    border-color: var(--text-hi);
    color: var(--text-hi);
  }
  .steward .btn.on::before {
    content: '';
    display: inline-block;
    width: 10px;
    height: 9px;
    background: var(--go);
    clip-path: polygon(0 0, 100% 50%, 0 100%);
    margin-right: 5px;
    vertical-align: -1px;
  }
  /* 按下失败的原因就地说（不弹窗）：终态任务与未接线那两句是端点给的、原样透传，
     故它用的是失败红档而不是琥珀——琥珀全站只留给急停（决策 203）。 */
  .steward .s-note.bad {
    color: var(--stop);
  }
  .hero-rail {
    margin: 18px 0 6px;
  }
  /* 图例：灯即状态（与 hero 信号灯同尺寸/同色，不用字符记号）。
     档位：读不到就分不清哪盏灯是什么 → 「次级必读」（决策 195 / 票 15）。 */
  .legend {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 4px 6px;
    font-size: 12px;
    color: var(--text-3);
    margin: 4px 0 14px;
  }
  .legend .lbl {
    margin-right: 4px;
  }
  .legend .sw {
    display: inline-block;
    width: 12px;
    height: 12px;
    border: 2px solid var(--pane);
    background: var(--bg);
    vertical-align: -1px;
  }
  .legend .sw + .lg {
    margin-right: 6px;
  }
  .legend .sw.d {
    background: var(--done);
    border-color: var(--done);
  }
  .legend .sw.c {
    background: var(--go);
    border-color: var(--go);
  }
  .legend .sw.w {
    background: var(--pending);
    border-color: var(--pending);
  }
  .legend .sw.x {
    background: var(--stop);
    border-color: var(--stop);
  }
  .legend .dash {
    color: var(--text-4);
    margin-left: 6px;
  }
  .terminal {
    padding: 8px 12px;
    border: 2px solid var(--pane);
    font-size: 12px;
    margin-bottom: 12px;
    color: var(--text-2);
  }
  .terminal.failed {
    border-color: var(--stop);
    color: var(--stop);
  }
  .terminal.done {
    border-color: var(--done);
  }
  /* 页签：工位标签盒基元在 app.css（票 03）；此处只补详情页的排布 */
  .tabs {
    margin: 10px 0 16px;
  }
  .hint {
    color: var(--text-3);
    font-size: 12px;
  }
  /* 「重新加载」挨着空态摆（票 01）：失败态的唯一出路，不能藏在别处。 */
  .reload {
    margin-top: 10px;
  }
  /* 横幅（断线 / 加载失败 / 动作提交失败，决策 159）：像素框，必须可见 */
  .banner {
    padding: 8px 12px;
    border: 2px solid var(--stop);
    background: var(--panel);
    color: var(--stop);
    font-size: 12px;
    margin-bottom: 10px;
  }

  /* ── 移动版（<480px）：站点带 · 标题行 + 纵向脊线 + 分段页签 + 动作坞（§5 移动款） ── */
  @media (max-width: 479px) {
    .detail {
      padding: 0 12px calc(30px + var(--safeb));
    }
    /* pending 时底部动作坞常驻，内容留出坞高（原型 #v-approve .detail padding-bottom） */
    /* 坞钉在底部堆叠上沿（票 05；窄档那一层现在只有页签栏，决策 300 摘掉了状态条），
       故内容要同时让出坞与底栏两份高度；`--dock-h` 与 `--sbar-h` 都已含各自的安全区那一份。 */
    .detail.docked {
      padding-bottom: calc(var(--dock-h) + var(--sbar-h) + 14px);
    }
    .detail.split {
      display: block;
      max-width: var(--detail-max);
    }

    /* 标题行：标题 + 状态标记（原先还有一颗「← 看板」返回链，决策 240 摘除，`.back` 随之删） */
    .bar-row {
      display: flex;
      align-items: center;
      gap: 10px;
      min-height: 42px;
      padding: 0 12px;
      margin: 0 -12px;
      border-bottom: 2px solid var(--hairline);
    }
    .d-title.bt {
      flex: 1;
      min-width: 0;
      font-size: 12px;
      color: var(--text-hi);
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    .bar-row .mark {
      flex: none;
      color: var(--text-3);
      font-size: 12px;
      letter-spacing: 0.06em;
      white-space: nowrap;
    }
    .bar-row .mark.pending {
      color: var(--pending);
    }
    .bar-row .mark.run {
      color: var(--text-hi);
    }

    .dmeta {
      display: flex;
      flex-wrap: wrap;
      gap: 2px 13px;
      margin: 0 -12px;
      padding: 0 12px 8px;
      font-size: 12px;
      color: var(--text-3);
      font-variant-numeric: tabular-nums;
    }
    .dmeta > span {
      white-space: nowrap;
    }
    .dmeta .big {
      font-size: 12px;
    }
    .dmeta .status.run {
      color: var(--text-hi);
    }
    .dmeta .status.pending {
      color: var(--pending);
    }

    .bypass {
      margin-bottom: 10px;
    }
    .hero-rail {
      margin: 12px 0 14px;
    }

    /* 页签：基元复用 app.css 的 .tabs/.tab，移动版只加横滚与触控高度（完整转写见票 11） */
    .tabs {
      overflow-x: auto;
      margin: 0 0 14px;
      -webkit-overflow-scrolling: touch;
    }
    .tab {
      flex: none;
      display: inline-flex;
      align-items: center;
      min-height: 42px;
      padding: 0 12px;
      white-space: nowrap;
    }
  }
</style>
