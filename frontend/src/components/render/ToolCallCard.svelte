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
    gap: 10px;
    border-left: 2px solid var(--pane);
    padding: 3px 0 3px 12px;
    margin: 5px 0;
    max-width: 680px;
    font-size: 11px;
    color: var(--text-2);
  }
  .fn {
    font-family: var(--font-mono);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .fn b {
    color: var(--text-hi);
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
    color: var(--go);
  }
  .res.ok {
    color: var(--go);
  }
  .res.error {
    color: var(--stop);
  }

  /* 移动版（<480px）：工具卡折行（theme-3 §8） */
  @media (max-width: 479px) {
    .toolcard {
      flex-wrap: wrap;
      gap: 2px 10px;
      padding: 5px 0 5px 10px;
      margin: 6px 0;
      max-width: none;
      font-size: 12.5px;
    }
    .fn {
      overflow: visible;
      overflow-wrap: break-word;
      text-overflow: clip;
      white-space: normal;
    }
    .res {
      font-size: 12px;
    }
  }
</style>
