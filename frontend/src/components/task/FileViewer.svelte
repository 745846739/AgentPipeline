<script lang="ts">
  import { parseUnifiedDiff } from '../../lib/diff';
  import CodeHighlight from '../render/CodeHighlight.svelte';
  import DiffView from '../render/DiffView.svelte';
  import MarkdownView from '../render/MarkdownView.svelte';

  export interface LoadedFileView {
    path: string;
    content: string | null;
    error: string | null;
    status: number | null;
  }

  /** 任务目录产出文件（data-model §4.2 文件目录约定）。 */
  const DEFAULT_FILES = [
    'design.md',
    'dev-plan.md',
    'test-scenarios.md',
    'review-report.md',
    'review-diff.diff',
    'test-report.md',
    'merge-proposal.diff',
  ];

  interface Props {
    files?: string[];
    loaded?: Record<string, LoadedFileView>;
    onload?: (path: string) => void;
  }

  let { files = DEFAULT_FILES, loaded = {}, onload }: Props = $props();

  let selected = $state<string | null>(null);

  function select(path: string) {
    selected = path;
    onload?.(path);
  }

  const current = $derived(selected ? loaded[selected] : undefined);
  const isDiff = $derived(selected?.endsWith('.diff') ?? false);
  const parsedDiff = $derived(
    isDiff && current?.content ? parseUnifiedDiff(current.content) : null,
  );
</script>

<div class="fileview">
  <div class="list">
    {#each files as file (file)}
      <button
        type="button"
        class="cmd"
        class:active={selected === file}
        onclick={() => select(file)}
      >
        <span class="c mono">{file}</span>
        <span class="ex">查看 ▸</span>
      </button>
    {/each}
  </div>

  {#if selected}
    <div class="content panel">
      <div class="chead">
        <span class="mono">{selected}</span>
        {#if current?.status === 403}<span class="degraded">403 · 已降级</span>{/if}
      </div>
      {#if !current}
        <div class="hint">正在加载…</div>
      {:else if current.error}
        <div class="hint error">{current.error}</div>
      {:else if current.content !== null}
        {#if isDiff}
          <DiffView parsed={parsedDiff} raw={current.content} />
        {:else if selected.endsWith('.md')}
          <MarkdownView source={current.content} />
        {:else}
          <CodeHighlight code={current.content} maxHeight={520} />
        {/if}
      {/if}
    </div>
  {/if}
</div>

<style>
  .fileview {
    display: flex;
    gap: 16px;
    align-items: flex-start;
  }
  .list {
    width: 240px;
    flex: none;
  }
  .cmd {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    text-align: left;
    padding: 7px 10px;
    border-radius: var(--r-panel);
    font-size: 11.5px;
    color: var(--text-2);
  }
  .cmd:hover,
  .cmd.active {
    background: var(--ink-800);
  }
  .cmd.active .c {
    color: var(--branch-dev);
  }
  .c {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ex {
    color: var(--text-3);
    flex: none;
  }
  .content {
    flex: 1;
    min-width: 0;
    padding: 12px 16px;
    max-height: 70vh;
    overflow: auto;
  }
  .chead {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    font-size: 11.5px;
    color: var(--text-3);
    margin-bottom: 10px;
  }
  .degraded {
    color: var(--signal-stop);
  }
  .hint {
    color: var(--text-3);
    font-size: 12px;
  }
  .hint.error {
    color: var(--signal-stop);
  }
</style>
