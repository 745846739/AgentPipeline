<script lang="ts">
  import type { SplitTaskSpec } from '../../api/client';
  import Modal from '../ui/Modal.svelte';

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

<Modal
  {open}
  width={520}
  title="拆分任务"
  submitLabel="确认拆分"
  {submitting}
  {onclose}
  onsubmit={submit}
>
  <!-- 正文只说动作与后果：拆分后原任务会怎样，而不是内部编号（决策 199）。 -->
  <div class="hint">
    每行一个子任务，格式 <span class="mono">标题 | 描述</span>。拆分后原任务会被取消。
  </div>
  <textarea class="input mono" rows="6" bind:value={text} placeholder="实现 A 部分 | 说明…&#10;实现 B 部分"></textarea>
  {#if error}<div class="error">{error}</div>{/if}
</Modal>

<style>
  .hint {
    font-size: 12px;
    color: var(--text-3);
    margin-bottom: 10px;
  }
  .error {
    color: var(--stop);
    font-size: 12px;
    margin-top: 6px;
  }
</style>
