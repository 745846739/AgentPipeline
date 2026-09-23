import { getForemanToolLabels } from '../api/client';

/**
 * 工具回执标签的取数与查表（决策 247⑤）。
 *
 * 后端是**唯一事实源**：`ForemanToolSpec.label` 是必填字段，加工具不写标签直接编译不过——
 * 前端不再各持一份手抄表（Talk 那张 18 键的 `TOOL_LABELS` 已删：它缺 4 个词、还带着死键
 * `delete_file`，而 `proposalToolLabel` 那 8 个 case 与它已经打架）。
 *
 * **取数一次、模块级缓存**：清单在进程生命周期里不变，每页每挂载都问一跳后端是白付的。
 * **失败不缓存**——首启时后端还没起完的那一跳瞬时失败，不该让标签永久退回英文原名，
 * 下一次调用照常重试。
 */
let inflight: Promise<Record<string, string>> | null = null;

export function loadToolLabels(): Promise<Record<string, string>> {
  if (!inflight) {
    inflight = getForemanToolLabels()
      .then((r) => Object.fromEntries(r.tools.map((t) => [t.name, t.label])))
      .catch((e: unknown) => {
        inflight = null;
        throw e;
      });
  }
  return inflight;
}

/**
 * 工具名 → 中文词（实时那一栏、回执那一栏、提议徽章都走它）。
 *
 * 认不出的值**原样显示工具名**，不兜底成别的词——那个兜底会把「值班长调了个界面还不认识的
 * 新工具」说成一件它没做的事（决策 200 的平实口径）。清单外的 `spawn_sub_agent` 照旧显原名，
 * 那是有 e2e 钉住的刻意行为。
 */
export function labelFor(labels: Record<string, string>, tool: string): string {
  return labels[tool] ?? tool;
}
