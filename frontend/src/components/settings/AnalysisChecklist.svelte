<script lang="ts">
  import type { ProjectAnalysis } from '../../api/types';
  import {
    analysisChecklist,
    analysisReady,
    analysisStatusLabel,
  } from '../../lib/analysis';

  /** 分析结果核对清单（design §7 / theme-6 §3.1）：伪阶段探测事实 → 清单供确认，不做 KPI 展示。 */
  interface Props {
    analysis: ProjectAnalysis;
    /** 这份分析针对哪个项目（票 07：从任务侧跳过来时，这一栏要说清在看哪个项目）。 */
    project?: string;
    onclose: () => void;
  }
  let { analysis, project, onclose }: Props = $props();

  const ready = $derived(analysisReady(analysis));
  const items = $derived(analysis.result ? analysisChecklist(analysis.result) : []);
</script>

<section class="checklist">
  <div class="sub-head">
    <h2>项目分析 · 核对清单</h2>
    {#if project}<span class="target">{project}</span>{/if}
    {#if analysis.status === 'done'}
      <span class="st run">{analysisStatusLabel(analysis.status)}</span>
    {:else if analysis.status === 'failed'}
      <span class="st stop">{analysisStatusLabel(analysis.status)}</span>
    {:else}
      <span class="st wait">{analysisStatusLabel(analysis.status)}</span>
    {/if}
    <button type="button" class="btn quiet" onclick={onclose}>收起</button>
  </div>

  {#if ready}
    <p class="hintline lead">以下为 project_analysis 探测到的事实，确认无误后即可创建任务。</p>
    <ul>
      {#each items as item (item.key)}
        <li class:miss={!item.ok}>
          <span class="mk">{item.ok ? '✓' : '—'}</span>
          <span class="lb">{item.label}</span>
          <span class="vl mono">{item.value ?? '未探测到'}</span>
        </li>
      {/each}
    </ul>
    <div class="foot">
      <span class="reg-sub">确认仅为本页核对记录，不改后端配置。</span>
      <button type="button" class="btn solid" onclick={onclose}>确认清单</button>
    </div>
  {:else if analysis.status === 'failed'}
    <div class="error">{analysis.error ?? '分析失败，未返回原因。'}</div>
  {:else}
    <div class="running"><span class="st run">分析中</span>正在探测仓库事实，稍候…</div>
  {/if}
</section>

<style>
  /* 核对清单：2px 描边盒 + 两列网格（移动款单列，见 app.css 媒体查询） */
  .checklist {
    border: 2px solid var(--pane);
    background: var(--bg);
    margin-top: 10px;
  }
  .checklist .sub-head {
    margin: 0;
    padding: 6px 12px;
    border-bottom: 2px solid var(--pane);
    background: var(--panel);
  }
  .checklist .sub-head h2 {
    font-size: 12px;
    letter-spacing: 0.08em;
    color: var(--text-hi);
  }
  .checklist .sub-head .btn {
    margin-left: auto;
  }
  /* 这份分析针对哪个项目（票 07）：次级必读档，跟在本节标题后面 */
  .target {
    color: var(--text-3);
  }
  .lead {
    padding: 8px 12px 0;
  }
  ul {
    list-style: none;
    display: grid;
    grid-template-columns: 1fr 1fr;
  }
  li {
    display: grid;
    grid-template-columns: 16px 96px 1fr;
    align-items: baseline;
    gap: 0;
    padding: 4px 12px;
    border-bottom: 2px solid var(--wash);
    border-right: 2px solid var(--wash);
  }
  /* 缺项：弱色「—」+ 「未探测到」，绝不假装通过。
     票 15 归位（决策 195）：缺什么正是「还差哪几步才能建档」，读不到就会挡住下一步
     ——整条（含那个「—」标记，它是同一条信息的第二编码）走次级必读档，不是装饰档。 */
  li.miss {
    color: var(--text-3);
  }
  .mk {
    color: var(--go);
  }
  li.miss .mk {
    color: var(--text-3);
  }
  .lb {
    color: var(--text-2);
  }
  .vl {
    color: var(--text-hi);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  li.miss .vl {
    color: var(--text-3);
  }
  .foot {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 12px;
  }
  .foot .btn {
    margin-left: auto;
  }
  .running {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    color: var(--text-2);
    padding: 10px 12px;
  }
  .error {
    font-size: 12px;
    color: var(--stop);
    white-space: pre-wrap;
    padding: 10px 12px;
  }

  @media (max-width: 479px) {
    ul {
      grid-template-columns: 1fr;
    }
    li {
      grid-template-columns: 16px 88px 1fr;
      border-right: 0;
      border-bottom-color: var(--hairline);
    }
    li:last-child {
      border-bottom: 0;
    }
  }
</style>
