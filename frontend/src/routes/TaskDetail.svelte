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
  import { buildHeroStations, formatDuration, formatTokens, pendingLabel } from '../lib/pipeline';
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
  /** 窄屏（<480px）：hero 轨道转纵向脊线（theme-3 §8）。 */
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

{#snippet statusMarker()}
  {#if isPending}
    ⏸ pending · {pendingLabel(pendingReason)}
  {:else if task?.status === 'running'}
    <span class="st run">[RUN]</span>
  {:else}
    {task?.status}
  {/if}
{/snippet}

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

    {#if task}
      {#if isMobile}
        <div class="bar-row">
          <a class="back" href="#/">← 看板</a>
          <h1 class="d-title bt">{task.title}</h1>
          <span class="mark" class:pending={isPending} class:run={!isPending && !isTerminal}>
            {@render statusMarker()}
          </span>
        </div>
        <div class="dmeta">
          <span class="status" class:pending={isPending} class:run={!isPending && !isTerminal}>
            {#if isPending}
              ⏸ pending · {pendingLabel(pendingReason)}
            {:else}
              {task.status}
              {#if focalCursor && !isTerminal}· {focalCursor.branch}.{focalCursor.node}{/if}
            {/if}
          </span>
          <span>⏱ {formatDuration(Math.max(0, Date.parse(task.updated_at) - Date.parse(task.created_at)))}</span>
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
                ⏸ pending · {pendingLabel(pendingReason)}
              {:else}
                {#if task.status === 'running'}<span class="st run">[RUN]</span>{/if}
                {task.status}
                {#if focalCursor && !isTerminal}· {focalCursor.branch}.{focalCursor.node}{/if}
              {/if}
            </span>
            <span>⏱ {formatDuration(Math.max(0, Date.parse(task.updated_at) - Date.parse(task.created_at)))}</span>
            <span>{formatTokens(task.total_tokens)} tok · {task.total_calls} 次调用</span>
            <span class="mono">{task.id}</span>
            {#if task.branch_name}<span>{task.branch_name}</span>{/if}
            <span>评审：{task.review_mode}</span>
            {#if task.model_override}<span class="mono">model: {task.model_override}</span>{/if}
          </div>
          {@render bypassActions()}
        </div>
      {/if}

      <div class="hero-rail">
        <PipelineRail variant={isMobile ? 'vrail' : 'hero'} stations={heroStations} />
      </div>
      <div class="legend">节点状态 ─ ✓ 已完成 · ● 执行中 · ○ 未开始 · ⏸ pending · ✗ 失败 · ↩ 已打回</div>

      {#if detail.terminal}
        <div class="terminal {detail.terminal}">任务已{detail.terminal === 'done' ? '完成' : detail.terminal === 'failed' ? '失败' : '取消'}。</div>
      {/if}

      <nav class="tabs" class:no-scrollbar={isMobile}>
        <button type="button" class="tab" class:on={tab === 'timeline'} onclick={() => (tab = 'timeline')}>[时间线]</button>
        <button type="button" class="tab" class:on={tab === 'conversation'} onclick={() => (tab = 'conversation')}>
          [会话]
        </button>
        <button type="button" class="tab" class:on={tab === 'commands'} onclick={() => (tab = 'commands')}>
          [命令与输出 {detail.commands.length}]
        </button>
        <button type="button" class="tab" class:on={tab === 'files'} onclick={() => (tab = 'files')}>[产出文件]</button>
        {#if showDiffTab}
          <button type="button" class="tab" class:on={tab === 'diff'} onclick={() => (tab = 'diff')}>[Diff]</button>
        {:else}
          <button type="button" class="tab dis" disabled>[Diff ─ merge 后生成]</button>
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
  .detail.split .main {
    display: contents;
  }
  .detail.split .main > * {
    grid-column: 1;
  }
  .crumb {
    display: inline-block;
    color: var(--text-3);
    font-size: 11.5px;
    margin-bottom: 10px;
  }
  .crumb:hover {
    color: var(--text-hi);
  }
  .d-head {
    display: flex;
    align-items: baseline;
    gap: 14px;
    flex-wrap: wrap;
    margin-bottom: 6px;
  }
  .d-title {
    font-size: 16px;
    font-weight: 600;
    color: var(--text-hi);
  }
  .dmeta {
    display: flex;
    gap: 14px;
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--text-3);
    flex-wrap: wrap;
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
  .legend {
    font-size: 10.5px;
    color: var(--text-4);
    margin: 4px 0 14px;
  }
  .terminal {
    padding: 8px 12px;
    border: 1px solid var(--pane);
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
  .tabs {
    display: flex;
    gap: 2px;
    border-bottom: 1px solid var(--pane);
    margin: 10px 0 16px;
  }
  .tab {
    padding: 5px 12px;
    color: var(--text-3);
    font-size: 11.5px;
    border-bottom: 1px solid transparent;
    margin-bottom: -1px;
  }
  .tab:hover:not(.dis) {
    color: var(--text-2);
  }
  .tab.on {
    color: var(--text-hi);
    background: var(--panel);
  }
  .tab.dis {
    color: var(--text-4);
    cursor: default;
  }
  .hint {
    color: var(--text-3);
    font-size: 12px;
  }
  .banner {
    padding: 8px 12px;
    border: 1px solid var(--stop);
    color: var(--stop);
    font-size: 12px;
    margin-bottom: 10px;
  }

  /* ── 移动版（<480px）：电报纸带 · 标题行 + 纵向脊线 + 分段页签 + 动作坞（theme-3 §8） ── */
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
    .detail.split .main {
      display: block;
    }
    .detail.split .main > * {
      grid-column: auto;
    }

    /* 标题行：返回 + 标题 + 状态标记 */
    .bar-row {
      display: flex;
      align-items: center;
      gap: 10px;
      min-height: 42px;
      padding: 0 12px;
      margin: 0 -12px;
      border-bottom: 1px solid var(--hairline);
    }
    .back {
      flex: none;
      display: inline-flex;
      align-items: center;
      min-height: 42px;
      color: var(--text-3);
      font-size: 13px;
    }
    .back:hover {
      color: var(--text-hi);
      text-decoration: none;
    }
    .d-title.bt {
      flex: 1;
      min-width: 0;
      font-size: 15px;
      color: var(--text-hi);
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    .bar-row .mark {
      flex: none;
      color: var(--text-3);
      font-weight: 600;
      font-size: 11px;
      letter-spacing: 0.06em;
      white-space: nowrap;
    }
    .bar-row .mark.pending {
      color: var(--pending);
    }
    .bar-row .mark.run {
      color: var(--text-hi);
    }
    .bar-row .mark .st.run {
      font-size: 11px;
    }

    .dmeta {
      display: flex;
      flex-wrap: wrap;
      gap: 2px 13px;
      margin: 0 -12px;
      padding: 0 12px 8px;
      font-size: 11.5px;
      color: var(--text-3);
      font-variant-numeric: tabular-nums;
    }
    .dmeta > span {
      white-space: nowrap;
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
    .legend {
      display: none;
    }

    /* 分段页签：横向滚动、可点 ≥42px、激活 2px 底边 */
    .tabs {
      display: flex;
      gap: 2px;
      overflow-x: auto;
      border-bottom: 1px solid var(--pane);
      margin: 0 0 14px;
      -webkit-overflow-scrolling: touch;
    }
    .tab {
      flex: none;
      display: inline-flex;
      align-items: center;
      min-height: 42px;
      padding: 0 12px;
      color: var(--text-3);
      font-size: 13px;
      white-space: nowrap;
      border-bottom: 2px solid transparent;
      margin-bottom: -1px;
    }
    .tab.on {
      color: var(--text-hi);
      background: none;
      border-bottom-color: var(--text-hi);
    }
    .tab.dis {
      color: var(--text-4);
    }
  }
</style>
