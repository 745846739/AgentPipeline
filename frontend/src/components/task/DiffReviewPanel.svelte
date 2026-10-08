<script lang="ts">
  import type { AllowedAction, BranchCursor, PendingKind } from '../../api/types';
  import { actionKey, actionTier, confirmSentence } from '../../lib/actions';
  import type { ParsedDiff } from '../../lib/diff';
  import DiffView from '../render/DiffView.svelte';

  interface Props {
    diff: ParsedDiff | null;
    raw: string | null;
    stale?: boolean;
    error?: string | null;
    actions: AllowedAction[];
    cursors?: BranchCursor[];
    pendingType?: PendingKind;
    /** 加载中（首次拉取 diff）。 */
    loading?: boolean;
    busy?: boolean;
    /** 仅渲染动作行（窄屏底部动作坞复用，§5 转写 3）。 */
    actionsOnly?: boolean;
    /**
     * 用户已经停在主区 Diff 页签（票 08）→ 本面板不渲染 diff 正文，只留**状态摘要**
     * （统计行 / 陈旧与错误提示）+ 动作行：同一屏两份同样的 diff 会让「哪个是真的」
     * 变成问题，而档案盒存在的理由是「不切页签也能拍板」——动作行是红线，始终在。
     * `actionsOnly` 优先于本开关（移动款底部动作坞本来就没有 diff 正文）。
     */
    diffInPane?: boolean;
    onaction?: (action: AllowedAction, opts: { cursorId?: string; input?: string; push?: boolean }) => void;
    onreload?: () => void;
  }

  let {
    diff,
    raw,
    stale = false,
    error = null,
    actions,
    cursors = [],
    pendingType = 'merge_approval',
    loading = false,
    busy = false,
    actionsOnly = false,
    diffInPane = false,
    onaction,
    onreload,
  }: Props = $props();

  // merge 审批动作仅 approve / return（决策 23：**没有"拒绝"**）
  const mergeActions = $derived(actions.filter((a) => a.action === 'approve' || a.action === 'return'));
  const returnChanges = $derived(mergeActions.filter((a) => a.action === 'return'));
  const approve = $derived(mergeActions.filter((a) => a.action === 'approve'));
  const stats = $derived(diff?.stats ?? null);

  // 「合入后 push」开关（决策 393）：只挂在 approve 上，随决策一起提交。
  let pushAfterMerge = $state(false);

  /** 内联两步确认（票 03 / 决策 216②）：null = 没有任何钮在确认态。 */
  let confirming = $state<string | null>(null);

  function cursorIdFor(action: AllowedAction): string | undefined {
    return action.cursor_id ?? cursors[0]?.cursor_id;
  }

  /** 三档量级（决策 216⑥）：合入 approve 在本面板恒为 destructive → 红描边 + 确认步。 */
  function tierClass(action: AllowedAction): string {
    switch (actionTier(action, pendingType)) {
      case 'destructive':
        return 'btn danger';
      case 'gate-skip':
        return 'btn gate';
      case 'advance':
        return 'btn solid';
      default:
        return 'btn quiet';
    }
  }

  /** 后果句（决策 216③；null = 点一下就发）。 */
  function sentence(action: AllowedAction): string | null {
    return confirmSentence(action, pendingType);
  }

  function armed(action: AllowedAction): boolean {
    return confirming !== null && confirming === actionKey(action, cursorIdFor(action)) && sentence(action) !== null;
  }

  function submit(action: AllowedAction) {
    const key = actionKey(action, cursorIdFor(action));
    // 第一步只亮后果句；同一颗钮再点才真提交（与 PendingActions 同一口径）
    if (sentence(action) !== null && confirming !== key) {
      confirming = key;
      return;
    }
    confirming = null;
    const opts: { cursorId?: string; push?: boolean } = { cursorId: cursorIdFor(action) };
    if (action.action === 'approve') opts.push = pushAfterMerge;
    onaction?.(action, opts);
  }

  /** Escape 从确认态退回（决策 216④）；焦点不移动。 */
  function onKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape' && confirming !== null) {
      event.stopPropagation();
      confirming = null;
    }
  }

  // 动作集换了就退回普通态——不让上一轮的确认态挂到新一轮的钮上
  $effect(() => {
    void actions;
    confirming = null;
  });
</script>

<div class="diffpanel">
  {#if !actionsOnly}
    <div class="diffhead">
      {#if stats}
        <span class="big mono">
          <span class="a">+{stats.insertions}</span>
          <span class="d">−{stats.deletions}</span>
          · {stats.files_changed} 个文件
        </span>
      {:else}
        <span class="big mono dim">— diff 统计不可用</span>
      {/if}
      {#if onreload}
        <button type="button" class="btn quiet" onclick={onreload}>重新加载 diff</button>
      {/if}
    </div>

    {#if stale}
      <div class="notice">基准已前移，diff 重新生成中；合入审批已由后端重置。</div>
    {/if}
    {#if error}
      <div class="error">{error}</div>
    {/if}

    {#if diffInPane}
      <!-- 票 08：用户已经停在主区 Diff 页签 → 右栏只留结论与动作，不再摆第二份 diff。
           这句话是必要的：不说，用户会以为 diff 没了（同一屏两份的另一种坏法）。 -->
      <div class="hint">diff 正文在 Diff 页签里展开，这里只留结论与动作。</div>
    {:else}
      {#if stats && stats.file_details.length > 0}
        <div class="breakdown mono">
          {#each stats.file_details as f (f.path)}
            <span class="fdetail">
              <span class="path">{f.path}</span>
              <span class="a">+{f.additions}</span>
              <span class="d">−{f.deletions}</span>
            </span>
          {/each}
        </div>
      {/if}

      {#if loading}
        <div class="hint">正在加载 diff…</div>
      {:else}
        <div class="diff-scroll">
          <DiffView parsed={diff} {raw} />
        </div>
      {/if}
    {/if}
  {/if}

  <div class="actions" class:dock-acts={actionsOnly}>
    {#each returnChanges as action (actionKey(action, cursorIdFor(action)))}
      {#if armed(action)}
        <span class="confirm-q">{sentence(action)}</span>
      {/if}
      <button
        type="button"
        class="{tierClass(action)}{actionsOnly ? ' quiet' : ''}"
        disabled={busy}
        onclick={() => submit(action)}
        onkeydown={onKeydown}
      >
        {action.label}
      </button>
      {#if armed(action)}
        <button type="button" class="btn quiet" onclick={() => (confirming = null)}>取消</button>
      {/if}
    {/each}
    {#if approve.length > 0}
      <!-- 决策 393：合入后是否推远端——纯本地仓没有 remote 也能勾，服务端会跳过 -->
      <label class="pushopt">
        <input
          type="checkbox"
          bind:checked={pushAfterMerge}
          disabled={busy}
        />
        合入后 push 到远端
      </label>
    {/if}
    {#each approve as action (actionKey(action, cursorIdFor(action)))}
      {#if armed(action)}
        <!-- 决策 216②：就地换成后果句（12px --text-3）+ 同一颗钮 + 紧邻一颗取消 -->
        <span class="confirm-q">{sentence(action)}</span>
      {/if}
      <button
        type="button"
        class={tierClass(action)}
        disabled={busy}
        onclick={() => submit(action)}
        onkeydown={onKeydown}
      >
        {#if busy}<span class="spin"></span>{/if}
        {action.label}
      </button>
      {#if armed(action)}
        <button type="button" class="btn quiet" onclick={() => (confirming = null)}>取消</button>
      {/if}
    {/each}
    {#if mergeActions.length === 0}
      <span class="hint">当前没有可用的审批动作。</span>
    {/if}
  </div>
</div>

<style>
  .diffpanel {
    max-width: 980px;
  }
  .diffhead {
    display: flex;
    gap: 16px;
    align-items: baseline;
    margin-bottom: 12px;
    flex-wrap: wrap;
  }
  .diffhead .big {
    font-size: 24px;
    color: var(--text-hi);
  }
  .diffhead .a {
    color: var(--diff-add);
  }
  .diffhead .d {
    color: var(--diff-del);
  }
  .dim {
    color: var(--text-3);
  }
  .notice {
    display: inline-block;
    font-size: 12px;
    color: var(--pending);
    border: 2px solid var(--pending);
    padding: 2px 10px;
    margin-bottom: 10px;
  }
  .error {
    color: var(--stop);
    font-size: 12px;
    margin-bottom: 8px;
  }
  /* 确认步后果句（决策 216②：12px --text-3，就地出现，常驻处不摆） */
  .confirm-q {
    display: block;
    font-size: 12px;
    color: var(--text-3);
    margin: 2px 0 5px;
  }
  .breakdown {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 14px;
    font-size: 12px;
    color: var(--text-3);
    margin-bottom: 10px;
  }
  .fdetail {
    display: inline-flex;
    gap: 6px;
  }
  .path {
    color: var(--text-2);
  }
  .breakdown .a {
    color: var(--diff-add);
  }
  .breakdown .d {
    color: var(--diff-del);
  }
  .actions {
    display: flex;
    gap: 8px;
    margin-top: 14px;
  }
  .pushopt {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--text-2);
    user-select: none;
    cursor: pointer;
  }
  .hint {
    color: var(--text-3);
    font-size: 12px;
  }
  /* 桌面：diff 包装层不生成盒子，与移动版横滚容器共用同一 DOM */
  .diff-scroll {
    display: contents;
  }

  /* ── 移动版（<480px）：动作进底部坞，diff 横滚（§5 移动款） ── */
  @media (max-width: 479px) {
    .diffpanel {
      max-width: none;
    }
    .diff-scroll {
      display: block;
    }
    .actions.dock-acts {
      display: flex;
      gap: 8px;
      margin-top: 0;
    }
    /* 主动作（合入）居左、旁路（返回修改）居右，同原型动作坞 */
    .actions.dock-acts .btn {
      order: 2;
    }
    .actions.dock-acts .btn.solid {
      order: 1;
    }
    .diffhead .big {
      font-size: 24px;
    }
    .notice {
      display: block;
      padding: 6px 10px;
      line-height: 1.6;
    }
  }
</style>
