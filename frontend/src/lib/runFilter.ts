import type { ConversationSummary } from '../api/types';

/**
 * run 药丸行的过滤（spec list-windowing 票 03）。
 *
 * 关键词做大小写不敏感的子串匹配，命中面 = 药丸上**看得见的字**（阶段、节点、尝试
 * 次数、子代理类型、run id）**加票 03 点名的状态维**（`status` 不印在药丸上，但
 * 「timeout / failed 的那几轮」正是要捞的）——判据住在这里不是组件里，因为「哪颗
 * 药丸留下」是规格。
 */
export function filterRuns(
  runs: readonly ConversationSummary[],
  query: string,
): ConversationSummary[] {
  const q = query.trim().toLowerCase();
  if (q === '') return [...runs];
  return runs.filter((r) =>
    `${r.stage} ${r.node} ${r.attempt} ${r.agent_type} ${r.run_id} ${r.status}`.toLowerCase().includes(q),
  );
}
