import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import type { AllowedAction, BranchCursor } from '../../api/types';
import DiffReviewPanel from './DiffReviewPanel.svelte';

const cursors: BranchCursor[] = [
  {
    cursor_id: 'c-merge',
    branch: 'main',
    stage: 'merge',
    node: 'execute',
    status: 'pending',
    validate_attempts: 0,
    skipped_to_join: false,
    pending_reason: {
      type: 'merge_approval',
      stage: 'merge',
      node: 'execute',
      message: '合并提案已生成',
    },
  },
];

const mergeActions: AllowedAction[] = [
  { action: 'approve', kind: 'side_effect', label: '合入', cursor_id: 'c-merge' },
  { action: 'return', kind: 'side_effect', label: '返回修改', cursor_id: 'c-merge' },
];

describe('DiffReviewPanel（决策 23 / 119）', () => {
  it('审批动作只有「合入 / 返回修改」，永不出现「拒绝」', () => {
    render(DiffReviewPanel, {
      props: { diff: null, raw: null, actions: mergeActions, cursors },
    });
    expect(screen.getByRole('button', { name: '合入' })).toBeTruthy();
    expect(screen.getByRole('button', { name: '返回修改' })).toBeTruthy();
    expect(screen.queryByText('拒绝')).toBeNull();
    expect(screen.queryByText(/拒绝/)).toBeNull();
  });

  it('点击「合入」提交 approve 动作并带所属游标', async () => {
    const onaction = vi.fn();
    render(DiffReviewPanel, {
      props: { diff: null, raw: null, actions: mergeActions, cursors, onaction },
    });
    await fireEvent.click(screen.getByRole('button', { name: '合入' }));
    expect(onaction).toHaveBeenCalledWith(mergeActions[0], { cursorId: 'c-merge' });
  });

  it('base_commit 过期（stale）时显示「基准已前移」提示', () => {
    render(DiffReviewPanel, {
      props: { diff: null, raw: null, stale: true, actions: mergeActions, cursors },
    });
    expect(screen.getByText(/基准已前移/)).toBeTruthy();
  });

  it('无审批动作时不渲染动作按钮', () => {
    render(DiffReviewPanel, { props: { diff: null, raw: null, actions: [], cursors } });
    expect(screen.getByText('当前没有可用的审批动作。')).toBeTruthy();
  });
});
