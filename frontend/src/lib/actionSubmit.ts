import type { AllowedAction, PendingKind } from '../api/types';
import { cancelTask, mergeDecision, resumeTask, reviewTask } from '../api/client';

export interface SubmitOptions {
  /** 所属游标（决策 91：从所属分支药丸取）。 */
  cursorId?: string;
  input?: string;
  /** 该动作所属 pending 的类型（用于 endpoint 行内解析，决策 130）。 */
  pendingType?: PendingKind;
}

/**
 * 执行一个 allowed_action：resume 类 → `POST /resume`，side_effect 类 → 配对端点。
 *
 * 决策 101/119：前端不做白名单，但每个 side_effect 必须有配对端点，否则抛错
 * （UI 侧已渲染为禁用并提示"无配对端点"）。
 */
export async function submitAllowedAction(
  taskId: string,
  action: AllowedAction,
  options: SubmitOptions = {},
): Promise<void> {
  if (action.kind === 'wait') return;
  const cursorId = action.cursor_id ?? options.cursorId;

  if (action.kind === 'resume') {
    await resumeTask(taskId, {
      action: action.action,
      cursor_id: cursorId || undefined,
      target_stage: action.target?.stage,
      target_node: action.target?.node,
      input: options.input,
    });
    return;
  }

  const ptype = options.pendingType;
  switch (action.action) {
    case 'cancel':
      await cancelTask(taskId);
      return;
    case 'split_task':
      throw new Error('split_task 需要提供拆分方案（请用拆分对话框）');
    case 'model_override':
      throw new Error('model_override 需要选择 provider（请用换模型对话框）');
    case 'approve':
      if (ptype === 'human_review') return void (await reviewTask(taskId, true, options.input));
      if (ptype === 'merge_approval') return void (await mergeDecision(taskId, 'approve'));
      throw new Error(`动作 approve 在 ${ptype ?? '未知'} 下无配对端点`);
    case 'return':
      if (ptype === 'merge_approval') return void (await mergeDecision(taskId, 'return'));
      throw new Error('动作 return 仅在 merge_approval 下有配对端点');
    case 'reject':
      if (ptype === 'human_review') return void (await reviewTask(taskId, false, options.input));
      throw new Error('动作 reject 仅在 human_review 下有配对端点');
    default:
      throw new Error(`动作 ${action.action} 无配对端点（决策 101）`);
  }
}
