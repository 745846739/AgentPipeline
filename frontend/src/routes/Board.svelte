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

  /** 未过滤的整列任务：移动版脊线/段头状态与 hide-cards 判定用（桌面不读）。 */
  const allByColumn = $derived.by(() => {
    const map = new Map<string, typeof board.tasks>();
    for (const col of BOARD_COLUMNS) map.set(col.key, []);
    for (const task of board.tasks) {
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

    <div class="boardpad">
      {#if board.error}
        <div class="banner error">加载失败：{board.error}</div>
      {/if}
      {#if board.actionError}
        <!-- 动作提交失败必须可见（主流程票 03）：吞掉它 = 用户点「重试」毫无反应的死面板 -->
        <div class="banner error">动作提交失败：{board.actionError}</div>
      {/if}
      {#if board.connectionState === 'error'}
        <div class="banner">实时流已断开，正在重连…（看板仍每 10s 对齐一次）</div>
      {/if}

      {#if board.projects.length === 0 && !board.loading}
        <div class="board-empty">还没有项目。到「设置 · 项目」添加一个本地 git 仓库，然后新建任务。</div>
      {:else if board.visibleTasks.length === 0 && !board.loading}
        <div class="board-empty">新建第一个任务，流水线从 init 开始拍发。</div>
      {/if}

      <main class="panes">
        {#each BOARD_COLUMNS as column (column.key)}
          <BoardColumn
            {column}
            tasks={tasksByColumn.get(column.key) ?? []}
            allTasks={allByColumn.get(column.key) ?? []}
            actionsFor={(id) => board.pendingActions[id] ?? []}
            cursorsFor={(id) => board.pendingCursors[id] ?? []}
            actionBusy={board.actionBusy}
            onopen={openTask}
            onaction={onAction}
          />
        {/each}
      </main>
    </div>
  </div>
</div>

<style>
  /* 工位阵列：列间共享 2px 框线，无间隙（§2.3 描边只有 2px 一档）
     总宽 = 8×264（列）+ 7×2（列间框线）+ 2×2（阵列边框）= 2130，与冻结原型一致 */
  .hscroll {
    overflow-x: auto;
    background: var(--bg);
  }
  .hinner {
    width: 2162px;
  }
  .boardpad {
    padding: 0 16px 8px;
  }
  .panes {
    display: flex;
    border: 2px solid var(--pane);
    background: var(--bg);
    width: 2130px;
    align-items: stretch;
  }
  .banner {
    background: var(--panel);
    border: 2px solid var(--pane);
    padding: 8px 12px;
    font-size: 12px;
    color: var(--text-2);
    margin-bottom: 8px;
  }
  .banner.error {
    border-color: var(--stop);
    color: var(--stop);
  }
  .board-empty {
    color: var(--text-4);
    font-size: 12px;
    padding: 6px 4px 12px;
  }

  /* ── 移动版：窗格阵列 → 纵向电报纸带（theme-3 §8） ── */
  @media (max-width: 479px) {
    .hscroll {
      overflow-x: visible;
    }
    .hinner {
      width: auto;
    }
    .boardpad {
      padding: 0 12px calc(52px + var(--safeb));
    }
    .panes {
      display: block;
      width: auto;
      border: 0;
    }
  }
</style>
