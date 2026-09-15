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
 * 配对令牌的取用与消费（决策 182㉙，票 07）。
 *
 * 这三件事各自对应一条用户故事，取舍都不是显然的：
 * ① 二维码递过来的 `?pair=` 必须**存下来**（用户故事 14：每次重扫会让人干脆把令牌关掉）；
 * ② 必须**从地址栏抹掉**（否则凭据留在截图、历史与 `Referer` 里）；
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

describe('配对令牌（决策 182㉙，票 07）', () => {
  it('从地址栏收下令牌、存本地、并把它抹出地址栏', () => {
    stubStorage();
    const replaced: string[] = [];
    const token = capturePairingFromLocation(
      { search: `?${PAIRING_QUERY}=abc123&tab=board`, pathname: '/', hash: '#/talk' },
      (url) => replaced.push(url),
    );

    expect(token).toBe('abc123');
    expect(getPairingToken()).toBe('abc123');
    // 抹掉的是**令牌那一个参数**，其余查询参数与 hash 原样留着
    expect(replaced).toEqual(['/?tab=board#/talk']);
  });

  it('令牌是唯一参数时地址栏回到干净的 path', () => {
    stubStorage();
    const replaced: string[] = [];
    capturePairingFromLocation(
      { search: `?${PAIRING_QUERY}=abc123`, pathname: '/', hash: '' },
      (url) => replaced.push(url),
    );
    expect(replaced).toEqual(['/']);
  });

  it('地址栏里没有配对参数时不动地址栏，只回已存的那份', () => {
    stubStorage({ 'agentpipeline.pairing': 'kept' });
    const replaced: string[] = [];
    const token = capturePairingFromLocation(
      { search: '?tab=board', pathname: '/', hash: '' },
      (url) => replaced.push(url),
    );
    expect(token).toBe('kept');
    expect(replaced).toEqual([]);
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
