<script lang="ts">
  import { board, FILTER_LABELS, type StatusFilter } from '../../stores/board.svelte';
  import { router } from '../../router.svelte';
  import NewTaskDialog from '../board/NewTaskDialog.svelte';

  const FILTERS: StatusFilter[] = ['all', 'running', 'pending', 'waiting', 'queued', 'done', 'ended'];

  let newTaskOpen = $state(false);

  function openTask(id: string) {
    board.pendingOpen = false;
    router.navigate(`/task/${id}`);
  }
</script>

<header class="topbar">
  <a class="wordmark" href="#/" onclick={() => router.navigate('/')}>Agent<em>Pipeline</em></a>

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
        class="chip {board.filter === f ? 'on' : ''} {f === 'pending' ? 'pending' : ''}"
        onclick={() => board.setFilter(f)}
      >
        {FILTER_LABELS[f]}
        {#if board.countFor(f) > 0}<span class="c">{board.countFor(f)}</span>{/if}
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
      待处理 <span class="c">{board.pendingCount}</span>
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

  <button type="button" class="btn-new" onclick={() => (newTaskOpen = true)}>＋ 新建任务</button>
</header>

<NewTaskDialog open={newTaskOpen} onclose={() => (newTaskOpen = false)} />

<style>
  .topbar {
    position: sticky;
    top: 0;
    z-index: 20;
    display: flex;
    align-items: center;
    gap: 20px;
    height: 48px;
    padding: 0 20px;
    background: rgba(12, 18, 27, 0.92);
    backdrop-filter: blur(8px);
    border-bottom: 1px solid var(--line-soft);
  }
  .wordmark {
    font-weight: 600;
    font-size: 15px;
    letter-spacing: 0.02em;
    color: var(--text-hi);
    text-decoration: none;
    white-space: nowrap;
  }
  .wordmark:hover {
    text-decoration: none;
  }
  .wordmark em {
    font-style: normal;
    color: var(--text-3);
  }
  .appnav {
    display: flex;
    gap: 2px;
    flex: none;
  }
  .navlink {
    padding: 4px 10px;
    border-radius: var(--r-pill);
    color: var(--text-3);
    font-size: 12px;
    font-weight: 500;
    white-space: nowrap;
  }
  .navlink:hover {
    color: var(--text-2);
    background: var(--ink-700);
    text-decoration: none;
  }
  .navlink.on {
    color: var(--text-hi);
    background: var(--ink-700);
  }
  .proj select {
    background: var(--ink-800);
    border: 1px solid var(--line);
    border-radius: var(--r-pill);
    color: var(--text-2);
    font-size: 12.5px;
    padding: 3px 8px;
  }
  .filters {
    display: flex;
    gap: 2px;
    margin-left: auto;
  }
  .chip {
    padding: 4px 10px;
    border-radius: var(--r-pill);
    color: var(--text-3);
    font-size: 12px;
    font-weight: 500;
    white-space: nowrap;
  }
  .chip:hover {
    color: var(--text-2);
    background: var(--ink-700);
  }
  .chip.on {
    color: var(--text-hi);
    background: var(--ink-700);
  }
  .chip .c {
    font-family: var(--font-mono);
    font-size: 11px;
  }
  .chip.pending .c {
    color: var(--signal-caution);
  }
  .pending-wrap {
    position: relative;
  }
  .pending-count {
    color: var(--text-2);
    background: rgba(242, 179, 61, 0.08);
  }
  .pending-count .c {
    color: var(--signal-caution);
  }
  .dropdown {
    position: absolute;
    top: 34px;
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
    font-size: 12px;
  }
  .dd-item {
    display: grid;
    grid-template-columns: 10px 1fr;
    gap: 2px 8px;
    width: 100%;
    text-align: left;
    padding: 8px 10px;
    border-radius: var(--r-panel);
  }
  .dd-item:hover {
    background: var(--ink-700);
  }
  .dd-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--signal-caution);
    margin-top: 5px;
  }
  .dd-title {
    color: var(--text-hi);
    font-size: 12.5px;
  }
  .dd-msg {
    grid-column: 2;
    color: var(--text-3);
    font-size: 11.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .btn-new {
    margin-left: 4px;
    padding: 6px 14px;
    border-radius: var(--r-pill);
    background: var(--ink-700);
    border: 1px solid var(--line);
    font-weight: 500;
    font-size: 12.5px;
    white-space: nowrap;
  }
  .btn-new:hover {
    border-color: var(--text-3);
  }
</style>
