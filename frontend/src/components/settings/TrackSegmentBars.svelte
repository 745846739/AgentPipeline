<script lang="ts">
  import type { MetricBar } from '../../lib/metrics';

  /**
   * 指标条形图（票 10 / design/theme-6-pixel.md §3.1「指标条形图」）。
   *
   * 横条挂在传送带站点下，延续「轨道即导航」——**无 KPI 卡片横排**。
   * 结构逐项对应冻结原型 `design/prototype-pixel.html#v-metrics`：
   * `chart`（台账盒 = 2px 描边 + 4px 硬投影）→ `chart-head`（标题 + 副标题）
   * → `plot`（绝对定位 `rail-line`）→ `cols`（站点列：标签 / 12px `mdot` 灯 /
   * 10px `track` 横条 / 数值）。
   *
   * `mdot` 灯与 `fill` 条**同色**（§3.1 + 票面）：灯是空心描边方块、条是实心，
   * 但取同一枚 token。原型深色款 `mdot.dev` 写 `--text-2` 而 `fill.dev` 写
   * `--belt-lit`，与本条「同色」要求冲突——此处按票面与 §3.1 取条色 `--belt-lit`，
   * 两处统一（浅色款同）。
   *
   * 站点数不缩不折：移动款（≤479px，与 `app.css` 同一断点）9 站放不进 430px 时
   * `plot` 横向滚动，列宽下限 56px（原型移动款实测）。
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

<section class="chart panel">
  <div class="chart-head">
    <h2>{title}</h2>
    {#if subtitle}<span class="sub">{subtitle}</span>{/if}
    {#if bars.length > 4}
      <span class="scroll-hint">左右滑动看全部 {bars.length} 站</span>
    {/if}
  </div>

  {#if bars.length === 0}
    <p class="chart-empty">{emptyText}</p>
  {:else}
    <div class="plot" role="img" aria-label={ariaLabel}>
      <div class="rail-line"></div>
      <div class="cols" style:--n={bars.length}>
        {#each bars as bar (bar.key)}
          <div class="col">
            <span class="lb" title={bar.label}>{bar.label}</span>
            <span class="mdot {bar.tone}" aria-hidden="true"></span>
            <div class="track">
              <span class="fill {bar.tone}" style:width={widthPct(bar.pct)}></span>
            </div>
            <span class="val mono">{bar.display}</span>
          </div>
        {/each}
      </div>
    </div>
  {/if}
</section>

<style>
  /* 台账盒：2px 描边 + 硬投影复用全站 `.panel` 基元，这里只补指标图的间距 */
  .chart {
    padding: 12px 14px;
    margin-bottom: 12px;
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
  .sub {
    font-size: 12px;
    color: var(--text-3);
  }
  /* 移动款提示：桌面不显示（只在需要横滚的窄屏出现） */
  .scroll-hint {
    display: none;
    font-size: 12px;
    color: var(--text-3);
  }
  .chart-empty {
    padding: 4px 0;
    font-size: 12px;
    line-height: 1.8;
    color: var(--text-3);
  }
  .plot {
    position: relative;
  }
  /* 传送带脊线：6px 未点亮链节段（站点灯骑在其上） */
  .rail-line {
    position: absolute;
    left: 0;
    right: 0;
    top: 20px;
    height: 6px;
    background: var(--pane);
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
  .lb {
    max-width: 100%;
    color: var(--text-3);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    letter-spacing: 0.02em;
  }
  /* 灯：12px 空心描边方块，与条同 token（§3.1） */
  .mdot {
    position: relative;
    z-index: 1;
    width: 12px;
    height: 12px;
    margin: 2px 0;
    border: 2px solid var(--text-3);
    background: var(--panel);
  }
  .mdot.go {
    border-color: var(--go);
  }
  .mdot.caution {
    border-color: var(--pending);
  }
  .mdot.stop {
    border-color: var(--stop);
  }
  .mdot.done {
    border-color: var(--done);
  }
  .mdot.dev {
    border-color: var(--belt-lit);
  }
  .mdot.test {
    border-color: var(--text-3);
  }
  /* 横条：10px 高、2px 描边、bg 底 */
  .track {
    width: 100%;
    height: 10px;
    margin-top: 8px;
    border: 2px solid var(--pane);
    background: var(--bg);
  }
  .fill {
    display: block;
    height: 100%;
    min-width: 2px;
    background: var(--go);
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
    background: var(--belt-lit);
  }
  .fill.test {
    background: var(--text-3);
  }
  .val {
    margin-top: 5px;
    color: var(--text-2);
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }

  /* 移动款：9 站放不进 430px → plot 横滚，站点不缩不折（§3.1 / 原型移动款） */
  @media (max-width: 479px) {
    .chart {
      padding: 11px 12px;
    }
    .chart-head {
      flex-wrap: wrap;
      margin-bottom: 10px;
    }
    .scroll-hint {
      display: inline;
    }
    .plot {
      overflow-x: auto;
      scrollbar-width: none;
    }
    .plot::-webkit-scrollbar {
      display: none;
    }
    .cols {
      grid-template-columns: repeat(var(--n, 1), minmax(56px, 1fr));
      min-width: max-content;
    }
  }
</style>
