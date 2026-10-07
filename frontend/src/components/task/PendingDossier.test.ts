import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import type { AllowedAction, BranchCursor, PendingReason } from '../../api/types';
import PendingDossier from './PendingDossier.svelte';

// jsdom 不实现 ResizeObserver，而 dock 用 `bind:clientHeight` 量自己（同 TopBar.test.ts 的垫片）
if (typeof globalThis.ResizeObserver !== 'function') {
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
}

const reason: PendingReason = {
  type: 'merge_approval',
  stage: 'merge',
  node: 'execute',
  message: '合并提案已生成，等你拍板',
};

const cursors: BranchCursor[] = [
  {
    cursor_id: 'c-merge',
    branch: 'main',
    stage: 'merge',
    node: 'execute',
    status: 'pending',
    validate_attempts: 0,
    skipped_to_join: false,
    pending_reason: reason,
  },
];

const mergeActions: AllowedAction[] = [
  { action: 'approve', kind: 'side_effect', label: '合入', cursor_id: 'c-merge' },
  { action: 'return', kind: 'side_effect', label: '返回修改', cursor_id: 'c-merge' },
];

describe('PendingDossier · 移动版动作坞的收展（决策 281）', () => {
  it('dock 默认收成一行手柄：说明与动作面都不在 DOM，详情不再被坞盖掉大半屏', () => {
    render(PendingDossier, {
      props: { reason, cursors, actions: mergeActions, dock: true },
    });
    const head = screen.getByRole('button', { name: /等你拍板/ });
    expect(head.getAttribute('aria-expanded')).toBe('false');
    expect(screen.queryByRole('button', { name: '合入' })).toBeNull();
    expect(screen.queryByText(reason.message)).toBeNull();
  });

  it('点开手柄：动作面与说明全量出现，aria-expanded 翻转；再点收回去', async () => {
    render(PendingDossier, {
      props: { reason, cursors, actions: mergeActions, dock: true },
    });
    const head = screen.getByRole('button', { name: /等你拍板/ });
    await fireEvent.click(head);
    expect(head.getAttribute('aria-expanded')).toBe('true');
    expect(screen.getByRole('button', { name: '合入' })).toBeTruthy();
    expect(screen.getByRole('button', { name: '返回修改' })).toBeTruthy();
    expect(screen.getByText(reason.message)).toBeTruthy();

    await fireEvent.click(head);
    expect(head.getAttribute('aria-expanded')).toBe('false');
    expect(screen.queryByRole('button', { name: '合入' })).toBeNull();
  });

  it('展开层里动作接线原样：点「合入」带所属游标提交', async () => {
    const onaction = vi.fn();
    render(PendingDossier, {
      props: { reason, cursors, actions: mergeActions, dock: true, onaction },
    });
    await fireEvent.click(screen.getByRole('button', { name: /等你拍板/ }));
    await fireEvent.click(screen.getByRole('button', { name: '合入' }));
    expect(onaction).toHaveBeenCalledWith(mergeActions[0], { cursorId: 'c-merge', push: false });
  });

  it('桌面档案盒（dock=false）不受折叠影响：动作直接可见', () => {
    render(PendingDossier, { props: { reason, cursors, actions: mergeActions } });
    expect(screen.getByRole('button', { name: '合入' })).toBeTruthy();
    expect(screen.queryByRole('button', { name: /等你拍板/ })).toBeNull();
  });
});
