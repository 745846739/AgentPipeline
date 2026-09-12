import type { AllowedAction, PendingKind } from '../api/types';
import { cancelTask, mergeDecision, resumeTask, reviewTask } from '../api/client';
import { endpointFor } from './actions';

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
 *
 * **单一事实来源**（票 03）：`action → endpoint` 的判定只在 `actions.ts::endpointFor`
 * 一处，本函数按解析出的路径分派到对应 client 调用，不再各写一份 switch。
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

  const endpoint = endpointFor(options.pendingType, action.action);
  if (endpoint === null) {
    throw new Error(`动作 ${action.action} 无配对端点（决策 101）`);
  }

  switch (endpoint.path) {
    case '/cancel':
      await cancelTask(taskId);
      return;
    case '/split':
      throw new Error('split_task 需要提供拆分方案（请用拆分对话框）');
    case '/model-override':
      throw new Error('model_override 需要选择 provider（请用换模型对话框）');
    case '/review': {
      // human_review 下 approve / reject 共用 /review，以动作名区分结论（决策 23）
      await reviewTask(taskId, action.action === 'approve', options.input);
      return;
    }
    case '/merge/decision': {
      await mergeDecision(taskId, action.action === 'approve' ? 'approve' : 'return');
      return;
    }
    default:
      throw new Error(`动作 ${action.action} 的端点 ${endpoint.path} 未接线（决策 101）`);
  }
}
