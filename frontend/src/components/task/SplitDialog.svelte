<script lang="ts">
  import type { SplitTaskSpec } from '../../api/client';

  interface Props {
    open: boolean;
    submitting?: boolean;
    error?: string | null;
    onclose: () => void;
    onsubmit: (tasks: SplitTaskSpec[]) => void;
  }
  let { open, submitting = false, error = null, onclose, onsubmit }: Props = $props();

  let text = $state('');

  function submit(e: SubmitEvent) {
    e.preventDefault();
    const tasks: SplitTaskSpec[] = text
      .split('\n')
      .map((line) => line.trim())
      .filter(Boolean)
      .map((line) => {
        const [title, description] = line.split('|').map((s) => s.trim());
        return { title, description: description ?? '' };
      });
    if (tasks.length > 0) onsubmit(tasks);
  }
</script>

{#if open}
  <div
    class="overlay"
    role="presentation"
    onclick={(e) => {
      if (e.target === e.currentTarget) onclose();
    }}
    onkeydown={(e) => e.key === 'Escape' && onclose()}
  >
    <form class="dialog panel" onsubmit={submit}>
      <div class="head cond">拆分任务</div>
      <div class="hint">
        每行一个子任务，格式 <span class="mono">标题 | 描述</span>。原任务将被置为 cancelled（决策 105）。
      </div>
      <textarea class="input mono" rows="6" bind:value={text} placeholder="实现 A 部分 | 说明…&#10;实现 B 部分"></textarea>
      {#if error}<div class="error">{error}</div>{/if}
      <div class="actions">
        <button type="button" class="btn quiet" onclick={onclose}>取消</button>
        <button type="submit" class="btn solid" disabled={submitting}>
          {#if submitting}<span class="spin"></span>{/if}
          确认拆分
        </button>
      </div>
    </form>
  </div>
{/if}

<style>
  .overlay {
    position: fixed;
    inset: 0;
    z-index: 60;
    background: rgba(6, 10, 16, 0.62);
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .dialog {
    width: 520px;
    max-width: calc(100vw - 32px);
    padding: 18px 20px;
  }
  .head {
    font-size: 15px;
    margin-bottom: 8px;
  }
  .hint {
    font-size: 12px;
    color: var(--text-3);
    margin-bottom: 10px;
  }
  .error {
    color: var(--signal-stop);
    font-size: 12px;
    margin-top: 6px;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 12px;
  }
</style>
