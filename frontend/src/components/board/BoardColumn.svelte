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
    <span class="col-name cond">{column.label}</span>
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
  }
  .col-head {
    display: flex;
    align-items: baseline;
    gap: 8px;
    padding: 2px 4px 10px;
  }
  .col-name {
    font-size: 12.5px;
    color: var(--text-2);
  }
  .col-n {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--text-3);
  }
  .col-empty {
    border: 1px dashed var(--line);
    border-radius: var(--r-panel);
    padding: 18px 14px;
    color: var(--text-3);
    font-size: 12px;
  }
</style>
