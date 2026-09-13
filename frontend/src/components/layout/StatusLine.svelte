<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { board } from '../../stores/board.svelte';
  import { formatTokens } from '../../lib/pipeline';
  import { GEOMETRY, gaugeFilled } from '../../theme/contract';
  import Gauge from '../render/Gauge.svelte';

  /**
   * 底部车间看板条（决策 169 / theme-6-pixel.md §3）。
   * 桌面为常驻看板条；窄屏同一条作载波行（§5 移动原型 `.carrier`），带安全区内边距。
   * 状态用实心像素灯 + 文字双编码，字符标记（⏸ / ▶ / 中点）已退役。
   */
  let clock = $state('');
  let timer: ReturnType<typeof setInterval> | null = null;

  const running = $derived(board.countFor('running'));
  const waiting = $derived(board.countFor('waiting'));
  const queued = $derived(board.countFor('queued'));
  const done = $derived(board.countFor('done'));
  const totalTokens = $derived(board.tasks.reduce((sum, t) => sum + (t.total_tokens ?? 0), 0));
  /** 量表接近满格（≥14/16 段）时转琥珀——与原型 `.gauge.g-warn` 的用法一致。 */
  const tokenTone = $derived(
    gaugeFilled(totalTokens) >= GEOMETRY.gaugeSegments - 2 ? 'warn' : 'go',
  );

  /* 深浅两款是像素机房的两套配色：夜班靛 / 掌机背光（§2.1 / §2.4）。 */
  const STORAGE_KEY = 'agentpipeline.theme';
  let theme = $state<'dark' | 'light'>('dark');

  function applyTheme(next: 'dark' | 'light') {
    theme = next;
    if (typeof document !== 'undefined') document.documentElement.dataset.theme = next;
    try {
      localStorage.setItem(STORAGE_KEY, next);
    } catch {
      // 隐私模式等禁用 localStorage：本次会话仍生效
    }
  }

  function toggleTheme() {
    applyTheme(theme === 'dark' ? 'light' : 'dark');
  }

  onMount(() => {
    try {
      const saved = localStorage.getItem(STORAGE_KEY);
      if (saved === 'light' || saved === 'dark') theme = saved;
    } catch {
      // 忽略
    }
    const tick = () => {
      clock = new Date().toLocaleTimeString('zh-CN', { hour12: false });
    };
    tick();
    timer = setInterval(tick, 1000);
  });

  onDestroy(() => {
    if (timer) clearInterval(timer);
  });
</script>

<footer class="statusline" aria-label="流水线状态">
  <span class="cell">
    <span class="lamp pen" aria-hidden="true"></span><b class="pen">{board.pendingCount}</b> 待处理
  </span>
  <span class="sep" aria-hidden="true">▪</span>
  <span class="cell">
    <span class="lamp go" aria-hidden="true"></span><b>{running}</b> 执行中
  </span>
  <span class="sep dep" aria-hidden="true">▪</span>
  <span class="cell dep">等依赖 <b>{waiting}</b></span>
  <span class="sep dep" aria-hidden="true">▪</span>
  <span class="cell dep">排队 <b>{queued}</b></span>
  <span class="sep dep" aria-hidden="true">▪</span>
  <span class="cell dep">已完成 <b>{done}</b></span>
  <span class="sep" aria-hidden="true">▪</span>
  <span class="cell tok">
    <Gauge tokens={totalTokens} tone={tokenTone} /> 总量 <b>{formatTokens(totalTokens)}</b> tok
  </span>
  <button
    type="button"
    class="theme-tog"
    onclick={toggleTheme}
    aria-label={theme === 'dark' ? '切换到浅色主题' : '切换到深色主题'}
    title="切换像素机房配色（夜班靛 / 掌机背光）"
  >
    <span class="sw" aria-hidden="true"></span>{theme === 'dark' ? '浅色' : '深色'}
  </button>
  <span class="clock">{clock}</span>
</footer>

<style>
  /* 车间看板条：2px 顶描边、分隔符 ▪、灯 + 文字双编码（§3） */
  .statusline {
    position: fixed;
    left: 0;
    right: 0;
    bottom: 0;
    z-index: 30;
    display: flex;
    align-items: center;
    gap: 12px;
    height: 36px;
    padding: 0 16px;
    background: var(--bg);
    border-top: 2px solid var(--pane);
    font-size: 12px;
    color: var(--text-3);
  }
  .cell {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    white-space: nowrap;
  }
  .statusline b {
    color: var(--text-2);
    font-variant-numeric: tabular-nums;
  }
  .statusline .pen,
  .statusline b.pen {
    color: var(--pending);
  }
  /* 实心像素灯（一枚灯 = 一个状态；不作大面积底色） */
  .lamp {
    width: 8px;
    height: 8px;
    background: var(--text-4);
  }
  .lamp.pen {
    background: var(--pending);
  }
  .lamp.go {
    background: var(--go);
  }
  .sep {
    color: var(--text-4);
  }
  .tok {
    gap: 6px;
  }
  .theme-tog {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    color: var(--text-3);
    white-space: nowrap;
  }
  .theme-tog:hover {
    color: var(--text-hi);
  }
  .theme-tog .sw {
    width: 8px;
    height: 8px;
    background: var(--go);
    border: 2px solid var(--ink);
  }
  .statusline .clock {
    margin-left: auto;
    color: var(--text-2);
    font-variant-numeric: tabular-nums;
  }

  @media (max-width: 479px) {
    /* 窄屏：载波行（§5 视图 0），安全区内边距；次级汇总收进桌面款 */
    .statusline {
      min-height: calc(40px + var(--safeb));
      height: auto;
      padding: 0 12px var(--safeb);
      gap: 12px;
    }
    .statusline .dep {
      display: none;
    }
    .statusline .theme-tog {
      min-height: 40px;
    }
  }
</style>
