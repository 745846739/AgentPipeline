import type { NodeCommand } from '../api/types';

/**
 * 命令页签的过滤（spec list-windowing 票 02）：命令行关键词 + 退出码档位。
 *
 * 判据住在这里而不是组件里，因为「哪一行该留下」是规格不是排版。与切片的叠加口径是
 * **先过滤后切**（调用点把本函数的输出喂 `windowSlice`），故过滤后再显尾部 50 条。
 *
 * 关键词对**两条串**都搜：折叠行显示的原串（`original_command ?? command`，决策 297）
 * 与实际执行的那条——排障的人两条都可能记得。大小写不敏感；空白串不过滤。
 */
export type ExitFilter = 'all' | 'nonzero' | 'zero';

export function filterCommands(
  commands: readonly NodeCommand[],
  keyword: string,
  exit: ExitFilter,
): NodeCommand[] {
  const q = keyword.trim().toLowerCase();
  return commands.filter((c) => {
    if (exit === 'nonzero' && !(c.exit_code !== null && c.exit_code !== 0)) return false;
    if (exit === 'zero' && c.exit_code !== 0) return false;
    if (q === '') return true;
    const hay = `${c.original_command ?? ''}\n${c.command}`.toLowerCase();
    return hay.includes(q);
  });
}
