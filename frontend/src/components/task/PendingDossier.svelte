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
    /** 窄屏（<480px）：dossier 转固定底部动作坞（theme-3 §8 转写 3）。 */
    dock?: boolean;
    /** dock 实际高度回传：详情内容据此留出底边距，避免被固定坞遮住。 */
    ondockheight?: (height: number) => void;
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
    dock = false,
    ondockheight,
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

  /** dock 模式：量取固定动作坞高度，供详情内容留出底边距（theme-3 §8 转写 3）。 */
  let dockH = $state(0);
  $effect(() => {
    if (dock) ondockheight?.(dockH);
  });
</script>

{#snippet infoBlock()}
  <div class="msg">{reason.message}</div>

  {#if conflicts.length > 0}
    <div class="ctx">
      冲突任务：<b>{conflicts.join('、')}</b>
    </div>
  {/if}
  {#if reason.context?.kind}
    <div class="ctx mono dim">kind = {reason.context.kind}</div>
  {/if}
{/snippet}

{#if dock}
  <aside class="dock" aria-label="待办" bind:clientHeight={dockH}>
    <div class="dock-tag">⏸ 等你拍板 · {pendingLabel(reason)}</div>
    {@render infoBlock()}

    {#if pendingType === 'merge_approval'}
      <DiffReviewPanel
        actionsOnly
        {diff}
        raw={rawDiff}
        stale={diffStale}
        error={diffError}
        loading={diffLoading}
        {actions}
        {cursors}
        {busy}
        {onaction}
      />
    {:else if pendingType === 'human_review'}
      <ReviewForm
        actionsOnly
        {diff}
        raw={rawDiff}
        {reviewReport}
        {unitTestReport}
        stale={diffStale}
        {busy}
        error={diffError}
        onsubmit={onsubmitreview}
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
{:else}
  <aside class="dossier" aria-label="待办">
    <div class="dtag">⏸ 等你拍板 · {pendingLabel(reason)}</div>
    {@render infoBlock()}

    {#if pendingType === 'merge_approval'}
      <div class="grp">恢复动作</div>
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
      <div class="grp">人工评审</div>
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
      <div class="grp">恢复动作</div>
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
{/if}

<style>
  .dossier {
    grid-column: 2;
    grid-row: 1 / span 3;
    align-self: start;
    position: sticky;
    top: 56px;
    background: var(--panel);
    border: 1px solid var(--pane);
    padding: 13px 15px;
    max-height: calc(100vh - 84px);
    overflow: auto;
  }
  .dtag {
    font-size: 11px;
    font-weight: 600;
    color: var(--pending);
    margin-bottom: 8px;
  }
  .msg {
    font-size: 11.5px;
    color: var(--text-hi);
    margin-bottom: 9px;
  }
  .ctx {
    font-size: 11px;
    color: var(--text-2);
    margin-bottom: 6px;
    line-height: 1.8;
  }
  .ctx b {
    color: var(--go);
    font-weight: 500;
  }
  .dim {
    color: var(--text-4);
  }
  .trigger {
    margin-top: 10px;
  }
  .grp {
    font-size: 10px;
    color: var(--text-4);
    letter-spacing: 0.08em;
    margin: 12px 0 7px;
    text-transform: uppercase;
  }
  .linklike {
    color: var(--text-hi);
    font-size: 11px;
    padding: 0;
    text-align: left;
    text-decoration: underline;
    text-underline-offset: 3px;
  }
  .dossier :global(.btn) {
    display: block;
    width: 100%;
    text-align: left;
    margin-bottom: 6px;
  }
  .dossier :global(.actions) {
    flex-direction: column;
    align-items: stretch;
    justify-content: flex-start;
  }

  /* ── 移动版底部动作坞（<480px，theme-3 §8 转写 3）：dossier 内容进坞 ── */
  .dock {
    max-height: 72vh;
    overflow: auto;
  }
  .dock .dock-tag {
    font-size: 12.5px;
  }
  .dock .msg {
    font-size: 13px;
    color: var(--text-hi);
    line-height: 1.7;
    margin-bottom: 7px;
  }
  .dock .ctx {
    margin: 7px 0 0;
    font-size: 12.5px;
    color: var(--text-2);
    line-height: 1.85;
  }
  .dock .ctx b {
    color: var(--go);
    font-weight: 500;
  }
  .dock .dim {
    color: var(--text-4);
  }
  .dock .linklike {
    color: var(--text-hi);
    font-size: 12.5px;
    padding: 0;
    text-align: left;
    text-decoration: underline;
    text-underline-offset: 3px;
  }
  /* 坞内动作可点目标 ≥48px（§8 触控） */
  .dock :global(.btn) {
    min-height: 48px;
  }
</style>
