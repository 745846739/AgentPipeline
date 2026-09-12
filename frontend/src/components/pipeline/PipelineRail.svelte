<script lang="ts">
  import type { MiniDotState, StationView } from '../../lib/pipeline';
  import { RAIL_LABELS, railTokens, tokenLitsSegment } from '../../lib/pipeline';

  /**
   * 字符轨道（theme-3 §3 共享元素映射）：结构靠字符与亮度，不靠盒子与 SVG。
   * 三种密度：spine（看板列头脊线）/ hero（任务详情）/ mini（卡片迷你轨，9 刻度）。
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

  /** 站点标记字符：与 frontend-design §6.1 图例逐字一致。 */
  function glyph(state: string): string {
    switch (state) {
      case 'done':
        return '✓';
      case 'go':
        return '●';
      case 'warn':
        return '⏸';
      case 'stop':
        return '✗';
      case 'dev':
      case 'test':
        return '●';
      default:
        return '○';
    }
  }

  /** 标记色彩阶（亮度编码 + 状态例外色）。 */
  function mkClass(state: string): string {
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

  function lbClass(state: string): string {
    switch (state) {
      case 'idle':
        return 'lb dim';
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

  /** 移动版纵向站点状态类（theme-3 §8：完成 / 当前 / 等待 / 失败）。 */
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

  /** 并行分支的纵向状态字符（✓ 完成 / ◆ 当前 / ⏸ 等待 / ✗ 失败 / ○ 未开始）。 */
  function vMark(state: string): string {
    switch (state) {
      case 'done':
        return '✓';
      case 'go':
      case 'dev':
      case 'test':
        return '◆';
      case 'warn':
        return '⏸';
      case 'stop':
        return '✗';
      default:
        return '○';
    }
  }

  /**
   * 纵向 8 行（theme-3 §8 / 原型）：desktop 的 9 站里并行双站合并为一行
   * `develop-design ∥ test-design`，分支状态收进 vsub（`[dev]✓ [tst]✓`）。
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
        label: s.parallel ? RAIL_LABELS['develop-design'] + ' ∥ ' + RAIL_LABELS['test-design'] : s.label,
        state: s.state,
        count: s.count,
        branches: s.parallel ? [{ kind: s.parallel, state: s.state }] : undefined,
      });
    }
    return rows;
  });
</script>

{#if variant === 'mini'}
  <div class="rail mini" role="img" aria-label={ariaLabel}>
    {#each tokens as t, i (i)}
      <span class="d {t}"></span>
      {#if i < tokens.length - 1}<span class="s {tokenLitsSegment(t) ? 'lit' : ''}"></span>{/if}
    {/each}
  </div>
{:else if variant === 'vrail'}
  <!-- 移动版纵向脊线（theme-3 §8）：8 站（并行双站合并为一行）+ 站段三态 -->
  <ul class="vrail" aria-label={ariaLabel}>
    {#each vrailRows as row (row.key)}
      <li class="vst {vClass(row.state)}">
        <span class="vmk">{vMark(row.state)}</span>
        <span class="vname">{row.label}</span>
        {#if row.branches}
          <span class="vsub"
            >{#each row.branches as b, i (b.kind)}<b class="bl" style={i > 0 ? 'margin-left:7px' : ''}
              >[{b.kind}]</b
            >{vMark(b.state)}{/each}</span
          >
        {:else if row.count !== undefined}
          <span class="vmeta">{row.count}</span>
        {/if}
      </li>
    {/each}
  </ul>
{:else}
  <div class="rail {variant}" role="img" aria-label={ariaLabel}>
    <div class="railline">
      {#if variant === 'spine'}
        <div class="ln" style="left:132px;width:1848px;top:36px"></div>
        <div class="ln br" style="left:396px;width:528px;top:22px"></div>
        <div class="ln br" style="left:396px;width:528px;top:50px"></div>
        <div class="ln vt" style="left:396px;top:22px"></div>
        <div class="ln vt" style="left:924px;top:22px"></div>
        <div class="ret" style="left:396px;width:264px;top:76px"><i>↩</i></div>
        <div class="ret" style="left:924px;width:264px;top:88px"><i>↩</i></div>
        <div class="ret" style="left:1452px;width:264px;top:76px"><i>↩</i></div>
        <div class="ret" style="left:924px;width:792px;top:100px"><i>↩</i></div>
      {:else}
        <div class="ln" style="left:50px;width:845px;top:36px"></div>
        <div class="ln br" style="left:145px;width:355px;top:22px"></div>
        <div class="ln br" style="left:145px;width:355px;top:50px"></div>
        <div class="ln vt" style="left:145px;top:22px"></div>
        <div class="ln vt" style="left:500px;top:22px"></div>
        <div class="ret" style="left:145px;width:165px;top:72px"><i>↩</i></div>
        <div class="ret" style="left:500px;width:110px;top:84px"><i>↩</i></div>
        <div class="ret" style="left:715px;width:105px;top:72px"><i>↩</i></div>
        <div class="ret" style="left:500px;width:320px;top:96px"><i>↩</i></div>
      {/if}

      {#each stations as s (s.key)}
        {#if s.parallel}
          <div
            class="stn side {s.parallel === 'dev' ? 'up' : 'dn'}"
            style="left:{s.x}px;top:{s.y}px"
          >
            <span class="mk {mkClass(s.state)}">{glyph(s.state)}</span>
            <span class="lb"
              ><b>[{s.parallel === 'dev' ? 'dev' : 'tst'}]</b>{s.label}</span
            >
          </div>
        {:else}
          <div class="stn" style="left:{s.x}px">
            <span class="mk {mkClass(s.state)}">{glyph(s.state)}</span>
            <span class={lbClass(s.state)}>{s.label}</span>
            {#if s.count !== undefined}<span class="ct">{s.count}</span>{/if}
          </div>
        {/if}
      {/each}
    </div>
  </div>
{/if}

<style>
  /* ── 迷你轨（卡片身份特征，9 刻度） ── */
  .rail.mini {
    display: flex;
    align-items: center;
    min-height: 15px;
    margin: 8px 0 7px;
  }
  .rail.mini .d {
    flex: none;
    width: 13px;
    text-align: center;
    font-size: 10.5px;
    line-height: 1;
    color: var(--text-4);
  }
  .rail.mini .d::before {
    content: '○';
  }
  .rail.mini .d.p {
    color: var(--text-2);
  }
  .rail.mini .d.p::before {
    content: '●';
  }
  .rail.mini .d.d {
    color: var(--text-3);
  }
  .rail.mini .d.d::before {
    content: '●';
  }
  .rail.mini .d.c,
  .rail.mini .d.v {
    color: var(--text-hi);
  }
  .rail.mini .d.c::before,
  .rail.mini .d.v::before {
    content: '◆';
  }
  .rail.mini .d.w {
    color: var(--pending);
  }
  .rail.mini .d.w::before {
    content: '◆';
  }
  .rail.mini .d.x {
    color: var(--stop);
  }
  .rail.mini .d.x::before {
    content: '◆';
  }
  .rail.mini .d.t {
    color: var(--text-3);
  }
  .rail.mini .d.t::before {
    content: '◇';
  }
  .rail.mini .s {
    flex: 1;
    height: 1px;
    background: var(--hairline);
  }
  .rail.mini .s.lit {
    background: var(--lit);
  }

  /* ── 移动版（theme-3 §8）：迷你轨放至 15px/12px；横向脊线由纵向段落取代 ── */
  @media (max-width: 479px) {
    .rail.mini {
      min-height: 18px;
      margin: 8px 0 7px;
    }
    .rail.mini .d {
      width: 15px;
      font-size: 12px;
    }
    .rail.spine {
      display: none;
    }
  }

  /* ── 字符线路行（脊线 / hero） ── */
  .rail.spine,
  .rail.hero {
    position: relative;
    background-image: var(--rail-band);
    background-size: var(--rail-band-size);
    background-position: var(--rail-band-pos);
    background-repeat: no-repeat;
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
    height: 106px;
    padding: 14px 0 0;
  }
  .railline {
    position: relative;
    height: 102px;
  }
  .ln {
    position: absolute;
    height: 1px;
    background: var(--text-2);
  }
  .ln.dim {
    background: var(--hairline);
  }
  .ln.br {
    background: var(--br);
  }
  .ln.br.done {
    background: var(--br-done);
  }
  .ln.vt {
    width: 1px;
    height: 29px;
  }
  .ret {
    position: absolute;
    border-top: 1px dashed var(--dash);
    color: var(--text-4);
    font-size: 9.5px;
    line-height: 1;
  }
  .ret i {
    font-style: normal;
    position: absolute;
    left: 0;
    top: -7px;
    background: var(--mask-bg);
    padding-right: 3px;
  }
  .stn {
    position: absolute;
    top: 29px;
    transform: translateX(-50%);
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 1px;
    text-align: center;
  }
  .stn .mk {
    font-size: 13px;
    line-height: 1;
    color: var(--text-4);
  }
  .stn .mk.p {
    color: var(--text-2);
  }
  .stn .mk.d {
    color: var(--text-3);
  }
  .stn .mk.c {
    color: var(--text-hi);
  }
  .stn .mk.w {
    color: var(--pending);
    animation: breath 2.4s ease-in-out infinite;
  }
  .stn .mk.x {
    color: var(--stop);
  }
  .stn .lb {
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.04em;
    color: var(--text-3);
    white-space: nowrap;
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
    font-size: 9.5px;
    color: var(--text-4);
    font-variant-numeric: tabular-nums;
  }
  /* 并行分岔侧站：两条分支共用同一 x，上下分行 */
  .stn.side {
    transform: translate(-6px, -50%);
  }
  .stn.side .mk {
    display: block;
  }
  .stn.side .lb {
    position: absolute;
    left: 16px;
    white-space: nowrap;
    background: var(--mask-bg);
    padding: 0 4px;
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.04em;
    color: var(--text-3);
  }
  .stn.side.up .lb {
    bottom: 9px;
  }
  /* 下轨标签同样右伸：原型的 right:16px 会向左越过前一站标签（theme-3 §8 已知缺陷） */
  .stn.side.dn .lb {
    top: 9px;
  }
  .stn.side .lb b {
    color: var(--text-4);
    font-weight: 500;
    margin-right: 4px;
  }

  @media (prefers-reduced-motion: reduce) {
    .stn .mk.w {
      animation: none;
    }
  }

  /* ── 移动版纵向脊线（hero 的窄屏转写，theme-3 §8） ── */
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
  .vst::before {
    content: '';
    position: absolute;
    left: 9px;
    top: 0;
    bottom: 0;
    width: var(--spin-w);
    background: var(--hairline);
  }
  .vmk {
    position: absolute;
    left: 0;
    top: 50%;
    transform: translateY(-50%);
    width: 19px;
    text-align: center;
    background: var(--bg);
    font-size: 12.5px;
    line-height: 1;
    color: var(--text-4);
  }
  .vname {
    font-size: 13px;
    color: var(--text-2);
  }
  .vmeta,
  .vsub {
    margin-left: auto;
    font-size: 11.5px;
    color: var(--text-3);
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }
  .vsub .bl {
    color: var(--text-3);
    font-weight: 600;
    font-size: 11px;
  }
  .vst.done::before {
    background: var(--spin-lit);
  }
  .vst.done .vmk {
    color: var(--text-3);
  }
  .vst.done .vname {
    color: var(--text-3);
  }
  .vst.cur::before {
    background: var(--spin-lit);
  }
  .vst.cur .vmk {
    color: var(--text-hi);
    animation: breath 2.4s ease-in-out infinite;
  }
  .vst.cur .vname {
    color: var(--text-hi);
    font-weight: 600;
  }
  .vst.pen::before {
    background: var(--spin-pen);
  }
  .vst.pen .vmk {
    color: var(--pending);
    animation: breath 2.4s ease-in-out infinite;
  }
  .vst.pen .vname {
    color: var(--pending);
    font-weight: 600;
  }
  .vst.fail .vmk {
    color: var(--stop);
  }
  .vst.cur .vmeta,
  .vst.pen .vmeta {
    color: var(--text-hi);
  }
  @media (prefers-reduced-motion: reduce) {
    .vst .vmk {
      animation: none;
    }
  }
</style>
