import { describe, expect, it } from 'vitest';
import type { ConversationSummary } from '../api/types';
import { filterRuns } from './runFilter';

/**
 * run 药丸行的过滤纯函数（spec list-windowing 票 03）：几十上百轮 run 的药丸墙
 * 一次性摆出来没法找——关键词对 run 的可读字段做大小写不敏感的子串匹配。
 */
function run(overrides: Partial<ConversationSummary> = {}): ConversationSummary {
  return {
    run_id: 7,
    stage: 'develop',
    node: 'execute',
    attempt: 1,
    agent_type: 'main',
    parent_run_id: null,
    prompt_tokens: 100,
    completion_tokens: 50,
    ...overrides,
  };
}

describe('filterRuns：run 药丸过滤（票 03）', () => {
  const rows = [
    run({ run_id: 1, stage: 'architect-design', node: 'validate_input' }),
    run({ run_id: 14, node: 'execute', agent_type: 'worker' }),
    run({ run_id: 23, stage: 'test-design', node: 'validate_output', attempt: 2 }),
  ];

  it('按阶段 / 节点 / 子代理 / run id 匹配，大小写不敏感', () => {
    expect(filterRuns(rows, 'architect').map((r) => r.run_id)).toEqual([1]);
    expect(filterRuns(rows, 'execute').map((r) => r.run_id)).toEqual([14]);
    expect(filterRuns(rows, 'worker').map((r) => r.run_id)).toEqual([14]);
    expect(filterRuns(rows, '23').map((r) => r.run_id)).toEqual([23]);
    expect(filterRuns(rows, 'VALIDATE_OUTPUT').map((r) => r.run_id)).toEqual([23]);
  });

  it('尝试次数也搜得到（药丸上看得见的「尝试 N」）', () => {
    expect(filterRuns(rows, '2').map((r) => r.run_id)).toEqual([23]);
  });

  it('空查询：不过滤', () => {
    expect(filterRuns(rows, '  ')).toHaveLength(3);
  });

  it('没命中：空名单（调用点据此摆「没有匹配」的提示）', () => {
    expect(filterRuns(rows, 'merge')).toEqual([]);
  });
});
