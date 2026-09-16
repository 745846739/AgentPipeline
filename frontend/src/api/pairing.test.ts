import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  PAIRING_HEADER,
  PAIRING_QUERY,
  capturePairingFromLocation,
  clearPairingToken,
  getPairingToken,
  setPairingToken,
} from './config';
import { pairedUrl } from './client';

/**
 * 配对令牌的取用与消费（决策 182㉙，**由决策 191 修订**）。
 *
 * 两条各自对应一条用户故事：
 * ① 二维码递过来的 `?pair=` 必须**存下来**（用户故事 14：每次重扫会让人干脆把令牌关掉）；
 * ② **参数不从地址栏抹掉**（191）——手机「添加到主屏幕」保存的就是当时那条 URL，而 iOS 的
 *    主屏 web app 与 Safari 存储隔离，「抹掉」等于让主屏图标每次从零开始配对（实测反馈：
 *    添加主屏后无法二次访问）。182㉙ 原先要抹掉的三条理由里，`Referer` 那条改由响应头
 *    `Referrer-Policy: no-referrer` 关掉（`assets.rs`），截图与历史那两条如实留下。
 * ③ 未配对时**不带这个头**（回环形态零摩擦是本设计的前提）。
 */

/** 最小 localStorage 替身：jsdom 之外也能跑，且能模拟「存储被禁」。 */
function stubStorage(seed: Record<string, string> = {}, throwOnAccess = false): void {
  const map = new Map(Object.entries(seed));
  vi.stubGlobal('window', {
    localStorage: {
      getItem: (k: string) => {
        if (throwOnAccess) throw new Error('storage disabled');
        return map.get(k) ?? null;
      },
      setItem: (k: string, v: string) => {
        if (throwOnAccess) throw new Error('storage disabled');
        map.set(k, v);
      },
      removeItem: (k: string) => {
        if (throwOnAccess) throw new Error('storage disabled');
        map.delete(k);
      },
    },
  });
}

afterEach(() => {
  vi.unstubAllGlobals();
  // 进程内缓存跨用例存活，必须显式清掉，否则上一条的令牌会漏进下一条。
  clearPairingToken();
});

describe('配对令牌（决策 182㉙ / 191）', () => {
  it('从地址栏收下令牌并存本地（地址栏那份不再被抹掉）', () => {
    stubStorage();
    const token = capturePairingFromLocation({ search: `?${PAIRING_QUERY}=abc123` });

    expect(token).toBe('abc123');
    expect(getPairingToken()).toBe('abc123');
  });

  it('地址栏里还有别的参数也不影响读取（令牌与它们并存）', () => {
    stubStorage();
    expect(
      capturePairingFromLocation({ search: `?${PAIRING_QUERY}=abc123&tab=board` }),
    ).toBe('abc123');
    expect(getPairingToken()).toBe('abc123');
  });

  it('每一次装载都从 URL 重新存一次——主屏图标与书签靠的就是这一条', () => {
    // 「以 URL 为准」是 191 的取舍：地址栏里那条是使用者的显式动作（点图标 / 点书签 /
    // 扫新码），而本地那份可能属于别人或已经陈旧。
    stubStorage({ 'agentpipeline.pairing': 'old' });
    expect(capturePairingFromLocation({ search: `?${PAIRING_QUERY}=fresh` })).toBe('fresh');
    expect(getPairingToken()).toBe('fresh');
  });

  it('地址栏里没有配对参数时不动已存的那份', () => {
    stubStorage({ 'agentpipeline.pairing': 'kept' });
    expect(capturePairingFromLocation({ search: '?tab=board' })).toBe('kept');
    expect(getPairingToken()).toBe('kept');
  });

  it('未配对时拿到的就是 null——这是回环形态的常态，不是错误', () => {
    stubStorage();
    expect(getPairingToken()).toBeNull();
  });

  it('存储被禁也不炸：本次会话内存里仍然有效', () => {
    stubStorage({}, true);
    // 读不到 → null，但不抛
    expect(getPairingToken()).toBeNull();
    // 写不进去也不抛，且进程内缓存仍然生效（用户这次会话能正常用）
    setPairingToken('mem-only');
    expect(getPairingToken()).toBe('mem-only');
  });

  it('clearPairingToken 之后不再带令牌（一键重置的本地那一半）', () => {
    stubStorage();
    setPairingToken('old');
    expect(getPairingToken()).toBe('old');
    clearPairingToken();
    expect(getPairingToken()).toBeNull();
  });

  it('pairedUrl 的参数名与后端同一约定', () => {
    // 后端 `server_info.rs::pairing_url` 产出 `{base}/?pair={token}`；
    // 参数名漂了，二维码就递不过令牌——两处各写一份名字正是这条用例要挡的。
    expect(pairedUrl('http://192.168.1.10:8788', 'tok')).toBe(
      'http://192.168.1.10:8788/?pair=tok',
    );
    // 令牌里的保留字符必须编码（Crockford base32 用不到，但 URL 形状不该依赖这个巧合）
    expect(pairedUrl('http://h:1', 'a/b+c')).toBe('http://h:1/?pair=a%2Fb%2Bc');
  });

  it('PAIRING_HEADER 与后端读取的头名逐字一致', () => {
    // 后端 `stream.rs::PAIRING_TOKEN_HEADER` = "x-agentpipeline-token"（HTTP 头名大小写不敏感，
    // 这里比的是规范化之后的同一个名字）。
    expect(PAIRING_HEADER.toLowerCase()).toBe('x-agentpipeline-token');
  });
});
