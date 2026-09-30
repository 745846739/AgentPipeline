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
    /* 长到一行放不下的**单个词**（JSON 正文、URL、commit hash、base64、命令输出）也得折，
       否则它把版面顶宽——iOS 在 `width=device-width` 下会因此把**整页**缩到塞得下为止
       （2026-09-30 实测：对讲台布局视口 390 → 1560，字号掉到四分之一）。决策 341。
       `anywhere` 而不是 `break-word`：前者参与 min-content 计算，flex/grid 祖先才会真的
       收得下来；后者只是「视觉上断行」，盒子的固有宽度照旧是那个长词的宽度。
       `pre` 不受这条管——它自己有 `overflow-x: auto`，是**有意**的横滚。 */
    overflow-wrap: anywhere;
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
