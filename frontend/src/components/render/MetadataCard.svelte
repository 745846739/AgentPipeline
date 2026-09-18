<script lang="ts">
  interface Props {
    metadata: unknown;
    title?: string;
  }
  let { metadata, title = 'submit_metadata' }: Props = $props();

  function entries(value: unknown): Array<[string, unknown]> {
    if (value && typeof value === 'object' && !Array.isArray(value)) {
      return Object.entries(value as Record<string, unknown>);
    }
    return [];
  }

  function renderValue(value: unknown): string {
    if (value === null || value === undefined) return '—';
    if (typeof value === 'string') return value;
    if (typeof value === 'number' || typeof value === 'boolean') return String(value);
    return JSON.stringify(value, null, 2);
  }

  const rows = $derived(entries(metadata));
</script>

<div class="meta-card">
  <div class="title cond">{title}</div>
  {#if rows.length === 0}
    <pre class="raw">{renderValue(metadata)}</pre>
  {:else}
    {#each rows as [key, value] (key)}
      <div class="row">
        <span class="key mono">{key}</span>
        <span class="val">{renderValue(value)}</span>
      </div>
    {/each}
  {/if}
</div>

<style>
  .meta-card {
    background: var(--panel);
    border: 2px solid var(--pane);
    border-radius: var(--r-panel);
    padding: 10px 12px;
    margin: 8px 0;
    max-width: 760px;
  }
  .title {
    font-size: 12px;
    color: var(--text-3);
    letter-spacing: 0.05em;
    margin-bottom: 6px;
  }
  .row {
    display: flex;
    gap: 12px;
    padding: 3px 0;
    border-bottom: 2px solid var(--hairline);
    font-size: 12px;
  }
  .row:last-child {
    border-bottom: none;
  }
  .key {
    flex: none;
    width: 180px;
    color: var(--text-3);
    font-size: 12px;
  }
  .val {
    color: var(--text-hi);
    white-space: pre-wrap;
    /* `word-break: break-word`（= `overflow-wrap: break-word`）**不给 min-content 尺寸
       提供软换行点**：一个长路径 / ULID / token 会把这一行的 min-content 撑到它那么宽，
       顶着整张卡片（以及右栏）横向溢出（票 17 / R2-22）。两件事一起做：
       ① `min-width: 0` 让 flex 子项真的能收缩（flex 子项的 min-width 默认是 auto）；
       ② `overflow-wrap: anywhere` 才在 min-content 的计算里也提供换行点。 */
    min-width: 0;
    overflow-wrap: anywhere;
    word-break: break-word;
  }
  .raw {
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--text-hi);
    white-space: pre-wrap;
    max-height: 280px;
    overflow: auto;
  }
</style>
