<script lang="ts">
  import { renderMarkdown } from '../../lib/markdown';

  interface Props {
    source: string;
    class?: string;
  }
  let { source, class: klass = '' }: Props = $props();
  const html = $derived(renderMarkdown(source ?? ''));
</script>

<div class="md {klass}">{@html html}</div>

<style>
  .md {
    color: var(--text-hi);
    font-size: 13px;
    line-height: 1.65;
    max-width: 80ch;
  }
  .md :global(h1),
  .md :global(h2),
  .md :global(h3),
  .md :global(h4) {
    font-family: var(--font-cond);
    font-weight: 600;
    letter-spacing: 0.02em;
    color: var(--text-hi);
    margin: 1.1em 0 0.45em;
  }
  .md :global(h1) {
    font-size: 18px;
  }
  .md :global(h2) {
    font-size: 16px;
  }
  .md :global(h3) {
    font-size: 14px;
  }
  .md :global(p) {
    margin: 0.45em 0;
  }
  .md :global(ul),
  .md :global(ol) {
    margin: 0.4em 0 0.4em 1.4em;
  }
  .md :global(li) {
    margin: 0.15em 0;
  }
  .md :global(code) {
    font-family: var(--font-mono);
    font-size: 11.5px;
    background: var(--ink-700);
    padding: 1px 5px;
    border-radius: var(--r-pill);
    color: var(--text-2);
  }
  .md :global(pre) {
    background: var(--ink-800);
    border: 1px solid var(--line-soft);
    border-radius: var(--r-panel);
    padding: 10px 12px;
    overflow-x: auto;
    margin: 0.6em 0;
  }
  .md :global(pre code) {
    background: none;
    padding: 0;
    color: var(--text-2);
    line-height: 1.7;
  }
  .md :global(blockquote) {
    border-left: 2px solid var(--line);
    padding-left: 10px;
    margin: 0.6em 0;
    color: var(--text-2);
  }
  .md :global(hr) {
    border: none;
    border-top: 1px solid var(--line-soft);
    margin: 1em 0;
  }
  .md :global(a) {
    color: var(--branch-dev);
  }
  .md :global(strong) {
    color: var(--text-hi);
    font-weight: 600;
  }
</style>
