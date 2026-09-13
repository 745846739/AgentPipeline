<script lang="ts">
  import type { ParsedDiff } from '../../lib/diff';

  interface Props {
    parsed: ParsedDiff | null;
    raw?: string | null;
  }
  let { parsed, raw = null }: Props = $props();

  const statusLabel: Record<string, string> = {
    added: '新增',
    modified: '修改',
    deleted: '删除',
  };
</script>

{#if parsed && parsed.files.length > 0}
  {#each parsed.files as file (file.path)}
    <div class="dfile">
      <h4>
        <span class="path mono">{file.path}</span>
        <span class="st {file.status}">{statusLabel[file.status]}</span>
        <span class="fstat mono">+{file.additions} −{file.deletions}</span>
      </h4>
      <div class="dbody">
        {#each file.lines as line, i (i)}
          <div class="dl {line.kind}">{line.text}</div>
        {/each}
      </div>
    </div>
  {/each}
{:else if raw}
  <pre class="raw mono">{raw}</pre>
{:else}
  <div class="empty">没有可渲染的 diff。</div>
{/if}

<style>
  .dfile {
    margin-bottom: 14px;
  }
  .dfile h4 {
    display: flex;
    align-items: center;
    gap: 8px;
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--text-hi);
    margin-bottom: 6px;
  }
  .path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* 文件状态徽标：2px 描边像素块（增 / 删用印刷色，其余中性） */
  .st {
    font-family: var(--font-cond);
    font-size: 12px;
    letter-spacing: 0.05em;
    padding: 0 6px;
    border-radius: var(--r-pill);
    flex: none;
  }
  .st.modified {
    color: var(--text-2);
    background: var(--panel);
    border: 2px solid var(--pane);
  }
  .st.added {
    color: var(--diff-add);
    background: var(--diff-add-bg);
  }
  .st.deleted {
    color: var(--diff-del);
    background: var(--diff-del-bg);
  }
  .fstat {
    margin-left: auto;
    font-size: 12px;
    color: var(--text-3);
    flex: none;
  }
  /* diff 体：2px 描边盒，等宽、不折行可横滚（增删行用 --diff-add* / --diff-del* 印刷色） */
  .dbody {
    border: 2px solid var(--pane);
    border-radius: var(--r-panel);
    overflow: auto;
    max-height: 520px;
    font-family: var(--font-mono);
    font-size: 12px;
    line-height: 1.7;
  }
  .dl {
    padding: 0 12px;
    white-space: pre;
    min-width: max-content;
    color: var(--text-3);
  }
  .dl.add {
    background: var(--diff-add-bg);
    color: var(--diff-add);
  }
  .dl.del {
    background: var(--diff-del-bg);
    color: var(--diff-del);
  }
  .dl.hunk {
    color: var(--text-2);
    background: var(--panel);
  }
  .dl.meta {
    color: var(--text-3);
    background: var(--panel);
    border-bottom: 2px solid var(--hairline);
  }
  .raw {
    font-family: var(--font-mono);
    font-size: 12px;
    white-space: pre-wrap;
    color: var(--text-2);
  }
  .empty {
    color: var(--text-3);
    font-size: 12px;
    padding: 12px 0;
  }
</style>
