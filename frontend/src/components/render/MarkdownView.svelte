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
    color: var(--text);
    font-size: 12px;
    line-height: 1.65;
    max-width: 80ch;
  }
  /* 像素字体无字重轴：层级靠字号倍数与亮度阶；标题 24px 起步（§2.2） */
  .md :global(h1),
  .md :global(h2),
  .md :global(h3),
  .md :global(h4) {
    font-family: var(--font-code);
    letter-spacing: 0.02em;
    color: var(--text-hi);
    margin: 1.1em 0 0.45em;
  }
  .md :global(h1) {
    font-size: 24px;
  }
  .md :global(h2) {
    font-size: 24px;
  }
  .md :global(h3) {
    font-size: 12px;
    letter-spacing: 0.08em;
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
    font-size: 12px;
    background: var(--pane);
    padding: 1px 5px;
    border-radius: var(--r-pill);
    color: var(--text);
  }
  .md :global(pre) {
    background: var(--panel);
    border: 2px solid var(--pane);
    border-radius: var(--r-panel);
    padding: 10px 12px;
    overflow-x: auto;
    margin: 0.6em 0;
  }
  .md :global(pre code) {
    background: none;
    padding: 0;
    color: var(--text);
    line-height: 1.7;
  }
  .md :global(blockquote) {
    border-left: 2px solid var(--pane);
    padding-left: 10px;
    margin: 0.6em 0;
    color: var(--text-2);
  }
  .md :global(hr) {
    border: none;
    border-top: 2px solid var(--hairline);
    margin: 1em 0;
  }
  .md :global(a) {
    color: var(--text-hi);
    text-decoration: underline;
    text-underline-offset: 3px;
  }
  .md :global(strong) {
    color: var(--text-hi);
  }
</style>
