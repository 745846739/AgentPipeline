/**
 * API base —— **唯一**配置点（决策 153④）。
 *
 * 默认同源相对路径（空串 = 当前 origin）。桌面壳（Tauri）在启动时注入
 * `http://127.0.0.1:{port}`，注入途径有两个（按优先级）：
 *   1. `window.__AGENTPIPELINE_API_BASE__`
 *   2. `import.meta.env.VITE_API_BASE`
 *
 * 业务代码一律用 `apiUrl(path)` 拼地址，不得散落硬编码。
 */

declare global {
  interface Window {
    __AGENTPIPELINE_API_BASE__?: string;
  }
}

let base = '';

/** 注入覆盖口（Tauri 壳 / 测试）。传空串回到同源相对路径。 */
export function setApiBase(value: string | null | undefined): void {
  base = (value ?? '').replace(/\/+$/, '');
}

export function getApiBase(): string {
  return base;
}

/** 相对路径 → 完整 URL。path 需以 `/` 开头。 */
export function apiUrl(path: string): string {
  return `${base}${path}`;
}

/** 跨源写请求防护旁路头（决策 128 / 153③）。 */
export const CLIENT_HEADER = 'X-AgentPipeline';

/** 配对令牌头（决策 182㉙，票 07）：非回环形态下写请求与对讲台接口凭它通行。 */
export const PAIRING_HEADER = 'X-AgentPipeline-Token';

/** 配对链接的查询参数名。与 `server_info.rs::pairing_url` 是同一个约定。 */
export const PAIRING_QUERY = 'pair';

/** localStorage 键。用 localStorage 而不是 sessionStorage：配对一次要跨标签页与会话有效。 */
const PAIRING_STORAGE_KEY = 'agentpipeline.pairing';

/** 进程内缓存，省掉每次请求读一次 localStorage（同步 API，但仍是磁盘/内存查找）。 */
let pairingToken: string | null = null;

/**
 * 解析地址栏里的配对参数、存本地、并把它从地址栏抹掉（决策 182㉙）。
 *
 * 三条都是刻意的：
 * - **存本地**：手机上重扫一次二维码是每次使用的额外步骤，而人一旦嫌麻烦就会去把令牌关掉，
 *   那时安全设计等于不存在（用户故事 14）。
 * - **从地址栏抹掉**：留在地址栏里会被截图、被分享、被浏览器历史记下来，也会随
 *   `Referer` 泄给第三方。配对是一次性动作，凭据不该长期挂在最显眼的地方。
 * - **只读一次**：抹掉之后再调用本函数自然拿不到值，不会把旧参数反复写回。
 */
export function capturePairingFromLocation(
  location: Pick<Location, 'search' | 'pathname' | 'hash'> = window.location,
  replace: (url: string) => void = (url) => window.history.replaceState(null, '', url),
): string | null {
  const params = new URLSearchParams(location.search);
  const token = params.get(PAIRING_QUERY);
  if (!token) return getPairingToken();

  setPairingToken(token);
  params.delete(PAIRING_QUERY);
  const rest = params.toString();
  replace(`${location.pathname}${rest ? `?${rest}` : ''}${location.hash}`);
  return token;
}

/** 当前配对令牌（未配对时为 `null`，回环形态下这是常态且合法）。 */
export function getPairingToken(): string | null {
  if (pairingToken !== null) return pairingToken;
  try {
    pairingToken = window.localStorage.getItem(PAIRING_STORAGE_KEY);
  } catch {
    // 隐私模式 / 存储被禁：退回「本次会话内存里有效」，不因为读不到就炸掉整个应用。
    pairingToken = null;
  }
  return pairingToken;
}

export function setPairingToken(token: string | null): void {
  pairingToken = token && token.trim() ? token.trim() : null;
  try {
    if (pairingToken) window.localStorage.setItem(PAIRING_STORAGE_KEY, pairingToken);
    else window.localStorage.removeItem(PAIRING_STORAGE_KEY);
  } catch {
    // 同上：写不进去不影响本次会话已经生效的令牌。
  }
}

/** 一键重置后由调用方清掉本地那份（服务端已换新令牌，旧的立即失效）。 */
export function clearPairingToken(): void {
  setPairingToken(null);
}

/** 启动时解析注入值。 */
export function initApiBase(
  injected: string | null | undefined = typeof window !== 'undefined'
    ? window.__AGENTPIPELINE_API_BASE__
    : undefined,
): string {
  const env = import.meta.env?.VITE_API_BASE as string | undefined;
  setApiBase(injected ?? env ?? '');
  return getApiBase();
}
