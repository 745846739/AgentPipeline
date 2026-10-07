/**
 * 动作提交的端点分派（决策 101 / 119）。
 *
 * 这里只钉 merge approve 那一条（决策 393）：「合入后 push」开关必须从 SubmitOptions
 * 透传进 `mergeDecision`——checkbox（DiffReviewPanel）与服务端 Phase B 之间唯一的
 * 接线点，断了它开关就永远是「不 push」。
 */

import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AllowedAction } from '../api/types';

const mocks = vi.hoisted(() => ({
  cancelTask: vi.fn(),
  mergeDecision: vi.fn(),
  resumeTask: vi.fn(),
  reviewTask: vi.fn(),
}));

vi.mock('../api/client', () => ({
  cancelTask: mocks.cancelTask,
  mergeDecision: mocks.mergeDecision,
  resumeTask: mocks.resumeTask,
  reviewTask: mocks.reviewTask,
}));

import { submitAllowedAction } from './actionSubmit';

const approve: AllowedAction = { action: 'approve', kind: 'side_effect', label: '合入' };

describe('submitAllowedAction → /merge/decision（决策 393）', () => {
  beforeEach(() => {
    mocks.mergeDecision.mockReset();
  });

  it('勾了 push：approve 透传 push: true', async () => {
    await submitAllowedAction('t1', approve, { pendingType: 'merge_approval', push: true });
    expect(mocks.mergeDecision).toHaveBeenCalledWith('t1', 'approve', true);
  });

  it('没勾 push：approve 透传 push: false（缺省不推）', async () => {
    await submitAllowedAction('t1', approve, { pendingType: 'merge_approval' });
    expect(mocks.mergeDecision).toHaveBeenCalledWith('t1', 'approve', false);
  });
});
