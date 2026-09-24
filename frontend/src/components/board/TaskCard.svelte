<script lang="ts">
  import type { AllowedAction, BranchCursor, TaskListItem } from '../../api/types';
  import { actionKey } from '../../lib/actions';
  import {
    branchShort,
    crateState,
    crateTone,
    formatDuration,
    formatTokens,
    miniRailState,
    pendingLabel,
    stalledHours,
    taskDuration,
  } from '../../lib/pipeline';
  import PipelineRail from '../pipeline/PipelineRail.svelte';
  import BranchPill from '../pipeline/BranchPill.svelte';
  import Gauge from '../render/Gauge.svelte';
  import PendingActions from './PendingActions.svelte';
  import StalledBadge from './StalledBadge.svelte';
  import BossBar from './BossBar.svelte';

  interface Props {
    task: TaskListItem;
    actions?: AllowedAction[];
    cursors?: BranchCursor[];
    actionBusy?: string | null;
    onopen?: (id: string) => void;
    onaction?: (action: AllowedAction, opts: { cursorId?: string; input?: string }) => void;
  }

  let { task, actions = [], cursors = [], actionBusy = null, onopen, onaction }: Props = $props();

  const dots = $derived(miniRailState(task));
  const visibleCursors = $derived(
    (cursors.length ? cursors : task.branches).filter((c) => c.status !== 'archived'),
  );
  const state = $derived(crateState(task));
  const tone = $derived(crateTone(state));
  const isPending = $derived(task.status === 'pending');
  const isTerminal = $derived(
    task.status === 'done' || task.status === 'failed' || task.status === 'cancelled',
  );
  const durationMs = $derived(taskDuration(task));
  const hours = $derived(stalledHours(task));
  const reason = $derived(
    task.pending_reason ?? visibleCursors.find((c) => c.pending_reason)?.pending_reason ?? null,
  );

  /** 该任务是否已耗尽重试（后端权威信号：pending 类型 retry_exhausted）。 */
  const exhausted = $derived(reason?.type === 'retry_exhausted');
  /**
   * boss 尝试条的出现条件：**运行中**（尝试数在跑时有意义）**或已耗尽重试**
   * （后者状态是 pending——「最后一次机会也没了」正是最该看到这条红的地方）。
   * 只按 `running` 会漏掉它，让「整条转红」永远不可达（票 05 的招牌行为之一）。
   */
  const showBoss = $derived(task.status === 'running' || exhausted);
  const attempts = $derived(
    Math.max(task.validate_attempts, ...visibleCursors.map((c) => c.validate_attempts), 0),
  );

  function isBusy(action: AllowedAction, cursorId?: string): boolean {
    // 只认四段式（票 05）：此前这里手工桥着两种拼法（store 的两段式 + actionKey 的
    // 截断版），那正是「同一个忙有两把尺子」的来源——现在生产者与消费者同一把。
    return actionBusy === actionKey(action, cursorId);
  }
</script>

<!-- 货箱（决策 169 / theme-6-pixel.md §3）：2px 描边盒 + dither 顶盖带 + 4px 硬投影。
     一张货箱 = 一枚灯 + 一种描边色；灯是实心像素方块，绝不作大面积底色。 -->
<article class="card {state}">
  <i class="lamp {state}" aria-hidden="true"></i>
  <a
    class="card-link"
    href={`#/task/${task.id}`}
    aria-label={`打开任务：${task.title}`}
    onclick={(e) => {
      if (onopen) {
        e.preventDefault();
        onopen(task.id);
      }
    }}><span class="visually-hidden">打开任务 {task.title}</span></a
  >
  <div class="card-top">
    <span class="card-title">{task.title}</span>
    {#if task.stalled}
      <StalledBadge hours={hours} />
    {:else}
      <span class="dur">{isTerminal ? '—' : formatDuration(durationMs)}</span>
    {/if}
  </div>

  <PipelineRail variant="mini" {dots} ariaLabel="任务迷你轨道" />

  {#if visibleCursors.length > 0}
    <div class="pillrow">
      {#each visibleCursors as cursor (cursor.cursor_id)}
        <BranchPill {cursor} />
      {/each}
    </div>
  {/if}

  {#if isPending && reason}
    <!-- 急停对话框（决策 169 / theme-6-pixel.md §3）：双线框 + 压在框沿上的琥珀名牌
         tab + ▼ 闪烁光标。`.reason` 只作层叠占位，语义与断言口不变。 -->
    <div class="reason dialog warn">
      <div class="dname">{pendingLabel(reason)}</div>
      <span class="dtxt">{reason.message}</span>
    </div>
  {/if}

  {#if isPending && actions.length > 0}
    <!-- 阻止点击冒泡到整卡导航 -->
    <div class="actions" role="presentation" onclick={(e) => e.stopPropagation()} onkeydown={(e) => e.stopPropagation()}>
      <PendingActions
        {actions}
        cursors={visibleCursors}
        pendingType={reason?.type}
        onaction={onaction}
        isBusy={isBusy}
      />
    </div>
  {:else if isPending}
    <div class="ctxlink"><span>前往详情处理 ▸</span></div>
  {/if}

  {#if showBoss}
    <BossBar used={attempts} {exhausted} />
  {/if}

  {#if task.status === 'waiting'}
    <div class="tagline">等待依赖完成</div>
  {:else if task.status === 'queued'}
    <div class="tagline">排队等待并发名额</div>
  {/if}

  <div class="meta">
    <Gauge tokens={task.total_tokens} {tone} />
    <span><b>{formatTokens(task.total_tokens)}</b> tok</span>
    <span><b>{task.total_calls}</b> 次调用</span>
    {#if task.branch_name}<span>{task.branch_name}</span>{/if}
    {#if visibleCursors.some((c) => c.branch !== 'main')}
      <span>{visibleCursors.map((c) => branchShort(c.branch)).join('/')}</span>
    {/if}
  </div>
</article>

<style>
  /* 货箱：2px 描边 + 4px 硬投影；顶盖 6px dither 带是"这东西是实体"的材质提示 */
  .card {
    position: relative;
    border: 2px solid var(--pane);
    background: var(--panel);
    box-shadow: 4px 4px 0 var(--ink);
    margin: 12px;
    padding: 0 0 10px;
    cursor: pointer;
  }
  .card::before {
    content: '';
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    height: 6px;
    background-image: conic-gradient(
      var(--wash) 25%,
      transparent 0 50%,
      var(--wash) 0 75%,
      transparent 0
    );
    background-size: 4px 4px;
  }
  .card > * {
    position: relative;
  }
  .card > .card-link {
    position: absolute;
  }
  .card > :first-child:not(.card-link):not(.lamp) {
    margin-top: 8px;
  }
  .card:hover {
    border-color: var(--text-2);
  }
  /* 一枚灯 + 一种描边色：pending 琥珀 / failed 红 / queued·waiting 无灯 */
  .card.pending {
    border-color: var(--pending);
    box-shadow:
      inset 4px 0 0 var(--pending),
      4px 4px 0 var(--ink);
  }
  .card.failed {
    border-color: var(--stop);
    box-shadow:
      inset 4px 0 0 var(--stop),
      4px 4px 0 var(--ink);
  }
  .card.done {
    box-shadow: none;
  }
  .card.done:hover {
    border-color: var(--text-4);
  }
  /* 灯：实心像素方块，压在顶盖带右端 */
  .lamp {
    position: absolute;
    top: 14px;
    right: 10px;
    width: 8px;
    height: 8px;
    background: var(--done);
  }
  .lamp.running {
    background: var(--go);
  }
  .lamp.pending {
    background: var(--pending);
  }
  .lamp.failed {
    background: var(--stop);
  }
  .lamp.queued,
  .lamp.waiting {
    background: transparent;
    border: 2px solid var(--text-4);
  }
  .card-link {
    inset: 0;
    z-index: 1;
  }
  .card-link:hover {
    text-decoration: none;
  }
  .card > :not(.card-link) {
    z-index: 0;
  }
  .card-top {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 8px;
    padding: 14px 12px 0;
  }
  .card-title {
    min-width: 0;
    font-size: 12px;
    line-height: 1.5;
    color: var(--text-hi);
  }
  /* 急停货箱：标题前缀 ?!（与琥珀左缘条对应的双编码） */
  .card.pending .card-title::before {
    content: '?! ';
    color: var(--pending);
  }
  .card.done .card-title {
    color: var(--text-2);
  }
  .dur {
    flex: none;
    font-size: 12px;
    color: var(--text-3);
    font-variant-numeric: tabular-nums;
  }
  .pillrow {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin: 8px 12px;
  }
  /* 看板货箱上的急停对话框（原型 `.dialog.warn`）：琥珀外框 + bg 空隙 + pane 内框，
     名牌 tab 压在框沿上，右下角 ▼ 闪烁光标。 */
  .reason {
    position: relative;
    margin: 18px 12px 8px;
    padding: 8px 10px;
    background: var(--bg);
    border: 2px solid var(--pending);
    box-shadow:
      inset 0 0 0 2px var(--bg),
      inset 0 0 0 4px var(--pane);
    font-size: 12px;
    line-height: 1.6;
    color: var(--text-2);
  }
  .reason::after {
    content: '▼';
    position: absolute;
    right: 6px;
    bottom: 0;
    color: var(--pending);
    font-size: 12px;
    line-height: 1;
    animation: blink 1s steps(2) infinite;
  }
  .dname {
    position: absolute;
    top: -16px;
    left: 6px;
    background: var(--bg);
    border: 2px solid var(--pending);
    color: var(--pending);
    padding: 0 8px;
    line-height: 1.5;
    white-space: nowrap;
  }
  .dtxt {
    display: block;
    color: var(--text-2);
    overflow-wrap: anywhere;
  }
  .ctxlink {
    margin: 2px 12px 4px;
    font-size: 12px;
    color: var(--text-3);
  }
  /* 必须压过整卡导航链接（`.card-link`，z-index 1）。上面 `.card > :not(.card-link)`
     把卡片子元素统一归零，那条规则特异性 (0,2,0) 高于 `.actions` (0,1,0)，所以这里
     也用 `.card > .actions` 取同等特异性、靠源码顺序取胜。主流程票 09 实测：不修则
     卡片动作按钮被链接覆盖，点击只跳详情（Playwright 报 element intercepts pointer events），
     即「看板卡上的动作按钮点不动」这个用户可见缺陷。 */
  .card > .actions {
    margin: 8px 12px 0;
    position: relative;
    z-index: 2;
  }
  .tagline {
    padding: 0 12px;
    margin-top: 6px;
    font-size: 12px;
    color: var(--text-3);
  }
  .meta {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
    margin: 8px 12px 10px;
    font-size: 12px;
    color: var(--text-3);
    font-variant-numeric: tabular-nums;
  }
  .meta b {
    color: var(--text-hi);
  }

  /* ── 移动版：货箱行组（完整转写见票 11） ── */
  @media (max-width: 479px) {
    .card {
      margin: 0 0 10px;
      padding-bottom: 11px;
    }
    .card-title {
      font-size: 12px;
    }
    .pillrow :global(.pill) {
      border: 0;
      padding: 0;
      gap: 6px;
      align-items: baseline;
    }
    .meta {
      font-size: 12px;
      margin: 8px 12px 0;
    }
  }
</style>
