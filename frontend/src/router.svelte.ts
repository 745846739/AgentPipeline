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
 * **开屏默认是对讲台（决策 241）**：地址栏**没写 hash**（`''` / `#`）时，进 store 之前先归一成
 * `#/talk`（见 `normalizeBareHash`）。「默认落点」问的是**什么都不指定时去哪儿**，所以它只接管
 * **空地址**：显式 `#/` 照旧解析成看板——决策 240 的「看板是根路由」与 §4.2 的四枚页签一个字不动。
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
  | { name: 'settings-notify'; query: RouteQuery }
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
  // 离线通知（决策 272）：总开关 / 通道四件 / 探针。
  if (path === '/settings/notify') return { name: 'settings-notify', query };
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

  /**
   * 程序改址（用户点链接仍走 `hashchange`）。地址栏是权威，`this.hash` 只是它的镜像
   * ——但这一镜像**当场就跟上**，不等 `hashchange`：那是**另一个任务**，而页面渲染读的是
   * 镜像，等它就等于「程序导航总会晚一帧渲染」。需要「改完址、渲染完、再定位到某个元素」
   * 的地方（顶栏信号灯在外页上点灯 → 去 `#/` 再 `scrollIntoView`，决策 218 ⑥）因此拿不到
   * 靶子——实测那一跳不动，目标停在视口下方 1027px 处、整页 `scrollTop` 还是 0。
   * 赋值与 `location.hash` 同步做，两者不会说两套；随后到的 `hashchange` 写同一个值，
   * 对 `$state` 是空操作，不会多渲染一次。
   */
  navigate(path: string): void {
    if (typeof window === 'undefined') return;
    const next = path.startsWith('#') ? path : `#${path}`;
    if (window.location.hash === next) {
      this.hash = next;
      return;
    }
    this.hash = next;
    window.location.hash = next;
  }
}

/**
 * 开屏默认落点（决策 241）：地址栏还没写 hash 时，把它归一成 `#/talk`（对讲台）。
 *
 * **住模块这一层、不在 `main.ts`**：`router` store 由 `App.svelte` 的 import 链先初始化
 * （ES 导入先于 `main.ts` 的语句体执行）。等 `main.ts` 再改就晚一拍——store 已经按空 hash
 * 读出「看板」，而 `replaceState` **不发 `hashchange`**，没有人会去把这面镜像追回来，
 * 于是地址说对讲台、页面画看板。故归一必须跑在 `new RouterStore()` **之前**，让 `$state`
 * 的首读就读到归一后的值。
 *
 * 用 `replaceState` 而不是 `navigate`：开屏这一跳**不进历史**（后退应当离开本应用，而不是
 * 退回一个「还没归一」的空地址）；`?pair=` 那段 search **原样带过去**——扫码进来的令牌就挂在
 * 它上面（决策 191，抹掉它等于让主屏图标再也配不上对）。
 *
 * 只认 `''` 与 `#` 两种空写法：地址栏一旦带了任何路径（含 `#/`），就不是「什么都不指定」。
 */
function normalizeBareHash(): void {
  if (typeof window === 'undefined') return;
  if (window.location.hash !== '' && window.location.hash !== '#') return;
  window.history.replaceState(
    null,
    '',
    `${window.location.pathname}${window.location.search}#/talk`,
  );
}

normalizeBareHash();

export const router = new RouterStore();

/**
 * 当前地址的**原始** hash。地址栏是权威，`router.hash` 只是它的镜像（由 `hashchange` 跟上
 * ——那一次跟进是异步的，故读写查询串时用地址栏本身，免得读到镜像的滞后值）。
 */
function currentHash(): string {
  if (typeof window !== 'undefined' && window.location.hash) return window.location.hash;
  return router.hash;
}

/**
 * 当前地址里的查询串（**只读**）。与 `router.route` 解析的是同一份，故不存在「路由看到的」
 * 与「这里读到的」两个版本。
 */
export function readQuery(): RouteQuery {
  return splitHash(currentHash()).query;
}

/**
 * 把 patch 写回地址（决策 217③）。`null` = 删掉这个 key（缺省值不写进地址，老地址照旧）。
 *
 * **默认 `pushState`、程序自己改的用 `replaceState`**：用户点页签 / 切过滤 / 换班次 push
 * （后退回到上一个页签是想要的），而程序化联动（触发节点直达、指标页 `?task=` 自动就位、
 * 装载后把兜底值落进地址）replace——否则自动联动会把历史灌满，后退不再是「回到上一页」。
 *
 * 两个实现细节都不是可选的：
 *   ① 值没变时**不动地址**——同一次切换被两条路径写两遍会产生两条历史；
 *   ② `pushState` / `replaceState` **不发 `hashchange`**，故写完要自己把路由状态接上，
 *      否则 `router.route` 还停在旧地址上（这一条不写就是「地址变了、页面没变」）。
 */
export function writeQuery(
  patch: Record<string, string | null>,
  opts: { replace?: boolean } = {},
): void {
  if (typeof window === 'undefined') return;
  const before = currentHash();
  const { path, query } = splitHash(before);
  const next: RouteQuery = { ...query };
  for (const [key, value] of Object.entries(patch)) {
    if (value === null) delete next[key];
    else next[key] = value;
  }
  const search = new URLSearchParams(Object.entries(next)).toString();
  const hash = `#${path}${search ? `?${search}` : ''}`;
  if (hash === before) return;
  const url = `${window.location.pathname}${window.location.search}${hash}`;
  if (opts.replace) window.history.replaceState(null, '', url);
  else window.history.pushState(null, '', url);
  router.hash = hash;
}

