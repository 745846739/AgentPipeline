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
