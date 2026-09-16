<script lang="ts">
  import type { AllowedAction, BranchCursor, PendingKind } from '../../api/types';
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
    onaction?: (action: AllowedAction, opts: { cursorId?: string; input?: string }) => void;
    onreload?: () => void;
  }

  let {
    diff,
    raw,
    stale = false,
    error = null,
    actions,
    cursors = [],
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

  function cursorIdFor(action: AllowedAction): string | undefined {
    return action.cursor_id ?? cursors[0]?.cursor_id;
  }
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
    {#each returnChanges as action (action.action + (action.cursor_id ?? ''))}
      <button
        type="button"
        class="btn"
        class:quiet={actionsOnly}
        disabled={busy}
        onclick={() => onaction?.(action, { cursorId: cursorIdFor(action) })}
      >
        {action.label}
      </button>
    {/each}
    {#each approve as action (action.action + (action.cursor_id ?? ''))}
      <button
        type="button"
        class="btn solid"
        disabled={busy}
        onclick={() => onaction?.(action, { cursorId: cursorIdFor(action) })}
      >
        {#if busy}<span class="spin"></span>{/if}
        {action.label}
      </button>
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
