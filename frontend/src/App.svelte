<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import TopBar from './components/layout/TopBar.svelte';
  import StatusLine from './components/layout/StatusLine.svelte';
  import ToastStack from './components/layout/ToastStack.svelte';
  import CompletionBanner from './components/layout/CompletionBanner.svelte';
  import EmptyState from './components/ui/EmptyState.svelte';
  import Board from './routes/Board.svelte';
  import Metrics from './routes/Metrics.svelte';
  import SettingsLanding from './routes/SettingsLanding.svelte';
  import SettingsProjects from './routes/SettingsProjects.svelte';
  import SettingsProviders from './routes/SettingsProviders.svelte';
  import SettingsStages from './routes/SettingsStages.svelte';
  import SettingsMarket from './routes/SettingsMarket.svelte';
  import Share from './routes/Share.svelte';
  import Talk from './routes/Talk.svelte';
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
{:else if route.name === 'talk'}
  <Talk />
{:else if taskId}
  {#key taskId}
    <TaskDetail id={taskId} />
  {/key}
{:else if route.name === 'settings-landing'}
  <SettingsLanding />
{:else if route.name === 'settings-projects'}
  <SettingsProjects />
{:else if route.name === 'settings-providers'}
  <SettingsProviders />
{:else if route.name === 'settings-stages'}
  <SettingsStages />
{:else if route.name === 'settings-market'}
  <SettingsMarket />
{:else if route.name === 'metrics'}
  <Metrics />
{:else if route.name === 'share'}
  <Share />
{:else}
  <!-- 404 给一条回看板的路（票 03）：与其余空态同一套语汇（状态 → 下一步 → 入口）。
       `route.path` 已经是干净的路由名（不带 hash 路由里恒有的 `#`），照原样显示即可。 -->
  <div class="notfound">
    <EmptyState
      state={`页面不存在：${notFoundPath}`}
      next="这个地址没有对应的页面。回看板看看任务现在跑到哪了。"
      href="#/"
      linkLabel="回看板"
    />
  </div>
{/if}

<ToastStack />
<!-- 完成横幅跨路由常驻（看板与详情都会触发）；层叠见组件说明：居中顶部 z35 < toast z70 -->
<CompletionBanner />
<StatusLine />

<style>
  .notfound {
    padding: 30px 20px;
  }
</style>
