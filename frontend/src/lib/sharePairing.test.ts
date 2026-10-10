import { describe, expect, it } from 'vitest';
import { ApiError } from '../api/client';
import {
  bindSourceLabel,
  isPairingRequired,
  phoneCanReach,
  portFallbackNote,
  qrCaption,
  sharePanel,
} from './sharePairing';

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

// 服务读数的三种形态（决策 334 起 `public_base_url` 是必填的一栏）。

/** 绑全网卡、没有公网入口：手机直连（决策 167 的经典形态）。 */
const LAN = { loopback_only: false, public_base_url: null };
/** 只绑回环、没有公网入口：手机真的够不着（决策 186 的改绑指引）。 */
const LOOPBACK = { loopback_only: true, public_base_url: null };
/** 只绑回环**但有公网入口**：106 的形态——后端在回环上听，门外是 Caddy（决策 334）。 */
const PUBLIC_ENTRY = { loopback_only: true, public_base_url: 'https://203.0.113.10:3389' };

describe('sharePanel（决策 189）', () => {
  it('有地址、有令牌 → 画带令牌的码，形状与后端 pairing_url 同约定', () => {
    const panel = sharePanel({
      info: LAN,
      addresses: addr,
      selected: null,
      token: 'tok',
    });
    expect(panel).toEqual({ kind: 'paired-qr', target: 'http://192.168.1.10:8788/?pair=tok' });
  });

  it('选中项优先于后端推荐的首项', () => {
    const panel = sharePanel({
      info: LAN,
      addresses: addr,
      selected: 'http://10.0.0.5:8788',
      token: 'tok',
    });
    expect(panel).toEqual({ kind: 'paired-qr', target: 'http://10.0.0.5:8788/?pair=tok' });
  });

  it('令牌里的保留字符被编码（URL 形状不依赖令牌字符集的巧合）', () => {
    const panel = sharePanel({
      info: LAN,
      addresses: addr,
      selected: null,
      token: 'a/b+c',
    });
    expect(panel).toEqual({ kind: 'paired-qr', target: 'http://192.168.1.10:8788/?pair=a%2Fb%2Bc' });
  });

  it('有地址、没令牌 → 指引块，**不画码**（这是本条决策的裁决）', () => {
    const panel = sharePanel({
      info: LAN,
      addresses: addr,
      selected: null,
      token: null,
    });
    expect(panel).toEqual({ kind: 'local-only-gate' });
  });

  it('令牌是空串也当没取到——不画一张配不上的码', () => {
    const panel = sharePanel({
      info: LAN,
      addresses: addr,
      selected: null,
      token: '',
    });
    expect(panel).toEqual({ kind: 'local-only-gate' });
  });

  it('只绑回环 → 改绑指引优先于一切（手机连不上，画了也扫不开）', () => {
    const panel = sharePanel({
      info: LOOPBACK,
      addresses: [],
      selected: null,
      token: 'tok',
    });
    expect(panel).toEqual({ kind: 'loopback-gate' });
  });

  it('枚举不出地址 → 无地址指引；选中项还留着也无处可指', () => {
    expect(
      sharePanel({
        info: LAN,
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

/**
 * 公网入口（决策 334）：手机上那个「扫码进去还是连不上」的抱怨，根因是这一页把
 * **绑定形态**当成了**可达性**——106 上后端只绑回环（门外是 Caddy），于是这一页永远说
 * 「手机现在连不上这台机器」，还递上一颗按下去会把刚关掉的明文入口装回来的钮。
 *
 * 判据因此换成 `phoneCanReach`：有一个手机走得到的入口就够了，绑在哪是另一回事。
 */
describe('phoneCanReach（决策 334：绑回环 ≠ 手机够不着）', () => {
  it('绑全网卡 → 够得着（三种形态里唯一不发公网入口的也行）', () => {
    expect(phoneCanReach(LAN)).toBe(true);
  });

  it('只绑回环、没配公网入口 → 够不着（决策 186 的改绑指引仍然成立）', () => {
    expect(phoneCanReach(LOOPBACK)).toBe(false);
  });

  it('只绑回环但配了公网入口 → 够得着（106 的形态：门在外面）', () => {
    expect(phoneCanReach(PUBLIC_ENTRY)).toBe(true);
  });

  it('读数还没到 → 不下结论（与「null 不当作回环」同一姿态）', () => {
    expect(phoneCanReach(null)).toBe(true);
  });

  it('配了公网入口就走二维码那条路，而不是改绑指引', () => {
    const panel = sharePanel({
      info: PUBLIC_ENTRY,
      addresses: [{ url: 'https://203.0.113.10:3389' }],
      selected: null,
      token: 'tok',
    });
    expect(panel).toEqual({
      kind: 'paired-qr',
      target: 'https://203.0.113.10:3389/?pair=tok',
    });
  });
});

describe('qrCaption（决策 334：这句话说错了，人就往错的方向查）', () => {
  it('有公网入口 → 说清走的是那个入口、后端只绑回环', () => {
    const text = qrCaption(PUBLIC_ENTRY);
    expect(text).toContain('https://203.0.113.10:3389');
    expect(text).toContain('反向代理');
    expect(text).not.toContain('同一 Wi-Fi');
  });

  it('没有公网入口 → 照旧是「同一局域网」（决策 167 的形态一字不动）', () => {
    expect(qrCaption(LAN)).toContain('同一 Wi-Fi');
    expect(qrCaption(null)).toContain('同一 Wi-Fi');
  });
});

describe('portFallbackNote（决策 213：端口退让要说出来）', () => {
  it('退让过 → 说清原因、当前端口、下一步', () => {
    const note = portFallbackNote({ port_source: 'fallback', port: 53311, loopback_only: false });
    expect(note).toContain('被别的程序占着');
    expect(note).toContain('53311');
    expect(note).toContain('重新扫');
  });

  it('端口来自配置（常态）→ 不说话', () => {
    expect(
      portFallbackNote({ port_source: 'config', port: 8788, loopback_only: false }),
    ).toBeNull();
  });

  it('启动参数指定的端口 → 不说话（它不是「这次才变」的那一档）', () => {
    expect(
      portFallbackNote({ port_source: 'startup', port: 9000, loopback_only: false }),
    ).toBeNull();
  });

  it('只绑回环 → 不说话：手机本来就连不上，端口是多少与下一步动作无关', () => {
    expect(
      portFallbackNote({ port_source: 'fallback', port: 53311, loopback_only: true }),
    ).toBeNull();
  });

  it('读数还没到（info 为 null）→ 不说话', () => {
    expect(portFallbackNote(null)).toBeNull();
  });
});

describe('绑定来源的叫法只有一处定义（票 15 / R2-18）', () => {
  it('三个取值各有名有姓，同一个值任何时候都是同一个词', () => {
    expect(bindSourceLabel('startup')).toBe('启动参数');
    expect(bindSourceLabel('settings')).toBe('界面上的选择');
    expect(bindSourceLabel('config')).toBe('配置文件');
    // 认不出来的来源回落成配置文件那一档（后端只会给这三种，兜底别抛）
    expect(bindSourceLabel(null)).toBe('配置文件');
    expect(bindSourceLabel('???')).toBe('配置文件');
  });

  it('页面里不再各写一份三元表达式（同一个值两个说法就是这么来的）', async () => {
    const { readFileSync } = await import('node:fs');
    const { join, resolve } = await import('node:path');
    // vitest 从 `frontend/` 运行（vite.config.ts 的 include 是 src/**），故以 cwd 定位
    const share = readFileSync(join(resolve(process.cwd(), 'src'), 'routes', 'Share.svelte'), 'utf8');
    expect(share).not.toContain('界面设置');
    expect(share.match(/bind_source === 'settings'/g) ?? []).toHaveLength(0);
  });
});

describe('isPairingRequired（票 04 / 决策 259：按 kind 判，不按报文字样）', () => {
  it('配对缺失的 403（kind = pairing_required）→ true', () => {
    expect(isPairingRequired(new ApiError(403, '这台设备还没配对：请在跑服务的电脑本机扫码', 'pairing_required'))).toBe(
      true,
    );
  });

  it('跨源 403（403 但没有 kind）→ false——这正是不能只看状态码的那条教训', () => {
    expect(isPairingRequired(new ApiError(403, '跨源写请求被拒绝：Origin/Referer = x'))).toBe(false);
  });

  it('报文写着「还没配对」但 kind 不是 → false（字样说了不算）', () => {
    expect(isPairingRequired(new ApiError(403, '这台设备还没配对', 'origin_rejected'))).toBe(false);
  });

  it('非 ApiError 与网络层错误（status 0）→ false', () => {
    expect(isPairingRequired(new Error('网络请求失败：连接被拒'))).toBe(false);
    expect(isPairingRequired(new ApiError(0, '请求超时（30 秒没有回应）。'))).toBe(false);
    expect(isPairingRequired(null)).toBe(false);
  });
});
