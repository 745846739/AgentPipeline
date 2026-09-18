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
    /** 窄屏底部动作坞复用：只渲染意见输入 + 动作（§5 转写 3）。 */
    actionsOnly?: boolean;
    /**
     * 用户已经停在主区 Diff 页签（票 08，与 `DiffReviewPanel` 同一口径）→ 不渲染第二份 diff，
     * 报告与动作照旧在：同一屏两份同样的 diff 会让「哪个是真的」变成问题。
     */
    diffInPane?: boolean;
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
    actionsOnly = false,
    diffInPane = false,
    onsubmit,
    onreload,
  }: Props = $props();

  let comments = $state('');
</script>

<div class="reviewform">
  {#if !actionsOnly}
    <div class="title cond">人工评审（review_mode = human）</div>

    {#if stale}
      <div class="notice">变更 diff 正在重新生成，请稍候再评审。</div>
    {/if}
    {#if error}<div class="error">{error}</div>{/if}

    <section class="block">
      <h3 class="cond">变更 diff</h3>
      {#if onreload}
        <button type="button" class="btn quiet small" onclick={onreload}>刷新</button>
      {/if}
      {#if diffInPane}
        <div class="hint">diff 正文在 Diff 页签里展开，这里只留报告与动作。</div>
      {:else if diff}
        <div class="diff-scroll">
          <DiffView parsed={diff} {raw} />
        </div>
      {:else}
        <div class="hint">review-diff.diff 尚未生成或不可读。</div>
      {/if}
    </section>

    <section class="block">
      <h3 class="cond">agent 预审报告</h3>
      {#if reviewReport}
        <MarkdownView source={reviewReport} />
      {:else}
        <div class="hint">评审报告尚未生成。</div>
      {/if}
    </section>

    <section class="block">
      <h3 class="cond">单元测试结果</h3>
      {#if unitTestReport}
        <MarkdownView source={unitTestReport} />
      {:else}
        <div class="hint">单元测试结果尚未生成（review 在 test 之前）。</div>
      {/if}
    </section>
  {/if}

  <label class="comment">
    <span class="hint">打回意见（可选）</span>
    <textarea class="input" rows="3" bind:value={comments} placeholder="打回时随流转原因带给 develop…"></textarea>
  </label>

  <div class="actions" class:dock-acts={actionsOnly}>
    <button
      type="button"
      class="btn"
      class:quiet={actionsOnly}
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
    font-size: 12px;
    color: var(--text-2);
    margin-bottom: 12px;
  }
  .block {
    margin-bottom: 18px;
  }
  .block h3 {
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
    font-size: 12px;
    color: var(--pending);
    border: 2px solid var(--pending);
    padding: 2px 10px;
    margin-bottom: 10px;
  }
  .small {
    font-size: 12px;
    padding: 2px 8px;
    margin-bottom: 6px;
  }
  /* 桌面：diff 包装层不生成盒子（移动版横滚容器） */
  .diff-scroll {
    display: contents;
  }

  /* ── 移动版（<480px）：动作坞内只留意见 + 动作 ── */
  @media (max-width: 479px) {
    .diff-scroll {
      display: block;
    }
    .comment {
      margin: 0 0 8px;
    }
    .actions {
      justify-content: flex-start;
    }
    /* 主动作（通过）居左、旁路（打回并附意见）居右，同原型动作坞 */
    .actions.dock-acts .btn.solid {
      order: 1;
    }
    .actions.dock-acts .btn {
      order: 2;
    }
  }
</style>
