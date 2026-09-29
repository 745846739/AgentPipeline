import { describe, expect, it } from 'vitest';

import {
  DENIED_HINT,
  IOS_INSTALL_HINT,
  LOCAL_SUBSCRIPTION_KEY,
  forgetLocalSubscriptionId,
  pushFace,
  pushFaceHint,
  pushFaceLabel,
  readLocalSubscriptionId,
  rememberLocalSubscriptionId,
  urlBase64ToUint8Array,
  type PushEnv,
  type PushFaceKind,
} from './pushSubscribe';

/**
 * 「订阅此设备」的前端判据（pwa-webpush 票 03）：权限四态、公钥的字节形、本机行 id。
 * 页面接线在 `routes/SettingsNotify.test.ts` 里另测。
 */

function env(over: Partial<PushEnv> = {}): PushEnv {
  return {
    isSecureContext: true,
    hasServiceWorker: true,
    hasPushManager: true,
    permission: 'default',
    isIos: false,
    standalone: false,
    ...over,
  };
}

describe('权限四态（pushFace）', () => {
  it('未申请 / 已授权 / 已拒绝 三态各归各位，只有前两态摆可点的钮', () => {
    expect(pushFace(env({ permission: 'default' }))).toEqual({
      kind: 'prompt',
      canSubscribe: true,
    });
    expect(pushFace(env({ permission: 'granted' }))).toEqual({
      kind: 'granted',
      canSubscribe: true,
    });
    expect(pushFace(env({ permission: 'denied' }))).toEqual({
      kind: 'denied',
      canSubscribe: false,
    });
  });

  it('iOS 非主屏优先于一切：即便权限已授权也只给「添加到主屏幕」的引导', () => {
    const face = pushFace(env({ isIos: true, standalone: false, permission: 'granted' }));
    expect(face.kind).toBe('ios-needs-install');
    expect(face.canSubscribe).toBe(false);
    // 主屏 PWA 里同一台设备就是正常一态（已授权 → 可订阅）。
    expect(pushFace(env({ isIos: true, standalone: true, permission: 'granted' }))).toEqual({
      kind: 'granted',
      canSubscribe: true,
    });
  });

  it('环境不满足（非安全上下文 / 没有 service worker / 没有 PushManager / 没有 Notification）→ 不支持', () => {
    for (const broken of [
      env({ isSecureContext: false }),
      env({ hasServiceWorker: false }),
      env({ hasPushManager: false }),
      env({ permission: 'unsupported' }),
    ]) {
      const face = pushFace(broken);
      expect(face.kind).toBe('unsupported');
      expect(face.canSubscribe).toBe(false);
    }
  });

  it('四态的短名与说明文案都非空（页面直接摆它们，空串会摆出一格空白）', () => {
    const kinds: PushFaceKind[] = ['ios-needs-install', 'unsupported', 'denied', 'granted', 'prompt'];
    for (const kind of kinds) {
      expect(pushFaceLabel(kind).length).toBeGreaterThan(0);
      expect(pushFaceHint(kind).length).toBeGreaterThan(0);
    }
    expect(pushFaceHint('ios-needs-install')).toBe(IOS_INSTALL_HINT);
    expect(pushFaceHint('denied')).toBe(DENIED_HINT);
    expect(IOS_INSTALL_HINT).toContain('添加到主屏幕');
    expect(DENIED_HINT).toContain('系统设置');
  });
});

describe('VAPID 公钥的字节形（urlBase64ToUint8Array）', () => {
  it('base64url 的两种字母表都要认（`-`/`_` 与 `+`/`/` 等价）', () => {
    // 0xFB 0xFF → base64 标准是 `+/8`，base64url 是 `-_8`。
    expect(Array.from(urlBase64ToUint8Array('-_8')!)).toEqual([0xfb, 0xff]);
    expect(Array.from(urlBase64ToUint8Array('+/8')!)).toEqual([0xfb, 0xff]);
  });

  it('长度不是 4 的倍数时自己补填充（服务端的 base64url 是**无填充**形）', () => {
    // `AQ` = 1 字节（补 2 个 =），`AQI` = 2 字节（补 1 个 =）。
    expect(Array.from(urlBase64ToUint8Array('AQ')!)).toEqual([0x01]);
    expect(Array.from(urlBase64ToUint8Array('AQI')!)).toEqual([0x01, 0x02]);
    expect(Array.from(urlBase64ToUint8Array('AQID')!)).toEqual([0x01, 0x02, 0x03]);
  });

  it('一段真实的公钥长度（65 字节未压缩点 → 87 个 base64url 字符）解得出 65 字节', () => {
    const point = new Uint8Array(65);
    point[0] = 0x04;
    for (let i = 1; i < 65; i += 1) point[i] = i;
    const encoded = Buffer.from(point).toString('base64url');
    expect(encoded.length).toBe(87);
    expect(Array.from(urlBase64ToUint8Array(encoded)!)).toEqual(Array.from(point));
  });

  it('空串与非法字符返回 null（调用方据此说话，而不是拿垃圾去订阅）', () => {
    expect(urlBase64ToUint8Array('')).toBeNull();
    expect(urlBase64ToUint8Array('   ')).toBeNull();
    expect(urlBase64ToUint8Array('!!!not base64!!!')).toBeNull();
    expect(urlBase64ToUint8Array('中文')).toBeNull();
  });
});

describe('本机那条订阅的行 id', () => {
  function fakeStore(initial: Record<string, string> = {}) {
    const map = new Map(Object.entries(initial));
    return {
      getItem: (k: string) => map.get(k) ?? null,
      setItem: (k: string, v: string) => void map.set(k, v),
      removeItem: (k: string) => void map.delete(k),
      dump: () => map,
    };
  }

  it('记得住、读得回、忘得掉', () => {
    const store = fakeStore();
    expect(readLocalSubscriptionId(store)).toBeNull();
    rememberLocalSubscriptionId(42, store);
    expect(store.dump().get(LOCAL_SUBSCRIPTION_KEY)).toBe('42');
    expect(readLocalSubscriptionId(store)).toBe(42);
    forgetLocalSubscriptionId(store);
    expect(readLocalSubscriptionId(store)).toBeNull();
  });

  it('存进去的是乱七八糟的东西时读回 null（不把 NaN 当 id 去删别人的行）', () => {
    expect(readLocalSubscriptionId(fakeStore({ [LOCAL_SUBSCRIPTION_KEY]: '不是数字' }))).toBeNull();
    expect(readLocalSubscriptionId(fakeStore({ [LOCAL_SUBSCRIPTION_KEY]: '' }))).toBeNull();
  });
});
