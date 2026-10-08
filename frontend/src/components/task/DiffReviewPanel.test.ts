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

  it('点击「合入」提交 approve 动作并带所属游标（未勾 push 默认 false，决策 393）', async () => {
    const onaction = vi.fn();
    render(DiffReviewPanel, {
      props: { diff: null, raw: null, actions: mergeActions, cursors, onaction },
    });
    await fireEvent.click(screen.getByRole('button', { name: '合入' }));
    // 决策 216②：合入是 destructive —— 第一颗只亮后果句，第二颗才真提交
    expect(onaction).not.toHaveBeenCalled();
    expect(screen.getByText('确认合入？')).toBeTruthy();
    await fireEvent.click(screen.getByRole('button', { name: '合入' }));
    expect(onaction).toHaveBeenCalledWith(mergeActions[0], { cursorId: 'c-merge', push: false });
  });

  it('勾选「合入后 push 到远端」后合入，approve 带 push: true（决策 393）', async () => {
    const onaction = vi.fn();
    render(DiffReviewPanel, {
      props: { diff: null, raw: null, actions: mergeActions, cursors, onaction },
    });
    await fireEvent.click(screen.getByLabelText('合入后 push 到远端'));
    await fireEvent.click(screen.getByRole('button', { name: '合入' }));
    await fireEvent.click(screen.getByRole('button', { name: '合入' }));
    expect(onaction).toHaveBeenCalledWith(mergeActions[0], { cursorId: 'c-merge', push: true });
  });

  it('没有 approve 动作时不渲染 push 开关（决策 393）', () => {
    render(DiffReviewPanel, {
      props: { diff: null, raw: null, actions: [mergeActions[1]], cursors },
    });
    expect(screen.queryByLabelText('合入后 push 到远端')).toBeNull();
  });

  it('base_commit 过期（stale）时显示「基准已前移」提示', () => {
    render(DiffReviewPanel, {
      props: { diff: null, raw: null, stale: true, actions: mergeActions, cursors },
    });
    expect(screen.getByText(/基准已前移/)).toBeTruthy();
  });

  it('动作集同内容刷新（SSE / 轮询换新数组）不清零确认态——第二下仍真提交', async () => {
    const onaction = vi.fn();
    const { rerender } = render(DiffReviewPanel, {
      props: { diff: null, raw: null, actions: mergeActions, cursors, onaction },
    });
    await fireEvent.click(screen.getByRole('button', { name: '合入' }));
    expect(screen.getByText('确认合入？')).toBeTruthy();

    // 同内容刷新：`actions` / `cursors` 都换一份新数组（每次对齐都这样，见 store.load）。
    // 判据若取数组身份，这次刷新就会把确认态抹掉 → 第二下变成「又亮一次」，请求永不发出。
    await rerender({
      diff: null,
      raw: null,
      actions: mergeActions.map((a) => ({ ...a })),
      cursors: cursors.map((c) => ({ ...c })),
      onaction,
    });
    expect(screen.getByText('确认合入？'), '同内容刷新不该抹掉确认态').toBeTruthy();

    await fireEvent.click(screen.getByRole('button', { name: '合入' }));
    expect(onaction, '确认态还在，第二下就该真提交').toHaveBeenCalledTimes(1);
  });

  it('换了新一轮待办（动作身份变了）才退回普通态', async () => {
    const onaction = vi.fn();
    const { rerender } = render(DiffReviewPanel, {
      props: { diff: null, raw: null, actions: mergeActions, cursors, onaction },
    });
    await fireEvent.click(screen.getByRole('button', { name: '合入' }));
    expect(screen.getByText('确认合入？')).toBeTruthy();

    // 上一个 pending 走了：换一条游标（动作身份随之改变）
    const nextCursor = { ...cursors[0], cursor_id: 'c-merge-2' };
    await rerender({
      diff: null,
      raw: null,
      actions: [{ action: 'approve', kind: 'side_effect', label: '合入', cursor_id: 'c-merge-2' }],
      cursors: [nextCursor],
      onaction,
    });
    expect(screen.queryByText('确认合入？'), '新一轮不该继承上一轮的确认态').toBeNull();
  });

  it('无审批动作时不渲染动作按钮', () => {
    render(DiffReviewPanel, { props: { diff: null, raw: null, actions: [], cursors } });
    expect(screen.getByText('当前没有可用的审批动作。')).toBeTruthy();
  });
});
