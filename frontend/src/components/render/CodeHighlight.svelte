<script lang="ts">
  interface Props {
    code: string;
    language?: string;
    maxHeight?: number;
  }
  let { code, language = 'text', maxHeight = 420 }: Props = $props();
</script>

<pre class="code" style="max-height:{maxHeight}px" data-lang={language}><code>{code}</code></pre>

<style>
  .code {
    background: var(--ink-800);
    border: 1px solid var(--line-soft);
    border-radius: var(--r-panel);
    padding: 10px 12px;
    overflow: auto;
    font-family: var(--font-mono);
    font-size: 11.5px;
    line-height: 1.7;
    color: var(--text-2);
    white-space: pre;
  }
</style>
