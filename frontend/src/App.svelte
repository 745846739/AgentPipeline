<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import TopBar from './components/layout/TopBar.svelte';
  import ToastStack from './components/layout/ToastStack.svelte';
  import Board from './routes/Board.svelte';
  import Metrics from './routes/Metrics.svelte';
  import SettingsProjects from './routes/SettingsProjects.svelte';
  import SettingsProviders from './routes/SettingsProviders.svelte';
  import TaskDetail from './routes/TaskDetail.svelte';
  import { router } from './router.svelte';
  import { board } from './stores/board.svelte';

  const route = $derived(router.route);
  const taskId = $derived(route.name === 'task' ? route.id : null);
  const notFoundPath = $derived(route.name === 'not-found' ? route.path : '');

  onMount(() => {
    void board.init();
  });

  onDestroy(() => {
    board.dispose();
  });
</script>

<TopBar />

{#if route.name === 'board'}
  <Board />
{:else if taskId}
  {#key taskId}
    <TaskDetail id={taskId} />
  {/key}
{:else if route.name === 'settings-projects'}
  <SettingsProjects />
{:else if route.name === 'settings-providers'}
  <SettingsProviders />
{:else if route.name === 'metrics'}
  <Metrics />
{:else}
  <div class="notfound">页面不存在：{notFoundPath}</div>
{/if}

<ToastStack />

<style>
  .notfound {
    padding: 30px 20px;
    color: var(--text-3);
    font-size: 12.5px;
  }
</style>
