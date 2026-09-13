<script lang="ts">
  import { formatTokens } from '../../lib/format';

  interface Props {
    promptTokens: number;
    completionTokens: number;
    label?: string;
    /** 流式累加中（每秒可能多次重渲，rAF 合帧）。 */
    live?: boolean;
  }
  let { promptTokens, completionTokens, label = '本次尝试', live = false }: Props = $props();

  // svelte-ignore state_referenced_locally
  let shownPrompt = $state(promptTokens);
  // svelte-ignore state_referenced_locally
  let shownCompletion = $state(completionTokens);
  // svelte-ignore state_referenced_locally
  let pendingPrompt = promptTokens;
  // svelte-ignore state_referenced_locally
  let pendingCompletion = completionTokens;
  let raf = 0;

  // TokenMeter 是唯一允许每秒多次重渲的组件：requestAnimationFrame 合帧。
  $effect(() => {
    pendingPrompt = promptTokens;
    pendingCompletion = completionTokens;
    if (typeof requestAnimationFrame === 'undefined') {
      shownPrompt = pendingPrompt;
      shownCompletion = pendingCompletion;
      return;
    }
    if (raf) return;
    raf = requestAnimationFrame(() => {
      raf = 0;
      shownPrompt = pendingPrompt;
      shownCompletion = pendingCompletion;
    });
  });

  const total = $derived(shownPrompt + shownCompletion);
</script>

<div class="tokmeter">
  {label} <b>{formatTokens(total)}</b> tok
  {#if live}<span class="live">· 流式中</span>{/if}
</div>

<style>
  .tokmeter {
    clear: both;
    text-align: right;
    font-family: var(--font-mono);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    color: var(--text-3);
    padding: 6px 0;
  }
  .tokmeter b {
    color: var(--text-hi);
    font-weight: 500;
  }
  .live {
    color: var(--go);
  }
</style>
