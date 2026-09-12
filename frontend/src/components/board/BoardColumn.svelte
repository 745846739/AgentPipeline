<script lang="ts">
  import type { AllowedAction, BranchCursor, TaskListItem } from '../../api/types';
  import type { BoardColumnDef } from '../../lib/pipeline';
  import { EMPTY_HINTS } from '../../lib/pipeline';
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
  /** 脊线三态（theme-3 §8）：pending 优先于 running，其余为无车。 */
  const spineState = $derived(hasPending ? 'pen' : hasRunning ? 'live' : '');

  /**
   * 段头站点字标（移动版）：live / pending 为 `◆`，整列完成 `●`，
   * 空列 `○`——与原型 `.spmk` 逐字一致。
   */
  const mark = $derived(
    hasPending || hasRunning
      ? '◆'
      : spineTasks.length > 0 && spineTasks.every((t) => t.status === 'done')
        ? '●'
        : '○',
  );

  /** 该列有任务但被当前过滤全部滤掉（移动版隐藏卡与空态，保留脊线/段头）。 */
  const hideCards = $derived(tasks.length === 0 && spineTasks.length > 0);
</script>

<section class="col {spineState} {hideCards ? 'hide-cards' : ''}">
  <div class="col-spine"><i class="spine-rule {spineState}"></i></div>
  <div class="col-main">
    <div class="col-head sec-head">
      <span class="spmk {spineState}">{mark}</span>
      <span class="col-name sec-name">{column.label}</span>
      <i class="col-rule sec-rule"></i>
      {#if tasks.length > 0}<span class="col-n sec-n col-n-desk">{tasks.length}</span>{/if}
      {#if spineTasks.length > 0}<span class="sec-n col-n-mob">{spineTasks.length}</span>{/if}
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
  .col {
    width: var(--rail-col-width);
    flex: none;
    border-right: 1px solid var(--pane);
    display: flex;
    flex-direction: column;
  }
  .col:last-child {
    border-right: 0;
  }
  /* 移动版专属装饰：桌面不参与布局（display:contents 让内容拍平进 .col 弹性列）。 */
  .col-spine,
  .col-rule,
  .spmk,
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
    gap: 8px;
    padding: 9px 12px;
    border-bottom: 1px solid var(--pane);
    background: var(--head-band);
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--text-3);
  }
  .col-n {
    color: var(--text-4);
    font-weight: 400;
    font-variant-numeric: tabular-nums;
  }
  .col-empty {
    padding: 14px 12px;
    color: var(--text-4);
    font-size: 11.5px;
  }

  /* ── 移动版：段落（脊线 + 段头字段条 + 电文行组，theme-3 §8） ── */
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
    /* .sec-head 提供字段条内边距/底色/下框线（app.css 共享原语）；
       显式覆盖桌面基础规则的同名属性，避免作用域高特异性胜出 */
    .col-head {
      position: relative;
      z-index: 1;
      justify-content: flex-start;
      gap: 8px;
      padding: 6px 6px 7px 12px;
      min-height: 32px;
      background: var(--head-band);
      border-bottom: 1px solid var(--pane);
      font-size: inherit;
      font-weight: inherit;
      letter-spacing: normal;
      text-transform: none;
      color: var(--text-3);
    }
    .spmk {
      display: block;
      position: absolute;
      left: -26px;
      top: 50%;
      transform: translateY(-50%);
      width: 26px;
      text-align: center;
      background: var(--mask-bg);
      font-size: 13px;
      line-height: 1;
      color: var(--text-3);
    }
    .spmk.pen {
      color: var(--pending);
      animation: breath 2.4s ease-in-out infinite;
    }
    .spmk.live {
      color: var(--text-hi);
    }
    .col-name {
      flex: none;
      white-space: nowrap;
    }
    .col.live .col-name {
      color: var(--text-hi);
    }
    .col.pen .col-name {
      color: var(--pending);
    }
    .col-rule {
      display: block;
    }
    .col-n-desk {
      display: none;
    }
    .col-n-mob {
      display: inline;
    }
    .col-empty {
      padding: 2px 0 6px;
      font-size: 12.5px;
      color: var(--text-4);
    }
    .col-empty::before {
      content: '-- 列空 -- ';
      color: var(--text-3);
    }
    .col.hide-cards .col-body {
      display: none;
    }
  }
</style>
