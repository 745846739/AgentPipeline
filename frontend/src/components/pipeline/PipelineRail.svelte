<script lang="ts">
  import type { MiniDotState, StationView } from '../../lib/pipeline';

  interface Props {
    variant: 'spine' | 'hero' | 'mini';
    /** spine / hero：各站点状态。 */
    stations?: StationView[];
    /** mini：9 刻度状态。 */
    dots?: MiniDotState[];
    /** mini 轨一行 9 站；游标点加光晕。 */
    ariaLabel?: string;
  }

  let { variant, stations = [], dots = [], ariaLabel = '流水线轨道' }: Props = $props();

  const SPINE_PATHS = [
    { d: 'M160 62 H452', cls: 'track' },
    { d: 'M452 62 C500 62 520 38 568 38 H920 C968 38 988 62 1036 62', cls: 'track dev' },
    { d: 'M452 62 C500 62 520 86 568 86 H920 C968 86 988 62 1036 62', cls: 'track test' },
    { d: 'M1036 62 H1328', cls: 'track' },
    { d: 'M1328 62 H1620', cls: 'track' },
    { d: 'M1620 62 H1912', cls: 'track' },
    { d: 'M1912 62 H2204', cls: 'track' },
  ];
  const SPINE_RETURNS = [
    'M985 74 C985 98 452 98 452 74',
    'M1328 74 C1328 104 1036 104 1036 74',
    'M1912 74 C1912 104 1620 104 1620 74',
    'M1912 78 C1912 110 1036 110 1036 78',
  ];

  const HERO_PATHS = [
    { d: 'M70 66 H196', cls: 'track' },
    { d: 'M196 66 C240 66 262 40 306 40 H334 C378 40 400 66 444 66', cls: 'track dev' },
    { d: 'M196 66 C240 66 262 92 306 92 H334 C378 92 400 66 444 66', cls: 'track test' },
    { d: 'M444 66 H710', cls: 'track' },
    { d: 'M710 66 H860', cls: 'track' },
    { d: 'M860 66 H1010', cls: 'track' },
    { d: 'M1010 66 H1090', cls: 'track' },
  ];
  const HERO_RETURNS = [
    'M470 78 C470 108 196 108 196 78',
    'M710 78 C710 108 560 108 560 78',
    'M1010 78 C1010 108 860 108 860 78',
    'M1010 82 C1010 114 560 114 560 82',
  ];

  function stationClass(state: string): string {
    switch (state) {
      case 'done':
        return 'station-done';
      case 'go':
        return 'station-go';
      case 'warn':
        return 'station-warn';
      case 'stop':
        return 'station-stop';
      case 'dev':
        return 'station-dev';
      case 'test':
        return 'station-test';
      default:
        return '';
    }
  }

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

  function labelClass(state: string): string {
    if (state === 'idle') return 'st-label dim';
    if (state === 'warn') return 'st-label warn';
    if (state === 'go') return 'st-label go';
    if (state === 'dev') return 'st-label dev';
    if (state === 'test') return 'st-label test';
    return 'st-label';
  }
</script>

{#if variant === 'mini'}
  <div class="minirail" role="img" aria-label={ariaLabel}>
    {#each dots as dot, i (i)}
      <span class="dot {dot}"></span>
      {#if i < dots.length - 1}<span class="seg {dot === 'past' || dot === 'done' ? 'lit' : ''}"></span>{/if}
    {/each}
  </div>
{:else if variant === 'spine'}
  <svg class="rail rail-spine" viewBox="0 0 2364 118" preserveAspectRatio="xMinYMid meet" role="img" aria-label="看板轨道脊线">
    {#each SPINE_RETURNS as d (d)}<path class="return" {d} />{/each}
    {#each SPINE_PATHS as p (p.d)}<path class="{p.cls} rail-draw" d={p.d} />{/each}
    {#each stations as s (s.key)}
      <g>
        <circle class="station {stationClass(s.state)}" cx={s.x} cy={s.y} r={s.parallel ? 4 : 5} />
        {#if s.label}<text class={labelClass(s.state)} x={s.x} y={s.y - 26}>{s.label}</text>{/if}
        {#if s.count !== undefined}<text class="st-count" x={s.x} y={s.y + 28}>{s.count}</text>{/if}
      </g>
    {/each}
  </svg>
{:else}
  <svg class="rail rail-hero" viewBox="0 0 1160 120" preserveAspectRatio="xMidYMid meet" role="img" aria-label="流水线轨道">
    {#each HERO_RETURNS as d, i (i)}<path class="return" {d} />{/each}
    {#each HERO_PATHS as p, i (i)}<path class="{p.cls} rail-draw" d={p.d} />{/each}
    {#each stations as s (s.key)}
      <g>
        <circle class="station {stationClass(s.state)}" cx={s.x} cy={s.y} r={s.parallel ? 4 : 5} />
        {#if s.label}
          <text class={labelClass(s.state)} x={s.x} y={s.y - 26}>
            {s.label} {glyph(s.state)}
          </text>
        {/if}
      </g>
    {/each}
  </svg>
{/if}

<style>
  .rail-spine {
    display: block;
    width: 2364px;
    height: 118px;
    border-bottom: 1px solid var(--line-soft);
  }
  .rail-hero {
    display: block;
    width: 100%;
    height: 120px;
  }
  .track {
    stroke: var(--line);
    stroke-width: 2;
    fill: none;
    stroke-linecap: round;
  }
  .track.dev {
    stroke: var(--branch-dev);
    opacity: 0.85;
  }
  .track.test {
    stroke: var(--branch-test);
    opacity: 0.85;
  }
  .return {
    stroke: var(--text-3);
    stroke-width: 1.2;
    fill: none;
    stroke-dasharray: 2 5;
    opacity: 0.5;
  }
  .station {
    fill: var(--ink-900);
    stroke: var(--text-3);
    stroke-width: 2;
  }
  .station-done {
    stroke: var(--signal-done);
  }
  .station-go {
    stroke: var(--signal-go);
  }
  .station-warn {
    stroke: var(--signal-caution);
    animation: breath 2.4s ease-in-out infinite;
  }
  .station-stop {
    stroke: var(--signal-stop);
  }
  .station-dev {
    stroke: var(--branch-dev);
  }
  .station-test {
    stroke: var(--branch-test);
  }
  .st-label {
    font-family: var(--font-cond);
    font-size: 11.5px;
    font-weight: 600;
    fill: var(--text-2);
    text-anchor: middle;
    letter-spacing: 0.03em;
  }
  .st-label.dim {
    fill: var(--text-3);
  }
  .st-label.warn {
    fill: var(--signal-caution);
  }
  .st-label.go {
    fill: var(--signal-go);
  }
  .st-label.dev {
    fill: var(--branch-dev);
  }
  .st-label.test {
    fill: var(--branch-test);
  }
  .st-count {
    font-family: var(--font-mono);
    font-size: 9.5px;
    fill: var(--text-3);
    text-anchor: middle;
  }

  /* 迷你轨（卡片 16px 密度） */
  .minirail {
    display: flex;
    align-items: center;
    margin: 10px 0 8px;
  }
  .minirail .seg {
    flex: 1;
    height: 1.5px;
    background: var(--line);
  }
  .minirail .seg.lit {
    background: var(--signal-done);
  }
  .minirail .dot {
    width: 5px;
    height: 5px;
    border-radius: 50%;
    background: var(--line);
    flex: none;
  }
  .minirail .dot.past,
  .minirail .dot.done {
    background: var(--signal-done);
  }
  .minirail .dot.cur {
    width: 8px;
    height: 8px;
    background: var(--signal-go);
    box-shadow: 0 0 6px var(--signal-go);
  }
  .minirail .dot.cur-warn {
    width: 8px;
    height: 8px;
    background: var(--signal-caution);
    box-shadow: 0 0 6px var(--signal-caution);
  }
  .minirail .dot.cur-stop {
    width: 8px;
    height: 8px;
    background: var(--signal-stop);
    box-shadow: 0 0 6px var(--signal-stop);
  }
  .minirail .dot.dev {
    background: var(--branch-dev);
    box-shadow: 0 0 5px var(--branch-dev);
  }
  .minirail .dot.tst {
    background: var(--branch-test);
    box-shadow: 0 0 5px var(--branch-test);
  }
</style>
