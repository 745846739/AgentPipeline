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

/**
 * 同名动作各是各的（现场发现：`retry_exhausted` 的整块动作区不再更新）。
 *
 * 后端下发的动作集里，同名动作可以合法地出现两次，两条的**落点不同**：
 *   - `retry_exhausted@develop|test`：「重试执行」+「带失败摘要回架构设计修订」（两个 `goto`）；
 *   - `user_decision@test_code_issue|gate_recheck`：「修改测试用例」+「修改业务代码」。
 * 渲染层的 key 若只看「动作名 + 游标」，这两条就撞 key → Svelte 抛 `each_key_duplicate`
 * → **整块动作区不再更新**：坞里留着上一个 pending 的按钮，点下去发的是别的动作。
 * 组件测试跑在 Svelte 的 dev 构建上，撞 key 会直接抛——这份用例就是那把卡尺。
 */
describe('PendingActions — 同游标下的两条 goto（retry_exhausted）', () => {
  /** `retry_exhausted@develop` 的完整动作集（crates/core/src/actions.rs）。 */
  const retryExhausted: AllowedAction[] = [
    {
      action: 'goto',
      kind: 'resume',
      label: '重试执行',
      cursor_id: 'c-dev',
      target: { stage: 'develop', node: 'execute' },
    },
    { action: 'skip', kind: 'resume', label: '强制进入下一阶段', cursor_id: 'c-dev' },
    {
      action: 'goto',
      kind: 'resume',
      label: '带失败摘要回架构设计修订',
      cursor_id: 'c-dev',
      target: { stage: 'architect-design', node: 'validate_input' },
    },
    { action: 'cancel', kind: 'side_effect', label: '终止任务', cursor_id: 'c-dev' },
  ];

  it('两条 goto 都画出来，四颗钮各就各位', () => {
    render(PendingActions, {
      props: { actions: retryExhausted, cursors: [cursor()], pendingType: 'retry_exhausted' },
    });

    expect(screen.getByRole('button', { name: '重试执行' })).toBeTruthy();
    expect(screen.getByRole('button', { name: '带失败摘要回架构设计修订' })).toBeTruthy();
    expect(screen.getByRole('button', { name: '强制进入下一阶段' })).toBeTruthy();
    expect(screen.getByRole('button', { name: '终止任务' })).toBeTruthy();
  });

  it('提交中态只落在被点的那一颗上（不是两颗 goto 一起转）', () => {
    render(PendingActions, {
      props: {
        actions: retryExhausted,
        cursors: [cursor()],
        pendingType: 'retry_exhausted',
        isBusy: (action: AllowedAction) => action.label === '重试执行',
      },
    });

    expect(screen.getByRole('button', { name: '重试执行' }).hasAttribute('disabled')).toBe(true);
    expect(
      screen
        .getByRole('button', { name: '带失败摘要回架构设计修订' })
        .hasAttribute('disabled'),
    ).toBe(false);
  });

  it('点哪颗发哪颗（同名不互相顶掉）', async () => {
    const onaction = vi.fn();
    render(PendingActions, {
      props: { actions: retryExhausted, cursors: [cursor()], pendingType: 'retry_exhausted', onaction },
    });

    await fireEvent.click(screen.getByRole('button', { name: '带失败摘要回架构设计修订' }));
    expect(onaction).toHaveBeenCalledTimes(1);
    expect(onaction.mock.calls[0][0]).toMatchObject({
      action: 'goto',
      target: { stage: 'architect-design', node: 'validate_input' },
    });
  });
});
