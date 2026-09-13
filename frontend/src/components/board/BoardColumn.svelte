<script lang="ts">
  import type { AllowedAction, BranchCursor, TaskListItem } from '../../api/types';
  import type { BoardColumnDef } from '../../lib/pipeline';
  import { COLUMN_SPRITES, EMPTY_HINTS, workerRhythm } from '../../lib/pipeline';
  import { WORKER_FRAMES } from '../../theme/contract';
  import Sprite from '../render/Sprite.svelte';
  import TaskCard from './TaskCard.svelte';

  interface Props {
    column: BoardColumnDef;
    tasks: TaskListItem[];
    /**
     * 该列未过滤任务（移动版脊线/段头状态与 hide-cards 判定用）。
     * 缺省退回 tasks，桌面版不读。
     */
    allTasks?: TaskListItem[];
    actionsFor?: (taskId: string) => AllowedAction[];
    cursorsFor?: (taskId: string) => BranchCursor[];
    actionBusy?: string | null;
    onopen?: (id: string) => void;
    onaction?: (taskId: string, action: AllowedAction, opts: { cursorId?: string; input?: string }) => void;
  }

  let {
    column,
    tasks,
    allTasks,
    actionsFor,
    cursorsFor,
    actionBusy = null,
    onopen,
    onaction,
  }: Props = $props();

  const spineTasks = $derived(allTasks ?? tasks);
  const hasPending = $derived(spineTasks.some((t) => t.status === 'pending'));
  const hasRunning = $derived(spineTasks.some((t) => t.status === 'running'));
  const hasFailed = $derived(
    spineTasks.some((t) => t.status === 'failed' || t.status === 'cancelled'),
  );
  const allDone = $derived(spineTasks.length > 0 && spineTasks.every((t) => t.status === 'done'));

  /**
   * 站段三态（优先序与契约 `STATE_STYLES` 一致）：急停琥珀 > 在跑绿 > 失败红 > 归档灰 > 空。
   */
  const colState = $derived(
    hasPending
      ? 'pen'
      : hasRunning
        ? 'live'
        : hasFailed
          ? 'fail'
          : allDone
            ? 'don'
            : 'idle',
  );

  /** 列头信号灯状态（契约 `StationState` 词表）。 */
  const stationState = $derived(
    hasPending ? 'warn' : hasRunning ? 'go' : hasFailed ? 'stop' : allDone ? 'done' : 'idle',
  );

  /** 挥锤小人节奏：run 快挥 / wait 慢挥 / idle 站立（帧切换为离散 opacity 翻转）。 */
  const rhythm = $derived(workerRhythm(stationState));

  /** 工位图标着色阶（与契约 `STATE_STYLES.lamp` 同源）：仅失败列换告警色，其余中性。 */
  function lampClass(state: string): string {
    return state === 'stop' ? 'fail' : '';
  }

  /** 该列有任务但被当前过滤全部滤掉（移动版隐藏卡与空态，保留脊线/段头）。 */
  const hideCards = $derived(tasks.length === 0 && spineTasks.length > 0);
</script>

<section class="col {colState} {hideCards ? 'hide-cards' : ''}">
  <div class="col-spine"><i class="spine-rule {colState}"></i></div>
  <div class="col-main">
    <div class="col-head sec-head">
      <span class="col-name sec-name">
        <i class="sp {lampClass(stationState)}"><Sprite name={COLUMN_SPRITES[column.key]} /></i
        >{column.label}
      </span>
      <span class="worker {rhythm}" aria-hidden="true">
        <svg
          class="f1"
          viewBox="0 0 8 8"
          width="16"
          height="16"
          shape-rendering="crispEdges"
          fill="currentColor"
        >
          {#each WORKER_FRAMES.raised as r, i (i)}
            <rect x={r.x} y={r.y} width={r.w} height={r.h} />
          {/each}
        </svg>
        <svg
          class="f2"
          viewBox="0 0 8 8"
          width="16"
          height="16"
          shape-rendering="crispEdges"
          fill="currentColor"
        >
          {#each WORKER_FRAMES.struck as r, i (i)}
            <rect x={r.x} y={r.y} width={r.w} height={r.h} />
          {/each}
        </svg>
      </span>
      <i class="col-rule sec-rule"></i>
      <span class="sec-n col-n col-n-desk">{tasks.length}</span>
    </div>

    <div class="col-body">
      {#if tasks.length === 0}
        <div class="col-empty">{EMPTY_HINTS[column.key]}</div>
      {:else}
        {#each tasks as task (task.id)}
          <TaskCard
            {task}
            actions={actionsFor?.(task.id) ?? []}
            cursors={cursorsFor?.(task.id) ?? []}
            {actionBusy}
            {onopen}
            onaction={(action, opts) => onaction?.(task.id, action, opts)}
          />
        {/each}
      {/if}
    </div>
  </div>
</section>

<style>
  /* 工位图标随列头状态取色（currentColor），形状保持 8×8 脆边 */
  .sp {
    display: inline-block;
    line-height: 0;
    margin-right: 6px;
    vertical-align: -3px;
    color: var(--text-3);
  }
  /* 已归档 / 失败列的工位图标用告警色（原型 `.col.arch .col-head .sp`） */
  .sp.fail {
    color: var(--stop);
  }

  /* 列头小人：run 快挥 0.6s / wait 慢挥 1.8s / idle 站立；双帧离散翻转 */
  .worker {
    position: relative;
    display: inline-block;
    width: 16px;
    height: 16px;
    margin-left: 8px;
    flex: none;
  }
  .worker svg {
    position: absolute;
    inset: 0;
  }
  .worker .f2 {
    opacity: 0;
  }
  .worker.run {
    color: var(--go);
  }
  .worker.wait {
    color: var(--pending);
  }
  .worker.idle {
    color: var(--text-4);
  }
  @keyframes wA {
    0%,
    50% {
      opacity: 1;
    }
    50.01%,
    100% {
      opacity: 0;
    }
  }
  @keyframes wB {
    0%,
    50% {
      opacity: 0;
    }
    50.01%,
    100% {
      opacity: 1;
    }
  }
  .worker.run .f1 {
    animation: wA 0.6s steps(2) infinite;
  }
  .worker.run .f2 {
    animation: wB 0.6s steps(2) infinite;
  }
  .worker.wait .f1 {
    animation: wA 1.8s steps(2) infinite;
  }
  .worker.wait .f2 {
    animation: wB 1.8s steps(2) infinite;
  }
  .col.don .worker {
    color: var(--done);
  }
  @media (prefers-reduced-motion: reduce) {
    .worker svg {
      animation: none !important;
    }
  }

  .col {
    width: var(--rail-col-width);
    flex: none;
    border-right: 2px solid var(--pane);
    display: flex;
    flex-direction: column;
  }
  .col:last-child {
    border-right: 0;
  }
  /* 移动版专属装饰：桌面不参与布局（display:contents 让内容拍平进 .col 弹性列）。 */
  .col-spine,
  .col-rule,
  .col-n-mob {
    display: none;
  }
  .col-main,
  .col-body {
    display: contents;
  }
  .col-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 6px;
    padding: 8px 12px;
    border-bottom: 2px solid var(--pane);
    background: var(--bg);
    font-size: 12px;
    letter-spacing: 0.08em;
    color: var(--text-3);
  }
  .col-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1;
  }
  /* 工位图标与名称同行，名称不换行 */
  .col-n {
    color: var(--text-4);
    font-variant-numeric: tabular-nums;
  }
  .col-empty {
    margin: 12px;
    padding: 10px 12px;
    border: 2px solid var(--pane);
    color: var(--text-4);
    font-size: 12px;
  }
  /* 待处理列：站点灯与名称转琥珀（全站唯一告警） */
  .col.pen .col-name {
    color: var(--pending);
  }
  .col.pen .sp {
    color: var(--pending);
  }
  .col.live .col-name {
    color: var(--text-hi);
  }

  /* ── 移动版：段落（脊线 + 段头字段条 + 货箱行组；完整转写见票 11） ── */
  @media (max-width: 479px) {
    .col {
      width: 100%;
      flex-direction: row;
      align-items: stretch;
      border-right: 0;
    }
    .col-spine {
      display: block;
      position: relative;
      flex: none;
      width: 26px;
    }
    .col-main {
      display: block;
      flex: 1;
      min-width: 0;
      padding-bottom: 10px;
    }
    .col-body {
      display: block;
    }
    .col-head {
      position: relative;
      z-index: 1;
      justify-content: flex-start;
      gap: 8px;
      padding: 6px 6px 7px 12px;
      min-height: 32px;
      background: var(--head-band);
      border-bottom: 2px solid var(--hairline);
      letter-spacing: normal;
      color: var(--text-3);
    }
    .col-name {
      flex: none;
      white-space: nowrap;
      letter-spacing: 0.08em;
    }
    .col-rule {
      display: block;
    }
    .col-n-desk {
      display: none;
    }
    .col-empty {
      margin: 10px 0 6px;
      padding: 0;
      border: 0;
      font-size: 12px;
      color: var(--text-4);
    }
    .col.hide-cards .col-body {
      display: none;
    }
  }
</style>
