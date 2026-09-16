import { getApiBase } from '../api/config';

/**
 * 「这一页是不是在跑服务的这台机器本机打开的」（决策 190）。
 *
 * 唯一的使用者是顶栏：**不是本机打开的页面不给「手机访问」入口**。理由是那一页从手机上看
 * 无事可做——它要读的配对令牌只允许回环来源读（决策 182㉗），所以从手机上打开它只能看到
 * 一块「去电脑上打开」的指引（决策 189）。把一个自己知道帮不上忙的入口摆在那，就是把使用
 * 者送去一次白跑。
 *
 * **判据是来源地址，不是视口宽度**（这是刻意的）：手机、平板、以及用局域网地址打开的电脑，
 * 三者在守卫眼里是同一件事（非回环来源）；而一个被拖窄到 380px 的桌面窗口**仍然是本机**，
 * 「手机访问」在那个窗口里完全可用，按宽度藏会把一个有用的入口藏掉。故判据与实际能不能
 * 拿到令牌的那条规则同源。
 *
 * 判定用页面的主机名，不发任何请求：`base` 非空时（桌面壳注入的 `http://127.0.0.1:{port}`）
 * 看 base，否则看当前地址（同源形态）。
 */

/**
 * 回环主机名。
 *
 * **要求四段点分数字**，故 `127.evil.com` 这类前缀伪装不算回环——这条谓词的输入在本模块里
 * 是页面自己的主机名（可被 DNS 影响），比 `host.startsWith('127.')` 宽一档的写法不够用。
 * `lib/lanToggle.ts` 复用同一个口径，避免「什么算回环」出现两个答案。
 */
export function isLoopbackHostname(hostname: string): boolean {
  // URL 里的 IPv6 主机名带方括号（`http://[::1]:8788` 的 hostname 是 `[::1]`）
  const host = hostname.trim().toLowerCase().replace(/^\[|\]$/g, '');
  if (host === 'localhost' || host === '::1') return true;
  return /^127(\.\d{1,3}){3}$/.test(host);
}

/**
 * 纯判定：`base` = API base（空串 = 同源相对路径），`href` = 当前地址。
 *
 * 判不出主机名时**返回 true**（不藏）：藏错一个有用的入口，比多显示一个入口更坏。
 */
export function isHostMachine(base: string, href: string): boolean {
  try {
    return isLoopbackHostname(new URL(base || href).hostname);
  } catch {
    return true;
  }
}

/** 生产入口：读真实的 base 与地址。 */
export function onHostMachine(): boolean {
  if (typeof window === 'undefined') return true;
  return isHostMachine(getApiBase(), window.location.href);
}
