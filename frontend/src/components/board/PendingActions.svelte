<script module lang="ts">
  import type { BranchCursor as BC } from '../../api/types';

  /** 仅为未找到游标时的兜底展示（不应发生——后端总是给 cursor_id）。 */
  export function syntheticCursor(cursorId: string, branch: string): BC {
    return {
      cursor_id: cursorId,
      branch,
      stage: 'init',
      node: 'execute',
      status: 'pending',
      validate_attempts: 0,
      skipped_to_join: false,
      pending_reason: null,
    };
  }
</script>

<script lang="ts">
  import type { AllowedAction, BranchCursor, PendingKind } from '../../api/types';
  import {
    actionKey,
    actionTier,
    allowsFreeInput,
    confirmSentence,
    groupActionsByBranch,
    sideEffectEnabled,
  } from '../../lib/actions';
  import { branchKind } from '../../lib/pipeline';

  interface Props {
    actions: AllowedAction[];
    cursors: BranchCursor[];
    /** 该任务 pending 原因类型（用于 side_effect 端点解析）。 */
    pendingType?: PendingKind;
    /** 合入确认句里的目标分支（决策 216③；取不到就不写数）。 */
    defaultBranch?: string | null;
    /** 提交回调：cursor_id 从所属分支药丸取（决策 91）。 */
    onaction?: (action: AllowedAction, opts: { cursorId?: string; input?: string }) => void;
    isBusy?: (action: AllowedAction, cursorId?: string) => boolean;
    disabled?: boolean;
  }

  let {
    actions,
    cursors,
    pendingType,
    defaultBranch = null,
    onaction,
    isBusy,
    disabled = false,
  }: Props = $props();

  let inputs = $state<Record<string, string>>({});
  let selectedCursor = $state<string | null>(null);
  /** 内联两步确认的第一步：哪颗钮已进确认态（决策 216②；null = 都没有）。 */
  let confirming = $state<string | null>(null);

  const groups = $derived(groupActionsByBranch(actions, cursors));

  function fallbackCursorId(groupCursorId: string): string | undefined {
    if (groupCursorId) return groupCursorId;
    if (selectedCursor) return selectedCursor;
    if (cursors.length === 1) return cursors[0].cursor_id;
    return undefined;
  }

  /** 与 submit 同一把 key 尺子——确认态和提交必须对到同一颗钮上。 */
  function keyOf(action: AllowedAction, groupCursorId: string): string {
    return actionKey(action, fallbackCursorId(groupCursorId));
  }

  /** 该动作的确认句（null = 无确认步，点一下就发）。 */
  function sentence(action: AllowedAction): string | null {
    return confirmSentence(action, pendingType, { defaultBranch });
  }

  /** 三档量级的渲染类（决策 216⑥：advance 实心 / gate-skip 琥珀描边 / destructive 红描边 / 其余 quiet）。 */
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

  function submit(action: AllowedAction, groupCursorId: string) {
    const key = keyOf(action, groupCursorId);
    // 第一步只亮确认态、不发请求；第二步（同一颗钮再点）才真提交（票 21 单测口径）
    if (sentence(action) !== null && confirming !== key) {
      confirming = key;
      return;
    }
    confirming = null;
    const cursorId = fallbackCursorId(groupCursorId);
    const input = inputs[key];
    onaction?.(action, { cursorId, input: input?.trim() ? input : undefined });
  }

  /** Escape 从确认态退回普通态（决策 216④）；焦点不移动，仍在刚点的那颗钮上。 */
  function onKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape' && confirming !== null) {
      event.stopPropagation();
      confirming = null;
    }
  }

  /** 这颗钮当前是否在确认态（就地换行文案，不换钮——焦点因此留在原地）。 */
  function armed(action: AllowedAction, groupCursorId: string): boolean {
    return confirming !== null && confirming === keyOf(action, groupCursorId) && sentence(action) !== null;
  }

  /**
   * 动作身份串——判「是不是换了新一轮待办」用。
   *
   * **不能按 `actions` 的数组身份清零**：SSE / 轮询每次对齐都换一份新数组，同内容刷新也会
   * 触发下面那支 `$effect`，把用户刚点亮的确认态在两次点击之间抹掉——两步确认（决策 216②）
   * 于是成了「点两下也不提交」（合入 / 跳闸档实测如此）。动作身份没变就不该动它。
   */
  const actionIdentity = $derived(actions.map((a) => actionKey(a, a.cursor_id)).join('|'));

  // 动作集换了（上一个 pending 走了）就退回普通态——不让上一轮的确认态挂到新一轮的钮上
  $effect(() => {
    void actionIdentity;
    confirming = null;
  });

  function busy(action: AllowedAction, groupCursorId: string): boolean {
    return isBusy?.(action, fallbackCursorId(groupCursorId)) ?? false;
  }

  /**
   * 分支文字标签（决策 169 对决策 84 的手段变更）：分支身份 = 徽章色相
   * （--branch-dev 蓝 / --branch-tst 紫）+ 文字双编码 `[dev]`/`[tst]`；
   * main 中性。色相不单独承载语义，仍与文字一起出现。
   */
  function branchLabel(branch: string): string {
    const kind = branchKind(branch);
    return kind === 'main' ? 'main' : `[${kind}]`;
  }

  /** 组头分支色相类（与 BranchPill 同一套 token）。 */
  function branchClass(branch: string): string {
    return branchKind(branch);
  }
</script>

{#if !actions || actions.length === 0}
  <div class="none">当前没有可下发的动作。</div>
{:else}
  {#each groups as group, gi (group.cursorId || `g${gi}`)}
    <div class="group">
      {#if groups.length > 1 || group.cursorId}
        <div class="head">
          <span class="head-label {branchClass(group.branch)}">{branchLabel(group.branch)}</span>
        </div>
      {/if}

      {#if !group.cursorId && cursors.length > 1}
        <!-- 多游标且动作未带 cursor_id：显式选择所属分支（决策 91） -->
        <label class="picker">
          <span class="grp-label">选择游标</span>
          <select class="input" bind:value={selectedCursor}>
            <option value={null}>—</option>
            {#each cursors as c (c.cursor_id)}
              <option value={c.cursor_id}>{c.branch} · {c.node}</option>
            {/each}
          </select>
        </label>
      {/if}

      {#if group.resume.length > 0}
        <div class="grp-label">恢复动作</div>
        {#each group.resume as action (actionKey(action, group.cursorId))}
          <div class="item">
            {#if allowsFreeInput(action)}
              <textarea
                class="input"
                rows="2"
                placeholder="补充说明后继续…"
                value={inputs[actionKey(action, group.cursorId)] ?? ''}
                oninput={(e) =>
                  (inputs[actionKey(action, group.cursorId)] = (e.currentTarget as HTMLTextAreaElement).value)}
              ></textarea>
            {/if}
            {#if armed(action, group.cursorId)}
              <!-- 决策 216②：动作行就地换成后果句（12px --text-3）+ 同一颗钮 + 紧邻一颗取消 -->
              <span class="confirm-q">{sentence(action)}</span>
            {/if}
            <button
              type="button"
              class="{tierClass(action)} block"
              disabled={disabled || busy(action, group.cursorId)}
              onclick={() => submit(action, group.cursorId)}
              onkeydown={onKeydown}
            >
              {#if busy(action, group.cursorId)}<span class="spin"></span>{/if}
              {action.label}
            </button>
            {#if armed(action, group.cursorId)}
              <button type="button" class="btn quiet block" onclick={() => (confirming = null)}>
                取消
              </button>
            {/if}
          </div>
        {/each}
      {/if}

      {#if group.sideEffect.length > 0}
        <div class="grp-label">旁路动作</div>
        {#each group.sideEffect as action (actionKey(action, group.cursorId))}
          <!-- 后果句与取消是按钮的兄弟节点：按钮本身不换节点，焦点留在原地（决策 216④） -->
          {#if armed(action, group.cursorId)}
            <span class="confirm-q">{sentence(action)}</span>
          {/if}
          <button
            type="button"
            class="{tierClass(action)} block add"
            disabled={disabled || !sideEffectEnabled(action, pendingType) || busy(action, group.cursorId)}
            title={!sideEffectEnabled(action, pendingType) ? '这个动作没有配对的端点' : action.label}
            onclick={() => submit(action, group.cursorId)}
            onkeydown={onKeydown}
          >
            {action.label}
          </button>
          {#if armed(action, group.cursorId)}
            <button type="button" class="btn quiet block" onclick={() => (confirming = null)}>
              取消
            </button>
          {/if}
        {/each}
      {/if}

      {#if group.wait.length > 0}
        {#each group.wait as action (actionKey(action, group.cursorId))}
          <button type="button" class="btn quiet block" disabled title="纯等待，无系统变更">
            {action.label}
          </button>
        {/each}
      {/if}
    </div>
  {/each}
{/if}

<style>
  .group {
    margin-bottom: 6px;
  }
  .head {
    margin-bottom: 6px;
  }
  .head-label {
    font-size: 12px;
    letter-spacing: 0.04em;
    color: var(--text-3);
  }
  /* 分支身份 = 徽章色相（与 BranchPill 同一套 token，决策 169） */
  .head-label.dev {
    color: var(--branch-dev);
  }
  .head-label.test {
    color: var(--branch-tst);
  }
  .grp-label {
    /* 分组名：冻结原型 `.grp` 就是装饰档，决策 195 的归位清单里也没有它——保持不动 */
    font-size: 12px;
    color: var(--text-4);
    letter-spacing: 0.08em;
    margin: 12px 0 7px;
  }
  .item {
    margin-bottom: 6px;
  }
  /* 确认步后果句（决策 216②：12px --text-3，就地出现，常驻处不摆） */
  .confirm-q {
    display: block;
    font-size: 12px;
    color: var(--text-3);
    margin: 2px 0 5px;
  }
  .item textarea {
    margin-bottom: 5px;
  }
  .add {
    margin-bottom: 5px;
  }
  .picker {
    display: block;
    margin-bottom: 6px;
  }
  .none {
    font-size: 12px;
    color: var(--text-3);
  }
</style>
