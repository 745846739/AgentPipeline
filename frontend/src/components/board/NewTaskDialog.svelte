<script lang="ts">
  import { board } from '../../stores/board.svelte';
  import { router } from '../../router.svelte';

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

  // 打开时同步默认项目
  $effect(() => {
    if (open) {
      projectId = board.projectId ?? board.projects[0]?.id ?? '';
      error = null;
    }
  });

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    if (!projectId || !title.trim()) {
      error = '请填写项目与标题。';
      return;
    }
    submitting = true;
    error = null;
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
      onclose();
      title = '';
      description = '';
      dependsOn = '';
      if (task) router.navigate(`/task/${task.id}`);
    } catch (err) {
      error = (err as Error).message;
    } finally {
      submitting = false;
    }
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
      <div class="head cond">新建任务</div>

      <label class="field">
        <span>项目</span>
        <select class="input" bind:value={projectId}>
          {#each board.projects as p (p.id)}
            <option value={p.id}>{p.name}</option>
          {/each}
        </select>
      </label>

      <label class="field">
        <span>标题</span>
        <input class="input" bind:value={title} placeholder="一句话说明要做什么" />
      </label>

      <label class="field">
        <span>描述</span>
        <textarea class="input" rows="3" bind:value={description} placeholder="补充上下文…"></textarea>
      </label>

      <label class="field">
        <span>依赖任务 ID（逗号分隔，可选）</span>
        <input class="input mono" bind:value={dependsOn} placeholder="01H…, 01H…" />
      </label>

      <label class="field">
        <span>评审模式</span>
        <select class="input" bind:value={reviewMode}>
          <option value="agent">agent 自动评审</option>
          <option value="human">human 人工评审</option>
        </select>
      </label>

      {#if error}<div class="error">{error}</div>{/if}

      <div class="actions">
        <button type="button" class="btn quiet" onclick={onclose}>取消</button>
        <button type="submit" class="btn solid" disabled={submitting}>
          {#if submitting}<span class="spin"></span>{/if}
          创建并启动
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
    width: 480px;
    max-width: calc(100vw - 32px);
    padding: 18px 20px;
  }
  .head {
    font-size: 15px;
    color: var(--text-hi);
    margin-bottom: 14px;
  }
  .field {
    display: block;
    margin-bottom: 10px;
  }
  .field > span {
    display: block;
    font-size: 11.5px;
    color: var(--text-3);
    margin-bottom: 4px;
  }
  .error {
    color: var(--signal-stop);
    font-size: 12px;
    margin: 4px 0 8px;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 12px;
  }
</style>
