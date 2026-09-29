import { describe, expect, it } from 'vitest';
import type { NodeCommand } from '../api/types';
import { filterCommands } from './commandFilter';

/**
 * 命令页签的过滤纯函数（spec list-windowing 票 02）：命令行关键词 + 退出码档位。
 * 过滤与切片可叠加——判据是**先过滤后切**（组件里 filterCommands 的输出喂 windowSlice）。
 */
function command(overrides: Partial<NodeCommand> = {}): NodeCommand {
  return {
    id: 1,
    task_id: 't1',
    run_id: 1,
    stage: 'develop',
    node: 'execute',
    source: 'agent',
    command: 'cargo test',
    original_command: null,
    cwd: '/repo',
    exit_code: 0,
    stdout_path: null,
    stdout_preview: null,
    stderr_preview: null,
    duration_ms: 100,
    started_at: '2026-09-29T10:00:00Z',
    finished_at: null,
    ...overrides,
  };
}

describe('filterCommands：退出码档位（全部 / 非零 / 零）', () => {
  const rows = [
    command({ id: 1, exit_code: 0 }),
    command({ id: 2, exit_code: 101 }),
    // 还在跑的命令没有退出码——只在「全部」里出现
    command({ id: 3, exit_code: null }),
  ];

  it('all：一条不少', () => {
    expect(filterCommands(rows, '', 'all').map((c) => c.id)).toEqual([1, 2, 3]);
  });

  it('nonzero：只留非零退出码；还在跑（null）的不算', () => {
    expect(filterCommands(rows, '', 'nonzero').map((c) => c.id)).toEqual([2]);
  });

  it('zero：只留干净退出；还在跑（null）的不算', () => {
    expect(filterCommands(rows, '', 'zero').map((c) => c.id)).toEqual([1]);
  });
});

describe('filterCommands：命令行关键词', () => {
  it('折叠行显示的是哪一条就搜哪一条：改写过的行搜**原串**也命中（决策 297 同源）', () => {
    const rows = [
      command({ id: 1, command: 'rtk read src/lib.rs', original_command: 'cat src/lib.rs' }),
      command({ id: 2, command: 'cargo build' }),
    ];
    expect(filterCommands(rows, 'cat src', 'all').map((c) => c.id)).toEqual([1]);
    // 实际执行的那条也搜得到——排障的人两条都可能记得
    expect(filterCommands(rows, 'rtk read', 'all').map((c) => c.id)).toEqual([1]);
  });

  it('大小写不敏感', () => {
    const rows = [command({ command: 'CARGO TEST' })];
    expect(filterCommands(rows, 'cargo', 'all')).toHaveLength(1);
  });

  it('空关键词：不过滤', () => {
    const rows = [command({ id: 1 }), command({ id: 2 })];
    expect(filterCommands(rows, '   ', 'all')).toHaveLength(2);
  });
});

describe('filterCommands：两个过滤叠加', () => {
  it('先按退出码、再按关键词——结果里两条件都成立', () => {
    const rows = [
      command({ id: 1, command: 'cargo test', exit_code: 0 }),
      command({ id: 2, command: 'cargo build', exit_code: 1 }),
      command({ id: 3, command: 'npm test', exit_code: 1 }),
    ];
    expect(filterCommands(rows, 'cargo', 'nonzero').map((c) => c.id)).toEqual([2]);
  });
});
