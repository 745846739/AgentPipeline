import { render, screen } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiError } from '../api/client';
import type { ServerInfo } from '../api/types';
import Share from './Share.svelte';

/**
 * 「手机访问」页在**取不到配对令牌**时的行为（决策 189）。
 *
 * 断言对着一个用户抱怨：手机扫码之后照样报「这台设备还没配对」，而报错页的指引又把人送回
 * 这一页。根因是这一页在非回环来源下会照画一张**裸地址的码**——它与正常的那张在视觉上毫无
 * 区别，扫了却永远配不上。所以这里最要紧的一条是「没有令牌就不画码」。
 *
 * 这条只有把客户端出口换掉才试得出来：真机上它需要一个非回环来源（手机 / 局域网地址），
 * 而单测里没有第二个来源。故这里直接让 `GET /pairing/token` 回 403（服务端对非回环来源的
 * 应答）与回 500 各来一次，钉住两条不同的说法。
 */

const mocks = vi.hoisted(() => ({
  getServerInfo: vi.fn(),
  fetchPairingToken: vi.fn(),
  resetPairing: vi.fn(),
  setServerLan: vi.fn(),
  clearServerLan: vi.fn(),
}));

vi.mock('../api/client', async () => ({
  // 保留真实实现里的 `ApiError`（组件要按它判 403）与 `qrSvgUrl` / `pairedUrl`（地址形状）
  ...(await vi.importActual<typeof import('../api/client')>('../api/client')),
  ...mocks,
}));

const lanInfo: ServerInfo = {
  host: '0.0.0.0',
  port: 8788,
  loopback_only: false,
  bind_source: 'settings',
  port_source: 'config',
  public_base_url: null,
  addresses: [{ interface: 'en0', url: 'http://192.168.1.10:8788', preferred: true }],
};

afterEach(() => {
  vi.resetAllMocks();
});

describe('手机访问页 · 没有配对令牌时不画码（决策 189）', () => {
  it('非回环来源（读取口 403）：给「去哪台机器上打开」的指引，页面上没有二维码', async () => {
    mocks.getServerInfo.mockResolvedValue(lanInfo);
    mocks.fetchPairingToken.mockRejectedValue(
      new ApiError(403, '配对令牌只能在本机读取：请在跑服务的电脑本机打开手机访问页扫码'),
    );
    render(Share);

    await screen.findByText('二维码要在这台电脑本机上打开本页才拿得到');
    // 这条是决策 189 的裁决本身：那张扫了配不上的码不再出现
    expect(screen.queryByAltText(/扫码访问/)).toBeNull();
    // 指引要说清去哪台机器、开哪个地址
    expect(screen.getByText('http://127.0.0.1:8788/#/share')).toBeTruthy();
    expect(screen.getByText(/没有令牌的二维码扫了也配不上/)).toBeTruthy();
  });

  it('令牌到手（本机打开的这一页）：画出带令牌的码，不再出现指引块', async () => {
    mocks.getServerInfo.mockResolvedValue(lanInfo);
    mocks.fetchPairingToken.mockResolvedValue({ token: 'tok' });
    render(Share);

    const img = await screen.findByAltText('扫码访问 http://192.168.1.10:8788/?pair=tok');
    expect(img.getAttribute('src')).toContain('pair%3Dtok');
    expect(screen.queryByText('二维码要在这台电脑本机上打开本页才拿得到')).toBeNull();
  });

  it('别的故障（500）也不画码，并把原因照实说出来', async () => {
    mocks.getServerInfo.mockResolvedValue(lanInfo);
    mocks.fetchPairingToken.mockRejectedValue(new ApiError(500, '500 Internal Server Error'));
    render(Share);

    await screen.findByText('读不到配对令牌');
    expect(screen.queryByAltText(/扫码访问/)).toBeNull();
    expect(screen.getByText('500 Internal Server Error')).toBeTruthy();
  });

  it('服务读数还没到时给「正在读取」而不是先画一张没令牌的码', async () => {
    mocks.getServerInfo.mockResolvedValue(lanInfo);
    // 令牌请求悬着不结算：窗口内不该出现二维码（旧行为在这里画的就是那张裸地址的码）
    mocks.fetchPairingToken.mockReturnValue(new Promise(() => {}));
    render(Share);

    await screen.findByText('正在读取配对令牌…');
    expect(screen.queryByAltText(/扫码访问/)).toBeNull();
  });

  it('只绑回环（没有公网入口）：仍是改绑指引（决策 186 的行为不被这次改动挤掉）', async () => {
    mocks.getServerInfo.mockResolvedValue({
      ...lanInfo,
      host: '127.0.0.1',
      loopback_only: true,
      bind_source: 'config',
      addresses: [],
    });
    mocks.fetchPairingToken.mockResolvedValue({ token: 'tok' });
    render(Share);

    await screen.findByText('手机现在连不上这台机器');
    expect(screen.queryByAltText(/扫码访问/)).toBeNull();
    expect(screen.getByRole('button', { name: /绑定全网卡/ })).toBeTruthy();
  });

  /**
   * 106 的形态（决策 334）：后端只绑回环、门外是 Caddy 的公网入口。
   *
   * 这一页此前在这里说「手机现在连不上这台机器」并递上「绑定全网卡」——按下去不但治不了
   * 病，还会把刚关掉的明文入口装回来（经反向代理进来的请求在守卫眼里是回环，那颗钮按得动）。
   * 现在它该画出指向公网入口的码，且**没有**那颗钮。
   */
  it('只绑回环但有公网入口：画指向入口的码，不再劝你去绑全网卡', async () => {
    mocks.getServerInfo.mockResolvedValue({
      ...lanInfo,
      host: '127.0.0.1',
      loopback_only: true,
      bind_source: 'startup',
      public_base_url: 'https://203.0.113.10:3389',
      addresses: [{ interface: '公网入口', url: 'https://203.0.113.10:3389', preferred: true }],
    });
    mocks.fetchPairingToken.mockResolvedValue({ token: 'tok' });
    render(Share);

    const img = await screen.findByAltText('扫码访问 https://203.0.113.10:3389/?pair=tok');
    expect(img.getAttribute('src')).toContain('pair%3Dtok');
    // 那句「手机现在连不上这台机器」与它的钮都不该出现
    expect(screen.queryByText('手机现在连不上这台机器')).toBeNull();
    expect(screen.queryByRole('button', { name: /绑定全网卡/ })).toBeNull();
    expect(screen.queryByRole('button', { name: /改回只绑本机/ })).toBeNull();
    // 换成说清入口与「怎么改入口」
    expect(screen.getByText(/由外面那道反向代理转发进来/)).toBeTruthy();
    expect(screen.getByText('--public-base-url')).toBeTruthy();
  });

  it('端口是退让来的：页面上说清「这次为什么变了」（决策 213）', async () => {
    // 固定端口被别的程序占着时后端会退让到临时端口——手机上的旧书签正是这样失效的，
    // 而日志在桌面应用里看不到，故这句话必须出现在这一页上。
    mocks.getServerInfo.mockResolvedValue({
      ...lanInfo,
      port: 53311,
      port_source: 'fallback',
      addresses: [{ interface: 'en0', url: 'http://192.168.1.10:53311', preferred: true }],
    });
    mocks.fetchPairingToken.mockResolvedValue({ token: 'tok' });
    render(Share);

    await screen.findByAltText('扫码访问 http://192.168.1.10:53311/?pair=tok');
    expect(screen.getByText(/临时端口 53311/)).toBeTruthy();
    expect(screen.getByText(/重新扫一次/)).toBeTruthy();
  });

  it('端口来自配置（常态）：不出现退让说明', async () => {
    mocks.getServerInfo.mockResolvedValue(lanInfo);
    mocks.fetchPairingToken.mockResolvedValue({ token: 'tok' });
    render(Share);

    await screen.findByAltText('扫码访问 http://192.168.1.10:8788/?pair=tok');
    expect(screen.queryByText(/临时端口/)).toBeNull();
  });
});
