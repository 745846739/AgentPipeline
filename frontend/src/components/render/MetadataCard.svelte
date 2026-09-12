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
    background: var(--ink-800);
    border: 1px solid var(--line-soft);
    border-radius: var(--r-panel);
    padding: 10px 12px;
    margin: 8px 0;
    max-width: 760px;
  }
  .title {
    font-size: 11px;
    color: var(--text-3);
    letter-spacing: 0.05em;
    margin-bottom: 6px;
  }
  .row {
    display: flex;
    gap: 12px;
    padding: 3px 0;
    border-bottom: 1px solid var(--line-soft);
    font-size: 12px;
  }
  .row:last-child {
    border-bottom: none;
  }
  .key {
    flex: none;
    width: 180px;
    color: var(--text-3);
    font-size: 11px;
  }
  .val {
    color: var(--text-2);
    white-space: pre-wrap;
    word-break: break-word;
  }
  .raw {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--text-2);
    white-space: pre-wrap;
    max-height: 280px;
    overflow: auto;
  }
</style>
