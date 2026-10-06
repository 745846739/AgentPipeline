import { fireEvent, render, screen } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { CompactionSettings } from '../api/types';
import SettingsCompaction from './SettingsCompaction.svelte';

/**
 * 管线压缩设置页的接线（long-run-budget 票 02）。
 *
 * 判据本体在后端契约（core 的 token 硬底谓词 + api_contract 的 provenance）里；
 * 这里钉的是**接线**：读数原样摆出来、provenance 逐字段可见、按下保存真的把
 * 两个值交出去并在重读后换上新读数、非正整数不交出去（本地先拦）。
 *
 * 断言只落在可访问性契约上（标题、按钮名、输入框标签），不落 class 名与内部状态。
 */

const mocks = vi.hoisted(() => ({
  getCompaction: vi.fn(),
  setCompaction: vi.fn(),
}));

vi.mock('../api/client', () => mocks);

/** 一份读数（缺省 = config 层的 300_000 / 5，没保存过）。 */
function readout(over: Partial<CompactionSettings> = {}): CompactionSettings {
  return {
    conversation_max_tokens: 300_000,
    conversation_max_tokens_origin: 'default',
    keep_recent_rounds: 5,
    keep_recent_rounds_origin: 'default',
    config_conversation_max_tokens: 300_000,
    config_keep_recent_rounds: 5,
    ...over,
  };
}

/** 标签正文被插值切成多个文本节点，按 textContent 整体匹配（限 LABEL，免撞祖先）。 */
function labelIn(container: HTMLElement, text: string): HTMLElement {
  const el = Array.from(container.querySelectorAll('label')).find((e) =>
    e.textContent?.includes(text),
  );
  expect(el, `找不到含「${text}」的标签`).toBeTruthy();
  return el as HTMLElement;
}

beforeEach(() => {
  mocks.getCompaction.mockResolvedValue(readout());
  mocks.setCompaction.mockResolvedValue(
    readout({
      conversation_max_tokens: 400_000,
      conversation_max_tokens_origin: 'settings',
      keep_recent_rounds: 8,
      keep_recent_rounds_origin: 'settings',
    }),
  );
});

afterEach(() => {
  vi.resetAllMocks();
});

describe('SettingsCompaction', () => {
  it('装载后摆出两个旋钮的读数与 config 层对照', async () => {
    const { container } = render(SettingsCompaction);
    expect(await screen.findByText('触发线')).toBeTruthy();
    expect(labelIn(container, 'token 硬底（当前 300000，config 值）')).toBeTruthy();
    expect(labelIn(container, '压缩保留轮数（当前 5，config 值）')).toBeTruthy();
    expect(screen.getByText(/config 层的值：token 硬底 300000/)).toBeTruthy();
  });

  it('保存：把两个值交出去、以重读为准、provenance 变「界面保存的」', async () => {
    const { container } = render(SettingsCompaction);
    const tokens = await screen.findByLabelText(/token 硬底/);
    const rounds = await screen.findByLabelText(/压缩保留轮数/);
    await fireEvent.input(tokens, { target: { value: '400000' } });
    await fireEvent.input(rounds, { target: { value: '8' } });
    mocks.getCompaction.mockResolvedValue(
      readout({
        conversation_max_tokens: 400_000,
        conversation_max_tokens_origin: 'settings',
        keep_recent_rounds: 8,
        keep_recent_rounds_origin: 'settings',
      }),
    );
    await fireEvent.click(screen.getByRole('button', { name: /保存/ }));
    expect(mocks.setCompaction).toHaveBeenCalledWith(400_000, 8);
    expect(await screen.findByText(/下一轮 attempt \/ 值守就按新值走/)).toBeTruthy();
    expect(labelIn(container, 'token 硬底（当前 400000，界面保存的）')).toBeTruthy();
  });

  it('非正整数本地先拦：不交出去、报错不静默', async () => {
    render(SettingsCompaction);
    const tokens = await screen.findByLabelText(/token 硬底/);
    await fireEvent.input(tokens, { target: { value: '0' } });
    await fireEvent.submit(screen.getByRole('button', { name: /保存/ }).closest('form')!);
    expect(mocks.setCompaction).not.toHaveBeenCalled();
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('正整数');
  });

  it('保存失败：报错不静默，读数以重读为准（本地猜测不冒充结果）', async () => {
    render(SettingsCompaction);
    const rounds = await screen.findByLabelText(/压缩保留轮数/);
    await fireEvent.input(rounds, { target: { value: '8' } });
    mocks.setCompaction.mockRejectedValue(new Error('落库失败'));
    await fireEvent.click(screen.getByRole('button', { name: /保存/ }));
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('落库失败');
    expect(mocks.getCompaction).toHaveBeenCalledTimes(2);
  });
});
