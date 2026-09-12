<script lang="ts">
  import { notifications } from '../../stores/notifications.svelte';
  import { router } from '../../router.svelte';

  function openTask(taskId: string | undefined, id: number) {
    notifications.dismiss(id);
    if (taskId) router.navigate(`/task/${taskId}`);
  }
</script>

<div class="toasts" aria-live="polite">
  {#each notifications.toasts as toast (toast.id)}
    <div class="toast {toast.cls}">
      <span class="dot {toast.cls === 'pending' ? 'warn-dot' : ''}"></span>
      <button type="button" class="body" onclick={() => openTask(toast.taskId, toast.id)}>
        <span class="title">{toast.title}</span>
        {#if toast.message}<span class="msg">{toast.message}</span>{/if}
      </button>
      <button
        type="button"
        class="close"
        aria-label="关闭"
        onclick={() => notifications.dismiss(toast.id)}>×</button
      >
    </div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    right: 16px;
    bottom: 16px;
    z-index: 70;
    display: flex;
    flex-direction: column;
    gap: 8px;
    width: 340px;
    max-width: calc(100vw - 32px);
  }
  .toast {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    text-align: left;
    background: var(--panel);
    border: 1px solid var(--pane);
    border-radius: 0;
    padding: 10px 12px;
  }
  .toast.pending {
    border-color: var(--pending);
  }
  .toast.failed {
    border-color: var(--stop);
  }
  .dot {
    margin-top: 5px;
    flex: none;
    color: var(--done);
    font-size: 10px;
    line-height: 1;
  }
  .dot::before {
    content: '●';
  }
  .toast.pending .dot {
    color: var(--pending);
  }
  .toast.failed .dot {
    color: var(--stop);
  }
  .body {
    display: flex;
    flex-direction: column;
    gap: 2px;
    flex: 1;
    min-width: 0;
    text-align: left;
    align-items: flex-start;
  }
  .title {
    color: var(--text-hi);
    font-size: 12.5px;
  }
  .msg {
    color: var(--text-3);
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .close {
    color: var(--text-3);
    font-size: 14px;
    line-height: 1;
    padding: 0 2px;
    flex: none;
  }
</style>
