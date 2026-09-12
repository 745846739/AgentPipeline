import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import type { AllowedAction, BranchCursor, PendingReason } from '../../api/types';
import PendingActions from './PendingActions.svelte';

const reason: PendingReason = {
  type: 'info_insufficient',
  stage: 'develop-design',
  node: 'validate_input',
  message: '缺少数据流定义',
};

function cursor(overrides: Partial<BranchCursor> = {}): BranchCursor {
  return {
    cursor_id: 'c-dev',
    branch: 'develop-design',
    stage: 'develop-design',
    node: 'validate_input',
    status: 'pending',
    validate_attempts: 0,
    skipped_to_join: false,
    pending_reason: reason,
    ...overrides,
  };
}

describe('PendingActions（决策 91 / 101 纯渲染）', () => {
  it('resume 动作提交时带上动作自带 cursor_id', async () => {
    const onaction = vi.fn();
    const actions: AllowedAction[] = [
      {
        action: 'continue',
        kind: 'resume',
        label: '补充信息并继续',
        cursor_id: 'c-dev',
        requires_input: true,
      },
    ];
    render(PendingActions, {
      props: { actions, cursors: [cursor()], pendingType: 'info_insufficient', onaction },
    });

    await fireEvent.click(screen.getByRole('button', { name: /补充信息并继续/ }));
    expect(onaction).toHaveBeenCalledTimes(1);
    const [action, opts] = onaction.mock.calls[0];
    expect(action.action).toBe('continue');
    expect(opts.cursorId).toBe('c-dev');
  });

  it('动作缺 cursor_id 且只有一条游标时，取所属分支药丸的 cursor_id', async () => {
    const onaction = vi.fn();
    const actions: AllowedAction[] = [{ action: 'skip', kind: 'resume', label: '跳过本设计阶段' }];
    render(PendingActions, {
      props: { actions, cursors: [cursor({ cursor_id: 'c-only' })], pendingType: 'user_decision', onaction },
    });

    await fireEvent.click(screen.getByRole('button', { name: /跳过本设计阶段/ }));
    expect(onaction.mock.calls[0][1].cursorId).toBe('c-only');
  });

  it('side_effect 旁路动作有配对端点时点击提交', async () => {
    const onaction = vi.fn();
    const actions: AllowedAction[] = [
      { action: 'cancel', kind: 'side_effect', label: '取消任务', cursor_id: 'c-dev' },
    ];
    render(PendingActions, {
      props: { actions, cursors: [cursor()], pendingType: 'info_insufficient', onaction },
    });

    const button = screen.getByRole('button', { name: '取消任务' });
    expect(button.hasAttribute('disabled')).toBe(false);
    await fireEvent.click(button);
    expect(onaction).toHaveBeenCalledWith(actions[0], { cursorId: 'c-dev', input: undefined });
  });

  it('无配对端点的 side_effect 渲染为禁用（决策 101）', () => {
    const actions: AllowedAction[] = [
      { action: 'merge_task', kind: 'side_effect', label: '合并任务' },
    ];
    render(PendingActions, {
      props: { actions, cursors: [cursor()], pendingType: 'user_decision' },
    });
    const button = screen.getByRole('button', { name: '合并任务' });
    expect(button.hasAttribute('disabled')).toBe(true);
  });

  it('wait 动作渲染为禁用，无系统变更', () => {
    const actions: AllowedAction[] = [
      { action: 'wait_dependency_retry', kind: 'wait', label: '等待依赖重试' },
    ];
    render(PendingActions, {
      props: { actions, cursors: [cursor()], pendingType: 'dependency_failed' },
    });
    expect(screen.getByRole('button', { name: '等待依赖重试' }).hasAttribute('disabled')).toBe(true);
  });
});
