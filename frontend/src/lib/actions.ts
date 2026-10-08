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

/**
 * 不可逆动作的档位判据（票 21 / 决策 216①⑥；2026-10-01 用户指示落地原 wontfix 票 03）。
 *
 * **不靠标签文字匹配**——只认 `action` 名与 `kind` / `requires_input` 结构字段：
 * - `destructive`：物理上回不去的（终结任务 `cancel`；合入 `merge`——写进 `default_branch`
 *   即终局，决策 6「无远程 PR」；让配对设备失效 `reset_pairing`）。红描边 `--stop` + 内联两步确认。
 * - `gate-skip`：跳过质量闸（决策 216① 原文判据：`resume` 且（`skip` 或（`continue` 且非
 *   `requires_input`）））。琥珀描边 `--pending`，不再用实心 + 同一条确认。
 * - `advance`：流水线的自然下一步（`approve` / `return` 评审、带输入的 `continue`、`goto`、
 *   `retry` 等）。实心，无确认步。
 * - `quiet`：其余（`split_task` / `model_override` 等旁路）。`.btn.quiet`。
 *
 * 一处原文张力，按 ① 与末句收口：决策 216⑥ 的举例把 `合入` 列在「推进 = 实心」里，而 ①(a)
 * 判它必须有确认步、⑥末句明写「同一个动作只有一档（不存在实心 + 确认步）」——两处冲突时
 * 取 ①（判据正文）与末句（互斥律），故 `merge` 归 `destructive`，量级与确认步同档。
 */
export type ActionTier = 'advance' | 'gate-skip' | 'destructive' | 'quiet';

/**
 * `pendingType` 决定同名动作的落点：`approve`@`merge_approval` 是「写进项目仓库」
 * （合入，决策 216①a → destructive），`approve`@`human_review` 是「通过评审」
 * （推进 → advance）。缺省（拿不到 pendingType）按推进处理，宁可少一层确认也不误判终结。
 */
export function actionTier(action: AllowedAction, pendingType?: PendingKind): ActionTier {
  switch (action.action) {
    case 'merge':
    case 'cancel':
    case 'reset_pairing':
      return 'destructive';
    case 'approve':
      return pendingType === 'merge_approval' ? 'destructive' : 'advance';
    case 'return':
      // §9.3 表：`返回修改` 列在弱化旁路 → `.btn.quiet`，不进确认步（决策 216⑤）
      return 'quiet';
    case 'skip':
      return action.kind === 'resume' ? 'gate-skip' : 'quiet';
    case 'continue':
      return action.kind === 'resume' && action.requires_input !== true ? 'gate-skip' : 'advance';
    case 'goto':
      return 'advance';
    default:
      return action.kind === 'resume' ? 'advance' : 'quiet';
  }
}

/** 后果句的运行时读数（票 21：取不到就不写数，不留半句 `…到 ？`）。 */
export interface ConfirmContext {
  /** 合入目标分支（`Project.default_branch`）。 */
  defaultBranch?: string | null;
  /** 已配对设备台数（`重置配对`）。 */
  pairedCount?: number | null;
}

/**
 * 确认步的后果句（决策 216③，逐条写死）：只在动手那一步出现，常驻处不摆。
 * 返回 `null` = 这个动作没有确认步。
 */
export function confirmSentence(
  action: AllowedAction,
  pendingType?: PendingKind,
  ctx: ConfirmContext = {},
): string | null {
  switch (actionTier(action, pendingType)) {
    case 'destructive':
      if (action.action === 'cancel') return '确认终止？任务会停在当前节点不再推进';
      if (action.action === 'reset_pairing') {
        const n = ctx.pairedCount;
        return n == null || n <= 0
          ? '确认重置？已配对的设备要重新扫码'
          : `确认重置？${n} 台已配对的设备要重新扫码`;
      }
      return ctx.defaultBranch ? `确认合入到 ${ctx.defaultBranch}？` : '确认合入？';
    case 'gate-skip':
      return '确认跳过评审闸门？';
    default:
      return null;
  }
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
