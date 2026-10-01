<script lang="ts">
  import { board } from '../../stores/board.svelte';
  import { router } from '../../router.svelte';
  import Modal from '../ui/Modal.svelte';

  interface Props {
    open: boolean;
    onclose: () => void;
  }
  let { open, onclose }: Props = $props();

  let projectId = $state('');
  let title = $state('');
  let description = $state('');
  let dependsOn = $state('');
  let reviewMode = $state<'agent' | 'human'>('agent');
  let submitting = $state(false);
  let error = $state<string | null>(null);
  /**
   * 字段级校验（票 02 / R2-06）：空项目、空标题、空描述是三件事，说清是哪一格才有落点
   * （`aria-invalid` + `aria-describedby` 指得到它）。
   */
  let fieldError = $state<{ field: 'project' | 'title' | 'description'; message: string } | null>(
    null
  );

  /**
   * 依赖候选（票 05）：当前项目**已有的任务**，用标题区分。
   *
   * 列表来源就是看板已经在用的那一份（`board.tasks`，由 `loadTasks` 按项目拉取）——
   * 本 effort 零新端点，也不做完整选择器：只在同一个输入框上挂一个原生候选项列表，
   * 手打与粘贴的路径原样保留。
   */
  const candidates = $derived(board.tasks.filter((t) => t.project_id === projectId));

  // 打开时同步默认项目
  $effect(() => {
    if (open) {
      projectId = board.projectId ?? board.projects[0]?.id ?? '';
      error = null;
      fieldError = null;
    }
  });

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    error = null;
    fieldError = null;
    if (!projectId) {
      fieldError = { field: 'project', message: '请选择项目。' };
      return;
    }
    if (!title.trim()) {
      fieldError = { field: 'title', message: '请填写标题。' };
      return;
    }
    // 描述与后端同一关（票 05①）：空描述的任务会让 architect 靠翻仓库猜范围
    if (!description.trim()) {
      fieldError = { field: 'description', message: '请填写描述：下游 agent 靠它判断要实现什么。' };
      return;
    }
    submitting = true;
    try {
      const task = await board.createTask({
        project_id: projectId,
        title: title.trim(),
        description: description.trim(),
        depends_on: dependsOn
          .split(',')
          .map((s) => s.trim())
          .filter(Boolean),
        review_mode: reviewMode,
      });
      if (!task?.id) {
        // 取不到 id 就说出来（票 10）：不然「创建成功却没跳」看起来像没建成，
        // 而任务其实已经在库里了。对话框**不关**——用户还能看到这句话。
        error = '任务已创建，但服务端没有回传任务 id，无法跳到它。请回看板刷新后手动打开。';
        return;
      }
      onclose();
      title = '';
      description = '';
      dependsOn = '';
      router.navigate(`/task/${task.id}`);
    } catch (err) {
      error = (err as Error).message;
    } finally {
      submitting = false;
    }
  }
</script>

<Modal
  {open}
  width={480}
  title="新建任务"
  submitLabel="创建并启动"
  {submitting}
  {onclose}
  onsubmit={submit}
>
  <label class="field">
    <span>项目</span>
    <select
      class="input"
      bind:value={projectId}
      aria-invalid={fieldError?.field === 'project' ? 'true' : undefined}
      aria-describedby={fieldError?.field === 'project' ? 'new-task-error' : undefined}
    >
      {#each board.projects as p (p.id)}
        <option value={p.id}>{p.name}</option>
      {/each}
    </select>
  </label>

  <label class="field">
    <span>标题</span>
    <input
      class="input"
      bind:value={title}
      placeholder="一句话说明要做什么"
      aria-invalid={fieldError?.field === 'title' ? 'true' : undefined}
      aria-describedby={fieldError?.field === 'title' ? 'new-task-error' : undefined}
    />
  </label>

  <label class="field">
    <span>描述（必填）</span>
    <textarea
      class="input"
      rows="3"
      bind:value={description}
      placeholder="要实现什么、验收标准是什么…"
      aria-invalid={fieldError?.field === 'description' ? 'true' : undefined}
      aria-describedby={fieldError?.field === 'description' ? 'new-task-error' : undefined}
    ></textarea>
  </label>

  <label class="field">
    <span>依赖任务 ID（逗号分隔，可选）</span>
    <input
      class="input mono"
      bind:value={dependsOn}
      list="depends-on-candidates"
      placeholder="01H…, 01H…"
    />
    <datalist id="depends-on-candidates">
      {#each candidates as t (t.id)}
        <option value={t.id}>{t.title}</option>
      {/each}
    </datalist>
  </label>

  <label class="field">
    <span>评审模式</span>
    <select class="input" bind:value={reviewMode}>
      <option value="agent">agent 自动评审</option>
      <option value="human">human 人工评审</option>
    </select>
  </label>

  {#if fieldError ?? error}
    <div class="error" id="new-task-error" role="alert">{fieldError?.message ?? error}</div>
  {/if}
</Modal>

<style>
  .field {
    display: block;
    margin-bottom: 10px;
  }
  /* 字段标签：告诉你这一格填什么，读不到就填不下去——次级必读档（票 15 归位）。 */
  .field > span {
    display: block;
    font-size: 12px;
    color: var(--text-3);
    letter-spacing: 0.04em;
    margin-bottom: 4px;
  }
  .error {
    color: var(--stop);
    font-size: 12px;
    margin: 4px 0 8px;
  }
</style>
