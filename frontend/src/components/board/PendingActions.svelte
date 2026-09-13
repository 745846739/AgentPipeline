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
  import { allowsFreeInput, groupActionsByBranch, sideEffectEnabled } from '../../lib/actions';
  import { branchKind } from '../../lib/pipeline';

  interface Props {
    actions: AllowedAction[];
    cursors: BranchCursor[];
    /** 该任务 pending 原因类型（用于 side_effect 端点解析）。 */
    pendingType?: PendingKind;
    /** 提交回调：cursor_id 从所属分支药丸取（决策 91）。 */
    onaction?: (action: AllowedAction, opts: { cursorId?: string; input?: string }) => void;
    isBusy?: (action: AllowedAction, cursorId?: string) => boolean;
    disabled?: boolean;
  }

  let { actions, cursors, pendingType, onaction, isBusy, disabled = false }: Props = $props();

  let inputs = $state<Record<string, string>>({});
  let selectedCursor = $state<string | null>(null);

  const groups = $derived(groupActionsByBranch(actions, cursors));

  function fallbackCursorId(groupCursorId: string): string | undefined {
    if (groupCursorId) return groupCursorId;
    if (selectedCursor) return selectedCursor;
    if (cursors.length === 1) return cursors[0].cursor_id;
    return undefined;
  }

  function submit(action: AllowedAction, groupCursorId: string) {
    const cursorId = fallbackCursorId(groupCursorId);
    const input = inputs[actionKey(action, cursorId)];
    onaction?.(action, { cursorId, input: input?.trim() ? input : undefined });
  }

  function actionKey(action: AllowedAction, cursorId?: string): string {
    return `${action.action}:${action.cursor_id ?? cursorId ?? ''}`;
  }

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
        {#each group.resume as action (action.action + (action.cursor_id ?? ''))}
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
            <button
              type="button"
              class="btn solid block"
              disabled={disabled || busy(action, group.cursorId)}
              onclick={() => submit(action, group.cursorId)}
            >
              {#if busy(action, group.cursorId)}<span class="spin"></span>{/if}
              {action.label}
            </button>
          </div>
        {/each}
      {/if}

      {#if group.sideEffect.length > 0}
        <div class="grp-label">旁路动作</div>
        {#each group.sideEffect as action (action.action + (action.cursor_id ?? ''))}
          <button
            type="button"
            class="btn quiet block add"
            disabled={disabled || !sideEffectEnabled(action, pendingType) || busy(action, group.cursorId)}
            title={!sideEffectEnabled(action, pendingType) ? '无配对端点（决策 101）' : action.label}
            onclick={() => submit(action, group.cursorId)}
          >
            {action.label}
          </button>
        {/each}
      {/if}

      {#if group.wait.length > 0}
        {#each group.wait as action (action.action)}
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
    font-size: 12px;
    color: var(--text-4);
    letter-spacing: 0.08em;
    margin: 12px 0 7px;
  }
  .item {
    margin-bottom: 6px;
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
    font-size: 11.5px;
    color: var(--text-3);
  }
</style>
