<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { archiveTask, listProviders, retryTask } from '../api/client';
  import type { AllowedAction, Provider } from '../api/types';
  import PipelineRail from '../components/pipeline/PipelineRail.svelte';
  import CommandLog from '../components/task/CommandLog.svelte';
  import ConversationViewer from '../components/task/ConversationViewer.svelte';
  import DiffReviewPanel from '../components/task/DiffReviewPanel.svelte';
  import FileViewer from '../components/task/FileViewer.svelte';
  import ModelOverrideDialog from '../components/task/ModelOverrideDialog.svelte';
  import PendingDossier from '../components/task/PendingDossier.svelte';
  import SplitDialog from '../components/task/SplitDialog.svelte';
  import TimelineView from '../components/task/TimelineView.svelte';
  import DiffView from '../components/render/DiffView.svelte';
  import { buildHeroStations, formatDuration, formatTokens, pendingLabel, statusCode } from '../lib/pipeline';
  import { taskDetail } from '../stores/taskDetail.svelte';

  interface Props {
    id: string;
  }
  let { id }: Props = $props();

  type Tab = 'timeline' | 'conversation' | 'commands' | 'files' | 'diff';
  let tab = $state<Tab>('timeline');
  let selectedRunId = $state<number | null>(null);
  let splitOpen = $state(false);
  let modelOpen = $state(false);
  let providers = $state<Provider[]>([]);
  let dialogError = $state<string | null>(null);
  let bypassBusy = $state<string | null>(null);
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

  function gotoconversation(stage: string, node: string) {
    const match = detail.conversations.find((c) => c.stage === stage && c.node === node);
    tab = 'conversation';
    if (match) {
      selectedRunId = match.run_id;
      void taskDetail.loadConversation(match.run_id);
    }
  }

  function selectRun(runId: number) {
    selectedRunId = runId;
    void taskDetail.loadConversation(runId);
  }

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
    try {
      if (kind === 'retry') await retryTask(id);
      else await archiveTask(id);
      await taskDetail.load(id, true);
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
  {/if}
{/snippet}

<div
  class="detail"
  class:split={isPending}
  class:docked={isMobile && isPending}
  style="--dock-h: {dockH > 0 ? `${dockH}px` : '122px'}"
>
  <div class="main">
    {#if !isMobile}<a class="crumb" href="#/">← 看板</a>{/if}

    {#if taskDetail.error}
      <div class="banner error">{taskDetail.error}</div>
    {/if}

    {#if taskDetail.actionError}
      <!-- 动作提交失败必须可见（主流程票 03）：吞掉它 = 用户点「重试」毫无反应的死面板 -->
      <div class="banner error">动作提交失败：{taskDetail.actionError}</div>
    {/if}

    {#if task}
      {#if isMobile}
        <div class="bar-row">
          <a class="back" href="#/">← 看板</a>
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

      <div class="hero-rail">
        <PipelineRail variant={isMobile ? 'vrail' : 'hero'} stations={heroStations} />
      </div>
      {#if !isMobile}
        <!-- 图例：灯即状态（不使用 ✓ ● ○ 字符；与 hero 同一套信号灯图元） -->
        <div class="legend">
          <span class="lbl">灯</span>
          <i class="sw d"></i><span class="lg">已完成</span>
          <i class="sw c"></i><span class="lg">执行中</span>
          <i class="sw idle"></i><span class="lg">未开始</span>
          <i class="sw w"></i><span class="lg">急停</span>
          <i class="sw x"></i><span class="lg">失败</span>
          <span class="lg dash">↩ 已打回</span>
        </div>
      {/if}

      {#if detail.terminal}
        <div class="terminal {detail.terminal}">任务已{detail.terminal === 'done' ? '完成' : detail.terminal === 'failed' ? '失败' : '取消'}。</div>
      {/if}

      <nav class="tabs" class:no-scrollbar={isMobile} aria-label="任务详情页签">
        <button type="button" class="tab" class:on={tab === 'timeline'} onclick={() => (tab = 'timeline')}>时间线</button>
        <button type="button" class="tab" class:on={tab === 'conversation'} onclick={() => (tab = 'conversation')}>
          会话
        </button>
        <button type="button" class="tab" class:on={tab === 'commands'} onclick={() => (tab = 'commands')}>
          命令与输出<span class="c">{detail.commands.length}</span>
        </button>
        <button type="button" class="tab" class:on={tab === 'files'} onclick={() => (tab = 'files')}>产出文件</button>
        {#if showDiffTab}
          <button type="button" class="tab" class:on={tab === 'diff'} onclick={() => (tab = 'diff')}>Diff</button>
        {:else}
          <button type="button" class="tab dis" disabled>Diff ─ merge 后生成</button>
        {/if}
      </nav>

      <div class="pane">
        {#if tab === 'timeline'}
          <TimelineView
            transitions={detail.transitions}
            currentBranch={focalCursor?.branch}
            currentStage={focalCursor?.stage}
            currentNode={focalCursor?.node}
          />
        {:else if tab === 'conversation'}
          <ConversationViewer
            conversations={detail.conversations}
            {selectedRunId}
            onselect={selectRun}
            getConversation={(runId) => taskDetail.conversationsFull[runId]}
            loading={taskDetail.conversationsLoading}
            liveDeltas={detail.liveDeltas}
            liveTools={detail.liveTools}
            streamTokens={detail.streamTokens}
          />
        {:else if tab === 'commands'}
          <CommandLog
            commands={detail.commands}
            outputFor={(c) => taskDetail.outputFor(c)}
            streamedFor={(c) => detail.commandOutput[c.id] ?? null}
            onload={(cmdId) => taskDetail.loadCommandOutput(cmdId)}
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
      <div class="hint">任务不存在：{id}</div>
    {/if}
  </div>

  {#if task && isPending && pendingReason}
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
      onreloaddiff={() => void taskDetail.loadDiff(pendingType === 'human_review' ? 'review-diff.diff' : 'merge-proposal.diff')}
      reviewReport={taskDetail.getFile('review-report.md')?.content ?? null}
      unitTestReport={taskDetail.getFile('test-report.md')?.content ?? null}
      onsubmitreview={(approved, comments) => void taskDetail.submitReview(approved, comments).catch(() => undefined)}
    />
  {/if}
</div>

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
  .hero-rail {
    margin: 18px 0 6px;
  }
  /* 图例：灯即状态（与 hero 信号灯同尺寸/同色，不用字符记号） */
  .legend {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 4px 6px;
    font-size: 12px;
    color: var(--text-4);
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
    .detail.docked {
      padding-bottom: calc(var(--dock-h) + 14px + var(--safeb));
    }
    .detail.split {
      display: block;
      max-width: var(--detail-max);
    }

    /* 标题行：返回 + 标题 + 状态标记 */
    .bar-row {
      display: flex;
      align-items: center;
      gap: 10px;
      min-height: 42px;
      padding: 0 12px;
      margin: 0 -12px;
      border-bottom: 2px solid var(--hairline);
    }
    .back {
      flex: none;
      display: inline-flex;
      align-items: center;
      min-height: 42px;
      color: var(--text-3);
      font-size: 12px;
    }
    .back:hover {
      color: var(--text-hi);
      text-decoration: none;
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
