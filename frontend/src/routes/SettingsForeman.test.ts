import { fireEvent, render, screen } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { ForemanWatchSettings } from '../api/types';
import SettingsForeman from './SettingsForeman.svelte';

/**
 * 值守轮设置页的接线（决策 287 / 票 02）。
 *
 * 判据本体在后端契约（core 的开关门 + api_contract 的 provenance）里；这里钉的是**接线**：
 * 读数原样摆出来、两种状态各有一句话（关掉 = 「今晚没人看」——票 02 要求给到的文案）、
 * 按下开关真的把 `{enabled}` 交出去并在重读后换上新读数、节奏五个数**只读在场**。
 *
 * 断言只落在可访问性契约上（标题、按钮名），不落 class 名与内部状态。
 */

const mocks = vi.hoisted(() => ({
  getForemanWatch: vi.fn(),
  setForemanWatch: vi.fn(),
}));

vi.mock('../api/client', () => mocks);

/** 一份读数（缺省 = 开、没保存过）。 */
function readout(over: Partial<ForemanWatchSettings> = {}): ForemanWatchSettings {
  return {
    enabled: true,
    origin: 'default',
    config: {
      watch_event_window_minutes: 30,
      watch_owner_stuck_minutes: 10,
      watch_debounce_sec: 60,
      watch_task_cooldown_minutes: 30,
      watch_max_wakes_per_hour: 12,
    },
    ...over,
  };
}

beforeEach(() => {
  mocks.getForemanWatch.mockResolvedValue(readout());
  mocks.setForemanWatch.mockResolvedValue({ enabled: false, origin: 'settings' });
});

afterEach(() => {
  vi.resetAllMocks();
});

describe('SettingsForeman', () => {
  it('装载后摆出开关读数与只读的节奏五个数', async () => {
    render(SettingsForeman);
    expect(await screen.findByText('[ON]')).toBeTruthy();
    expect(screen.getByText('值守开关')).toBeTruthy();
    // 节奏五个数都在场（只读展示，不是输入框）。
    expect(screen.getByText('事件新鲜窗口')).toBeTruthy();
    expect(screen.getByText('去抖窗口')).toBeTruthy();
    expect(screen.getByText('唤醒上限')).toBeTruthy();
    expect(screen.queryByRole('textbox')).toBeNull();
  });

  it('关掉：把 {enabled:false} 交出去、以重读为准、文案说「今晚没人看」', async () => {
    render(SettingsForeman);
    const button = await screen.findByRole('button', { name: /关掉值守/ });
    mocks.getForemanWatch.mockResolvedValue(readout({ enabled: false, origin: 'settings' }));
    await fireEvent.click(button);
    expect(mocks.setForemanWatch).toHaveBeenCalledWith(false);
    expect(await screen.findByText('[OFF]')).toBeTruthy();
    expect(screen.getAllByText(/今晚没人看/).length).toBeGreaterThan(0);
  });

  it('打开：同一颗钮的另一半，交 {enabled:true} 且重读换上新读数', async () => {
    mocks.getForemanWatch.mockResolvedValue(readout({ enabled: false, origin: 'settings' }));
    render(SettingsForeman);
    const button = await screen.findByRole('button', { name: /打开值守/ });
    mocks.setForemanWatch.mockResolvedValue({ enabled: true, origin: 'settings' });
    mocks.getForemanWatch.mockResolvedValue(readout());
    await fireEvent.click(button);
    expect(mocks.setForemanWatch).toHaveBeenCalledWith(true);
    expect(await screen.findByText('[ON]')).toBeTruthy();
    expect(screen.getAllByText(/今晚有人看/).length).toBeGreaterThan(0);
  });

  it('保存失败：报错不静默，读数以重读为准（本地猜测不冒充结果）', async () => {
    render(SettingsForeman);
    const button = await screen.findByRole('button', { name: /关掉值守/ });
    mocks.setForemanWatch.mockRejectedValue(new Error('落库失败'));
    await fireEvent.click(button);
    expect(await screen.findByRole('alert').then((el) => el.textContent)).toContain('落库失败');
  });
});
