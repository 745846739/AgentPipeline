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

/** localStorage 键。用 localStorage 而不是 sessionStorage：配对一次要跨标签页与会话有效。
 * （主屏图标那一份不靠它——iOS 的主屏 web app 与 Safari 存储隔离，那条路靠地址栏里的
 * `?pair=`，见 `capturePairingFromLocation` 的注释。） */
const PAIRING_STORAGE_KEY = 'agentpipeline.pairing';

/** 进程内缓存，省掉每次请求读一次 localStorage（同步 API，但仍是磁盘/内存查找）。 */
let pairingToken: string | null = null;

/**
 * 解析地址栏里的配对参数并存进本地（决策 182㉙，**由决策 191 修订**）。
 *
 * **改了什么**：原实现还会把 `?pair=` 从地址栏**抹掉**（`history.replaceState`），理由是
 * 「留在地址栏里会被截图、被分享、被浏览器历史记下来，也会随 `Referer` 泄给第三方」。
 * 那条理由被一个更硬的现实压过：**手机「添加到主屏幕」保存的就是当时地址栏里的那条 URL**，
 * 而 iOS 的主屏 web app 与 Safari **各有独立存储**（localStorage / cookie 都不互通）。地址栏
 * 一被抹干净，主屏图标就在「URL 里没有令牌、容器里也没有存储」的空状态下启动，从此再也配不上
 * ——扫码只会打开 Safari，救不了那个图标（这是实测反馈：添加主屏后无法二次访问）。
 *
 * **所以令牌留在地址栏里**：主屏图标与书签每次启动都从 URL 拿到它，存储隔离不再相关。
 * 三种取用途径并存、互不冲突：URL（主屏 / 书签）→ localStorage（同一浏览器里跨标签页）→
 * 进程内缓存。
 *
 * **代价如实记**（决策 191）：地址栏与浏览历史里有凭据，截图即泄露。`Referer` 那条通道改由
 * 响应头 `Referrer-Policy: no-referrer` 关掉（`assets.rs`）；怀疑泄露时的出路是既有的
 * 「重置配对」——旧令牌立即失效，各设备重扫一次（**已添加到主屏幕的要重新添加一次**，图标里
 * 记的是旧地址）。
 */
export function capturePairingFromLocation(
  location: Pick<Location, 'search'> = window.location,
): string | null {
  const token = new URLSearchParams(location.search).get(PAIRING_QUERY);
  if (!token) return getPairingToken();

  setPairingToken(token);
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
