import type { AllowedAction, BranchCursor, PendingKind, PendingReason } from '../api/types';
import { branchKind } from './pipeline';

/**
 * allowed_actions 渲染辅助（决策 101 / 49 / 130）。
 *
 * 前端**不做动作白名单**：后端下发什么就渲染什么。这里只做两件事：
 * ① 按分支 / 类型分组（视觉）；② 动作 → 配对端点的映射（用于发起请求）。
 * 未知 side_effect 动作渲染为禁用，并提示"无配对端点"（决策 101 的兜底）。
 */

export interface ActionEndpoint {
  /** `POST /tasks/{id}/...` 的后缀。 */
  path: string;
  method: 'POST';
}

export function endpointFor(pendingType: PendingKind | undefined, action: string): ActionEndpoint | null {
  switch (action) {
    case 'cancel':
      return { path: '/cancel', method: 'POST' };
    case 'split_task':
      return { path: '/split', method: 'POST' };
    case 'model_override':
      return { path: '/model-override', method: 'POST' };
    case 'approve':
    case 'return':
      return pendingType === 'human_review'
        ? { path: '/review', method: 'POST' }
        : pendingType === 'merge_approval'
          ? { path: '/merge/decision', method: 'POST' }
          : null;
    case 'reject':
      return pendingType === 'human_review' ? { path: '/review', method: 'POST' } : null;
    default:
      return null;
  }
}

export interface ActionBranchGroup {
  /** 归属游标（多游标时用于取 cursor_id，决策 91）。 */
  cursorId: string;
  branch: string;
  branchKind: 'main' | 'dev' | 'test';
  pendingReason: PendingReason | null;
  /** resume 类（continue / skip / goto）。 */
  resume: AllowedAction[];
  /** 旁路动作（cancel / split_task / model_override / approve / return / reject）——弱化。 */
  sideEffect: AllowedAction[];
  /** 纯等待（渲染为禁用）。 */
  wait: AllowedAction[];
  actions: AllowedAction[];
}

/** 按分支分组的动作集（决策 84）。 */
export function groupActionsByBranch(
  actions: AllowedAction[],
  cursors: BranchCursor[],
): ActionBranchGroup[] {
  const byCursor = new Map(cursors.map((c) => [c.cursor_id, c]));
  const groups = new Map<string, ActionBranchGroup>();

  for (const action of actions) {
    const cursor = action.cursor_id ? byCursor.get(action.cursor_id) : undefined;
    const key = action.cursor_id ?? '';
    if (!groups.has(key)) {
      groups.set(key, {
        cursorId: action.cursor_id ?? '',
        branch: cursor?.branch ?? 'main',
        branchKind: branchKind(cursor?.branch ?? 'main'),
        pendingReason: cursor?.pending_reason ?? null,
        resume: [],
        sideEffect: [],
        wait: [],
        actions: [],
      });
    }
    const group = groups.get(key)!;
    group.actions.push(action);
    if (action.kind === 'resume') group.resume.push(action);
    else if (action.kind === 'side_effect') group.sideEffect.push(action);
    else group.wait.push(action);
  }

  return [...groups.values()];
}

/** 该 side_effect 动作是否可点击（有配对端点）。 */
export function sideEffectEnabled(action: AllowedAction, pendingType: PendingKind | undefined): boolean {
  if (action.kind !== 'side_effect') return true;
  return endpointFor(pendingType, action.action) !== null;
}

/** 只有 info_insufficient 的 resume 动作有自由输入（决策 79）。 */
export function allowsFreeInput(action: AllowedAction): boolean {
  return action.kind === 'resume' && action.requires_input === true;
}

/**
 * 动作的身份（**同名动作不是同一个动作**）。
 *
 * 为什么必须有这把尺子：`allowed_actions` 里同名动作可以合法地出现两次，两条的**落点不同**——
 * `retry_exhausted@develop|test` 是「重试执行」+「带失败摘要回架构设计修订」（两个 `goto`），
 * `user_decision@test_code_issue|gate_recheck` 是「修改测试用例」+「修改业务代码」（同样是两个
 * `goto`）。只看 `action` + `cursor_id` 会把它们认成同一个：
 *   ① 渲染层 `{#each ... (key)}` 撞 key → Svelte 抛 `each_key_duplicate`，**整块动作区不再更新**，
 *      坞里留着上一个 pending 的按钮（点下去发的是别的动作）；
 *   ② 「提交中」态同时点亮两颗钮（`busyKey` 相同）。
 * 落点（stage / node）进身份，两条 `goto` 就各是各的。没有落点的动作（`continue` / `skip` /
 * side_effect）行为不变，仍是「动作名 + 游标」。
 *
 * 游标取 `action.cursor_id`，缺省时回退到调用方给的所属分支游标（决策 91）；两者都没有时
 * 用空串——`groupActionsByBranch` 的分组也把这批动作归在同一个不具名组里。
 */
export function actionKey(action: AllowedAction, cursorId?: string): string {
  const target = action.target;
  return [
    action.action,
    action.cursor_id ?? cursorId ?? '',
    target?.stage ?? '',
    target?.node ?? '',
  ].join(':');
}
