<script lang="ts">
  import type { AllowedAction, BranchCursor, PendingKind } from '../../api/types';
  import type { ParsedDiff } from '../../lib/diff';
  import DiffView from '../render/DiffView.svelte';

  interface Props {
    diff: ParsedDiff | null;
    raw: string | null;
    stale?: boolean;
    error?: string | null;
    actions: AllowedAction[];
    cursors?: BranchCursor[];
    pendingType?: PendingKind;
    /** 加载中（首次拉取 diff）。 */
    loading?: boolean;
    busy?: boolean;
    onaction?: (action: AllowedAction, opts: { cursorId?: string; input?: string }) => void;
    onreload?: () => void;
  }

  let {
    diff,
    raw,
    stale = false,
    error = null,
    actions,
    cursors = [],
    loading = false,
    busy = false,
    onaction,
    onreload,
  }: Props = $props();

  // merge 审批动作仅 approve / return（决策 23：**没有"拒绝"**）
  const mergeActions = $derived(actions.filter((a) => a.action === 'approve' || a.action === 'return'));
  const returnChanges = $derived(mergeActions.filter((a) => a.action === 'return'));
  const approve = $derived(mergeActions.filter((a) => a.action === 'approve'));
  const stats = $derived(diff?.stats ?? null);

  function cursorIdFor(action: AllowedAction): string | undefined {
    return action.cursor_id ?? cursors[0]?.cursor_id;
  }
</script>

<div class="diffpanel">
  <div class="diffhead">
    {#if stats}
      <span class="big mono">
        <span class="a">+{stats.insertions}</span>
        <span class="d">−{stats.deletions}</span>
        · {stats.files_changed} 个文件
      </span>
    {:else}
      <span class="big mono dim">— diff 统计不可用</span>
    {/if}
    {#if onreload}
      <button type="button" class="btn quiet" onclick={onreload}>重新加载 diff</button>
    {/if}
  </div>

  {#if stale}
    <div class="notice">基准已前移，diff 重新生成中；合入审批已由后端重置。</div>
  {/if}
  {#if error}
    <div class="error">{error}</div>
  {/if}

  {#if stats && stats.file_details.length > 0}
    <div class="breakdown mono">
      {#each stats.file_details as f (f.path)}
        <span class="fdetail">
          <span class="path">{f.path}</span>
          <span class="a">+{f.additions}</span>
          <span class="d">−{f.deletions}</span>
        </span>
      {/each}
    </div>
  {/if}

  {#if loading}
    <div class="hint">正在加载 diff…</div>
  {:else}
    <DiffView parsed={diff} {raw} />
  {/if}

  <div class="actions">
    {#each returnChanges as action (action.action + (action.cursor_id ?? ''))}
      <button
        type="button"
        class="btn"
        disabled={busy}
        onclick={() => onaction?.(action, { cursorId: cursorIdFor(action) })}
      >
        {action.label}
      </button>
    {/each}
    {#each approve as action (action.action + (action.cursor_id ?? ''))}
      <button
        type="button"
        class="btn solid"
        disabled={busy}
        onclick={() => onaction?.(action, { cursorId: cursorIdFor(action) })}
      >
        {#if busy}<span class="spin"></span>{/if}
        {action.label}
      </button>
    {/each}
    {#if mergeActions.length === 0}
      <span class="hint">当前没有可用的审批动作。</span>
    {/if}
  </div>
</div>

<style>
  .diffpanel {
    max-width: 980px;
  }
  .diffhead {
    display: flex;
    gap: 16px;
    align-items: baseline;
    margin-bottom: 10px;
    flex-wrap: wrap;
  }
  .diffhead .big {
    font-size: 13px;
  }
  .diffhead .a {
    color: var(--diff-add);
  }
  .diffhead .d {
    color: var(--diff-del);
  }
  .dim {
    color: var(--text-3);
  }
  .notice {
    display: inline-block;
    font-size: 12px;
    color: var(--signal-caution);
    border: 1px dashed var(--signal-caution);
    border-radius: var(--r-pill);
    padding: 3px 10px;
    margin-bottom: 10px;
  }
  .error {
    color: var(--signal-stop);
    font-size: 12px;
    margin-bottom: 8px;
  }
  .breakdown {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 14px;
    font-size: 10.5px;
    color: var(--text-3);
    margin-bottom: 10px;
  }
  .fdetail {
    display: inline-flex;
    gap: 6px;
  }
  .path {
    color: var(--text-2);
  }
  .breakdown .a {
    color: var(--diff-add);
  }
  .breakdown .d {
    color: var(--diff-del);
  }
  .actions {
    display: flex;
    gap: 8px;
    margin-top: 14px;
  }
  .hint {
    color: var(--text-3);
    font-size: 12px;
  }
</style>
