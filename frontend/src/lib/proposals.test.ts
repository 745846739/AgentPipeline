/**
 * 提议轮的渲染判据（决策 188 / 207，票 03）。判据全在 `lib/proposals.ts`，这里逐条钉住。
 *
 * 这一档最该被测的不是「渲染出来长什么样」（那是 e2e 的活），而是**三条会悄悄错的规矩**：
 * ① 过期由前端按 `expires_at` 自己算（等后端标会让按钮多亮一小时）；
 * ② 过期只让按钮变灰、那一轮仍在（审计）；
 * ③ 同一个动作已经在状态区有钮时只指路——而**详情没到时不指路**（不能把「还没读到」
 *    说成「已经有了」，那样两颗钮都没有，人无处可按）。
 */

import { describe, expect, it } from 'vitest';
import type { AllowedAction, ForemanProposal } from '../api/types';
import {
  expiryReached,
  isRepairProposal,
  proposalActionable,
  proposalPointerOnly,
  proposalRemainingLabel,
  proposalState,
  proposalStateLabel,
  proposalTaskId,
  proposalToolLabel,
  repairActionLabel,
  repairGateLabel,
  stateZoneActionName,
} from './proposals';

const NOW = Date.parse('2026-09-17T22:00:00Z');

function proposal(over: Partial<ForemanProposal> = {}): ForemanProposal {
  return {
    id: 'p1',
    session_id: 's1',
    tool: 'write_file',
    args: { path: 'notes.md' },
    summary: '写文件 notes.md',
    status: 'pending',
    created_at: '2026-09-17T21:55:00Z',
    // TTL 10 分钟（决策 207）：下面这批用例的「现在」落在到期前 5 分钟
    expires_at: '2026-09-17T22:05:00Z',
    resolved_at: null,
    ...over,
  };
}

describe('过期由前端自己算', () => {
  it('未到点仍是待按键', () => {
    expect(proposalState(proposal(), NOW)).toBe('pending');
    expect(proposalActionable(proposal(), NOW)).toBe(true);
  });

  it('到点即过期——不等后端把它标成 expired', () => {
    const p = proposal({ expires_at: '2026-09-17T22:00:00Z' });
    // 恰好到点也算过期（`now >= expires_at`），与后端 `is_expired` 同一条
    expect(expiryReached(p, NOW)).toBe(true);
    expect(proposalState(p, NOW)).toBe('expired');
    expect(proposalActionable(p, NOW)).toBe(false);
  });

  it('后端的终态原样透过，不受过期判定影响', () => {
    for (const status of ['executed', 'rejected', 'expired']) {
      expect(proposalState(proposal({ status }), NOW)).toBe(status);
    }
  });

  it('认不出的 status 当成不可按，不假装它是 pending', () => {
    // 后端将来加了第五种状态：宁可少一颗能按的钮，不可多一颗按下去会报错的钮
    expect(proposalState(proposal({ status: 'weird' }), NOW)).toBe('expired');
    expect(proposalActionable(proposal({ status: 'weird' }), NOW)).toBe(false);
  });

  it('坏时间戳算过期（宁可让人重新提一条，不可让一颗按不动的钮亮着）', () => {
    expect(expiryReached(proposal({ expires_at: '不是时间' }), NOW)).toBe(true);
    expect(proposalRemainingLabel(proposal({ expires_at: '不是时间' }), NOW)).toBe('已过期');
  });

  it('倒计时只给分钟这一档', () => {
    expect(proposalRemainingLabel(proposal(), NOW)).toBe('还剩 5 分钟');
    expect(proposalRemainingLabel(proposal({ expires_at: '2026-09-17T22:00:30Z' }), NOW)).toBe(
      '还剩不到 1 分钟',
    );
    expect(proposalRemainingLabel(proposal({ expires_at: '2026-09-17T21:59:00Z' }), NOW)).toBe(
      '已过期',
    );
  });
});

describe('状态文案：三种终态各自说得清', () => {
  it('未决 = 等你按键', () => {
    expect(proposalStateLabel(proposal(), NOW)).toBe('等你按键');
  });

  it('被拒绝要说清「没有执行任何动作」', () => {
    // 只说「被拒绝」在读者眼里与「执行失败」分不开，而两者要做的事完全不同
    const label = proposalStateLabel(proposal({ status: 'rejected' }), NOW);
    expect(label).toContain('被拒绝');
    expect(label).toContain('没有执行任何动作');
  });

  it('过期要说清「要做得重新提一次」', () => {
    const label = proposalStateLabel(proposal({ status: 'expired' }), NOW);
    expect(label).toContain('过期');
    expect(label).toContain('重新提一次');
  });
});

describe('只指路：同一个动作不摆第二颗钮', () => {
  const actions = (names: string[]): AllowedAction[] =>
    names.map((action) => ({ action, kind: 'side_effect' as const, label: action }));

  it('提议的 action 不在状态区那份动作集里时不指路（该画钮）', () => {
    const p = proposal({ tool: 'task', args: { action: 'cancel', task_id: 't1' } });
    expect(proposalPointerOnly(p, actions(['approve', 'return']))).toBe(false);
    expect(proposalPointerOnly(p, actions(['cancel']))).toBe(true);
  });

  it('详情还没到（actions 为 undefined）时不指路——否则两颗钮都没有', () => {
    const p = proposal({ tool: 'task', args: { action: 'cancel', task_id: 't1' } });
    expect(proposalPointerOnly(p, undefined)).toBe(false);
    expect(proposalPointerOnly(p, [])).toBe(false);
  });

  it('动作名映射照 actions.rs 那张 (PendingKind, action) → 端点表', () => {
    expect(stateZoneActionName(proposal({ tool: 'task', args: { action: 'cancel' } }))).toBe(
      'cancel',
    );
    // 人工评审：通过 → approve，打回 → reject（`POST /tasks/{id}/review`）
    expect(
      stateZoneActionName(proposal({ tool: 'task', args: { action: 'review', approved: true } })),
    ).toBe('approve');
    expect(
      stateZoneActionName(proposal({ tool: 'task', args: { action: 'review', approved: false } })),
    ).toBe('reject');
    // 合入决定：approve → 合入，reject → 返回修改（动作集里叫 return）
    expect(
      stateZoneActionName(proposal({ tool: 'task', args: { action: 'merge', decision: 'approve' } })),
    ).toBe('approve');
    expect(
      stateZoneActionName(proposal({ tool: 'task', args: { action: 'merge', decision: 'reject' } })),
    ).toBe('return');
    // resume 的动作名由模型填，与动作集同一套词表
    expect(
      stateZoneActionName(proposal({ tool: 'task', args: { action: 'resume', resume_action: 'continue' } })),
    ).toBe('continue');
  });

  it('环境层工具与「本来就不在状态区」的动作永远画钮', () => {
    for (const [tool, args] of [
      ['write_file', { path: 'a.md' }],
      ['edit_file', { path: 'a.md' }],
      ['run_command', { command: 'ls' }],
      ['task', { action: 'create', project_id: 'p1' }],
      ['task', { action: 'retry', task_id: 't1' }],
      ['config', { action: 'set', stage: 'develop' }],
      ['skills', { action: 'delete', name: 'x' }],
    ] as const) {
      expect(stateZoneActionName(proposal({ tool, args }))).toBeNull();
      expect(proposalPointerOnly(proposal({ tool, args }), actions(['cancel']))).toBe(false);
    }
  });

  it('合入 / 评审的指路要认那个任务的 id', () => {
    const p = proposal({ tool: 'task', args: { action: 'cancel', task_id: 't-42' } });
    expect(proposalTaskId(p)).toBe('t-42');
    expect(proposalTaskId(proposal({ tool: 'write_file' }))).toBeNull();
    expect(proposalTaskId(proposal({ tool: 'task', args: { action: 'create' } }))).toBeNull();
  });
});

describe('工具名的人话', () => {
  it('认得的工具按族翻译，动作并进去', () => {
    expect(proposalToolLabel(proposal({ tool: 'write_file' }))).toBe('写文件');
    expect(proposalToolLabel(proposal({ tool: 'run_command' }))).toBe('跑命令');
    expect(proposalToolLabel(proposal({ tool: 'task', args: { action: 'cancel' } }))).toBe(
      '任务动作 · cancel',
    );
    expect(proposalToolLabel(proposal({ tool: 'config', args: { action: 'set' } }))).toBe(
      '改阶段配置 · set',
    );
  });

  it('认不出的工具名**原样显示**——假装认识它才是真的误导', () => {
    // 判据是「认不出就照抄」，不是「照抄工具名再加一半动作」——把 action 拼到一个
    // 不认识的工具名后面，读起来像「我们支持这个工具，只是它叫 pairing」
    expect(proposalToolLabel(proposal({ tool: 'pairing', args: { action: 'reset' } }))).toBe(
      'pairing',
    );
  });
});

describe('终态优先于指路（渲染顺序会决定人能读到什么）', () => {
  const actions = [{ action: 'cancel', kind: 'side_effect' as const, label: '取消' }];

  it('已执行 / 被拒绝的提议即便动作还在动作集里，也不该继续指路', () => {
    // 判据是「它是否还是待办」——终态的提议已无待办可言，指路会把人送回状态区去按一个
    // 已经按过的动作（那颗钮才是真的会再改一次状态的那颗）
    for (const status of ['executed', 'rejected']) {
      const p = proposal({ tool: 'task', args: { action: 'cancel', task_id: 't1' }, status });
      expect(proposalPointerOnly(p, actions)).toBe(true); // 判据本身照旧
      expect(proposalState(p, NOW)).toBe(status); // 但终态要先被渲染分支判掉
      expect(proposalActionable(p, NOW)).toBe(false);
    }
  });

  it('过期与终态是两件事：过期的仍可能指路（状态区那颗钮还是活的）', () => {
    const p = proposal({
      tool: 'task',
      args: { action: 'cancel', task_id: 't1' },
      expires_at: '2026-09-17T21:59:00Z',
    });
    expect(proposalState(p, NOW)).toBe('expired');
    expect(proposalActionable(p, NOW)).toBe(false);
    expect(proposalPointerOnly(p, actions)).toBe(true);
  });
});

// ─────────────── 修复提议的渲染判据（决策 212① / 票 12）───────────────

describe('修复提议：合入而不是执行，闸门读数说清有没有补丁', () => {
  const repair = (over: Partial<ForemanProposal> = {}): ForemanProposal =>
    proposal({
      id: 'p-repair',
      tool: 'repair',
      kind: 'repair',
      summary: '合入修复分支 repair/abc → main（示例）：闸门已过',
      args: { project_id: 'p1' },
      payload: {
        repair_id: 'abc',
        worktree_path: '/tmp/home/worktrees/repair-abc',
        branch: 'repair/abc',
        base_ref: 'main',
        base_commit: 'deadbeef',
        gate_passed: true,
        gate: [
          {
            kind: 'test',
            command: 'cargo test --quiet',
            exit_code: 0,
            duration_ms: 1200,
            output_path: null,
            output_preview: '',
          },
        ],
        commit: 'cafe',
        diff: 'diff --git a/x.rs b/x.rs',
        diff_stat: ' x.rs | 2 ++',
      },
      ...over,
    });

  it('名牌与按钮点名「合入」——它会动主干', () => {
    expect(proposalToolLabel(repair())).toContain('修复');
    expect(repairActionLabel(repair())).toBe('合入');
    expect(repairActionLabel(proposal())).toBe('执行');
  });

  it('闸门读数在场；没过时明说「没有补丁」', () => {
    expect(repairGateLabel(repair())).toContain('test 过');
    const failed = repair({
      payload: {
        ...repair().payload!,
        gate_passed: false,
        gate: [{ ...repair().payload!.gate[0], exit_code: 1 }],
        diff: null,
      },
    });
    expect(repairGateLabel(failed)).toContain('没有补丁');
    expect(isRepairProposal(failed)).toBe(true);
  });

  it('普通提议不走修复那一块', () => {
    expect(isRepairProposal(proposal())).toBe(false);
    expect(repairGateLabel(proposal())).toBeNull();
  });

  it('修复提议**不按时间过期**：远期有效期不会让按钮变灰', () => {
    // 决策 212①：修复是唯一一条有意留到第二天早上看的东西——10 分钟的 TTL 会让早上
    // 看到的是一排灰按钮。后端给它的是远期有效期（`FOREMAN_PROPOSAL_NO_TTL_DAYS`）。
    const overnight = repair({ expires_at: '2126-01-01T00:00:00Z' });
    expect(proposalState(overnight, NOW)).toBe('pending');
    expect(proposalActionable(overnight, NOW)).toBe(true);
  });
});
