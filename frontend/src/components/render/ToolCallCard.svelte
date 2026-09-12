<script lang="ts">
  interface Props {
    tool: string;
    argsSummary?: string;
    resultSummary?: string;
    phase?: 'start' | 'end' | 'error';
  }
  let { tool, argsSummary = '', resultSummary, phase = 'end' }: Props = $props();
</script>

<div class="toolcard">
  <span class="fn"><b>{tool}</b>({argsSummary})</span>
  {#if resultSummary}<span class="res {phase}">{resultSummary}</span>{/if}
</div>

<style>
  .toolcard {
    display: flex;
    align-items: center;
    gap: 8px;
    background: var(--ink-800);
    border: 1px solid var(--line-soft);
    border-radius: var(--r-panel);
    padding: 6px 10px;
    margin: 6px 0;
    max-width: 640px;
  }
  .fn {
    font-family: var(--font-mono);
    font-size: 11.5px;
    color: var(--text-2);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .fn b {
    color: var(--branch-dev);
    font-weight: 500;
  }
  .res {
    margin-left: auto;
    font-family: var(--font-mono);
    font-size: 10.5px;
    color: var(--text-3);
    flex: none;
  }
  .res.end {
    color: var(--signal-go);
  }
  .res.error {
    color: var(--signal-stop);
  }
</style>
