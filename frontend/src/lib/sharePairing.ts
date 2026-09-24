import { ApiError, pairedUrl } from '../api/client';
import type { ServerInfo } from '../api/types';

/**
 * 「手机访问」页画什么（决策 189）。
 *
 * 四种形态的判定收在这里，模板只按 `kind` 分派——其中**最要紧的一条**是
 * 「没有配对令牌就不画二维码」。
 *
 * 为什么这一条值得单独存在：配对令牌只允许本机（回环来源）读取（决策 182㉙ / 票 07），
 * 所以从手机打开这一页、或在电脑上用局域网地址打开这一页，**必然**取不到令牌。原先是
 * 「降级为裸地址、照画一张码」，而那张码与正常的那张**在视觉上毫无区别**：扫了它，看板
 * 照常打开（只读 GET 不护），一动写操作或进对讲台就撞 403「这台设备还没配对」，而报错页
 * 的指引又把人送回这一页——**一张扫不出结果的码比没有码更坏**，它把「没配对」伪装成
 * 「扫过了」，使用者会把力气花在重复扫码上而不是去找那台电脑。
 */

/** 本模块用到的服务读数（生产是 `GET /server-info` 的应答）。 */
export type ShareInfo = Pick<ServerInfo, 'loopback_only'>;

/** 页面该呈现的形态。 */
export type SharePanel =
  /** 服务只绑回环：手机根本连不上，先给运行期改绑的入口（决策 186）。 */
  | { kind: 'loopback-gate' }
  /** 已绑全网卡但枚举不出可用地址：给可手动输入的地址形状。 */
  | { kind: 'no-address-gate' }
  /** **有地址却没有令牌**：这一页不是从本机打开的，给「去那台电脑上打开」的指引而不是码。 */
  | { kind: 'local-only-gate' }
  /** 令牌到手：画带令牌的码，扫一次即配对。 */
  | { kind: 'paired-qr'; target: string };

/**
 * 判定顺序：绑定形态 → 有没有地址 → 有没有令牌。
 *
 * 前两项各自对应「这一页的用途本身不成立」，令牌那条是决策 189 的裁决。地址表为空时
 * 直接落到无地址指引：此时选中项即便还留着也无处可指（`selected` 只能来自地址表）。
 */
export function sharePanel(input: {
  info: ShareInfo | null;
  addresses: readonly { url: string }[];
  /** 当前选中用于生成二维码的地址（缺省取后端推荐的首项）。 */
  selected: string | null;
  /** 配对令牌；`null` = 没取到（未配对 / 这页不是从本机打开的）。 */
  token: string | null;
}): SharePanel {
  if (input.info?.loopback_only) return { kind: 'loopback-gate' };
  if (input.addresses.length === 0) return { kind: 'no-address-gate' };
  if (!input.token) return { kind: 'local-only-gate' };
  const base = input.selected ?? input.addresses[0]?.url ?? '';
  return { kind: 'paired-qr', target: pairedUrl(base, input.token) };
}

/**
 * 「这次绑定是谁定的」那句话（决策 186 / 票 15）。
 *
 * **同一个值只许有一个说法**：此前 `Share.svelte` 里两处各写了一份三元表达式，而
 * `bind_source === 'settings'` 一处叫「界面设置」、另一处叫「界面上的选择」——同一个东西
 * 在同一个页面上有两个名字。定义收在这里，模板只调它。
 */
export function bindSourceLabel(source: string | null | undefined): string {
  if (source === 'startup') return '启动参数';
  if (source === 'settings') return '界面上的选择';
  return '配置文件';
}

/**
 * 退让端口要说的话（决策 213），不说则 `null`。
 *
 * 后端只在**首选端口被别的进程占着**时退让到内核随机端口（`/server-info.port_source`
 * = `fallback`）——那正是「手机上存过的地址这次打不开」的原因，而它在界面上原本没有任何
 * 痕迹（日志在桌面应用里看不到）。故这里给一句能对上号的话：**说清发生了什么 + 下一步做什么**。
 *
 * 只绑回环时不说话：那时手机本来就连不上，端口是多少与使用者的下一步动作（先按那颗钮）
 * 无关，多说一句只是噪音。
 */
export function portFallbackNote(
  info: Pick<ServerInfo, 'port_source' | 'port' | 'loopback_only'> | null,
): string | null {
  if (!info || info.loopback_only || info.port_source !== 'fallback') return null;
  return `这次没能绑上固定的端口（被别的程序占着），当前用的是临时端口 ${info.port}——手机上存过的网址这次要重新扫一次。`;
}

/**
 * 这次失败是「设备还没配对」吗——**按后端给的 `kind` 判，不按报文字样**（票 04 / 决策 259）。
 *
 * 403 在本应用里被**两处**用着：配对缺失（`pairing_guard`，带
 * `kind: "pairing_required"`）与跨源防护（决策 128，**不带 kind**）。此前界面只能
 * `message.includes('还没配对')` 才分得开——报文即接口，改一句话就断。与技能市场八类
 * （决策 194⑦）同姿态：`api/client.ts` 已把 `kind` 从错误体里解出来挂在 `ApiError` 上，
 * 这里只是把它变成一个可以逐条测的判定。
 *
 * **判在 ApiError 还在手上的那一层**（两处 catch），不等错误被降级成流里的字符串——
 * 那时 `kind` 已经丢了，任何补救都要重新摸报文。
 */
export function isPairingRequired(err: unknown): boolean {
  return err instanceof ApiError && err.kind === 'pairing_required';
}
