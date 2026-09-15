/**
 * 轻量 hash 路由（design §4：本地应用，无 SEO 诉求）。
 *
 * `/`          看板
 * `/talk`      对讲台（决策 174；主题六 §3.3 的稿件落地）
 * `/task/:id`  任务详情
 * `/settings/projects` · `/settings/providers` · `/metrics` · `/share`（票 22 / 决策 167）
 */

export type Route =
  | { name: 'board' }
  | { name: 'talk' }
  | { name: 'task'; id: string }
  | { name: 'settings-projects' }
  | { name: 'settings-providers' }
  | { name: 'metrics' }
  | { name: 'share' }
  | { name: 'not-found'; path: string };

export function parseRoute(hash: string): Route {
  const path = hash.replace(/^#/, '').split('?')[0] || '/';
  if (path === '/' || path === '') return { name: 'board' };
  // 对讲台（决策 174）。`v-talk` 是设计原型（theme-6-pixel.md §3.3）的视图 id，
  // 直接按原型写法手敲的地址也会来，故与正名 `/talk` 一并接受，不落 not-found。
  if (path === '/talk' || path === 'v-talk') return { name: 'talk' };
  const task = /^\/task\/([^/]+)$/.exec(path);
  if (task) return { name: 'task', id: decodeURIComponent(task[1]) };
  if (path === '/settings/projects') return { name: 'settings-projects' };
  if (path === '/settings/providers') return { name: 'settings-providers' };
  if (path === '/metrics') return { name: 'metrics' };
  if (path === '/share') return { name: 'share' };
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
