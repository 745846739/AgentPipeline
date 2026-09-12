<script lang="ts">
  import type { MetricBar } from '../../lib/metrics';

  /**
   * 轨道分段条形图（design §7）：横条挂在轨道站点下，延续「轨道即导航」。
   * 无 KPI 卡片横排——每个站点一列：标签 / 站点圆点（骑在轨道线上）/ 横条 / 数值。
   */
  interface Props {
    title: string;
    subtitle?: string;
    bars: MetricBar[];
    emptyText?: string;
    ariaLabel?: string;
  }
  let {
    title,
    subtitle = '',
    bars,
    emptyText = '暂无数据。',
    ariaLabel = title,
  }: Props = $props();

  function widthPct(pct: number): string {
    const clamped = Math.max(0, Math.min(1, pct));
    return `${(clamped * 100).toFixed(1)}%`;
  }
</script>

<section class="chart">
  <header class="head">
    <h2 class="title cond">{title}</h2>
    {#if subtitle}<span class="sub">{subtitle}</span>{/if}
  </header>

  {#if bars.length === 0}
    <p class="empty">{emptyText}</p>
  {:else}
    <div class="plot" role="img" aria-label={ariaLabel}>
      <div class="rail-line"></div>
      <div class="cols" style:--n={bars.length}>
        {#each bars as bar (bar.key)}
          <div class="col">
            <span class="lbl" title={bar.label}>{bar.label}</span>
            <span class="dot {bar.tone}"></span>
            <div class="track">
              <div class="fill {bar.tone}" style:width={widthPct(bar.pct)}></div>
            </div>
            <span class="val mono">{bar.display}</span>
          </div>
        {/each}
      </div>
    </div>
  {/if}
</section>

<style>
  .chart {
    background: var(--panel);
    border: 1px solid var(--pane);
    border-radius: var(--r-panel);
    padding: 12px 14px 14px;
  }
  .head {
    display: flex;
    align-items: baseline;
    gap: 10px;
    margin-bottom: 12px;
  }
  .title {
    font-size: 13px;
    color: var(--text-hi);
  }
  .sub {
    font-size: 11.5px;
    color: var(--text-3);
  }
  .empty {
    color: var(--text-3);
    font-size: 12px;
    padding: 6px 0;
  }
  .plot {
    position: relative;
  }
  /* 轨道线：贯穿所有站点圆点（圆点以 panel 底挖空覆盖） */
  .rail-line {
    position: absolute;
    left: 0;
    right: 0;
    top: 22px;
    height: 1px;
    background: var(--hairline);
  }
  .cols {
    position: relative;
    display: grid;
    grid-template-columns: repeat(var(--n, 1), minmax(0, 1fr));
    gap: 6px;
  }
  .col {
    display: flex;
    flex-direction: column;
    align-items: center;
    min-width: 0;
  }
  .lbl {
    height: 16px;
    max-width: 100%;
    font-family: var(--font-code);
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.02em;
    color: var(--text-3);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .dot {
    box-sizing: border-box;
    position: relative;
    z-index: 1;
    width: 9px;
    height: 9px;
    border-radius: 0;
    background: var(--panel);
    border: 2px solid var(--text-3);
  }
  .dot.go {
    border-color: var(--go);
  }
  .dot.caution {
    border-color: var(--pending);
  }
  .dot.stop {
    border-color: var(--stop);
  }
  .dot.done {
    border-color: var(--done);
  }
  /* 分支不再是色相，降为亮度阶 */
  .dot.dev {
    border-color: var(--text-2);
  }
  .dot.test {
    border-color: var(--text-3);
  }
  .track {
    width: 100%;
    height: 6px;
    margin-top: 8px;
    border-radius: 0;
    background: var(--hairline);
    overflow: hidden;
  }
  .fill {
    height: 100%;
    border-radius: 0;
    background: var(--go);
    min-width: 2px;
    transition: width 0.3s ease-out;
  }
  .fill.caution {
    background: var(--pending);
  }
  .fill.stop {
    background: var(--stop);
  }
  .fill.done {
    background: var(--done);
  }
  .fill.dev {
    background: var(--text-2);
  }
  .fill.test {
    background: var(--text-3);
  }
  .val {
    margin-top: 5px;
    font-size: 10.5px;
    color: var(--text-2);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  @media (prefers-reduced-motion: reduce) {
    .fill {
      transition: none;
    }
  }
</style>
