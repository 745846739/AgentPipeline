import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { OffloadSettings, RtkSettings } from '../api/types';
import SettingsTools from './SettingsTools.svelte';

/**
 * 「命令执行」页的接线层（决策 297 / 票 05）。
 *
 * 判据（三态分界与两句话）钉在 `lib/rtkToggle.test.ts`；这里钉的是**接线**：
 * 每次打开这一页都重读一次活体探测、保存失败不静默、探测失败时手填框在。
 * 断言只落在可访问性契约上（heading / alert / 输入框的可读名），不落 class 名。
 */

const mocks = vi.hoisted(() => ({
  getRtk: vi.fn(),
  setRtk: vi.fn(),
  getOffload: vi.fn(),
  setOffload: vi.fn(),
}));

vi.mock('../api/client', () => ({
  getRtk: mocks.getRtk,
  setRtk: mocks.setRtk,
  getOffload: mocks.getOffload,
  setOffload: mocks.setOffload,
}));

/** 外发开关的缺省读数（票 runner-offload/05）：同一页的第二颗钮。 */
const OFFLOAD_OFF: OffloadSettings = {
  enabled: false,
  origin: 'default',
  probe: { gh_authed: false, gh_reason: null, workflow_present: false },
  last_failure_at: null,
};

const OFFLOAD_ON_OK: OffloadSettings = {
  enabled: true,
  origin: 'settings',
  probe: { gh_authed: true, gh_reason: null, workflow_present: true },
  last_failure_at: null,
};

mocks.getOffload.mockResolvedValue(OFFLOAD_OFF);
mocks.setOffload.mockResolvedValue(OFFLOAD_ON_OK);

const OFF: RtkSettings = {
  enabled: false,
  origin: 'default',
  path: null,
  probe: {
    available: false,
    path: null,
    source: null,
    version: null,
    reason: '没找到 rtk：服务进程的 PATH 与五个已知目录里都没有。',
  },
};

const ON_READY: RtkSettings = {
  enabled: true,
  origin: 'settings',
  path: null,
  probe: {
    available: true,
    path: '/usr/local/bin/rtk',
    source: 'path',
    version: 'rtk 0.42.4',
    reason: null,
  },
};

const ON_UNAVAILABLE: RtkSettings = {
  enabled: true,
  origin: 'settings',
  path: null,
  probe: {
    available: false,
    path: null,
    source: null,
    version: null,
    reason: '找到了 /opt/rtk，但它不能改写（版本太老？）。',
  },
};

afterEach(() => {
  vi.resetAllMocks();
  document.body.innerHTML = '';
});

async function rendered(settings: RtkSettings) {
  mocks.getRtk.mockResolvedValue(settings);
  render(SettingsTools);
  await screen.findByRole('heading', { level: 1, name: '设置 · 命令执行' });
  await waitFor(() => expect(mocks.getRtk).toHaveBeenCalled());
}

describe('命令执行页（决策 297 / 票 05）', () => {
  it('三态各显示各的：关着 / 开着且可用（给路径与版本）/ 开着但用不了（给原因）', async () => {
    await rendered(OFF);
    expect(screen.getByText('[OFF]')).not.toBeNull();

    document.body.innerHTML = '';
    await rendered(ON_READY);
    expect(screen.getByText('[ON]')).not.toBeNull();
    expect(screen.getByText(/\/usr\/local\/bin\/rtk · rtk 0\.42\.4/)).not.toBeNull();

    document.body.innerHTML = '';
    await rendered(ON_UNAVAILABLE);
    expect(screen.getByText('[ON · 用不了]')).not.toBeNull();
    expect(screen.getByText(/但它不能改写/)).not.toBeNull();
    // 用不了 → 手填框在场（那是这一页给出的出路）
    expect(screen.getByLabelText('手填 rtk 的绝对路径')).not.toBeNull();
  });

  it('每次打开这一页都重读一次活体探测（不缓存上一次的结果）', async () => {
    await rendered(ON_READY);
    expect(mocks.getRtk).toHaveBeenCalledTimes(1);
    // 再打开一次 = 再问一次这台机器（拿上次的结论糊过去就是「设置页说可用、命令全 127」）
    document.body.innerHTML = '';
    await rendered(ON_READY);
    expect(mocks.getRtk).toHaveBeenCalledTimes(2);
  });

  it('打开：保存后以重读到的读数为准，成功那条说「本机可用」', async () => {
    await rendered(OFF);
    mocks.setRtk.mockResolvedValue(ON_READY);
    await fireEvent.click(screen.getByRole('button', { name: '打开' }));
    await waitFor(() => expect(mocks.setRtk).toHaveBeenCalledTimes(1));
    // 空白草稿归一成「没说」（回到自动解析）——不是填了一个空路径
    expect(mocks.setRtk).toHaveBeenCalledWith({ enabled: true, path: '' });
    const note = await screen.findByRole('status');
    expect(note.textContent).toContain('本机可用');
    await waitFor(() => expect(mocks.getRtk).toHaveBeenCalledTimes(2));
  });

  it('探测失败**仍然保存**：两件事一起说——已启用 + 当前找不到能用的', async () => {
    await rendered(OFF);
    mocks.setRtk.mockResolvedValue(ON_UNAVAILABLE);
    // 打开之后 getRtk 要回「开着但用不了」，那条提示才是真的
    mocks.getRtk.mockResolvedValue(ON_UNAVAILABLE);
    await fireEvent.click(screen.getByRole('button', { name: '打开' }));
    await waitFor(() => expect(mocks.setRtk).toHaveBeenCalledTimes(1));
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('已启用');
    expect(alert.textContent).toContain('找不到能用的 rtk');
    // 不静默成功：那颗钮真的发过请求（不是本地翻转状态糊过去）
    expect(mocks.setRtk).toHaveBeenCalled();
  });

  it('保存请求本身失败：错误原样播报，并重读读数', async () => {
    await rendered(OFF);
    mocks.setRtk.mockRejectedValue(new Error('保存失败：500'));
    await fireEvent.click(screen.getByRole('button', { name: '打开' }));
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('保存失败：500');
    await waitFor(() => expect(mocks.getRtk).toHaveBeenCalledTimes(2));
  });

  it('手填一条路径：整条交出去（不是只改本地草稿）', async () => {
    await rendered(ON_UNAVAILABLE);
    const input = screen.getByLabelText('手填 rtk 的绝对路径');
    mocks.setRtk.mockResolvedValue(ON_READY);
    await fireEvent.input(input, { target: { value: ' /opt/homebrew/bin/rtk ' } });
    await fireEvent.click(screen.getByRole('button', { name: '保存路径' }));
    await waitFor(() => expect(mocks.setRtk).toHaveBeenCalledTimes(1));
    expect(mocks.setRtk).toHaveBeenCalledWith({ enabled: true, path: '/opt/homebrew/bin/rtk' });
  });

  it('说清与技能的关系，但明说不自动改技能配置', async () => {
    await rendered(ON_READY);
    expect(screen.getByText(/本页不会自动改/)).not.toBeNull();
    expect(screen.getByRole('link', { name: '技能市场' }).getAttribute('href')).toBe(
      '#/settings/market',
    );
  });
  it('外发卡（票 runner-offload/05）按探测读数渲染:开着且探测齐全=外发', async () => {
    // rtk 置于开启态:页上只剩外发卡一颗「打开」钮,断言不撞歧义。
    mocks.getRtk.mockResolvedValue(ON_READY);
    mocks.getOffload.mockResolvedValue(OFFLOAD_ON_OK);
    render(SettingsTools);
    expect(await screen.findByText('开启：重活外发')).toBeTruthy();
    expect(
      screen.getByText(/agent 可把全量测试 \/ clippy \/ 构建外发到 GitHub runner/),
    ).toBeTruthy();
  });

  it('外发卡:探测有缺口时保存**不拦**,提示回退本机执行并留痕', async () => {
    mocks.getRtk.mockResolvedValue(ON_READY);
    mocks.getOffload.mockResolvedValue(OFFLOAD_OFF);
    // 保存读数:开着,但 gh 未登录(先开开关、后登录是共识里写明的顺序)
    mocks.setOffload.mockResolvedValue({
      enabled: true,
      origin: 'settings',
      probe: { gh_authed: false, gh_reason: 'gh 不在场', workflow_present: false },
      last_failure_at: null,
    });
    render(SettingsTools);
    await fireEvent.click(await screen.findByRole('button', { name: '打开' }));
    expect(await screen.findByText(/回退本机执行并留痕/)).toBeTruthy();
  });

  it('外发卡:最近一次链路失败带出时间戳,没失败过的机器读「无」(票 runner-offload/08)', async () => {
    // 落过一笔链路失败:时间戳原样摆出来,并说清外发正在静默降级。
    mocks.getRtk.mockResolvedValue(ON_READY);
    mocks.getOffload.mockResolvedValue({
      ...OFFLOAD_ON_OK,
      last_failure_at: '2026-10-03T02:03:04+00:00',
    });
    render(SettingsTools);
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('2026-10-03T02:03:04');
    expect(screen.getByText(/外发在静默降级/)).toBeTruthy();

    // 从没失败过的机器:读数是「无」,不是「0」(诚实口径,决策 257)。
    document.body.innerHTML = '';
    mocks.getOffload.mockResolvedValue(OFFLOAD_ON_OK);
    render(SettingsTools);
    await screen.findByText('开启：重活外发');
    expect(screen.getByText('无')).toBeTruthy();
  });

});
