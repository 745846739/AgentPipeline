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
    if (isPending) return 'wait';
    if (cursor.status === 'waiting_join') return 'dim';
    if (cursor.status === 'archived') return 'dim';
    return 'run';
  });

  function handle() {
    onclick?.(cursor);
  }
</script>

<button
  type="button"
  class="pill {isPending ? 'pend' : ''} {selected ? 'selected' : ''}"
  onclick={handle}
  title="cursor_id: {cursor.cursor_id}"
>
  <b class="bl">[{kind === 'main' ? 'main' : kind}]</b>
  <span class="mono">{cursor.branch} · {cursor.node}</span>
  <span class="st {statusClass}">{statusText}</span>
</button>

<style>
  /* 分支不用色相，用 [dev]/[tst] 文字标签（决策 84）；状态与文字双编码。 */
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    align-self: flex-start;
    font-family: var(--font-code);
    font-size: 10.5px;
    padding: 2px 7px;
    border-radius: var(--r-pill);
    background: transparent;
    color: var(--text-2);
    border: 1px solid var(--hairline);
  }
  .pill:hover {
    border-color: var(--text-2);
    color: var(--text-hi);
  }
  .bl {
    font-weight: 600;
    font-size: 10px;
    letter-spacing: 0.04em;
    color: var(--text-3);
  }
  .pill.selected {
    border-color: var(--text-hi);
    color: var(--text-hi);
  }
  .pill.pend {
    border-color: var(--pending);
  }
  .st.run {
    color: var(--go);
  }
  .st.wait {
    color: var(--pending);
  }
  .st.stop {
    color: var(--stop);
  }
  .st.dim {
    color: var(--text-4);
  }
</style>
