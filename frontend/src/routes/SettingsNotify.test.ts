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
    vapid_public_key: '',
    vapid_private_key: '',
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
        readout({ politeness_origin: 'settings',
    vapid_public_key: '',
    vapid_private_key: '', quiet_hours: [22, 7] }),
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
    mocks.getNotifySettings.mockResolvedValue(readout({ politeness_origin: 'settings', vapid_public_key: '', vapid_private_key: '' }));
    await openPage();
    await fireEvent.click(screen.getByRole('button', { name: '交还配置文件' }));

    expect(mocks.clearNotifyPoliteness).toHaveBeenCalledTimes(1);
    expect(mocks.clearNotifyChannel).not.toHaveBeenCalled();
    expect(mocks.setNotifyEnabled).not.toHaveBeenCalled();
    expect(await screen.findByText(/已交还配置文件那一级的礼貌/)).toBeTruthy();
  });
});

/**
 * 浏览器推送那一节（pwa-webpush 票 03/04）：通道第四支 + 订阅按钮 + 权限四态 + 设备清单。
 *
 * 判据本体在 `lib/pushSubscribe.test.ts` 与后端契约里；这里钉的是**接线**：切到第四支并
 * 保存时送出的载荷、四态各自摆出什么（按钮 / 引导文案）、点订阅是否真的走
 * `requestPermission → subscribe → 上报服务端` 这一条链、清单增删调的是哪几条路。
 *
 * 环境（service worker / PushManager / Notification / isSecureContext）在这里就地搭出来
 * ——jsdom 没有它们，而这一节的全部价值就在「环境不同、界面不同」。
 */
const pushMocks = vi.hoisted(() => ({
  listPushSubscriptions: vi.fn(),
  subscribePushDevice: vi.fn(),
  deletePushSubscription: vi.fn(),
  clearPushSubscriptions: vi.fn(),
}));

vi.mock('../api/client', () => ({ ...mocks, ...pushMocks }));

/** 一台设备的订阅清单行。 */
function deviceRow(over: Record<string, unknown> = {}) {
  return {
    id: 7,
    endpoint_hint: '…abcdef…uvwxyz',
    user_agent: 'iPhone Safari',
    created_at: '2026-09-29T10:00:00+00:00',
    ...over,
  };
}

/** 摆好推送环境：注册器 + PushManager 能力 + 权限 + iOS 主屏与否。 */
function stubPushEnv(opts: {
  permission?: 'default' | 'granted' | 'denied';
  /** `requestPermission()` 的回话；缺省跟着 `permission` 走（点了就同意）。 */
  grant?: 'default' | 'granted' | 'denied';
  isIos?: boolean;
  standalone?: boolean;
  secure?: boolean;
  manager?: boolean;
  subscribed?: boolean;
} = {}) {
  const fetchPermission = opts.permission ?? 'granted';
  const requestPermission = vi
    .fn()
    .mockResolvedValue(opts.grant ?? (fetchPermission === 'denied' ? 'denied' : 'granted'));
  vi.stubGlobal('Notification', { permission: fetchPermission, requestPermission });
  vi.stubGlobal('PushManager', class PushManagerStub {});
  Object.defineProperty(window, 'isSecureContext', {
    value: opts.secure ?? true,
    configurable: true,
  });
  if (opts.manager === false) {
    // 「环境不支持」那一态：连 PushManager 都没有。
    Reflect.deleteProperty(window, 'PushManager');
  }
  // 「点订阅」永远造出一条新订阅（浏览器的行为）；`getSubscription` 则回答「这台浏览器里
  // 现在有没有一条」——缺省是没有，要用「已订阅」那一态时显式传 `subscribed: true`。
  const created = {
    toJSON: () => ({
      endpoint: 'https://push.example.net/device-abcdef',
      keys: { p256dh: 'p256dh-value', auth: 'auth-value' },
    }),
    unsubscribe: vi.fn().mockResolvedValue(true),
  };
  const existing = opts.subscribed ? created : null;
  const registration = {
    pushManager: {
      subscribe: vi.fn().mockResolvedValue(created),
      getSubscription: vi.fn().mockResolvedValue(existing),
    },
  };
  Object.defineProperty(navigator, 'serviceWorker', {
    value: {
      register: vi.fn().mockResolvedValue(registration),
      ready: Promise.resolve(registration),
      getRegistration: vi.fn().mockResolvedValue(registration),
    },
    configurable: true,
  });
  // iOS 判据读的是 UA 与 standalone：用 UA 摆出 iPhone，standalone 由属性给。
  Object.defineProperty(navigator, 'userAgent', {
    value: opts.isIos
      ? 'Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15'
      : 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)',
    configurable: true,
  });
  Object.defineProperty(navigator, 'standalone', {
    value: Boolean(opts.standalone),
    configurable: true,
  });
  return { registration, requestPermission };
}

/** 一把形状正确的 VAPID 公钥（65 字节未压缩点 → 87 个 base64url 字符）。 */
const VAPID_PUBLIC_KEY = (() => {
  const bytes = new Uint8Array(65);
  bytes[0] = 0x04;
  for (let i = 1; i < 65; i += 1) bytes[i] = i;
  return Buffer.from(bytes).toString('base64url');
})();

/** 打开这一页（读数是浏览器推送频道 + 有一对 VAPID 密钥）。 */
async function openPushPage(over: Partial<NotifySettings> = {}) {
  mocks.getNotifySettings.mockResolvedValue(
    readout({
      channel: 'webpush',
      origin: 'settings',
      vapid_public_key: VAPID_PUBLIC_KEY,
      vapid_private_key: '***',
      ...over,
    }),
  );
  return openPage();
}

describe('离线通知设置页 · 浏览器推送一节的接线（pwa-webpush）', () => {
  beforeEach(() => {
    pushMocks.listPushSubscriptions.mockResolvedValue({ subscriptions: [] });
    pushMocks.subscribePushDevice.mockResolvedValue({ ok: true, id: 7 });
    pushMocks.deletePushSubscription.mockResolvedValue({ ok: true, removed: true });
    pushMocks.clearPushSubscriptions.mockResolvedValue({ ok: true, removed: 2 });
  });

  afterEach(() => {
    Reflect.deleteProperty(navigator, 'serviceWorker');
    Reflect.deleteProperty(navigator, 'standalone');
    Reflect.deleteProperty(window, 'Notification');
    Reflect.deleteProperty(window, 'PushManager');
    vi.unstubAllGlobals();
  });

  it('第四支 chip 在场；选中它保存时载荷只带通道名，且不摆别人家的输入框', async () => {
    stubPushEnv();
    await openPushPage();
    expect(screen.getByRole('radio', { name: '浏览器推送' })).toBeTruthy();
    // 当前是 webpush（读数如此）→ 这一格没有 webhook / BlueBubbles 的输入框
    expect(screen.queryByLabelText(/webhook 地址/)).toBeNull();
    expect(screen.queryByLabelText('BlueBubbles 端点')).toBeNull();

    await fireEvent.click(screen.getByRole('button', { name: '保存通道' }));
    expect(mocks.saveNotifyChannel).toHaveBeenCalledWith({ channel: 'webpush' });
    expect(await screen.findByText(/VAPID 密钥对已在服务端生成/)).toBeTruthy();
  });

  it('未申请 → 摆可点的「订阅此设备」；点它走完权限 → 订阅 → 上报那一条链', async () => {
    const { requestPermission, registration } = stubPushEnv({
      permission: 'default',
      grant: 'granted',
    });
    await openPushPage();
    const button = await screen.findByRole('button', { name: '订阅此设备' });

    await fireEvent.click(button);

    expect(requestPermission).toHaveBeenCalledTimes(1);
    expect(registration.pushManager.subscribe).toHaveBeenCalledTimes(1);
    // 订阅参数：userVisibleOnly + 服务端那把公钥（解成字节）
    const options = registration.pushManager.subscribe.mock.calls[0][0];
    expect(options.userVisibleOnly).toBe(true);
    const keyBytes = Array.from(options.applicationServerKey as Uint8Array);
    // 未压缩的 P-256 点：65 字节且首字节 0x04
    expect(keyBytes.length).toBe(65);
    expect(keyBytes[0]).toBe(0x04);
    // 上报那一步在手势之后的异步链上——先等页面把结果摆出来，再核对载荷。
    expect(await screen.findByText(/这台设备已订阅/)).toBeTruthy();
    // 上报的三件来自浏览器的 toJSON（形状原样，不做转换）
    expect(pushMocks.subscribePushDevice).toHaveBeenCalledWith({
      endpoint: 'https://push.example.net/device-abcdef',
      keys: { p256dh: 'p256dh-value', auth: 'auth-value' },
    });
  });

  it('已拒绝 → 不摆按钮，摆去系统设置的指引', async () => {
    stubPushEnv({ permission: 'denied' });
    await openPushPage();
    await screen.findByRole('heading', { name: '订阅' });
    expect(screen.queryByRole('button', { name: '订阅此设备' })).toBeNull();
    expect(screen.getByText(/通知权限已被拒绝/)).toBeTruthy();
    expect(screen.getByText(/系统设置/)).toBeTruthy();
  });

  it('iOS 非主屏 → 按钮位换成「请先添加到主屏幕」的引导', async () => {
    stubPushEnv({ permission: 'granted', isIos: true, standalone: false });
    await openPushPage();
    await screen.findByRole('heading', { name: '订阅' });
    expect(screen.queryByRole('button', { name: '订阅此设备' })).toBeNull();
    expect(screen.getByText('请先添加到主屏幕')).toBeTruthy();
    expect(screen.getByText(/添加到主屏幕：点分享/)).toBeTruthy();
  });

  it('非安全上下文（局域网 http）→ 不支持那一态，按钮不摆', async () => {
    stubPushEnv({ secure: false, permission: 'granted' });
    await openPushPage();
    await screen.findByRole('heading', { name: '订阅' });
    expect(screen.queryByRole('button', { name: '订阅此设备' })).toBeNull();
    expect(screen.getByText(/推送只在 HTTPS/)).toBeTruthy();
  });

  it('空清单给订阅引导而不是空白；有行时显示摘要与 UA，撤销调的是删那一行', async () => {
    stubPushEnv({ permission: 'granted' });
    await openPushPage();
    expect(await screen.findByText(/还没有设备订阅/)).toBeTruthy();

    pushMocks.listPushSubscriptions.mockResolvedValue({ subscriptions: [deviceRow()] });
    await fireEvent.click(screen.getByRole('button', { name: '保存通道' }));
    expect(await screen.findByText('…abcdef…uvwxyz')).toBeTruthy();
    expect(screen.getByText('iPhone Safari')).toBeTruthy();

    await fireEvent.click(screen.getByRole('button', { name: '撤销' }));
    expect(pushMocks.deletePushSubscription).toHaveBeenCalledWith(7);
    expect(await screen.findByText(/那一台设备已撤销/)).toBeTruthy();
  });

  it('「全部清空」调的是清空那条路，并把清了几个报出来', async () => {
    stubPushEnv({ permission: 'granted' });
    pushMocks.listPushSubscriptions.mockResolvedValue({
      subscriptions: [deviceRow(), deviceRow({ id: 8, user_agent: 'Chrome' })],
    });
    await openPushPage();
    await screen.findByText('iPhone Safari');

    await fireEvent.click(screen.getByRole('button', { name: '全部清空' }));
    expect(pushMocks.clearPushSubscriptions).toHaveBeenCalledTimes(1);
    expect(await screen.findByText(/已清空 2 条订阅/)).toBeTruthy();
  });

  it('通道不是浏览器推送时不摆这一节（订阅了也不会被推）', async () => {
    stubPushEnv();
    mocks.getNotifySettings.mockResolvedValue(
      readout({ channel: 'feishu', origin: 'settings', vapid_public_key: '', vapid_private_key: '' }),
    );
    await openPage();
    expect(screen.queryByRole('heading', { name: '订阅' })).toBeNull();
    expect(pushMocks.listPushSubscriptions).not.toHaveBeenCalled();
  });
});
