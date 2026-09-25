import { fireEvent, render, screen } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { NotifySettings } from '../api/types';
import SettingsNotify from './SettingsNotify.svelte';

/**
 * 「离线通知」设置页**礼貌小节**的接线（决策 284②③⑤）。
 *
 * 判据本体在 `lib/notifyPoliteness.test.ts`（纯函数）与后端契约里；这里钉的是**接线**：
 * 读数预填进三个输入框、两个来源各报各的、判读没过时**不发请求**并把错摆出来、
 * 过了则把整体载荷交出去并在重读后换上新读数、`origin === 'settings'` 时才出现「交还」
 * 那颗钮（点了真的调交还）。
 *
 * 断言只落在可访问性契约上（label 名、role、按钮名），不落 class 名与内部状态。
 */

const mocks = vi.hoisted(() => ({
  getNotifySettings: vi.fn(),
  setNotifyEnabled: vi.fn(),
  saveNotifyChannel: vi.fn(),
  clearNotifyChannel: vi.fn(),
  testNotifyChannel: vi.fn(),
  saveNotifyPoliteness: vi.fn(),
  clearNotifyPoliteness: vi.fn(),
}));

vi.mock('../api/client', () => mocks);

/** 一份读数（缺省 = 两级都在配置文件那一级、没有通道声明）。 */
function readout(over: Partial<NotifySettings> = {}): NotifySettings {
  return {
    enabled: true,
    channel: null,
    origin: 'config',
    webhook_url: '',
    bluebubbles_url: '',
    bluebubbles_password: '',
    bluebubbles_recipient: '',
    cooldown_sec: 300,
    quiet_hours: [22, 8],
    politeness_origin: 'config',
    ...over,
  };
}

/** 等页面装载完（礼貌小节的第一个输入框在场）。 */
async function openPage(): Promise<HTMLInputElement> {
  render(SettingsNotify);
  return (await screen.findByLabelText(/节流（秒）/)) as HTMLInputElement;
}

beforeEach(() => {
  mocks.getNotifySettings.mockResolvedValue(readout());
  mocks.saveNotifyPoliteness.mockResolvedValue({ ok: true });
  mocks.clearNotifyPoliteness.mockResolvedValue({ ok: true });
});

afterEach(() => {
  vi.resetAllMocks();
  document.body.innerHTML = '';
});

describe('离线通知设置页 · 礼貌小节的接线（决策 284）', () => {
  it('生效值预填进三个输入框，两个来源各报各的，实时描述跟着算出来', async () => {
    const cooldown = await openPage();
    expect(cooldown.value).toBe('300');
    expect((screen.getByLabelText('免打扰开始（整点）') as HTMLInputElement).value).toBe('22');
    expect((screen.getByLabelText('结束（整点）') as HTMLInputElement).value).toBe('8');
    expect(screen.getByRole('heading', { name: '礼貌' })).toBeTruthy();
    // 通道与礼貌各有一个来源标签（都是配置文件定的）——「通道来自界面不代表礼貌也来自界面」
    expect(screen.getAllByText('配置文件定的').length).toBe(2);
    // 描述按生效值算：节流 + 免打扰两句都在
    expect(screen.getByText(/同类 300 秒内只出站一条/)).toBeTruthy();
    expect(screen.getByText(/22 点–次日 8 点之间除待办与失败外不出站/)).toBeTruthy();
    // 礼貌来自配置文件 → 这一节没有「交还」钮（通道那一节也没有：它同样是配置级）
    expect(screen.queryByRole('button', { name: '交还配置文件' })).toBeNull();
  });

  it('判读没过：不发请求，把错摆在页面上', async () => {
    await openPage();
    await fireEvent.input(screen.getByLabelText('结束（整点）'), {
      target: { value: '24' },
    });
    // 实时判读当场就说了（不等按下保存）
    expect(screen.getAllByText(/免打扰起止要填 0–23 之间的整点/).length).toBeGreaterThan(0);

    await fireEvent.click(screen.getByRole('button', { name: '保存礼貌' }));

    expect(mocks.saveNotifyPoliteness).not.toHaveBeenCalled();
    expect(screen.getAllByText(/免打扰起止要填 0–23 之间的整点/).length).toBeGreaterThan(0);
  });

  it('整体交出去、重读为准：载荷是两件一起，来源翻面后出现「交还」钮', async () => {
    mocks.getNotifySettings
      .mockResolvedValueOnce(readout())
      .mockResolvedValueOnce(
        readout({ politeness_origin: 'settings', quiet_hours: [22, 7] }),
      );
    await openPage();
    await fireEvent.input(screen.getByLabelText('结束（整点）'), {
      target: { value: '7' },
    });
    await fireEvent.click(screen.getByRole('button', { name: '保存礼貌' }));

    expect(mocks.saveNotifyPoliteness).toHaveBeenCalledTimes(1);
    expect(mocks.saveNotifyPoliteness.mock.calls[0][0]).toEqual({
      cooldown_sec: 300,
      quiet_hours: [22, 7],
    });
    // 消息以重读到的读数为准（不拿本地猜测冒充结果）
    expect(await screen.findByText(/礼貌已保存/)).toBeTruthy();
    expect(screen.getByText('界面上的选择定的')).toBeTruthy();
    expect(screen.getByText('配置文件定的')).toBeTruthy();
    expect(screen.getByRole('button', { name: '交还配置文件' })).toBeTruthy();
  });

  it('「交还配置文件」调的是交还礼貌那条路（开关与通道都不动）', async () => {
    mocks.getNotifySettings.mockResolvedValue(readout({ politeness_origin: 'settings' }));
    await openPage();
    await fireEvent.click(screen.getByRole('button', { name: '交还配置文件' }));

    expect(mocks.clearNotifyPoliteness).toHaveBeenCalledTimes(1);
    expect(mocks.clearNotifyChannel).not.toHaveBeenCalled();
    expect(mocks.setNotifyEnabled).not.toHaveBeenCalled();
    expect(await screen.findByText(/已交还配置文件那一级的礼貌/)).toBeTruthy();
  });
});
