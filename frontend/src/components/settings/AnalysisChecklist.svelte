<script lang="ts">
  import type { ProjectAnalysis } from '../../api/types';
  import {
    analysisChecklist,
    analysisReady,
    analysisStatusLabel,
  } from '../../lib/analysis';

  /** 分析结果核对清单（design §7）：伪阶段探测事实 → 清单供确认，不做 KPI 展示。 */
  interface Props {
    analysis: ProjectAnalysis;
    onclose: () => void;
  }
  let { analysis, onclose }: Props = $props();

  const ready = $derived(analysisReady(analysis));
  const items = $derived(analysis.result ? analysisChecklist(analysis.result) : []);
</script>

<section class="analysis">
  <header class="head">
    <span class="cond">项目分析 · 核对清单</span>
    {#if analysis.status === 'done'}
      <span class="st run">{analysisStatusLabel(analysis.status)}</span>
    {:else if analysis.status === 'failed'}
      <span class="st stop">{analysisStatusLabel(analysis.status)}</span>
    {:else}
      <span class="st wait">{analysisStatusLabel(analysis.status)}</span>
    {/if}
    <button type="button" class="btn quiet" onclick={onclose}>收起</button>
  </header>

  {#if ready}
    <p class="lead">以下为 project_analysis 探测到的事实，确认无误后即可创建任务（决策 78）。</p>
    <ul class="checklist">
      {#each items as item (item.key)}
        <li class:missing={!item.ok}>
          <span class="mark">{item.ok ? '✓' : '—'}</span>
          <span class="label">{item.label}</span>
          <span class="value mono">{item.value ?? '未探测到'}</span>
        </li>
      {/each}
    </ul>
    <div class="foot">
      <span class="foot-hint">确认仅为本页核对记录，不改后端配置。</span>
      <button type="button" class="btn solid" onclick={onclose}>确认清单</button>
    </div>
  {:else if analysis.status === 'failed'}
    <div class="error">{analysis.error ?? '分析失败，未返回原因。'}</div>
  {:else}
    <div class="running"><span class="st run">[RUN]</span>正在探测仓库事实，稍候…</div>
  {/if}
</section>

<style>
  .analysis {
    padding: 12px 14px;
    margin-top: 10px;
    background: var(--panel);
    border: 1px solid var(--pane);
  }
  .head {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 8px;
  }
  .head .cond {
    font-size: 13px;
    color: var(--text-hi);
  }
  .head .btn {
    margin-left: auto;
  }
  .lead {
    font-size: 11.5px;
    color: var(--text-3);
    margin-bottom: 8px;
  }
  .checklist {
    list-style: none;
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0 18px;
  }
  .checklist li {
    display: grid;
    grid-template-columns: 16px 96px 1fr;
    align-items: baseline;
    font-size: 12px;
    padding: 3px 0;
    border-bottom: 1px solid var(--hairline);
  }
  .checklist li.missing {
    color: var(--text-3);
  }
  .mark {
    color: var(--go);
  }
  .checklist li.missing .mark {
    color: var(--text-4);
  }
  .label {
    color: var(--text-2);
  }
  .value {
    color: var(--text-hi);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .checklist li.missing .value {
    color: var(--text-3);
  }
  .running {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    color: var(--text-2);
    padding: 4px 0;
  }
  .error {
    font-size: 12px;
    color: var(--stop);
    white-space: pre-wrap;
  }
  .foot {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-top: 10px;
  }
  .foot-hint {
    font-size: 11px;
    color: var(--text-3);
  }
  .foot .btn {
    margin-left: auto;
  }
</style>
