<script lang="ts">
  import { onMount } from 'svelte';
  import type { AllowedAction } from '../api/types';
  import BoardColumn from '../components/board/BoardColumn.svelte';
  import PipelineRail from '../components/pipeline/PipelineRail.svelte';
  import EmptyState from '../components/ui/EmptyState.svelte';
  import { router } from '../router.svelte';
  import { board } from '../stores/board.svelte';
  import { BOARD_COLUMNS, boardSegments, buildSpineStations, columnForTask } from '../lib/pipeline';
  import { GEOMETRY } from '../theme/contract';

  /**
   * 看板溢出（决策 196）：`merge` / `done` 先看见结论。
   *
   * - **≥ `GEOMETRY.boardPinBreakpoint`（1400px）**：两列钉在右侧不参与横滚（`position: sticky`），
   *   中间六列横滚；钉缝 = `merge` 列左缘的 2px `--pane` 竖线，脊线与主带在同一把刀下切开
   *   （`PipelineRail` 的 B 段是一块同样钉住、同样宽的脊线，段内站心 x 从 0 计）。
   * - **< 1400px**：不钉右，两段同属一条横滚内容；打开时的滚动位置落在 `merge`。
   * - 移动款（<480px）不受影响：那里本来就是纵向站点带。
   *
   * 阵列只有一份（八列同高、框线连续），钉住靠列的 sticky——不是两套阵列拼起来。
   */
  const segments = boardSegments();
  /** 钉右档里不算「还有列」的那些列（它们就压在右缘上）。 */
  const pinnedKeys = new Set(BOARD_COLUMNS.slice(segments.pinned.base).map((c) => c.key));

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

  /**
   * 钉右档的媒体查询：值与契约 `GEOMETRY.boardPinBreakpoint` 同源（这里是同一个数；
   * 样式表里那条 `@media (min-width: 1400px)` 是字面量——CSS 不认自定义属性）。
   * 两者相等由 `lib/pipeline.geometry.test.ts` 静态扫描守卫。
   */
  const pinQuery = `(min-width: ${GEOMETRY.boardPinBreakpoint}px)`;

  let pinned = $state(false);
  /** 右缘之外**完整不可见**的列数（决策 196：`n = 0` 时整条指示不渲染）。 */
  let hiddenColumns = $state(0);
  let scroller = $state<HTMLElement | null>(null);
  let pinnedLane = $state<HTMLElement | null>(null);
  /** 初始滚动只做一次：任务刷新不该抢人已经拖到的位置。 */
  let initialScrollDone = false;

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

  /** 列元素（`BoardColumn` 的 `<section id="s-<key>">`，移动款跳段也用这个 id）。 */
  function columnEl(key: string): HTMLElement | null {
    return document.getElementById(`s-${key}`);
  }

  /**
   * 量一次「右缘之外完整不可见的列数」。
   *
   * 边界：钉右档是钉缝（钉右段左缘 = `merge` 的左框线），窄档是横滚区右缘——同一口径，
   * 随档位挪边界。钉住的两列就压在右缘上（可见），不算「还有列」；窄档里没有列被钉住，
   * 八列一律参算——`done` 因此会被数进去，这正是「右缘还有列没显示」成立的原因。
   */
  function measure() {
    const view = scroller;
    if (!view) return;
    const boundary =
      pinned && pinnedLane
        ? pinnedLane.getBoundingClientRect().left
        : view.getBoundingClientRect().right;
    let hidden = 0;
    for (const column of BOARD_COLUMNS) {
      if (pinned && pinnedKeys.has(column.key)) continue;
      const el = columnEl(column.key);
      if (el && el.getBoundingClientRect().left >= boundary - 0.5) hidden += 1;
    }
    hiddenColumns = hidden;
  }

  /**
   * 窄档打开时的滚动位置（决策 196）：`scrollLeft = merge 右缘 − 横滚区宽`，钳制在 `[0, max]`。
   *
   * 让 `merge` 贴横滚区**左缘**在算术上不可达（最大可滚量 < merge 左缘），故「落在 merge」
   * 的可实现读法只能是贴**右缘**——`done` 仍在右缘之外，这正是「右缘还有列没显示」成立的原因。
   * 滚动是瞬时的（§1 原则 4：无缓动），直接写 `scrollLeft`。
   */
  function scrollToMerge() {
    const view = scroller;
    const merge = columnEl('merge');
    if (!view || !merge) return;
    const mergeRight =
      merge.getBoundingClientRect().right - view.getBoundingClientRect().left + view.scrollLeft;
    const max = Math.max(0, view.scrollWidth - view.clientWidth);
    view.scrollLeft = Math.min(Math.max(mergeRight - view.clientWidth, 0), max);
    measure();
  }

  onMount(() => {
    const mq = window.matchMedia(pinQuery);
    const sync = () => {
      pinned = mq.matches;
      if (!initialScrollDone) {
        // 钉右档不滚：merge / done 本来就常驻可见，初始位置留在流水线开头
        initialScrollDone = true;
        if (!pinned) scrollToMerge();
      }
      measure();
    };
    sync();
    mq.addEventListener('change', sync);
    window.addEventListener('resize', sync);
    return () => {
      mq.removeEventListener('change', sync);
      window.removeEventListener('resize', sync);
    };
  });
</script>

<div class="board" bind:this={scroller} onscroll={measure}>
  <!-- 看板此前没有标题：读屏进来只听到一片列头，不知道这是哪一页（票 06 / R2-20）。
       视觉上不摆（列头与脊线已经说清了画面），语义上必须有。 -->
  <h1 class="visually-hidden">看板</h1>
  <div class="hinner">
    <div class="row rails">
      <!-- A 段：可横滚的六列（站心 x 从 0 计） -->
      <div class="lane a">
        <PipelineRail variant="spine" {stations} segment={segments.scroll} />
      </div>
      <!-- B 段：钉住的两列。同一把刀——它的左缘就是钉缝，脊线与列在同一 x 处断开 -->
      <div class="lane b" bind:this={pinnedLane}>
        <PipelineRail variant="spine" {stations} segment={segments.pinned} lead={0} />
      </div>
      <!-- 右缘「还有 N 列」指示：指示不是控件——对读屏隐藏、不吃指针事件、不进 tab 序 -->
      <div class="more-anchor" aria-hidden="true">
        {#if hiddenColumns > 0}
          <span class="more">还有 {hiddenColumns} 列 ▸</span>
        {/if}
      </div>
    </div>

    <div class="notices">
      {#if board.error}
        <!-- 错误要说得出口、也要有出路（票 02 / R2-06 / R2-07c）：读屏靠 role=alert 听到，
             手上有这颗「重试」——不在别处，就在这条横幅底下。 -->
        <div class="banner error" role="alert">加载失败：{board.error}</div>
        <button
          type="button"
          class="btn retry"
          disabled={board.loading}
          onclick={() => void board.loadTasks()}
        >
          重试
        </button>
      {/if}
      {#if board.actionError}
        <!-- 动作提交失败必须可见（主流程票 03）：吞掉它 = 用户点「重试」毫无反应的死面板 -->
        <div class="banner error" role="alert">动作提交失败：{board.actionError}</div>
      {/if}
      {#if board.connectionState === 'error'}
        <div class="banner">实时流已断开，正在重连…（看板仍每 10s 对齐一次）</div>
      {/if}

      {#if board.projects.length === 0 && !board.loading}
        <!-- 空态（票 13）：状态 → 下一步 → 入口可点。提到另一个页面，就必须点得动 -->
        <div class="board-empty">
          <EmptyState
            state="还没有项目。"
            next="添加一个本地 git 仓库，就能新建任务了。"
            href="#/settings/projects"
            linkLabel="设置 · 项目"
          />
        </div>
      {:else if board.visibleTasks.length === 0 && !board.loading}
        <div class="board-empty">
          <EmptyState state="看板还是空的。" next="新建第一个任务，流水线从 init 开始走。" />
        </div>
      {/if}
    </div>

    <div class="row panes-row">
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
  /* 工位阵列（§2.3 描边只有 2px 一档）。宽档把最后两列钉在右侧，窄档整体横滚。
     几何都是列宽的整数关系（手工对齐），守护在 lib/pipeline.geometry.test.ts。 */
  .board {
    overflow-x: auto;
    background: var(--bg);
  }
  /* 横滚内容：左内缩 16 + 阵列 2130 + 右内缩 16 = 2162（与列宽手工对齐） */
  .hinner {
    width: calc(var(--rail-col-width) * 8 + 50px);
  }
  .row {
    display: flex;
  }
  /* A 段宽 = 左内缩 16 + 阵列左框线 2 + 六列；B 段宽 = 钉住的两列 */
  .lane.a {
    flex: none;
    width: calc(var(--rail-col-width) * 6 + 18px);
  }
  .lane.b {
    flex: none;
    width: calc(var(--rail-col-width) * 2);
    background: var(--bg);
  }
  .notices {
    padding: 0 16px;
  }
  .panes-row {
    /* 右内缩在这里（不是靠一个尾部占位块）：.hinner 的 2162 已把两端内缩算进去，
       阵列仍是冻结原型的 2130，flex 收缩不会把它压窄 */
    padding: 0 16px 8px;
  }
  .panes {
    display: flex;
    border: 2px solid var(--pane);
    background: var(--bg);
    width: calc(var(--rail-col-width) * 8 + 18px);
    align-items: stretch;
  }
  /* 右缘「还有 N 列」：两档共用一枚，锚在横滚区右缘（钉右档 = 紧贴钉缝左侧），
     纵向压在脊线带底部空带与阵列顶框之间。零宽锚点 + 绝对定位的子元素，
     故指示本身不占横滚内容宽度；`▸` 是文字字形，不新增 sprite。 */
  .more-anchor {
    flex: none;
    width: 0;
    position: sticky;
    right: 2px;
    align-self: flex-end;
  }
  .more {
    position: absolute;
    right: 0;
    bottom: 0;
    pointer-events: none;
    white-space: nowrap;
    background: var(--wash);
    border: 2px solid var(--pane);
    color: var(--text-3);
    font-size: 12px;
    line-height: 1;
    /* 高由内容撑出：12px 字 + 2×2px 描边 + 2px 竖内边距 ≈ 20px（§2.7 定稿形态） */
    padding: 2px 6px;
  }
  .banner {
    background: var(--panel);
    border: 2px solid var(--pane);
    padding: 8px 12px;
    font-size: 12px;
    color: var(--text-2);
    margin-bottom: 8px;
  }
  /* 错误横幅下的出路（票 02）：横幅与它的重试钮是同一件事。 */
  .btn.retry {
    margin: 0 0 10px;
  }
  .banner.error {
    border-color: var(--stop);
    color: var(--stop);
  }
  .board-empty {
    padding: 6px 4px 12px;
  }

  /* ── 钉右档（≥1400px）：merge / done 常驻右侧，钉缝 = merge 列左缘的 2px 竖线 ── */
  @media (min-width: 1400px) {
    /* 脊线 B 段钉在同一处：它的左缘就是钉缝 */
    .lane.b {
      position: sticky;
      right: 0;
    }
    /* 指示落在横滚区右缘、紧贴钉缝左侧（偏移 = 钉住区宽 + 2px，别与 2px 钉缝叠成一条粗线） */
    .more-anchor {
      right: calc(var(--rail-col-width) * 2 + 2px);
    }
    /* 钉住的两列：不参与横滚，且必须盖住滚到它们下面的列——否则点下去的是被盖住的那张卡 */
    .panes > :global(#s-merge) {
      position: sticky;
      right: var(--rail-col-width);
      border-left: 2px solid var(--pane);
      background: var(--bg);
      z-index: 3;
    }
    .panes > :global(#s-done) {
      position: sticky;
      right: 0;
      background: var(--bg);
      z-index: 3;
    }
  }

  /* ── 移动版：工位阵列 → 纵向站点带（§5 移动款） ── */
  @media (max-width: 479px) {
    .board {
      overflow-x: visible;
    }
    .hinner {
      width: auto;
    }
    .row {
      display: block;
    }
    .lane.a,
    .lane.b {
      width: auto;
      background: none;
    }
    .panes {
      display: block;
      width: auto;
      border: 0;
    }
    .notices {
      padding: 0 12px;
    }
    .panes-row {
      padding: 0 12px calc(52px + var(--safeb));
    }
    .more-anchor {
      display: none;
    }    .board-empty {
      padding: 6px 0 12px;
    }
  }
</style>
