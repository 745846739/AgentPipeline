/**
 * 轻量 hash 路由（design §4：本地应用，无 SEO 诉求）。
 *
 * `/`          看板
 * `/talk`      对讲台（决策 174；主题六 §3.3 的稿件落地）
 * `/task/:id`  任务详情
 * `/settings`  设置落地页（决策 198；分类「谁能进来」/「怎么跑」）
 * `/settings/projects` · `/settings/providers` · `/settings/stages` · `/settings/market` · `/metrics` · `/share`
 * （票 22 / 决策 167 / 决策 187 / 决策 198）
 *
 * **查询串（`?k=v`）解析出 `query`，两个跨流入口的 key 名是契约的一部分**
 * （`parallel-brief.md` §二 的跨流接口表与 `design/frontend-design.md` §4.5，别改名）：
 *
 * - 任务指标入口 `#/metrics?task=<task_id>`：指标页据 `query.task` 自动载入并高亮（消费方 Me）。
 * - 项目分析入口 `#/settings/projects?project=<id>&analyze=1`：项目页据此自动就位并触发分析（消费方 S）。
 *
 * 约定：`query` 的每个值都是已解码的字符串（选项串的重复 key 取**首次出现**的值）；
 * 没有查询串时是空对象（不是 `undefined`）——调用方一律 `route.query.x ?? fallback` 读，
 * 不必先判空。
 */

/** 已解码的查询串。**只读**——路由解析的产物，调用方不得就地改。 */
export type RouteQuery = Record<string, string>;

export type Route =
  | { name: 'board'; query: RouteQuery }
  | { name: 'talk'; query: RouteQuery }
  | { name: 'task'; id: string; query: RouteQuery }
  | { name: 'settings-landing'; query: RouteQuery }
  | { name: 'settings-projects'; query: RouteQuery }
  | { name: 'settings-providers'; query: RouteQuery }
  | { name: 'settings-stages'; query: RouteQuery }
  | { name: 'settings-market'; query: RouteQuery }
  | { name: 'metrics'; query: RouteQuery }
  | { name: 'share'; query: RouteQuery }
  | { name: 'not-found'; path: string; query: RouteQuery };

/**
 * 把 `#/path?k=v&k2=v2` 拆成路径与查询串。
 *
 * `#` 只剥开头那一个（`hash` 里其它位置的 `#` 与它无关）；路径部分照旧只取 `?` 之前。
 */
function splitHash(hash: string): { path: string; query: RouteQuery } {
  const raw = hash.replace(/^#/, '');
  const q = raw.indexOf('?');
  const path = (q >= 0 ? raw.slice(0, q) : raw) || '/';
  const query: RouteQuery = {};
  if (q >= 0) {
    for (const [key, value] of new URLSearchParams(raw.slice(q + 1))) {
      // 重复 key 取首次出现的值：`?task=a&task=b` 不是「后一个覆盖前一个」，
      // 后者多半是拼接失误，取第一个更接近「用户点的那个」。
      if (!(key in query)) query[key] = value;
    }
  }
  return { path, query };
}

export function parseRoute(hash: string): Route {
  const { path, query } = splitHash(hash);
  if (path === '/' || path === '') return { name: 'board', query };
  // 对讲台（决策 174）。`v-talk` 是设计原型（theme-6-pixel.md §3.3）的视图 id，
  // 直接按原型写法手敲的地址也会来，故与正名 `/talk` 一并接受，不落 not-found。
  if (path === '/talk' || path === 'v-talk') return { name: 'talk', query };
  const task = /^\/task\/([^/]+)$/.exec(path);
  if (task) return { name: 'task', id: decodeURIComponent(task[1]), query };
  // 设置落地页（决策 198）：分类两项、每项仍是独立路由，落地页只是入口。
  // 各条都是精确匹配，故 `/settings` 不吃 `/settings/xxx` 的前缀（反之亦然）。
  if (path === '/settings') return { name: 'settings-landing', query };
  if (path === '/settings/projects') return { name: 'settings-projects', query };
  if (path === '/settings/providers') return { name: 'settings-providers', query };
  // 阶段配置（决策 198）：内容整体从「模型与密钥」页搬出的那一页。
  if (path === '/settings/stages') return { name: 'settings-stages', query };
  // 技能市场（决策 194，页骨架承自 187）：仓名单 / 该仓的技能列表 / 安装。
  // 此前只有 config.toml 一条路，界面上无处可改。
  if (path === '/settings/market') return { name: 'settings-market', query };
  if (path === '/metrics') return { name: 'metrics', query };
  if (path === '/share') return { name: 'share', query };
  return { name: 'not-found', path, query };
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
