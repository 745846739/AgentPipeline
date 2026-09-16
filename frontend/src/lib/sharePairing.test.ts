import { describe, expect, it } from 'vitest';
import { sharePanel } from './sharePairing';

/**
 * 「手机访问」页的形态判定（决策 189）。
 *
 * 钉的是**一个用户抱怨**：手机扫码之后照样报「这台设备还没配对」，而报错页的指引又把人
 * 送回这一页——因为这一页在非回环来源下会画出一张**裸地址的码**，看起来与正常的那张毫无
 * 区别，扫了却永远配不上。故这里最要紧的一条是：**没有令牌就不画码**。
 */

const addr = [
  { url: 'http://192.168.1.10:8788', preferred: true },
  { url: 'http://10.0.0.5:8788', preferred: false },
];

describe('sharePanel（决策 189）', () => {
  it('有地址、有令牌 → 画带令牌的码，形状与后端 pairing_url 同约定', () => {
    const panel = sharePanel({
      info: { loopback_only: false },
      addresses: addr,
      selected: null,
      token: 'tok',
    });
    expect(panel).toEqual({ kind: 'paired-qr', target: 'http://192.168.1.10:8788/?pair=tok' });
  });

  it('选中项优先于后端推荐的首项', () => {
    const panel = sharePanel({
      info: { loopback_only: false },
      addresses: addr,
      selected: 'http://10.0.0.5:8788',
      token: 'tok',
    });
    expect(panel).toEqual({ kind: 'paired-qr', target: 'http://10.0.0.5:8788/?pair=tok' });
  });

  it('令牌里的保留字符被编码（URL 形状不依赖令牌字符集的巧合）', () => {
    const panel = sharePanel({
      info: { loopback_only: false },
      addresses: addr,
      selected: null,
      token: 'a/b+c',
    });
    expect(panel).toEqual({ kind: 'paired-qr', target: 'http://192.168.1.10:8788/?pair=a%2Fb%2Bc' });
  });

  it('有地址、没令牌 → 指引块，**不画码**（这是本条决策的裁决）', () => {
    const panel = sharePanel({
      info: { loopback_only: false },
      addresses: addr,
      selected: null,
      token: null,
    });
    expect(panel).toEqual({ kind: 'local-only-gate' });
  });

  it('令牌是空串也当没取到——不画一张配不上的码', () => {
    const panel = sharePanel({
      info: { loopback_only: false },
      addresses: addr,
      selected: null,
      token: '',
    });
    expect(panel).toEqual({ kind: 'local-only-gate' });
  });

  it('只绑回环 → 改绑指引优先于一切（手机连不上，画了也扫不开）', () => {
    const panel = sharePanel({
      info: { loopback_only: true },
      addresses: [],
      selected: null,
      token: 'tok',
    });
    expect(panel).toEqual({ kind: 'loopback-gate' });
  });

  it('枚举不出地址 → 无地址指引；选中项还留着也无处可指', () => {
    expect(
      sharePanel({
        info: { loopback_only: false },
        addresses: [],
        selected: 'http://192.168.1.10:8788',
        token: 'tok',
      }),
    ).toEqual({ kind: 'no-address-gate' });
  });

  it('服务读数还没到（info 为 null）不当作回环形态', () => {
    const panel = sharePanel({ info: null, addresses: addr, selected: null, token: 'tok' });
    expect(panel.kind).toBe('paired-qr');
  });
});
