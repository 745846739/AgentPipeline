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
            <span class="bbadge {branchKind(t.branch)}">∥ {branchKind(t.branch)}</span>
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
    font-size: 11.5px;
    color: var(--text-2);
    max-width: 760px;
  }
  .trow {
    display: flex;
    gap: 14px;
    padding: 5px 8px;
    border-radius: var(--r-pill);
  }
  .trow:hover {
    background: var(--ink-800);
  }
  .t {
    color: var(--text-3);
    flex: none;
    width: 64px;
  }
  .tr {
    flex: none;
    width: 88px;
    color: var(--text-3);
  }
  .tr.warn {
    color: var(--signal-caution);
  }
  .tr.stop {
    color: var(--signal-stop);
  }
  .to {
    flex: 1;
    color: var(--text-2);
    word-break: break-word;
  }
  .why {
    color: var(--text-3);
  }
  .cur {
    color: var(--signal-go);
  }
  .bbadge {
    font-family: var(--font-cond);
    font-size: 10px;
    font-weight: 600;
    padding: 0 5px;
    border-radius: var(--r-pill);
  }
  .bbadge.dev {
    color: var(--branch-dev);
    background: rgba(76, 195, 224, 0.1);
  }
  .bbadge.test {
    color: var(--branch-test);
    background: rgba(167, 139, 250, 0.1);
  }
  .empty {
    color: var(--text-3);
    font-size: 12px;
  }
</style>
