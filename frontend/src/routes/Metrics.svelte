<script lang="ts">
  import { onMount } from 'svelte';
  import { getGlobalMetrics, getTaskMetrics } from '../api/client';
  import type { GlobalMetrics, TaskMetrics } from '../api/types';
  import TrackSegmentBars from '../components/settings/TrackSegmentBars.svelte';
  import { mapGlobalMetrics, mapTaskMetrics } from '../lib/metrics';
  import { CompositionGuard, shouldSubmitOnEnter } from '../lib/enterToSend';

  /**
   * 全局指标（design/frontend-design.md §7 / theme-6-pixel.md §3.1「车间台账」）：
   * 成功率 / 各阶段平均耗时 / 重试率 / 逃逸事件 / 首过率，全部以「轨道分段条形图」
   * 呈现（横条挂在轨道站点下），**无 KPI 卡片横排**——「轨道即导航」在指标页成立。
   *
   * 首过率缺数据时**不画 0 冒充真实值**：整条换成一句话说明（§3.1）。其余指标照常。
   * token 消耗随导入语与任务级汇总两处露出。
   * 数据源与字段映射逐字不变（`lib/metrics.ts`，决策 130 / 137）。
   */

  let global = $state<GlobalMetrics | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);

  let taskIdInput = $state('');
  let taskMetrics = $state<TaskMetrics | null>(null);
  let taskLoading = $state(false);
  let taskError = $state<string | null>(null);
  /** 输入法组合态（决策 184）：输入法里敲字再回车是选字，不该直接去查台账。 */
  const composing = new CompositionGuard();

  const view = $derived(global ? mapGlobalMetrics(global) : null);
  const taskView = $derived(taskMetrics ? mapTaskMetrics(taskMetrics) : null);

  async function loadGlobal() {
    loading = true;
    error = null;
    try {
      global = await getGlobalMetrics();
    } catch (err) {
      error = (err as Error).message;
    } finally {
      loading = false;
    }
  }

  onMount(() => void loadGlobal());

  async function loadTask() {
    const id = taskIdInput.trim();
    if (!id) {
      taskError = '请填写任务 ID。';
      return;
    }
    taskLoading = true;
    taskError = null;
    try {
      taskMetrics = await getTaskMetrics(id);
    } catch (err) {
      taskMetrics = null;
      taskError = (err as Error).message;
    } finally {
      taskLoading = false;
    }
  }
</script>

<div class="page">
  <a class="crumb" href="#/">看板</a>
  <div class="p-head">
    <h1 class="p-title">全局指标</h1>
    <button type="button" class="btn" disabled={loading} onclick={() => loadGlobal()}>
      {#if loading}<span class="spin"></span>{/if}刷新
    </button>
  </div>

  {#if error}
    <div class="blank error">{error}</div>
  {:else if loading && !global}
    <div class="blank">正在加载指标…</div>
  {:else if global && view}
    <p class="hintline">
      统计口径见 core metrics（决策 130 / 137）：成功率 = done ÷（done+failed+cancelled）；
      重试率 = attempt &gt; 1 的 run 占比；逃逸事件 = trigger = kickback 的流转数；
      首过率 = validate_output 首次 attempt 即通过的比例。共 <b>{global.tasks}</b> 个任务，
      逃逸事件 <b>{view.escapeEvents}</b> 次，
      <span class="mono">total_tokens {view.tokenDisplay}</span> ·
      <span class="mono">calls {view.callsDisplay}</span>。
    </p>

    <!-- 站点分段条形图：横条挂在传送带站点下，无 KPI 卡片横排 -->
    <TrackSegmentBars
      title="成功率"
      subtitle="done ÷ (done + failed + cancelled)"
      bars={view.success}
      ariaLabel="任务成功率"
    />

    <TrackSegmentBars
      title="各阶段平均耗时"
      subtitle="横条按最长阶段归一"
      bars={view.duration}
      emptyText="还没有节点运行记录。"
      ariaLabel="各阶段平均耗时"
    />

    <TrackSegmentBars
      title="各阶段重试率"
      subtitle="attempt > 1 的比例"
      bars={view.retry}
      emptyText="还没有节点运行记录。"
      ariaLabel="各阶段重试率"
    />

    <TrackSegmentBars
      title="逃逸事件（kickback 按来源阶段）"
      subtitle="触发打回的流转数；条长按最大阶段归一"
      bars={view.escape}
      emptyText="还没有打回记录。"
      ariaLabel="逃逸事件分布"
    />

    {#if view.firstPassAvailable}
      <TrackSegmentBars
        title="首过率"
        subtitle="validate_output 首次 attempt 即通过的比例"
        bars={view.firstPass}
        ariaLabel="首过率"
      />
    {:else}
      <!-- 首过率缺数据：整条改用一句话说明，不画 0 冒充真实值（§3.1） -->
      <TrackSegmentBars
        title="首过率"
        subtitle="validate_output 首次 attempt 即通过的比例"
        bars={[]}
        emptyText="还没有 validate_output 运行记录（分母为 0），首过率无从计算——此处不显示 0 冒充真实值。"
        ariaLabel="首过率（无数据）"
      />
    {/if}

    {#if view.excludedStages.length > 0}
      <p class="excl">
        已从轨道图排除非站点阶段（决策 107）：{view.excludedStages.join('、')}。
      </p>
    {/if}
  {/if}

  <section class="chart panel task">
    <div class="chart-head">
      <h2>任务级指标</h2>
    </div>
    <div class="subform">
      <input
        class="input mono"
        bind:value={taskIdInput}
        placeholder="任务 ID（ULID）"
        onkeydown={(e) => {
          if (!shouldSubmitOnEnter(e, composing.active())) return;
          e.preventDefault();
          void loadTask();
        }}
        oncompositionstart={() => composing.start()}
        oncompositionend={() => composing.end()}
      />
      <button type="button" class="btn" disabled={taskLoading} onclick={loadTask}>
        {#if taskLoading}<span class="spin"></span>{/if}载入
      </button>
    </div>

    {#if taskError}
      <div class="blank error">{taskError}</div>
    {:else if taskView && taskMetrics}
      <div class="msum mono">
        <span>total_tokens {taskView.tokenDisplay}</span>
        <span>total_calls {taskView.callsDisplay}</span>
        <span class={taskView.tokenDrift ? 'drift' : ''}>stored_tokens {taskView.storedTokenDisplay}</span>
        <span class={taskView.callsDrift ? 'drift' : ''}>stored_calls {taskView.storedCallsDisplay}</span>
      </div>
      {#if taskView.tokenDrift || taskView.callsDrift}
        <p class="drift-note">
          stored_* 是任务表持久化值，与按 run 求和存在差异（决策 100 的父/子行口径或未落库更新）。
        </p>
      {/if}

      <div class="task-charts">
        <TrackSegmentBars
          title="阶段平均耗时"
          bars={taskView.duration}
          emptyText="该任务还没有节点运行记录。"
          ariaLabel="任务阶段平均耗时"
        />
        <TrackSegmentBars
          title="阶段重试率"
          bars={taskView.retry}
          emptyText="该任务还没有节点运行记录。"
          ariaLabel="任务阶段重试率"
        />
        {#if taskView.firstPass.length > 0}
          <TrackSegmentBars
            title="首过率"
            bars={taskView.firstPass}
            ariaLabel="任务首过率"
          />
        {:else}
          <!-- 缺数据不画 0：整条一句话说明（§3.1） -->
          <TrackSegmentBars
            title="首过率"
            bars={[]}
            emptyText="该任务还没有 validate_output 记录，首过率无从计算——此处不显示 0 冒充真实值。"
            ariaLabel="任务首过率（无数据）"
          />
        {/if}
      </div>
    {/if}
  </section>
</div>

<style>
  .page {
    max-width: var(--detail-max);
    margin: 0 auto;
    padding: 18px 20px 44px;
  }
  /* 页头：面包屑 + 24px 标题 + 右侧动作钮（§3.1） */
  .crumb {
    display: inline-block;
    color: var(--text-3);
    margin-bottom: 10px;
  }
  .crumb::before {
    content: '← ';
  }
  .crumb:hover {
    color: var(--text-hi);
  }
  .p-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 14px;
    margin: 6px 0 8px;
  }
  .p-title {
    font-size: 24px;
    font-weight: 400;
    color: var(--text-hi);
    line-height: 1.2;
  }
  .hintline {
    color: var(--text-3);
    line-height: 1.8;
    margin-bottom: 14px;
    max-width: 86ch;
  }
  .blank {
    padding: 10px 12px;
    border: 2px solid var(--pane);
    color: var(--text-3);
    margin-top: 10px;
  }
  .blank.error {
    border-color: var(--stop);
    color: var(--stop);
  }
  .excl {
    color: var(--text-3);
    margin-top: 8px;
  }
  /* 任务级台账盒：同一套 2px 描边 + 硬投影，只是内容更密 */
  .task {
    margin-top: 24px;
    padding: 12px 14px;
  }
  .chart-head {
    display: flex;
    align-items: baseline;
    gap: 10px;
    margin-bottom: 12px;
  }
  .chart-head h2 {
    font-size: 12px;
    font-weight: 400;
    letter-spacing: 0.08em;
    color: var(--text-hi);
  }
  .subform {
    display: flex;
    gap: 8px;
    align-items: center;
    flex-wrap: wrap;
  }
  .subform .input {
    max-width: 360px;
  }
  .msum {
    display: flex;
    gap: 16px;
    flex-wrap: wrap;
    margin-top: 12px;
    color: var(--text-2);
  }
  .msum .drift {
    color: var(--pending);
  }
  .drift-note {
    margin-top: 5px;
    color: var(--text-3);
  }
  .task-charts {
    margin-top: 12px;
  }
  .task-charts :global(.chart:last-child) {
    margin-bottom: 0;
  }
</style>
