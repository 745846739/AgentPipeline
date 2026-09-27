import { describe, expect, it } from 'vitest';
import type { RtkProbe, RtkSettings } from '../api/types';
import {
  manualPathVisible,
  normalizeManualPath,
  probeLine,
  rtkState,
  saveNote,
  sourceLabel,
  stateLabel,
} from './rtkToggle';

/**
 * 「命令执行」页的判据（决策 297 / 票 05）。
 *
 * 这一页存在的意义是**不让「启用」与「真能用」混起来**，故三态的分界与那两句话的措辞
 * 就是它要钉的东西。
 */

const OK_PROBE: RtkProbe = {
  available: true,
  path: '/usr/local/bin/rtk',
  source: 'path',
  version: 'rtk 0.42.4',
  reason: null,
};

const BAD_PROBE: RtkProbe = {
  available: false,
  path: null,
  source: null,
  version: null,
  reason: '没找到 rtk：服务进程的 PATH 与五个已知目录里都没有。',
};

function settings(partial: Partial<RtkSettings>): RtkSettings {
  return {
    enabled: false,
    origin: 'default',
    path: null,
    probe: BAD_PROBE,
    ...partial,
  };
}

describe('三态', () => {
  it('关着是 off——哪怕探测结果本身是好的', () => {
    expect(rtkState(settings({ enabled: false, probe: OK_PROBE }))).toBe('off');
    expect(stateLabel('off')).toBe('[OFF]');
  });

  it('开着且可用是 ready（显示路径与版本）', () => {
    expect(rtkState(settings({ enabled: true, probe: OK_PROBE }))).toBe('ready');
    expect(stateLabel('ready')).toBe('[ON]');
    expect(probeLine(OK_PROBE)).toBe(
      '/usr/local/bin/rtk · rtk 0.42.4（服务进程的 PATH 里找到的）',
    );
  });

  it('开着但用不了是 unavailable（原因可归因）', () => {
    expect(rtkState(settings({ enabled: true, probe: BAD_PROBE }))).toBe('unavailable');
    expect(stateLabel('unavailable')).toBe('[ON · 用不了]');
    expect(probeLine(BAD_PROBE)).toBe(BAD_PROBE.reason);
  });

  it('可用但读数缺格时逐格判在场，不编造', () => {
    expect(
      probeLine({ available: true, path: null, source: null, version: null, reason: null }),
    ).toBe('（路径没读回来）（来源没读回来）');
    expect(sourceLabel(null)).toBe('来源没读回来');
    expect(sourceLabel('manual')).toBe('这一页手填的');
    expect(sourceLabel('known-dir')).toBe('几个常用安装目录里找到的');
  });

  it('不可用但连原因都没读回来时，也说得出「没读回来」', () => {
    expect(
      probeLine({ available: false, path: null, source: null, version: null, reason: null }),
    ).toContain('没读回来');
  });
});

describe('保存之后说的话（不静默成功、也不静默失败）', () => {
  it('打开且可用：说「已打开」并给读数', () => {
    const note = saveNote(true, OK_PROBE);
    expect(note.kind).toBe('ok');
    expect(note.message).toContain('本机可用');
    expect(note.message).toContain('/usr/local/bin/rtk');
  });

  it('打开但用不了：**两件事一起说**——已启用 + 当前找不到能用的', () => {
    const note = saveNote(true, BAD_PROBE);
    expect(note.kind).toBe('bad');
    expect(note.message).toContain('已启用');
    expect(note.message).toContain('找不到能用的 rtk');
    // 只说半句会让人以为没存上、于是再按一次
    expect(note.message).toContain('手填绝对路径');
  });

  it('关掉：说命令按原样跑', () => {
    const note = saveNote(false, OK_PROBE);
    expect(note.kind).toBe('ok');
    expect(note.message).toContain('按原样跑');
  });
});

describe('手填路径', () => {
  it('空白 = 没说（回到自动解析），不是「填了一个空路径」', () => {
    expect(normalizeManualPath('')).toBeNull();
    expect(normalizeManualPath('   ')).toBeNull();
    expect(normalizeManualPath('  /usr/local/bin/rtk ')).toBe('/usr/local/bin/rtk');
  });

  it('框只在开着、且（用不了 或 已存过一条）时出现', () => {
    expect(manualPathVisible(settings({ enabled: false, probe: BAD_PROBE }))).toBe(false);
    expect(manualPathVisible(settings({ enabled: true, probe: BAD_PROBE }))).toBe(true);
    // 可用但手填过：仍要能看见、能改、能清
    expect(manualPathVisible(settings({ enabled: true, probe: OK_PROBE, path: '/opt/rtk' }))).toBe(
      true,
    );
    // 可用且从没填过：不摆一个用不上的框
    expect(manualPathVisible(settings({ enabled: true, probe: OK_PROBE }))).toBe(false);
  });
});
