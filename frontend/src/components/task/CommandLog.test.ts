import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import type { NodeCommand } from '../../api/types';
import CommandLog from './CommandLog.svelte';

/**
 * 命令与输出 · 完整输出的读取失败（票 12 / R2-16）。
 *
 * 此前 `commandOutputError` **存了没人读**：取不到完整输出时这一段永远显示
 * 「（正在加载完整输出…）」——一句能永久停住的谎。这里钉的是「失败说得出来」。
 */

function command(overrides: Partial<NodeCommand> = {}): NodeCommand {
  return {
    id: 1,
    task_id: 't1',
    node_run_id: 1,
    source: 'agent',
    command: 'cargo test',
    started_at: '2026-09-18T10:00:00Z',
    duration_ms: 1200,
    exit_code: 0,
    stdout_path: 'commands/1.stdout',
    stdout_preview: 'test result: ok',
    ...overrides,
  } as NodeCommand;
}

describe('命令与输出（票 12 / R2-16）', () => {
  it('读取失败：显示失败原因，不再说「正在加载完整输出…」', async () => {
    render(CommandLog, {
      props: {
        commands: [command()],
        // 取不到（`outputFor` 没有这一条）→ onload 走失败分支
        outputFor: () => null,
        onload: () => undefined,
        errorFor: () => '404 Not Found',
      },
    });

    await fireEvent.click(screen.getByRole('button', { name: /cargo test/ }));

    const alert = screen.getByRole('alert');
    expect(alert.textContent).toContain('完整输出没读回来');
    expect(alert.textContent).toContain('404 Not Found');
    expect(screen.queryByText(/正在加载完整输出/)).toBeNull();
  });

  it('改写过的行：折叠显示**原串**、带「改写」标，展开时两条都摆出来（决策 297）', async () => {
    render(CommandLog, {
      props: {
        commands: [
          command({ command: 'rtk read src/lib.rs', original_command: 'cat src/lib.rs' }),
        ],
        outputFor: () => '全文',
        onload: () => undefined,
        errorFor: () => null,
      },
    });

    // 折叠行显示的是模型想要的那一条，不是被换成的那一条
    const button = screen.getByRole('button', { name: /cat src\/lib\.rs/ });
    expect(screen.getByText('改写')).not.toBeNull();
    expect(button.textContent).toContain('cat src/lib.rs');

    await fireEvent.click(button);
    // 展开里两条都在：原串与实际执行的那条
    expect(screen.getByText(/实际执行：rtk read src\/lib\.rs/)).not.toBeNull();
  });

  it('没改写过的行（绝大多数）：照旧只显示那一条，不带标（决策 297）', async () => {
    render(CommandLog, {
      props: {
        commands: [command({ command: 'cargo test', original_command: null })],
        outputFor: () => 'ok',
        onload: () => undefined,
        errorFor: () => null,
      },
    });

    expect(screen.getByText('cargo test')).not.toBeNull();
    expect(screen.queryByText('改写')).toBeNull();
  });

  it('没有错误时：preview 照旧显示（失败分支不误伤正常路径）', async () => {
    render(CommandLog, {
      props: {
        commands: [command()],
        outputFor: () => 'test result: ok. 12 passed',
        onload: () => undefined,
        errorFor: () => null,
      },
    });

    await fireEvent.click(screen.getByRole('button', { name: /cargo test/ }));

    expect(screen.queryByRole('alert')).toBeNull();
    expect(screen.getByText(/12 passed/)).toBeTruthy();
  });

  it('加载中：先说加载中（异步窗口里的话不该是失败）', async () => {
    let release = (): void => undefined;
    const onload = vi.fn(() => new Promise<void>((resolve) => (release = resolve)));
    render(CommandLog, {
      props: { commands: [command()], outputFor: () => null, onload, errorFor: () => null },
    });

    await fireEvent.click(screen.getByRole('button', { name: /cargo test/ }));
    expect(screen.getByText(/正在加载完整输出/)).toBeTruthy();

    release();
  });
});

/**
 * 长列表窗口化（spec list-windowing 票 02）：默认显尾部 50 条、「已省略」行点得开、
 * 过滤与切片可叠加（先过滤后切）。过滤纯函数自身的判据在 `lib/commandFilter.test.ts`。
 */
describe('命令页签：切片显尾部 + 过滤（票 02）', () => {
  function rows(n: number): NodeCommand[] {
    return Array.from({ length: n }, (_, i) =>
      command({ id: i + 1, command: `cmd-${i + 1}`, exit_code: i === 5 ? 101 : 0 }),
    );
  }

  function visibleCommands(container: HTMLElement): number {
    return container.querySelectorAll('button.cmd').length;
  }

  it('超上限：只画尾部 50 条，顶部一行「已省略前 N 条，点此展开」', () => {
    const { container } = render(CommandLog, { props: { commands: rows(137) } });
    expect(visibleCommands(container)).toBe(50);
    // 最新的一条（最后一条）在场
    expect(screen.getByRole('button', { name: /cmd-137/ })).toBeTruthy();
    expect(screen.queryByRole('button', { name: /cmd-1\b/ })).toBeNull();
    const hint = screen.getByRole('button', { name: '已省略前 87 条，点此展开' });
    expect(hint).toBeTruthy();
  });

  it('点「已省略」行：游标推进一页（50 → 100），省略计数跟着缩', async () => {
    const { container } = render(CommandLog, { props: { commands: rows(137) } });
    await fireEvent.click(screen.getByRole('button', { name: /已省略前 87 条/ }));
    expect(visibleCommands(container)).toBe(100);
    expect(screen.getByRole('button', { name: '已省略前 37 条，点此展开' })).toBeTruthy();
  });

  it('没超上限：不出现「已省略」行', () => {
    const { container } = render(CommandLog, { props: { commands: rows(30) } });
    expect(visibleCommands(container)).toBe(30);
    expect(screen.queryByRole('button', { name: /已省略前/ })).toBeNull();
  });

  it('过滤与切片叠加（先过滤后切）：127 条失败里只画尾部 50 条失败', async () => {
    const all = rows(137).map((c) => ({ ...c, exit_code: 101 }));
    const { container } = render(CommandLog, { props: { commands: all } });
    await fireEvent.click(screen.getByRole('button', { name: '非零' }));
    expect(visibleCommands(container)).toBe(50);
    // 过滤后的总数还是 137，省略计数按**过滤后**的名单算
    expect(screen.getByRole('button', { name: '已省略前 87 条，点此展开' })).toBeTruthy();
  });

  it('关键词过滤：命令行搜得到，换档后窗口游标回缺省', async () => {
    const all = rows(137);
    const { container } = render(CommandLog, { props: { commands: all } });
    // 先把窗口推到全量
    await fireEvent.click(screen.getByRole('button', { name: /已省略前 87 条/ }));
    await fireEvent.click(screen.getByRole('button', { name: /已省略前 37 条/ }));
    expect(visibleCommands(container)).toBe(137);
    // 然后搜「cmd-10」：命中的行远少于 50 条，窗口回到缺省也不至于漏
    const input = screen.getByLabelText('按命令行关键词过滤') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'cmd-10' } });
    // cmd-10 与 cmd-100…cmd-109 共 11 条
    expect(visibleCommands(container)).toBe(11);
    expect(screen.queryByRole('button', { name: /已省略前/ })).toBeNull();
  });
});
