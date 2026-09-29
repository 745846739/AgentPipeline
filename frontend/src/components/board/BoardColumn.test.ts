import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import type { TaskListItem } from '../../api/types';
import type { BoardColumnDef } from '../../lib/pipeline';
import BoardColumn from './BoardColumn.svelte';

/**
 * 看板列的窗口化（spec list-windowing 票 04）：列内显**头部** 50 张、其余折进
 * 「还有 M 张，加载更多」；换过滤档游标回缺省。切片判据在 `lib/windowSlice.test.ts`，
 * 按列分组（`tasksByColumn`）喂进来的每列名单各自独立切片——这里钉接线。
 */
function task(overrides: Partial<TaskListItem> = {}): TaskListItem {
  return {
    id: 't1',
    project_id: 'p1',
    title: '任务',
    description: '',
    status: 'done',
    current_stage: 'done',
    current_node: 'merge',
    validate_attempts: 0,
    pending_reason: null,
    stewardship: null,
    worktree_path: null,
    branch_name: null,
    total_tokens: 0,
    total_calls: 0,
    review_mode: 'supervised',
    model_override: null,
    archived_at: null,
    stalled: false,
    created_at: '2026-09-29T10:00:00Z',
    updated_at: '2026-09-29T10:00:00Z',
    branches: [],
    blocks: [],
    ...overrides,
  } as TaskListItem;
}

function tasks(n: number): TaskListItem[] {
  return Array.from({ length: n }, (_, i) => task({ id: `t${i + 1}`, title: `任务 ${i + 1}` }));
}

function columnProps(over: Record<string, unknown> = {}) {
  return {
    column: { key: 'done', label: '完成', stages: ['done'] } satisfies BoardColumnDef,
    tasks: tasks(57),
    ...over,
  };
}

describe('看板列：列内上限折叠显头部（票 04）', () => {
  it('超上限：先画 50 张，底部一行「还有 M 张，加载更多」', () => {
    render(BoardColumn, { props: columnProps() });
    expect(screen.getByText('任务 1')).toBeTruthy();
    expect(screen.getByText('任务 50')).toBeTruthy();
    expect(screen.queryByText('任务 51')).toBeNull();
    expect(screen.getByRole('button', { name: '还有 7 张，加载更多' })).toBeTruthy();
  });

  it('点「加载更多」：游标推进一页（50 → 57 全出）', async () => {
    render(BoardColumn, { props: columnProps() });
    await fireEvent.click(screen.getByRole('button', { name: /还有 7 张/ }));
    expect(screen.getByText('任务 57')).toBeTruthy();
    expect(screen.queryByRole('button', { name: /加载更多/ })).toBeNull();
  });

  it('换过滤档（filterKey 变化）：窗口游标回缺省，回到 50 张', async () => {
    const { rerender } = render(BoardColumn, { props: columnProps({ filterKey: 'all' }) });
    await fireEvent.click(screen.getByRole('button', { name: /还有 7 张/ }));
    expect(screen.getByText('任务 57')).toBeTruthy();

    await rerender(columnProps({ tasks: tasks(57), filterKey: 'running' }));
    expect(screen.queryByText('任务 51')).toBeNull();
    expect(screen.getByRole('button', { name: '还有 7 张，加载更多' })).toBeTruthy();
  });

  it('列头计数是过滤后全量（不因折叠缩水）', () => {
    const { container } = render(BoardColumn, { props: columnProps() });
    const n = container.querySelector('.col-n');
    expect(n?.textContent).toBe('57');
  });

  it('没超上限：不出现「加载更多」行', () => {
    render(BoardColumn, { props: columnProps({ tasks: tasks(12) }) });
    expect(screen.queryByRole('button', { name: /加载更多/ })).toBeNull();
  });
});
