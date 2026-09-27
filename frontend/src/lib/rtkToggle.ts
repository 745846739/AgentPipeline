import type { RtkProbe, RtkSettings } from '../api/types';

/**
 * 「命令执行」页的三态与两句话（决策 297 / 票 05）。
 *
 * 判据住在这里（纯逻辑）而不是组件里，是为了能直接钉住三态的分界与措辞：**「启用」
 * 与「真能用」是两件事**，而这一页存在的全部意义就是不让它们混起来——设置页说可用、
 * 命令全 127 是最坏的形状。
 *
 * 三态：
 * - `off`：开关关着（缺省或是人关的）——命令按原样跑。
 * - `ready`：开着且三条判据全过——显示解析到的路径与版本。
 * - `unavailable`：开着但本机用不了——显示**可归因**的原因，并把「手填路径」摆出来。
 */

export type RtkState = 'off' | 'ready' | 'unavailable';

export function rtkState(settings: Pick<RtkSettings, 'enabled' | 'probe'>): RtkState {
  if (!settings.enabled) return 'off';
  return settings.probe.available ? 'ready' : 'unavailable';
}

/** 三态各自的标签（与页面上的状态灯用同一份词）。 */
export function stateLabel(state: RtkState): string {
  switch (state) {
    case 'off':
      return '[OFF]';
    case 'ready':
      return '[ON]';
    case 'unavailable':
      return '[ON · 用不了]';
  }
}

/**
 * 探测读数的一句话。
 *
 * 每一格都可能是空的（探测在半路就失败了），故**逐格判在场**再拼——不编造读数。
 * 解析来源翻成人话：这一条决定用户该去改哪儿（手填的填错了 / 系统 PATH 里那份该升级）。
 */
export function probeLine(probe: RtkProbe): string {
  if (!probe.available) {
    return probe.reason ?? '不可用（原因没读回来，重进这一页再看一次）。';
  }
  const where = probe.path ?? '（路径没读回来）';
  const version = probe.version ? ` · ${probe.version}` : '';
  return `${where}${version}（${sourceLabel(probe.source)}）`;
}

/** 解析来源的人话。 */
export function sourceLabel(source: RtkProbe['source']): string {
  switch (source) {
    case 'manual':
      return '这一页手填的';
    case 'path':
      return '服务进程的 PATH 里找到的';
    case 'known-dir':
      return '几个常用安装目录里找到的';
    default:
      return '来源没读回来';
  }
}

/**
 * 保存之后要说的话（票 05：**不静默成功、也不静默失败**）。
 *
 * 探测失败时保存是成功的，但那句话必须同时说出「已启用」与「本服务当前找不到它」——
 * 只说前一半是骗人，只说后一半会让人以为没存上、于是再按一次。
 */
export function saveNote(
  enabled: boolean,
  probe: RtkProbe,
): { kind: 'ok' | 'bad'; message: string } {
  if (!enabled) {
    return { kind: 'ok', message: '已关掉：命令按原样跑，不再交给 rtk。' };
  }
  if (probe.available) {
    return {
      kind: 'ok',
      message: `已打开，本机可用：${probeLine(probe)}`,
    };
  }
  return {
    kind: 'bad',
    message: `已启用，但本服务当前找不到能用的 rtk：${
      probe.reason ?? '原因没读回来'
    }。装好它，或在下面手填绝对路径再保存一次。`,
  };
}

/**
 * 手填路径的归一：空白 = 没说（回到自动解析），其余原样（前後空白去掉）。
 *
 * 「空串」与「填了一个空路径」是两件事——后端也按同一条归一（空串进不了库）。
 */
export function normalizeManualPath(raw: string): string | null {
  const trimmed = raw.trim();
  return trimmed === '' ? null : trimmed;
}

/** 手填框该不该出现：开着**且**（自动解析用不了 **或** 已经存过一条手填路径——要能改能清）。 */
export function manualPathVisible(settings: Pick<RtkSettings, 'enabled' | 'path' | 'probe'>): boolean {
  if (!settings.enabled) return false;
  return !settings.probe.available || settings.path !== null;
}
