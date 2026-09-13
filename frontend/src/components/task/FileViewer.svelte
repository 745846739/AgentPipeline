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
          <div class="diff-scroll">
            <DiffView parsed={parsedDiff} raw={current.content} />
          </div>
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
    padding: 4px 8px;
    font-size: 12px;
    color: var(--text-2);
    cursor: pointer;
  }
  .cmd:hover,
  .cmd.active {
    background: var(--hover-bg);
  }
  .cmd.active .c {
    color: var(--text-hi);
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
  /* 内容区：像素框（2px 描边 + 硬投影），与台账盒同材质 */
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
    font-size: 12px;
    color: var(--text-3);
    margin-bottom: 10px;
    padding-bottom: 8px;
    border-bottom: 2px solid var(--pane);
  }
  .degraded {
    color: var(--stop);
  }
  .hint {
    color: var(--text-3);
    font-size: 12px;
  }
  .hint.error {
    color: var(--stop);
  }
  /* 桌面：diff 包装层不生成盒子（移动版横滚容器） */
  .diff-scroll {
    display: contents;
  }

  /* ── 移动版（<480px）：文件列表两行制 + diff 横滚（theme-3 §8） ── */
  @media (max-width: 479px) {
    .diff-scroll {
      display: block;
      overflow-x: auto;
    }
    .diff-scroll :global(.dl) {
      min-width: max-content;
      white-space: pre;
    }
    .fileview {
      flex-direction: column;
      gap: 0;
    }
    .list {
      width: 100%;
      margin-bottom: 8px;
    }
    .cmd {
      padding: 9px 2px;
      border-bottom: 2px solid var(--hairline);
      gap: 9px;
    }
    .cmd:last-child {
      border-bottom: 0;
    }
    .cmd .c {
      overflow: visible;
      text-overflow: clip;
      white-space: normal;
      word-break: break-all;
      font-size: 12px;
      color: var(--text-hi);
    }
    .content {
      max-height: none;
      padding: 12px 0 0;
      border-top: 2px solid var(--pane);
      border-left: 0;
      border-right: 0;
      border-bottom: 0;
      box-shadow: none;
    }
    .chead {
      font-size: 12px;
    }
  }
</style>
