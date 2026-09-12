<script lang="ts">
  import { board, FILTER_LABELS, type StatusFilter } from '../../stores/board.svelte';
  import { router } from '../../router.svelte';
  import NewTaskDialog from '../board/NewTaskDialog.svelte';

  const FILTERS: StatusFilter[] = ['all', 'running', 'pending', 'waiting', 'queued', 'done', 'ended'];

  let newTaskOpen = $state(false);

  const sessionName = $derived(
    board.projects.find((p) => p.id === board.projectId)?.name ?? 'AgentPipeline',
  );

  function openTask(id: string) {
    board.pendingOpen = false;
    router.navigate(`/task/${id}`);
  }
</script>

<header class="topbar">
  <a class="wordmark" href="#/" onclick={() => router.navigate('/')}>agentpipeline</a>

  <span class="sess">0:{sessionName}</span>

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
</style>
