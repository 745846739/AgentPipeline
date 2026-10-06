<script lang="ts">
  import { notifications } from '../../stores/notifications.svelte';
  import { router } from '../../router.svelte';

  /** 容器引用：Escape 的**归属判据**（焦点在不在本组件里），不靠 class 名猜。 */
  let stack: HTMLDivElement | undefined = $state();

  function openTask(taskId: string | undefined, id: number) {
    notifications.dismiss(id);
    if (taskId) router.navigate(`/task/${taskId}`);
  }

  /**
   * 票 05（ux-audit-3）：toast 的 Escape 关闭路径。
   * 口径：**只关焦点所在的那一条**；焦点不在 `.toasts` 内 → 这条 Escape 不归 toast 管，
   * 一个字节都不动（对话框 / 菜单 / 决策 216④ 确认态各自有自己的 Escape）。
   */
  function handleKey(e: KeyboardEvent): void {
    if (e.key !== 'Escape' || notifications.toasts.length === 0) return;
    const active = document.activeElement;
    if (!stack || !active || !stack.contains(active)) return;
    const raw = active.closest('[data-toast-id]')?.getAttribute('data-toast-id') ?? null;
    if (raw === null || !Number.isInteger(Number(raw))) return;
    e.preventDefault();
    notifications.dismiss(Number(raw));
  }
</script>

<svelte:window onkeydown={handleKey} />

<!-- 每条 toast 自己是一个 polite live region（`role=status` + `aria-atomic`）：
     新节点入 DOM 即播报，且**整条一起念**（标题 + 消息分成两个 span，不 atomic 就只念
     变化的那一小段）。容器不再是 live region——否则每条新增都会让读屏重念整列。
     悬停 / 聚焦暂停计时（票 17）：正在读的那一条不该在手指底下消失。 -->
<div class="toasts" bind:this={stack}>
  {#each notifications.toasts as toast (toast.id)}
    <div
      class="toast {toast.cls}"
      role="status"
      aria-atomic="true"
      data-toast-id={toast.id}
      onmouseenter={() => notifications.pause(toast.id)}
      onmouseleave={() => notifications.resume(toast.id)}
      onfocusin={() => notifications.pause(toast.id)}
      onfocusout={() => notifications.resume(toast.id)}
    >
      <span class="dot {toast.cls === 'pending' ? 'warn-dot' : ''}"></span>
      <button type="button" class="body" onclick={() => openTask(toast.taskId, toast.id)}>
        <span class="title" title={toast.title}>{toast.title}</span>
        {#if toast.message}<span class="msg" title={toast.message}>{toast.message}</span>{/if}
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
  /* 窄屏：挪到顶栏下方（票 17 / R2-23）。底部那 16px 与详情页的动作坞（`bottom` = 底栏
     上沿、z-index 31）**正面重叠**，而 toast 整块是可点导航按钮——点动作的位置可能跳去
     另一个任务。挪到上面之后，钉住的两样（底栏、动作坞）都不再被压。
     高度取实测的 `--topbar-h`（票 09 引入），不再写死。 */
  @media (max-width: 479px) {
    .toasts {
      top: calc(var(--topbar-h) + 8px);
      bottom: auto;
      left: 16px;
      right: 16px;
      width: auto;
    }
  }
  .toast {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    text-align: left;
    background: var(--panel);
    border: 2px solid var(--pane);
    border-radius: 0;
    /* 像素框硬投影（§2.3） */
    box-shadow: 4px 4px 0 var(--ink);
    padding: 10px 12px;
  }
  .toast.pending {
    border-color: var(--pending);
  }
  .toast.failed {
    border-color: var(--stop);
  }
  /* 状态灯：实心像素方块（与 .st 同一套双编码），琥珀是唯一告警 */
  .dot {
    margin-top: 5px;
    flex: none;
    width: 8px;
    height: 8px;
    background: var(--done);
  }
  .toast.pending .dot {
    background: var(--pending);
  }
  .toast.failed .dot {
    background: var(--stop);
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
    font-size: 12px;
  }
  .msg {
    color: var(--text-3);
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .close {
    color: var(--text-3);
    font-size: 12px;
    line-height: 1;
    padding: 0 2px;
    flex: none;
  }
  .close:hover {
    color: var(--text-hi);
  }
</style>
