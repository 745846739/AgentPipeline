<script lang="ts">
  import type { Transition } from '../../api/types';
  import { TRIGGER_LABELS, branchKind } from '../../lib/pipeline';
  import { formatClock } from '../../lib/format';

  interface Props {
    transitions: Transition[];
    /** 当前游标（用于标注 ← 当前）。 */
    currentBranch?: string;
    currentStage?: string;
    currentNode?: string;
  }

  let { transitions, currentBranch, currentStage, currentNode }: Props = $props();

  function triggerClass(trigger: string): string {
    if (trigger === 'retry' || trigger === 'node_retry') return 'stop';
    if (trigger === 'kickback') return 'warn';
    return '';
  }

  function isCurrent(t: Transition): boolean {
    return t.branch === currentBranch && t.to_stage === currentStage && t.to_node === currentNode;
  }
</script>

{#if transitions.length === 0}
  <div class="empty">还没有流转记录。</div>
{:else}
  <div class="tline">
    {#each transitions as t (t.id)}
      <div class="trow">
        <span class="t">{formatClock(t.created_at)}</span>
        <span class="tr {triggerClass(t.trigger)}">{TRIGGER_LABELS[t.trigger] ?? t.trigger}</span>
        <span class="to">
          {t.to_stage}.{t.to_node}
          {#if branchKind(t.branch) !== 'main'}
            <span class="bbadge {branchKind(t.branch)}">{branchKind(t.branch) === 'dev' ? '[dev]' : '[tst]'}</span>
          {/if}
          {#if isCurrent(t)}<span class="cur">← 当前</span>{/if}
          {#if t.reason}<span class="why">（{t.reason}）</span>{/if}
        </span>
      </div>
    {/each}
  </div>
{/if}

<style>
  .tline {
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--text-2);
    max-width: 820px;
  }
  .trow {
    display: flex;
    gap: 12px;
    padding: 3px 8px;
  }
  .trow:hover {
    background: var(--hover-bg);
  }
  .t {
    color: var(--text-4);
    flex: none;
    width: 66px;
  }
  .tr {
    flex: none;
    width: 96px;
    color: var(--text-3);
  }
  .tr.warn {
    color: var(--pending);
  }
  .tr.stop {
    color: var(--stop);
  }
  .to {
    flex: 1;
    color: var(--text-2);
    word-break: break-word;
  }
  .why {
    color: var(--text-4);
  }
  .cur {
    color: var(--go);
  }
  /* 分支徽标：2px 描边像素块；分支身份靠色相（蓝 / 紫）+ 文字双编码（决策 84 / 169） */
  .bbadge {
    font-size: 12px;
    letter-spacing: 0.06em;
    padding: 0 5px;
    border: 2px solid currentColor;
  }
  .bbadge.dev {
    color: var(--branch-dev);
  }
  .bbadge.test {
    color: var(--branch-tst);
  }
  .empty {
    color: var(--text-3);
    font-size: 12px;
  }

  /* ── 移动版（<480px）：时间线折行为块（theme-3 §8 原型 .trow） ── */
  @media (max-width: 479px) {
    .tline {
      max-width: none;
      font-size: 12px;
    }
    .trow {
      display: block;
      padding: 7px 0;
      border-bottom: 2px solid var(--hairline);
    }
    .trow:last-child {
      border-bottom: 0;
    }
    .t {
      width: auto;
      font-size: 12px;
    }
    .tr {
      display: inline;
      width: auto;
      margin-left: 8px;
      font-size: 12px;
    }
    .to {
      display: block;
      margin-top: 1px;
      word-break: break-all;
    }
    .why {
      display: block;
      margin-top: 2px;
      font-size: 12px;
    }
  }
</style>
