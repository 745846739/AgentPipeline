import { pairedUrl } from '../api/client';
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
