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
