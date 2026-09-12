<script lang="ts">
  import type { AllowedAction, BranchCursor, TaskListItem } from '../../api/types';
  import type { BoardColumnDef } from '../../lib/pipeline';
  import { EMPTY_HINTS } from '../../lib/pipeline';
  import TaskCard from './TaskCard.svelte';

  interface Props {
    column: BoardColumnDef;
    tasks: TaskListItem[];
    actionsFor?: (taskId: string) => AllowedAction[];
    cursorsFor?: (taskId: string) => BranchCursor[];
    actionBusy?: string | null;
    onopen?: (id: string) => void;
    onaction?: (taskId: string, action: AllowedAction, opts: { cursorId?: string; input?: string }) => void;
  }

  let { column, tasks, actionsFor, cursorsFor, actionBusy = null, onopen, onaction }: Props =
    $props();
</script>

<section class="col">
  <div class="col-head">
    <span class="col-name">{column.label}</span>
    {#if tasks.length > 0}<span class="col-n">{tasks.length}</span>{/if}
  </div>

  {#if tasks.length === 0}
    <div class="col-empty">{EMPTY_HINTS[column.key]}</div>
  {:else}
    {#each tasks as task (task.id)}
      <TaskCard
        {task}
        actions={actionsFor?.(task.id) ?? []}
        cursors={cursorsFor?.(task.id) ?? []}
        {actionBusy}
        {onopen}
        onaction={(action, opts) => onaction?.(task.id, action, opts)}
      />
    {/each}
  {/if}
</section>

<style>
  .col {
    width: var(--rail-col-width);
    flex: none;
    border-right: 1px solid var(--pane);
    display: flex;
    flex-direction: column;
  }
  .col:last-child {
    border-right: 0;
  }
  .col-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 8px;
    padding: 9px 12px;
    border-bottom: 1px solid var(--pane);
    background: var(--head-band);
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--text-3);
  }
  .col-n {
    color: var(--text-4);
    font-weight: 400;
    font-variant-numeric: tabular-nums;
  }
  .col-empty {
    padding: 14px 12px;
    color: var(--text-4);
    font-size: 11.5px;
  }
</style>
