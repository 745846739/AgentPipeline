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
  /** 本地解析错（票 11 / R2-13）：与父组件递进来的提交错各自独立。 */
  let localError = $state<string | null>(null);
  const shownError = $derived(error ?? localError);

  /**
   * 解析文本域。出错时说清**第几行**（票 11 / R2-13）。
   *
   * 此前 `line.split('|')` 对 `| 说明` 这种行产出 `{title: '', description: '说明'}`：
   * 原任务被取消、一个**空标题**子任务被创建；而文本域全空时点「确认拆分」什么都不发生、
   * 也不说为什么（静默 no-op）。两条都在这里拦掉。
   */
  function parse(raw: string): { tasks: SplitTaskSpec[] } | { message: string } {
    const rows = raw
      .split('\n')
      .map((line, index) => ({ line: index + 1, text: line.trim() }))
      .filter((r) => r.text !== '');
    if (rows.length === 0) {
      return { message: '还没有拆出行来：每行一个子任务，格式「标题 | 描述」。' };
    }
    const tasks: SplitTaskSpec[] = [];
    for (const row of rows) {
      const [title, description] = row.text.split('|').map((s) => s.trim());
      if (!title) {
        return {
          message: `第 ${row.line} 行没有标题——「| 描述」这种写法会造出一个没有名字的子任务。`,
        };
      }
      tasks.push({ title, description: description ?? '' });
    }
    return { tasks };
  }

  function submit(e: SubmitEvent) {
    e.preventDefault();
    const parsed = parse(text);
    if ('message' in parsed) {
      localError = parsed.message;
      return;
    }
    localError = null;
    onsubmit(parsed.tasks);
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
  {#if shownError}<div class="error" role="alert">{shownError}</div>{/if}
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
