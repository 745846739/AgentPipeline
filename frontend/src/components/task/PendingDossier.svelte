<script lang="ts">
  import type { AllowedAction, BranchCursor, PendingKind, PendingReason } from '../../api/types';
  import type { ParsedDiff } from '../../lib/diff';
  import { pendingLabel } from '../../lib/pipeline';
  import PendingActions from '../board/PendingActions.svelte';
  import DiffReviewPanel from './DiffReviewPanel.svelte';
  import ReviewForm from './ReviewForm.svelte';

  interface Props {
    reason: PendingReason;
    cursors: BranchCursor[];
    actions: AllowedAction[];
    busy?: boolean;
    onaction?: (action: AllowedAction, opts: { cursorId?: string; input?: string }) => void;
    /** 触发 pending 的节点会话直达（§12.4.3 联动）。 */
    ongotoconversation?: (stage: string, node: string) => void;
    onopenfiles?: () => void;
    /** merge_approval 内嵌 Diff。 */
    diff?: ParsedDiff | null;
    rawDiff?: string | null;
    diffStale?: boolean;
    diffError?: string | null;
    diffLoading?: boolean;
    onreloaddiff?: () => void;
    /** human_review 三件套。 */
    reviewReport?: string | null;
    unitTestReport?: string | null;
    onsubmitreview?: (approved: boolean, comments?: string) => void;
  }

  let {
    reason,
    cursors,
    actions,
    busy = false,
    onaction,
    ongotoconversation,
    onopenfiles,
    diff = null,
    rawDiff = null,
    diffStale = false,
    diffError = null,
    diffLoading = false,
    onreloaddiff,
    reviewReport = null,
    unitTestReport = null,
    onsubmitreview,
  }: Props = $props();

  const pendingType = $derived<PendingKind>(reason.type);
  // 触发 pending 的游标（用于会话直达）
  const triggerCursor = $derived(
    cursors.find((c) => c.status === 'pending' && c.pending_reason?.type === reason.type) ??
      cursors.find((c) => c.status === 'pending'),
  );
  const conflicts = $derived(reason.context?.conflict_task_ids ?? []);
</script>

<aside class="dossier" aria-label="待办">
  <div class="dtag cond">等待你处理 · {pendingLabel(reason)}</div>
  <div class="msg">{reason.message}</div>

  {#if conflicts.length > 0}
    <div class="ctx">
      冲突任务：{conflicts.join('、')}
    </div>
  {/if}
  {#if reason.context?.kind}
    <div class="ctx mono dim">kind = {reason.context.kind}</div>
  {/if}

  {#if pendingType === 'merge_approval'}
    <DiffReviewPanel
      {diff}
      raw={rawDiff}
      stale={diffStale}
      error={diffError}
      loading={diffLoading}
      {actions}
      {cursors}
      {busy}
      {onaction}
      onreload={onreloaddiff}
    />
  {:else if pendingType === 'human_review'}
    <ReviewForm
      {diff}
      raw={rawDiff}
      {reviewReport}
      {unitTestReport}
      stale={diffStale}
      {busy}
      error={diffError}
      onsubmit={onsubmitreview}
      onreload={onreloaddiff}
    />
  {:else}
    <PendingActions
      {actions}
      {cursors}
      {pendingType}
      {onaction}
      disabled={busy}
      isBusy={() => busy}
    />
  {/if}

  {#if triggerCursor}
    <div class="ctx trigger">
      触发节点：
      <button type="button" class="linklike" onclick={() => ongotoconversation?.(triggerCursor.stage, triggerCursor.node)}>
        {triggerCursor.stage}.{triggerCursor.node} ▸
      </button>
    </div>
  {/if}

  <div class="ctx">
    <button type="button" class="linklike" onclick={() => onopenfiles?.()}>查看产出文件 ▸</button>
  </div>
</aside>

<style>
  .dossier {
    align-self: start;
    position: sticky;
    top: 64px;
    background: var(--ink-800);
    border: 1px solid var(--signal-caution);
    border-radius: var(--r-panel);
    padding: 14px 16px;
    max-height: calc(100vh - 84px);
    overflow: auto;
  }
  .dtag {
    font-size: 11px;
    color: var(--signal-caution);
    margin-bottom: 8px;
  }
  .msg {
    font-size: 12.5px;
    color: var(--text-hi);
    margin-bottom: 10px;
  }
  .ctx {
    font-size: 12px;
    color: var(--text-2);
    margin-bottom: 6px;
  }
  .dim {
    color: var(--text-3);
    font-size: 11px;
  }
  .trigger {
    margin-top: 10px;
  }
  .linklike {
    color: var(--branch-dev);
    font-size: 12px;
    padding: 0;
    text-align: left;
  }
  .linklike:hover {
    text-decoration: underline;
  }
</style>
