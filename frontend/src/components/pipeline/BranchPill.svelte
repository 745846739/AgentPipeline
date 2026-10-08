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

  /** 分支徽章文字（与色相双编码，决策 169）：main 中性，dev / tst 带分支身份。 */
  const branchBadge = $derived(kind === 'main' ? 'main' : `[${kind}]`);

  const statusText = $derived.by(() => {
    if (isPending) return '[WAIT]';
    if (cursor.status === 'active') return '[RUN]';
    if (cursor.status === 'waiting_join') return '等待汇合';
    if (cursor.status === 'archived') return '已归档';
    return cursor.status;
  });

  /** 悬停说明（文字标记之外的可读语义，不占视觉层级）。 */
  const statusHint = $derived.by(() => {
    if (isPending) {
      const t = cursor.pending_reason?.type;
      if (t === 'merge_approval') return '等待审批';
      if (t === 'human_review') return '等待评审';
      return '等待决定';
    }
    if (cursor.status === 'waiting_join') return '等待并行分支汇合';
    if (cursor.status === 'archived') return '游标已归档';
    if (cursor.status === 'active') {
      return cursor.node === 'execute' ? '执行中' : '可继续';
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
  class="pill {kind} {isPending ? 'pend' : ''} {selected ? 'selected' : ''}"
  onclick={handle}
  title="{statusHint} · cursor_id {cursor.cursor_id}"
>
  <b class="bl">{branchBadge}</b>
  <span class="mono">{cursor.branch} · {cursor.node}</span>
  <span class="st {statusClass}">{statusText}</span>
</button>

<style>
  /* 分支身份 = 徽章色相（蓝 dev / 紫 tst，决策 169 对决策 84 的手段变更）
     + 文字双编码（[dev] / [tst] 仍在），色相不单独承载语义。 */
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    align-self: flex-start;
    font-family: var(--font-code);
    font-size: 12px;
    padding: 2px 7px;
    border-radius: var(--r-pill);
    background: transparent;
    color: var(--text-2);
    border: 2px solid var(--hairline);
  }
  .pill:hover {
    border-color: var(--text-2);
    color: var(--text-hi);
  }
  .bl {
    font-size: 12px;
    letter-spacing: 0.04em;
    color: var(--text-3);
  }
  /* 徽章色相：仅开发设计 / 测试设计两条分支（main 保持中性） */
  .pill.dev .bl {
    color: var(--branch-dev);
  }
  .pill.test .bl {
    color: var(--branch-tst);
  }
  .pill.pend.dev {
    border-color: var(--branch-dev);
  }
  .pill.pend.test {
    border-color: var(--branch-tst);
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
