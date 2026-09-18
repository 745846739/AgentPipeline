<script lang="ts">
  import type { AllowedAction, BranchCursor, PendingKind, PendingReason } from '../../api/types';
  import type { ParsedDiff } from '../../lib/diff';
  import { pendingLabel } from '../../lib/pipeline';
  import PendingActions from '../board/PendingActions.svelte';
  import Sprite from '../render/Sprite.svelte';
  import DiffReviewPanel from './DiffReviewPanel.svelte';
  import ReviewForm from './ReviewForm.svelte';
  interface Props {
    reason: PendingReason;
    cursors: BranchCursor[];
    actions: AllowedAction[];
    busy?: boolean;
    /** 窄屏（<480px）：dossier 转固定底部动作坞（theme-6 §5 转写 3）。 */
    dock?: boolean;
    /** dock 实际高度回传：详情内容据此留出底边距，避免被固定坞遮住。 */
    ondockheight?: (height: number) => void;
    onaction?: (action: AllowedAction, opts: { cursorId?: string; input?: string }) => void;
    /** 触发 pending 的节点会话直达（§12.4.3 联动）。 */
    ongotoconversation?: (stage: string, node: string) => void;
    onopenfiles?: () => void;
    /** merge_approval 内嵌 Diff。 */
    diff?: ParsedDiff | null;
    rawDiff?: string | null;
    diffStale?: boolean;
    diffError?: string | null;
    diffLoading?: boolean;
    /**
     * 票 08：用户已经停在主区 Diff 页签（`merge_approval`）→ 档案盒收成不重复内容的状态摘要。
     * 动作行照旧渲染（那是被测合约的一部分，也是「不必先切页签就能拍板」的落点）。
     */
    diffInPane?: boolean;
    onreloaddiff?: () => void;
    /** human_review 三件套。 */
    reviewReport?: string | null;
    unitTestReport?: string | null;
    onsubmitreview?: (approved: boolean, comments?: string) => void;
  }

  let {
    reason,
    cursors,
    actions,
    busy = false,
    dock = false,
    ondockheight,
    onaction,
    ongotoconversation,
    onopenfiles,
    diff = null,
    rawDiff = null,
    diffStale = false,
    diffError = null,
    diffLoading = false,
    diffInPane = false,
    onreloaddiff,
    reviewReport = null,
    unitTestReport = null,
    onsubmitreview,
  }: Props = $props();

  const pendingType = $derived<PendingKind>(reason.type);
  // 触发 pending 的游标（用于会话直达）
  const triggerCursor = $derived(
    cursors.find((c) => c.status === 'pending' && c.pending_reason?.type === reason.type) ??
      cursors.find((c) => c.status === 'pending'),
  );
  const conflicts = $derived(reason.context?.conflict_task_ids ?? []);

  /** dock 模式：量取固定动作坞高度，供详情内容留出底边距（theme-6 §5 转写 3）。 */
  let dockH = $state(0);
  $effect(() => {
    if (dock) ondockheight?.(dockH);
  });
</script>

{#snippet infoBlock()}
  <div class="msg">{reason.message}</div>

  {#if conflicts.length > 0}
    <div class="ctx">
      冲突任务：<b>{conflicts.join('、')}</b>
    </div>
  {/if}
  {#if reason.context?.kind}
    <div class="ctx mono dim">kind = {reason.context.kind}</div>
  {/if}
  {#if reason.context?.diagnostic}
    <!-- 主流程票 03：原始诊断与 message 分离渲染——可操作提示为主，原始串供排查。
         像素主题（票 07）：诊断仍是对话框里逐字可见的次级行，样式不得吞掉它。 -->
    <div class="ctx mono dim">诊断：{reason.context.diagnostic}</div>
  {/if}
{/snippet}

{#snippet foreman()}
  <i class="dface" aria-hidden="true"><Sprite name="foreman" /></i>
{/snippet}

{#if dock}
  <aside class="dock" aria-label="待办" bind:clientHeight={dockH}>
    <div class="dock-tag">⏸ 等你拍板 · {pendingLabel(reason)}</div>
    {@render infoBlock()}

    {#if pendingType === 'merge_approval'}
      <DiffReviewPanel
        actionsOnly
        {diff}
        raw={rawDiff}
        stale={diffStale}
        error={diffError}
        loading={diffLoading}
        {actions}
        {cursors}
        {busy}
        {onaction}
      />
    {:else if pendingType === 'human_review'}
      <ReviewForm
        actionsOnly
        {diff}
        raw={rawDiff}
        {reviewReport}
        {unitTestReport}
        stale={diffStale}
        {busy}
        error={diffError}
        onsubmit={onsubmitreview}
      />
    {:else}
      <PendingActions
        {actions}
        {cursors}
        {pendingType}
        {onaction}
        disabled={busy}
        isBusy={() => busy}
      />
    {/if}

    {#if triggerCursor}
      <div class="ctx trigger">
        触发节点：
        <button type="button" class="linklike" onclick={() => ongotoconversation?.(triggerCursor.stage, triggerCursor.node)}>
          {triggerCursor.stage}.{triggerCursor.node} ▸
        </button>
      </div>
    {/if}

    <div class="ctx">
      <button type="button" class="linklike" onclick={() => onopenfiles?.()}>查看产出文件 ▸</button>
    </div>
  </aside>
{:else}
  <!-- 急停对话框（决策 169 / theme-6-pixel.md §3）：奶油双线框 + 压在框沿上的琥珀名牌
       tab（FF 式）+ 闪烁 ▼ 光标 + 左侧 16×16 工头头像。恢复动作 = 对话框菜单项按钮。 -->
  <aside class="dossier" aria-label="待办">
    <div class="dtag">⏸ 等你拍板 · {pendingLabel(reason)}</div>
    {@render foreman()}
    <div class="dmain">
      {@render infoBlock()}

      {#if pendingType === 'merge_approval'}
        <div class="grp">恢复动作</div>
        <DiffReviewPanel
          {diffInPane}
          {diff}
          raw={rawDiff}
          stale={diffStale}
          error={diffError}
          loading={diffLoading}
          {actions}
          {cursors}
          {busy}
          {onaction}
          onreload={onreloaddiff}
        />
      {:else if pendingType === 'human_review'}
        <div class="grp">人工评审</div>
        <ReviewForm
          {diffInPane}
          {diff}
          raw={rawDiff}
          {reviewReport}
          {unitTestReport}
          stale={diffStale}
          {busy}
          error={diffError}
          onsubmit={onsubmitreview}
          onreload={onreloaddiff}
        />
      {:else}
        <!-- 恢复动作 / 旁路动作的分组头由 PendingActions 逐组渲染，不在此重复。 -->
        <PendingActions
          {actions}
          {cursors}
          {pendingType}
          {onaction}
          disabled={busy}
          isBusy={() => busy}
        />
      {/if}

      {#if triggerCursor}
        <div class="ctx trigger">
          触发节点：
          <button type="button" class="linklike" onclick={() => ongotoconversation?.(triggerCursor.stage, triggerCursor.node)}>
            {triggerCursor.stage}.{triggerCursor.node} ▸
          </button>
        </div>
      {/if}

      <div class="ctx">
        <button type="button" class="linklike" onclick={() => onopenfiles?.()}>查看产出文件 ▸</button>
      </div>
    </div>
  </aside>
{/if}

<style>
  /* ── 急停对话框（原型 .dossier）：双线框 = 琥珀外框 + panel 空隙 + pane 内框 ──
     右栏与左栏同处第 1 行：左栏现在是一个整体网格项（`.detail.split .main`），
     跨多行只会凭空多出零高的隐式行，没有收益。 */
  .dossier {
    grid-column: 2;
    grid-row: 1;
    align-self: start;
    position: sticky;
    /* 让位给**实测的**顶栏高度（票 09 / R2-11）：56px 那个旧值与真实顶栏（78–81px）
       对不上，于是顶栏把压在框沿上的琥珀铭牌整块盖住——而那句「等你拍板」正是
       「为什么这里有东西等你」的答案。再 +16px 是铭牌自己向上压出的那一截
       （`.dtag` 的 `top: -16px`）：不让它，铭牌照样会钻到顶栏底下。 */
    top: calc(var(--topbar-h) + 16px);
    margin-top: 20px; /* 给压在框沿上的名牌 tab 留出空间 */
    background: var(--panel);
    border: 2px solid var(--pending);
    box-shadow:
      inset 0 0 0 2px var(--panel),
      inset 0 0 0 4px var(--pane),
      4px 4px 0 var(--ink);
    padding: 12px 14px;
    display: flex;
    gap: 12px;
    align-items: flex-start;
  }
  /* 名牌 tab：绝对定位，压在琥珀外框上（FF 式），框线被 panel 底盖住 */
  .dtag {
    position: absolute;
    top: -16px;
    left: 6px;
    background: var(--panel);
    border: 2px solid var(--pending);
    color: var(--pending);
    padding: 0 8px;
    line-height: 1.5;
    white-space: nowrap;
  }
  /* ▼ 闪烁光标：原型 .dialog::after；全站四处允许动画之一 */
  .dossier::after {
    content: '▼';
    position: absolute;
    right: 6px;
    bottom: 2px;
    color: var(--pending);
    font-size: 12px;
    line-height: 1;
    animation: blink 1s steps(2) infinite;
  }
  .dface {
    flex: none;
    display: block;
    width: 48px;
    height: 48px;
    line-height: 0;
  }
  .dmain {
    flex: 1;
    min-width: 0;
    max-height: calc(100vh - 160px);
    overflow: auto;
    padding-bottom: 4px; /* 不压住右下角的 ▼ */
  }
  .msg {
    color: var(--text-hi);
    margin-bottom: 9px;
    overflow-wrap: anywhere;
  }
  .ctx {
    color: var(--text-2);
    margin-bottom: 6px;
    line-height: 1.8;
    overflow-wrap: anywhere;
  }
  .ctx b {
    color: var(--go);
  }
  .dim {
    color: var(--text-3);
  }
  .trigger {
    margin-top: 10px;
  }
  .grp {
    font-size: 12px;
    color: var(--text-3);
    letter-spacing: 0.08em;
    margin: 12px 0 7px;
  }
  .linklike {
    color: var(--text-hi);
    padding: 0;
    text-align: left;
    text-decoration: underline;
    text-underline-offset: 3px;
  }
  .dossier :global(.btn) {
    display: block;
    width: 100%;
    text-align: left;
    margin-bottom: 6px;
  }
  .dossier :global(.actions) {
    flex-direction: column;
    align-items: stretch;
    justify-content: flex-start;
  }

  /* ── 移动版底部动作坞（<480px，theme-6 §5 转写 3）：对话框式常驻动作坞 ──
     dossier 的对话框内容（含工头头像）留在正文流，恢复动作下沉为固定底部坞；
     坞带琥珀顶框 + ▼ 光标（对话框语汇），异步按钮点击即禁用。 */
  .dock {
    max-height: 72vh;
    overflow: auto;
  }
  .dock .dock-tag {
    position: relative;
    color: var(--pending);
    margin-bottom: 8px;
  }
  /* 坞内的 ▼ 光标（原型 .dock-tag::after）：琥珀，离散闪烁 */
  .dock .dock-tag::after {
    content: '▼';
    float: right;
    color: var(--pending);
    animation: blink 1s steps(2) infinite;
  }
  .dock .msg {
    color: var(--text-hi);
    line-height: 1.7;
    margin-bottom: 7px;
    overflow-wrap: anywhere;
  }
  .dock .ctx {
    margin: 7px 0 0;
    color: var(--text-2);
    line-height: 1.85;
    overflow-wrap: anywhere;
  }
  .dock .ctx b {
    color: var(--go);
  }
  .dock .dim {
    color: var(--text-3);
  }
  .dock .linklike {
    color: var(--text-hi);
    padding: 0;
    text-align: left;
    text-decoration: underline;
    text-underline-offset: 3px;
  }
  /* 坞内动作可点目标 ≥48px（§5 触控） */
  .dock :global(.btn) {
    min-height: 48px;
  }
</style>
