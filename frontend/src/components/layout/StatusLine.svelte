<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { board } from '../../stores/board.svelte';
  import { formatClockAt } from '../../lib/format';
  import { formatTokens } from '../../lib/pipeline';
  import { GEOMETRY, gaugeFilled } from '../../theme/contract';
  import Gauge from '../render/Gauge.svelte';
  import ThemeToggle from './ThemeToggle.svelte';

  /**
   * 底部车间看板条（决策 169 / theme-6-pixel.md §3）。
   * 状态用实心像素灯 + 文字双编码，字符标记（⏸ / ▶ / 中点）已退役。
   *
   * **只在 ≥480px 露出**（决策 300，修订决策 243 的 ②④）：窄档整条 `display:none`——
   * 手机自己那条状态栏在报时、屏幕本来就窄，条上的读数另有去处（看板顶栏道具栏行的
   * 待处理 / 执行中、指标页的 token 总量），而唯一的动作（深浅切换）已随
   * `ThemeToggle` 迁到设置落地页页头。**元素仍在 DOM**（桌面档要靠它），与铭牌行
   * 退场（决策 242①）同一处置；底部让位账本 `--sbar-h` 在窄档随之收成 `--nav-h`
   * （只剩页签栏一层），五个消费点一个都不用改。中间档（480–1240）的舍格逻辑
   * （决策 215）原样保留在下面。
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

  /* 深浅切换的状态住在 `stores/theme.svelte.ts`（决策 300：切换钮两处挂载、一份状态）。 */

  onMount(() => {
    // 时钟也走全站口径（票 15）：时间格式只有 `lib/format.ts` 一处出处
    const tick = () => {
      clock = formatClockAt(new Date());
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
  <ThemeToggle />
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
  .statusline .clock {
    margin-left: auto;
    color: var(--text-2);
    font-variant-numeric: tabular-nums;
  }

  /* ── 中间档（决策 215 / 票 08 / R2-10）：480–748 不再静默裁切 ──
     实测这一行在 749px 之下**恒定**需要 748px 宽（`clockRight=748`），此前既没有折行也没有
     横滚，于是时钟整块、主题钮的一部分安静地消失。取舍按决策 215：**先舍在看板列头与过滤槽
     上都有同一个数的三格汇总**（约 220px），再舍纯视觉量表（约 110px，数字逐字保留）。
     容器同时加一条横滚兜底——**舍格优先、可滚兜底，绝不静默裁切**；滚动条不占高
     （`--sbar-h` 参与对讲台的高度公式，这里不能长出 15px 去动那笔账），与 `.slots` /
     `.navbar` / `.tabs` 的既有手法一致。 */
  @media (max-width: 748px) {
    .statusline {
      gap: 10px;
      overflow-x: auto;
      scrollbar-width: none;
    }
    .statusline::-webkit-scrollbar {
      display: none;
    }
    .statusline .dep {
      display: none;
    }
  }
  @media (max-width: 560px) {
    .statusline .tok :global(.gauge) {
      display: none;
    }
  }

  @media (max-width: 479px) {
    /* 窄档整条退场（决策 300，修订决策 243 的 ②④）。
       这一条在退场之前就已经收得只剩四组（时钟交给手机状态栏、16 段量表放不下——
       决策 215 的舍格在这一档走过一轮），剩下的读数各有去处、唯一的动作已随
       `ThemeToggle` 迁到设置落地页页头，于是**整条不再露出**：
         · 待处理 / 执行中 → 看板顶栏道具栏行的过滤槽徽章与「待处理 N」芯片；
         · token 总量 → 指标页的「到现在用掉 N 个 token」；
         · 深浅切换 → `#/settings` 页头（手机从底部「设置」页签一进就看得到）。
       `display:none` 而不是卸载：桌面档同一份 DOM 还要用（同铭牌行的处置，决策 242①），
       也让 `pixel-theme.spec` 断言的是「不露出」而不是「不存在」。
       钉底的东西随之少让 42px——那一层在 `app.css` 的 `--sbar-h` 账本里一并摘掉。 */
    .statusline {
      display: none;
    }
  }
</style>
