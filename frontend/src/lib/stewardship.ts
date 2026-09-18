import type { Task, TaskStatus } from '../api/types';

/**
 * 托管开关的判据（决策 210① / 票 08 的端点、票 14 的界面）。
 *
 * 判据住在这里（纯逻辑 + 注入出口）而不是组件里，是为了能直接钉住两件事：
 * **什么情况下这颗钮根本不该摆**，以及**拨一下之后说什么**。
 *
 * 「不该摆」的两条都来自端点的拒绝语义（`crates/app/src/routes/tasks.rs::set_stewardship`）：
 * 终态任务（400——托管随任务自限）与值班长未接线（503——没人会用那份授权）。
 * 一颗摆出来必然报错的钮是噪声，故这里先判掉，而不是让人按完再读错误。
 */

/** 自动 `resume` 的次数上限（决策 210⑨：N = 2）。与 `types.rs::STEWARDSHIP_MAX_AUTO_RESUMES` 同源。 */
export const STEWARDSHIP_MAX_AUTO_RESUMES = 2;

/** 终态任务上没有可托管的下一步（与后端 `TaskStatus::is_terminal` 同一集合）。 */
const TERMINAL_STATUSES: TaskStatus[] = ['done', 'failed', 'cancelled'];

/** 托管开关在界面上的一面；`null` = 摆不出来。 */
export interface StewardshipFace {
  /** 现在是开着还是关着。 */
  enabled: boolean;
  /** 状态那一句（开关旁边那一行说明）。 */
  note: string;
  /** 按钮上的词。 */
  label: string;
}

/**
 * 这个任务的托管开关该长什么样。
 *
 * `foremanWired` 是 `/foreman/sessions` 读回来的接线状态——未接线时那颗钮无处可去。
 */
export function stewardshipFace(
  task: Pick<Task, 'status' | 'stewardship'>,
  foremanWired: boolean,
): StewardshipFace | null {
  if (!foremanWired) return null;
  if (TERMINAL_STATUSES.includes(task.status)) return null;
  const stewardship = task.stewardship;
  if (stewardship?.enabled !== true) {
    return {
      enabled: false,
      label: '托管：关',
      note: '值班长对这条任务只能提议，动手要你按键',
    };
  }
  const left = Math.max(0, STEWARDSHIP_MAX_AUTO_RESUMES - stewardship.auto_resumes);
  return {
    enabled: true,
    label: '托管：开',
    note:
      left > 0
        ? `值班长可免按键 resume(continue)，还剩 ${left} 次自动动作`
        : `自动动作已用满 ${STEWARDSHIP_MAX_AUTO_RESUMES} 次，它现在只能提议`,
  };
}

/** 拨这一下要的出口（生产是 `api/client.ts::setStewardship`）。 */
export interface StewardshipDeps {
  set(id: string, enabled: boolean): Promise<unknown>;
}

export type StewardshipToggleResult =
  | { ok: true; note: string }
  | { ok: false; message: string };

/**
 * 拨一下，并按端点的回答给出结果。
 *
 * 拒绝的两句报文本身就是给人读的（终态 / 未接线），故**原样透传**而不在这里另写一套说法：
 * 两处各写一份必然漂移，而漂移之后界面说的与后端拒的原因就不是一件事了。
 */
export async function toggleStewardship(
  taskId: string,
  enabled: boolean,
  deps: StewardshipDeps,
): Promise<StewardshipToggleResult> {
  try {
    await deps.set(taskId, enabled);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    return {
      ok: false,
      message: message.trim() === '' ? '托管开关没能提交（原因未知）' : message,
    };
  }
  return {
    ok: true,
    note: enabled
      ? '已打开托管：值班长对这条任务可免按键 resume(continue)——满 2 次或指纹相同即停手'
      : '已关掉托管：值班长对这条任务恢复为只能提议',
  };
}
