/**
 * 轻量 hash 路由（design §4：本地应用，无 SEO 诉求）。
 *
 * `/`          看板
 * `/task/:id`  任务详情
 * `/settings/projects` · `/settings/providers` · `/metrics`（票 22）
 */

export type Route =
  | { name: 'board' }
  | { name: 'task'; id: string }
  | { name: 'settings-projects' }
  | { name: 'settings-providers' }
  | { name: 'metrics' }
  | { name: 'not-found'; path: string };

export function parseRoute(hash: string): Route {
  const path = hash.replace(/^#/, '').split('?')[0] || '/';
  if (path === '/' || path === '') return { name: 'board' };
  const task = /^\/task\/([^/]+)$/.exec(path);
  if (task) return { name: 'task', id: decodeURIComponent(task[1]) };
  if (path === '/settings/projects') return { name: 'settings-projects' };
  if (path === '/settings/providers') return { name: 'settings-providers' };
  if (path === '/metrics') return { name: 'metrics' };
  return { name: 'not-found', path };
}

class RouterStore {
  hash = $state(typeof window !== 'undefined' ? window.location.hash : '#/');

  constructor() {
    if (typeof window !== 'undefined') {
      window.addEventListener('hashchange', () => {
        this.hash = window.location.hash;
      });
    }
  }

  get route(): Route {
    return parseRoute(this.hash);
  }

  navigate(path: string): void {
    if (typeof window === 'undefined') return;
    const next = path.startsWith('#') ? path : `#${path}`;
    if (window.location.hash === next) {
      this.hash = next;
      return;
    }
    window.location.hash = next;
  }
}

export const router = new RouterStore();
