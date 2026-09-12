<script lang="ts">
  import type { ParsedDiff } from '../../lib/diff';
  import DiffView from '../render/DiffView.svelte';
  import MarkdownView from '../render/MarkdownView.svelte';

  interface Props {
    diff: ParsedDiff | null;
    raw: string | null;
    /** review-report.md（agent 预审报告）。 */
    reviewReport?: string | null;
    /** 单元测试结果（test-report.md；review 在 test 之前，可能尚未生成）。 */
    unitTestReport?: string | null;
    stale?: boolean;
    busy?: boolean;
    error?: string | null;
    onsubmit?: (approved: boolean, comments?: string) => void;
    onreload?: () => void;
  }

  let {
    diff,
    raw,
    reviewReport = null,
    unitTestReport = null,
    stale = false,
    busy = false,
    error = null,
    onsubmit,
    onreload,
  }: Props = $props();

  let comments = $state('');
</script>

<div class="reviewform">
  <div class="title cond">人工评审（review_mode = human）</div>

  {#if stale}
    <div class="notice">变更 diff 正在重新生成，请稍候再评审。</div>
  {/if}
  {#if error}<div class="error">{error}</div>{/if}

  <section class="block">
    <h4 class="cond">变更 diff</h4>
    {#if onreload}
      <button type="button" class="btn quiet small" onclick={onreload}>刷新</button>
    {/if}
    {#if diff}
      <DiffView parsed={diff} {raw} />
    {:else}
      <div class="hint">review-diff.diff 尚未生成或不可读。</div>
    {/if}
  </section>

  <section class="block">
    <h4 class="cond">agent 预审报告</h4>
    {#if reviewReport}
      <MarkdownView source={reviewReport} />
    {:else}
      <div class="hint">评审报告尚未生成。</div>
    {/if}
  </section>

  <section class="block">
    <h4 class="cond">单元测试结果</h4>
    {#if unitTestReport}
      <MarkdownView source={unitTestReport} />
    {:else}
      <div class="hint">单元测试结果尚未生成（review 在 test 之前）。</div>
    {/if}
  </section>

  <label class="comment">
    <span class="hint">打回意见（可选）</span>
    <textarea class="input" rows="3" bind:value={comments} placeholder="打回时随流转原因带给 develop…"></textarea>
  </label>

  <div class="actions">
    <button
      type="button"
      class="btn"
      disabled={busy}
      onclick={() => onsubmit?.(false, comments.trim() || undefined)}
    >
      {#if busy}<span class="spin"></span>{/if}
      打回并附意见
    </button>
    <button type="button" class="btn solid" disabled={busy} onclick={() => onsubmit?.(true, undefined)}>
      {#if busy}<span class="spin"></span>{/if}
      通过
    </button>
  </div>
</div>

<style>
  .reviewform {
    max-width: 980px;
  }
  .title {
    font-size: 13px;
    color: var(--text-2);
    margin-bottom: 12px;
  }
  .block {
    margin-bottom: 18px;
  }
  .block h4 {
    font-size: 12px;
    color: var(--text-3);
    margin-bottom: 8px;
  }
  .comment {
    display: block;
    margin: 12px 0;
  }
  .comment .hint {
    display: block;
    margin-bottom: 4px;
  }
  .actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
  }
  .hint {
    color: var(--text-3);
    font-size: 12px;
  }
  .error {
    color: var(--stop);
    font-size: 12px;
    margin-bottom: 8px;
  }
  .notice {
    display: inline-block;
    font-size: 11px;
    color: var(--pending);
    border: 1px dashed var(--pending);
    padding: 2px 10px;
    margin-bottom: 10px;
  }
  .small {
    font-size: 11px;
    padding: 2px 8px;
    margin-bottom: 6px;
  }
</style>
