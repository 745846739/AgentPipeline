<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { board } from '../../stores/board.svelte';
  import { formatTokens } from '../../lib/pipeline';

  /**
   * 底部状态行（theme-3 §3 共享元素映射 / §2.5 覆盖③）。
   * 桌面为 tmux 状态行；窄屏同一条，作移动版载波行（§8 视图 0）。
   */
  let clock = $state('');
  let timer: ReturnType<typeof setInterval> | null = null;

  const running = $derived(board.countFor('running'));
  const waiting = $derived(board.countFor('waiting'));
  const queued = $derived(board.countFor('queued'));
  const done = $derived(board.countFor('done'));
  const totalTokens = $derived(board.tasks.reduce((sum, t) => sum + (t.total_tokens ?? 0), 0));

  /* 深浅两款是同一份电文的两种材料（§2.5），切换即换 token。 */
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
  {#if board.pendingCount > 0}
    <span>⏸ <b class="pen">*{board.pendingCount}</b> 待处理</span>
  {/if}
  <span>▶ <b>{running}</b> 执行中</span>
  <span class="sum dep">等依赖 {waiting} · 排队 {queued} · 已完成 {done}</span>
  <span>本时辰 <b>{formatTokens(totalTokens)}</b> tok</span>
  <span class="sum keys">? 键位</span>
  <button type="button" class="theme-tog" onclick={toggleTheme} title="切换深色 / 浅色（同一份电文的两种材料）">
    [{theme === 'dark' ? '浅色' : '深色'}]
  </button>
  <span class="clock">{clock}</span>
</footer>

<style>
  /* 桌面 tmux 状态行（§3.1）；窄屏由 app.css 的 .carrier 规则接管 */
  .statusline {
    position: fixed;
    left: 0;
    right: 0;
    bottom: 0;
    z-index: 30;
    display: flex;
    align-items: center;
    gap: 16px;
    height: 30px;
    padding: 0 16px;
    background: var(--bar-band);
    border-top: 1px solid var(--pane);
    font-size: 11px;
    color: var(--text-3);
  }
  .statusline b {
    color: var(--text-2);
    font-weight: 500;
    font-variant-numeric: tabular-nums;
  }
  .statusline .pen {
    color: var(--pending);
  }
  .theme-tog {
    color: var(--text-3);
    font-size: 11px;
    letter-spacing: 0.04em;
  }
  .theme-tog:hover {
    color: var(--text-hi);
  }
  .statusline .clock {
    margin-left: auto;
    color: var(--text-2);
    font-variant-numeric: tabular-nums;
  }

  @media (max-width: 479px) {
    /* 窄屏：载波行（§8 视图 0），安全区内边距 */
    .statusline {
      height: calc(34px + var(--safeb));
      padding: 0 12px var(--safeb);
      font-size: 12px;
      gap: 14px;
    }
    /* 窄屏收掉次级汇总与键位提示，保留待处理 / 执行中 / token */
    .statusline .dep,
    .statusline .keys {
      display: none;
    }
    .statusline .theme-tog {
      min-height: 30px;
      font-size: 12px;
    }
    .statusline .clock {
      margin-left: auto;
    }
  }
</style>
