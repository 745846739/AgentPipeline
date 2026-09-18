import { describe, expect, it } from 'vitest';
import type { AllowedAction, BranchCursor } from '../api/types';
import {
  actionKey,
  allowsFreeInput,
  endpointFor,
  groupActionsByBranch,
  sideEffectEnabled,
} from './actions';

function cursor(overrides: Partial<BranchCursor> = {}): BranchCursor {
  return {
    cursor_id: 'c-dev',
    branch: 'develop-design',
    stage: 'develop-design',
    node: 'validate_input',
    status: 'pending',
    validate_attempts: 0,
    skipped_to_join: false,
    pending_reason: {
      type: 'info_insufficient',
      stage: 'develop-design',
      node: 'validate_input',
      message: '缺少数据流',
    },
    ...overrides,
  };
}

describe('groupActionsByBranch — 按分支 / 类型分组', () => {
  it('按 cursor_id 归组，并拆分 resume / side_effect / wait', () => {
    const actions: AllowedAction[] = [
      { action: 'continue', kind: 'resume', label: '补充信息并继续', cursor_id: 'c-dev', requires_input: true },
      { action: 'cancel', kind: 'side_effect', label: '取消任务', cursor_id: 'c-dev' },
      { action: 'wait_dependency_retry', kind: 'wait', label: '等待依赖重试', cursor_id: 'c-main' },
    ];
    const groups = groupActionsByBranch(actions, [
      cursor(),
      cursor({ cursor_id: 'c-main', branch: 'main', stage: 'init', node: 'execute' }),
    ]);
    const dev = groups.find((g) => g.cursorId === 'c-dev')!;
    expect(dev.branchKind).toBe('dev');
    expect(dev.resume.map((a) => a.action)).toEqual(['continue']);
    expect(dev.sideEffect.map((a) => a.action)).toEqual(['cancel']);
    const main = groups.find((g) => g.cursorId === 'c-main')!;
    expect(main.branchKind).toBe('main');
    expect(main.wait.map((a) => a.action)).toEqual(['wait_dependency_retry']);
  });

  it('两个分支各自成组（决策 84：动作集按游标独立下发）', () => {
    const actions: AllowedAction[] = [
      { action: 'skip', kind: 'resume', label: '跳过本设计阶段', cursor_id: 'c-dev' },
      { action: 'skip', kind: 'resume', label: '跳过本设计阶段', cursor_id: 'c-test' },
    ];
    const groups = groupActionsByBranch(actions, [
      cursor(),
      cursor({ cursor_id: 'c-test', branch: 'test-design' }),
    ]);
    expect(groups).toHaveLength(2);
    expect(groups.map((g) => g.branchKind).sort()).toEqual(['dev', 'test']);
  });
});

describe('endpointFor — 动作 → 配对端点（决策 101 / 119）', () => {
  it('resume 类没有专用端点', () => {
    expect(endpointFor('info_insufficient', 'continue')).toBeNull();
    expect(endpointFor('timeout', 'goto')).toBeNull();
  });

  it('side_effect 全部有端点', () => {
    expect(endpointFor('context_overflow', 'cancel')?.path).toBe('/cancel');
    expect(endpointFor('context_overflow', 'split_task')?.path).toBe('/split');
    expect(endpointFor('context_overflow', 'model_override')?.path).toBe('/model-override');
    expect(endpointFor('merge_approval', 'approve')?.path).toBe('/merge/decision');
    expect(endpointFor('merge_approval', 'return')?.path).toBe('/merge/decision');
    expect(endpointFor('human_review', 'approve')?.path).toBe('/review');
    expect(endpointFor('human_review', 'reject')?.path).toBe('/review');
  });

  it('同名动作按 (type, kind) 行内解析：human_review 的 approve ≠ merge_approval 的 approve', () => {
    expect(endpointFor('human_review', 'approve')?.path).toBe('/review');
    expect(endpointFor('merge_approval', 'approve')?.path).toBe('/merge/decision');
  });

  it('未知 side_effect 无端点 → 禁用（不崩）', () => {
    const unknown: AllowedAction = { action: 'merge_task', kind: 'side_effect', label: '合并任务' };
    expect(sideEffectEnabled(unknown, 'user_decision')).toBe(false);
    expect(endpointFor('merge_approval', 'reject')).toBeNull();
  });
});

describe('allowsFreeInput — 只有 info_insufficient 有自由输入（决策 79）', () => {
  it('requires_input 的 resume 才允许输入', () => {
    expect(
      allowsFreeInput({ action: 'continue', kind: 'resume', label: '补充信息并继续', requires_input: true }),
    ).toBe(true);
    expect(allowsFreeInput({ action: 'skip', kind: 'resume', label: '跳过' })).toBe(false);
    expect(
      allowsFreeInput({ action: 'cancel', kind: 'side_effect', label: '取消', requires_input: true }),
    ).toBe(false);
  });
});

describe('actionKey — 同名不同落点不是一个动作', () => {
  /** `retry_exhausted@develop` 的两条 goto（crates/core/src/actions.rs 的动作集）。 */
  const retryDevelop: AllowedAction[] = [
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
  ];

  it('同一游标下的两条 goto 各是各的（否则渲染层撞 key、提交中态同时点亮两颗）', () => {
    const keys = retryDevelop.map((a) => actionKey(a, 'c-dev'));
    expect(new Set(keys).size).toBe(keys.length);
    expect(keys[0]).not.toBe(keys[2]);
  });

  it('落点相同才算同一个动作：对象重造不影响身份', () => {
    const a: AllowedAction = {
      action: 'goto',
      kind: 'resume',
      label: '重试执行',
      cursor_id: 'c-dev',
      target: { stage: 'develop', node: 'execute' },
    };
    expect(actionKey({ ...a }, 'c-dev')).toBe(actionKey(a, 'c-dev'));
    // 动作自带游标优先（决策 91）——显式传入的所属分支游标只在它缺省时兜底
    expect(actionKey(a, 'c-test')).toBe(actionKey(a, 'c-dev'));
    const bare: AllowedAction = { action: 'goto', kind: 'resume', label: '重试执行' };
    expect(actionKey(bare, 'c-test')).not.toBe(actionKey(bare, 'c-dev'));
  });

  it('没有落点的动作仍是「动作名 + 游标」（游标缺省回退到所属分支）', () => {
    const cont: AllowedAction = {
      action: 'continue',
      kind: 'resume',
      label: '补充信息并继续',
      requires_input: true,
    };
    expect(actionKey(cont)).toBe('continue:::');
    expect(actionKey(cont, 'c-main')).toBe('continue:c-main::');
  });
});
