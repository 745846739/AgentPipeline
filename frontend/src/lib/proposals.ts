/**
 * 提议轮的渲染判据（决策 188 / 207，票 03）。全部判据在这里，组件里那段 `{#if}` 只有渲染——
 * 「哪一轮有钮 / 钮灰不灰 / 这件事要不要指路」于是能被 L1 逐条钉住，而不必开浏览器。
 *
 * ## 三条规矩
 *
 * **① 过期由前端按 `expires_at` 自己算。** 后端那份 `status` 是权威终态，但它不会「到点」
 * 主动改一行——到期清理挂在每小时的维护作业上（决策 207），等它把 `pending` 改成 `expired`
 * 会让按钮在过期后还亮着最长一小时。表格判据取「`status === 'pending' && now >= expires_at`」，
 * 与后端 `ForemanProposal::is_expired` 是同一条。
 *
 * **② 过期**只让按钮变灰，那一轮**留在时间线**（决策 207）：审计要的是「它当时提议过什么」，
 * 与 `briefing_json` / `traces_json` 同一理由。同一轮里执行过 / 被拒绝的照旧在。
 *
 * **③ 同一个动作已经在状态区的急停轮里有一颗钮 → 提议只指路、不画第二颗**（决策 207③，
 * 保留决策 176④ 的原顾虑）。判据是「提议要做的那件事，在**那个**任务的 `allowed_actions`
 * 里已经有了」——两处各一颗钮会让「哪颗是真的」变成用户的问题，而它们是同一个端点。
 */

import type { AllowedAction, ForemanProposal } from '../api/types';

/** 提议轮在时间线上的形态。 */
export type ProposalState = 'pending' | 'expired' | 'executed' | 'rejected';

/**
 * 这一条提议此刻**看起来**是什么状态。
 *
 * `status` 之外的唯一加工是过期：`pending` 且已过 `expires_at` → `expired`。
 * 其余三种（执行过 / 被拒绝 / 后端已标过期）原样透过——前端不猜后端已经判过的事。
 */
export function proposalState(p: ForemanProposal, now: number): ProposalState {
  if (p.status === 'pending') return expiryReached(p, now) ? 'expired' : 'pending';
  if (p.status === 'executed' || p.status === 'rejected' || p.status === 'expired') {
    return p.status;
  }
  // 认不出的 status（后端将来加了第五种）：当成「不可按」处理，不假装它是 pending。
  return 'expired';
}

/** 到期了吗。坏时间戳（解析不出）**算过期**——宁可让人重新提一条，不可让一颗按不动的钮亮着。 */
export function expiryReached(p: ForemanProposal, now: number): boolean {
  const at = Date.parse(p.expires_at);
  if (Number.isNaN(at)) return true;
  return now >= at;
}

/** 这一刻可以按键吗（未决且未过期）。 */
export function proposalActionable(p: ForemanProposal, now: number): boolean {
  return proposalState(p, now) === 'pending';
}

/**
 * 这一条提议要不要**指路**（而不是画钮）：同一个动作在状态区那颗急停轮里已经有钮了。
 *
 * `actions` 是那个任务的 `allowed_actions`（`undefined` = 详情还没到）。**详情没到时不指路**
 * ——那会把「还没读到」说成「已经有了」，于是人哪颗钮都看不到（与 `stopActionCount` 的
 * 「没到就不报数」是同一条纪律）。
 */
export function proposalPointerOnly(
  p: ForemanProposal,
  actions: readonly AllowedAction[] | undefined,
): boolean {
  if (!actions || actions.length === 0) return false;
  const name = stateZoneActionName(p);
  if (!name) return false;
  return actions.some((a) => a.action === name);
}

/**
 * 这条提议对应的**状态区动作名**（`allowed_actions[].action` 那一栏），认不出来时 `null`。
 *
 * 认不出来的情况是「这件事本来就不在状态区」——建任务、改配置、装技能都没有对应的急停钮，
 * 故它们永远是「画钮」而不是「指路」。映射表写在这里而不是散在组件里：它同时是
 * `crates/core/src/actions.rs` 那张 `(PendingKind, action) → 端点` 表的镜像，
 * 两处不一致的表现是「明明有钮却还画了第二颗」。
 */
export function stateZoneActionName(p: ForemanProposal): string | null {
  if (p.tool !== 'task') return null;
  const args = p.args as Record<string, unknown>;
  const action = typeof args?.action === 'string' ? args.action : '';
  switch (action) {
    case 'cancel':
      return 'cancel';
    case 'review':
      // 人工评审：通过与打回（`POST /tasks/{id}/review`）。
      return args.approved === true ? 'approve' : 'reject';
    case 'merge':
      // 合入决定：`approve` 走合入，`reject` 在动作集里叫 `return`（返回修改）。
      return args.decision === 'approve' ? 'approve' : 'return';
    case 'resume':
      // 恢复动作名由模型填（`continue` / `skip` / …），与动作集里的名字同一套词表。
      return typeof args.resume_action === 'string' ? args.resume_action : null;
    default:
      return null;
  }
}

/**
 * 这一条提议属于哪个任务（`null` = 与任务无关：文件 / 命令 / 建任务 / 改配置 / 装技能）。
 *
 * 用来在状态区那一摞急停里找它的 `allowed_actions`——找不着就只能画钮（宁可多一颗，
 * 不能让人无处可按）。
 */
export function proposalTaskId(p: ForemanProposal): string | null {
  const args = p.args as Record<string, unknown>;
  const id = args?.task_id;
  return typeof id === 'string' && id.length > 0 ? id : null;
}

/** 过期还剩多少（毫秒，已过期为 0）。倒计时那句话用它。 */
export function proposalRemainingMs(p: ForemanProposal, now: number): number {
  const at = Date.parse(p.expires_at);
  if (Number.isNaN(at)) return 0;
  return Math.max(0, at - now);
}

/**
 * 倒计时的人话（「还剩 7 分钟」/「已过期」）。
 *
 * 只给**分钟**这一档：提议的 TTL 是 10 分钟（决策 207），秒级数字会让人盯着它跳，
 * 而这颗钮要的是「现在还来得及吗」这一个判断。
 */
export function proposalRemainingLabel(p: ForemanProposal, now: number): string {
  const ms = proposalRemainingMs(p, now);
  if (ms <= 0) return '已过期';
  const minutes = Math.floor(ms / 60_000);
  if (minutes >= 1) return `还剩 ${minutes} 分钟`;
  return '还剩不到 1 分钟';
}

/**
 * 状态短文案（`dtag` 那一行用）。
 *
 * 与 [`proposalStateLabel`] 分开：「过期」的完整说法（「当时的情况未必还成立，要做得重新提
 * 一次」）长得放不进那一行标签，而两处都写全会让同一句话在同一轮里读两遍。
 */
export function proposalShortLabel(p: ForemanProposal, now: number): string {
  switch (proposalState(p, now)) {
    case 'pending':
      return '等你按键';
    case 'expired':
      return '已过期';
    case 'executed':
      return '执行过';
    case 'rejected':
      return '被拒绝';
  }
}

/**
 * 状态文案（时间线上那一行）。
 *
 * 「等你按键」是**未决**的说法；三种终态各自说得清做了/没做什么——被拒绝的要说清
 * 「没有执行任何动作」，否则「被拒绝」看起来像「执行失败」。
 */
export function proposalStateLabel(p: ForemanProposal, now: number): string {
  switch (proposalState(p, now)) {
    case 'pending':
      return '等你按键';
    case 'expired':
      return '已过期（当时的情况未必还成立，要做得重新提一次）';
    case 'executed':
      return '执行过';
    case 'rejected':
      return '被拒绝（没有执行任何动作）';
  }
}

/**
 * 工具名的人话（这一轮在做什么）。**基词查后端标签**（决策 247⑥，02 的取数缓存传进来）
 * ——提议徽章与回执从此说同一个词，前端不再各持一份基词镜像。
 *
 * **后缀与特例留前端**：`task` / `config` / `skills` 拼 `· action`，`service` 无后缀，
 * `repair` 是固定句——那是**说法**不是标签，后端只供标签。
 *
 * 认不出的工具名**原样显示**（`labels[tool]` 查不到时基词就是工具名本身）：模型或旧版本
 * 可能报出一个这边不认识的工具，假装认识它才是真的误导。
 */
export function proposalToolLabel(
  p: ForemanProposal,
  labels: Record<string, string> = {},
): string {
  const args = p.args as Record<string, unknown>;
  const action = typeof args?.action === 'string' ? ` · ${args.action}` : '';
  const base = labels[p.tool] ?? p.tool;
  switch (p.tool) {
    case 'task':
    case 'config':
    case 'skills':
      return `${base}${action}`;
    // 修复提议（决策 212① / 票 12）：执行的是「合入一个分支」，名牌要说出来——
    // 「合入」这两个字是人按下之前最该看见的。**不查 label**：这句话是固定的。
    case 'repair':
      return '修复 · 合入分支';
    // `write_file` / `edit_file` / `run_command` / `service` 与其余一切名字都落这里：
    // 前四个给标签（`service` 恰好无后缀），认不出的给原名。
    default:
      return base;
  }
}

/**
 * 这一条提议是不是**修复**（票 12）：它的渲染多一块（闸门读数 + diff）。
 *
 * 判据是 `kind`，不是 `tool`：`tool` 是执行分派的键（将来可能改名），而 `kind` 是载荷形态
 * 的正式说法。
 */
export function isRepairProposal(p: ForemanProposal): boolean {
  return p.kind === 'repair' && !!p.payload;
}

/**
 * 修复提议的按钮文案（票 12）：`合入` 与 `执行` 是两件事——前者会动主干。
 */
export function repairActionLabel(p: ForemanProposal): string {
  return isRepairProposal(p) ? '合入' : '执行';
}

/**
 * 闸门那一行的读数（票 12：闸门读数显示在那条提议上）。
 *
 * 没过闸门的修复**根本没有 diff**（决策 210④），所以这一行的措辞要说清「为什么没有补丁」——
 * 否则人会以为是加载失败。
 */
export function repairGateLabel(p: ForemanProposal): string | null {
  if (!isRepairProposal(p)) return null;
  const gate = p.payload?.gate ?? [];
  if (gate.length === 0) return '闸门：未记录';
  const passed = p.payload?.gate_passed ?? false;
  const steps = gate
    .map((g) => `${g.kind} ${g.exit_code === 0 ? '过' : `失败(${g.exit_code})`}`)
    .join(' / ');
  return passed ? `闸门：${steps}` : `闸门未过，没有补丁：${steps}`;
}
