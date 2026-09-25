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
  import SettingsNotify from './routes/SettingsNotify.svelte';
  import Share from './routes/Share.svelte';
  import Talk from './routes/Talk.svelte';
  import TaskDetail from './routes/TaskDetail.svelte';
  import { router, type Route } from './router.svelte';
  import { board } from './stores/board.svelte';
  import { taskDetail } from './stores/taskDetail.svelte';
  import { talk } from './stores/talk.svelte';

  const route = $derived(router.route);
  const taskId = $derived(route.name === 'task' ? route.id : null);
  const notFoundPath = $derived(route.name === 'not-found' ? route.path : '');

  /**
   * 每个路由一个 `document.title`（票 06 / R2-20）。
   *
   * 此前 11 条路由共用 `index.html` 里那一个 `AgentPipeline · 像素机房`——标签页、前进后退的
   * 历史、书签全都分不清哪是哪。任务详情再带上**任务标题**：那一页的「这一页是什么」就是它。
   */
  const ROUTE_TITLES: Record<Route['name'], string> = {
    board: '看板',
    talk: '对讲台',
    task: '任务详情',
    metrics: '指标',
    'settings-landing': '设置',
    'settings-projects': '设置 · 项目',
    'settings-providers': '设置 · 模型与密钥',
    'settings-stages': '设置 · 阶段配置',
    'settings-market': '设置 · 技能市场',
    'settings-notify': '设置 · 离线通知',
    share: '手机访问',
    'not-found': '页面不存在',
  };
  const BASE_TITLE = 'AgentPipeline · 像素机房';

  $effect(() => {
    const name = route.name;
    const taskTitle = name === 'task' ? taskDetail.state.task?.title : null;
    document.title = `${taskTitle ?? ROUTE_TITLES[name]} · ${BASE_TITLE}`;
  });

  onMount(() => {
    void board.init();
    // 对讲台那条 `/foreman/stream` 也是**应用级**的（决策 275）：它随 App 起、随 App 收，
    // 不随页面来去——页面切走时它照旧收着那一轮的步骤，回来才不是「本轮之前的输出不见了」。
    talk.init();
  });

  onDestroy(() => {
    board.dispose();
    talk.dispose();
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
{:else if route.name === 'settings-notify'}
  <SettingsNotify />
{:else if route.name === 'metrics'}
  <Metrics />
{:else if route.name === 'share'}
  <Share />
{:else}
  <!-- 404（票 03，入口由决策 240 改口径）：与其余空态同一套语汇（状态 → 下一步），
       但**不再自带「回看板」链**——看板是顶栏那一枚页签，死路上再摆一个它的入口，
       等于「同一个目的地、两套进法」。出口交给**恒在的那一行页签**（设置 / 对讲台 / 指标 / 看板
       都在），故这里的下一步改成指路、不再代跑。
       `route.path` 已经是干净的路由名（不带 hash 路由里恒有的 `#`），照原样显示即可。
       标题与地标照其余路由补齐（票 06 / R2-20）：404 此前既没有 `<h1>` 也没有 `<main>`。 -->
  <main class="notfound">
    <h1 class="visually-hidden">页面不存在</h1>
    <EmptyState
      state={`页面不存在：${notFoundPath}`}
      next="这个地址没有对应的页面。顶栏那一行页签可以去别处。"
    />
  </main>
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
