import { describe, expect, it } from 'vitest';

import type { ServerInfo } from '../api/types';
import { changeLanMode, type LanToggleDeps } from './lanToggle';

function info(host: string): ServerInfo {
  return {
    host,
    port: 8788,
    loopback_only: host === '127.0.0.1',
    bind_source: 'settings',
    port_source: 'config',
    addresses: [],
  };
}

/** 记录调用顺序的假出口；`infos` 是逐次重读的返回（用尽后重复最后一项）。 */
function deps(
  behaviour: {
    setLan?: () => Promise<unknown>;
    clearLan?: () => Promise<unknown>;
    infos?: ServerInfo[];
    infoThrows?: number;
  } = {},
) {
  const calls: string[] = [];
  const infos = behaviour.infos ?? [info('127.0.0.1')];
  let reads = 0;
  const subject: LanToggleDeps = {
    async setLan(enabled) {
      calls.push(`set:${enabled}`);
      return behaviour.setLan ? behaviour.setLan() : undefined;
    },
    async clearLan() {
      calls.push('clear');
      return behaviour.clearLan ? behaviour.clearLan() : undefined;
    },
    async info() {
      calls.push('info');
      reads += 1;
      if (behaviour.infoThrows && reads <= behaviour.infoThrows) {
        throw new Error('Failed to fetch');
      }
      return infos[Math.min(infos.length - 1, reads - 1)];
    },
    async sleep(ms) {
      calls.push(`sleep:${ms}`);
    },
  };
  return { subject, calls };
}

describe('局域网开关的判定（决策 186）', () => {
  it('正常路径：请求成功且重读确认绑定已变 → 成功', async () => {
    const d = deps({ infos: [info('0.0.0.0')] });
    const result = await changeLanMode(true, d.subject);
    expect(result.ok).toBe(true);
    expect(d.calls).toEqual(['set:true', 'info']);
  });

  it('应答读不到但绑定确实变了 → 仍算成功，并如实说明这次连接被切断了', async () => {
    // 这是**常态**：触发改绑的请求就在被切断的那条连接上
    const d = deps({
      setLan: () => Promise.reject(new TypeError('Failed to fetch')),
      infos: [info('0.0.0.0')],
    });
    const result = await changeLanMode(true, d.subject);
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.note).toContain('切断');
      expect(result.info.host).toBe('0.0.0.0');
    }
  });

  it('改绑空窗期内重读失败 → 重试到读到为止', async () => {
    const d = deps({ infos: [info('0.0.0.0')], infoThrows: 2 });
    const result = await changeLanMode(true, d.subject);
    expect(result.ok).toBe(true);
    expect(d.calls.filter((c) => c === 'info')).toHaveLength(3);
  });

  it('服务端明确拒绝（绑不上）→ 失败并回显它的原因', async () => {
    const d = deps({
      setLan: () => Promise.reject(new Error('无法绑定 0.0.0.0:8788（端口被占用）；已恢复为 127.0.0.1')),
      infos: [info('127.0.0.1')],
    });
    const result = await changeLanMode(true, d.subject);
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.message).toContain('无法绑定');
    }
  });

  it('传输失败且状态没变 → 失败，但**不编造**原因（只说没读到目标状态）', async () => {
    const d = deps({
      setLan: () => Promise.reject(new TypeError('Failed to fetch')),
      infos: [info('127.0.0.1')],
    });
    const result = await changeLanMode(true, d.subject);
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.message).toContain('当前仍绑定 127.0.0.1');
      expect(result.message).toContain('--host');
    }
  });

  it('始终读不到服务状态 → 失败，且不把「读不到」说成「没生效」', async () => {
    const d = deps({ infos: [info('0.0.0.0')], infoThrows: 999 });
    const result = await changeLanMode(true, d.subject);
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.message).toContain('读不到服务状态');
    }
  });

  it('关掉走的是**清除**接口（回到启动参数 / 配置文件那一级），不是写死 127.0.0.1', async () => {
    const d = deps({ infos: [info('127.0.0.1')] });
    const result = await changeLanMode(false, d.subject);
    expect(d.calls[0]).toBe('clear');
    expect(result.ok).toBe(true);
    expect(d.calls).toEqual(['clear', 'info']);
  });

  it('关掉时启动参数仍要求对外绑定 → 如实报「关不掉」并说清是谁定的', async () => {
    // 界面清掉选择改不了这一次（启动参数最优先，决策 186）。说成成功会让用户以为
    // 手机已经进不来了，那是最坏的一种谎。
    const d = deps({
      infos: [{ ...info('0.0.0.0'), bind_source: 'startup' }],
    });
    const result = await changeLanMode(false, d.subject);
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.message).toContain('AGENTPIPELINE_LAN');
      expect(result.message).toContain('关不掉');
    }
  });

  it('开启成功但结果是启动参数定的 → 成功，并提醒重启后仍以启动参数为准', async () => {
    const d = deps({ infos: [{ ...info('0.0.0.0'), bind_source: 'startup' }] });
    const result = await changeLanMode(true, d.subject);
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.note).toContain('启动参数');
    }
  });

  it('开启时绑定被启动参数钉在回环 → 失败并指明该去掉哪个参数', async () => {
    const d = deps({ infos: [{ ...info('127.0.0.1'), bind_source: 'startup' }] });
    const result = await changeLanMode(true, d.subject);
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.message).toContain('--host');
    }
  });
});
