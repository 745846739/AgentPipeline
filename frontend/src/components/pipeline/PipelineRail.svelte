<script lang="ts">
  import type { MiniDotState, SpineSegment, StationView } from '../../lib/pipeline';
  import {
    RAIL_LABELS,
    SPINE_COUNT_LABEL,
    railTokens,
    segmentStationX,
    spineBelts,
    spineColumnIndex,
    tokenLitsSegment,
  } from '../../lib/pipeline';
  import { GEOMETRY } from '../../theme/contract';
  import Worker from './Worker.svelte';

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
    /**
     * spine 段（决策 196）：本段在看板列坐标里的起点与列数，缺省 = 整条看板。
     *
     * 看板把脊线切成两段（可横滚段 + 钉右段），两段各自从 0 计站心 x——
     * 与列的切法同源，脊线才不会和列错位。
     */
    segment?: SpineSegment;
    /** 左侧留白 px：与所在容器的内缩对齐（可横滚段 16px、钉右段 0）。 */
    lead?: number;
  }

  let {
    variant,
    stations = [],
    dots = [],
    ariaLabel = '流水线轨道',
    segment = { base: 0, span: GEOMETRY.columnCount },
    lead = 16,
  }: Props = $props();

  const tokens = $derived(railTokens(dots));

  /** spine 段内的链节带几何（px 由契约列宽推导）；hero 不用（它逐字对齐冻结原型）。 */
  const belts = $derived(spineBelts(segment));

  /** 本段的站点：按列归段，站心 x 减去段起点（段内从 0 计）。 */
  const shown = $derived(
    variant === 'spine'
      ? stations
          .filter((s) => {
            const col = spineColumnIndex(s.x);
            return col >= segment.base && col < segment.base + segment.span;
          })
          .map((s) => ({ ...s, x: segmentStationX(s.x, segment.base) }))
      : stations,
  );

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
        <!-- 小人随站（§5 转写 4）：只在当前站出现（在跑 / 急停） -->
        {#if row.state === 'go' || row.state === 'dev' || row.state === 'test' || row.state === 'warn'}
          <Worker rhythm={row.state === 'warn' ? 'wait' : 'run'} />
        {/if}
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
  <div
    class="rail {variant}"
    role="img"
    aria-label={ariaLabel}
    style={variant === 'spine' ? `padding-left:${lead}px` : undefined}
  >
    <div class="railline" class:run={variant === 'hero' && running}>
      {#if variant === 'spine'}
        <!-- 主链节带 + 并行双带 + 回流带：一套静态几何（冻结原型那套），按**段**平移
             （决策 196「同一把刀切列与脊线」）；段外那截由 .rail.spine 的 overflow 裁掉，
             段内站心 x 也从 0 计——脊线与列因此断在同一条 x 上 -->
        {#each belts as b (b.key)}
          {#if b.kind === 'ret'}
            <div class="ret" style="left:{b.left}px;width:{b.width}px;top:{b.top}px"><i>↩</i></div>
          {:else if b.kind === 'vt'}
            <div class="belt vt" style="left:{b.left}px;top:{b.top}px"></div>
          {:else}
            <div
              class="belt {b.kind === 'br' ? 'br' : ''}"
              style="left:{b.left}px;width:{b.width}px;top:{b.top}px"
            ></div>
          {/if}
        {/each}
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

      {#each shown as s (s.key)}
        {#if s.parallel}
          <div class="stn side {s.parallel === 'dev' ? 'up' : 'dn'}" style="left:{s.x}px;top:{s.y}px">
            <i class="lamp {lampClass(s.state)}"></i>
            <span class="lb {s.parallel === 'test' ? 't' : ''}">{s.label}</span>
          </div>
        {:else}
          <div class="stn" class:cur={s.state === 'go'} style="left:{s.x}px">
            <i class="lamp {lampClass(s.state)}"></i>
            <span class={lbClass(s.state)}>{s.label}</span>
            {#if s.count !== undefined}
              <!-- 口径标注（决策 197）：框里带 `累计` 词的必是流量，不带词的必是存量 -->
              <span class="ct" class:pen={s.state === 'warn'} title="累计到过这一站"
                ><span class="cum">{SPINE_COUNT_LABEL}&nbsp;</span><span class="n">{s.count}</span></span
              >
            {/if}
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
  /* 急停灯闪烁 + 小人双帧翻转（离散 opacity，§2.3 原则 4） */
  @keyframes blink {
    50% {
      opacity: 0;
    }
  }
  .rail.spine {
    /* 宽度随所在段（看板把它切成「可横滚段 + 钉右段」两块），站心 x 由段内列号推导 */
    width: 100%;
    height: 116px;
    /* 与看板阵列的左内缩对齐（16px，两者的 16 由宿主给）：
       站点 x = columnWidth/2 + columnWidth × j（j 在段内从 0 计）即列中心 */
    padding: 14px 0 0 16px;
    /* 段外那截链节带 / 回流带在这里被裁掉：两段拼起来就在钉缝处严丝合缝
       （决策 196「同一把刀切列与脊线」——脊线与列断在同一条 x 上） */
    overflow: hidden;
  }
  .rail.hero {
    width: 100%;
    max-width: var(--detail-max);
    height: 168px;
    padding: 6px 0 0;
    /* 票 18 / 决策 215（2026-10-01 起按用户指示落地「有意不做」项）：hero 轨道固定
       812px，从约 830px 起把整页撑出横向滚动。改为**容器内横滚**——不裁切（站点坐标
       写死在 lib/pipeline.ts，裁掉等于「后面的工位不存在」），滚动只发生在本容器，
       不传给文档；`.rail.spine` 的裁切语义不动。 */
    overflow-x: auto;
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
  /* hero 当前游标：灯的心跳微光（离散步进，取代字符时代的滑动圆点 + 柔光） */
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
  }
  /* 口径词（决策 197）：与数字同色同框、同字号；完整读法 `[累计 3]`
     （词后那个不换行空格就是读法里的空格），tabular-nums 只作用于数字本身 */
  .stn .ct .n {
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
    /* 遮罩要真的遮住链节（票 10）：原型 `.stn.side .lb{padding:0 4px}` 实现漏抄了，
       少了这两条边距，标签就比原型窄 8px、链节从字的两端透出 */
    padding: 0 4px;
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
