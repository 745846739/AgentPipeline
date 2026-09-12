<script lang="ts">
  import type { AllowedAction, BranchCursor, TaskListItem } from '../../api/types';
  import {
    branchShort,
    formatDuration,
    formatTokens,
    miniRailState,
    pendingLabel,
    stalledHours,
    taskDuration,
  } from '../../lib/pipeline';
  import PipelineRail from '../pipeline/PipelineRail.svelte';
  import BranchPill from '../pipeline/BranchPill.svelte';
  import PendingActions from './PendingActions.svelte';
  import StalledBadge from './StalledBadge.svelte';

  interface Props {
    task: TaskListItem;
    actions?: AllowedAction[];
    cursors?: BranchCursor[];
    actionBusy?: string | null;
    onopen?: (id: string) => void;
    onaction?: (action: AllowedAction, opts: { cursorId?: string; input?: string }) => void;
  }

  let { task, actions = [], cursors = [], actionBusy = null, onopen, onaction }: Props = $props();

  const dots = $derived(miniRailState(task));
  const visibleCursors = $derived(
    (cursors.length ? cursors : task.branches).filter((c) => c.status !== 'archived'),
  );
  const isPending = $derived(task.status === 'pending');
  const isTerminal = $derived(
    task.status === 'done' || task.status === 'failed' || task.status === 'cancelled',
  );
  const durationMs = $derived(taskDuration(task));
  const hours = $derived(stalledHours(task));
  const reason = $derived(task.pending_reason ?? visibleCursors.find((c) => c.pending_reason)?.pending_reason ?? null);

  function isBusy(action: AllowedAction, cursorId?: string): boolean {
    return actionBusy === `${task.id}:${action.action}` || actionBusy === `${action.action}:${cursorId ?? ''}`;
  }
</script>

<article
  class="card {isPending || task.stalled ? 'warn' : ''} {task.status === 'failed' || task.status === 'cancelled' ? 'stopped' : ''} {task.stalled ? 'stalled' : ''} {isTerminal && task.status === 'done' ? 'mute' : ''}"
>
  <a
    class="card-link"
    href={`#/task/${task.id}`}
    aria-label={`打开任务：${task.title}`}
    onclick={(e) => {
      if (onopen) {
        e.preventDefault();
        onopen(task.id);
      }
    }}><span class="visually-hidden">打开任务 {task.title}</span></a
  >
  <div class="card-top">
    <span class="card-title">{task.title}</span>
    {#if task.stalled}<StalledBadge hours={hours} />{/if}
    <span class="dur">{isTerminal ? '—' : formatDuration(durationMs)}</span>
  </div>

  <PipelineRail variant="mini" {dots} ariaLabel="任务迷你轨道" />

  {#if visibleCursors.length > 0}
    <div class="pillrow">
      {#each visibleCursors as cursor (cursor.cursor_id)}
        <BranchPill {cursor} />
      {/each}
    </div>
  {/if}

  {#if isPending && reason}
    <div class="reason warn">
      <span class="rlabel cond">{pendingLabel(reason)}</span><br />
      {reason.message}
    </div>
  {/if}

  {#if isPending && actions.length > 0}
    <!-- 阻止点击冒泡到整卡导航 -->
    <div class="actions" role="presentation" onclick={(e) => e.stopPropagation()} onkeydown={(e) => e.stopPropagation()}>
      <PendingActions
        {actions}
        cursors={visibleCursors}
        pendingType={reason?.type}
        onaction={onaction}
        isBusy={isBusy}
      />
    </div>
  {:else if isPending}
    <div class="ctxlink"><span>前往详情处理 ▸</span></div>
  {/if}

  {#if task.status === 'waiting'}
    <div class="tagline">等待依赖完成</div>
  {:else if task.status === 'queued'}
    <div class="tagline">排队等待并发名额</div>
  {/if}

  <div class="meta">
    <span><b>{formatTokens(task.total_tokens)}</b> tok</span>
    <span><b>{task.total_calls}</b> 次调用</span>
    {#if task.branch_name}<span>{task.branch_name}</span>{/if}
    {#if visibleCursors.some((c) => c.branch !== 'main')}
      <span>{visibleCursors.map((c) => branchShort(c.branch)).join('/')}</span>
    {/if}
  </div>
</article>

<style>
  .card {
    position: relative;
    background: var(--ink-800);
    border: 1px solid var(--line);
    border-radius: var(--r-panel);
    padding: 12px 12px 10px;
    margin-bottom: 10px;
    cursor: pointer;
    transition: border-color 0.15s;
  }
  .card:hover {
    border-color: var(--text-3);
  }
  .card:focus-visible {
    border-color: var(--text-3);
  }
  .card-link {
    position: absolute;
    inset: 0;
    z-index: 1;
    border-radius: var(--r-panel);
  }
  .card-link:hover {
    text-decoration: none;
  }
  .card > :not(.card-link) {
    position: relative;
    z-index: 0;
  }
  .card::before {
    content: '';
    position: absolute;
    left: -1px;
    top: 10px;
    bottom: 10px;
    width: 2px;
    border-radius: 1px;
    background: transparent;
  }
  .card.warn::before {
    background: var(--signal-caution);
  }
  .card.stopped::before {
    background: var(--signal-stop);
  }
  .card.stalled {
    border-color: var(--signal-caution);
  }
  .card-top {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 8px;
  }
  .card-title {
    font-weight: 500;
    font-size: 13.5px;
    letter-spacing: 0.01em;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .card.mute .card-title {
    color: var(--text-2);
  }
  .dur {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--text-3);
    flex: none;
  }
  .pillrow {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-bottom: 8px;
  }
  .reason {
    margin: 8px 0;
    font-size: 12px;
    color: var(--text-2);
    border-left: 2px solid var(--line);
    padding-left: 8px;
  }
  .reason.warn {
    border-color: var(--signal-caution);
  }
  .rlabel {
    color: var(--signal-caution);
    font-size: 11px;
  }
  .ctxlink {
    font-size: 11.5px;
    margin: 4px 0;
  }
  .actions {
    margin-top: 8px;
    position: relative;
    z-index: 2;
  }
  .tagline {
    display: flex;
    gap: 8px;
    align-items: center;
    margin-top: 6px;
    font-size: 11px;
    color: var(--text-3);
  }
  .meta {
    display: flex;
    gap: 10px;
    font-family: var(--font-mono);
    font-size: 10.5px;
    color: var(--text-3);
    margin-top: 8px;
    flex-wrap: wrap;
  }
  .meta b {
    color: var(--text-2);
    font-weight: 500;
  }
</style>
