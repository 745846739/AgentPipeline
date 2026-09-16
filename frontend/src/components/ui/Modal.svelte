<script lang="ts">
  import type { Snippet } from 'svelte';

  /**
   * 对话框的**唯一形状**（UX 审计票 02）。
   *
   * 新建任务 / 拆分任务 / 更换长上下文模型这三个对话框此前是同一份复制粘贴的写法，
   * 毛病也一样：Escape 挂在遮罩上而焦点从没进过对话框（点开按 Escape 关不掉）、
   * Tab 会一路跑到背后的页面上、版面上没有可被读屏播报的模态语义。
   * 这里把三处共用的那一层抽出来，只抽一次：
   *
   * - **Escape 一律可关**：监听挂在 `window` 上，不依赖焦点在哪（焦点还在背后的按钮上也关得掉）；
   * - **打开时焦点进第一个可填控件**：用户不必先点一下；
   * - **焦点关在框内**：Tab / Shift+Tab 在框内循环，不会被背后的页面接走；
   * - **可播报的语义**：`role="dialog"` + `aria-modal` + `aria-labelledby` 指向标题
   *   （读屏播报的是「进了一个对话框：<标题>」，而不是一段无名的排版）。
   *
   * 版面纪律：2px 描边 / 零圆角 / 4px 硬投影走 `.panel`（与 `GEOMETRY.shadow` 同值），
   * 三个调用点只各自保住自己原来的宽度。
   *
   * 新增模态框请一律用它，别再复制一份遮罩 + 对话框出来。
   */
  interface Props {
    open: boolean;
    /** 标题：同时是对话框的可访问名（`aria-labelledby` 指过去）。 */
    title: string;
    /** 三条退出路径（Escape / 点遮罩 / 取消）都走它。 */
    onclose: () => void;
    /** 表单提交：Modal 已 preventDefault，调用点只管自己的提交逻辑与错误。 */
    onsubmit: (event: SubmitEvent) => void;
    /** 主按钮文案（契约性字符串，各调用点自己给）。 */
    submitLabel: string;
    /** 主按钮转圈并禁用（提交中）。 */
    submitting?: boolean;
    /** 主按钮的额外禁用条件（如「还没选东西」）。 */
    submitDisabled?: boolean;
    cancelLabel?: string;
    /** 版面宽度：三个调用点沿用改动前的宽度。 */
    width?: number;
    /** 表单内容（字段 / 说明 / 错误）。按钮行由 Modal 画。 */
    children: Snippet;
  }

  let {
    open,
    title,
    onclose,
    onsubmit,
    submitLabel,
    submitting = false,
    submitDisabled = false,
    cancelLabel = '取消',
    width = 480,
    children,
  }: Props = $props();

  /** 标题节点的 id：`aria-labelledby` 指过去（每实例唯一，同页多个框不会串）。 */
  const headId = $props.id();

  let panel = $state<HTMLElement | null>(null);

  /**
   * 同一个按键事件可能被两处接到（遮罩上的处理器 + `window` 上的兜底），
   * 用事件对象去重，避免 Tab 被处理两次（第二次会把焦点推回另一端）。
   */
  const handled = new WeakSet<Event>();

  /** 框内可聚焦的控件（Tab 循环的边界；看不见的不算）。 */
  function focusables(): HTMLElement[] {
    if (!panel) return [];
    const sel =
      'input:not([disabled]), select:not([disabled]), textarea:not([disabled]), button:not([disabled]), a[href], [tabindex]:not([tabindex="-1"])';
    return [...panel.querySelectorAll<HTMLElement>(sel)].filter((el) => el.getClientRects().length > 0);
  }

  /**
   * 打开时焦点落在**第一个输入框**：优先 `input` / `textarea`（能直接开始打字的那些），
   * 没有可打字的才落到框内第一个控件（下拉框 / 按钮）——比如「更换长上下文模型」只有一个下拉。
   */
  function firstField(): HTMLElement | null {
    if (!panel) return null;
    return (
      panel.querySelector<HTMLElement>('input:not([disabled]), textarea:not([disabled])') ??
      panel.querySelector<HTMLElement>('select:not([disabled]), button:not([disabled])')
    );
  }

  /** 打开时把焦点送进去（票 02 的验收点之一：不必先点一下）。 */
  $effect(() => {
    if (!open || !panel) return;
    firstField()?.focus();
  });

  function handleKey(e: KeyboardEvent) {
    if (!open || handled.has(e)) return;
    handled.add(e);

    if (e.key === 'Escape') {
      // 焦点从未进过对话框时也要关得掉——这正是改动前的毛病。
      e.preventDefault();
      onclose();
      return;
    }
    if (e.key !== 'Tab') return;

    const items = focusables();
    const active = document.activeElement as HTMLElement | null;
    const inside = !!panel && !!active && panel.contains(active);
    if (items.length === 0) {
      // 框内没有可聚焦物：焦点留在这里，别漏到背后去。
      e.preventDefault();
      panel?.focus();
      return;
    }
    const first = items[0];
    const last = items[items.length - 1];
    if (e.shiftKey) {
      if (!inside || active === first) {
        e.preventDefault();
        last.focus();
      }
    } else if (!inside || active === last) {
      e.preventDefault();
      first.focus();
    }
  }
</script>

<svelte:window onkeydown={handleKey} />

{#if open}
  <div
    class="overlay"
    role="presentation"
    onclick={(e) => {
      if (e.target === e.currentTarget) onclose();
    }}
    onkeydown={(e) => {
      handleKey(e);
      e.stopPropagation();
    }}
  >
    <!-- 语义壳（role=dialog 只能落在非交互元素上；表单是它的孩子）：
         读屏播报「进了一个对话框：<标题>」，标题节点由 aria-labelledby 指过去。 -->
    <div
      class="shell"
      role="dialog"
      aria-modal="true"
      aria-labelledby={headId}
      tabindex="-1"
      style="width: {width}px"
      bind:this={panel}
    >
      <form
        class="dialog panel"
        onsubmit={(e) => {
          e.preventDefault();
          onsubmit(e);
        }}
      >
        <div class="head cond" id={headId}>{title}</div>
        {@render children()}
        <div class="actions">
          <button type="button" class="btn quiet" onclick={onclose}>{cancelLabel}</button>
          <button type="submit" class="btn solid" disabled={submitting || submitDisabled}>
            {#if submitting}<span class="spin"></span>{/if}
            {submitLabel}
          </button>
        </div>
      </form>
    </div>
  </div>
{/if}

<style>
  .overlay {
    position: fixed;
    inset: 0;
    z-index: 60;
    background: var(--overlay);
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .shell {
    max-width: calc(100vw - 32px);
  }
  .dialog {
    padding: 18px 20px;
  }
  .head {
    font-size: 12px;
    color: var(--text-hi);
    margin-bottom: 8px;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 12px;
  }
</style>
