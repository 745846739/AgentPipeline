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
