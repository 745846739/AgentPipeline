<script lang="ts">
  import { onMount } from 'svelte';
  import { getGlobalMetrics, getTaskMetrics } from '../api/client';
  import type { GlobalMetrics, TaskMetrics } from '../api/types';
  import TrackSegmentBars from '../components/settings/TrackSegmentBars.svelte';
  import { mapGlobalMetrics, mapTaskMetrics } from '../lib/metrics';

  /**
   * 全局指标面板（design §7）：成功率 / 各阶段平均耗时 / 重试率 / 逃逸事件，
   * 全部以「轨道分段条形图」呈现（横条挂在轨道站点下），**无 KPI 卡片横排**。
   * 任务级指标走 GET /tasks/{id}/metrics。
   */

  let global = $state<GlobalMetrics | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);

  let taskIdInput = $state('');
  let taskMetrics = $state<TaskMetrics | null>(null);
  let taskLoading = $state(false);
  let taskError = $state<string | null>(null);

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
  <a class="crumb" href="#/">← 看板</a>
  <header class="head">
    <h1 class="cond">全局指标</h1>
    <button type="button" class="btn" disabled={loading} onclick={() => loadGlobal()}>
      {#if loading}<span class="spin"></span>{/if}刷新
    </button>
  </header>

  {#if error}
    <div class="banner error">{error}</div>
  {:else if loading && !global}
    <div class="banner">正在加载指标…</div>
  {:else if global && view}
    <p class="hint">
      统计口径见 core metrics（决策 130 / 137）：成功率 = done ÷（done+failed+cancelled）；
      重试率 = attempt &gt; 1 的 run 占比；逃逸事件 = trigger = kickback 的流转数；
      首过率 = validate_output 首次 attempt 即通过的比例。
      共 {global.tasks} 个任务，逃逸事件 {view.escapeEvents} 次，
      <span class="mono">total_tokens {view.tokenDisplay}</span> ·
      <span class="mono">calls {view.callsDisplay}</span>。
    </p>

    <div class="charts">
      <TrackSegmentBars
        title="成功率"
        subtitle={global.success_rate === null ? '暂无已结算任务（分母为 0）' : 'done ÷ (done + failed + cancelled)'}
        bars={view.success}
        ariaLabel="任务成功率"
      />

      <TrackSegmentBars
        title="各阶段平均耗时"
        subtitle="横条长按最长阶段归一"
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
        <section class="note panel">
          <div class="note-head cond">首过率</div>
          <p>
            还没有 <span class="mono">validate_output</span> 运行记录（分母为 0），
            后端按「无数据」下发 <span class="mono">null</span>；此处不显示 0 冒充真实值。
          </p>
        </section>
      {/if}
    </div>

    {#if view.excludedStages.length > 0}
      <p class="excl">
        已从轨道图排除非站点阶段（决策 107）：{view.excludedStages.join('、')}。
      </p>
    {/if}
  {/if}

  <section class="task panel">
    <div class="task-head cond">任务级指标</div>
    <div class="task-form">
      <input
        class="input mono"
        bind:value={taskIdInput}
        placeholder="任务 ID（ULID）"
        onkeydown={(e) => e.key === 'Enter' && loadTask()}
      />
      <button type="button" class="btn" disabled={taskLoading} onclick={loadTask}>
        {#if taskLoading}<span class="spin"></span>{/if}载入
      </button>
    </div>

    {#if taskError}
      <div class="banner error">{taskError}</div>
    {:else if taskView && taskMetrics}
      <div class="task-summary mono">
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

      <div class="charts task-charts">
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
          <section class="note panel">
            <div class="note-head cond">首过率</div>
            <p>该任务还没有 validate_output 记录，首过率无从计算。</p>
          </section>
        {/if}
      </div>
    {/if}
  </section>
</div>

<style>
  .page {
    max-width: var(--detail-max);
    margin: 0 auto;
    padding: 20px 24px 60px;
  }
  .crumb {
    display: inline-flex;
    color: var(--text-3);
    font-size: 12px;
    margin-bottom: 10px;
  }
  .crumb:hover {
    color: var(--text-2);
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 14px;
    margin-bottom: 8px;
  }
  h1 {
    font-size: 18px;
    color: var(--text-hi);
  }
  .hint {
    font-size: 11.5px;
    color: var(--text-3);
    line-height: 1.6;
    margin-bottom: 14px;
  }
  .banner {
    padding: 10px 12px;
    border: 1px solid var(--pane);
    border-radius: 0;
    color: var(--text-3);
    font-size: 12px;
    margin-top: 10px;
  }
  .banner.error {
    border-color: var(--stop);
    color: var(--stop);
  }
  .charts {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .excl {
    margin-top: 8px;
    font-size: 11px;
    color: var(--text-3);
  }
  .note {
    padding: 12px 14px;
    background: var(--panel);
    border: 1px solid var(--pane);
    border-radius: 0;
  }
  .note-head {
    font-size: 13px;
    color: var(--text-hi);
    margin-bottom: 6px;
  }
  .note p {
    font-size: 11.5px;
    color: var(--text-3);
    line-height: 1.6;
  }
  .task {
    margin-top: 24px;
    padding: 14px 16px;
    background: var(--panel);
    border: 1px solid var(--pane);
    border-radius: 0;
  }
  .task-head {
    font-size: 13px;
    color: var(--text-hi);
    margin-bottom: 10px;
  }
  .task-form {
    display: flex;
    gap: 8px;
  }
  .task-form .input {
    max-width: 360px;
  }
  .task-summary {
    display: flex;
    gap: 16px;
    flex-wrap: wrap;
    margin-top: 12px;
    font-size: 12px;
    color: var(--text-2);
  }
  .task-summary .drift {
    color: var(--pending);
  }
  .drift-note {
    margin-top: 5px;
    font-size: 11px;
    color: var(--text-3);
  }
  .task-charts {
    margin-top: 12px;
  }
</style>
