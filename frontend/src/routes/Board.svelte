<script lang="ts">
  import type { AllowedAction } from '../api/types';
  import BoardColumn from '../components/board/BoardColumn.svelte';
  import PipelineRail from '../components/pipeline/PipelineRail.svelte';
  import { router } from '../router.svelte';
  import { board } from '../stores/board.svelte';
  import { BOARD_COLUMNS, buildSpineStations, columnForTask } from '../lib/pipeline';

  const tasksByColumn = $derived.by(() => {
    const map = new Map<string, typeof board.visibleTasks>();
    for (const col of BOARD_COLUMNS) map.set(col.key, []);
    for (const task of board.visibleTasks) {
      const key = columnForTask(task);
      map.get(key)?.push(task);
    }
    return map;
  });

  const stations = $derived(buildSpineStations(board.visibleTasks));

  function openTask(id: string) {
    router.navigate(`/task/${id}`);
  }

  function onAction(
    taskId: string,
    action: AllowedAction,
    opts: { cursorId?: string; input?: string },
  ) {
    void board.handleTaskAction(taskId, action, opts).catch(() => undefined);
  }
</script>

<div class="hscroll">
  <div class="hinner">
    <PipelineRail variant="spine" {stations} />
  </div>
</div>

<main class="board">
  {#if board.error}
    <div class="banner error">加载失败：{board.error}</div>
  {/if}
  {#if board.connectionState === 'error'}
    <div class="banner">实时流已断开，正在重连…（看板仍每 10s 对齐一次）</div>
  {/if}

  {#if board.projects.length === 0 && !board.loading}
    <div class="board-empty">还没有项目。到「设置 · 项目」添加一个本地 git 仓库，然后新建任务。</div>
  {:else if board.visibleTasks.length === 0 && !board.loading}
    <div class="board-empty">新建第一个任务，流水线会从 init 开始走。</div>
  {/if}

  {#each BOARD_COLUMNS as column (column.key)}
    <BoardColumn
      {column}
      tasks={tasksByColumn.get(column.key) ?? []}
      actionsFor={(id) => board.pendingActions[id] ?? []}
      cursorsFor={(id) => board.pendingCursors[id] ?? []}
      actionBusy={board.actionBusy}
      onopen={openTask}
      onaction={onAction}
    />
  {/each}
</main>

<style>
  .hscroll {
    overflow-x: auto;
    background: var(--ink-900);
  }
  .hinner {
    width: max-content;
  }
  .board {
    display: flex;
    gap: var(--rail-col-gap);
    padding: 16px 20px 40px;
    align-items: flex-start;
  }
  .banner {
    position: sticky;
    left: 0;
    flex: none;
    width: calc(100vw - 40px);
    background: var(--ink-800);
    border: 1px solid var(--line);
    border-radius: var(--r-panel);
    padding: 8px 12px;
    font-size: 12px;
    color: var(--text-2);
  }
  .banner.error {
    border-color: var(--signal-stop);
    color: var(--signal-stop);
  }
  .board-empty {
    flex: none;
    width: calc(100vw - 40px);
    color: var(--text-3);
    font-size: 12.5px;
    padding: 6px 4px;
  }
</style>
