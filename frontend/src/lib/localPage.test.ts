import { describe, expect, it } from 'vitest';
import { isHostMachine, isLoopbackHostname } from './localPage';

/**
 * 「这一页是不是在跑服务的这台机器本机上打开的」（决策 190）。
 *
 * 顶栏据此决定给不给「手机访问」入口：那一页从手机上打开无事可做（配对令牌只允许回环来源
 * 读，决策 182㉗），故判据必须与「令牌读不读得到」同源，而不是看视口宽度——一个被拖窄的
 * 桌面窗口仍是本机，那里的「手机访问」完全可用。
 */

describe('isLoopbackHostname（决策 190）', () => {
  it('回环的几种写法都算', () => {
    for (const host of ['127.0.0.1', '127.0.0.2', '127.255.0.9', 'localhost', '::1', '[::1]']) {
      expect(isLoopbackHostname(host), host).toBe(true);
    }
  });

  it('大小写与空白不影响判定', () => {
    expect(isLoopbackHostname(' LocalHost ')).toBe(true);
    expect(isLoopbackHostname('[::1]')).toBe(true);
  });

  it('局域网地址与域名不算', () => {
    for (const host of ['192.168.3.12', '10.0.0.5', '172.16.1.1', '0.0.0.0', 'example.com']) {
      expect(isLoopbackHostname(host), host).toBe(false);
    }
  });

  it('前缀伪装不算回环——要求四段点分数字', () => {
    // 这条谓词的输入是页面自己的主机名（可被 DNS 影响），故 `127.` 前缀不够
    expect(isLoopbackHostname('127.evil.com')).toBe(false);
    expect(isLoopbackHostname('127.0.0.1.evil.com')).toBe(false);
    expect(isLoopbackHostname('localhost.evil.com')).toBe(false);
  });
});

describe('isHostMachine（决策 190）', () => {
  it('同源形态看当前地址：127.0.0.1 是本机，局域网 IP 不是', () => {
    expect(isHostMachine('', 'http://127.0.0.1:8788/#/share')).toBe(true);
    expect(isHostMachine('', 'http://localhost:8788/')).toBe(true);
    expect(isHostMachine('', 'http://192.168.3.12:8788/#/share')).toBe(false);
  });

  it('注入了 API base 时以 base 为准（桌面壳是 `http://127.0.0.1:{port}`）', () => {
    // 桌面壳里页面自身的地址可能是壳的伪协议，真正的服务仍是本机
    expect(isHostMachine('http://127.0.0.1:8788', 'tauri://localhost/')).toBe(true);
    expect(isHostMachine('http://192.168.3.12:8788', 'http://192.168.3.12:8788/')).toBe(false);
  });

  it('判不出主机名时不藏（藏错一个有用的入口更坏）', () => {
    expect(isHostMachine('', 'not a url')).toBe(true);
  });
});
