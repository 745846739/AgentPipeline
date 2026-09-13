<script lang="ts">
  import { board, FILTER_LABELS, type StatusFilter } from '../../stores/board.svelte';
  import { router } from '../../router.svelte';
  import NewTaskDialog from '../board/NewTaskDialog.svelte';
  import { BOARD_COLUMNS, columnForTask } from '../../lib/pipeline';

  const FILTERS: StatusFilter[] = ['all', 'running', 'pending', 'waiting', 'queued', 'done', 'ended'];

  let newTaskOpen = $state(false);

  const sessionName = $derived(
    board.projects.find((p) => p.id === board.projectId)?.name ?? 'AgentPipeline',
  );

  /**
   * 移动版轨道缩略条（theme-3 §8）：8 列各压成一个字符站点，
   * 站点字标与桌面脊线同源（● 完成 / ◆ 在跑或待处理 / ○ 空）。
   */
  const railCells = $derived.by(() =>
    BOARD_COLUMNS.map((column) => {
      const tasks = board.tasks.filter((t) => columnForTask(t) === column.key);
      const pen = tasks.some((t) => t.status === 'pending');
      const live = tasks.some((t) => t.status === 'running');
      const don = tasks.length > 0 && tasks.every((t) => t.status === 'done');
      const state = pen ? 'pen' : live ? 'live' : don ? 'don' : 'idle';
      const mark = pen || live ? '◆' : don ? '●' : '○';
      return { key: column.key, label: column.label, state, mark };
    }),
  );

  function openTask(id: string) {
    board.pendingOpen = false;
    router.navigate(`/task/${id}`);
  }
</script>

<header class="topbar">
  <div class="bar-top">
    <a class="wordmark" href="#/" onclick={() => router.navigate('/')}>agentpipeline</a>

    <span class="sess">0:{sessionName}</span>

    <nav class="railnav" aria-label="站点状态缩略">
      {#each railCells as cell (cell.key)}
        <span class="rn {cell.state}" title={cell.label} aria-hidden="true">
          <span class="mk">{cell.mark}</span>
        </span>
      {/each}
    </nav>
  </div>

  <div class="filters-row no-scrollbar">
    <nav class="appnav" aria-label="页面导航">
      <a
        href="#/metrics"
        class="navlink"
        class:on={router.route.name === 'metrics'}
        onclick={() => router.navigate('/metrics')}>指标</a
      >
      <a
        href="#/settings/projects"
        class="navlink"
        class:on={router.route.name === 'settings-projects'}
        onclick={() => router.navigate('/settings/projects')}>项目</a
      >
      <a
        href="#/settings/providers"
        class="navlink"
        class:on={router.route.name === 'settings-providers'}
        onclick={() => router.navigate('/settings/providers')}>模型与密钥</a
      >
      <a
        href="#/share"
        class="navlink"
        class:on={router.route.name === 'share'}
        onclick={() => router.navigate('/share')}>手机访问</a
      >
    </nav>

    {#if board.projects.length > 0}
      <label class="proj">
        <span class="visually-hidden">项目</span>
        <select
          value={board.projectId ?? ''}
          onchange={(e) => board.selectProject((e.currentTarget as HTMLSelectElement).value)}
        >
          {#each board.projects as p (p.id)}
            <option value={p.id}>{p.name}</option>
          {/each}
        </select>
      </label>
    {/if}

    <nav class="filters" aria-label="状态过滤">
      {#each FILTERS as f (f)}
        <button
          type="button"
          class="chip {board.filter === f ? 'on' : ''} {f === 'pending' ? 'pend' : ''}"
          onclick={() => board.setFilter(f)}
        >
          {FILTER_LABELS[f]}
          {#if board.countFor(f) > 0}
            <span class="c"
              >{#if f === 'pending'}<i class="star">*</i>{/if}{board.countFor(f)}</span
            >
          {/if}
        </button>
      {/each}
    </nav>

    <div class="pending-wrap">
      <button
        type="button"
        class="chip pending-count"
        aria-expanded={board.pendingOpen}
        onclick={() => board.togglePendingDropdown()}
      >
        待处理 <span class="c"><i class="star">*</i>{board.pendingCount}</span>
      </button>
      {#if board.pendingOpen}
        <div class="dropdown panel" role="menu">
          {#if board.pendingTasks.length === 0}
            <div class="dd-empty">当前没有待办任务。</div>
          {:else}
            {#each board.pendingTasks as task (task.id)}
              <button type="button" class="dd-item" role="menuitem" onclick={() => openTask(task.id)}>
                <span class="dd-dot"></span>
                <span class="dd-title">{task.title}</span>
                <span class="dd-msg">{task.pending_reason?.message ?? ''}</span>
              </button>
            {/each}
          {/if}
        </div>
      {/if}
    </div>

    <button type="button" class="btn btn-new" onclick={() => (newTaskOpen = true)}>
      <i>n</i> 新建任务
    </button>
  </div>
</header>

<NewTaskDialog open={newTaskOpen} onclose={() => (newTaskOpen = false)} />

<style>
  .topbar {
    position: sticky;
    top: 0;
    z-index: 20;
    display: flex;
    align-items: center;
    gap: 14px;
    height: 38px;
    padding: 0 16px;
    background: var(--bg);
    border-bottom: 1px solid var(--pane);
  }
  /* 移动版专属分组容器：桌面拍平（children 直接成为 .topbar 的 flex 子项），
     布局与旧 DOM 逐字一致 */
  .bar-top,
  .filters-row {
    display: contents;
  }
  .railnav {
    display: none;
  }
  .wordmark {
    background: var(--go);
    color: var(--go-ink);
    padding: 1px 8px;
    font-weight: 600;
    font-size: 12px;
    white-space: nowrap;
  }
  .wordmark:hover {
    text-decoration: none;
  }
  .sess {
    color: var(--text-2);
    font-size: 12px;
    white-space: nowrap;
  }
  .appnav {
    display: flex;
    gap: 2px;
    flex: none;
  }
  .navlink {
    padding: 2px 8px;
    color: var(--text-3);
    font-size: 11.5px;
    white-space: nowrap;
  }
  .navlink:hover {
    color: var(--text-2);
    background: var(--hover-bg);
    text-decoration: none;
  }
  .navlink.on {
    color: var(--text-hi);
    background: var(--panel);
  }
  .proj select {
    background: var(--input);
    border: 1px solid var(--pane);
    color: var(--text-2);
    font-size: 11.5px;
    padding: 2px 6px;
  }
  .filters {
    display: flex;
    gap: 2px;
    margin-left: auto;
  }
  .chip {
    padding: 2px 8px;
    color: var(--text-3);
    font-size: 11.5px;
    white-space: nowrap;
  }
  .chip:hover {
    color: var(--text-2);
  }
  .chip.on {
    background: var(--panel);
    color: var(--text-hi);
    border: 1px solid var(--pane);
    padding: 1px 7px;
  }
  .chip .c {
    color: var(--text-4);
    margin-left: 2px;
    font-variant-numeric: tabular-nums;
  }
  .chip .star {
    font-style: normal;
    color: var(--pending);
  }
  .pending-wrap {
    position: relative;
  }
  .pending-count {
    color: var(--text-3);
    background: var(--pending-tint);
  }
  .dropdown {
    position: absolute;
    top: 28px;
    right: 0;
    width: 360px;
    max-height: 60vh;
    overflow: auto;
    padding: 6px;
    z-index: 30;
    box-shadow: none;
  }
  .dd-empty {
    padding: 10px 12px;
    color: var(--text-3);
    font-size: 11.5px;
  }
  .dd-item {
    display: grid;
    grid-template-columns: 10px 1fr;
    gap: 2px 8px;
    width: 100%;
    text-align: left;
    padding: 8px 10px;
  }
  .dd-item:hover {
    background: var(--hover-bg);
  }
  .dd-dot::before {
    content: '!';
    color: var(--pending);
    font-weight: 600;
    font-size: 10.5px;
  }
  .dd-title {
    color: var(--text-hi);
    font-size: 12.5px;
  }
  .dd-msg {
    grid-column: 2;
    color: var(--text-3);
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .btn-new {
    margin-left: 4px;
    white-space: nowrap;
  }
  .btn-new i {
    font-style: normal;
    color: var(--text-3);
  }

  /* ── 移动版：状态行 + 轨道缩略条 + 横滚过滤行（theme-3 §8） ── */
  @media (max-width: 479px) {
    .topbar {
      display: block;
      height: auto;
      padding: 0;
      gap: 0;
    }
    .bar-top {
      display: flex;
      align-items: center;
      gap: 8px;
      height: 40px;
      padding: 0 12px;
      background: var(--bar-band);
      border-bottom: 1px solid var(--hairline);
    }
    .wordmark {
      flex: none;
    }
    .sess {
      flex: none;
    }
    .railnav {
      display: flex;
      align-items: center;
      flex: 1;
      min-width: 0;
      margin-left: 4px;
    }
    .rn {
      position: relative;
      flex: 1;
      min-width: 0;
      height: 40px;
      display: flex;
      align-items: center;
      justify-content: center;
    }
    .rn::before {
      content: '';
      position: absolute;
      left: 0;
      right: 0;
      top: 50%;
      height: 1px;
      background: var(--lit);
    }
    .rn .mk {
      position: relative;
      z-index: 1;
      background: var(--mask-bg);
      padding: 0 3px;
      font-size: 12px;
      line-height: 1;
      color: var(--text-4);
    }
    .rn.don .mk,
    .rn.idle .mk {
      color: var(--text-3);
    }
    .rn.live .mk {
      color: var(--text-hi);
    }
    .rn.pen .mk {
      color: var(--pending);
      animation: breath 2.4s ease-in-out infinite;
    }

    .filters-row {
      display: flex;
      align-items: center;
      gap: 2px;
      padding: 6px 12px;
      overflow-x: auto;
      scrollbar-width: none;
    }
    .filters-row::-webkit-scrollbar {
      display: none;
    }
    .appnav {
      flex: none;
    }
    .navlink {
      display: inline-flex;
      align-items: center;
      min-height: 34px;
      padding: 0 8px;
    }
    .proj select {
      min-height: 34px;
      font-size: 13px;
    }
    .filters {
      flex: none;
      margin-left: 2px;
    }
    .chip {
      display: inline-flex;
      align-items: center;
      min-height: 34px;
      padding: 0 10px;
      font-size: 13px;
      border: 1px solid transparent;
    }
    .chip.on {
      padding: 0 10px;
      background: var(--wash);
    }
    .pending-count {
      min-height: 34px;
    }
    /* 下拉脱离横滚容器的裁剪，改用视口定位 */
    .dropdown {
      position: fixed;
      left: 12px;
      right: 12px;
      top: 88px;
      width: auto;
      max-height: 55vh;
      background: var(--bg);
    }
    .btn-new {
      flex: none;
      margin-left: 4px;
    }
  }
</style>
