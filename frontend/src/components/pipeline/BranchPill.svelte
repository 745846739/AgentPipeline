<script lang="ts">
  import type { BranchCursor } from '../../api/types';
  import { branchKind } from '../../lib/pipeline';

  interface Props {
    cursor: BranchCursor;
    /** 点击药丸（用于把 cursor_id 交给动作提交，决策 91）。 */
    onclick?: (cursor: BranchCursor) => void;
    selected?: boolean;
  }

  let { cursor, onclick, selected = false }: Props = $props();

  const kind = $derived(branchKind(cursor.branch));
  const isPending = $derived(cursor.status === 'pending');

  const statusText = $derived.by(() => {
    if (isPending) {
      const t = cursor.pending_reason?.type;
      if (t === 'merge_approval') return '⏸ 等待审批';
      if (t === 'human_review') return '⏸ 等待评审';
      return '⏸ 等待决定';
    }
    if (cursor.status === 'waiting_join') return '等待汇合';
    if (cursor.status === 'archived') return '已归档';
    if (cursor.status === 'active') {
      return cursor.node === 'execute' ? '● 执行中' : '● 可继续';
    }
    return cursor.status;
  });

  const statusClass = $derived.by(() => {
    if (isPending) return kind === 'main' ? 'st-warn' : 'st-pend';
    if (cursor.status === 'waiting_join') return 'st-dim';
    if (cursor.status === 'archived') return 'st-dim';
    return 'st-go';
  });

  function handle() {
    onclick?.(cursor);
  }
</script>

<button
  type="button"
  class="pill {kind} {isPending ? 'pend' : ''} {selected ? 'selected' : ''}"
  onclick={handle}
  title="cursor_id: {cursor.cursor_id}"
>
  <span class="mono">{cursor.branch} · {cursor.node}</span>
  <span class={statusClass}>{statusText}</span>
</button>

<style>
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    align-self: flex-start;
    font-family: var(--font-mono);
    font-size: 10.5px;
    padding: 2px 7px;
    border-radius: var(--r-pill);
    background: var(--ink-700);
    color: var(--text-2);
    border: 1px solid transparent;
  }
  .pill.dev {
    color: var(--branch-dev);
    background: rgba(76, 195, 224, 0.08);
  }
  .pill.test {
    color: var(--branch-test);
    background: rgba(167, 139, 250, 0.08);
  }
  .pill.dev.pend {
    background: rgba(76, 195, 224, 0.14);
  }
  .pill.test.pend {
    background: rgba(167, 139, 250, 0.14);
  }
  .pill.selected {
    border-color: currentColor;
  }
  .st-go {
    color: var(--signal-go);
  }
  .st-warn,
  .pill.warn {
    color: var(--signal-caution);
  }
  .st-pend {
    color: var(--signal-caution);
  }
  .st-dim {
    color: var(--text-3);
  }
</style>
