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
    {#if task.stalled}
      <StalledBadge hours={hours} />
    {:else}
      <span class="dur">{isTerminal ? '—' : formatDuration(durationMs)}</span>
    {/if}
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
    padding: 11px 12px 12px;
    border-bottom: 1px solid var(--hairline);
    cursor: pointer;
    transition: background 0.12s;
  }
  .card:last-child {
    border-bottom: 0;
  }
  .card:hover {
    background: var(--hover-bg);
  }
  .card.warn {
    box-shadow: inset 2px 0 0 var(--pending);
  }
  .card.stopped {
    box-shadow: inset 2px 0 0 var(--stop);
  }
  .card.stalled {
    background: var(--panel);
  }
  .card.stalled:hover {
    background: var(--wash);
  }
  .card-link {
    position: absolute;
    inset: 0;
    z-index: 1;
  }
  .card-link:hover {
    text-decoration: none;
  }
  .card > :not(.card-link) {
    position: relative;
    z-index: 0;
  }
  .card-top {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 8px;
  }
  .card-title {
    min-width: 0;
    font-weight: 500;
    font-size: 12.5px;
    color: var(--text-hi);
  }
  .card.warn .card-title::before {
    content: '! ';
    color: var(--pending);
    font-weight: 600;
  }
  .card.mute .card-title {
    color: var(--text-2);
    font-weight: 400;
  }
  .dur {
    flex: none;
    font-size: 11px;
    color: var(--text-3);
    font-variant-numeric: tabular-nums;
  }
  .pillrow {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin: 8px 0;
  }
  .reason {
    margin: 8px 0;
    font-size: 11.5px;
    color: var(--text-2);
    border-left: 2px solid var(--hairline);
    padding-left: 10px;
  }
  .reason.warn {
    border-left-color: var(--pending);
  }
  .rlabel {
    color: var(--pending);
    font-weight: 600;
    font-size: 11px;
  }
  .rlabel::before {
    content: '> ';
  }
  .ctxlink {
    margin: 2px 0 4px;
    font-size: 11px;
    color: var(--text-3);
  }
  /* 必须压过整卡导航链接（`.card-link`，z-index 1）。上面 `.card > :not(.card-link)`
     把卡片子元素统一归零，那条规则特异性 (0,2,0) 高于 `.actions` (0,1,0)，所以这里
     也用 `.card > .actions` 取同等特异性、靠源码顺序取胜。主流程票 09 实测：不修则
     卡片动作按钮被链接覆盖，点击只跳详情（Playwright 报 element intercepts pointer events），
     即「看板卡上的动作按钮点不动」这个用户可见缺陷。 */
  .card > .actions {
    margin-top: 8px;
    position: relative;
    z-index: 2;
  }
  .tagline {
    margin-top: 6px;
    font-size: 11px;
    color: var(--text-3);
  }
  .meta {
    display: flex;
    gap: 10px;
    flex-wrap: wrap;
    margin-top: 7px;
    font-size: 10.5px;
    color: var(--text-3);
    font-variant-numeric: tabular-nums;
  }
  .meta b {
    color: var(--text-hi);
    font-weight: 500;
  }

  /* ── 移动版：电文行组（theme-3 §8，原型 .tg） ── */
  @media (max-width: 479px) {
    .card {
      padding: 10px 0 11px 12px;
      border-bottom: 1px solid var(--hairline);
    }
    /* pending 由脊线/段头承担信号，卡上不再加琥珀左缘 */
    .card.warn {
      box-shadow: none;
    }
    .card.stopped {
      box-shadow: inset 2px 0 0 var(--stop);
    }
    .card-title {
      font-size: 14px;
      line-height: 1.45;
    }
    .card.warn .card-title::before {
      content: '! ';
    }
    .dur {
      font-size: 12px;
    }
    :global(.card .card-top .stalltag) {
      font-size: 11px;
      letter-spacing: 0.05em;
    }
    .pillrow {
      margin: 6px 0 7px;
    }
    /* 分支/状态行按 .tg-st 字号（13px，次文本） */
    .pillrow :global(.pill) {
      font-size: 13px;
      color: var(--text-2);
      border: 0;
      padding: 0;
      gap: 6px;
      align-items: baseline;
    }
    .pillrow :global(.pill .bl) {
      font-size: 11px;
      font-weight: 600;
    }
    .pillrow :global(.pill .mono) {
      color: var(--text-2);
    }
    .reason {
      margin: 9px 0 8px;
      font-size: 13px;
      line-height: 1.6;
      padding-left: 10px;
    }
    .rlabel {
      font-size: 12.5px;
    }
    .ctxlink {
      font-size: 13px;
      color: var(--text-2);
    }
    .tagline {
      margin-top: 6px;
      font-size: 12.5px;
    }
    .actions {
      margin-top: 10px;
    }
    .meta {
      font-size: 12px;
      gap: 10px;
      margin-top: 8px;
    }
  }
</style>
