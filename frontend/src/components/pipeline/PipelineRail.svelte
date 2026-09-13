<script lang="ts">
  import type { MiniDotState, StationView } from '../../lib/pipeline';
  import { RAIL_LABELS, railTokens, tokenLitsSegment } from '../../lib/pipeline';

  /**
   * 传送带轨道（决策 169 / theme-6-pixel.md §3）：像素灯 + 链节取代字符线路行。
   * 三种密度：spine（看板列头脊线）/ hero（任务详情）/ mini（卡片迷你轨，9 刻度）。
   * 站点状态一律用灯表达（亮 / 半亮 / 空 / 琥珀 / 红），不再靠字符字形。
   */
  interface Props {
    variant: 'spine' | 'hero' | 'mini' | 'vrail';
    /** spine / hero / vrail：各站点状态。 */
    stations?: StationView[];
    /** mini：9 刻度状态。 */
    dots?: MiniDotState[];
    ariaLabel?: string;
  }

  let { variant, stations = [], dots = [], ariaLabel = '流水线轨道' }: Props = $props();

  const tokens = $derived(railTokens(dots));

  /** 有工位在跑（hero）：链节离散步进 + 当前灯心跳微光（票 06）。 */
  const running = $derived(stations.some((s) => s.state === 'go' || s.state === 'dev' || s.state === 'test'));

  /** 灯状态类：与契约 `STATE_STYLES.lamp` 的取色同源（d 完成 / p 已过 / c 在跑 / w 琥珀 / x 红）。 */
  function lampClass(state: string): string {
    switch (state) {
      case 'done':
        return 'd';
      case 'go':
        return 'c';
      case 'warn':
        return 'w';
      case 'stop':
        return 'x';
      case 'dev':
      case 'test':
        return 'p';
      default:
        return '';
    }
  }

  /** 迷你轨刻度：p 已过 / d 完成 / c 当前 / v 分叉 / w 琥珀 / x 红 / t test 未决 / f 未到。 */
  function miniClass(t: string): string {
    return t === 'd' ? 'dd' : t;
  }

  function lbClass(state: string): string {
    switch (state) {
      case 'idle':
      case 'done':
        return 'lb dim';
      case 'go':
        return 'lb hot';
      case 'warn':
        return 'lb pen';
      default:
        return 'lb';
    }
  }

  /** 移动版纵向站点状态类（完成 / 当前 / 等待 / 失败）。 */
  function vClass(state: string): string {
    switch (state) {
      case 'done':
        return 'done';
      case 'go':
      case 'dev':
      case 'test':
        return 'cur';
      case 'warn':
        return 'pen';
      case 'stop':
        return 'fail';
      default:
        return 'idle';
    }
  }

  /**
   * 纵向 8 行（原型移动款）：desktop 的 9 站里并行双站合并为一行
   * `develop-design ∥ test-design`，分支状态收进 vsub。
   */
  const vrailRows = $derived.by(() => {
    const rows: {
      key: string;
      label: string;
      state: string;
      count?: number;
      branches?: { kind: string; state: string }[];
    }[] = [];
    for (const s of stations) {
      if (s.parallel) {
        const prev = rows[rows.length - 1];
        if (prev && prev.branches) {
          prev.branches.push({ kind: s.parallel, state: s.state });
          continue;
        }
      }
      rows.push({
        key: s.key,
        label: s.parallel
          ? RAIL_LABELS['develop-design'] + ' ∥ ' + RAIL_LABELS['test-design']
          : s.label,
        state: s.state,
        count: s.count,
        branches: s.parallel ? [{ kind: s.parallel, state: s.state }] : undefined,
      });
    }
    return rows;
  });
</script>

{#if variant === 'mini'}
  <!-- 卡片迷你轨道：9 刻度像素方块 + 链节，游标处点亮（§3「看板卡」） -->
  <div class="rail mini" role="img" aria-label={ariaLabel}>
    {#each tokens as t, i (i)}
      <i class="d {miniClass(t)}"></i>
      {#if i < tokens.length - 1}<i class="s {tokenLitsSegment(t) ? 'lit' : ''}"></i>{/if}
    {/each}
  </div>
{:else if variant === 'vrail'}
  <!-- 移动版纵向脊线：8 站（并行双站合并为一行）+ 站段三态（完整转写见票 11） -->
  <ul class="vrail" aria-label={ariaLabel}>
    {#each vrailRows as row (row.key)}
      <li class="vst {vClass(row.state)}">
        <span class="vmk"><i class="lamp {lampClass(row.state)}"></i></span>
        <span class="vname">{row.label}</span>
        {#if row.branches}
          <span class="vsub"
            >{#each row.branches as b, i (b.kind)}<b
                class="bl {b.kind === 'test' ? 't' : ''}"
                style={i > 0 ? 'margin-left:7px' : ''}>[{b.kind}]</b
              >{/each}</span
          >
        {:else if row.count !== undefined}
          <span class="vmeta">{row.count}</span>
        {/if}
      </li>
    {/each}
  </ul>
{:else}
  <div class="rail {variant}" role="img" aria-label={ariaLabel}>
    <div class="railline" class:run={variant === 'hero' && running}>
      {#if variant === 'spine'}
        <!-- 主链节带 + 并行双带（在 develop 前合流） -->
        <div class="belt" style="left:132px;width:1848px;top:36px"></div>
        <div class="belt br" style="left:396px;width:528px;top:22px"></div>
        <div class="belt br" style="left:396px;width:528px;top:50px"></div>
        <div class="belt vt" style="left:396px;top:22px"></div>
        <div class="belt vt" style="left:924px;top:22px"></div>
        <!-- 回流带（打回路径，虚线） -->
        <div class="ret" style="left:396px;width:264px;top:76px"><i>↩</i></div>
        <div class="ret" style="left:924px;width:264px;top:88px"><i>↩</i></div>
        <div class="ret" style="left:1452px;width:264px;top:76px"><i>↩</i></div>
        <div class="ret" style="left:924px;width:792px;top:100px"><i>↩</i></div>
      {:else}
        <!-- hero：与冻结原型 #v-run .hrail 逐行对齐——主站灯在顶行，主带 y=76，
             并行双带 y=62/90（twin belts），回流带 116/132/148 -->
        <div class="belt" style="left:56px;width:742px;top:76px"></div>
        <div class="belt br" style="left:162px;width:212px;top:62px"></div>
        <div class="belt br" style="left:162px;width:212px;top:90px"></div>
        <div class="belt vt" style="left:162px;top:62px"></div>
        <div class="belt vt" style="left:374px;top:62px"></div>
        <!-- 回流带（打回路径，虚线） -->
        <div class="ret" style="left:162px;width:212px;top:116px"><i>↩</i></div>
        <div class="ret" style="left:374px;width:106px;top:132px"><i>↩</i></div>
        <div class="ret" style="left:586px;width:106px;top:116px"><i>↩</i></div>
        <div class="ret" style="left:374px;width:318px;top:148px"><i>↩</i></div>
      {/if}

      {#each stations as s (s.key)}
        {#if s.parallel}
          <div class="stn side {s.parallel === 'dev' ? 'up' : 'dn'}" style="left:{s.x}px;top:{s.y}px">
            <i class="lamp {lampClass(s.state)}"></i>
            <span class="lb {s.parallel === 'test' ? 't' : ''}">{s.label}</span>
          </div>
        {:else}
          <div class="stn" class:cur={s.state === 'go'} style="left:{s.x}px">
            <i class="lamp {lampClass(s.state)}"></i>
            <span class={lbClass(s.state)}>{s.label}</span>
            {#if s.count !== undefined}<span class="ct">{s.count}</span>{/if}
          </div>
        {/if}
      {/each}
    </div>
  </div>
{/if}

<style>
  /* ── 迷你轨（卡片身份特征，9 刻度）：像素方块 + 链节，取代字符 ●○◆ ── */
  .rail.mini {
    display: flex;
    align-items: center;
    min-height: 12px;
    margin: 8px 0 7px;
  }
  .rail.mini .d {
    flex: none;
    width: 6px;
    height: 6px;
    background: var(--pane);
  }
  .rail.mini .d.p,
  .rail.mini .d.dd {
    background: var(--belt-lit);
  }
  .rail.mini .d.c,
  .rail.mini .d.v {
    background: var(--go);
  }
  .rail.mini .d.w {
    background: var(--pending);
  }
  .rail.mini .d.x {
    background: var(--stop);
  }
  .rail.mini .d.t {
    background: transparent;
    border: 2px solid var(--pane);
  }
  .rail.mini .s {
    flex: 1;
    height: 4px;
    background: var(--pane);
  }
  .rail.mini .s.lit {
    background: var(--belt-lit);
  }

  /* ── 移动版：脊线由纵向段落取代（横向脊线隐藏） ── */
  @media (max-width: 479px) {
    .rail.mini {
      min-height: 12px;
      margin: 8px 0 7px;
    }
    .rail.mini .d {
      width: 8px;
      height: 8px;
    }
    .rail.spine {
      display: none;
    }
  }

  /* ── 传送带（脊线 / hero）：像素链节 + 信号灯 ── */
  .rail.spine,
  .rail.hero {
    position: relative;
    background-repeat: no-repeat;
  }
  /* 运行中工位两侧链节以离散步进位移（唯一动画位之一，§2.3 原则 4）
     步长 = 一个亮/暗块周期（12px），steps(2) 离散到 6px 块边界 */
  @keyframes beltstep {
    from {
      background-position: 0 0;
    }
    to {
      background-position: 12px 0;
    }
  }
  .rail.spine {
    width: 2144px;
    height: 116px;
    /* 与 .boardpad 的 16px 内缩对齐：站点 x=132+264i 即列中心 */
    padding: 14px 0 0 16px;
  }
  .rail.hero {
    width: 100%;
    max-width: var(--detail-max);
    height: 168px;
    padding: 6px 0 0;
  }
  /* hero 站灯排在顶行、链节带在其下（冻结原型 #v-run .hrail 的层级） */
  .rail.hero .railline {
    height: 162px;
  }
  .rail.hero .stn {
    top: 0;
  }
  .railline {
    position: relative;
    height: 102px;
  }
  /* 链节带：6px 高，6px 亮 / 6px 暗（周期 12px），硬边像素条 */
  .belt {
    position: absolute;
    height: 6px;
    background: repeating-linear-gradient(90deg, var(--pane) 0 6px, transparent 6px 12px);
  }
  /* 运行中工位两侧链节以离散步进位移（唯一动画位之一，§2.3 原则 4）；
     只动水平链节带，纵向连接带（.vt）与回流带不参与 */
  .railline.run .belt:not(.vt) {
    animation: beltstep 0.6s steps(2) infinite;
  }
  .belt.br {
    background: repeating-linear-gradient(90deg, var(--branch-dev) 0 6px, transparent 6px 12px);
    opacity: 0.55;
  }
  .belt.vt {
    width: 6px;
    height: 29px;
    background: repeating-linear-gradient(180deg, var(--pane) 0 6px, transparent 6px 12px);
  }
  /* 回流带：打回路径，虚线，出现打回任务时由宿主加 .lit 点亮为红 */
  .ret {
    position: absolute;
    height: 2px;
    background: repeating-linear-gradient(90deg, var(--text-4) 0 6px, transparent 6px 12px);
    color: var(--text-4);
    font-size: 12px;
    line-height: 1;
  }
  .ret i {
    font-style: normal;
    position: absolute;
    left: 0;
    top: -6px;
    background: var(--bg);
    padding-right: 4px;
  }
  /* 站点：12px 信号灯方块 + 工位名。top 使灯心正落在链节带中线（y=36）上。 */
  .stn {
    position: absolute;
    top: 30px;
    transform: translateX(-50%);
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    text-align: center;
  }
  .lamp {
    display: block;
    width: 12px;
    height: 12px;
    border: 2px solid var(--pane);
    background: var(--bg);
  }
  .lamp.d {
    background: var(--done);
    border-color: var(--done);
  }
  .lamp.p {
    background: var(--belt-lit);
    border-color: var(--belt-lit);
  }
  .lamp.c {
    background: var(--go);
    border-color: var(--go);
  }
  /* hero 当前游标：灯的心跳微光（离散步进，取代主题三的滑动圆点 + 柔光） */
  @keyframes heartbeat {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.4;
    }
  }
  .rail.hero .stn.cur .lamp.c {
    animation: heartbeat 1.2s steps(2) infinite;
  }
  .lamp.w {
    background: var(--pending);
    border-color: var(--pending);
    animation: blink 1s steps(2) infinite;
  }
  .lamp.x {
    background: var(--stop);
    border-color: var(--stop);
  }
  .stn .lb {
    font-size: 12px;
    letter-spacing: 0.08em;
    color: var(--text-3);
    white-space: nowrap;
    line-height: 1.2;
  }
  .stn .lb.dim {
    color: var(--text-4);
  }
  .stn .lb.hot {
    color: var(--text-hi);
  }
  .stn .lb.pen {
    color: var(--pending);
  }
  .stn .ct {
    display: inline-block;
    font-size: 12px;
    line-height: 1.2;
    color: var(--text-3);
    border: 2px solid var(--pane);
    padding: 0 4px;
    font-variant-numeric: tabular-nums;
  }
  .stn .ct.pen {
    color: var(--pending);
    border-color: var(--pending);
  }
  /* 并行分岔侧站：两条分支共用同一 x，上下分行 */
  .stn.side {
    transform: translate(-6px, -50%);
    top: 0;
  }
  .stn.side .lb {
    position: absolute;
    left: 18px;
    white-space: nowrap;
    background: var(--bg);
    font-size: 12px;
    letter-spacing: 0.04em;
    color: var(--text-3);
  }
  .stn.side.up .lb {
    bottom: 2px;
  }
  /* 下轨标签右伸（原型 .stn.side.dn .lb 用 right:16px 会向左越过前一站标签，
     此处统一右伸为 left:18px，是票 05 登记的既有偏差） */
  .stn.side.dn .lb {
    top: 2px;
  }
  .stn.side .lb.t {
    color: var(--branch-tst);
  }

  @media (prefers-reduced-motion: reduce) {
    .lamp.w,
    .rail.hero .stn.cur .lamp.c,
    .railline.run .belt {
      animation: none;
    }
  }

  /* ── 移动版纵向脊线（hero 的窄屏转写；完整转写见票 11） ── */
  .vrail {
    list-style: none;
    margin: 0;
  }
  .vst {
    position: relative;
    display: flex;
    align-items: center;
    gap: 9px;
    min-height: 32px;
    padding-left: 30px;
  }
  /* 站段脊线 = 6px 纵向链节；三态与桌面链节同色 */
  .vst::before {
    content: '';
    position: absolute;
    left: 9px;
    top: 0;
    bottom: 0;
    width: 6px;
    background: repeating-linear-gradient(180deg, var(--pane) 0 6px, transparent 6px 12px);
  }
  .vmk {
    position: absolute;
    left: 0;
    top: 50%;
    transform: translateY(-50%);
    width: 24px;
    display: flex;
    justify-content: center;
  }
  .vname {
    font-size: 12px;
    color: var(--text-2);
  }
  .vmeta,
  .vsub {
    margin-left: auto;
    font-size: 12px;
    color: var(--text-3);
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }
  .vsub .bl {
    color: var(--branch-dev);
    font-size: 12px;
  }
  .vsub .bl.t {
    color: var(--branch-tst);
  }
  .vst.done::before,
  .vst.cur::before {
    background: repeating-linear-gradient(180deg, var(--belt-lit) 0 6px, transparent 6px 12px);
  }
  .vst.pen::before {
    background: repeating-linear-gradient(180deg, var(--pending) 0 6px, transparent 6px 12px);
    opacity: 0.45;
  }
  .vst.done .vname {
    color: var(--text-3);
  }
  .vst.cur .vname {
    color: var(--text-hi);
  }
  .vst.pen .vname {
    color: var(--pending);
  }
  .vst.cur .vmeta,
  .vst.pen .vmeta {
    color: var(--text-hi);
  }
</style>
